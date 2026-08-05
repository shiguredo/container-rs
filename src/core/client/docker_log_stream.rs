//! Linux (Docker Engine API) のログストリーム。
//!
//! `GET /containers/{id}/logs` を `spawn_blocking` 内で `UnixStream` により叩き、
//! multiplex フレームを demux して stdout / stderr 別の共有バッファへ書き込む。
//! 呼び出し側 (async / sync のリーダー、LogConsumer 配信タスク) は同じ共有バッファを
//! 独立オフセットで読むため、1 本の HTTP ストリームを複数読取者が共有できる。
//!
//! キャンセルは (a) 読取側が mpsc / リーダーを drop する経路と (b) `DockerLogsHandle::stop`
//! が `UnixStream::shutdown(Shutdown::Both)` を明示発行する経路の 2 本で行う。
//! demux タスクは `spawn_blocking` 内で `UnixStream` を所有し続けるため、外部から同じ
//! ソケットを閉じられるよう `try_clone()` した複製をハンドルに保持する。
//! `try_clone` 失敗時は複製が無いため、`log_stop` フラグと後続の remove
//! (デーモン側が接続を閉じる) による停止に頼る。

use std::collections::VecDeque;
use std::future::Future;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use shiguredo_http11::{BodyProgress, ResponseDecoder};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, ReadBuf};

use crate::core::logs::{LogFrame, consumer::LogConsumer};

/// 共有バッファの既定上限 (ストリームあたり)。
///
/// 上限到達時は先頭から捨てる (drop-oldest)。将来の外部設定化は本実装の範囲外。
const DEFAULT_BUFFER_LIMIT: usize = 8 * 1024 * 1024;

/// multiplex フレームのヘッダ長 (バイト)。
const FRAME_HEADER_LEN: usize = 8;

/// 1 回の poll / read で共有バッファから引き出す上限バイト数。
const READ_CHUNK: usize = 8192;

/// ログセッションの起動 (接続・リクエスト送信・ヘッダ検証) に適用するタイムアウト。
/// デーモン無応答時の無限ブロックを抑止する。起動成功後は解除する。
const LOG_SESSION_TIMEOUT: Option<Duration> = Some(Duration::from_secs(30));

