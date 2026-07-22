//! `AsyncRunner` — コンテナを非同期に起動するトレイト。
//! 元の 0.27 の `runners::async_runner` と同一シグネチャ。
//!
//! macOS (XPC) では `ContainerRequest<I>` から `ContainerCfg` を構築し、
//! XPC の `containerCreate` → `containerBootstrap` → `containerStartProcess` を呼ぶ。

use std::time::Duration;

use crate::{
    ContainerAsync, ContainerRequest, Image,
    core::{
        client::Client,
        error::{Result, WaitContainerError},
        wait::WaitFor,
    },
};

#[cfg(target_os = "macos")]
use crate::core::error::ClientError;
#[cfg(target_os = "macos")]
use crate::core::util::unique_suffix;

#[cfg(target_os = "macos")]
use crate::core::{
    containers::request::ExtraHost,
    copy::{CopyDataSource, CopyToContainer},
    image::ExecCommand,
    wait::CmdWaitFor,
};

const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// コンテナを非同期に起動するトレイト。
#[expect(async_fn_in_trait)]
pub trait AsyncRunner<I: Image> {
    /// コンテナを起動し `ContainerAsync` を返す。
    async fn start(self) -> Result<ContainerAsync<I>>;

    /// イメージをプルする。
    async fn pull_image(self) -> Result<ContainerRequest<I>>;
}

