//! `ExecResult` — exec の結果。元の 0.27 の `core::containers::async_container::exec` と同一シグネチャ。
//!
//! macOS (XPC) では `containerCreateProcess` に pipe FD を渡して stdout / stderr を取得する。
//! 本家はログストリームを追いかけるリーダーを返すが、shiguredo は exec 完了時点で
//! 全出力を取得済みのため、バッファ上のリーダーを返す。読み出しは本家と同様に
//! 消費型 (一度読んだ分は次回の読み出しに含まれない)。

use std::{fmt, io::Cursor, pin::Pin};

use tokio::io::{AsyncBufRead, AsyncReadExt};

use crate::core::error::Result;

/// exec の結果。
pub struct ExecResult {
    pub(crate) exit_code: Option<i64>,
    pub(crate) stdout: Cursor<Vec<u8>>,
    pub(crate) stderr: Cursor<Vec<u8>>,
}

impl ExecResult {
    /// 終了コードを返す。コマンドがまだ終了していない場合は `None`。
    pub async fn exit_code(&self) -> Result<Option<i64>> {
        Ok(self.exit_code)
    }

    /// stdout の非同期リーダーを返す。
    pub fn stdout<'b>(&'b mut self) -> Pin<Box<dyn AsyncBufRead + Send + 'b>> {
        Box::pin(&mut self.stdout)
    }

    /// stderr の非同期リーダーを返す。
    pub fn stderr<'b>(&'b mut self) -> Pin<Box<dyn AsyncBufRead + Send + 'b>> {
        Box::pin(&mut self.stderr)
    }

    /// stdout を `Vec<u8>` で返す。
    pub async fn stdout_to_vec(&mut self) -> Result<Vec<u8>> {
        let mut stdout = Vec::new();
        self.stdout().read_to_end(&mut stdout).await?;
        Ok(stdout)
    }

    /// stderr を `Vec<u8>` で返す。
    pub async fn stderr_to_vec(&mut self) -> Result<Vec<u8>> {
        let mut stderr = Vec::new();
        self.stderr().read_to_end(&mut stderr).await?;
        Ok(stderr)
    }
}

impl fmt::Debug for ExecResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecResult")
            .field("exit_code", &self.exit_code)
            .finish()
    }
}
