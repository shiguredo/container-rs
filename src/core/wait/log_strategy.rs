//! ログメッセージ待機戦略。

use std::{collections::VecDeque, time::Duration};

use tokio::io::AsyncReadExt;

use crate::{
    ContainerAsync, Image,
    core::{
        client::Client,
        error::{Result, WaitContainerError, WaitLogError},
        logs::LogSource,
    },
};

/// EOF 到達時の再ポーリング間隔。ログファイルは追記型なので EOF は「終端」ではない。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// プロセス終了を検出した後も、ログのフラッシュ遅延を考慮して読み続ける猶予。
///
/// LogConsumer 配信タスク (macOS) の自然終了後のドレイン猶予と共有する。
/// `core::wait` が `core::containers` に依存している関係上、ここから macOS 側へ
/// 定数を参照する逆依存が生じるが、「コンテナ終了後のドレイン猶予」という同一概念を
/// 2 つの実装が共有する意図による。用途ごとに猶予を独立に調整する場合は分離すること。
pub(crate) const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// `EndOfStream` 診断用に保持するログの上限バイト数。
///
/// 診断には直近ログがあれば足りる一方、既定の startup_timeout (60 秒) 間の
/// 最悪メモリを抑えるために 1 MiB とする。
const MAX_COLLECTED_LOG_BYTES: usize = 1024 * 1024;

/// 診断用ログのリングバッファ。
///
/// チャンク列ではなく連続バイトで保持し、短い read によるメタデータ爆発を防ぐ。
/// 上限超過時は先頭から捨て、常に直近 `MAX_COLLECTED_LOG_BYTES` バイト以下を保つ。
pub(crate) struct CollectedLogs {
    buf: VecDeque<u8>,
}

impl CollectedLogs {
    pub(crate) fn new() -> Self {
        Self {
            buf: VecDeque::new(),
        }
    }

    /// バイト列を末尾に追加し、上限を超えた分は先頭から捨てる。
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // 単発入力が上限超過なら末尾 MAX だけ残す。
        let keep = if bytes.len() > MAX_COLLECTED_LOG_BYTES {
            &bytes[bytes.len() - MAX_COLLECTED_LOG_BYTES..]
        } else {
            bytes
        };
        self.buf.extend(keep.iter().copied());
        while self.buf.len() > MAX_COLLECTED_LOG_BYTES {
            self.buf.pop_front();
        }
    }

    /// `EndOfStream` 用に連結済みチャンク列へ変換する (要素は 0 または 1)。
    pub(crate) fn into_chunks(self) -> Vec<Vec<u8>> {
        if self.buf.is_empty() {
            Vec::new()
        } else {
            vec![self.buf.into_iter().collect()]
        }
    }
}

/// ログメッセージ待機戦略。
#[derive(Debug, Clone)]
pub struct LogWaitStrategy {
    pub(crate) source: LogSource,
    pub(crate) message: Vec<u8>,
    pub(crate) times: usize,
}

impl LogWaitStrategy {
    /// stdout に指定メッセージが出るまで待つ戦略を作る。
    pub fn stdout(message: impl AsRef<[u8]>) -> Self {
        Self::new(LogSource::StdOut, message)
    }

    /// stderr に指定メッセージが出るまで待つ戦略を作る。
    pub fn stderr(message: impl AsRef<[u8]>) -> Self {
        Self::new(LogSource::StdErr, message)
    }

    /// stdout / stderr のどちらかにメッセージが出るまで待つ戦略を作る。
    pub fn stdout_or_stderr(message: impl AsRef<[u8]>) -> Self {
        Self::new(LogSource::BothStd, message)
    }

    /// ログ出力元とメッセージを指定して新しい戦略を作る。
    pub fn new(source: LogSource, message: impl AsRef<[u8]>) -> Self {
        Self {
            source,
            message: message.as_ref().to_vec(),
            times: 1,
        }
    }

    /// メッセージの出現回数閾値を設定する。
    ///
    /// 0 を指定した場合は 1 にクランプされる (0 では読み取り結果を無視して即 ready になるため)。
    pub fn with_times(mut self, times: usize) -> Self {
        // 0 を指定すると total >= 0 が即 true になり読み取り結果を無視して即 ready になるため、
        // 最低 1 を保証する。
        self.times = times.max(1);
        self
    }
}

