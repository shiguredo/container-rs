//! ヘルスチェック待機戦略。元の testcontainers 0.27 の `core::wait::health_strategy` と同一シグネチャ。
//!
//! wait_until_ready は macOS / Linux とも未実装で、即座に `HealthCheckNotConfigured` を返す
//! （実装および `docs/TESTCONTAINERS.md` 10.2）。

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
        _client: &Client,
        container: &ContainerAsync<I>,
    ) -> Result<()> {
        // macOS / Linux ともヘルスチェック待機は未実装のため即座にエラーにする。
        // コンテナが "running" でもヘルスチェック通過とは別物。
        Err(
            crate::core::error::WaitContainerError::HealthCheckNotConfigured(
                container.id().to_string(),
            )
            .into(),
        )
    }
}
