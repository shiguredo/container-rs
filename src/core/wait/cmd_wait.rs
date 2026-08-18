//! exec コマンドの待機条件。

use std::time::Duration;

/// exec コマンドの完了を待つ条件。
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum CmdWaitFor {
    /// 空の条件。
    Nothing,
    /// stdout に特定メッセージが出るまで。
    StdOutMessage {
        /// 待機対象のメッセージ。
        message: Vec<u8>,
    },
    /// stderr に特定メッセージが出るまで。
    StdErrMessage {
        /// 待機対象のメッセージ。
        message: Vec<u8>,
    },
    /// 指定時間待機。
    Duration {
        /// 待機時間。
        length: Duration,
    },
    /// コマンドの終了を待つ。`code` が `Some` なら終了コードの一致も検証する。
    Exit {
        /// 期待する終了コード。`None` の場合は終了コードを検証しない。
        code: Option<i64>,
    },
}

impl CmdWaitFor {
    /// stdout に指定メッセージが出るまで待機する条件を作る。
    pub fn message_on_stdout(message: impl AsRef<[u8]>) -> Self {
        Self::StdOutMessage {
            message: message.as_ref().to_vec(),
        }
    }

    /// stderr に指定メッセージが出るまで待機する条件を作る。
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

    /// 指定秒数だけ待機する条件を作る。
    pub fn seconds(length: u64) -> Self {
        Self::Duration {
            length: Duration::from_secs(length),
        }
    }

    /// 指定ミリ秒だけ待機する条件を作る。
    pub fn millis(length: u64) -> Self {
        Self::Duration {
            length: Duration::from_millis(length),
        }
    }
}
