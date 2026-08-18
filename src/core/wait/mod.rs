//! 待機戦略。元の testcontainers 0.27 のサブセット（差分の正は `docs/TESTCONTAINERS.md` の該当節 10）。
//!
//! `Log` はログ FD を読んで待機する。macOS では利用可能。Linux ではログ FD が無く、
//! 非空メッセージ待ちは通常 startup timeout になる（空メッセージは即 Ok。詳細は docs 10.1）。
//! `Healthcheck` は Linux では inspect ポーリング (`starting` / `healthy` / `unhealthy` /
//! `Health` 不在 = running 後は `HealthCheckNotConfigured`)。macOS は現行通り常に
//! `HealthCheckNotConfigured` を返す（実装および docs 10.2）。
//! `Exit` は exit_code_hint（containerWait）と container_state（inspect / list）で待つ。
//! macOS / Linux とも同一ロジック（ポーリング間隔・startup_timeout 打ち切り）。

pub(crate) mod cmd_wait;
pub(crate) mod exit_strategy;
pub(crate) mod health_strategy;
#[cfg(feature = "http_wait_plain")]
pub(crate) mod http_strategy;
pub(crate) mod log_strategy;

pub use cmd_wait::CmdWaitFor;
pub use exit_strategy::ExitWaitStrategy;
pub use health_strategy::HealthWaitStrategy;
#[cfg(feature = "http_wait_plain")]
pub use http_strategy::{HttpResponse, HttpWaitError, HttpWaitStrategy};
pub use log_strategy::LogWaitStrategy;

use std::time::Duration;

use crate::{
    ContainerAsync, Image,
    core::{client::Client, error::Result},
};

/// コンテナが準備完了と見なされるための条件。
#[derive(Debug, Clone)]
pub enum WaitFor {
    /// 空の条件。
    Nothing,
    /// ログに特定メッセージが出るまで。
    Log(LogWaitStrategy),
    /// 指定時間待機。
    Duration {
        /// 待機時間。
        length: Duration,
    },
    /// ヘルスチェックが通過するまで。
    Healthcheck(HealthWaitStrategy),
    /// HTTP レスポンスが条件を満たすまで。
    #[cfg(feature = "http_wait_plain")]
    Http(Box<HttpWaitStrategy>),
    /// コンテナが終了するまで。
    Exit(ExitWaitStrategy),
}

impl WaitFor {
    /// stdout に指定メッセージが出るまで待機する条件を作る。
    pub fn message_on_stdout(message: impl AsRef<[u8]>) -> WaitFor {
        Self::log(LogWaitStrategy::new(
            crate::core::logs::LogSource::StdOut,
            message,
        ))
    }

    /// stderr に指定メッセージが出るまで待機する条件を作る。
    pub fn message_on_stderr(message: impl AsRef<[u8]>) -> WaitFor {
        Self::log(LogWaitStrategy::new(
            crate::core::logs::LogSource::StdErr,
            message,
        ))
    }

    /// stdout / stderr のどちらかにメッセージが出るまで待機する。
    pub fn message_on_either_std(message: impl AsRef<[u8]>) -> WaitFor {
        Self::log(LogWaitStrategy::new(
            crate::core::logs::LogSource::BothStd,
            message,
        ))
    }

    /// ログ待機戦略から条件を作る。
    pub fn log(log_strategy: LogWaitStrategy) -> WaitFor {
        WaitFor::Log(log_strategy)
    }

    /// HTTP レスポンスが条件を満たすまで待機する。
    ///
    /// # Feature
    ///
    /// この API は `http_wait_plain` feature が必要です。
    #[cfg(feature = "http_wait_plain")]
    pub fn http(http_strategy: HttpWaitStrategy) -> WaitFor {
        WaitFor::Http(Box::new(http_strategy))
    }

    /// ヘルスチェックが通過するまで待機する条件を作る。
    ///
    /// macOS (Apple container) は Docker HEALTHCHECK 相当を実装していないため、
    /// 常に `HealthCheckNotConfigured` エラーを返す。
    pub fn healthcheck() -> WaitFor {
        WaitFor::Healthcheck(HealthWaitStrategy::default())
    }

    /// コンテナが終了するまで待機する条件を作る。
    pub fn exit(exit_strategy: ExitWaitStrategy) -> WaitFor {
        WaitFor::Exit(exit_strategy)
    }

