//! `Image` トレイトと `ImageExt` 拡張トレイト。
//! 元の 0.27 の `core::image` と同一シグネチャ。

pub mod exec;
pub mod image_ext;

pub use exec::ExecCommand;
pub use image_ext::ImageExt;

use std::borrow::Cow;

use crate::core::host::Host;
use crate::{
    ContainerAsync, Error,
    core::{
        WaitFor,
        copy::CopyToContainer,
        error::Result,
        mounts::Mount,
        ports::{ContainerPort, Ports},
    },
};

/// Docker イメージを表すトレイト。
pub trait Image
where
    Self: Sized + Sync + Send,
{
    /// イメージ名。
    fn name(&self) -> &str;

    /// タグ。
    fn tag(&self) -> &str;

    /// コンテナが準備完了と見なされるための条件リスト。
    fn ready_conditions(&self) -> Vec<WaitFor>;

    /// 環境変数。
    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        std::iter::empty::<(String, String)>()
    }

    /// マウント。
    fn mounts(&self) -> impl IntoIterator<Item = &Mount> {
        std::iter::empty()
    }

    /// 起動時にコンテナへコピーするファイル。
    fn copy_to_sources(&self) -> impl IntoIterator<Item = &CopyToContainer> {
        std::iter::empty()
    }

    /// entrypoint。
    fn entrypoint(&self) -> Option<&str> {
        None
    }

    /// CMD。
    fn cmd(&self) -> impl IntoIterator<Item = impl Into<Cow<'_, str>>> {
        std::iter::empty::<String>()
    }

    /// 公開ポート。
    fn expose_ports(&self) -> &[ContainerPort] {
        &[]
    }

    /// 起動後に実行するコマンド。
    fn exec_after_start(&self, _cs: ContainerState) -> Result<Vec<ExecCommand>> {
        Ok(Default::default())
    }

    /// 起動後・準備完了待機前に実行するコマンド。
    fn exec_before_ready(&self, _cs: ContainerState) -> Result<Vec<ExecCommand>> {
        Ok(Default::default())
    }
}

/// コンテナの状態 snapshot。
#[derive(Debug)]
pub struct ContainerState {
    id: String,
    host: Host,
    ports: Ports,
}

impl ContainerState {
    pub(crate) fn new(id: String, host: Host, ports: Ports) -> Self {
        Self { id, host, ports }
    }

    /// 実行中コンテナから状態 snapshot を取得する。
    ///
    /// 本家 testcontainers-rs 0.27 と同じシグネチャ。
    pub async fn from_container<I: Image>(container: &ContainerAsync<I>) -> Result<Self> {
        container.container_state().await
    }

    /// コンテナ ID を返す。
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    pub fn host_port_ipv4(&self, internal_port: ContainerPort) -> Result<u16> {
        self.ports
            .map_to_host_port_ipv4(internal_port)
            .ok_or_else(|| Error::PortNotExposed {
                id: self.id.clone(),
                port: internal_port,
            })
    }

    pub fn host_port_ipv6(&self, internal_port: ContainerPort) -> Result<u16> {
        self.ports
            .map_to_host_port_ipv6(internal_port)
            .ok_or_else(|| Error::PortNotExposed {
                id: self.id.clone(),
                port: internal_port,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::host::Host;
    use crate::core::ports::Ports;

    /// `ContainerState::new` に渡した ID が `id()` で取得できること。
    #[test]
    fn container_state_id_returns_constructed_value() {
        let state = ContainerState::new(
            "test-container-id".to_string(),
            Host::parse("127.0.0.1"),
            Ports::default(),
        );
        assert_eq!(
            state.id(),
            "test-container-id",
            "構築時に渡したコンテナ ID が取得できること"
        );
    }
}
