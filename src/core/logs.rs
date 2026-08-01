//! ログ関連の型。
//!
//! macOS (XPC) では、`containerLogs` が返す FD を `ContainerAsync::stdout` と
//! `ContainerAsync::stderr` から読み取る。

pub(crate) mod consumer;

pub use consumer::{LogConsumer, LoggingConsumer};

/// ログの 1 フレーム。stdout または stderr。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogFrame {
    /// 標準出力のログフレーム。
    StdOut(Vec<u8>),
    /// 標準エラー出力のログフレーム。
    StdErr(Vec<u8>),
}

/// ログの出力元。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LogSource {
    /// 標準出力。
    StdOut,
    /// 標準エラー出力。
    StdErr,
    /// stdout / stderr の両方。
    BothStd,
}

impl LogFrame {
    /// このフレームの出力元を返す。
    pub fn source(&self) -> LogSource {
        match self {
            LogFrame::StdOut(_) => LogSource::StdOut,
            LogFrame::StdErr(_) => LogSource::StdErr,
        }
    }

    /// ログのバイト列を返す。
    pub fn bytes(&self) -> &[u8] {
        match self {
            LogFrame::StdOut(bytes) => bytes,
            LogFrame::StdErr(bytes) => bytes,
        }
    }
}