    /// 指定秒数だけ待機する条件を作る。
    pub fn seconds(length: u64) -> WaitFor {
        WaitFor::Duration {
            length: Duration::from_secs(length),
        }
    }

    /// 指定ミリ秒だけ待機する条件を作る。
    pub fn millis(length: u64) -> WaitFor {
        WaitFor::Duration {
            length: Duration::from_millis(length),
        }
    }

    /// 環境変数からミリ秒を読み取り、その時間だけ待機する条件を作る。
    ///
    /// 環境変数が未設定または不正値の場合は `WaitFor::Nothing` を返す。
    pub fn millis_in_env_var(name: &'static str) -> WaitFor {
        let additional_sleep_period = std::env::var(name).map(|value| value.parse());

        (|| {
            let length = additional_sleep_period.ok()?.ok()?;

            Some(WaitFor::Duration {
                length: Duration::from_millis(length),
            })
        })()
        .unwrap_or(WaitFor::Nothing)
    }
}

#[cfg(feature = "http_wait_plain")]
impl From<HttpWaitStrategy> for WaitFor {
    fn from(value: HttpWaitStrategy) -> Self {
        Self::Http(Box::new(value))
    }
}

impl WaitFor {
    /// 条件が満たされるまで待機する。
    pub(crate) async fn wait_until_ready<I: Image>(
        self,
        client: &Client,
        container: &ContainerAsync<I>,
    ) -> Result<()> {
        match self {
            WaitFor::Log(strategy) => strategy.wait_until_ready(client, container).await?,
            WaitFor::Duration { length } => {
                tokio::time::sleep(length).await;
            }
            WaitFor::Healthcheck(strategy) => {
                strategy.wait_until_ready(client, container).await?;
            }
            #[cfg(feature = "http_wait_plain")]
            WaitFor::Http(strategy) => {
                strategy.wait_until_ready(client, container).await?;
            }
            WaitFor::Exit(strategy) => {
                strategy.wait_until_ready(client, container).await?;
            }
            WaitFor::Nothing => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT_MILLIS_ENV: &str = "SHIGUREDO_CONTAINER_WAIT_MILLIS_TEST";
    const WAIT_MILLIS_CHILD_ENV: &str = "SHIGUREDO_CONTAINER_WAIT_MILLIS_TEST_CHILD";

    fn run_env_case(value: Option<&str>) {
        // 環境変数はプロセスグローバルなので、子プロセスの起動前にだけ設定する。
        let executable = std::env::current_exe().expect("テストバイナリのパスを取得できること");
        let mut command = std::process::Command::new(executable);
        command.args(["--exact", "core::wait::tests::millis_in_env_var_child"]);
        command.env(WAIT_MILLIS_CHILD_ENV, "1");
        match value {
            Some(value) => {
                command.env(WAIT_MILLIS_ENV, value);
            }
            None => {
                command.env_remove(WAIT_MILLIS_ENV);
            }
        }
        let status = command
            .status()
            .expect("環境変数を検証する子テストを起動できること");
        assert!(status.success(), "子テストが成功すること: {status}");
    }

    #[test]
    fn millis_in_env_var_handles_missing_invalid_and_valid_values() {
        // 未設定・不正値では Nothing、整数ミリ秒では Duration になることを subprocess で検証する。
        run_env_case(None);
        run_env_case(Some("invalid"));
        run_env_case(Some("125"));
    }

    #[test]
    fn millis_in_env_var_child() {
        if std::env::var_os(WAIT_MILLIS_CHILD_ENV).is_none() {
            return;
        }
        match std::env::var(WAIT_MILLIS_ENV).ok().as_deref() {
            Some("125") => {
                assert!(matches!(
                    WaitFor::millis_in_env_var(WAIT_MILLIS_ENV),
                    WaitFor::Duration { length } if length == Duration::from_millis(125)
                ));
            }
            None | Some("invalid") => {
                assert!(matches!(
                    WaitFor::millis_in_env_var(WAIT_MILLIS_ENV),
                    WaitFor::Nothing
                ));
            }
            Some(value) => panic!("想定外の子プロセス環境変数値: {value}"),
        }
    }

    #[cfg(feature = "http_wait_plain")]
    #[test]
    fn wait_for_from_http_wait_strategy() {
        // From<HttpWaitStrategy> が WaitFor::Http に変換すること。
        let strategy = HttpWaitStrategy::new("/health");
        let wait = WaitFor::from(strategy);
        assert!(matches!(wait, WaitFor::Http(_)));
    }
}
