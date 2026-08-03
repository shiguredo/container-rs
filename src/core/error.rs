//! エラー型。
//!
//! macOS (XPC) の内部エラーは `Error::Other` でラップする。

use std::error::Error as StdError;
use std::fmt;
use std::time::Duration;

use crate::core::ports::ContainerPort;

/// このクレートで使用する結果型。
pub type Result<T> = std::result::Result<T, Error>;

/// `EndOfStream` の表示に含めるログ末尾プレビューの上限バイト数。
const MAX_END_OF_STREAM_PREVIEW_BYTES: usize = 1024;

/// shiguredo_container で発生しうるエラー。
#[derive(Debug)]
pub enum Error {
    /// クライアントエラー。macOS では XPC のエラーもここに入る。
    Client(ClientError),
    /// コンテナが準備完了でない。
    WaitContainer(WaitContainerError),
    /// コンテナが指定ポートを公開していない。
    PortNotExposed {
        /// コンテナ ID。
        id: String,
        /// 公開されていないポート。
        port: ContainerPort,
    },
    /// コンテナの情報が足りない。
    MissingInfo(ContainerMissingInfo),
    /// exec 操作の失敗。
    Exec(ExecError),
    /// I/O エラー。
    Io(std::io::Error),
    /// その他のエラー。
    Other(Box<dyn StdError + Sync + Send>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Client(e) => write!(f, "client error: {e}"),
            Error::WaitContainer(e) => write!(f, "container is not ready: {e}"),
            Error::PortNotExposed { id, port } => {
                write!(f, "container '{id}' does not expose port {port}")
            }
            Error::MissingInfo(e) => write!(f, "{e}"),
            Error::Exec(e) => write!(f, "exec operation failed: {e}"),
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Other(e) => write!(f, "other error: {e}"),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Error::Client(e) => Some(e),
            Error::WaitContainer(e) => Some(e),
            Error::MissingInfo(e) => Some(e),
            Error::Exec(e) => Some(e),
            Error::Io(e) => Some(e),
            Error::Other(e) => Some(e.as_ref()),
            Error::PortNotExposed { .. } => None,
        }
    }
}

impl From<ClientError> for Error {
    fn from(e: ClientError) -> Self {
        Self::Client(e)
    }
}

impl From<WaitContainerError> for Error {
    fn from(e: WaitContainerError) -> Self {
        Self::WaitContainer(e)
    }
}

impl From<WaitLogError> for Error {
    fn from(e: WaitLogError) -> Self {
        Self::WaitContainer(WaitContainerError::WaitLog(e))
    }
}

impl From<ContainerMissingInfo> for Error {
    fn from(e: ContainerMissingInfo) -> Self {
        Self::MissingInfo(e)
    }
}

impl From<ExecError> for Error {
    fn from(e: ExecError) -> Self {
        Self::Exec(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<tokio::task::JoinError> for Error {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Other(Box::new(e))
    }
}

/// クライアントエラー。macOS (XPC) では XPC の通信エラー・プロトコルエラーをここに入れる。
#[derive(Debug)]
pub enum ClientError {
    /// XPC の接続失敗。
    XpcConnect,
    /// XPC からエラー応答が返った。
    Xpc(String),
    /// XPC から null 応答が返った。
    XpcNullReply,
    /// XPC 呼び出しがタイムアウトした。
    XpcTimeout,
    /// イメージが見つからない。
    ImageNotFound(String),
    /// コンテナが見つからない。
    ContainerNotFound(String),
    /// コンテナ内のパスが見つからない (コンテナ自体は存在する)。
    ContainerPathNotFound(String),
    /// 設定エラー。
    Configuration(String),
    /// JSON パースエラー。
    Json(String),
    /// その他のクライアントエラー。
    Other(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::XpcConnect => write!(f, "XPC connect failed"),
            ClientError::Xpc(s) => write!(f, "XPC error: {s}"),
            ClientError::XpcNullReply => write!(f, "XPC returned null reply"),
            ClientError::XpcTimeout => write!(f, "XPC request timed out"),
            ClientError::ImageNotFound(s) => write!(f, "image not found: {s}"),
            ClientError::ContainerNotFound(s) => write!(f, "container not found: {s}"),
            ClientError::ContainerPathNotFound(s) => write!(f, "container path not found: {s}"),
            ClientError::Configuration(s) => write!(f, "configuration error: {s}"),
            ClientError::Json(s) => write!(f, "JSON parse error: {s}"),
            ClientError::Other(s) => write!(f, "{s}"),
        }
    }
}

impl StdError for ClientError {}

/// コンテナに必要な情報が存在しないことを示すエラー。
#[derive(Debug)]
pub struct ContainerMissingInfo {
    pub(crate) id: String,
    pub(crate) path: String,
}

impl fmt::Display for ContainerMissingInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "container '{}' does not have: {}", self.id, self.path)
    }
}