impl<T, I> AsyncRunner<I> for T
where
    T: Into<ContainerRequest<I>> + Send,
    I: Image,
{
    async fn start(self) -> Result<ContainerAsync<I>> {
        let container_req = self.into();
        let client = Client::detect()?;

        #[cfg(target_os = "macos")]
        {
            // cfg で片方のアームが消えるため clippy::infallible_destructuring_match を抑制する。
            #[expect(clippy::infallible_destructuring_match)]
            let client = match client {
                Client::MacOs(c) => c,
                #[cfg(target_os = "linux")]
                Client::Linux(_) => unreachable!("macOS block is not compiled on Linux"),
            };

            let descriptor = container_req.descriptor();

            // platform を正規化する。許可外文字列は resolve / pull / create に渡さない。
            let (arch, _rosetta, resolve_platform) =
                crate::core::client::container_cfg::normalize_platform(
                    container_req.platform().as_deref(),
                );

            // イメージの descriptor を解決 (amd64 強制 pull / ImageNotFound 時のみ再試行)。
            let desc_raw =
                resolve_or_pull_macos(&client, &descriptor, arch, resolve_platform).await?;

            // イメージ既定の CMD/ENTRYPOINT を OCI config から解決する。
            // 許可外の生文字列は渡さず、正規化後の resolve_platform のみを渡す。
            let image_config = crate::core::client::image_config::resolve_image_config(
                &client,
                &desc_raw,
                resolve_platform,
            )
            .await?;

            // コンテナ ID。名前が指定されていればそれを使い、無ければタイムスタンプベースで生成。
            let id = container_req
                .container_name()
                .clone()
                .unwrap_or_else(|| format!("c-{}", unique_suffix()));

            // デフォルトカーネルを取得する。
            // amd64 (Rosetta) ゲストでも arm64 カーネルを使う (CLI と同方針)。
            let kernel = client.get_default_kernel().await?;

            // ContainerCfg を構築して containerCreate。
            // 失敗時のロールバックは spawn の投げっぱなしにせず await する。
            // 呼び出し元がすぐ終了 (テストプロセス等) しても削除が完了することを保証する。
            let cfg = crate::core::client::container_cfg::build_config(
                &container_req,
                &id,
                &desc_raw,
                &image_config,
            )?;
            if let Err(e) = client.create_container(&cfg, kernel).await {
                // 作成失敗時はロールバックを試みる。
                // Keep 指定時は構築前ロールバックでも削除しない (失敗したコンテナを残して調査する)。
                if matches!(
                    crate::core::env::Config.command(),
                    crate::core::env::Command::Remove
                ) && let Err(rm_err) = client.remove(&id, true).await
                {
                    tracing::warn!("failed to remove container {id} during rollback: {rm_err}");
                }
                return Err(e);
            }

            // watchdog: create 直後に登録し、bootstrap / start / copy 途中の
            // クラッシュでも孤立コンテナを掃除できるようにする。
            // TESTCONTAINERS_COMMAND=keep 指定時はコンテナを残す意図なので登録しない。
            #[cfg(feature = "watchdog")]
            if !matches!(
                crate::core::env::Config.command(),
                crate::core::env::Command::Keep
            ) {
                crate::watchdog::register(&id);
            }

            // containerBootstrap
            if let Err(e) = client.bootstrap_container(&id).await {
                // ブートストラップ失敗時もロールバック。
                // Keep 指定時は構築前ロールバックでも削除しない (失敗したコンテナを残して調査する)。
                if matches!(
                    crate::core::env::Config.command(),
                    crate::core::env::Command::Remove
                ) && let Err(rm_err) = client.remove(&id, true).await
                {
                    tracing::warn!("failed to remove container {id} during rollback: {rm_err}");
                }
                return Err(e);
            }

            // containerStartProcess
            if let Err(e) = client.start_process(&id).await {
                // 初期プロセス起動失敗時もロールバック。
                // Keep 指定時は構築前ロールバックでも削除しない (失敗したコンテナを残して調査する)。
                if matches!(
                    crate::core::env::Config.command(),
                    crate::core::env::Command::Remove
                ) && let Err(rm_err) = client.remove(&id, true).await
                {
                    tracing::warn!("failed to remove container {id} during rollback: {rm_err}");
                }
                return Err(e);
            }

            // copy_to_sources を実行。
            // Apple container の containerCopyIn はコンテナが running の場合にのみ
            // 利用できるため、start_process 後に実行する。
            if let Err(e) = copy_to_sources(&client, &id, &container_req).await {
                // コピー失敗時もロールバック。
                // Keep 指定時は構築前ロールバックでも削除しない (失敗したコンテナを残して調査する)。
                if matches!(
                    crate::core::env::Config.command(),
                    crate::core::env::Command::Remove
                ) && let Err(rm_err) = client.remove(&id, true).await
                {
                    tracing::warn!("failed to remove container {id} during rollback: {rm_err}");
                }
                return Err(e);
            }

            // ready_conditions 待機用のタイムアウトを事前に決定。
            // ContainerAsync::new は container_req の所有権を取るため、その前に取得する。
            let startup_timeout = container_req
                .startup_timeout()
                .unwrap_or(DEFAULT_STARTUP_TIMEOUT);
            let ready_conditions = container_req.ready_conditions();

            // init プロセスの exit code をランタイム非管理の std スレッドで待機する。
            // spawn_blocking を使うと、コンテナ実行中にランタイムを drop した際に
            // tokio が LONG_TIMEOUT (24 時間) の containerWait を join しようとして
            // ランタイムの drop がハングする。std スレッドなら join されず、
            // プロセス終了時にそのまま破棄される。
            let wait_state = crate::core::containers::async_container::new_wait_state();
            {
                let generation = wait_state
                    .lock()
                    .expect("wait state mutex must not be poisoned while spawning exit code waiter")
                    .generation();
                crate::core::containers::async_container::spawn_exit_code_waiter(
                    id.clone(),
                    wait_state.clone(),
                    generation,
                );
            }

            // with_host で指定された extra_hosts を /etc/hosts に反映する。
            // Apple container の XPC には extra_hosts に相当する設定が無いため、
            // コンテナ起動後に exec で追記する。
            let hosts: Vec<(String, ExtraHost)> = container_req
                .hosts()
                .map(|(name, host)| (name.into_owned(), *host))
                .collect();

            // コンテナログ用の FD を取得。
            // Log 戦略があるのに FD が取れない場合は startup_timeout まで待つより
            // 明示エラーで落とす。Log が無い場合は従来どおり warn + None 続行。
            let (stdout_fd, stderr_fd) = match client.logs(&id).await {
                Ok((out, err)) => (Some(out), Some(err)),
                Err(e) => {
                    if ready_conditions_require_log_fds(&ready_conditions) {
                        if let Err(rm_err) = client.remove(&id, true).await {
                            tracing::warn!(
                                "failed to remove container {id} during rollback: {rm_err}"
                            );
                        }
                        return Err(log_fd_required_error(&e));
                    }
                    tracing::warn!("failed to get log fds: {e}");
                    (None, None)
                }
            };

            // ContainerAsync を構築。
            let container = ContainerAsync::new(
                id,
                Client::MacOs(client),
                container_req,
                wait_state,
                stdout_fd,
                stderr_fd,
            );

            // extra_hosts は ready 共通化の外 (呼び出し前) に残す。
            // 失敗時も Keep ゲート付き明示 rm でロールバックする。
            if let Err(e) = apply_extra_hosts(&container, &hosts).await {
                return cleanup_on_ready_failure(container, e).await;
            }

            run_ready_sequence(container, startup_timeout, ready_conditions).await
        }

        #[cfg(target_os = "linux")]
        {
            // cfg で片方のアームが消えるため clippy::infallible_destructuring_match を抑制する。
            #[expect(clippy::infallible_destructuring_match)]
            let client = match client {
                Client::Linux(c) => c,
                #[cfg(target_os = "macos")]
                Client::MacOs(_) => unreachable!("Linux block is not compiled on macOS"),
            };

            // Linux では copy_to / with_copy_to 未実装。
            // 黙って無視するとコンテナ内にファイルが無い状態で進むため、作成前に fail-fast する。
            if container_req.copy_to_sources().next().is_some() {
                return Err(crate::Error::other("copy_to() is not implemented on Linux"));
            }

            // Linux で設定構築に載らない ImageExt は黙って成功させない。
            if let Some(msg) = linux_unsupported_request_reason(&container_req) {
                return Err(crate::Error::other(msg));
            }

            // ログ FD が無いため Log 待機は成立しない。timeout まで空振りさせない。
            let ready_conditions = container_req.ready_conditions();
            if ready_conditions_require_log_fds(&ready_conditions) {
                return Err(crate::Error::other(
                    "log wait is not supported on Linux (log file descriptors are unavailable)",
                ));
            }

            let descriptor = container_req.descriptor();

            // イメージの descriptor を解決。未発見時はプルして再試行。
            let _desc_raw = resolve_or_pull_linux(&client, &descriptor).await?;

            // コンテナ設定を構築。
            let config = build_container_config(&container_req);
            let id = client.create_container(config).await?;

            // 作成に成功した後に起動。起動失敗時はロールバック。
            // 失敗時のロールバックは spawn の投げっぱなしにせず await する。
            // 呼び出し元がすぐ終了 (テストプロセス等) しても削除が完了することを保証する。
            // Keep 指定時は構築前ロールバックでも削除しない (失敗したコンテナを残して調査する)。
            if let Err(e) = client.start_container(&id).await {
                if matches!(
                    crate::core::env::Config.command(),
                    crate::core::env::Command::Remove
                ) && let Err(rm_err) = client.remove(&id, true).await
                {
                    tracing::warn!("failed to remove container {id} during rollback: {rm_err}");
                }
                return Err(e);
            }

            // ready_conditions 待機用のタイムアウトを事前に決定。
            // ContainerAsync::new は container_req の所有権を取るため、その前に取得する。
            let startup_timeout = container_req
                .startup_timeout()
                .unwrap_or(DEFAULT_STARTUP_TIMEOUT);

            // ContainerAsync を構築。Linux ではログ FD は未対応。
            let container = ContainerAsync::new(
                id,
                Client::Linux(client),
                container_req,
                crate::core::containers::async_container::new_wait_state(),
                None,
                None,
            );

            run_ready_sequence(container, startup_timeout, ready_conditions).await
        }
    }

    async fn pull_image(self) -> Result<ContainerRequest<I>> {
        let container_req = self.into();
        let descriptor = container_req.descriptor();

        match Client::detect()? {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => {
                let (arch, _, resolve_platform) =
                    crate::core::client::container_cfg::normalize_platform(
                        container_req.platform().as_deref(),
                    );
                c.pull_image(&descriptor, resolve_platform.map(|_| arch))
                    .await?;
            }
            #[cfg(target_os = "linux")]
            Client::Linux(c) => {
                c.pull_image(&descriptor).await?;
            }
        }

        Ok(container_req)
    }
}

