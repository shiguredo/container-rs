//! コンテナランタイムクライアント。
//!
//! macOS では Apple Container XPC、Ubuntu では Docker Engine API を使う。
//! このモジュールは OS ごとのクライアントを統合し、呼び出し側は特定のランタイムを意識しない。

use std::sync::Arc;

#[cfg(target_os = "macos")]
pub(crate) mod container_cfg;
#[cfg(target_os = "linux")]
pub(crate) mod docker_client;
#[cfg(target_os = "linux")]
pub(crate) mod docker_log_stream;
#[cfg(target_os = "linux")]
pub(crate) mod docker_tar;
pub(crate) mod http_decode;
#[cfg(target_os = "macos")]
pub(crate) mod image_config;
#[cfg(target_os = "macos")]
pub(crate) mod xpc_client;

#[cfg(target_os = "linux")]
pub(crate) use docker_client::DockerClient;
#[cfg(target_os = "macos")]
pub(crate) use xpc_client::XpcClient;

#[cfg(target_os = "linux")]
use std::collections::BTreeMap;

use crate::core::error::Result;
#[cfg(target_os = "linux")]
use crate::core::healthcheck::Healthcheck;
#[cfg(target_os = "linux")]
use crate::core::mounts::Mount;
use crate::core::ports::Ports;

/// コンテナの状態 snapshot。`XpcClient::container_state` / `DockerClient::container_state` の結果。
#[derive(Debug, Clone, Default)]
pub(crate) struct ContainerSnapshot {
    pub(crate) running: bool,
    pub(crate) ports: Ports,
}

/// Docker Engine の `State.Health.Status` を表す。
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HealthStatus {
    Starting,
    Healthy,
    Unhealthy,
}

/// inspect から取り出した running と health 状態。
#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
pub(crate) struct HealthProbe {
    pub(crate) running: bool,
    pub(crate) health: Option<HealthStatus>,
}

/// Docker API 用のコンテナ設定。
#[cfg(target_os = "linux")]
pub(crate) struct ContainerConfig {
    pub image: String,
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub ports: Vec<crate::core::containers::request::PortMapping>,
    pub mounts: Vec<Mount>,
    pub name: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub privileged: bool,
    pub working_dir: Option<String>,
    pub user: Option<String>,
    pub init: bool,
    pub health_check: Option<Healthcheck>,
}

/// macOS / Linux のコンテナクライアントを統合した内部型。
#[derive(Clone)]
pub(crate) enum Client {
    #[cfg(target_os = "macos")]
    MacOs(Arc<XpcClient>),
    #[cfg(target_os = "linux")]
    Linux(Arc<DockerClient>),
}

impl Client {
    /// 実行中の OS に応じたクライアントを返す。
    ///
    /// 現状は macOS / Linux とも常に `Ok` を返す（Linux の `DockerClient::detect` も常に `Ok`）。
    /// `Result` は Linux 側の `DockerClient::detect()?` 伝播と、OS 間でシグネチャを揃えるため。
    pub fn detect() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self::MacOs(Arc::new(XpcClient)))
        }
        #[cfg(target_os = "linux")]
        {
            Ok(Self::Linux(Arc::new(DockerClient::detect()?)))
        }
    }
}
