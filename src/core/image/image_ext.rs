//! `ImageExt` 拡張トレイト。元の testcontainers 0.27 のサブセット（差分の正は `docs/TESTCONTAINERS.md` の該当節 2）。

use std::time::Duration;

use crate::{
    ContainerRequest, Image,
    core::{
        PortMapping,
        containers::request::ExtraHost,
        copy::{CopyDataSource, CopyTargetOptions, CopyToContainer},
        logs::consumer::LogConsumer,
        mounts::Mount,
        ports::ContainerPort,
    },
};

/// `Image` に設定を重ねる拡張トレイト。
pub trait ImageExt<I: Image> {
    fn with_cmd(self, cmd: impl IntoIterator<Item = impl Into<String>>) -> ContainerRequest<I>;
    fn with_name(self, name: impl Into<String>) -> ContainerRequest<I>;
    fn with_tag(self, tag: impl Into<String>) -> ContainerRequest<I>;
    fn with_container_name(self, name: impl Into<String>) -> ContainerRequest<I>;
    /// コンテナのホスト名を設定する。
    ///
    /// macOS (Apple container) では `networks[0].options.hostname` に反映される。
    /// 未指定時は `with_container_name` の値、それも無ければコンテナ ID が使われる。
    fn with_hostname(self, hostname: impl Into<String>) -> ContainerRequest<I>;
    fn with_network(self, network: impl Into<String>) -> ContainerRequest<I>;
    fn with_label(self, key: impl Into<String>, value: impl Into<String>) -> ContainerRequest<I>;
    fn with_labels(
        self,
        labels: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> ContainerRequest<I>;
    fn with_env_var(self, name: impl Into<String>, value: impl Into<String>)
    -> ContainerRequest<I>;
    fn with_host(self, key: impl Into<String>, value: impl Into<ExtraHost>) -> ContainerRequest<I>;
    fn with_mount(self, mount: impl Into<Mount>) -> ContainerRequest<I>;
    /// コンテナへコピーするファイルを登録する。
    ///
    /// # タイミング契約
    ///
    /// - **Linux (Docker Engine API)**: 初回の `AsyncRunner::start` で `create` 後・
    ///   `start` 前に投入が完了する。初期プロセスが起動時に読むファイルにも使える。
    /// - **macOS (Apple container)**: `start_process` 後に `containerCopyIn` で投入する。
    ///   running でないと XPC が拒否するため、起動前投入の公開契約は無い。
    ///   初期プロセスが起動時に読むファイルには、利用側の起動待ち等が別途必要になり得る。
    ///
    /// # 投入能力
    ///
    /// - **Linux**: 親ディレクトリの自動作成とホストディレクトリの再帰投入に対応する。
    ///   配下の regular file には `CopyTargetOptions.mode` / `uid` / `gid` を適用する。
    ///   中間 directory の mode は `0o755`。symlink / 特殊ファイルは拒否する。
    /// - **macOS**: 親ディレクトリは `createParents` で自動作成される。ホストディレクトリの
    ///   再帰投入も XPC が受理する（Apple container 1.1.0 で実測）。`uid` / `gid` は非反映。
    ///
    /// 停止後の `ContainerAsync::start`（再起動）では再投入しない。
    fn with_copy_to(
        self,
        target: impl Into<CopyTargetOptions>,
        source: impl Into<CopyDataSource>,
    ) -> ContainerRequest<I>;
    fn with_mapped_port(self, host_port: u16, container_port: ContainerPort)
    -> ContainerRequest<I>;
    fn with_privileged(self, privileged: bool) -> ContainerRequest<I>;
    /// ルートファイルシステムを読み取り専用にする。
    ///
    /// macOS (Apple container) では XPC `readOnly` に反映される。
    /// 個別マウントの read-only (`Mount` の AccessMode) とは別設定である。
    fn with_readonly_rootfs(self, readonly_rootfs: bool) -> ContainerRequest<I>;
    fn with_cap_add(self, capability: impl Into<String>) -> ContainerRequest<I>;
    fn with_cap_drop(self, capability: impl Into<String>) -> ContainerRequest<I>;
    fn with_shm_size(self, bytes: u64) -> ContainerRequest<I>;
    fn with_startup_timeout(self, timeout: Duration) -> ContainerRequest<I>;
    fn with_working_dir(self, working_dir: impl Into<String>) -> ContainerRequest<I>;
    /// ログフレームを受け取るコールバックを登録する。
    ///
    /// # 制約
    ///
    /// `blocking` feature 使用時、このコールバック内から同期 API (`SyncRunner::start` 等)
    /// を呼び出すと共有 Runtime への再入によって deadlock する。コールバック内では
    /// 同期 API の呼び出しを避けること。
    fn with_log_consumer(self, log_consumer: impl LogConsumer + 'static) -> ContainerRequest<I>;
    fn with_user(self, user: impl Into<String>) -> ContainerRequest<I>;
    /// stdin を開いた状態で起動する (XPC では `initProcess.terminal` に反映)。
    ///
    /// Docker の `OpenStdin` と XPC の `terminal` は同義ではないが、
    /// Apple container で利用可能な最も近い設定口として使う。
    fn with_open_stdin(self, open_stdin: bool) -> ContainerRequest<I>;
    fn with_ready_conditions(
        self,
        ready_conditions: Vec<crate::core::WaitFor>,
    ) -> ContainerRequest<I>;

    /// コンテナを実行するプラットフォームを指定する。
    ///
    /// macOS (Apple container) では `"linux/amd64"` (または `"amd64"`) を指定すると
    /// Rosetta・`imagePull` の `ociPlatform`・`containerCreate` の `platform.architecture`
    /// に反映される。`"linux/arm64"` / `"arm64"` も architecture / pull に反映される。
    /// それ以外の値は無視される。
    fn with_platform(self, platform: impl Into<String>) -> ContainerRequest<I>;

    // ── shiguredo 拡張（本家には無いが既存 API 互換のため残す）──
    fn with_init(self) -> ContainerRequest<I>;
    fn with_ssh(self) -> ContainerRequest<I>;
}

impl<RI: Into<ContainerRequest<I>>, I: Image> ImageExt<I> for RI {
    fn with_cmd(self, cmd: impl IntoIterator<Item = impl Into<String>>) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            overridden_cmd: cmd.into_iter().map(Into::into).collect(),
            ..container_req
        }
    }

