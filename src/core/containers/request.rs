//! `ContainerRequest` と関連型。
//! testcontainers-rs 0.27 のサブセット (差分の正は `docs/TESTCONTAINERS.md`)。

use std::{
    borrow::Cow,
    collections::BTreeMap,
    fmt::{Debug, Formatter},
    net::IpAddr,
    time::Duration,
};

use crate::{
    Error, Image,
    core::{
        ContainerState, copy::CopyToContainer, healthcheck::Healthcheck, image::exec::ExecCommand,
        logs::consumer::LogConsumer, mounts::Mount, ports::ContainerPort, wait::WaitFor,
    },
};

/// コンテナ起動のリクエスト。`Image` に設定を重ねて作る。
#[must_use]
pub struct ContainerRequest<I: Image> {
    pub(crate) image: I,
    pub(crate) overridden_cmd: Vec<String>,
    pub(crate) image_name: Option<String>,
    pub(crate) image_tag: Option<String>,
    pub(crate) container_name: Option<String>,
    pub(crate) hostname: Option<String>,
    pub(crate) network: Option<String>,
    pub(crate) labels: BTreeMap<String, String>,
    pub(crate) env_vars: BTreeMap<String, String>,
    pub(crate) hosts: BTreeMap<String, ExtraHost>,
    pub(crate) mounts: Vec<Mount>,
    pub(crate) health_check: Option<Healthcheck>,
    pub(crate) copy_to_sources: Vec<CopyToContainer>,
    pub(crate) ports: Option<Vec<PortMapping>>,
    pub(crate) privileged: bool,
    pub(crate) readonly_rootfs: bool,
    pub(crate) cap_add: Option<Vec<String>>,
    pub(crate) cap_drop: Option<Vec<String>>,
    pub(crate) shm_size: Option<u64>,
    pub(crate) ready_conditions: Option<Vec<WaitFor>>,
    pub(crate) startup_timeout: Option<Duration>,
    pub(crate) working_dir: Option<String>,
    pub(crate) user: Option<String>,
    pub(crate) open_stdin: Option<bool>,
    pub(crate) log_consumers: Vec<Box<dyn LogConsumer + 'static>>,
    pub(crate) init: bool,
    pub(crate) platform: Option<String>,
    pub(crate) ssh: bool,
}

/// ポートマッピング。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortMapping {
    pub(crate) host_port: u16,
    pub(crate) container_port: ContainerPort,
}

/// extra_hosts 用のホスト指定。
#[derive(Debug, Clone, Copy)]
pub enum ExtraHost {
    Addr(IpAddr),
    HostGateway,
}

impl std::fmt::Display for ExtraHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtraHost::Addr(a) => write!(f, "{a}"),
            ExtraHost::HostGateway => write!(f, "host-gateway"),
        }
    }
}

/// cgroup namespace モード。macOS では未使用だが本家 API に合わせて定義。
#[derive(Debug, Clone, Copy)]
pub enum CgroupnsMode {
    Host,
    Private,
}

impl<I: Image> ContainerRequest<I> {
    pub fn image(&self) -> &I {
        &self.image
    }

    pub fn network(&self) -> &Option<String> {
        &self.network
    }

    pub fn labels(&self) -> &BTreeMap<String, String> {
        &self.labels
    }

    pub fn container_name(&self) -> &Option<String> {
        &self.container_name
    }

    pub fn hostname(&self) -> Option<&str> {
        self.hostname.as_deref()
    }

    pub fn env_vars(&self) -> impl Iterator<Item = (Cow<'_, str>, Cow<'_, str>)> {
        self.image
            .env_vars()
            .into_iter()
            .map(|(name, val)| (name.into(), val.into()))
            .chain(
                self.env_vars
                    .iter()
                    .map(|(name, val)| (name.into(), val.into())),
            )
    }

    pub fn hosts(&self) -> impl Iterator<Item = (Cow<'_, str>, &ExtraHost)> {
        self.hosts.iter().map(|(name, host)| (name.into(), host))
    }

    pub fn mounts(&self) -> impl Iterator<Item = &Mount> {
        self.image.mounts().into_iter().chain(self.mounts.iter())
    }

    pub fn health_check(&self) -> Option<&Healthcheck> {
        self.health_check.as_ref()
    }

    pub fn copy_to_sources(&self) -> impl Iterator<Item = &CopyToContainer> {
        self.image
            .copy_to_sources()
            .into_iter()
            .chain(self.copy_to_sources.iter())
    }

    pub fn ports(&self) -> Option<&Vec<PortMapping>> {
        self.ports.as_ref()
    }

    pub fn privileged(&self) -> bool {
        self.privileged
    }

    pub fn readonly_rootfs(&self) -> bool {
        self.readonly_rootfs
    }

    pub fn cap_add(&self) -> Option<&Vec<String>> {
        self.cap_add.as_ref()
    }

    pub fn cap_drop(&self) -> Option<&Vec<String>> {
        self.cap_drop.as_ref()
    }

    pub fn shm_size(&self) -> Option<u64> {
        self.shm_size
    }

