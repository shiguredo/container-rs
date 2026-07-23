//! コンテナへのファイルコピー関連。元の 0.27 の `core::copy` と同一シグネチャ。
//!
//! macOS (XPC) では `CopyToContainer` の tar 構築は未実装。`AsyncRunner::start` の
//! `copy_to_sources` 処理で XPC の `containerCopyIn` を呼ぶが、tar 経由ではなく
//! 単一ファイルコピーになるため、本家と完全同じ挙動にはならない点に注意。

use std::path::PathBuf;

/// コピー元データソース。
#[derive(Debug, Clone)]
pub enum CopyDataSource {
    File(PathBuf),
    Data(Vec<u8>),
}

impl From<PathBuf> for CopyDataSource {
    fn from(p: PathBuf) -> Self {
        CopyDataSource::File(p)
    }
}

impl From<Vec<u8>> for CopyDataSource {
    fn from(b: Vec<u8>) -> Self {
        CopyDataSource::Data(b)
    }
}

/// コピー先のファイルオプション。
///
/// 本家 testcontainers-rs 0.27 と同じフィールドを持つ。
/// macOS (Apple container XPC) では `containerCopyIn` が `fileMode` のみを受け付けるため、
/// `mode` のみが反映され、`uid` / `gid` は反映できない。
#[derive(Debug, Clone)]
pub struct CopyTargetOptions {
    /// コピー先のパス。
    pub path: String,
    /// コピー先のファイルモード。
    pub mode: u32,
    /// コピー先のオーナー UID。
    ///
    /// macOS (XPC) では非対応。
    pub uid: u32,
    /// コピー先のグループ GID。
    ///
    /// macOS (XPC) では非対応。
    pub gid: u32,
}

impl CopyTargetOptions {
    /// パスのみを指定してデフォルト値を使う。
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            mode: 0o644,
            uid: 0,
            gid: 0,
        }
    }

    /// コピー先のファイルモードを設定する。
    pub fn with_mode(mut self, mode: u32) -> Self {
        self.mode = mode;
        self
    }

    /// コピー先のファイルモードを返す。
    ///
    /// 本家互換のため `Option<u32>` を返す。既定値を含む常に設定済みのため `Some` になる。
    pub fn mode(&self) -> Option<u32> {
        Some(self.mode)
    }
}

impl From<String> for CopyTargetOptions {
    fn from(path: String) -> Self {
        Self::new(path)
    }
}

impl From<&str> for CopyTargetOptions {
    fn from(path: &str) -> Self {
        Self::new(path)
    }
}

/// コンテナへコピーするファイル。
///
/// macOS (XPC) では `AsyncRunner::start` の `copy_to_sources` 処理で
/// XPC `containerCopyIn` ルートを使ってコピーされる。
/// Linux (Docker Engine API) では `copy_to_sources_linux` が `PUT /containers/{id}/archive` を
/// 自前 POSIX ustar (`docker_tar`) で叩き、単一 regular file を投入する。
#[derive(Debug, Clone)]
pub struct CopyToContainer {
    pub(crate) source: CopyDataSource,
    pub(crate) target: CopyTargetOptions,
}