/// macOS: イメージ descriptor を解決し、必要なら pull して再試行する。
///
/// - amd64 platform 指定時は既存参照があっても強制 pull する
/// - それ以外は `ImageNotFound` のときだけ pull 再試行する (他エラーは伝播)
#[cfg(target_os = "macos")]
async fn resolve_or_pull_macos(
    client: &crate::core::client::XpcClient,
    descriptor: &str,
    arch: &str,
    resolve_platform: Option<&str>,
) -> Result<String> {
    // amd64 指定時は既存参照があっても ociPlatform 付き pull を必ず行い、
    // arm64 のみ取得済みの multi-arch 参照で amd64 digest の content_get が
    // 失敗する経路を潰す。未指定時は従来どおり resolve 成功なら pull をスキップする。
    if resolve_platform == Some("linux/amd64") {
        client.pull_image(descriptor, Some(arch)).await?;
        return client.resolve_image_descriptor(descriptor).await;
    }

    match client.resolve_image_descriptor(descriptor).await {
        Ok(d) => Ok(d),
        Err(crate::Error::Client(ClientError::ImageNotFound(_))) => {
            // arm64 明示時は ociPlatform を送り、未指定時はキーごと省略する。
            client
                .pull_image(descriptor, resolve_platform.map(|_| arch))
                .await?;
            client.resolve_image_descriptor(descriptor).await
        }
        Err(e) => Err(e),
    }
}

