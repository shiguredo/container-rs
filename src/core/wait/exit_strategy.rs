//! 終了待機戦略。元の 0.27 の `core::wait::exit_strategy` と同一シグネチャ。

use std::time::Duration;

use crate::{
    ContainerAsync, Image,
    core::{client::Client, error::Result},
};

#[derive(Debug, Clone)]
pub struct ExitWaitStrategy {
    expected_code: Option<i64>,
    poll_interval: Duration,
}

impl ExitWaitStrategy {
    pub fn new() -> Self {
        Self {
            expected_code: None,
            poll_interval: Duration::from_millis(100),
        }
    }

    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    pub fn with_exit_code(mut self, expected_code: i64) -> Self {
        self.expected_code = Some(expected_code);
        self
    }
}

impl Default for ExitWaitStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl ExitWaitStrategy {
    pub(crate) async fn wait_until_ready<I: Image>(
        self,
        client: &Client,
        container: &ContainerAsync<I>,
    ) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let _ = (client, container);
            Err(crate::core::error::Error::other(
                "ExitWaitStrategy is not implemented on Linux",
            ))
        }

        #[cfg(target_os = "macos")]
        loop {
            // containerList (container_state) は exit code を返さないため、
            // バックグラウンドの containerWait が観測した値で判定する。
            if let Some(actual) = container.exit_code_hint() {
                if let Some(expected_code) = self.expected_code
                    && actual != expected_code
                {
                    return Err(crate::core::error::WaitContainerError::UnexpectedExitCode {
                        expected: expected_code,
                        actual: Some(actual),
                    }
                    .into());
                }
                return Ok(());
            }

            let state = match client {
                Client::MacOs(c) => c.container_state(container.id()).await?,
            };

            // exit code の検証が不要なら停止の観測だけで完了。検証が必要な場合は
            // containerWait が exit code を書き込むまで待つ (startup_timeout で打ち切られる)。
            if !state.running && self.expected_code.is_none() {
                return Ok(());
            }

            tokio::time::sleep(self.poll_interval).await;
        }
    }
}
