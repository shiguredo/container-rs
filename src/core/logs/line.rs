//! 行単位のログ配信フレームを切り出すロジック。
//!
//! macOS / Linux の LogConsumer 配信タスクはこのモジュールの `read_line_limited` で
//! 1 行ずつ読み取る。行長上限を超える行は先頭を切り捨てフレームとして配信し、
//! 残余を読み捨てるため、改行を含まない巨大出力が続いても配信タスクのメモリ
//! 使用量が有界に保たれる。

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

use crate::core::logs::{LogFrame, consumer::LogConsumer};

/// 配信タスクの行長上限 (バイト)。
///
/// 改行を含まない巨大出力 (バイナリ・単一行ダンプ等) がコンテナから続いても、
/// 配信タスクの行蓄積がこの値を超えないようにする。Linux の共有バッファ上限
/// (docker_log_stream の `DEFAULT_BUFFER_LIMIT`) と同じ値に揃え、両プラットフォームの
/// 配信タスクで同じ上限を使う。上限を変える場合はこの定数と `DEFAULT_BUFFER_LIMIT` を
/// 合わせて変更する (docker_log_stream の単体テストが両者の一致を固定している)。
pub(crate) const MAX_LINE_LENGTH: usize = 8 * 1024 * 1024;

/// 1 行分の読み取り結果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LineRead {
    /// 改行終端の行。末尾の `\n` / `\r` は除去済み。行は上限以下。
    Line(Vec<u8>),
    /// 上限超過。先頭 `limit` バイトの切り捨てフレーム。
    /// 残余は次の改行まで読み捨て済み (読み捨て中に EOF に達した場合は捨て残りを
    /// 配信しない)。フレームは行の途中で切るため、末尾の `\n` / `\r` 除去は
    /// 適用しない (適用すると 8 MiB 境界の `\r` で 1 バイト欠損するため)。
    Truncated(Vec<u8>),
    /// 改行なしの最終行 (EOF)。末尾の `\r` は除去済み。行は上限以下。
    Final(Vec<u8>),
    /// EOF。読み取るデータが無い。
    Closed,
}

/// 1 行を上限付きで読み取る。
///
/// 行の長さは「改行 (`\n`) を除くバイト数」で数える。ちょうど `limit` バイトの行は
/// 超過としない。`limit` を超える行は先頭 `limit` バイトを切り捨てフレームとして
/// 返し、残余を次の改行まで読み捨てる (超過時に warn ログを出す)。
/// CRLF 行末の `\r` は長さに数えるため、CRLF 行は LF 行より 1 バイト先に
/// 切り捨て判定される。
pub(crate) async fn read_line_limited<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> std::io::Result<LineRead> {
    let mut line = Vec::new();
    // 上限超過で切り出した先頭フレーム。読み捨て完了後に `Truncated` として返す。
    let mut truncated: Option<Vec<u8>> = None;
    loop {
        let buf = reader.fill_buf().await?;
        if buf.is_empty() {
            if truncated.is_some() {
                // 読み捨て中に EOF。切り出した先頭フレームだけ返し、捨て残りは配信しない。
                let head = truncated.take().expect("truncated frame must be present");
                return Ok(LineRead::Truncated(head));
            }
            if line.is_empty() {
                return Ok(LineRead::Closed);
            }
            strip_trailing_cr(&mut line);
            return Ok(LineRead::Final(line));
        }
        if truncated.is_some() {
            // 上限超過行の残余を次の改行まで読み捨てる。
            match buf.iter().position(|&b| b == b'\n') {
                Some(pos) => {
                    reader.consume(pos + 1);
                    let head = truncated.take().expect("truncated frame must be present");
                    return Ok(LineRead::Truncated(head));
                }
                None => {
                    let len = buf.len();
                    reader.consume(len);
                }
            }
        } else {
            match buf.iter().position(|&b| b == b'\n') {
                Some(pos) => {
                    if line.len() + pos > limit {
                        // 上限超過。先頭 limit バイトを切り出し、残余を読み捨てる。
                        let head_len = limit - line.len();
                        line.extend_from_slice(&buf[..head_len]);
                        reader.consume(pos + 1);
                        tracing::warn!("log line exceeds limit; truncating to {limit} bytes");
                        return Ok(LineRead::Truncated(line));
                    }
                    line.extend_from_slice(&buf[..pos]);
                    strip_trailing_cr(&mut line);
                    reader.consume(pos + 1);
                    return Ok(LineRead::Line(line));
                }
                None => {
                    // 改行なし。行に蓄積する。上限を超え始めたら先頭 limit バイトだけ
                    // 残して読み捨てへ移行する。
                    if line.len() + buf.len() > limit {
                        let head_len = limit - line.len();
                        line.extend_from_slice(&buf[..head_len]);
                        reader.consume(head_len);
                        truncated = Some(line);
                        line = Vec::new();
                        tracing::warn!("log line exceeds limit; truncating to {limit} bytes");
                    } else {
                        line.extend_from_slice(buf);
                        let len = buf.len();
                        reader.consume(len);
                    }
                }
            }
        }
    }
}