/// Linux: イメージ descriptor を解決し、失敗時は pull して再試行する。
///
/// `DockerClient::resolve_image_descriptor` 内の 404 pull との二重構造は維持する。
#[cfg(target_os = "linux")]
async fn resolve_or_pull_linux(
    client: &crate::core::client::DockerClient,
    descriptor: &str,
) -> Result<String> {
    match client.resolve_image_descriptor(descriptor).await {
        Ok(d) => Ok(d),
        Err(_) => {
            client.pull_image(descriptor).await?;
            client.resolve_image_descriptor(descriptor).await
        }
    }
}

/// 構築後の ready シーケンスを実行する。
///
/// `exec_before_ready` → `timeout` + `block_until_ready` → `exec_after_start`。
/// 失敗時は Keep ゲート付きで明示 `rm` してから Err を返す。
async fn run_ready_sequence<I: Image>(
    container: ContainerAsync<I>,
    startup_timeout: Duration,
    ready_conditions: Vec<WaitFor>,
) -> Result<ContainerAsync<I>> {
    // 構築後の全区間を 1 つの async ブロックにまとめ、失敗時は Drop に
    // 委ねず runner 側で明示的に rm する (ランタイム解放との競合を避ける)。
    let result: Result<()> = async {
        // exec_before_ready を実行。
        let state = container.container_state().await?;
        for cmd in container.image().exec_before_ready(state)? {
            container.exec(cmd).await?;
        }

        // ready_conditions を待機。timeout の外側 (経過時間) と内側 (待機中エラー) の
        // 両方の Result を伝播させる必要があるため二重 ? を使う。
        tokio::time::timeout(
            startup_timeout,
            container.block_until_ready(ready_conditions),
        )
        .await
        .map_err(|_| WaitContainerError::StartupTimeout {
            id: container.id().to_string(),
            timeout: startup_timeout,
        })??;

        // exec_after_start を実行 (startup_timeout の外側。exec_before_ready と同じ配置)。
        // Linux では非空の exec_after_start は exec 配線に依存する。
        // stdout / stderr メッセージ待ちは Linux では明示エラーになる。
        let state = container.container_state().await?;
        for cmd in container.image().exec_after_start(state)? {
            container.exec(cmd).await?;
        }

        Ok(())
    }
    .await;

    if let Err(e) = result {
        return cleanup_on_ready_failure(container, e).await;
    }

    Ok(container)
}

