//! 終了待機戦略。元の 0.27 の `core::wait::exit_strategy` と同一シグネチャ。

use std::time::Duration;

use crate::{
    ContainerAsync, Image,
    core::{client::Client, error::Result},
};

/// コンテナの終了を待つ戦略。
///
/// バックグラウンドの `containerWait` が観測した exit code と
/// `container_state` のポーリングで終了を判定する。
#[derive(Debug, Clone)]
pub struct ExitWaitStrategy {
    expected_code: Option<i64>,
    poll_interval: Duration,
}

impl ExitWaitStrategy {
    /// デフォルト設定 (exit code 不問、100ms ポーリング) で戦略を作る。
    pub fn new() -> Self {
        Self {
            expected_code: None,
            poll_interval: Duration::from_millis(100),
        }
    }

    /// ポーリング間隔を設定する。
    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    /// 期待する exit code を設定する。不一致の場合はエラーになる。
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
                #[cfg(target_os = "macos")]
                Client::MacOs(c) => c.container_state(container.id()).await?,
                #[cfg(target_os = "linux")]
                Client::Linux(c) => c.container_state(container.id()).await?,
            };

            // exit code の検証が不要なら停止の観測だけで完了。検証が必要な場合は
            // containerWait が exit code を書き込むまで待つ (startup_timeout で打ち切られる)。
            if !state.running && self.expected_code.is_none() {
                return Ok(());
            }

            // macOS は停止観測後に `exit_code()` の都度取得フォールバックを 1 回試す。
            // バックグラウンド wait の記録が XPC 障害等で無い環境でも、期待コードで
            // 正常終了していれば判定できる。取得できなければ `UnexpectedExitCode`
            // (actual: None) で明示エラーにする (StartupTimeout と誤診断させない)。
            // フォールバックの 5 秒は startup_timeout の残り予算を消費するため、
            // 残り予算が 5 秒未満の場合は外側の timeout が先に発火する。
            //
            // `exit_code()` は世代チェック付きの都度取得に倒れるため、都度 wait の間に
            // 再 start が挟まっても新世代の値で判定されることはない (世代不一致は
            // `Ok(None)` になる)。None を受け取ったら後続のバックグラウンド wait を
            // 待たず、確認できない旨の明示エラーに倒すのが安全側の挙動。
            #[cfg(target_os = "macos")]
            if !state.running {
                // 上記のガード (exit code 不問の停止は即完了) により、ここでは必ず
                // 期待コードが設定されている。
                let Some(expected) = self.expected_code else {
                    return Ok(());
                };
                return match container.exit_code().await? {
                    Some(actual) if actual != expected => {
                        Err(crate::core::error::WaitContainerError::UnexpectedExitCode {
                            expected,
                            actual: Some(actual),
                        }
                        .into())
                    }
                    Some(_) => Ok(()),
                    None => Err(crate::core::error::WaitContainerError::UnexpectedExitCode {
                        expected,
                        actual: None,
                    }
                    .into()),
                };
            }

            tokio::time::sleep(self.poll_interval).await;
        }
    }
}