impl LogWaitStrategy {
    pub(crate) async fn wait_until_ready<I: Image>(
        self,
        _client: &Client,
        container: &ContainerAsync<I>,
    ) -> Result<()> {
        // 空メッセージは常にマッチ扱い (windows(0) の panic を避ける)。
        if self.message.is_empty() {
            return Ok(());
        }

        // BothStd は stdout / stderr の両ストリームを並行に照合し、
        // 出現回数は両ストリームの合算で判定する (本家のマージ照合と同等)。
        let mut readers = match self.source {
            LogSource::StdOut => vec![container.stdout(true)],
            LogSource::StdErr => vec![container.stderr(true)],
            LogSource::BothStd => vec![container.stdout(true), container.stderr(true)],
        };

        // 行単位ではなくバイトストリームで照合する。
        // 行単位だと改行を含むパターンが永遠に不成立になり、非 UTF-8 のログ行で
        // 待機全体が Io エラーになってしまう。
        let mut matchers: Vec<StreamMatcher> = readers
            .iter()
            .map(|_| StreamMatcher::new(self.message.clone()))
            .collect();
        let mut chunk = vec![0u8; 8192];
        let mut exited_at: Option<tokio::time::Instant> = None;
        let mut collected = CollectedLogs::new();

        loop {
            let mut progressed = false;
            let mut total = 0usize;
            for (reader, matcher) in readers.iter_mut().zip(matchers.iter_mut()) {
                // 各リーダーをタイムアウト付きで読む。Notify ベースのリーダー (Linux) は
                // 無音ストリームで永久に Pending になるため、タイムアウトを「今回はデータ無し」
                // (n=0) として扱い次のリーダーへ進む。これにより `BothStd` で片方のストリームが
                // 無音でも他方の照合が遅延しない。データが流れている場合は通知がタイムアウトより
                // 先に read を起床させるため、検出遅延は発生しない。
                let n = match tokio::time::timeout(POLL_INTERVAL, reader.read(&mut chunk)).await {
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => return Err(WaitLogError::Io(e).into()),
                    Err(_elapsed) => 0,
                };
                let count = if n > 0 {
                    progressed = true;
                    collected.push(&chunk[..n]);
                    matcher.feed(&chunk[..n])
                } else {
                    matcher.feed(&[])
                };
                total = total.saturating_add(count);
            }
            if total >= self.times {
                return Ok(());
            }
            if progressed {
                continue;
            }

            // 全ストリーム EOF。ログは追記されうるので待って読み直す。
            // プロセス終了後は DRAIN_GRACE だけ追加で読み、それでも現れなければ打ち切る。
            // 全体の時間制限は呼び出し側の startup_timeout が担う。
            // macOS / Linux とも exit_code_hint 経路で EOF 判定する。
            // Linux は加えて logs_terminated (demux 終端) 経路でも判定する。
            if container.exit_code_hint().is_some() || container.logs_terminated() {
                let at = exited_at.get_or_insert_with(tokio::time::Instant::now);
                if at.elapsed() >= DRAIN_GRACE {
                    return Err(WaitContainerError::WaitLog(WaitLogError::EndOfStream(
                        collected.into_chunks(),
                    ))
                    .into());
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// バイトストリームを跨いでパターンの出現回数を数える逐次マッチャ。
///
/// チャンク境界をまたぐマッチを検出するため、直前チャンクの末尾
/// (パターン長 - 1) バイトを持ち越して次のチャンクと連結して照合する。
/// 持ち越し部分だけでは完全なマッチが成立しない (パターン長未満) ため、二重カウントしない。
pub(crate) struct StreamMatcher {
    pattern: Vec<u8>,
    carry: Vec<u8>,
    count: usize,
}

impl StreamMatcher {
    pub(crate) fn new(pattern: Vec<u8>) -> Self {
        Self {
            pattern,
            carry: Vec::new(),
            count: 0,
        }
    }

    /// チャンクを与え、これまでの累計出現回数を返す。空パターンは常にマッチ扱い。
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> usize {
        if self.pattern.is_empty() {
            return usize::MAX;
        }
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(chunk);
        if buf.len() >= self.pattern.len() {
            self.count += buf
                .windows(self.pattern.len())
                .filter(|w| *w == self.pattern.as_slice())
                .count();
            let keep = self.pattern.len() - 1;
            self.carry = buf[buf.len() - keep.min(buf.len())..].to_vec();
        } else {
            self.carry = buf;
        }
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collected_logs_trims_front_when_over_limit() {
        // 複数回 push 後に先頭が捨てられ、合計が上限以下・末尾が保持されること。
        let mut logs = CollectedLogs::new();
        let first = vec![b'a'; MAX_COLLECTED_LOG_BYTES];
        let second = b"TAIL";
        logs.push(&first);
        logs.push(second);
        let chunks = logs.into_chunks();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), MAX_COLLECTED_LOG_BYTES);
        assert!(chunks[0].ends_with(second));
    }

    #[test]
    fn collected_logs_keeps_all_when_exactly_at_limit() {
        // 合計がちょうど上限のときは破棄しないこと。
        let mut logs = CollectedLogs::new();
        let exact = vec![b'x'; MAX_COLLECTED_LOG_BYTES];
        logs.push(&exact);
        let chunks = logs.into_chunks();
        assert_eq!(chunks, vec![exact]);
    }

    #[test]
    fn collected_logs_keeps_tail_of_oversized_single_push() {
        // 1 回の push が上限超過のとき末尾 MAX だけ残ること。
        let mut logs = CollectedLogs::new();
        let mut oversized = vec![b'0'; MAX_COLLECTED_LOG_BYTES];
        oversized.extend_from_slice(b"END!");
        logs.push(&oversized);
        let chunks = logs.into_chunks();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), MAX_COLLECTED_LOG_BYTES);
        assert!(chunks[0].ends_with(b"END!"));
        assert_eq!(
            chunks[0],
            oversized[oversized.len() - MAX_COLLECTED_LOG_BYTES..]
        );
    }

    #[test]
    fn collected_logs_survives_many_one_byte_pushes() {
        // 1 バイトずつ多数回 push しても保持は上限以下で末尾側だけ残ること。
        let mut logs = CollectedLogs::new();
        let total = MAX_COLLECTED_LOG_BYTES + 100;
        for i in 0..total {
            logs.push(&[((i % 256) as u8)]);
        }
        let chunks = logs.into_chunks();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), MAX_COLLECTED_LOG_BYTES);
        let start = total - MAX_COLLECTED_LOG_BYTES;
        let expected: Vec<u8> = (start..total).map(|i| (i % 256) as u8).collect();
        assert_eq!(chunks[0], expected);
    }