    fn with_name(self, name: impl Into<String>) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            image_name: Some(name.into()),
            ..container_req
        }
    }

    fn with_tag(self, tag: impl Into<String>) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            image_tag: Some(tag.into()),
            ..container_req
        }
    }

    fn with_container_name(self, name: impl Into<String>) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            container_name: Some(name.into()),
            ..container_req
        }
    }

    fn with_hostname(self, hostname: impl Into<String>) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            hostname: Some(hostname.into()),
            ..container_req
        }
    }

    fn with_network(self, network: impl Into<String>) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            network: Some(network.into()),
            ..container_req
        }
    }

    fn with_label(self, key: impl Into<String>, value: impl Into<String>) -> ContainerRequest<I> {
        let mut container_req = self.into();
        container_req.labels.insert(key.into(), value.into());
        container_req
    }

    fn with_labels(
        self,
        labels: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> ContainerRequest<I> {
        let mut container_req = self.into();
        container_req
            .labels
            .extend(labels.into_iter().map(|(k, v)| (k.into(), v.into())));
        container_req
    }

    fn with_env_var(
        self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> ContainerRequest<I> {
        let mut container_req = self.into();
        container_req.env_vars.insert(name.into(), value.into());
        container_req
    }

    fn with_host(self, key: impl Into<String>, value: impl Into<ExtraHost>) -> ContainerRequest<I> {
        let mut container_req = self.into();
        container_req.hosts.insert(key.into(), value.into());
        container_req
    }

    fn with_mount(self, mount: impl Into<Mount>) -> ContainerRequest<I> {
        let mut container_req = self.into();
        container_req.mounts.push(mount.into());
        container_req
    }

    fn with_copy_to(
        self,
        target: impl Into<CopyTargetOptions>,
        source: impl Into<CopyDataSource>,
    ) -> ContainerRequest<I> {
        let mut container_req = self.into();
        container_req
            .copy_to_sources
            .push(CopyToContainer::new(source, target));
        container_req
    }

    fn with_mapped_port(
        self,
        host_port: u16,
        container_port: ContainerPort,
    ) -> ContainerRequest<I> {
        let container_req = self.into();
        let mut ports = container_req.ports.unwrap_or_default();
        ports.push(PortMapping::new(host_port, container_port));
        ContainerRequest {
            ports: Some(ports),
            ..container_req
        }
    }

    fn with_privileged(self, privileged: bool) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            privileged,
            ..container_req
        }
    }

    fn with_readonly_rootfs(self, readonly_rootfs: bool) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            readonly_rootfs,
            ..container_req
        }
    }

    fn with_cap_add(self, capability: impl Into<String>) -> ContainerRequest<I> {
        let mut container_req = self.into();
        container_req
            .cap_add
            .get_or_insert_with(Vec::new)
            .push(capability.into());
        container_req
    }

    fn with_cap_drop(self, capability: impl Into<String>) -> ContainerRequest<I> {
        let mut container_req = self.into();
        container_req
            .cap_drop
            .get_or_insert_with(Vec::new)
            .push(capability.into());
        container_req
    }

    fn with_shm_size(self, bytes: u64) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            shm_size: Some(bytes),
            ..container_req
        }
    }

    fn with_startup_timeout(self, timeout: Duration) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            startup_timeout: Some(timeout),
            ..container_req
        }
    }

    fn with_working_dir(self, working_dir: impl Into<String>) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            working_dir: Some(working_dir.into()),
            ..container_req
        }
    }

    fn with_log_consumer(self, log_consumer: impl LogConsumer + 'static) -> ContainerRequest<I> {
        let mut container_req = self.into();
        container_req.log_consumers.push(Box::new(log_consumer));
        container_req
    }

    fn with_user(self, user: impl Into<String>) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            user: Some(user.into()),
            ..container_req
        }
    }

    fn with_open_stdin(self, open_stdin: bool) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            open_stdin: Some(open_stdin),
            ..container_req
        }
    }

    fn with_ready_conditions(
        self,
        ready_conditions: Vec<crate::core::WaitFor>,
    ) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            ready_conditions: Some(ready_conditions),
            ..container_req
        }
    }

    fn with_init(self) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            init: true,
            ..container_req
        }
    }

    fn with_platform(self, platform: impl Into<String>) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            platform: Some(platform.into()),
            ..container_req
        }
    }

    fn with_ssh(self) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            ssh: true,
            ..container_req
        }
    }
}