/// ready シーケンス失敗時の Keep ゲート付き明示 `rm`。
async fn cleanup_on_ready_failure<I: Image>(
    container: ContainerAsync<I>,
    error: crate::Error,
) -> Result<ContainerAsync<I>> {
    // Keep 指定時は Drop のガードと同じく削除しない (失敗したコンテナを残して調査する)。
    // rm(self) は所有権を消費するため、ログ用に ID を先に確保する。
    let id = container.id().to_string();
    if matches!(
        crate::core::env::Config.command(),
        crate::core::env::Command::Remove
    ) && let Err(re) = container.rm().await
    {
        tracing::error!("failed to remove container {id} after startup failure: {re}");
    }
    Err(error)
}

/// `ContainerRequest<I>` から Docker Engine API 用の `ContainerConfig` を構築する。
///
/// `with_mapped_port` の明示マッピングに加え、`Image::expose_ports` /
/// `with_exposed_port` で宣言されたポートのうち未登場のものを
/// `host_port = 0`（Docker Engine のランダム割当）で追加する。
#[cfg(target_os = "linux")]
fn build_container_config<I: Image>(
    req: &ContainerRequest<I>,
) -> crate::core::client::ContainerConfig {
    use crate::core::containers::request::PortMapping;

    // 明示マッピングをベースに、未登場の expose を host_port 0 で足す。
    let mut ports = req.ports().cloned().unwrap_or_default();
    for &exposed in req.expose_ports() {
        if ports.iter().any(|p| p.container_port() == exposed) {
            continue;
        }
        ports.push(PortMapping::new(0, exposed));
    }

    crate::core::client::ContainerConfig {
        image: req.descriptor(),
        entrypoint: req.entrypoint().map(|e| vec![e.to_string()]),
        cmd: req.cmd().map(|c| c.into_owned()).collect(),
        env: req.env_vars().map(|(k, v)| format!("{k}={v}")).collect(),
        ports,
        mounts: req.mounts().cloned().collect(),
        name: req.container_name().clone(),
        labels: req.labels().clone(),
        privileged: req.privileged(),
        working_dir: req.working_dir().map(|d| d.to_string()),
        user: req.user().map(|u| u.to_string()),
        init: req.init(),
    }
}

/// ready_conditions に `WaitFor::Log` が含まれているか。
///
/// ログ FD 取得失敗時に Log 戦略へ進むと EOF 後もポーリングし続け、
/// `startup_timeout` まで原因不明に待つため、事前判定に使う。
fn ready_conditions_require_log_fds(ready_conditions: &[WaitFor]) -> bool {
    ready_conditions
        .iter()
        .any(|c| matches!(c, WaitFor::Log(_)))
}

/// ログ FD 取得失敗かつ Log 戦略がある場合の明示エラーを組み立てる。
#[cfg(target_os = "macos")]
fn log_fd_required_error(cause: &crate::Error) -> crate::Error {
    crate::Error::other(format!(
        "log wait requires log file descriptors, but containerLogs failed: {cause}"
    ))
}