    #[test]
    fn collected_logs_into_chunks_is_empty_or_single() {
        // into_chunks は空または要素 1 個であること。
        assert!(CollectedLogs::new().into_chunks().is_empty());
        let mut logs = CollectedLogs::new();
        logs.push(b"abc");
        logs.push(b"def");
        let chunks = logs.into_chunks();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], b"abcdef");
    }

    #[test]
    fn stream_matcher_empty_pattern_always_matches() {
        // 空パターンは panic せず常にマッチ扱いになること。
        let mut m = StreamMatcher::new(Vec::new());
        assert_eq!(m.feed(b"anything"), usize::MAX);
    }

    /// 連結入力での出現回数を数える (StreamMatcher と同じ windows 照合)。
    fn count_occurrences(pattern: &[u8], data: &[u8]) -> usize {
        if pattern.is_empty() {
            return usize::MAX;
        }
        data.windows(pattern.len())
            .filter(|w| *w == pattern)
            .count()
    }

    #[test]
    fn stream_matcher_chunking_preserves_count_across_boundaries() {
        // チャンク境界をまたいでも出現回数が連結入力と一致すること。
        let pattern = b"ab";
        let data = b"xxababxxab";
        let expected = count_occurrences(pattern, data);
        let mut matcher = StreamMatcher::new(pattern.to_vec());
        let mut actual = 0;
        for chunk in [b"xxa".as_slice(), b"bab".as_slice(), b"xxab".as_slice()] {
            actual = matcher.feed(chunk);
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn stream_matcher_chunking_handles_split_pattern() {
        // パターンがチャンク境界で分割されてもマッチすること。
        let mut matcher = StreamMatcher::new(b"ready".to_vec());
        assert_eq!(matcher.feed(b"re"), 0);
        assert_eq!(matcher.feed(b"ady"), 1);
    }

    #[test]
    fn with_times_zero_is_clamped_to_one() {
        // 0 を指定しても 1 として扱われること (即 ready 化の防止)。
        let strategy = LogWaitStrategy::stdout("ready").with_times(0);
        assert_eq!(strategy.times, 1, "with_times(0) は 1 にクランプされること");
    }

    #[test]
    fn with_times_positive_value_is_preserved() {
        // 正の値はそのまま保持されること。
        let strategy = LogWaitStrategy::stdout("ready").with_times(3);
        assert_eq!(strategy.times, 3, "正の値は変更されないこと");
    }
}