/// `std::io::Error::other` の短縮形。内部エラーを Read エラーとして上位に包む。
fn io_other(msg: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> std::io::Error {
    std::io::Error::other(msg)
}

/// ログストリーム用のレスポンスデコーダを作る。
///
/// ログは非有界ストリームのため `max_body_size` を無制限にする (既定の 10 MiB のままだと、
/// 合計ログが 10 MiB を超えた時点でデコーダが `BodyTooLarge` でストリームを強制終了する)。
/// 一方 `max_buffer_size` は既定 (64 KiB) のまま据え置く。drain が常にボディを消費し続ける
/// ためデコーダの内部バッファは 64 KiB 上限に収まる。なおログ全体のメモリ上限は共有バッファ
/// 側 (ストリームあたり 8 MiB・drop-oldest) が別途担う。
fn log_stream_decoder() -> ResponseDecoder {
    ResponseDecoder::with_limits(shiguredo_http11::DecoderLimits {
        max_body_size: u64::MAX,
        ..shiguredo_http11::DecoderLimits::default()
    })
}

/// stdout / stderr 片方の共有バッファと追記・終端通知。
///
/// async 文脈で保持したまま await しない運用を守るため、バッファは `std::sync::Mutex` で
/// 保持し、ロック取得中はコピーとヘッド位置更新だけを行って即 drop する。
pub(crate) struct LogStream {
    buffer: std::sync::Mutex<SharedLogBuffer>,
    /// 追記・終端を通知する。demux タスクが `notify_waiters` を呼ぶ。
    notify: tokio::sync::Notify,
    /// ストリーム終端フラグ。demux タスクが TCP EOF / 停止検出時に立てる。
    terminated: AtomicBool,
}

impl LogStream {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            buffer: std::sync::Mutex::new(SharedLogBuffer::new(limit)),
            notify: tokio::sync::Notify::new(),
            terminated: AtomicBool::new(false),
        })
    }

    /// ストリームを終端にし、待機中のリーダーを起こす。
    fn terminate(&self) {
        self.terminated.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

/// 先頭絶対オフセット付きのリングバッファ。上限超過時は先頭から捨てる。
///
/// 各リーダーは「自分が次に読むべき絶対オフセット」を保持し、バッファ先頭より前を
/// 読もうとした場合は先頭 drop が発生したと判定してスキップする。
///
/// `strict` モード (1-shot 経路) では drop-oldest せず、上限超過を検知した時点で
/// エラーを返す (切り詰めるとログ取得の「決定的に全ログを返す」契約が壊れるため)。
pub(crate) struct SharedLogBuffer {
    buf: VecDeque<u8>,
    /// バッファ先頭の絶対オフセット。先頭 drop のたびに進む。
    head: u64,
    /// ストリームあたりの上限バイト数。
    limit: usize,
    /// true なら上限超過時にエラーを返す (1-shot 経路専用)。
    strict: bool,
}

impl SharedLogBuffer {
    /// `strict = false` のバッファを作る (follow 経路・drop-oldest)。
    fn new(limit: usize) -> Self {
        Self::with_mode(limit, false)
    }

    /// 上限超過時に drop-oldest せずエラーを返す strict モードで作る (1-shot 経路)。
    fn new_strict(limit: usize) -> Self {
        Self::with_mode(limit, true)
    }

    fn with_mode(limit: usize, strict: bool) -> Self {
        Self {
            buf: VecDeque::new(),
            head: 0,
            limit,
            strict,
        }
    }

    /// バイト列を末尾に追加する。
    ///
    /// `strict` モードで上限を超えた場合は `false` を返し、追記は行わない
    /// (呼び出し側が即時エラーにできるよう、検知した時点で止める)。
    fn append(&mut self, data: &[u8]) -> bool {
        if data.is_empty() {
            return true;
        }
        if self.strict {
            // 単発入力が上限超過なら false (このデータは捨てる。呼び出し側がエラーにする)。
            if data.len() > self.limit || self.buf.len() + data.len() > self.limit {
                return false;
            }
            self.buf.extend(data.iter().copied());
            return true;
        }
        // 単発入力が上限超過なら末尾 limit だけ残す。
        let keep = if data.len() > self.limit {
            &data[data.len() - self.limit..]
        } else {
            data
        };
        self.buf.extend(keep.iter().copied());
        while self.buf.len() > self.limit {
            self.buf.pop_front();
            self.head += 1;
        }
        true
    }

    /// `reader_offset` から読めるだけ `buf` にコピーする。
    ///
    /// 返り値は `(コピーしたバイト数, 次のオフセット, スキップ発生フラグ)`。
    /// `reader_offset` がバッファ先頭より前なら先頭 drop 済みと判定し、先頭までスキップして
    /// スキップフラグを true にする。
    fn read_at(&self, reader_offset: u64, buf: &mut [u8]) -> (usize, u64, bool) {
        let tail = self.head + self.buf.len() as u64;
        let (mut offset, skipped) = if reader_offset < self.head {
            (self.head, true)
        } else {
            (reader_offset, false)
        };
        if offset >= tail || buf.is_empty() {
            return (0, offset, skipped);
        }
        let start = (offset - self.head) as usize;
        let n = (self.buf.len() - start).min(buf.len());
        for (i, &b) in self.buf.range(start..start + n).enumerate() {
            buf[i] = b;
        }
        offset += n as u64;
        (n, offset, skipped)
    }
}

/// multiplex フレームの demux 状態機械。
///
/// ヘッダ 8 バイト (`stream_type (1B) + reserved (3B) + payload_len (4B big-endian)`) を
/// 読み、`payload_len` バイトの payload をストリーム種別に応じて共有バッファへ直接追記する。
/// payload を 1 フレーム分まとめて保持せず、到着したバイトをそのままバッファへ流すため、
/// 巨大フレームでもメモリ消費は実際の入力サイズに比例する (OOM 回避)。
struct FrameDemuxer {
    /// ヘッダ読取中か payload 読取中か。
    in_payload: bool,
    header: [u8; FRAME_HEADER_LEN],
    header_pos: usize,
    stream_type: u8,
    payload_remaining: usize,
}

impl FrameDemuxer {
    fn new() -> Self {
        Self {
            in_payload: false,
            header: [0; FRAME_HEADER_LEN],
            header_pos: 0,
            stream_type: 0,
            payload_remaining: 0,
        }
    }

    fn reset_header(&mut self) {
        self.in_payload = false;
        self.header_pos = 0;
        self.stream_type = 0;
        self.payload_remaining = 0;
    }

    /// 到着バイトを demux し、stdout / stderr バッファへ追記する。
    ///
    /// 返り値は `Result<(stdout へ追記したか, stderr へ追記したか)>`。通知の要否判定に使う。
    /// strict モードのバッファ (1-shot 経路) が上限超過を検知した場合はエラーを返す
    /// (呼び出し側が即時アボートする)。
    fn feed(
        &mut self,
        mut data: &[u8],
        stdout: &mut SharedLogBuffer,
        stderr: &mut SharedLogBuffer,
    ) -> std::io::Result<(bool, bool)> {
        let mut wrote_out = false;
        let mut wrote_err = false;
        while !data.is_empty() {
            if !self.in_payload {
                let need = FRAME_HEADER_LEN - self.header_pos;
                let take = need.min(data.len());
                self.header[self.header_pos..self.header_pos + take].copy_from_slice(&data[..take]);
                self.header_pos += take;
                data = &data[take..];
                if self.header_pos == FRAME_HEADER_LEN {
                    self.stream_type = self.header[0];
                    self.payload_remaining = u32::from_be_bytes([
                        self.header[4],
                        self.header[5],
                        self.header[6],
                        self.header[7],
                    ]) as usize;
                    self.in_payload = true;
                    if self.payload_remaining == 0 {
                        self.reset_header();
                    }
                }
            } else {
                let take = self.payload_remaining.min(data.len());
                let chunk = &data[..take];
                match self.stream_type {
                    // 1 = stdout, 2 = stderr。0 (stdin) と未知の種別は捨てる。
                    1 => {
                        if !stdout.append(chunk) {
                            return Err(io_other(format!(
                                "stdout output exceeds {} bytes limit",
                                stdout.limit
                            )));
                        }
                        wrote_out |= !chunk.is_empty();
                    }
                    2 => {
                        if !stderr.append(chunk) {
                            return Err(io_other(format!(
                                "stderr output exceeds {} bytes limit",
                                stderr.limit
                            )));
                        }
                        wrote_err |= !chunk.is_empty();
                    }
                    _ => {}
                }
                self.payload_remaining -= take;
                data = &data[take..];
                if self.payload_remaining == 0 {
                    self.reset_header();
                }
            }
        }
        Ok((wrote_out, wrote_err))
    }
}

/// ログストリーム全体のハンドル。`ContainerAsync` が保持し、Drop / stop / rm で閉じる。
pub(crate) struct DockerLogsHandle {
    /// 1-shot 取得 (`follow=false`) が新規セッションを張るためのソケットパス。
    socket_path: String,
    /// 1-shot 取得が使うコンテナ ID。
    id: String,
    stdout: Arc<LogStream>,
    stderr: Arc<LogStream>,
    /// demux タスクへの停止指示。
    log_stop: Arc<AtomicBool>,
    /// 外部から `shutdown(Shutdown::Both)` を発行するための複製ソケット。
    shutdown_socket: std::sync::Mutex<Option<UnixStream>>,
    /// demux タスク完了フラグ。
    demux_done: Arc<AtomicBool>,
    /// 生存中の consumer 配信タスク数。`all_done` はこの値が 0 かどうかで判定する。
    active_consumers: Arc<AtomicUsize>,
}

impl DockerLogsHandle {
    fn new(socket_path: String, id: String) -> Arc<Self> {
        Arc::new(Self {
            socket_path,
            id,
            stdout: LogStream::new(DEFAULT_BUFFER_LIMIT),
            stderr: LogStream::new(DEFAULT_BUFFER_LIMIT),
            log_stop: Arc::new(AtomicBool::new(false)),
            shutdown_socket: std::sync::Mutex::new(None),
            demux_done: Arc::new(AtomicBool::new(false)),
            active_consumers: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// demux / consumer タスクを停止する。`log_stop` を立て、ソケットを `shutdown` する。
    ///
    /// 通常は `try_clone` した複製ソケットを `shutdown` する。`try_clone` に失敗して複製が
    /// 無い場合は `log_stop` フラグと後続の remove (デーモン側が接続を閉じる) による停止に頼る。
    pub(crate) fn stop(&self) {
        self.log_stop.store(true, Ordering::SeqCst);
        // 複製ソケットがあればそれを shutdown して読み込み中の demux を起こす。
        let socket = self
            .shutdown_socket
            .lock()
            .expect("shutdown socket mutex must not be poisoned while stopping log stream")
            .take();
        if let Some(socket) = socket {
            // 失敗しても log_stop と後続の remove (デーモン側が接続を閉じる) で止まる。
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }

    /// 両ストリームを終端にし、完了フラグを立てる。demux タスク終了時に呼ぶ。
    fn terminate_all(&self) {
        self.stdout.terminate();
        self.stderr.terminate();
        self.demux_done.store(true, Ordering::SeqCst);
    }

    /// demux ストリームが終端したか。`LogWaitStrategy` の EOF 判定に使う。
    pub(crate) fn logs_terminated(&self) -> bool {
        self.demux_done.load(Ordering::SeqCst)
    }

    /// stdout の独立オフセットリーダー (follow=true 相当) を返す。
    pub(crate) fn stdout_reader(&self) -> LogReader {
        LogReader::new(self.stdout.clone())
    }

    /// stdout の共有ストリームを返す。LogConsumer 配信タスクの spawn に使う。
    pub(crate) fn stdout_stream(&self) -> Arc<LogStream> {
        self.stdout.clone()
    }

    /// stderr の共有ストリームを返す。LogConsumer 配信タスクの spawn に使う。
    pub(crate) fn stderr_stream(&self) -> Arc<LogStream> {
        self.stderr.clone()
    }

    /// stderr の独立オフセットリーダー (follow=true 相当) を返す。
    pub(crate) fn stderr_reader(&self) -> LogReader {
        LogReader::new(self.stderr.clone())
    }

    /// stdout の同期リーダー (follow=true 相当) を返す。
    pub(crate) fn stdout_sync_reader(&self) -> SyncLogReader {
        SyncLogReader::new(self.stdout.clone())
    }

    /// stderr の同期リーダー (follow=true 相当) を返す。
    pub(crate) fn stderr_sync_reader(&self) -> SyncLogReader {
        SyncLogReader::new(self.stderr.clone())
    }

    /// stdout の 1-shot 非同期リーダー (`follow=false` 相当) を返す。
    pub(crate) fn stdout_oneshot(&self) -> OneshotReader {
        OneshotReader::new(self.socket_path.clone(), self.id.clone(), true)
    }

    /// stderr の 1-shot 非同期リーダー (`follow=false` 相当) を返す。
    pub(crate) fn stderr_oneshot(&self) -> OneshotReader {
        OneshotReader::new(self.socket_path.clone(), self.id.clone(), false)
    }

    /// stdout の 1-shot 同期リーダー (`follow=false` 相当) を返す。
    pub(crate) fn stdout_sync_oneshot(&self) -> SyncOneshotReader {
        SyncOneshotReader::new(self.socket_path.clone(), self.id.clone(), true)
    }

    /// stderr の 1-shot 同期リーダー (`follow=false` 相当) を返す。
    pub(crate) fn stderr_sync_oneshot(&self) -> SyncOneshotReader {
        SyncOneshotReader::new(self.socket_path.clone(), self.id.clone(), false)
    }

    /// consumer 配信タスクを 1 本登録する。spawn 直前に呼んで生存数を増やす。
    fn register_consumer(&self) {
        self.active_consumers.fetch_add(1, Ordering::SeqCst);
    }

    /// consumer 配信タスク終了時に呼んで生存数を減らす。
    fn consumer_finished(&self) {
        self.active_consumers.fetch_sub(1, Ordering::SeqCst);
    }

    /// demux と全 consumer が完了したか。Drop の完了待機ポーリングに使う。
    ///
    /// 冗長な完了ブーリアンを持たず、生存 consumer 数が 0 かどうかを直接判定する
    /// (register / finished の競合で完了扱いが前後するのを防ぐ)。
    pub(crate) fn all_done(&self) -> bool {
        self.demux_done.load(Ordering::SeqCst) && self.active_consumers.load(Ordering::SeqCst) == 0
    }
}

/// `?follow=true` の常駐ログセッションを起動する。
///
/// `spawn_blocking` 内で `UnixStream` を接続して HTTP ヘッダを検証し、成功したら
/// demux ループに移行する。起動結果は oneshot で呼び出し側に通知され、ヘッダ検証
/// (ステータス / Content-Type) の失敗は `Err` として伝播する。
pub(crate) async fn spawn_log_session(
    socket_path: String,
    id: String,
) -> crate::core::error::Result<Arc<DockerLogsHandle>> {
    let handle = DockerLogsHandle::new(socket_path.clone(), id.clone());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let session_handle = handle.clone();
    tokio::task::spawn_blocking(move || {
        run_log_session(&socket_path, &id, &session_handle, started_tx);
    });
    started_rx
        .await
        .map_err(|_| crate::Error::other("log session task exited before startup completed"))??;
    Ok(handle)
}

/// drop 時に必ず `terminate_all` を呼ぶガード。
///
/// demux タスクが正常終了・panic のいずれで終わっても、リーダーが永久ブロックしないよう
/// ストリーム終端と完了フラグ設定を保証する。
struct TerminateOnDrop<'a> {
    handle: &'a DockerLogsHandle,
}

impl Drop for TerminateOnDrop<'_> {
    fn drop(&mut self) {
        self.handle.terminate_all();
    }
}

/// drop 時に必ず `consumer_finished` を呼ぶガード。
///
/// LogConsumer 配信タスクが正常終了・panic のいずれで終わっても `active_consumers` を
/// 減少させる。これがないと、`LogConsumer::accept` が panic した場合にカウンタが減らず、
/// `all_done()` が永久に false のままになる。
///
/// このガードは `tokio::spawn` で起動した future 内で生成される。`register_consumer()`
/// は spawn の外で呼ばれるため、Runtime コンテキスト不在で `tokio::spawn` 自体が
/// panic した場合、または spawn 後最初の poll 前にタスクが cancel (runtime shutdown 等)
/// された場合はカウンタが 1 増えたままになるが、呼び出し元は全て async 文脈
/// (`ContainerAsync::start` 等) であり、`all_done()` を読む主体が残らない時点のため
/// 実害はない。
struct ConsumerFinishedOnDrop {
    handle: Arc<DockerLogsHandle>,
}

impl Drop for ConsumerFinishedOnDrop {
    fn drop(&mut self) {
        self.handle.consumer_finished();
    }
}

/// blocking スレッド上でログセッション全体 (接続 → ヘッダ検証 → demux ループ) を実行する。
fn run_log_session(
    socket_path: &str,
    id: &str,
    handle: &DockerLogsHandle,
    started_tx: tokio::sync::oneshot::Sender<crate::core::error::Result<()>>,
) {
    // 正常終了・panic のいずれでもストリームを終端するガード (リーダーの永久ブロック防止)。
    let _guard = TerminateOnDrop { handle };
    let mut started = Some(started_tx);
    let result = start_and_demux(socket_path, id, handle, &mut started);
    // 起動結果が未送信なら (接続・ヘッダ検証の失敗) その Err を詳細ごと転送する。
    if let Some(tx) = started.take() {
        let err = match result {
            Err(e) => e,
            Ok(()) => crate::Error::other("log session failed before startup completed"),
        };
        let _ = tx.send(Err(err));
    } else if let Err(e) = result {
        // 起動成功後の中途エラー (demux ループ内のエラー)。診断のため記録する
        // (ここで握り潰すとログが理由不明で止まる)。
        tracing::warn!("log stream demux terminated with error: {e}");
    }
    // `_guard` の Drop で `terminate_all` が呼ばれ、待機中のリーダーを起こして完了フラグを立てる。
}

/// 接続・リクエスト送信・ヘッダ検証を行い、成功したら demux ループを回す。
///
/// ヘッダ検証に成功した時点で `started` を `Ok` で消費し、呼び出し側に起動成功を通知する。
fn start_and_demux(
    socket_path: &str,
    id: &str,
    handle: &DockerLogsHandle,
    started: &mut Option<tokio::sync::oneshot::Sender<crate::core::error::Result<()>>>,
) -> crate::core::error::Result<()> {
    let mut stream = UnixStream::connect(socket_path)?;
    // デーモン無応答時の無限ブロックを抑止するため起動検証にタイムアウトを設定する。
    // demux_loop 移行前に解除する (follow ストリームは legitimately 長時間無ログになり得る)。
    stream
        .set_read_timeout(LOG_SESSION_TIMEOUT)
        .map_err(|e| crate::core::error::Error::other(e.to_string()))?;
    stream
        .set_write_timeout(LOG_SESSION_TIMEOUT)
        .map_err(|e| crate::core::error::Error::other(e.to_string()))?;
    // 外部から閉じられるよう複製をハンドルに保持する。try_clone 失敗時は
    // log_stop フラグと後続の remove (デーモン側が接続を閉じる) による停止に頼る。
    if let Ok(clone) = stream.try_clone() {
        *handle
            .shutdown_socket
            .lock()
            .expect("shutdown socket mutex must not be poisoned while starting log stream") =
            Some(clone);
    }

    let path = format!(
        "/containers/{}/logs?stdout=1&stderr=1&follow=true&tail=all",
        crate::core::client::docker_client::percent_encode_path_segment(id)
    );
    let request_bytes =
        crate::core::client::docker_client::encode_docker_api_request("GET", &path, None)?;
    stream.write_all(&request_bytes)?;

    let mut decoder = log_stream_decoder();
    decoder.set_request_method("GET");
    read_and_validate_head(&mut stream, &mut decoder)?;

    // 起動成功を通知 (demux ループは継続する)。送信失敗は呼び出し側が去った
    // (キャンセルされた) ことを意味するため、demux に進まず即終了する (タスクのリーク防止)。
    if let Some(tx) = started.take()
        && tx.send(Ok(())).is_err()
    {
        return Ok(());
    }

    // 起動検証が完了したためタイムアウトを解除する。
    // follow ストリームは legitimately 長時間無ログになり得る。
    stream
        .set_read_timeout(None)
        .map_err(|e| crate::core::error::Error::other(e.to_string()))?;
    stream
        .set_write_timeout(None)
        .map_err(|e| crate::core::error::Error::other(e.to_string()))?;

    demux_loop(&mut stream, &mut decoder, handle)
}

/// レスポンスヘッダを読み切り、ステータスと Content-Type を検証する。
///
/// Content-Type が multiplex 系 (`application/vnd.docker.multiplexed-stream` /
/// `application/octet-stream` / 空) のいずれかであることを確認する。TTY 有効時の
/// `application/vnd.docker.raw-stream` や 400 系応答はエラーで拒否する。
fn read_and_validate_head(
    stream: &mut UnixStream,
    decoder: &mut ResponseDecoder,
) -> crate::core::error::Result<()> {
    let head = loop {
        let want = decoder.available_buf().min(READ_CHUNK);
        if want == 0 {
            return Err(io_other("decoder buffer full while reading log head").into());
        }
        let buf = decoder
            .mut_buf(want)
            .map_err(|e| crate::Error::other(io_other(e.to_string())))?;
        let n = stream.read(buf)?;
        decoder.advance_buf(n);
        if n == 0 {
            decoder.mark_eof();
        }
        match decoder
            .decode_headers()
            .map_err(|e| crate::Error::other(io_other(e.to_string())))?
        {
            Some((head, _body_kind)) => break head,
            None => {
                if n == 0 {
                    return Err(io_other("connection closed before log head complete").into());
                }
            }
        }
    };

    let status = head.status_code();
    if status != 200 {
        return Err(io_other(format!("logs request failed with status {status}")).into());
    }
    let content_type = head
        .headers()
        .iter()
        .find(|(name, _)| name.as_str().eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.as_str());
    if !is_multiplex_content_type(content_type) {
        return Err(io_other(format!(
            "unsupported log stream content type: {}",
            content_type.unwrap_or("<absent>")
        ))
        .into());
    }
    Ok(())
}

/// Content-Type が multiplex demux で処理できる種類か。
///
/// 古い Docker Engine (API < 1.42 相当) は Content-Type を空または `application/octet-stream`
/// で返すため、Docker CLI と同じくこれらを multiplex として扱う。
fn is_multiplex_content_type(content_type: Option<&str>) -> bool {
    match content_type {
        None => true,
        Some(ct) => {
            let media = ct.split(';').next().unwrap_or(ct).trim();
            media.is_empty()
                || media.eq_ignore_ascii_case("application/vnd.docker.multiplexed-stream")
                || media.eq_ignore_ascii_case("application/octet-stream")
        }
    }
}

/// デコード済みボディを demux し、共有バッファへ追記して通知する。
fn drain_decoded(
    decoder: &mut ResponseDecoder,
    demuxer: &mut FrameDemuxer,
    handle: &DockerLogsHandle,
) -> crate::core::error::Result<()> {
    loop {
        let peeked = decoder.peek_body().map(|data| data.len());
        if let Some(len) = peeked {
            let (wrote_out, wrote_err) = {
                let data = decoder
                    .peek_body()
                    .expect("peek_body must return data right after measuring length");
                let mut stdout = handle
                    .stdout
                    .buffer
                    .lock()
                    .expect("stdout buffer mutex must not be poisoned while demuxing logs");
                let mut stderr = handle
                    .stderr
                    .buffer
                    .lock()
                    .expect("stderr buffer mutex must not be poisoned while demuxing logs");
                demuxer
                    .feed(data, &mut stdout, &mut stderr)
                    .map_err(crate::Error::other)?
            };
            decoder
                .consume_body(len)
                .map_err(|e| crate::Error::other(io_other(e.to_string())))?;
            if wrote_out {
                handle.stdout.notify.notify_waiters();
            }
            if wrote_err {
                handle.stderr.notify.notify_waiters();
            }
            continue;
        }
        match decoder
            .progress()
            .map_err(|e| crate::Error::other(io_other(e.to_string())))?
        {
            BodyProgress::Complete { .. } => return Ok(()),
            BodyProgress::Advanced => continue,
            BodyProgress::NeedData => return Ok(()),
        }
    }
}

/// ソケットから読み込みつつ demux ループを回す。TCP EOF / 停止指示 / ソケットエラーで抜ける。
fn demux_loop(
    stream: &mut UnixStream,
    decoder: &mut ResponseDecoder,
    handle: &DockerLogsHandle,
) -> crate::core::error::Result<()> {
    let mut demuxer = FrameDemuxer::new();
    loop {
        if handle.log_stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        drain_decoded(decoder, &mut demuxer, handle)?;
        let want = decoder.available_buf().min(READ_CHUNK);
        if want == 0 {
            return Err(io_other("decoder buffer full while streaming logs").into());
        }
        let buf = decoder
            .mut_buf(want)
            .map_err(|e| crate::Error::other(io_other(e.to_string())))?;
        let n = stream.read(buf)?;
        decoder.advance_buf(n);
        if n == 0 {
            // TCP EOF。残りを demux して抜ける (終端通知は呼び出し元が行う)。
            decoder.mark_eof();
            drain_decoded(decoder, &mut demuxer, handle)?;
            return Ok(());
        }
    }
}

/// `?follow=false&tail=all` の 1-shot 取得。blocking スレッド上で完了まで読み切る。
///
/// 戻り値は `(stdout, stderr)`。`stdout_to_vec` / `stderr_to_vec` 系が呼び出しごとに新規
/// セッションを張って決定的に全ログを取得するための経路。
fn fetch_logs_oneshot_blocking(socket_path: &str, id: &str) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    fetch_logs_oneshot_blocking_with_limit(
        socket_path,
        id,
        crate::core::client::docker_client::DOCKER_RESPONSE_BODY_LIMIT,
    )
}

/// 1-shot 取得の上限付き版。テストから小さな上限を渡して境界を検証するための内部関数。
fn fetch_logs_oneshot_blocking_with_limit(
    socket_path: &str,
    id: &str,
    limit: usize,
) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let mut stream = UnixStream::connect(socket_path)?;
    // デーモン無応答時の無限ブロックを抑止するためタイムアウトを設定する。
    stream.set_read_timeout(LOG_SESSION_TIMEOUT)?;
    stream.set_write_timeout(LOG_SESSION_TIMEOUT)?;
    let path = format!(
        "/containers/{}/logs?stdout=1&stderr=1&follow=false&tail=all",
        crate::core::client::docker_client::percent_encode_path_segment(id)
    );
    let request_bytes =
        crate::core::client::docker_client::encode_docker_api_request("GET", &path, None)
            .map_err(|e| io_other(e.to_string()))?;
    stream.write_all(&request_bytes)?;

    let mut decoder = log_stream_decoder();
    decoder.set_request_method("GET");
    let mut demuxer = FrameDemuxer::new();
    // 1-shot 蓄積はストリームあたり上限・超過時エラー (exec 出力と同じ方針)。
    // 切り詰めると「決定的に全ログを取得する」契約が壊れるため strict モードにする。
    let mut stdout = SharedLogBuffer::new_strict(limit);
    let mut stderr = SharedLogBuffer::new_strict(limit);
    let mut head_done = false;

    loop {
        if !head_done {
            let want = decoder.available_buf().min(READ_CHUNK);
            if want == 0 {
                return Err(io_other("decoder buffer full while reading oneshot head"));
            }
            let buf = decoder.mut_buf(want).map_err(|e| io_other(e.to_string()))?;
            let n = stream.read(buf)?;
            decoder.advance_buf(n);
            if n == 0 {
                decoder.mark_eof();
            }
            match decoder
                .decode_headers()
                .map_err(|e| io_other(e.to_string()))?
            {
                Some((head, _body_kind)) => {
                    if head.status_code() != 200 {
                        return Err(io_other(format!(
                            "logs request failed with status {}",
                            head.status_code()
                        )));
                    }
                    // follow 経路と同じく Content-Type を検証する (TTY 有効時の raw-stream 拒否)。
                    let content_type = head
                        .headers()
                        .iter()
                        .find(|(name, _)| name.as_str().eq_ignore_ascii_case("content-type"))
                        .map(|(_, value)| value.as_str());
                    if !is_multiplex_content_type(content_type) {
                        return Err(io_other(format!(
                            "unsupported log stream content type: {}",
                            content_type.unwrap_or("<absent>")
                        )));
                    }
                    head_done = true;
                }
                None => {
                    if n == 0 {
                        return Err(io_other("connection closed before oneshot head complete"));
                    }
                    continue;
                }
            }
        }

        // デコード済みボディを demux。完了したら抜ける。
        if drain_oneshot_body(&mut decoder, &mut demuxer, &mut stdout, &mut stderr)? {
            break;
        }

        let want = decoder.available_buf().min(READ_CHUNK);
        if want == 0 {
            return Err(io_other("decoder buffer full while reading oneshot body"));
        }
        let buf = decoder.mut_buf(want).map_err(|e| io_other(e.to_string()))?;
        let n = stream.read(buf)?;
        decoder.advance_buf(n);
        if n == 0 {
            // TCP EOF。close-delimited なら mark_eof で Complete になるが、chunked /
            // content-length の未完成ボディでは mark_eof は何もしない。そのまま読み続けると
            // ビジーループになるため、最終 drain だけ行ってここで抜ける (未完成ボディは打ち切り)。
            decoder.mark_eof();
            drain_oneshot_body(&mut decoder, &mut demuxer, &mut stdout, &mut stderr)?;
            break;
        }
    }

    let out = drain_buffer(&stdout);
    let err = drain_buffer(&stderr);
    Ok((out, err))
}

/// 1-shot 経路: デコード済みボディを demux してバッファへ追記する。
///
/// ボディが完了 (`Complete`) したら `true` を返す。`NeedData` (追加読み込みが必要) なら
/// `false` を返す。follow 経路の `drain_decoded` と同型だが、エラー型が `std::io::Error`。
/// strict モードのバッファが上限超過を検知した場合は `Err` を返し、読み出しを即座に止める。
fn drain_oneshot_body(
    decoder: &mut ResponseDecoder,
    demuxer: &mut FrameDemuxer,
    stdout: &mut SharedLogBuffer,
    stderr: &mut SharedLogBuffer,
) -> std::io::Result<bool> {
    loop {
        let peeked = decoder.peek_body().map(|data| data.len());
        if let Some(len) = peeked {
            let data = decoder
                .peek_body()
                .expect("peek_body must return data right after measuring length");
            demuxer.feed(data, stdout, stderr)?;
            decoder
                .consume_body(len)
                .map_err(|e| io_other(e.to_string()))?;
            continue;
        }
        return Ok(
            match decoder.progress().map_err(|e| io_other(e.to_string()))? {
                BodyProgress::Complete { .. } => true,
                BodyProgress::Advanced => continue,
                BodyProgress::NeedData => false,
            },
        );
    }
}

/// 共有バッファの全内容を `Vec<u8>` に取り出す (1-shot 経路用)。
fn drain_buffer(buffer: &SharedLogBuffer) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunk = [0u8; READ_CHUNK];
    let mut offset = buffer.head;
    loop {
        let (n, next, _) = buffer.read_at(offset, &mut chunk);
        if n == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n]);
        offset = next;
    }
    out
}