impl CopyToContainer {
    pub fn new(source: impl Into<CopyDataSource>, target: impl Into<CopyTargetOptions>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_target_options_from_string_uses_defaults() {
        // String からの変換でデフォルトの mode / uid / gid が設定されること。
        let opts: CopyTargetOptions = "/data/hello.txt".to_string().into();
        assert_eq!(opts.path, "/data/hello.txt");
        assert_eq!(opts.mode, 0o644);
        assert_eq!(opts.uid, 0);
        assert_eq!(opts.gid, 0);
    }

    #[test]
    fn copy_target_options_from_str_uses_defaults() {
        // &str からの変換でデフォルトの mode / uid / gid が設定されること。
        let opts: CopyTargetOptions = "/data/hello.txt".into();
        assert_eq!(opts.path, "/data/hello.txt");
        assert_eq!(opts.mode, 0o644);
        assert_eq!(opts.uid, 0);
        assert_eq!(opts.gid, 0);
    }

    #[test]
    fn copy_target_options_with_mode_sets_field_and_accessor() {
        // with_mode がフィールドと mode() accessor の両方に反映されること。
        let opts = CopyTargetOptions::new("/data/secret.txt").with_mode(0o600);
        assert_eq!(opts.mode, 0o600);
        assert_eq!(opts.mode(), Some(0o600));
    }

    #[test]
    fn copy_to_container_accepts_string_target() {
        // CopyToContainer::new に String / &str を渡しても後方互換で動くこと。
        let from_string = CopyToContainer::new(
            CopyDataSource::File("/host/file.txt".into()),
            "/data/file.txt".to_string(),
        );
        assert_eq!(from_string.target.path, "/data/file.txt");
        assert_eq!(from_string.target.mode, 0o644);

        let from_str = CopyToContainer::new(
            CopyDataSource::File("/host/file.txt".into()),
            "/data/file.txt",
        );
        assert_eq!(from_str.target.path, "/data/file.txt");
        assert_eq!(from_str.target.mode, 0o644);
    }
}

/// `CopyToContainer` のエラー。
#[derive(Debug)]
pub enum CopyToContainerError {
    IoError(std::io::Error),
    PathNameError(String),
}

impl std::fmt::Display for CopyToContainerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyToContainerError::IoError(e) => write!(f, "I/O error: {e}"),
            CopyToContainerError::PathNameError(s) => {
                write!(f, "source is not a regular file: {s}")
            }
        }
    }
}

impl std::error::Error for CopyToContainerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CopyToContainerError::IoError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for CopyToContainerError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
    }
}

/// コンテナからのファイルコピー先。
///
/// macOS (XPC) では `ContainerAsync::copy_file_from` が `containerCopyOut` で一時ファイルへ
/// コピーし、そのリーダーをこのトレイトへ渡す。Linux (Docker Engine API) では
/// `GET /containers/{id}/archive` で取得した tar を自前 ustar パーサ (`docker_tar`) で展開し、
/// 先頭 regular file の内容を `Cursor` に載せてこのトレイトへ渡す。
pub trait CopyFileFromContainer: Sized + Send {
    type Output: Send;
    fn copy_from_reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
        self,
        reader: R,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = std::result::Result<Self::Output, CopyFromContainerError>,
                > + Send,
        >,
    >;
}

impl CopyFileFromContainer for PathBuf {
    type Output = ();

    fn copy_from_reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
        self,
        mut reader: R,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = std::result::Result<Self::Output, CopyFromContainerError>,
                > + Send,
        >,
    > {
        Box::pin(async move {
            let mut file = tokio::fs::File::create(&self).await?;
            tokio::io::copy(&mut reader, &mut file).await?;
            Ok(())
        })
    }
}

impl CopyFileFromContainer for Vec<u8> {
    type Output = Vec<u8>;

    fn copy_from_reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
        self,
        mut reader: R,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = std::result::Result<Self::Output, CopyFromContainerError>,
                > + Send,
        >,
    > {
        Box::pin(async move {
            let mut buf = self;
            buf.clear();
            tokio::io::copy(&mut reader, &mut buf).await?;
            Ok(buf)
        })
    }
}

/// `CopyFileFromContainer` のエラー。
#[derive(Debug)]
pub enum CopyFromContainerError {
    Io(std::io::Error),
    IsDirectory,
    EmptyArchive,
    UnsupportedEntry(&'static str),
}

impl std::fmt::Display for CopyFromContainerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyFromContainerError::Io(e) => write!(f, "I/O error: {e}"),
            CopyFromContainerError::IsDirectory => write!(f, "is a directory"),
            CopyFromContainerError::EmptyArchive => write!(f, "empty archive"),
            CopyFromContainerError::UnsupportedEntry(s) => write!(f, "unsupported entry type: {s}"),
        }
    }
}

impl std::error::Error for CopyFromContainerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CopyFromContainerError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for CopyFromContainerError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