/// Linux では設定構築に載らない ImageExt 項目を列挙する。
///
/// 1 つでも設定されていればその理由文字列を返す (fail-fast 用)。
#[cfg(target_os = "linux")]
fn linux_unsupported_request_reason<I: Image>(req: &ContainerRequest<I>) -> Option<&'static str> {
    if req.platform().is_some() {
        return Some("with_platform() is not implemented on Linux");
    }
    if req.network().is_some() {
        return Some("with_network() is not implemented on Linux");
    }
    if req.hostname().is_some() {
        return Some("with_hostname() is not implemented on Linux");
    }
    if req.hosts().next().is_some() {
        return Some("with_host() is not implemented on Linux");
    }
    if req.cap_add().is_some_and(|v| !v.is_empty()) {
        return Some("with_cap_add() is not implemented on Linux");
    }
    if req.cap_drop().is_some_and(|v| !v.is_empty()) {
        return Some("with_cap_drop() is not implemented on Linux");
    }
    if req.shm_size().is_some() {
        return Some("with_shm_size() is not implemented on Linux");
    }
    if req.readonly_rootfs() {
        return Some("with_readonly_rootfs() is not implemented on Linux");
    }
    if req.open_stdin() == Some(true) {
        return Some("with_open_stdin() is not implemented on Linux");
    }
    if req.ssh() {
        return Some("with_ssh() is not implemented on Linux");
    }
    if !req.log_consumers.is_empty() {
        return Some(
            "with_log_consumer() is not supported on Linux (log file descriptors are unavailable)",
        );
    }
    None
}

/// macOS (XPC) 用の `with_host` フォールバック。
///
/// Apple container の XPC API には Docker の `extra_hosts` に相当する設定が無いため、
/// コンテナ起動後に `exec` で `/etc/hosts` へエントリを追記する。
/// `HostGateway` はホストゲートウェイ IP が環境依存なため未対応とする。
#[cfg(target_os = "macos")]
async fn apply_extra_hosts<I: Image>(
    container: &ContainerAsync<I>,
    hosts: &[(String, ExtraHost)],
) -> Result<()> {
    for (hostname, host) in hosts {
        let ip = match host {
            ExtraHost::Addr(ip) => ip.to_string(),
            ExtraHost::HostGateway => {
                return Err(crate::Error::other(
                    "with_host(..., HostGateway) is not supported on macOS",
                ));
            }
        };

        let cmd = ExecCommand::new([
            "sh",
            "-c",
            r#"printf '%s\t%s\n' "$1" "$2" >> /etc/hosts"#,
            "_",
            &ip,
            hostname,
        ])
        .with_cmd_ready_condition(CmdWaitFor::exit_code(0));

        container.exec(cmd).await?;
    }

    Ok(())
}

/// `CopyDataSource::Data` 用一時ファイルを所有し、破棄時に同期削除する。
///
/// `copy_in` 失敗や `write_all` 失敗、パニックでもスコープ脱出時に削除される。
#[cfg(target_os = "macos")]
struct CopyDataTempFile {
    path: std::path::PathBuf,
}

#[cfg(target_os = "macos")]
impl CopyDataTempFile {
    fn as_path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(target_os = "macos")]
impl Drop for CopyDataTempFile {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            tracing::warn!(
                "failed to remove copy_to temp file {}: {e}",
                self.path.display()
            );
        }
    }
}

/// `CopyDataSource::Data` の内容を mode `0o600` の一時ファイルへ書き出す。
///
/// ファイル作成 (`create_new`) 直後から Drop ガードで所有するため、書き込み失敗や
/// 後続の `copy_in` 失敗でも一時ファイルが `$TMPDIR` に残留しない。
#[cfg(target_os = "macos")]
async fn write_copy_data_temp(data: &[u8]) -> Result<CopyDataTempFile> {
    use tokio::io::AsyncWriteExt;

    // ファイル名には unique_suffix を使う。呼び出しローカルのカウンタだと
    // 並行 start() 間で同名になり、別コンテナの内容が混入する。
    let path =
        std::env::temp_dir().join(format!("shiguredo_container_copy_{}.bin", unique_suffix()));

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .await
        .map_err(|e| crate::Error::other(format!("failed to create temp file for copy_to: {e}")))?;

    // 作成成功直後からガードで所有する (write_all の ? より前に装着する)。
    let guard = CopyDataTempFile { path };

    file.write_all(data)
        .await
        .map_err(|e| crate::Error::other(format!("failed to write temp file for copy_to: {e}")))?;

    Ok(guard)
}

