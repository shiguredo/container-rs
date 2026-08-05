//! `ImageExt` 拡張トレイト。元の testcontainers 0.27 のサブセット（差分の正は `docs/TESTCONTAINERS.md` の該当節 2）。

use std::time::Duration;

use crate::{
    ContainerRequest, Image,
    core::{
        PortMapping,
        containers::request::ExtraHost,
        copy::{CopyDataSource, CopyTargetOptions, CopyToContainer},
        healthcheck::Healthcheck,
        logs::consumer::LogConsumer,
        mounts::Mount,
        ports::ContainerPort,
    },
};

/// `Image` に設定を重ねる拡張トレイト。
pub trait ImageExt<I: Image> {
    /// コンテナの CMD を上書きする。
    fn with_cmd(self, cmd: impl IntoIterator<Item = impl Into<String>>) -> ContainerRequest<I>;
    /// イメージ名を上書きする。
    fn with_name(self, name: impl Into<String>) -> ContainerRequest<I>;
    /// イメージタグを上書きする。
    fn with_tag(self, tag: impl Into<String>) -> ContainerRequest<I>;
    /// コンテナ名を設定する。
    ///
    /// 設定した名前はコンテナ ID として使われる。Apple container 1.2.0 の `nameValid` と
    /// 同じ制約 (先頭は英数字・実質 2 文字以上・63 文字以下・文字種は英数字 / `_` / `.` / `-`)
    /// を満たさない場合、macOS の `AsyncRunner::start` が pull / resolve より前に明示エラーを返す。
    fn with_container_name(self, name: impl Into<String>) -> ContainerRequest<I>;
    /// コンテナのホスト名を設定する。
    ///
    /// macOS (Apple container) では `networks[0].options.hostname` に反映される。
    /// 未指定時は `with_container_name` の値、それも無ければコンテナ ID が使われる。
    fn with_hostname(self, hostname: impl Into<String>) -> ContainerRequest<I>;
    /// 接続するネットワークを設定する。
    fn with_network(self, network: impl Into<String>) -> ContainerRequest<I>;
    /// ラベルを 1 件追加する。
    fn with_label(self, key: impl Into<String>, value: impl Into<String>) -> ContainerRequest<I>;
    /// ラベルを複数追加する。
    fn with_labels(
        self,
        labels: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> ContainerRequest<I>;
    /// 環境変数を 1 件追加する。
    fn with_env_var(self, name: impl Into<String>, value: impl Into<String>)
    -> ContainerRequest<I>;
    /// extra_hosts エントリを追加する。
    fn with_host(self, key: impl Into<String>, value: impl Into<ExtraHost>) -> ContainerRequest<I>;
    /// マウントを追加する。
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
    ///   macOS で起動前にファイルを見せたい場合は `with_mount(Mount::bind_mount(host_path,
    ///   container_path))` を使うこと (virtiofs として起動前に見えるようになる)。
    ///   container_path はコンテナ内の絶対パスを渡すこと。
    ///   次の制約に注意すること。
    ///   - `host_path` は絶対パスかつ実ファイル / 実ディレクトリを渡す前提 (相対パスは
    ///     apiserver 側の cwd で解決されるため。symlink は未検証のため対象外)
    ///   - `CopyDataSource::Data` (インメモリ bytes) の起動前投入は対象外。必要なら
    ///     利用者側で一時ファイル (`tempfile` クレート等) に書き出して bind する。
    ///     コンテナ稼働中はその一時ファイルを削除 (unlink) しないこと (virtiofs は
    ///     ホスト側ファイルを直接共有するため。スコープを抜けると自動 unlink される
    ///     `NamedTempFile` 等はコンテナ停止まで変数で保持すること)
    ///   - virtiofs はコンテナ稼働中にホスト側ファイルを書き換えるとコンテナ内の
    ///     見え方が変わり得る (即時反映は実測されていないため断言しない)。既定は
    ///     ReadWrite のため、読み取り専用にしたい場合は
    ///     `with_access_mode(AccessMode::ReadOnly)` を指定する。`with_copy_to` の
    ///     スナップショット投入とは意味論が異なる
    ///
    /// # 投入能力
    ///
    /// - **Linux**: 親ディレクトリの自動作成とホストディレクトリの再帰投入に対応する。
    ///   配下の regular file には `CopyTargetOptions.mode` / `uid` / `gid` を適用する。
    ///   中間 directory の mode は `0o755`。symlink / 特殊ファイルは拒否する。
    ///   ターゲットパスに `..` (親ディレクトリ参照)・終端の `.`・空コンポーネント (`//`) を
    ///   含めることはできない。
    ///   コピー対象 1 ファイルあたりと、1 ソースにつき生成する tar 全体 (ヘッダ + データ +
    ///   トレーラ) の蓄積はともに 64 MiB を上限とし、超過時は切り詰めずエラーにする
    ///   (OOM 防止)。`CopyDataSource::Data` は読み込みを伴わないため per-file 上限の対象外
    ///   だが、tar 全体の蓄積上限は受ける。
    /// - **macOS**: 親ディレクトリは `createParents` で自動作成される。ホストディレクトリの
    ///   再帰投入も XPC が受理する（Apple container 1.1.0 で実測）。`uid` / `gid` は非反映。
    ///
    /// 停止後の `ContainerAsync::start`（再起動）では再投入しない。
    fn with_copy_to(
        self,
        target: impl Into<CopyTargetOptions>,
        source: impl Into<CopyDataSource>,
    ) -> ContainerRequest<I>;
    /// ホストポートとコンテナポートのマッピングを追加する。
    ///
    /// `host_port` に 0 を指定すると (Docker のランダム割当の慣用)、**macOS
    /// (Apple container)** では起動時に空きホストポートを自動割当する (Apple
    /// container には `hostPort: 0` のランダム割当が無いため)。**Linux (Docker
    /// Engine API)** では 0 のまま Docker Engine に渡し、Engine 側のランダム割当に
    /// 任せる。割当は bind(0) → 即 release のため、起動までにポートを奪われると
    /// start が失敗し得る (自動再試行は無い)。
    fn with_mapped_port(self, host_port: u16, container_port: ContainerPort)
    -> ContainerRequest<I>;
    /// privileged モードを設定する。
    fn with_privileged(self, privileged: bool) -> ContainerRequest<I>;
    /// ルートファイルシステムを読み取り専用にする。
    ///
    /// macOS (Apple container) では XPC `readOnly` に反映される。
    /// 個別マウントの read-only (`Mount` の AccessMode) とは別設定である。
    fn with_readonly_rootfs(self, readonly_rootfs: bool) -> ContainerRequest<I>;
    /// Linux capability を追加する。
    fn with_cap_add(self, capability: impl Into<String>) -> ContainerRequest<I>;
    /// Linux capability を削除する。
    fn with_cap_drop(self, capability: impl Into<String>) -> ContainerRequest<I>;
    /// /dev/shm のサイズをバイト単位で設定する。
    fn with_shm_size(self, bytes: u64) -> ContainerRequest<I>;
    /// 起動タイムアウトを設定する。
    fn with_startup_timeout(self, timeout: Duration) -> ContainerRequest<I>;
    /// 作業ディレクトリを設定する。
    fn with_working_dir(self, working_dir: impl Into<String>) -> ContainerRequest<I>;
    /// ログフレームを受け取るコールバックを登録する。
    ///
    /// # 制約
    ///
    /// `blocking` feature 使用時、このコールバック内から同期 API (`SyncRunner::start` 等)
    /// を呼び出すと共有 Runtime への再入によって deadlock する。コールバック内では
    /// 同期 API の呼び出しを避けること。
    fn with_log_consumer(self, log_consumer: impl LogConsumer + 'static) -> ContainerRequest<I>;
    /// コンテナ内でプロセスを実行するユーザーを設定する。
    fn with_user(self, user: impl Into<String>) -> ContainerRequest<I>;
    /// stdin を開いた状態で起動する (XPC では `initProcess.terminal` に反映)。
    ///
    /// Docker の `OpenStdin` と XPC の `terminal` は同義ではないが、
    /// Apple container で利用可能な最も近い設定口として使う。
    fn with_open_stdin(self, open_stdin: bool) -> ContainerRequest<I>;
    /// 準備完了条件を設定する。`Image::ready_conditions` を上書きする。
    fn with_ready_conditions(
        self,
        ready_conditions: Vec<crate::core::WaitFor>,
    ) -> ContainerRequest<I>;
    /// ヘルスチェックを設定する。
    ///
    /// Linux (Docker Engine API) では create JSON の `Config.Healthcheck` に配線する。
    /// macOS では start 時に明示エラーを返す。
    fn with_health_check(self, healthcheck: Healthcheck) -> ContainerRequest<I>;

    /// コンテナを実行するプラットフォームを指定する。
    ///
    /// macOS (Apple container) では `"linux/amd64"` (または `"amd64"`) を指定すると
    /// Rosetta・`imagePull` の `ociPlatform`・`containerCreate` の `platform.architecture`
    /// に反映される。`"linux/arm64"` / `"arm64"` も architecture / pull に反映される。
    /// それ以外の値は無視される。
    fn with_platform(self, platform: impl Into<String>) -> ContainerRequest<I>;

    // ── shiguredo 拡張（本家には無いが既存 API 互換のため残す）──
    /// init プロセスを有効にする (shiguredo 拡張)。
    fn with_init(self) -> ContainerRequest<I>;
    /// SSH 転送を有効にする (shiguredo 拡張)。
    fn with_ssh(self) -> ContainerRequest<I>;

    /// OCI `maskedPaths` を設定する (shiguredo 拡張、Apple container 1.2.0 以上)。
    ///
    /// 未指定 (`None`) はランタイム既定セット、空リストは既定の無効化、
    /// 明示リストは既定を完全に上書きする。複数回呼び出しは上書きされる。
    /// macOS のみ対応で、Linux (Docker) では start 時に明示エラーを返す。
    ///
    /// 例: `with_masked_paths(std::iter::empty::<String>())` で既定マスクを無効化する。
    fn with_masked_paths(
        self,
        paths: impl IntoIterator<Item = impl Into<String>>,
    ) -> ContainerRequest<I>;

    /// OCI `readonlyPaths` を設定する (shiguredo 拡張、Apple container 1.2.0 以上)。
    ///
    /// 指定したパスを読み取り専用にする。OCI の個別パス読み取り専用化であり、
    /// ルート FS 全体の `with_readonly_rootfs` や `Mount` の `AccessMode::ReadOnly` とは別物。
    /// 未指定 (`None`) はランタイム既定、空リストは既定の無効化、
    /// 明示リストは既定を完全に上書きする。複数回呼び出しは上書きされる。
    /// macOS のみ対応で、Linux (Docker) では start 時に明示エラーを返す。
    fn with_readonly_paths(
        self,
        paths: impl IntoIterator<Item = impl Into<String>>,
    ) -> ContainerRequest<I>;
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

    fn with_health_check(self, healthcheck: Healthcheck) -> ContainerRequest<I> {
        let mut container_req = self.into();
        container_req.health_check = Some(healthcheck);
        container_req
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

    fn with_masked_paths(
        self,
        paths: impl IntoIterator<Item = impl Into<String>>,
    ) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            masked_paths: Some(paths.into_iter().map(Into::into).collect()),
            ..container_req
        }
    }

    fn with_readonly_paths(
        self,
        paths: impl IntoIterator<Item = impl Into<String>>,
    ) -> ContainerRequest<I> {
        let container_req = self.into();
        ContainerRequest {
            readonly_paths: Some(paths.into_iter().map(Into::into).collect()),
            ..container_req
        }
    }
}