/// 行末の `\r` を除去する。CRLF 行末の `\r` は配信フレームから除く。
fn strip_trailing_cr(line: &mut Vec<u8>) {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
}

/// 1 行を読み取り、フレーム化して全 consumer に配信する。
///
/// macOS / Linux の LogConsumer 配信タスクが共用する行配信の共通部分。
/// 戻り値は「ループ継続」の判定:
/// - `Ok(true)`: 配信した (読み続ける)
/// - `Ok(false)`: EOF (`Closed`)。終了判定は呼び出し側の責任
///   (macOS は exit code 観測 + DRAIN_GRACE 猶予、Linux は即 break)
/// - `Err(())`: 読み取り失敗 (配信を止める。warn はここで出す)
pub(crate) async fn deliver_line_to_consumers<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    consumers: &[Box<dyn LogConsumer + 'static>],
    to_frame: fn(Vec<u8>) -> LogFrame,
) -> std::result::Result<bool, ()> {
    match read_line_limited(reader, MAX_LINE_LENGTH).await {
        Ok(LineRead::Closed) => Ok(false),
        Ok(LineRead::Line(line) | LineRead::Final(line) | LineRead::Truncated(line)) => {
            let frame = to_frame(line);
            for consumer in consumers {
                consumer.accept(&frame).await;
            }
            Ok(true)
        }
        Err(e) => {
            tracing::warn!("log consumer read failed; stopping delivery: {e}");
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 入力バイト列を小さな容量の `BufReader` に包み、`read_line_limited` で
    /// 読み切る。残りが無くなったら `Closed` が返ることを確認する。
    async fn read_all(input: &[u8], limit: usize) -> Vec<LineRead> {
        let mut reader = tokio::io::BufReader::with_capacity(4, input);
        let mut results = Vec::new();
        loop {
            match read_line_limited(&mut reader, limit).await {
                Ok(LineRead::Closed) => break,
                Ok(read) => results.push(read),
                Err(e) => panic!("読み取りに失敗した: {e}"),
            }
        }
        results
    }

    #[tokio::test]
    async fn line_reads_lf_terminated_lines() {
        let results = read_all(b"alpha\nbeta\ngamma\n", 8).await;
        assert_eq!(
            results,
            vec![
                LineRead::Line(b"alpha".to_vec()),
                LineRead::Line(b"beta".to_vec()),
                LineRead::Line(b"gamma".to_vec()),
            ]
        );
    }

    #[tokio::test]
    async fn line_strips_cr_from_crlf_line_endings() {
        let results = read_all(b"alpha\r\nbeta\r\n", 8).await;
        assert_eq!(
            results,
            vec![
                LineRead::Line(b"alpha".to_vec()),
                LineRead::Line(b"beta".to_vec()),
            ]
        );
    }

    #[tokio::test]
    async fn line_reads_final_line_without_newline() {
        let results = read_all(b"alpha\nbeta", 8).await;
        assert_eq!(
            results,
            vec![
                LineRead::Line(b"alpha".to_vec()),
                LineRead::Final(b"beta".to_vec()),
            ]
        );
    }

    #[tokio::test]
    async fn line_returns_closed_on_empty_input() {
        let results = read_all(b"", 8).await;
        assert_eq!(results, vec![]);
    }

    #[tokio::test]
    async fn line_keeps_exact_limit_line() {
        let line = vec![b'a'; 8];
        let results = read_all(&[line.as_slice(), b"\n".as_slice()].concat(), 8).await;
        assert_eq!(results, vec![LineRead::Line(line)]);
    }

    #[tokio::test]
    async fn line_keeps_exact_max_line_length_in_bytes() {
        // 実値の 8 MiB ちょうどの行は切り捨てない (定数と境界ロジックの乖離検出用)。
        let line = vec![b'a'; MAX_LINE_LENGTH];
        let results = read_all(
            &[line.as_slice(), b"\n".as_slice()].concat(),
            MAX_LINE_LENGTH,
        )
        .await;
        assert_eq!(results, vec![LineRead::Line(line)]);
    }

    #[tokio::test]
    async fn line_truncates_over_limit_line() {
        let line = vec![b'a'; 9];
        let results = read_all(&[line.as_slice(), b"\n".as_slice()].concat(), 8).await;
        assert_eq!(results, vec![LineRead::Truncated(vec![b'a'; 8])]);
    }

    #[tokio::test]
    async fn line_truncates_line_spanning_chunks() {
        // BufReader の容量 (4 バイト) をまたいで送られる巨大行。
        let line = vec![b'a'; 17];
        let results = read_all(&[line.as_slice(), b"\n".as_slice()].concat(), 8).await;
        assert_eq!(results, vec![LineRead::Truncated(vec![b'a'; 8])]);
    }

    #[tokio::test]
    async fn line_reads_next_line_after_truncation() {
        let over = vec![b'a'; 9];
        let input = [over.as_slice(), b"\nrest\n".as_slice()].concat();
        let results = read_all(&input, 8).await;
        assert_eq!(
            results,
            vec![
                LineRead::Truncated(vec![b'a'; 8]),
                LineRead::Line(b"rest".to_vec()),
            ]
        );
    }

    #[tokio::test]
    async fn line_returns_truncated_frame_on_eof_during_discard() {
        // 上限超過行の残余が改行なしで EOF に達する。切り出した先頭フレームだけ
        // 返し、捨て残り (残余) は配信しない。
        let over = vec![b'a'; 12];
        let results = read_all(&over, 8).await;
        assert_eq!(results, vec![LineRead::Truncated(vec![b'a'; 8])]);
    }

    #[tokio::test]
    async fn line_keeps_trailing_cr_in_truncated_frame() {
        // 行途中の \r は切り捨てフレームに残る (行末除去は行の終わりにのみ適用)。
        // 入力は「a × 7 + \r + b + \n」で、切り出される先頭 8 バイトの末尾が \r になる。
        let results = read_all(b"aaaaaaa\rb\n", 8).await;
        assert_eq!(results, vec![LineRead::Truncated(b"aaaaaaa\r".to_vec())]);
    }

    #[tokio::test]
    async fn line_reads_empty_lines() {
        let results = read_all(b"\n\n", 8).await;
        assert_eq!(
            results,
            vec![LineRead::Line(Vec::new()), LineRead::Line(Vec::new())]
        );
    }

    #[tokio::test]
    async fn line_keeps_exact_limit_final_line() {
        // 改行なし最終行がちょうど limit バイトでも切り捨てない。
        let line = vec![b'a'; 8];
        let results = read_all(&line, 8).await;
        assert_eq!(results, vec![LineRead::Final(line)]);
    }

    #[tokio::test]
    async fn line_reads_final_line_after_truncation() {
        let over = vec![b'a'; 9];
        let input = [over.as_slice(), b"\ntail".as_slice()].concat();
        let results = read_all(&input, 8).await;
        assert_eq!(
            results,
            vec![
                LineRead::Truncated(vec![b'a'; 8]),
                LineRead::Final(b"tail".to_vec()),
            ]
        );
    }

    #[tokio::test]
    async fn line_counts_cr_at_limit_boundary_as_content() {
        // CRLF 行末の \r は長さに数える。a × 7 + \r でちょうど limit の行は
        // 切り捨てず、末尾 \r を除去して配信する。
        let results = read_all(b"aaaaaaa\r\n", 8).await;
        assert_eq!(results, vec![LineRead::Line(b"aaaaaaa".to_vec())]);
    }

    #[tokio::test]
    async fn line_truncates_when_limit_not_aligned_to_chunk() {
        // limit がチャンクサイズ (4 バイト) の倍数でない場合、改行なし蓄積の
        // 途中で切り出しが始まる経路を検証する。
        let results = read_all(b"aaaa\n", 3).await;
        assert_eq!(results, vec![LineRead::Truncated(b"aaa".to_vec())]);
    }

    #[tokio::test]
    async fn line_strips_trailing_cr_on_final_line() {
        let results = read_all(b"alpha\r", 8).await;
        assert_eq!(results, vec![LineRead::Final(b"alpha".to_vec())]);
    }
}