impl StdError for ContainerMissingInfo {}

/// exec 操作のエラー。
#[derive(Debug)]
pub enum ExecError {
    /// exec プロセスの終了コードが期待値と異なる。
    ExitCodeMismatch {
        /// 期待していた終了コード。
        expected: i64,
        /// 実際の終了コード。
        actual: i64,
    },
    /// exec のログ待機に失敗した。
    WaitLog(WaitLogError),
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::ExitCodeMismatch { expected, actual } => {
                write!(
                    f,
                    "exec process exited with code {actual}, expected {expected}"
                )
            }
            ExecError::WaitLog(e) => write!(f, "failed to wait for exec log: {e}"),
        }
    }
}

impl StdError for ExecError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            ExecError::WaitLog(e) => Some(e),
            ExecError::ExitCodeMismatch { .. } => None,
        }
    }
}

impl From<WaitLogError> for ExecError {
    fn from(e: WaitLogError) -> Self {
        Self::WaitLog(e)
    }
}

/// コンテナ準備完了待機のエラー。
#[derive(Debug)]
pub enum WaitContainerError {
    /// ログ待機に失敗した。
    WaitLog(WaitLogError),
    /// コンテナの状態を取得できない。
    StateUnavailable,
    /// HTTP 待機に失敗した。
    #[cfg(feature = "http_wait_plain")]
    HttpWait(crate::core::wait::http_strategy::HttpWaitError),
    /// ヘルスチェックが設定されていない。
    HealthCheckNotConfigured(String),
    /// コンテナが unhealthy 状態である。
    Unhealthy(String),
    /// コンテナの起動がタイムアウトした。
    StartupTimeout {
        /// コンテナ ID。
        id: String,
        /// タイムアウト時間。
        timeout: Duration,
    },
    /// コンテナが予期しない終了コードで終了した。
    UnexpectedExitCode {
        /// 期待していた終了コード。
        expected: i64,
        /// 実際の終了コード。取得できない場合は `None`。
        actual: Option<i64>,
    },
}

impl fmt::Display for WaitContainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WaitContainerError::WaitLog(e) => write!(f, "failed to wait for container log: {e}"),
            WaitContainerError::StateUnavailable => write!(f, "container state is unavailable"),
            #[cfg(feature = "http_wait_plain")]
            WaitContainerError::HttpWait(e) => write!(f, "{e}"),
            WaitContainerError::HealthCheckNotConfigured(s) => {
                write!(f, "healthcheck is not configured for container: {s}")
            }
            WaitContainerError::Unhealthy(s) => write!(f, "container is unhealthy: {s}"),
            WaitContainerError::StartupTimeout { id, timeout } => write!(
                f,
                "container startup timeout: container {id} did not become ready within {timeout:?}"
            ),
            WaitContainerError::UnexpectedExitCode { expected, actual } => write!(
                f,
                "container exited with unexpected code: expected {expected}, actual {actual:?}"
            ),
        }
    }
}

impl StdError for WaitContainerError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            WaitContainerError::WaitLog(e) => Some(e),
            WaitContainerError::StateUnavailable => None,
            #[cfg(feature = "http_wait_plain")]
            WaitContainerError::HttpWait(e) => Some(e),
            WaitContainerError::HealthCheckNotConfigured(_) => None,
            WaitContainerError::Unhealthy(_) => None,
            WaitContainerError::StartupTimeout { .. } => None,
            WaitContainerError::UnexpectedExitCode { .. } => None,
        }
    }
}

impl From<WaitLogError> for WaitContainerError {
    fn from(e: WaitLogError) -> Self {
        Self::WaitLog(e)
    }
}

/// ログ待機のエラー。
#[derive(Debug)]
pub enum WaitLogError {
    /// ストリームがメッセージを見つける前に終端に達した。
    /// 診断のため、上限内で保持した直近ログを含める (連結済み、要素は 0 または 1)。
    EndOfStream(Vec<Vec<u8>>),
    /// I/O エラー。
    Io(std::io::Error),
}

impl fmt::Display for WaitLogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WaitLogError::EndOfStream(chunks) => {
                let total = chunks.iter().map(|c| c.len()).sum::<usize>();
                let mut preview = Vec::new();
                for chunk in chunks.iter().rev() {
                    let remaining = MAX_END_OF_STREAM_PREVIEW_BYTES - preview.len();
                    preview.extend(chunk.iter().rev().take(remaining).copied());
                    if preview.len() == MAX_END_OF_STREAM_PREVIEW_BYTES {
                        break;
                    }
                }
                preview.reverse();
                let preview = String::from_utf8_lossy(&preview);
                write!(
                    f,
                    "end of stream reached before finding message (collected {total} bytes across {} chunks); log preview: {preview}",
                    chunks.len(),
                )
            }
            WaitLogError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl StdError for WaitLogError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            WaitLogError::Io(e) => Some(e),
            WaitLogError::EndOfStream(_) => None,
        }
    }
}