/// 共有バッファに対する独立オフセットの非同期リーダー (follow=true 相当)。
///
/// 追記待ちは `tokio::sync::Notify` で行う。バッファ空 + terminated のとき `Ok(&[])`
/// (EOF) を返す。先頭 drop 済み領域を読もうとした場合はスキップして継続し、初回のみ warn する。
pub(crate) struct LogReader {
    stream: Arc<LogStream>,
    /// 次に読むべき絶対オフセット。
    offset: u64,
    /// 先頭 drop の warn を一度だけ出すためのフラグ。
    warned_skip: bool,
    /// 共有バッファからコピー済みの未消費データ。
    buf: Vec<u8>,
    /// `buf` 内の読み取り位置。
    pos: usize,
    /// 追記待ちの future。`Notify` を `Arc` ごと move して所有するため自己参照にならない。
    wait: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl LogReader {
    fn new(stream: Arc<LogStream>) -> Self {
        Self {
            stream,
            offset: 0,
            warned_skip: false,
            buf: Vec::new(),
            pos: 0,
            wait: None,
        }
    }

    /// 内部バッファを補充し、未消費スライスを返す。`poll_fill_buf` / `poll_read` の共通実装。
    fn poll_fill(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<&[u8]>> {
        loop {
            if self.pos < self.buf.len() {
                return Poll::Ready(Ok(&self.buf[self.pos..]));
            }
            // 内部バッファが空。追記通知の取りこぼしを防ぐため、先に notified を登録してから
            // 共有バッファを再確認する。
            let wait = self.wait.get_or_insert_with(|| {
                let stream = self.stream.clone();
                Box::pin(async move { stream.notify.notified().await })
            });
            // 一度 poll して Notified を登録する (登録後の通知は必ず wakeup になる)。
            if wait.as_mut().poll(cx).is_ready() {
                self.wait = None;
                continue;
            }
            // 共有バッファから読む。ロック保持中に await しない。
            self.buf.clear();
            self.pos = 0;
            self.buf.resize(READ_CHUNK, 0);
            let (n, next, skipped) = {
                let guard = self
                    .stream
                    .buffer
                    .lock()
                    .expect("log buffer mutex must not be poisoned while reading logs");
                guard.read_at(self.offset, &mut self.buf)
            };
            self.offset = next;
            self.buf.truncate(n);
            if skipped && !self.warned_skip {
                tracing::warn!("log buffer overflow detected; skipping dropped bytes");
                self.warned_skip = true;
            }
            if n > 0 {
                self.wait = None;
                return Poll::Ready(Ok(&self.buf[self.pos..]));
            }
            if self.stream.terminated.load(Ordering::SeqCst) {
                self.wait = None;
                return Poll::Ready(Ok(&[]));
            }
            // 追記待ち。Notified は poll 済み (waker 登録済み) なので Pending を返す。
            return Poll::Pending;
        }
    }
}

impl AsyncBufRead for LogReader {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<&[u8]>> {
        self.get_mut().poll_fill(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.get_mut();
        this.pos = (this.pos + amt).min(this.buf.len());
    }
}

impl tokio::io::AsyncRead for LogReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        match this.poll_fill(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Ready(Ok(data)) => {
                let n = data.len().min(buf.remaining());
                buf.put_slice(&data[..n]);
                this.pos += n;
                Poll::Ready(Ok(()))
            }
        }
    }
}

/// 共有バッファに対する独立オフセットの同期リーダー (follow=true 相当)。
///
/// `tokio::sync::Notify` は `std::thread::park` と繋がらないため、`park_timeout(50ms)` の
/// 周期起床でバッファと terminated を再確認する。バッファ空 + terminated のとき `Ok(0)`。
pub(crate) struct SyncLogReader {
    stream: Arc<LogStream>,
    offset: u64,
    warned_skip: bool,
}

impl SyncLogReader {
    fn new(stream: Arc<LogStream>) -> Self {
        Self {
            stream,
            offset: 0,
            warned_skip: false,
        }
    }
}

impl Read for SyncLogReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let (n, next, skipped) = {
                let guard = self
                    .stream
                    .buffer
                    .lock()
                    .expect("log buffer mutex must not be poisoned while reading logs");
                guard.read_at(self.offset, buf)
            };
            self.offset = next;
            if skipped && !self.warned_skip {
                tracing::warn!("log buffer overflow detected; skipping dropped bytes");
                self.warned_skip = true;
            }
            if n > 0 {
                return Ok(n);
            }
            if self.stream.terminated.load(Ordering::SeqCst) {
                return Ok(0);
            }
            std::thread::park_timeout(Duration::from_millis(50));
        }
    }
}

