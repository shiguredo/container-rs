//! ログ関連の型。
//!
//! macOS (XPC) では、`containerLogs` が返す FD を `ContainerAsync::stdout` と
//! `ContainerAsync::stderr` から読み取る。

pub(crate) mod consumer;

pub use consumer::{LogConsumer, LoggingConsumer};

/// ログの 1 フレーム。stdout または stderr。
#[derive(Debug, Clone)]
pub enum LogFrame {
    StdOut(Vec<u8>),
    StdErr(Vec<u8>),
}

/// ログの出力元。
#[derive(Copy, Clone, Debug)]
pub enum LogSource {
    StdOut,
    StdErr,
    /// stdout / stderr の両方。
    BothStd,
}

impl LogFrame {
    pub fn source(&self) -> LogSource {
        match self {
            LogFrame::StdOut(_) => LogSource::StdOut,
            LogFrame::StdErr(_) => LogSource::StdErr,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            LogFrame::StdOut(bytes) => bytes,
            LogFrame::StdErr(bytes) => bytes,
        }
    }
}