/// `ContainerRequest::copy_to_sources` をコンテナへコピーする。
///
/// XPC `containerCopyIn` を使う。`CopyDataSource::Data` は一時ファイルに書き出してからコピーする。
#[cfg(target_os = "macos")]
async fn copy_to_sources<I: Image>(
    client: &crate::core::client::xpc_client::XpcClient,
    id: &str,
    req: &ContainerRequest<I>,
) -> Result<()> {
    // iterator の型を future に含めないため、あらかじめ collect する。
    // これにより `AsyncRunner::start` の future が `Send` になる。
    let sources: Vec<&CopyToContainer> = req.copy_to_sources().collect();
    for src in sources {
        match &src.source {
            // File ソースはユーザー指定パスをそのまま渡し、Drop ガードは付けない。
            CopyDataSource::File(p) => {
                client
                    .copy_in(id, p, &src.target.path, src.target.mode)
                    .await?;
            }
            CopyDataSource::Data(b) => {
                let guard = write_copy_data_temp(b).await?;
                client
                    .copy_in(id, guard.as_path(), &src.target.path, src.target.mode)
                    .await?;
                // Data コピー用の一時ファイルはガードの Drop で削除する。
            }
        }
    }

    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use crate::core::wait::WaitFor;

    use super::*;

    #[test]
    fn unique_suffix_has_no_collision_under_parallel_generation() {
        // 並列に大量生成しても ID サフィックスが衝突しないこと。
        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| (0..1000).map(|_| unique_suffix()).collect::<Vec<_>>()))
            .collect();
        let mut all: Vec<String> = handles
            .into_iter()
            .flat_map(|h| h.join().expect("スレッドが panic しないこと"))
            .collect();
        let total = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), total, "unique_suffix should never collide");
    }

    #[test]
    fn ready_conditions_require_log_fds_detects_log_strategy() {
        // WaitFor::Log があれば true、無ければ false になること。
        assert!(ready_conditions_require_log_fds(&[
            WaitFor::message_on_stdout("ready")
        ]));
        assert!(ready_conditions_require_log_fds(&[
            WaitFor::seconds(1),
            WaitFor::message_on_stderr("err"),
        ]));
        assert!(ready_conditions_require_log_fds(&[
            WaitFor::message_on_either_std("either")
        ]));
        assert!(!ready_conditions_require_log_fds(&[]));
        assert!(!ready_conditions_require_log_fds(&[WaitFor::Nothing]));
        assert!(!ready_conditions_require_log_fds(&[WaitFor::seconds(1)]));
        assert!(!ready_conditions_require_log_fds(&[WaitFor::healthcheck()]));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn log_fd_required_error_includes_cause() {
        // 原因エラーがメッセージに含まれること。
        let cause = crate::Error::other("simulated containerLogs failure");
        let err = log_fd_required_error(&cause);
        let msg = err.to_string();
        assert!(
            msg.contains("log wait requires log file descriptors, but containerLogs failed:"),
            "明示メッセージが含まれること: {msg}"
        );
        assert!(
            msg.contains("simulated containerLogs failure"),
            "原因が含まれること: {msg}"
        );
    }

    #[tokio::test]
    async fn write_copy_data_temp_creates_file_with_mode_600() {
        // 一時ファイルが mode 0o600 で作成されること。
        use std::os::unix::fs::PermissionsExt;

        let guard = write_copy_data_temp(b"mode-check")
            .await
            .expect("一時ファイルの作成に失敗した");
        let mode = std::fs::metadata(guard.as_path())
            .expect("メタデータの取得に失敗した")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "一時ファイルの mode が 0o600 であること"
        );
        // ガード破棄で後始末する。
    }
}