    pub fn entrypoint(&self) -> Option<&str> {
        self.image.entrypoint()
    }

    pub fn cmd(&self) -> impl Iterator<Item = Cow<'_, str>> {
        // `either` クレートに依存せず、自前で切り替える。
        // `overridden_cmd` が空なら `image.cmd()` を使う。
        if !self.overridden_cmd.is_empty() {
            let front: Vec<Cow<'_, str>> = self.overridden_cmd.iter().map(Cow::from).collect();
            CmdIter {
                front: front.into_iter(),
                back: Vec::new().into_iter(),
            }
        } else {
            let back: Vec<Cow<'_, str>> = self.image.cmd().into_iter().map(Into::into).collect();
            CmdIter {
                front: Vec::new().into_iter(),
                back: back.into_iter(),
            }
        }
    }

    pub fn descriptor(&self) -> String {
        let original_name = self.image.name();
        let original_tag = self.image.tag();

        let name = self.image_name.as_deref().unwrap_or(original_name);
        let tag = self.image_tag.as_deref().unwrap_or(original_tag);

        format!("{name}:{tag}")
    }

    pub fn ready_conditions(&self) -> Vec<WaitFor> {
        self.ready_conditions
            .clone()
            .unwrap_or_else(|| self.image.ready_conditions())
    }

    pub fn expose_ports(&self) -> &[ContainerPort] {
        self.image.expose_ports()
    }

    pub fn exec_after_start(
        &self,
        cs: ContainerState,
    ) -> std::result::Result<Vec<ExecCommand>, Error> {
        self.image.exec_after_start(cs)
    }

    pub fn startup_timeout(&self) -> Option<Duration> {
        self.startup_timeout
    }

    pub fn working_dir(&self) -> Option<&str> {
        self.working_dir.as_deref()
    }

    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    pub fn open_stdin(&self) -> Option<bool> {
        self.open_stdin
    }

    pub fn init(&self) -> bool {
        self.init
    }

    pub fn platform(&self) -> &Option<String> {
        &self.platform
    }

    pub fn ssh(&self) -> bool {
        self.ssh
    }
}

impl<I: Image> From<I> for ContainerRequest<I> {
    fn from(image: I) -> Self {
        Self {
            image,
            overridden_cmd: Vec::new(),
            image_name: None,
            image_tag: None,
            container_name: None,
            hostname: None,
            network: None,
            labels: BTreeMap::default(),
            env_vars: BTreeMap::default(),
            hosts: BTreeMap::default(),
            mounts: Vec::new(),
            health_check: None,
            copy_to_sources: Vec::new(),
            ports: None,
            privileged: false,
            readonly_rootfs: false,
            cap_add: None,
            cap_drop: None,
            shm_size: None,
            ready_conditions: None,
            startup_timeout: None,
            working_dir: None,
            user: None,
            open_stdin: None,
            log_consumers: vec![],
            init: false,
            platform: None,
            ssh: false,
        }
    }
}

impl PortMapping {
    pub(crate) fn new(local: u16, internal: ContainerPort) -> Self {
        Self {
            host_port: local,
            container_port: internal,
        }
    }

    pub fn host_port(&self) -> u16 {
        self.host_port
    }

    pub fn container_port(&self) -> ContainerPort {
        self.container_port
    }
}

impl<I: Image + Debug> Debug for ContainerRequest<I> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut repr = f.debug_struct("ContainerRequest");
        repr.field("image", &self.image)
            .field("overridden_cmd", &self.overridden_cmd)
            .field("image_name", &self.image_name)
            .field("image_tag", &self.image_tag)
            .field("container_name", &self.container_name)
            .field("hostname", &self.hostname)
            .field("network", &self.network)
            .field("labels", &self.labels)
            .field("env_vars", &self.env_vars)
            .field("hosts", &self.hosts)
            .field("mounts", &self.mounts)
            .field("health_check", &self.health_check)
            .field("ports", &self.ports)
            .field("privileged", &self.privileged)
            .field("readonly_rootfs", &self.readonly_rootfs)
            .field("cap_add", &self.cap_add)
            .field("cap_drop", &self.cap_drop)
            .field("shm_size", &self.shm_size)
            .field("startup_timeout", &self.startup_timeout)
            .field("working_dir", &self.working_dir)
            .field("user", &self.user)
            .field("open_stdin", &self.open_stdin)
            .field("init", &self.init)
            .field("platform", &self.platform)
            .field("ssh", &self.ssh);
        repr.finish()
    }
}

/// `cmd` のイテレーター。`overridden_cmd` が空なら `image.cmd()` に切り替える。
/// `either` クレートに依存しないための自前実装。
pub(crate) struct CmdIter<'a> {
    pub(crate) front: std::vec::IntoIter<Cow<'a, str>>,
    pub(crate) back: std::vec::IntoIter<Cow<'a, str>>,
}

impl<'a> Iterator for CmdIter<'a> {
    type Item = Cow<'a, str>;

    fn next(&mut self) -> Option<Self::Item> {
        self.front.next().or_else(|| self.back.next())
    }
}