/// `?follow=false` の 1-shot 同期リーダー。初回 read で blocking 取得を遅延実行する。
///
/// 同期 API (`Container::stdout(false)` 等) が `follow=false` で使う経路。async 版の
/// 1-shot と同じ `fetch_logs_oneshot_blocking` を共有する。
pub(crate) struct SyncOneshotReader {
    socket_path: String,
    id: String,
    stdout: bool,
    data: Option<Vec<u8>>,
    pos: usize,
    failed: bool,
}

impl SyncOneshotReader {
    fn new(socket_path: String, id: String, stdout: bool) -> Self {
        Self {
            socket_path,
            id,
            stdout,
            data: None,
            pos: 0,
            failed: false,
        }
    }
}

impl Read for SyncOneshotReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.data.is_none() && !self.failed {
            match fetch_logs_oneshot_blocking(&self.socket_path, &self.id) {
                Ok((out, err)) => {
                    self.data = Some(if self.stdout { out } else { err });
                }
                Err(e) => {
                    self.failed = true;
                    return Err(e);
                }
            }
        }
        if self.failed {
            return Err(io_other("oneshot log reader already failed"));
        }
        let data = self
            .data
            .as_ref()
            .expect("data must be fetched before reading oneshot logs");
        let remaining = &data[self.pos..];
        let n = remaining.len().min(buf.len());
        buf[..n].copy_from_slice(&remaining[..n]);
        self.pos += n;
        Ok(n)
    }
}