/// Linux: `build_container_config` のポート合成を検証する。
///
/// 既存の macOS 向け `mod tests` は触らず、別モジュールとして追加する。
#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use crate::core::ports::IntoContainerPort;
    use crate::{ContainerRequest, GenericImage, ImageExt};

    use super::build_container_config;

    #[test]
    fn expose_only_gets_host_port_zero() {
        // with_exposed_port のみのとき host_port 0 の PortMapping が 1 本載ること。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_exposed_port(80.tcp())
            .with_cmd(["sleep", "1"]);
        let cfg = build_container_config(&req);
        assert_eq!(cfg.ports.len(), 1, "expose のみなら 1 本であること");
        assert_eq!(cfg.ports[0].container_port(), 80.tcp());
        assert_eq!(
            cfg.ports[0].host_port(),
            0,
            "create 前は Docker ランダム割当用に host_port が 0 であること"
        );
    }

    #[test]
    fn mapped_port_takes_precedence_over_same_expose() {
        // 同番号・同プロトコルでは明示マッピングが優先され、自動追加されないこと。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_exposed_port(80.tcp())
            .with_mapped_port(18080, 80.tcp())
            .with_cmd(["sleep", "1"]);
        let cfg = build_container_config(&req);
        assert_eq!(cfg.ports.len(), 1, "重複は 1 本に収まること");
        assert_eq!(cfg.ports[0].host_port(), 18080);
        assert_eq!(cfg.ports[0].container_port(), 80.tcp());
    }

    #[test]
    fn mapped_zero_takes_precedence_over_same_expose() {
        // host_port 0 の明示マッピングも同判定で優先され、expose で二重にならないこと。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_exposed_port(80.tcp())
            .with_mapped_port(0, 80.tcp())
            .with_cmd(["sleep", "1"]);
        let cfg = build_container_config(&req);
        assert_eq!(cfg.ports.len(), 1, "重複は 1 本に収まること");
        assert_eq!(cfg.ports[0].host_port(), 0);
        assert_eq!(cfg.ports[0].container_port(), 80.tcp());
    }

    #[test]
    fn different_protocol_same_number_keeps_both() {
        // 同番号・異プロトコルは別エントリとして両方載ること。
        // with_exposed_port は GenericImage のメソッドなので mapped より先に呼ぶ。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_exposed_port(80.udp())
            .with_mapped_port(18080, 80.tcp())
            .with_cmd(["sleep", "1"]);
        let cfg = build_container_config(&req);
        assert_eq!(cfg.ports.len(), 2, "異 proto は 2 本であること");
        assert!(
            cfg.ports
                .iter()
                .any(|p| p.container_port() == 80.tcp() && p.host_port() == 18080),
            "tcp の明示マッピングが残ること"
        );
        assert!(
            cfg.ports
                .iter()
                .any(|p| p.container_port() == 80.udp() && p.host_port() == 0),
            "udp の expose が host_port 0 で追加されること"
        );
    }

    #[test]
    fn empty_expose_leaves_ports_empty() {
        // expose も mapped も無いとき ports は空のままであること。
        let req: ContainerRequest<GenericImage> =
            GenericImage::new("alpine", "latest").with_cmd(["sleep", "1"]);
        let cfg = build_container_config(&req);
        assert!(cfg.ports.is_empty(), "ports が空であること");
    }

    #[test]
    fn duplicate_expose_collapses_to_one_mapping() {
        // 同じ ContainerPort を二重に expose しても PortMapping は 1 本であること。
        let req: ContainerRequest<GenericImage> = GenericImage::new("alpine", "latest")
            .with_exposed_port(80.tcp())
            .with_exposed_port(80.tcp())
            .with_cmd(["sleep", "1"]);
        let cfg = build_container_config(&req);
        assert_eq!(cfg.ports.len(), 1, "二重 expose は 1 本に収まること");
        assert_eq!(cfg.ports[0].host_port(), 0);
        assert_eq!(cfg.ports[0].container_port(), 80.tcp());
    }
}
