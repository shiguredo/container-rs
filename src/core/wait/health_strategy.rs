//! ヘルスチェック待機戦略。元の testcontainers 0.27 の `core::wait::health_strategy` と同一シグネチャ。
//!
//! Linux は inspect をポーリングし、`starting` / `healthy` / `unhealthy` /
//! `Health` 不在 (running 後は `HealthCheckNotConfigured`) の 4 分岐で判定する。
//! macOS は現行通り常に `HealthCheckNotConfigured` を返す。

use std::time::Duration;

use crate::{
    ContainerAsync, Image,
    core::{client::Client, error::Result},
};

#[derive(Debug, Clone)]
pub struct HealthWaitStrategy {
    poll_interval: Duration,
}

impl HealthWaitStrategy {
    pub fn new() -> Self {
        Self {
            poll_interval: Duration::from_millis(100),
        }
    }

    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }
}

impl Default for HealthWaitStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthWaitStrategy {
    pub(crate) async fn wait_until_ready<I: Image>(
        self,
        client: &Client,
        container: &ContainerAsync<I>,
    ) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let _ = (client, self.poll_interval);
            // Apple container は Docker HEALTHCHECK 相当を実行・公開しない。
            Err(
                crate::core::error::WaitContainerError::HealthCheckNotConfigured(
                    container.id().to_string(),
                )
                .into(),
            )
        }

        #[cfg(target_os = "linux")]
        {
            use crate::core::client::HealthStatus;
            use crate::core::error::WaitContainerError;

            // cfg で片方のアームが消えるため clippy::infallible_destructuring_match を抑制する。
            #[expect(clippy::infallible_destructuring_match)]
            let docker = match client {
                Client::Linux(c) => c,
                #[cfg(target_os = "macos")]
                Client::MacOs(_) => unreachable!("Linux block is not compiled on macOS"),
            };
            let id = container.id().to_string();
            let mut seen_running = false;
            loop {
                let probe = docker.container_health(&id).await?;
                if probe.running {
                    seen_running = true;
                }
                match probe.health {
                    Some(HealthStatus::Healthy) => return Ok(()),
                    Some(HealthStatus::Unhealthy) => {
                        return Err(WaitContainerError::Unhealthy(id).into());
                    }
                    Some(HealthStatus::Starting) => {
                        // Docker が判定中。次の tick へ。
                    }
                    None => {
                        if seen_running {
                            // running 観測後に Health 不在 = healthcheck 未設定と確定。
                            return Err(WaitContainerError::HealthCheckNotConfigured(id).into());
                        }
                        // まだ running を見ていない間は daemon 初期化ラグの可能性が高い。
                    }
                }
                tokio::time::sleep(self.poll_interval).await;
            }
        }
    }
}
