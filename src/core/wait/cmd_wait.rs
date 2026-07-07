//! exec コマンドの待機条件。

use std::time::Duration;

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum CmdWaitFor {
    Nothing,
    StdOutMessage {
        message: Vec<u8>,
    },
    StdErrMessage {
        message: Vec<u8>,
    },
    Duration {
        length: Duration,
    },
    /// コマンドの終了を待つ。`code` が `Some` なら終了コードの一致も検証する。
    Exit {
        code: Option<i64>,
    },
}

impl CmdWaitFor {
    pub fn message_on_stdout(message: impl AsRef<[u8]>) -> Self {
        Self::StdOutMessage {
            message: message.as_ref().to_vec(),
        }
    }

    pub fn message_on_stderr(message: impl AsRef<[u8]>) -> Self {
        Self::StdErrMessage {
            message: message.as_ref().to_vec(),
        }
    }

    /// コマンドの終了を待つ (終了コードは不問)。
    pub fn exit() -> Self {
        Self::Exit { code: None }
    }

    /// コマンドが指定の終了コードで終了するのを待つ。
    pub fn exit_code(code: i64) -> Self {
        Self::Exit { code: Some(code) }
    }

    pub fn seconds(length: u64) -> Self {
        Self::Duration {
            length: Duration::from_secs(length),
        }
    }

    pub fn millis(length: u64) -> Self {
        Self::Duration {
            length: Duration::from_millis(length),
        }
    }
}