/// `?follow=false` の 1-shot 非同期リーダー。初回 poll で `spawn_blocking` を遅延起動する。
///
/// リーダーを作って即 drop する場合に HTTP セッションの副作用を残さないため、起動は
/// 初回 poll まで遅延させる。Drop は受信側を drop するだけ (`spawn_blocking` は完了まで走る)。
pub(crate) struct OneshotReader {
    socket_path: String,
    id: String,
    /// 取り出すストリーム (stdout なら true)。
    stdout: bool,
    state: OneshotState,
}

enum OneshotState {
    NotStarted,
    Running {
        rx: tokio::sync::oneshot::Receiver<std::io::Result<(Vec<u8>, Vec<u8>)>>,
    },
    Done {
        data: Vec<u8>,
        pos: usize,
    },
    Failed,
}

impl OneshotReader {
    fn new(socket_path: String, id: String, stdout: bool) -> Self {
        Self {
            socket_path,
            id,
            stdout,
            state: OneshotState::NotStarted,
        }
    }
}

impl tokio::io::AsyncRead for OneshotReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        loop {
            match &mut this.state {
                OneshotState::NotStarted => {
                    let socket_path = this.socket_path.clone();
                    let id = this.id.clone();
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    tokio::task::spawn_blocking(move || {
                        let result = fetch_logs_oneshot_blocking(&socket_path, &id);
                        let _ = tx.send(result);
                    });
                    this.state = OneshotState::Running { rx };
                }
                OneshotState::Running { rx } => match Pin::new(rx).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(Ok((out, err)))) => {
                        let data = if this.stdout { out } else { err };
                        this.state = OneshotState::Done { data, pos: 0 };
                    }
                    Poll::Ready(Ok(Err(e))) => {
                        this.state = OneshotState::Failed;
                        return Poll::Ready(Err(e));
                    }
                    Poll::Ready(Err(_)) => {
                        this.state = OneshotState::Failed;
                        return Poll::Ready(Err(io_other("oneshot log fetch task cancelled")));
                    }
                },
                OneshotState::Done { data, pos } => {
                    let remaining = &data[*pos..];
                    let n = remaining.len().min(buf.remaining());
                    buf.put_slice(&remaining[..n]);
                    *pos += n;
                    return Poll::Ready(Ok(()));
                }
                OneshotState::Failed => {
                    return Poll::Ready(Err(io_other("oneshot log reader already failed")));
                }
            }
        }
    }
}