impl From<std::io::Error> for WaitLogError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl Error {
    /// 任意のエラーを `Other` で包む。
    pub fn other<E>(error: E) -> Self
    where
        E: Into<Box<dyn StdError + Send + Sync>>,
    {
        Self::Other(error.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_contains_variant_description() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err = Error::Io(io);
        assert!(
            err.to_string().contains("I/O error"),
            "Display がバリアントを表現すること"
        );
    }

    #[test]
    fn error_from_io_is_io_variant() {
        let io = std::io::Error::other("test");
        let err: Error = io.into();
        assert!(
            matches!(err, Error::Io(_)),
            "std::io::Error から Error::Io への変換が正しいこと"
        );
    }

    #[test]
    fn client_error_display_roundtrip() {
        let err = ClientError::ImageNotFound("nginx:latest".into());
        let text = err.to_string();
        assert!(
            text.contains("nginx:latest") && text.contains("image not found"),
            "ClientError::ImageNotFound の Display が画像参照を含むこと"
        );
    }

    #[test]
    fn wait_log_error_end_of_stream_display_contains_counts_and_preview() {
        let chunks = vec![b"hello".to_vec(), b" world".to_vec()];
        let err = WaitLogError::EndOfStream(chunks);
        let text = err.to_string();
        assert!(
            text.contains("11 bytes")
                && text.contains("2 chunks")
                && text.contains("log preview: hello world"),
            "EndOfStream の Display がチャンク数、合計バイト数、ログプレビューを含むこと: {text}"
        );
    }

    #[test]
    fn wait_log_error_end_of_stream_display_limits_preview_to_tail() {
        let chunks = vec![
            vec![b'a'; MAX_END_OF_STREAM_PREVIEW_BYTES],
            b"tail".to_vec(),
        ];
        let err = WaitLogError::EndOfStream(chunks);
        let text = err.to_string();
        assert!(
            text.contains("log preview: ") && text.ends_with("tail"),
            "EndOfStream の Display がログ末尾を含むこと: {text}"
        );
        assert!(
            !text.contains(&"a".repeat(MAX_END_OF_STREAM_PREVIEW_BYTES)),
            "EndOfStream の Display がプレビュー上限を超える先頭ログを含まないこと: {text}"
        );
    }

    #[test]
    fn wait_log_error_source_is_io_error() {
        let io = std::io::Error::other("io failed");
        let err = WaitLogError::Io(io);
        assert!(
            err.source().is_some(),
            "WaitLogError::Io は source を持つこと"
        );
    }

    #[test]
    fn exec_error_wait_log_roundtrip() {
        let wait = WaitLogError::EndOfStream(vec![b"x".to_vec()]);
        let exec: ExecError = wait.into();
        assert!(
            matches!(exec, ExecError::WaitLog(_)),
            "WaitLogError から ExecError::WaitLog への変換が正しいこと"
        );
        assert!(
            exec.source().is_some(),
            "ExecError::WaitLog は source を持つこと"
        );
    }

    #[test]
    fn container_missing_info_display_contains_id_and_path() {
        let info = ContainerMissingInfo {
            id: "abc".to_owned(),
            path: "bridge ip".to_owned(),
        };
        let text = info.to_string();
        assert!(
            text.contains("abc") && text.contains("bridge ip"),
            "ContainerMissingInfo の Display が id と path を含むこと"
        );
    }

    #[test]
    fn wait_container_error_from_wait_log() {
        let wait = WaitLogError::EndOfStream(Vec::new());
        let err: WaitContainerError = wait.into();
        assert!(
            matches!(err, WaitContainerError::WaitLog(_)),
            "WaitLogError から WaitContainerError::WaitLog への変換が正しいこと"
        );
    }

    #[cfg(feature = "http_wait_plain")]
    #[test]
    fn http_wait_error_has_source_and_single_outer_prefix() {
        let err = Error::WaitContainer(WaitContainerError::HttpWait(
            crate::core::wait::HttpWaitError::NoResponseMatcher,
        ));
        let text = err.to_string();
        assert!(
            err.source().is_some(),
            "WaitContainerError::HttpWait は source を持つこと"
        );
        assert_eq!(
            text.matches("container is not ready:").count(),
            1,
            "HttpWait の Display は外側のプレフィックスだけを含むこと: {text}"
        );
    }

    #[test]
    fn startup_timeout_display_contains_id_and_timeout() {
        let err = WaitContainerError::StartupTimeout {
            id: "test-container".into(),
            timeout: Duration::from_secs(2),
        };
        let text = err.to_string();
        assert!(
            text.contains("test-container") && text.contains("2s"),
            "StartupTimeout の Display がコンテナ ID と timeout を含むこと: {text}"
        );
    }
}