/// LogConsumer へログ行を配信するタスクを起動する (Linux 版)。
///
/// 共有バッファの独立オフセットリーダーを行単位で読み、行末 `\n` / `\r` を剥がして配信する。
/// TCP FIN で終端したとき、最終行に `\n` が無い残余バイトがあっても `read_until` がそれを
/// 返すため 1 フレームとして配信する (macOS 側 `spawn_log_consumer_task` の `read_until` +
/// `Ok(_)` 配信と同じ挙動で、両プラットフォームで揃えている)。
pub(crate) fn spawn_log_consumer_task(
    handle: Arc<DockerLogsHandle>,
    stream: Arc<LogStream>,
    consumers: Arc<Vec<Box<dyn LogConsumer + 'static>>>,
    to_frame: fn(Vec<u8>) -> LogFrame,
) {
    handle.register_consumer();
    tokio::spawn(async move {
        // 正常終了・panic のいずれでも consumer_finished を呼ぶガード。
        let _guard = ConsumerFinishedOnDrop {
            handle: handle.clone(),
        };
        let mut reader = tokio::io::BufReader::new(LogReader::new(stream));
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) => break,
                Ok(_) => {
                    if buf.last() == Some(&b'\n') {
                        buf.pop();
                    }
                    if buf.last() == Some(&b'\r') {
                        buf.pop();
                    }
                    let frame = to_frame(std::mem::take(&mut buf));
                    for consumer in consumers.as_ref() {
                        consumer.accept(&frame).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("log consumer read failed; stopping delivery: {e}");
                    break;
                }
            }
        }
    });
}

/// 単体テスト用: 共有バッファ (stdout 相当) と独立オフセットリーダー 2 本を作る。
///
/// demux パーサと共有バッファ + 独立オフセットリーダーの純ロジックテストに使う。
/// crate 内の任意のモジュールの単体テストから使えるよう `pub(crate)` で露出する。
#[cfg(test)]
pub(crate) fn new_shared_log_buffer_for_test(
    limit: usize,
) -> (Arc<LogStream>, LogReader, LogReader) {
    let stream = LogStream::new(limit);
    let r1 = LogReader::new(stream.clone());
    let r2 = LogReader::new(stream.clone());
    (stream, r1, r2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// multiplex フレームを 1 本作る (テスト用ヘルパ)。
    fn frame(stream_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(stream_type);
        out.extend_from_slice(&[0, 0, 0]);
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn demux_separates_stdout_and_stderr() {
        // stdout / stderr フレームが各自のバッファに分離されること。
        let mut stdout = SharedLogBuffer::new(DEFAULT_BUFFER_LIMIT);
        let mut stderr = SharedLogBuffer::new(DEFAULT_BUFFER_LIMIT);
        let mut demuxer = FrameDemuxer::new();
        let mut input = frame(1, b"out1");
        input.extend(frame(2, b"err1"));
        input.extend(frame(1, b"out2"));
        demuxer
            .feed(&input, &mut stdout, &mut stderr)
            .expect("非 strict バッファでは feed は失敗しないこと");
        assert_eq!(drain_buffer(&stdout), b"out1out2");
        assert_eq!(drain_buffer(&stderr), b"err1");
    }

    #[test]
    fn demux_handles_frame_split_across_chunks() {
        // フレームがチャンク境界で分割されても正しく demux されること。
        let mut stdout = SharedLogBuffer::new(DEFAULT_BUFFER_LIMIT);
        let mut stderr = SharedLogBuffer::new(DEFAULT_BUFFER_LIMIT);
        let mut demuxer = FrameDemuxer::new();
        let input = frame(1, b"hello world");
        // ヘッダ途中・payload 途中で分割する。
        let split_points = [3usize, 8, 12];
        let mut prev = 0;
        for &sp in &split_points {
            demuxer
                .feed(&input[prev..sp], &mut stdout, &mut stderr)
                .expect("非 strict バッファでは feed は失敗しないこと");
            prev = sp;
        }
        demuxer
            .feed(&input[prev..], &mut stdout, &mut stderr)
            .expect("非 strict バッファでは feed は失敗しないこと");
        assert_eq!(drain_buffer(&stdout), b"hello world");
    }

    #[test]
    fn demux_ignores_stdin_and_empty_payload() {
        // stdin (0) と空 payload はバッファに書かれないこと。
        let mut stdout = SharedLogBuffer::new(DEFAULT_BUFFER_LIMIT);
        let mut stderr = SharedLogBuffer::new(DEFAULT_BUFFER_LIMIT);
        let mut demuxer = FrameDemuxer::new();
        let mut input = frame(0, b"stdin");
        input.extend(frame(1, b""));
        input.extend(frame(2, b"e"));
        demuxer
            .feed(&input, &mut stdout, &mut stderr)
            .expect("非 strict バッファでは feed は失敗しないこと");
        assert_eq!(drain_buffer(&stdout), b"");
        assert_eq!(drain_buffer(&stderr), b"e");
    }

    #[test]
    fn demux_ignores_unknown_stream_types() {
        // 未知の STREAM_TYPE (3 以上) はどちらのバッファにも書かれないこと。
        let mut stdout = SharedLogBuffer::new(DEFAULT_BUFFER_LIMIT);
        let mut stderr = SharedLogBuffer::new(DEFAULT_BUFFER_LIMIT);
        let mut demuxer = FrameDemuxer::new();
        let mut input = frame(3, b"x");
        input.extend(frame(255, b"y"));
        input.extend(frame(1, b"ok"));
        demuxer
            .feed(&input, &mut stdout, &mut stderr)
            .expect("非 strict バッファでは feed は失敗しないこと");
        assert_eq!(drain_buffer(&stdout), b"ok");
        assert_eq!(drain_buffer(&stderr), b"");
    }

    #[test]
    fn demux_huge_payload_len_does_not_allocate_upfront() {
        // 巨大な payload_len (u32::MAX) でも実データ分のみの逐次処理で、パニックも
        // 巨大確保も起きないこと (Vec::with_capacity 不使用の回帰)。
        let mut stdout = SharedLogBuffer::new(DEFAULT_BUFFER_LIMIT);
        let mut stderr = SharedLogBuffer::new(DEFAULT_BUFFER_LIMIT);
        let mut demuxer = FrameDemuxer::new();
        // ヘッダ: stream_type=1, payload_len=u32::MAX。実データは 3 バイトだけ与える。
        let mut input = vec![1u8, 0, 0, 0];
        input.extend_from_slice(&u32::MAX.to_be_bytes());
        input.extend_from_slice(b"abc");
        demuxer
            .feed(&input, &mut stdout, &mut stderr)
            .expect("非 strict バッファでは feed は失敗しないこと");
        // 実データ 3 バイトのみがバッファに書かれ、状態機械は payload 待ちに留まる。
        assert_eq!(drain_buffer(&stdout), b"abc");
        assert!(demuxer.in_payload, "巨大 payload 待ちの状態を維持すること");
        assert_eq!(demuxer.payload_remaining, u32::MAX as usize - 3);
    }

    #[test]
    fn shared_buffer_drops_oldest_over_limit() {
        // 上限超過時に先頭から捨てられ、末尾が保持されること。
        let mut buffer = SharedLogBuffer::new(8);
        buffer.append(b"abcdefgh");
        buffer.append(b"ijkl");
        // 12 バイト投入、上限 8 → 先頭 4 バイト (abcd) が drop。
        let mut out = [0u8; 16];
        let (n, _, _) = buffer.read_at(0, &mut out);
        assert_eq!(n, 8);
        assert_eq!(&out[..n], b"efghijkl");
    }

    #[test]
    fn strict_buffer_accepts_exactly_at_limit() {
        // strict モードではちょうど上限まで成功すること (境界: > であり >= ではない)。
        let mut buffer = SharedLogBuffer::new_strict(8);
        assert!(
            buffer.append(b"abcdefgh"),
            "ちょうど 8 バイトは成功すること"
        );
        assert_eq!(drain_buffer(&buffer), b"abcdefgh");
    }

    #[test]
    fn strict_buffer_rejects_over_limit() {
        // strict モードでは上限超過を検知して false を返し、追記しないこと。
        let mut buffer = SharedLogBuffer::new_strict(8);
        assert!(buffer.append(b"abcdefgh"), "先頭 8 バイトは成功すること");
        assert!(!buffer.append(b"i"), "9 バイト目で上限超過を検知すること");
        // 超過分は追記されない (drop-oldest もしない)。
        assert_eq!(drain_buffer(&buffer), b"abcdefgh");
    }

    #[test]
    fn strict_buffer_rejects_single_chunk_over_limit() {
        // 単発入力が上限超過の場合は false を返し、末尾保持もしないこと。
        let mut buffer = SharedLogBuffer::new_strict(8);
        assert!(!buffer.append(b"abcdefghi"), "9 バイト投入は検知されること");
        assert_eq!(drain_buffer(&buffer), b"", "超過分は一切追記されないこと");
    }

    #[test]
    fn strict_demux_reports_overflow_error() {
        // 1-shot 経路 (strict) の demux が上限超過をエラーとして報告すること。
        // フレーム境界をまたいで累積超過した場合も検知できること。
        let mut stdout = SharedLogBuffer::new_strict(8);
        let mut stderr = SharedLogBuffer::new_strict(8);
        let mut demuxer = FrameDemuxer::new();
        // 8 バイトちょうどは成功。
        let ok = demuxer.feed(&frame(1, b"abcdefgh"), &mut stdout, &mut stderr);
        assert!(ok.is_ok(), "ちょうど上限までは成功すること: {ok:?}");
        // 1 バイト超過でエラー。文言はストリーム識別子付きで完全一致。
        let err = demuxer.feed(&frame(1, b"i"), &mut stdout, &mut stderr);
        assert!(err.is_err(), "上限超過はエラーになること");
        assert_eq!(
            err.expect_err("エラーであること").to_string(),
            "stdout output exceeds 8 bytes limit"
        );
    }

    #[test]
    fn strict_demux_reports_stderr_overflow_error() {
        // stderr 側の上限超過も stdout と同じくエラーとして報告されること。
        let mut stdout = SharedLogBuffer::new_strict(8);
        let mut stderr = SharedLogBuffer::new_strict(8);
        let mut demuxer = FrameDemuxer::new();
        demuxer
            .feed(&frame(2, b"abcdefgh"), &mut stdout, &mut stderr)
            .expect("ちょうど上限までは成功すること");
        let err = demuxer.feed(&frame(2, b"i"), &mut stdout, &mut stderr);
        assert!(err.is_err(), "stderr の上限超過はエラーになること");
        assert_eq!(
            err.expect_err("エラーであること").to_string(),
            "stderr output exceeds 8 bytes limit"
        );
    }

    #[test]
    fn shared_buffer_reader_behind_head_is_skipped() {
        // 先頭 drop 済み領域を読もうとしたリーダーはスキップされ、フラグが立つこと。
        let mut buffer = SharedLogBuffer::new(4);
        // 累積で上限超過させ head を進める (1 回の巨大投入は末尾保持で head が進まないため)。
        buffer.append(b"abcd");
        // 合計 6 バイト > 上限 4 → 先頭 2 バイト (ab) が drop され head = 2 になる。
        buffer.append(b"ef");
        let mut out = [0u8; 8];
        let (n, next, skipped) = buffer.read_at(0, &mut out);
        assert!(skipped, "先頭 drop 済みならスキップフラグが立つこと");
        assert_eq!(&out[..n], b"cdef");
        assert_eq!(next, 6);
    }

    #[tokio::test]
    async fn log_reader_returns_independent_offsets() {
        // 2 本のリーダーが同じバッファを独立オフセットで読むこと。
        let (stream, mut r1, mut r2) = new_shared_log_buffer_for_test(DEFAULT_BUFFER_LIMIT);
        stream
            .buffer
            .lock()
            .expect("テストでロックが取得できること")
            .append(b"first");
        stream.notify.notify_waiters();

        // r1 で先頭から 5 バイト読む (terminate 前は EOF まで読もうとするとハングするため、
        // read_exact でデータ分だけ読む)。
        let mut buf1 = vec![0u8; 5];
        tokio::io::AsyncReadExt::read_exact(&mut r1, &mut buf1)
            .await
            .expect("r1 の読み取りに失敗した");
        assert_eq!(buf1, b"first");

        // r1 が読み進めた後でも、r2 は独立オフセットで先頭から読めること。
        stream.terminate();
        let mut buf2 = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut r2, &mut buf2)
            .await
            .expect("r2 の読み取りに失敗した");
        assert_eq!(buf2, b"first");
    }

    #[tokio::test]
    async fn log_reader_wakes_on_append_notify() {
        // 空バッファで Pending にパークしたリーダーが、追記通知で起床してデータを読むこと。
        let (stream, mut r, _r2) = new_shared_log_buffer_for_test(DEFAULT_BUFFER_LIMIT);
        // 別タスクから少し後に追記 + 通知する。
        let writer = stream.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            writer
                .buffer
                .lock()
                .expect("テストでロックが取得できること")
                .append(b"later");
            writer.notify.notify_waiters();
        });
        // 空バッファから読み始めると Pending になり、追記通知で起床してデータを読む。
        let mut buf = vec![0u8; 5];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::io::AsyncReadExt::read_exact(&mut r, &mut buf),
        )
        .await
        .expect("追記通知で起床せずタイムアウトした")
        .expect("読み取りに失敗した");
        assert_eq!(buf, b"later");
    }

    #[tokio::test]
    async fn log_reader_eofs_when_terminated_and_empty() {
        // バッファ空 + terminated で EOF (Ok(&[])) を返すこと。
        let (stream, mut r, _r2) = new_shared_log_buffer_for_test(DEFAULT_BUFFER_LIMIT);
        stream.terminate();
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut r, &mut buf)
            .await
            .expect("terminated 後の読み取りに失敗した");
        assert_eq!(buf, b"");
    }

    #[test]
    fn sync_log_reader_eofs_when_terminated() {
        // 同期リーダーもバッファ空 + terminated で Ok(0) を返すこと。
        let (stream, _r1, _r2) = new_shared_log_buffer_for_test(DEFAULT_BUFFER_LIMIT);
        let mut sync_reader = SyncLogReader::new(stream.clone());
        stream
            .buffer
            .lock()
            .expect("テストでロックが取得できること")
            .append(b"data");
        stream.terminate();
        let mut out = Vec::new();
        sync_reader
            .read_to_end(&mut out)
            .expect("同期リーダーの読み取りに失敗した");
        assert_eq!(out, b"data");
    }

    #[test]
    fn multiplex_content_type_detection() {
        // multiplex 系 Content-Type の判定。
        assert!(is_multiplex_content_type(None));
        assert!(is_multiplex_content_type(Some("")));
        assert!(is_multiplex_content_type(Some(
            "application/vnd.docker.multiplexed-stream"
        )));
        assert!(is_multiplex_content_type(Some("application/octet-stream")));
        assert!(is_multiplex_content_type(Some(
            "application/octet-stream; charset=x"
        )));
        // TTY 有効時の raw-stream は拒否する。
        assert!(!is_multiplex_content_type(Some(
            "application/vnd.docker.raw-stream"
        )));
    }

    #[test]
    fn oneshot_returns_without_hang_on_truncated_body() {
        // 異常 EOF (Content-Length ボディの途中で接続が切れる) でもビジーループせず、
        // 受信済みのフレームを demux して戻ることを実 Unix ソケットで検証する (F1 の回帰)。
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;

        // 一時ソケットパス (プロセス ID で一意化)。
        let path = std::env::temp_dir().join(format!(
            "container-rs-log-oneshot-test-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("UnixListener の bind に失敗した");

        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().expect("accept に失敗した");
            // リクエストを読み捨てる (接続確立の同期代わり)。
            let mut req = [0u8; 1024];
            let _ = conn.read(&mut req);
            // 完全な multiplex フレーム 1 個 (stdout "abc") を送った後、Content-Length を
            // 大きく偽ってボディ途中で接続を閉じる (異常 EOF を再現)。
            let mut frame = vec![1u8, 0, 0, 0];
            frame.extend_from_slice(&3u32.to_be_bytes());
            frame.extend_from_slice(b"abc");
            let head = "HTTP/1.1 200 OK\r\n\
                        Content-Type: application/vnd.docker.multiplexed-stream\r\n\
                        Content-Length: 100\r\n\
                        \r\n";
            conn.write_all(head.as_bytes())
                .expect("ヘッダ書き込みに失敗した");
            conn.write_all(&frame).expect("フレーム書き込みに失敗した");
            // drop で close。ボディは未完成 (11/100 バイト)。
        });

        let socket_path = path
            .to_str()
            .expect("ソケットパスが UTF-8 であること")
            .to_string();
        // 回帰時にハングせずクリーンに失敗するよう、タイムアウト付きで結果を受ける。
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_logs_oneshot_blocking(&socket_path, "test-id"));
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("fetch_logs_oneshot_blocking がタイムアウトした (ビジーループの疑い)");
        server.join().expect("サーバスレッドが panic した");
        let _ = std::fs::remove_file(&path);

        // ハングせず戻り、受信済みのフレームは demux されていること。
        let (out, err) = result.expect("異常 EOF でもエラーにならず戻ること");
        assert_eq!(out, b"abc", "受信済みの stdout フレームを demux すること");
        assert_eq!(err, b"");
    }

    #[test]
    fn oneshot_aborts_early_when_over_limit() {
        // 上限超過を検知した時点で読み出しを止めてエラーを返すこと (ハングしないこと)。
        // サーバが上限を超えるフレームを送り続けても、fetch 側は即座に Err で戻り、
        // サーバ側の書き込みは EPIPE で終わる (クライアントが読みを止めた証拠)。
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;

        // 一時ソケットパス (プロセス ID で一意化)。
        let path = std::env::temp_dir().join(format!(
            "container-rs-log-oneshot-limit-test-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("UnixListener の bind に失敗した");

        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().expect("accept に失敗した");
            // リクエストを読み捨てる (接続確立の同期代わり)。
            let mut req = [0u8; 1024];
            let _ = conn.read(&mut req);
            let head = "HTTP/1.1 200 OK\r\n\
                        Content-Type: application/vnd.docker.multiplexed-stream\r\n\
                        \r\n";
            conn.write_all(head.as_bytes())
                .expect("ヘッダ書き込みに失敗した");
            // 上限 8 バイトを超える stdout フレーム (payload 9 バイト) を送り続ける。
            // クライアントが上限超過で読みを止めて接続を閉じるまでループする
            // (EPIPE はクライアント側の早期アボートの証拠なので無視する)。
            let mut frame = vec![1u8, 0, 0, 0];
            frame.extend_from_slice(&9u32.to_be_bytes());
            frame.extend_from_slice(b"abcdefghi");
            while conn.write_all(&frame).is_ok() {}
        });

        let socket_path = path
            .to_str()
            .expect("ソケットパスが UTF-8 であること")
            .to_string();
        // 回帰時にハングせずクリーンに失敗するよう、タイムアウト付きで結果を受ける。
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_logs_oneshot_blocking_with_limit(
                &socket_path,
                "test-id",
                8,
            ));
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("fetch_logs_oneshot_blocking がタイムアウトした (ビジーループの疑い)");
        server.join().expect("サーバスレッドが panic した");
        let _ = std::fs::remove_file(&path);

        // 上限超過を検知してエラーを返すこと。
        let err = result.expect_err("上限超過はエラーになること");
        assert_eq!(
            err.to_string(),
            "stdout output exceeds 8 bytes limit",
            "エラー文言がストリーム識別子と上限を明示すること"
        );
    }

    #[test]
    fn log_stream_decoder_has_unlimited_body_size() {
        // ログは非有界ストリームのため max_body_size が無制限であること。
        // 既定の 10 MiB のままだと合計ログ 10 MiB 超でデコーダが BodyTooLarge を起こし、
        // ストリームが強制終了する (致命的バグの回帰)。
        assert_eq!(log_stream_decoder().limits().max_body_size, u64::MAX);
    }

    /// 配信タスクにログ行を追記する (テスト用ヘルパ)。
    ///
    /// 共有バッファに追記し、待機中のリーダーを起こす。`terminate_all` と違い
    /// ストリーム終端と `demux_done` は立てないため、`active_consumers` の動きを
    /// 単独で観測できる。
    fn append_line(handle: &DockerLogsHandle, line: &[u8]) {
        let stream = handle.stdout_stream();
        let mut buffer = stream
            .buffer
            .lock()
            .expect("ログストリームのバッファ mutex が poison されていないこと");
        buffer.append(line);
        drop(buffer);
        stream.notify.notify_waiters();
    }

    /// `LogConsumer::accept` が panic しても `active_consumers` が減少し、
    /// `all_done()` が true になること (カウンタの不変条件が保たれること)。
    ///
    /// 修正前は panic でタスクが終了しても `consumer_finished()` が呼ばれず、
    /// `active_consumers` が 1 のまま残り、`all_done()` が永久に false になった。
    #[tokio::test]
    async fn consumer_panic_still_decrements_active_consumers() {
        use crate::core::logs::LogFrame;

        let handle = DockerLogsHandle::new("unused-socket".to_string(), "test-id".to_string());
        // accept が panic するコールバック。
        // この panic は tokio の既定どおり stderr にバックトレース付きで出力されるが、
        // 意図的に発火させているためテスト失敗ではない (握り潰さない方針の検証でもある)。
        let consumers = Arc::new(vec![Box::new(|_: &LogFrame| -> () {
            panic!("意図的な consumer の panic");
        }) as Box<dyn LogConsumer + 'static>]);

        spawn_log_consumer_task(
            handle.clone(),
            handle.stdout_stream(),
            consumers,
            LogFrame::StdOut,
        );
        assert_eq!(
            handle
                .active_consumers
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "配信タスク起動直後はカウンタが 1 であること (register_consumer が効いていること)"
        );

        // 配信対象の行を追記して通知し、panic を発火させる。
        append_line(&handle, b"hello\n");

        // panic 経由でタスクが終了した後、active_consumers が 0 に戻ることを待つ。
        // EOF を経ずにタスクが終了した = panic 経由で減算されたことの独立検証。
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if handle
                .active_consumers
                .load(std::sync::atomic::Ordering::SeqCst)
                == 0
            {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("panic 後も active_consumers が 0 に戻らなかったこと");
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        // terminate_all で demux_done を立てると all_done() が true になること。
        handle.terminate_all();
        assert!(
            handle.all_done(),
            "panic 後も all_done() が true になること"
        );
    }

    /// 正常終了後も `all_done()` が true になること (カウンタがちょうど 1 回だけ減算されること)。
    #[tokio::test]
    async fn consumer_normal_finish_decrements_active_consumers_once() {
        use std::sync::atomic::Ordering;

        use crate::core::logs::LogFrame;

        let handle = DockerLogsHandle::new("unused-socket".to_string(), "test-id".to_string());
        let received = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let received_clone = received.clone();
        let consumers = Arc::new(vec![Box::new(move |_: &LogFrame| {
            received_clone.fetch_add(1, Ordering::SeqCst);
        }) as Box<dyn LogConsumer + 'static>]);

        spawn_log_consumer_task(
            handle.clone(),
            handle.stdout_stream(),
            consumers,
            LogFrame::StdOut,
        );
        assert_eq!(
            handle.active_consumers.load(Ordering::SeqCst),
            1,
            "配信タスク起動直後はカウンタが 1 であること"
        );

        append_line(&handle, b"hello\n");
        // 配信タスクがフレームを消費するのを期限付きで待つ (固定 sleep に依存しない)。
        let received_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if received.load(Ordering::SeqCst) == 1 {
                break;
            }
            if tokio::time::Instant::now() > received_deadline {
                panic!("配信されたフレームが 1 件に達しなかったこと");
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        // ストリームを終端すると配信ループが EOF で終了し、カウンタが 0 に戻る。
        handle.terminate_all();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if handle.active_consumers.load(Ordering::SeqCst) == 0 {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("正常終了後も active_consumers が 0 に戻らなかったこと");
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        assert!(
            handle.all_done(),
            "正常終了後も all_done() が true になること"
        );
    }
}
