//! `ContainerAsync` — 実行中コンテナのハンドル。
//! 元の 0.27 の `core::containers::async_container` と同一シグネチャ。
//!
//! macOS (XPC) では内部で `XpcClient` を使い、Apple Container の XPC API を叩く。

pub mod exec;

use std::{
    fmt,
    net::IpAddr,
    pin::Pin,
    sync::{Arc, mpsc},
    time::Duration,
};

#[cfg(target_os = "macos")]
use std::{
    os::fd::{FromRawFd, RawFd},
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

use tokio::io::{AsyncBufRead, AsyncReadExt};

#[cfg(target_os = "macos")]
use tokio::io::ReadBuf;

use crate::core::client::Client;
use crate::core::containers::request::DEFAULT_STARTUP_TIMEOUT;
use crate::core::host::Host;
use crate::{
    ContainerRequest, Image,
    core::{
        WaitFor,
        copy::CopyFileFromContainer,
        error::{Error, Result, WaitContainerError},
        image::{ContainerState, ExecCommand},
        ports::{ContainerPort, Ports},
    },
};

#[cfg(target_os = "macos")]
use crate::core::copy::CopyFromContainerError;
use crate::core::error::ExecError;
#[cfg(target_os = "macos")]
use crate::core::logs::line::deliver_line_to_consumers;

/// ログ取得元の抽象。macOS は FD、Linux は Docker ログストリーム。
///
/// 両バックエンドのログ取得元を単一の型で保持するため enum で表現する。
/// `Arc` を含む variant があるため Copy にはならない。
pub(crate) enum ContainerLogSource {
    /// ログ未取得 (空リーダーを返す)。macOS の `logs()` 失敗時や Linux のフォールバック時。
    None,
    /// macOS (XPC): `containerLogs` が返した stdout / stderr の FD。Drop で close する。
    #[cfg(target_os = "macos")]
    Fd { stdout: RawFd, stderr: RawFd },
    /// Linux (Docker Engine API): demux されたログストリーム。
    #[cfg(target_os = "linux")]
    DockerStream(Arc<crate::core::client::docker_log_stream::DockerLogsHandle>),
}

/// 実行中コンテナ。Drop で削除される。
pub struct ContainerAsync<I: Image> {
    id: String,
    image: ContainerRequest<I>,
    client: Client,
    dropped: bool,
    /// init プロセスの exit code を保持する。
    /// バックグラウンド wait スレッドが `containerWait` (macOS) /
    /// `POST /containers/{id}/wait` (Linux) を待ち、終了時に値が入る。
    /// 世代番号で再 start 後の旧スレッド書き込みを破棄する。
    wait_state: Arc<std::sync::Mutex<WaitState>>,
    /// ログ取得元。再 start 時に差し替えるため Mutex で保持する。
    log_source: std::sync::Mutex<ContainerLogSource>,
    /// LogConsumer 配信タスクへの停止指示 (macOS)。再 start 時に新旧を差し替える。
    #[cfg(target_os = "macos")]
    log_stop: std::sync::Mutex<Arc<AtomicBool>>,
    /// LogConsumer 再 spawn 用に保持する。未登録なら None。
    /// 再 start (`refresh_log_streams`) で consumer を再武装するため両プラットフォームで保持する。
    log_consumers: Option<Arc<Vec<Box<dyn crate::core::logs::consumer::LogConsumer + 'static>>>>,
}

/// `containerWait` スレッドと共有する終了コード状態。
#[derive(Debug, Default)]
pub(crate) struct WaitState {
    /// 再 start のたびに進む世代。旧スレッドの書き込み判定に使う。
    generation: u64,
    exit_code: Option<i64>,
}

impl WaitState {
    /// 世代が一致する場合のみ exit_code を記録する。
    /// 一致して書き込んだら true、旧世代で破棄したら false。
    pub(crate) fn store_if_current(&mut self, generation: u64, code: i64) -> bool {
        if self.generation == generation {
            self.exit_code = Some(code);
            true
        } else {
            false
        }
    }

    /// 世代を進め、exit_code をクリアする。新しい世代番号を返す。
    pub(crate) fn bump(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.exit_code = None;
        self.generation
    }

    pub(crate) fn exit_code(&self) -> Option<i64> {
        self.exit_code
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

/// 空の `WaitState` を共有用に作る。
pub(crate) fn new_wait_state() -> Arc<std::sync::Mutex<WaitState>> {
    Arc::new(std::sync::Mutex::new(WaitState::default()))
}

/// init プロセスの exit code を待つ std スレッドを起動する (macOS)。
///
/// spawn 時点の世代を保持し、書き込み時に世代が一致する場合のみ記録する。
#[cfg(target_os = "macos")]
pub(crate) fn spawn_exit_code_waiter(
    id: String,
    wait_state: Arc<std::sync::Mutex<WaitState>>,
    generation: u64,
) {
    std::thread::spawn(move || {
        if let Ok(code) = crate::core::client::xpc_client::XpcClient::wait_blocking(
            &id,
            &id,
            crate::xpc::LONG_TIMEOUT,
        ) {
            let mut guard = wait_state
                .lock()
                .expect("wait state mutex must not be poisoned while recording exit code");
            let _ = guard.store_if_current(generation, code);
        }
    });
}

/// init プロセスの exit code を待つ std スレッドを起動する (Linux)。
///
/// `DockerClient::wait_blocking` (`POST /containers/{id}/wait?condition=not-running`) を
/// 別スレッドで呼び、終了時に世代が一致する場合のみ exit code を記録する。
/// wait のエラー (削除後の 404 等) は macOS と同様に無視する。
#[cfg(target_os = "linux")]
pub(crate) fn spawn_exit_code_waiter(
    client: std::sync::Arc<crate::core::client::docker_client::DockerClient>,
    id: String,
    wait_state: Arc<std::sync::Mutex<WaitState>>,
    generation: u64,
) {
    std::thread::spawn(move || {
        if let Ok(code) = client.wait_blocking(&id) {
            let mut guard = wait_state
                .lock()
                .expect("wait state mutex must not be poisoned while recording exit code");
            let _ = guard.store_if_current(generation, code);
        }
    });
}

impl<I: Image> ContainerAsync<I> {
    /// `AsyncRunner::start` から呼ばれる構築子。
    ///
    /// macOS は `log_source` の FD から LogConsumer 配信タスクを spawn する。
    /// Linux は配信タスクの spawn は `AsyncRunner::start` 側で行い、ここでは `log_consumers`
    /// の保持だけを行う (再 start 時の再武装用)。
    pub(crate) fn new(
        id: String,
        client: Client,
        #[cfg_attr(target_os = "linux", expect(unused_mut))] mut image: ContainerRequest<I>,
        wait_state: Arc<std::sync::Mutex<WaitState>>,
        log_source: ContainerLogSource,
        #[cfg(target_os = "linux")] log_consumers: Option<
            Arc<Vec<Box<dyn crate::core::logs::consumer::LogConsumer + 'static>>>,
        >,
    ) -> Self {
        // macOS: image から consumer を取り出し、FD に接続して配信タスクを spawn する。
        // 以前は FD 自体を consumer に奪わせていたため、`stdout()` / ログ待機戦略が
        // 空リーダーになり、`with_log_consumer` + `message_on_stdout` の併用が必ず失敗していた。
        // 今は独立オフセットのリーダーを渡すため FD は消費しない。
        #[cfg(target_os = "macos")]
        let (log_stop, log_consumers) = {
            let taken = std::mem::take(&mut image.log_consumers);
            let log_stop = Arc::new(AtomicBool::new(false));
            let log_consumers = if taken.is_empty() {
                None
            } else {
                let consumers = Arc::new(taken);
                let (out_fd, err_fd) = match &log_source {
                    ContainerLogSource::Fd { stdout, stderr } => (Some(*stdout), Some(*stderr)),
                    ContainerLogSource::None => (None, None),
                };
                spawn_log_consumer_task(
                    out_fd,
                    log_stop.clone(),
                    wait_state.clone(),
                    consumers.clone(),
                    crate::core::logs::LogFrame::StdOut,
                );
                spawn_log_consumer_task(
                    err_fd,
                    log_stop.clone(),
                    wait_state.clone(),
                    consumers.clone(),
                    crate::core::logs::LogFrame::StdErr,
                );
                // 再 start 時の再 spawn 用に保持する。
                Some(consumers)
            };
            (std::sync::Mutex::new(log_stop), log_consumers)
        };

        Self {
            id,
            image,
            client,
            dropped: false,
            wait_state,
            log_source: std::sync::Mutex::new(log_source),
            #[cfg(target_os = "macos")]
            log_stop,
            log_consumers,
        }
    }

    /// コンテナ ID を返す。
    pub fn id(&self) -> &str {
        &self.id
    }

    /// イメージを返す。
    pub fn image(&self) -> &I {
        self.image.image()
    }

    /// コンテナの公開ポートマッピングを取得する。
    pub async fn ports(&self) -> Result<Ports> {
        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => c.ports(&self.id).await,
            #[cfg(target_os = "linux")]
            Client::Linux(c) => c.ports(&self.id).await,
        }
    }

    /// コンテナポートに対応するホストの IPv4 ポートを取得する。
    pub async fn get_host_port_ipv4(&self, internal_port: impl Into<ContainerPort>) -> Result<u16> {
        let internal_port = internal_port.into();
        self.ports()
            .await?
            .map_to_host_port_ipv4(internal_port)
            .ok_or_else(|| Error::PortNotExposed {
                id: self.id.clone(),
                port: internal_port,
            })
    }

    /// コンテナポートに対応するホストの IPv6 ポートを取得する。
    pub async fn get_host_port_ipv6(&self, internal_port: impl Into<ContainerPort>) -> Result<u16> {
        let internal_port = internal_port.into();
        self.ports()
            .await?
            .map_to_host_port_ipv6(internal_port)
            .ok_or_else(|| Error::PortNotExposed {
                id: self.id.clone(),
                port: internal_port,
            })
    }

    /// コンテナのブリッジ IP アドレスを取得する。
    pub async fn get_bridge_ip_address(&self) -> Result<IpAddr> {
        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => c.bridge_ip_address(&self.id).await,
            #[cfg(target_os = "linux")]
            Client::Linux(c) => c.bridge_ip_address(&self.id).await,
        }
    }

    /// コンテナのネットワークゲートウェイ IP を取得する (macOS のみ)。
    ///
    /// `ExtraHost::HostGateway` の解決に使う。
    #[cfg(target_os = "macos")]
    pub(crate) async fn gateway_ip_address(&self) -> Result<IpAddr> {
        match &self.client {
            Client::MacOs(c) => c.gateway_ip_address(&self.id).await,
        }
    }

    /// コンテナからホストへファイルをコピーする。
    ///
    /// apple/container の `containerCopyOut` はディレクトリもコピーできるが、
    /// 本 API はファイル専用のため、ディレクトリなら `IsDirectory` で拒否する。
    ///
    /// # Linux
    ///
    /// Docker Engine API の `GET /containers/{id}/archive` で取得した tar を自前 ustar パーサ
    /// (`docker_tar`) で展開し、先頭 regular file の内容を `target` へ渡す。`source` は絶対パス
    /// 必須。ディレクトリを指定すると tar 先頭エントリの typeflag で `IsDirectory` になる。
    /// 受信する tar 全体 (ヘッダ + データ + トレーラ) は 64 MiB 上限で、超過時はエラーを返す
    /// (ファイル内容がちょうど 64 MiB でも tar オーバーヘッド分でエラーになり得る)。
    /// 存在しないコンテナ内パスは `ClientError::ContainerPathNotFound`、コンテナ自体が
    /// 存在しない場合は `ClientError::ContainerNotFound` になる (Linux)。
    /// macOS 側にこの上限は無い。
    pub async fn copy_file_from<T: CopyFileFromContainer>(
        &self,
        source: impl Into<String> + Send,
        target: T,
    ) -> Result<T::Output> {
        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => {
                let source = std::path::PathBuf::from(source.into());
                let temp_path = std::env::temp_dir().join(format!(
                    "container-rs-copy-out-{}-{}",
                    self.id,
                    crate::core::util::unique_suffix()
                ));
                c.copy_out(&self.id, &source, &temp_path).await?;
                // 一時パスはエラーパスでも削除する (以前は ? での早期 return 時に残留していた)。
                let result = async {
                    let meta = tokio::fs::metadata(&temp_path).await?;
                    if meta.is_dir() {
                        return Err(Error::other(CopyFromContainerError::IsDirectory));
                    }
                    let file = tokio::fs::File::open(&temp_path).await?;
                    target.copy_from_reader(file).await.map_err(Error::other)
                }
                .await;
                remove_copy_out_temp(&temp_path).await;
                result
            }
            #[cfg(target_os = "linux")]
            Client::Linux(c) => {
                let source = source.into();
                // Docker Engine の 400 相当を明示エラーで返すため、絶対パスを事前検証する。
                if source.is_empty() || !source.starts_with('/') {
                    return Err(Error::other("copy_file_from path must be absolute"));
                }
                let tar = c.copy_from(&self.id, &source).await?;
                let content =
                    crate::core::client::docker_tar::parse_first_regular_file_from_ustar(&tar)
                        .map_err(Error::other)?;
                target
                    .copy_from_reader(std::io::Cursor::new(content))
                    .await
                    .map_err(Error::other)
            }
        }
    }

    /// コンテナに接続するためのホストを返す。
    pub async fn get_host(&self) -> Result<Host> {
        // macOS / Linux ともホストは localhost。
        Ok(Host::parse("localhost"))
    }

    /// コンテナ内でコマンドを実行する。
    ///
    /// `ExecCommand::container_ready_conditions` の待機には start 時の `startup_timeout`
    /// (未設定の場合は既定値 60 秒) が適用され、超過時は `WaitContainerError::StartupTimeout`
    /// になる。ログ取得元が無い (macOS の `containerLogs` 失敗時 / Linux のログストリーム
    /// 欠如時) のに `WaitFor::Log` を含む ready_conditions を指定した場合は、コンテナ内
    /// コマンドの実行前に明示エラーで打ち切る。
    ///
    /// # Linux
    ///
    /// Docker Engine API の exec は `AttachStdout` / `AttachStderr` で stdout / stderr を
    /// 取得する。`CmdWaitFor::StdOutMessage` / `StdErrMessage` は取得済みバッファに
    /// 対する部分一致で判定する。`ExecCommand::with_env_vars` はコンテナ env を
    /// inspect で取得し、exec 分で上書きマージして `ExecConfig.Env` に設定する。
    ///
    /// 出力には 64 MiB の蓄積上限がある。上限は demux 前の multiplexed stream 全体
    /// (stdout + stderr の合計、フレームヘッダ込み) に適用されるため、実効上限は
    /// macOS の stdout / stderr 各 64 MiB より厳しい。蓄積超過の時点で切り詰めず
    /// 即座にエラーを返し、exit code を取得できない (コンテナ内のプロセスが継続
    /// するかは実測されていない)。
    pub async fn exec(&self, cmd: ExecCommand) -> Result<exec::ExecResult> {
        let ExecCommand {
            cmd,
            container_ready_conditions,
            cmd_ready_condition,
            env_vars,
        } = cmd;

        // ログ取得元が無い (log_source = None) のに WaitFor::Log を含む ready_conditions を
        // 待機すると、空リーダー + exit_code_hint 未観測のままポーリングが永久に回る
        // (コンテナ生存中は exit_code_hint が None のままのため)。start 側と同じ趣旨の
        // 明示エラーで、コンテナ内コマンドの実行前に打ち切る。
        if self.log_source_is_none() && ready_conditions_require_log(&container_ready_conditions) {
            return Err(crate::Error::other(
                "log wait requires a log source, but none is available",
            ));
        }

        let cmd_owned: Vec<String> = cmd;
        let raw = match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => {
                // XPC は environment が空だとコンテナ env を継承しないため、
                // ContainerRequest の env を基底にし、ExecCommand の env で上書きする。
                let environment = crate::core::env::fold_env(
                    self.image
                        .env_vars()
                        .map(|(k, v)| (k.into_owned(), v.into_owned())),
                    env_vars,
                );
                c.exec(&self.id, &cmd_owned, environment).await?
            }
            #[cfg(target_os = "linux")]
            Client::Linux(c) => {
                // Docker は Env 省略時にコンテナ env を継承するが、Env 指定時は置換する。
                // env_vars が非空ならコンテナ env を inspect で取得し、exec 分で上書きマージする。
                if env_vars.is_empty() {
                    c.exec(&self.id, &cmd_owned, Vec::new()).await?
                } else {
                    let container_env = c.container_env(&self.id).await?;
                    // "KEY=VALUE" 形式を展開し、exec 分で上書きする
                    let env = crate::core::env::fold_env(
                        container_env.iter().filter_map(|s| {
                            let (k, v) = s.split_once('=')?;
                            Some((k.to_string(), v.to_string()))
                        }),
                        env_vars,
                    );
                    c.exec(&self.id, &cmd_owned, env).await?
                }
            }
        };

        // container_ready_conditions を待機。start 側 (run_ready_sequence) と同じ
        // startup_timeout を適用して無期限待ちを防ぐ。未設定 (None) の場合は
        // start 側と同じ既定値 60 秒を使う。
        let startup_timeout = self
            .image
            .startup_timeout()
            .unwrap_or(DEFAULT_STARTUP_TIMEOUT);
        tokio::time::timeout(
            startup_timeout,
            self.block_until_ready(container_ready_conditions),
        )
        .await
        .map_err(|_| WaitContainerError::StartupTimeout {
            id: self.id.to_string(),
            timeout: startup_timeout,
        })??;

        // cmd_ready_condition の処理。
        match cmd_ready_condition {
            crate::core::CmdWaitFor::StdOutMessage { message } => {
                // exec はコマンド完走後に stdout / stderr 全体を取得済みなので、
                // メッセージ待ちは取得済みバッファに対する部分一致で判定する。
                if !contains_bytes(&raw.stdout, &message) {
                    return Err(crate::core::error::Error::other(format!(
                        "expected message not found in stdout: {}",
                        String::from_utf8_lossy(&message)
                    )));
                }
            }
            crate::core::CmdWaitFor::StdErrMessage { message } => {
                if !contains_bytes(&raw.stderr, &message) {
                    return Err(crate::core::error::Error::other(format!(
                        "expected message not found in stderr: {}",
                        String::from_utf8_lossy(&message)
                    )));
                }
            }
            crate::core::CmdWaitFor::Exit { code: None } => {
                // 終了コード不問ならこの時点で条件成立。
            }
            crate::core::CmdWaitFor::Exit { code: Some(code) } => {
                // exit code が取得できなかった場合は「確認できない = 成功」ではなく
                // エラーにする (黙って合格させると失敗コマンドを見逃す)。
                match raw.exit_code {
                    Some(actual) if actual != code => {
                        return Err(ExecError::ExitCodeMismatch {
                            expected: code,
                            actual,
                        }
                        .into());
                    }
                    Some(_) => {}
                    None => {
                        return Err(crate::core::error::Error::other(
                            "exec exit code is unavailable, cannot verify expected exit code",
                        ));
                    }
                }
            }
            crate::core::CmdWaitFor::Duration { length } => {
                tokio::time::sleep(length).await;
            }
            crate::core::CmdWaitFor::Nothing => {}
        }

        Ok(exec::ExecResult {
            exit_code: raw.exit_code,
            stdout: std::io::Cursor::new(raw.stdout),
            stderr: std::io::Cursor::new(raw.stderr),
        })
    }

    /// 停止済みなら再起動し、その後 `Image::exec_after_start` を実行する。
    ///
    /// macOS では停止済みの場合に Apple container 公式 CLI と同じく
    /// `containerBootstrap` + `containerStartProcess` を再送して再起動する。
    /// 起動済みの場合はプロセス起動を行わず `exec_after_start` のみを実行する。
    /// Linux では停止済みの場合に Docker Engine API の `start` を呼んで再起動する。
    pub async fn start(&self) -> Result<()> {
        // 停止済みの場合は再起動する。
        #[cfg(target_os = "macos")]
        if let Client::MacOs(c) = &self.client
            && !c.container_state(&self.id).await?.running
        {
            // Apple container の公式 CLI `container start` と同じ経路。
            c.bootstrap_container(&self.id).await?;
            c.start_process(&self.id).await?;
            self.reset_wait_state_and_respawn();
            // 再 bootstrap 後は旧ログ FD が死ぬため、差し替えて consumer も再武装する。
            // 差し替えに失敗した場合は、ログ系 API が機能しない実行中コンテナを残さず
            // SIGKILL で巻き戻してから元のエラーを返す。巻き戻しは Linux の refresh 失敗時と
            // 同じ方針で、コンテナ再起動を内部に持たない単一責務の refresh_log_streams の
            // 呼び出し側 (start) で完結させる。
            if let Err(e) = self.refresh_log_streams(c).await {
                // stop_with_timeout は stop_log_delivery を経由して現役の LogConsumer タスクも
                // 停止するため、client.stop 直接呼びより望ましい。
                if let Err(stop_err) = self.stop_with_timeout(Some(0)).await {
                    tracing::warn!(
                        "failed to stop container after log refresh failure: {stop_err}"
                    );
                    // 巻き戻し失敗時は実行中コンテナ + 死んだログ FD が残る。次回 start は
                    // running のため再起動分岐をスキップしてログは回復しない。先に stop() を
                    // 呼んでから start() すると回復する。
                }
                return Err(e);
            }
        }
        #[cfg(target_os = "linux")]
        if let Client::Linux(c) = &self.client
            && !c.container_state(&self.id).await?.running
        {
            // 旧ログストリーム停止 → コンテナ再起動 → 新ログストリーム起動を一括で行う。
            self.refresh_log_streams(c).await?;
            // 再起動成功後に wait スレッドを再武装する。refresh_log_streams より前に
            // 再武装すると、停止中のコンテナに対して waiter が旧 exit code を即取得してしまう。
            self.reset_wait_state_and_respawn();
        }

        // exec_after_start を実行する (本家の公開 start() と同じ構造)。
        let state = self.container_state().await?;
        for cmd in self.image.exec_after_start(state)? {
            self.exec(cmd).await?;
        }
        Ok(())
    }

    /// 再 start 時に exit_code をリセットし、containerWait スレッドを再武装する。
    #[cfg(target_os = "macos")]
    fn reset_wait_state_and_respawn(&self) {
        let generation = self
            .wait_state
            .lock()
            .expect("wait state mutex must not be poisoned while restarting container")
            .bump();
        spawn_exit_code_waiter(self.id.clone(), self.wait_state.clone(), generation);
    }

    /// 再 start 時に exit_code をリセットし、containerWait スレッドを再武装する (Linux)。
    ///
    /// `refresh_log_streams` 成功後に呼ぶこと。macOS のように `refresh_log_streams` より
    /// 前に再武装すると、停止中のコンテナに対して waiter が旧 exit code を即取得し、
    /// 新世代として記録してしまう。
    #[cfg(target_os = "linux")]
    fn reset_wait_state_and_respawn(&self) {
        let generation = self
            .wait_state
            .lock()
            .expect("wait state mutex must not be poisoned while restarting container")
            .bump();
        let Client::Linux(c) = &self.client;
        spawn_exit_code_waiter(
            c.clone(),
            self.id.clone(),
            self.wait_state.clone(),
            generation,
        );
    }

    /// 再 start 後にログ FD を再取得し、旧 FD を close、LogConsumer を再 spawn する (macOS)。
    ///
    /// `logs()` が成功するまで旧 `log_stop` / FD / consumer は維持する。
    /// 失敗時は部分更新せず `Err` を返す (呼び出し側の `start` が SIGKILL で巻き戻す)。
    #[cfg(target_os = "macos")]
    async fn refresh_log_streams(
        &self,
        client: &crate::core::client::xpc_client::XpcClient,
    ) -> Result<()> {
        let (new_out, new_err) = client.logs(&self.id).await.map_err(|e| {
            crate::Error::other(format!("failed to refresh log fds after restart: {e}"))
        })?;

        // 成功後にだけ旧 consumer を止め、stop フラグと FD を差し替える。
        self.log_stop
            .lock()
            .expect("log stop mutex must not be poisoned while refreshing log file descriptors")
            .store(true, Ordering::Relaxed);
        let new_stop = Arc::new(AtomicBool::new(false));
        *self
            .log_stop
            .lock()
            .expect("log stop mutex must not be poisoned while refreshing log file descriptors") =
            new_stop.clone();

        {
            let mut source = self
                .log_source
                .lock()
                .expect("log source mutex must not be poisoned while refreshing logs");
            // 旧 FD を close。
            if let ContainerLogSource::Fd { stdout, stderr } = *source {
                unsafe { libc::close(stdout) };
                unsafe { libc::close(stderr) };
            }
            *source = ContainerLogSource::Fd {
                stdout: new_out,
                stderr: new_err,
            };
        }

        if let Some(consumers) = &self.log_consumers {
            spawn_log_consumer_task(
                Some(new_out),
                new_stop.clone(),
                self.wait_state.clone(),
                consumers.clone(),
                crate::core::logs::LogFrame::StdOut,
            );
            spawn_log_consumer_task(
                Some(new_err),
                new_stop,
                self.wait_state.clone(),
                consumers.clone(),
                crate::core::logs::LogFrame::StdErr,
            );
        }

        Ok(())
    }

    /// 再 start 時にログストリームを再武装する (Linux)。
    ///
    /// race 防止と失敗時整合性のため、旧経路の停止を `start_container` (docker restart) より
    /// 前に行う。新規セッション起動に失敗した場合は `stop_container` で巻き戻し、docker 側だけ
    /// 動いて log が古い、というちぐはぐな状態を残さない。
    ///
    /// 旧経路を先に停止する設計上、新規セッション起動に失敗して `Err` を返した時点で
    /// `log_source` は停止済み (terminated) の旧ハンドルを指したままになる。この場合ログは
    /// 即 EOF になるが、次回の `start()` で再 refresh されて回復する。
    #[cfg(target_os = "linux")]
    async fn refresh_log_streams(
        &self,
        client: &crate::core::client::docker_client::DockerClient,
    ) -> Result<()> {
        use crate::core::client::docker_log_stream::spawn_log_consumer_task;

        // 1. 旧経路の停止 (docker restart より前)。
        let old_handle = {
            let source = self
                .log_source
                .lock()
                .expect("log source mutex must not be poisoned while refreshing logs");
            match &*source {
                ContainerLogSource::DockerStream(handle) => Some(handle.clone()),
                ContainerLogSource::None => None,
            }
        };
        if let Some(handle) = &old_handle {
            handle.stop();
        }

        // 2. コンテナ再起動。
        client.start_container(&self.id).await?;

        // ログストリームが無かったコンテナは再起動後もログ無しでよい。
        if old_handle.is_none() {
            return Ok(());
        }

        // 3. 新規 follow=true セッション起動。失敗時は docker 側を止めて巻き戻す。
        let new_handle = match client.spawn_log_session(&self.id).await {
            Ok(handle) => handle,
            Err(e) => {
                if let Err(stop_err) = client.stop(&self.id, Some(0)).await {
                    tracing::warn!(
                        "failed to stop container after log refresh failure: {stop_err}"
                    );
                }
                return Err(e);
            }
        };

        // 4-5. log_source を新ストリームに差し替え。
        {
            let mut source = self
                .log_source
                .lock()
                .expect("log source mutex must not be poisoned while refreshing logs");
            *source = ContainerLogSource::DockerStream(new_handle.clone());
        }

        // 6. consumer 再 spawn。
        if let Some(consumers) = &self.log_consumers {
            spawn_log_consumer_task(
                new_handle.clone(),
                new_handle.stdout_stream(),
                consumers.clone(),
                crate::core::logs::LogFrame::StdOut,
            );
            spawn_log_consumer_task(
                new_handle.clone(),
                new_handle.stderr_stream(),
                consumers.clone(),
                crate::core::logs::LogFrame::StdErr,
            );
        }

        Ok(())
    }

    /// コンテナを停止する。デフォルトのタイムアウトで SIGTERM を送信する。
    pub async fn stop(&self) -> Result<()> {
        self.stop_with_timeout(None).await
    }

    /// タイムアウトを指定してコンテナを停止する。
    ///
    /// `timeout_seconds` が `None` の場合はデフォルト (SIGTERM + 30 秒)、
    /// `Some(0)` の場合は即時 SIGKILL。負値は macOS では無限待ちに近いタイムアウト
    /// (`i32::MAX` 秒) で SIGTERM、Linux では 30 秒に変換される。
    ///
    /// macOS では、負値 (またはグレース + 30 秒が 24 時間を超える正の値) を指定すると、
    /// SIGTERM を無視するコンテナでこの呼び出しが最大 24 時間ブロックされた後、
    /// XPC タイムアウトのエラーが返り得る。Linux では指定グレース時間のまま待つ。
    pub async fn stop_with_timeout(&self, timeout_seconds: Option<i32>) -> Result<()> {
        // LogConsumer 配信 / ログストリームを止める (Drop を待たない)。
        self.stop_log_delivery();

        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => c.stop(&self.id, timeout_seconds).await,
            #[cfg(target_os = "linux")]
            Client::Linux(c) => c.stop(&self.id, timeout_seconds).await,
        }
    }

    /// コンテナを一時停止する。Linux (Docker) のみ対応。
    #[cfg(target_os = "linux")]
    pub async fn pause(&self) -> Result<()> {
        match &self.client {
            Client::Linux(c) => c.pause(&self.id).await,
        }
    }

    /// コンテナの一時停止を解除する。Linux (Docker) のみ対応。
    #[cfg(target_os = "linux")]
    pub async fn unpause(&self) -> Result<()> {
        match &self.client {
            Client::Linux(c) => c.unpause(&self.id).await,
        }
    }

    /// ログ配信を停止する。macOS は停止フラグ、Linux はログストリームの `shutdown`。
    ///
    /// `stop` / `rm` / `Drop` の共通前置きとして呼ぶ。
    fn stop_log_delivery(&self) {
        #[cfg(target_os = "macos")]
        {
            self.log_stop
                .lock()
                .expect("log stop mutex must not be poisoned while stopping log delivery")
                .store(true, Ordering::Relaxed);
        }
        #[cfg(target_os = "linux")]
        {
            let source = self
                .log_source
                .lock()
                .expect("log source mutex must not be poisoned while stopping log delivery");
            if let ContainerLogSource::DockerStream(handle) = &*source {
                handle.stop();
            }
        }
    }

    /// コンテナが実行中かどうかを返す。
    pub async fn is_running(&self) -> Result<bool> {
        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => Ok(c.container_state(&self.id).await?.running),
            #[cfg(target_os = "linux")]
            Client::Linux(c) => Ok(c.container_state(&self.id).await?.running),
        }
    }

    /// バックグラウンドの `containerWait` が観測した exit code を返す (未終了なら None)。
    /// XPC 呼び出しを伴わない即時のヒント。ログ待機戦略の終了判定に使う。
    pub(crate) fn exit_code_hint(&self) -> Option<i64> {
        self.wait_state
            .lock()
            .expect("wait state mutex must not be poisoned while reading exit code")
            .exit_code()
    }

    /// ログストリームが終端したか (Linux の demux 完了)。macOS では常に false。
    ///
    /// `LogWaitStrategy` の EOF 判定に使う。macOS は `exit_code_hint` 経路で EOF 判定するため
    /// 常に false を返す。Linux は demux タスクが TCP EOF / 停止を検出すると true になる。
    pub(crate) fn logs_terminated(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            false
        }
        #[cfg(target_os = "linux")]
        {
            let source = self
                .log_source
                .lock()
                .expect("log source mutex must not be poisoned while checking log termination");
            match &*source {
                ContainerLogSource::DockerStream(handle) => handle.logs_terminated(),
                ContainerLogSource::None => false,
            }
        }
    }

    /// コンテナの exit code を返す (未終了なら `None`)。
    ///
    /// macOS: バックグラウンド wait のキャッシュを優先し、停止済みかつ未観測の場合は
    /// 短いタイムアウト (5 秒) で都度 `containerWait` を呼んで取得を試みる。
    /// 取得失敗時は `Ok(None)` を返す (エラーにしない)。都度の wait の間に再 start で
    /// 世代が進んだ場合は、取得した値が現世代のものか確証が持てないため `Ok(None)` を
    /// 返す (新世代のバックグラウンド wait が記録するため、新コンテナ終了後の呼び出しで
    /// 取得できる見込みがある)。
    pub async fn exit_code(&self) -> Result<Option<i64>> {
        // バックグラウンドの containerWait が既に終了コードを取得していれば返す。
        if let Some(code) = self
            .wait_state
            .lock()
            .expect("wait state mutex must not be poisoned while reading exit code")
            .exit_code()
        {
            return Ok(Some(code));
        }

        // running 中は exit code を持たない。
        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => {
                if c.container_state(&self.id).await?.running {
                    Ok(None)
                } else {
                    // 停止済みだがバックグラウンド wait が未完了。
                    // 短いタイムアウトで都度 containerWait を呼んで exit code を取得する。
                    let id = self.id.clone();
                    let wait_state = self.wait_state.clone();
                    let generation = self
                        .wait_state
                        .lock()
                        .expect("wait state mutex must not be poisoned")
                        .generation();
                    let result = tokio::task::spawn_blocking(move || {
                        crate::core::client::xpc_client::XpcClient::wait_blocking(
                            &id,
                            &id,
                            std::time::Duration::from_secs(5),
                        )
                    })
                    .await;
                    match result {
                        Ok(Ok(code)) => {
                            let mut guard = wait_state.lock().expect(
                                "wait state mutex must not be poisoned while recording exit code",
                            );
                            if guard.store_if_current(generation, code) {
                                Ok(Some(code))
                            } else {
                                // 世代不一致 (都度の wait の間に再 start が挟まった)。
                                // 取得した値が現世代のものか確証が持てないため、呼び出し側が
                                // 旧コンテナの exit code を現在のものと誤認しないよう
                                // `Ok(None)` に倒す。
                                Ok(None)
                            }
                        }
                        // タイムアウト・XPC 失敗・runtime 解放済みは None を返す。
                        _ => Ok(None),
                    }
                }
            }
            #[cfg(target_os = "linux")]
            Client::Linux(c) => {
                // running の確認はコンテナ不存在 (404) の検出を兼ねる。exit code は
                // バックグラウンド wait の観測値のみで、停止済みでも未観測なら None を返す。
                let _ = c.container_state(&self.id).await?.running;
                Ok(None)
            }
        }
    }

    /// コンテナを削除する。
    ///
    /// # 完了保証
    ///
    /// コンテナを削除する。
    ///
    /// # 完了保証
    ///
    /// `rm().await` の復帰時点で削除処理は終わっており、成否は返り値の `Result` として
    /// 呼び出し側に届く。`Drop` は `DROP_REMOVE_TIMEOUT` (5 秒) 内で完了を待つが、
    /// 超過時は best-effort であり成否の `Result` も返さないため、確実な完了保証と
    /// 成否が必要ならこのメソッドを使うこと。
    ///
    /// # `keep` ゲートとの非対称
    ///
    /// このメソッドは `TESTCONTAINERS_COMMAND=keep` でも削除する。`keep` ゲートは
    /// `Drop` の削除のみを抑止する仕様であり、明示 `rm` には効かない。
    ///
    /// 削除は常に `force=true` で行われるため実行中でもそのまま削除でき、バックエンドが
    /// 404 を返した場合 (既に削除済み) は冪等成功として扱う。
    pub async fn rm(mut self) -> Result<()> {
        // 共通の前置き: remove より前にログストリームを止める。
        // remove の await 中に demux 側の完了フラグが立つため、後続の Drop での polling が
        // ほぼ即抜けする。
        self.stop_log_delivery();
        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => c.remove(&self.id, true).await?,
            #[cfg(target_os = "linux")]
            Client::Linux(c) => c.remove(&self.id, true).await?,
        }
        self.dropped = true;
        Ok(())
    }

    /// コンテナを同期的に削除する。tokio Runtime 内外のどちらから呼んでも安全。
    ///
    /// # 完了保証
    ///
    /// `Ok` を返した時点で削除処理は終わっており、成否は返り値の `Result` として
    /// 呼び出し側に届く。`block_on` を使わず `remove_blocking` (同期 I/O) を直接
    /// 呼び出すため、tokio Runtime 内の同期コンテキスト (Drop ガードや
    /// `spawn_blocking` 内) から呼んでも deadlock しない。
    ///
    /// # `rm()` との使い分け
    ///
    /// - async コンテキストから呼べるなら `rm().await` を使う (非同期 I/O で待つ)
    /// - 同期コンテキスト (Runtime 内の Drop ガード、`spawn_blocking` 内、Runtime 外)
    ///   から削除完了を待ちたい場合はこのメソッドを使う
    /// - `Drop` は `DROP_REMOVE_TIMEOUT` (5 秒) 内で完了を待つが、超過時は best-effort
    ///   であり成否の `Result` も返さない。確実な完了保証と成否が必要ならこのメソッド
    ///   または `rm()` を使うこと
    ///
    /// # `keep` ゲートとの非対称
    ///
    /// このメソッドは `TESTCONTAINERS_COMMAND=keep` でも削除する。`keep` ゲートは
    /// `Drop` の削除のみを抑止する仕様であり、明示 `rm` / `rm_blocking` には効かない。
    ///
    /// 削除は常に `force=true` で行われるため実行中でもそのまま削除でき、バックエンドが
    /// 404 を返した場合 (既に削除済み) は冪等成功として扱う。
    pub fn rm_blocking(mut self) -> Result<()> {
        // 共通の前置き: remove より前にログストリームを止める。
        self.stop_log_delivery();
        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(_) => {
                crate::core::client::xpc_client::XpcClient::remove_blocking(&self.id, true)?
            }
            #[cfg(target_os = "linux")]
            Client::Linux(c) => c.remove_blocking(&self.id, true)?,
        }
        self.dropped = true;
        Ok(())
    }

    /// stdout のリーダーを返す。
    ///
    /// # macOS
    ///
    /// `containerLogs` から取得した FD を pread で読む。リーダーは独立した読み取り位置を
    /// 持ち、常にログ先頭から読む。`follow = true` のときは末尾到達後も追記をポーリングする。
    /// init プロセス終了またはコンテナ Drop / ログ FD 差し替えで EOF する。
    ///
    /// # Linux
    ///
    /// `follow = true` では demux 開始時点以降のログを先頭から読む。8 MiB 上限で先頭が
    /// drop された場合は取りこぼした旨を `warn` ログに出力し、読み進みは継続する。再 start
    /// (`refresh_log_streams`) で demux 開始時点が更新されると、以前に取得した古いリーダーは
    /// 新バッファに接続されない。`follow = false` では呼び出しごとに新規 HTTP セッションを
    /// 張って現時点までの全ログを取得するため、ループで N 回呼ばず結果を保持すること。
    /// `follow = false` (1-shot) は各ストリーム 64 MiB 上限で、超過時は読み出しを
    /// 即座に止めてエラーを返す (stdout / stderr は同一セッションで取得するため、片方の
    /// ストリームの超過で両方の取得が失敗し、合計最大 128 MiB が一時保持され得る)。
    /// macOS 側にこの上限は無い。
    pub fn stdout(&self, follow: bool) -> Pin<Box<dyn AsyncBufRead + Send>> {
        let source = self
            .log_source
            .lock()
            .expect("log source mutex must not be poisoned while reading logs");
        match &*source {
            ContainerLogSource::None => Box::pin(tokio::io::BufReader::new(tokio::io::empty())),
            #[cfg(target_os = "macos")]
            ContainerLogSource::Fd { stdout, .. } => fd_reader_or_empty(
                Some(*stdout),
                follow,
                self.log_stop_flag(),
                self.wait_state.clone(),
            ),
            #[cfg(target_os = "linux")]
            ContainerLogSource::DockerStream(handle) => {
                if follow {
                    Box::pin(handle.stdout_reader())
                } else {
                    Box::pin(tokio::io::BufReader::new(handle.stdout_oneshot()))
                }
            }
        }
    }

    /// stderr のリーダーを返す。
    ///
    /// # macOS
    ///
    /// `containerLogs` から取得した FD を pread で読む。リーダーは独立した読み取り位置を
    /// 持ち、常にログ先頭から読む。`follow = true` のときは末尾到達後も追記をポーリングする。
    /// 注意: Apple container の `containerLogs` が返す 2 本目の FD は VM の bootlog であり、
    /// アプリケーションの stderr は stdout 側のログに混流する。
    ///
    /// # Linux
    ///
    /// Docker Engine API は STREAM_TYPE で stdout / stderr を分離するため、`follow = true`
    /// では本当に stderr のみのログを読む (macOS より優れた挙動)。8 MiB 上限で先頭が
    /// drop された場合は取りこぼした旨を `warn` ログに出力し、読み進みは継続する。
    /// `follow = false` では呼び出しごとに新規 HTTP セッションを張って現時点までの全ログを
    /// 取得するため、ループで N 回呼ばず結果を保持すること。
    /// `follow = false` (1-shot) は各ストリーム 64 MiB 上限で、超過時は読み出しを
    /// 即座に止めてエラーを返す (stdout / stderr は同一セッションで取得するため、片方の
    /// ストリームの超過で両方の取得が失敗し、合計最大 128 MiB が一時保持され得る)。
    /// macOS 側にこの上限は無い。
    pub fn stderr(&self, follow: bool) -> Pin<Box<dyn AsyncBufRead + Send>> {
        let source = self
            .log_source
            .lock()
            .expect("log source mutex must not be poisoned while reading logs");
        match &*source {
            ContainerLogSource::None => Box::pin(tokio::io::BufReader::new(tokio::io::empty())),
            #[cfg(target_os = "macos")]
            ContainerLogSource::Fd { stderr, .. } => fd_reader_or_empty(
                Some(*stderr),
                follow,
                self.log_stop_flag(),
                self.wait_state.clone(),
            ),
            #[cfg(target_os = "linux")]
            ContainerLogSource::DockerStream(handle) => {
                if follow {
                    Box::pin(handle.stderr_reader())
                } else {
                    Box::pin(tokio::io::BufReader::new(handle.stderr_oneshot()))
                }
            }
        }
    }

    /// stdout の同期リーダーを返す。
    ///
    /// リーダーは独立した読み取り位置を持ち、常にログ先頭から読む。
    /// `follow = true` のときは末尾到達後も追記をポーリングする。呼び出しスレッドをブロックするため、
    /// tokio ランタイムワーカー上や LogConsumer コールバックからは呼ばないこと。
    /// Linux の `follow = true` は `tokio::sync::Notify` を使わず `park_timeout(50ms)` で周期起床する。
    pub(crate) fn stdout_sync(&self, follow: bool) -> Box<dyn std::io::BufRead + Send> {
        let source = self
            .log_source
            .lock()
            .expect("log source mutex must not be poisoned while reading logs");
        match &*source {
            ContainerLogSource::None => Box::new(std::io::BufReader::new(std::io::empty())),
            #[cfg(target_os = "macos")]
            ContainerLogSource::Fd { stdout, .. } => fd_reader_or_empty_sync(
                Some(*stdout),
                follow,
                self.log_stop_flag(),
                self.wait_state.clone(),
            ),
            #[cfg(target_os = "linux")]
            ContainerLogSource::DockerStream(handle) => {
                if follow {
                    Box::new(std::io::BufReader::new(handle.stdout_sync_reader()))
                } else {
                    Box::new(std::io::BufReader::new(handle.stdout_sync_oneshot()))
                }
            }
        }
    }

    /// stderr の同期リーダーを返す。
    ///
    /// リーダーは独立した読み取り位置を持ち、常にログ先頭から読む。
    /// `follow = true` のときは末尾到達後も追記をポーリングする。呼び出しスレッドをブロックするため、
    /// tokio ランタイムワーカー上や LogConsumer コールバックからは呼ばないこと。
    /// Linux の `follow = true` は `tokio::sync::Notify` を使わず `park_timeout(50ms)` で周期起床する。
    ///
    /// 注意: Apple container の `containerLogs` が返す 2 本目の FD は VM の bootlog であり、
    /// アプリケーションの stderr は stdout 側のログに混流する。
    pub(crate) fn stderr_sync(&self, follow: bool) -> Box<dyn std::io::BufRead + Send> {
        let source = self
            .log_source
            .lock()
            .expect("log source mutex must not be poisoned while reading logs");
        match &*source {
            ContainerLogSource::None => Box::new(std::io::BufReader::new(std::io::empty())),
            #[cfg(target_os = "macos")]
            ContainerLogSource::Fd { stderr, .. } => fd_reader_or_empty_sync(
                Some(*stderr),
                follow,
                self.log_stop_flag(),
                self.wait_state.clone(),
            ),
            #[cfg(target_os = "linux")]
            ContainerLogSource::DockerStream(handle) => {
                if follow {
                    Box::new(std::io::BufReader::new(handle.stderr_sync_reader()))
                } else {
                    Box::new(std::io::BufReader::new(handle.stderr_sync_oneshot()))
                }
            }
        }
    }

    /// 現在の LogConsumer / follow 停止フラグを返す (macOS)。
    #[cfg(target_os = "macos")]
    fn log_stop_flag(&self) -> Arc<AtomicBool> {
        self.log_stop
            .lock()
            .expect("log stop mutex must not be poisoned while creating log readers")
            .clone()
    }

    /// stdout を全量読み出して `Vec<u8>` で返す (follow なし)。
    ///
    /// Linux では各ストリーム 64 MiB 上限で、超過時はエラーを返す (切り詰めない)。
    /// macOS 側にこの上限は無い。
    pub async fn stdout_to_vec(&self) -> Result<Vec<u8>> {
        let mut stdout = Vec::new();
        self.stdout(false).read_to_end(&mut stdout).await?;
        Ok(stdout)
    }

    /// stderr を全量読み出して `Vec<u8>` で返す (follow なし)。
    ///
    /// Linux では各ストリーム 64 MiB 上限で、超過時はエラーを返す (切り詰めない)。
    /// macOS 側にこの上限は無い。
    pub async fn stderr_to_vec(&self) -> Result<Vec<u8>> {
        let mut stderr = Vec::new();
        self.stderr(false).read_to_end(&mut stderr).await?;
        Ok(stderr)
    }

    /// ログ取得元が無い (空リーダーを返す) 状態かどうか。
    fn log_source_is_none(&self) -> bool {
        let source = self
            .log_source
            .lock()
            .expect("log source mutex must not be poisoned");
        matches!(*source, ContainerLogSource::None)
    }

    /// コンテナの準備完了まで待機する。
    pub(crate) async fn block_until_ready(&self, ready_conditions: Vec<WaitFor>) -> Result<()> {
        for condition in ready_conditions {
            condition.wait_until_ready(&self.client, self).await?;
        }
        Ok(())
    }

    /// `ContainerState` を取得する。
    ///
    /// macOS (Apple container) では XPC `containerState` ルートを使って最新の状態を取得する。
    /// Linux (Docker) では Docker Engine API の inspect から状態 snapshot を組み立てる。
    pub async fn container_state(&self) -> Result<ContainerState> {
        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => {
                let ports = c.container_state(&self.id).await?.ports;
                let host = self.get_host().await?;
                Ok(ContainerState::new(self.id.clone(), host, ports))
            }
            #[cfg(target_os = "linux")]
            Client::Linux(c) => {
                let ports = c.container_state(&self.id).await?.ports;
                let host = self.get_host().await?;
                Ok(ContainerState::new(self.id.clone(), host, ports))
            }
        }
    }
}

/// `haystack` に `needle` が部分一致で含まれるか。空の `needle` は常に true。
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// ready_conditions が `WaitFor::Log` を 1 つでも含むか。
///
/// start 側 (macOS のログ FD 欠如判定) と exec 側で共用する共通述語。
/// ログ取得元が無い場合にこの条件を含む待機は永久に回るため、事前に明示エラーにする。
///
/// 空メッセージの `WaitFor::Log` (`message_on_stdout("")` 等) はログ取得元が無くても
/// 即成立するが、判定はメッセージの内容を見ず `WaitFor::Log` の存在だけで true に
/// する (start 側と同粒度。空メッセージ待機は実用頻度が低く、事前エラー側に倒しても
/// 実害は小さい)。
pub(crate) fn ready_conditions_require_log(ready_conditions: &[WaitFor]) -> bool {
    ready_conditions
        .iter()
        .any(|c| matches!(c, WaitFor::Log(_)))
}

/// pread ベースの独立オフセットリーダー。
///
/// `dup(2)` はオープンファイル記述 (ファイルオフセット) を共有するため、複数のリーダーが
/// 互いの読み取り位置を進めてしまい「後から読んだ側にログの前半が見えない」誤動作になる。
/// `read_at` (pread) は共有オフセットを使わず動かさないので、各リーダーが独立に読める。
/// `containerLogs` の FD は通常ファイルなので pread が使える。
#[cfg(target_os = "macos")]
struct FdReader {
    file: std::fs::File,
    pos: u64,
}

#[cfg(target_os = "macos")]
impl FdReader {
    /// `fd` を dup して所有するリーダーを作る。dup 失敗時は None。
    fn dup_from(fd: RawFd) -> Option<Self> {
        let duped = unsafe { libc::dup(fd) };
        if duped < 0 {
            return None;
        }
        Some(Self {
            file: unsafe { std::fs::File::from_raw_fd(duped) },
            pos: 0,
        })
    }

    /// `pread` で読み取り、リーダー固有の位置だけを進める。
    fn read_at(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::os::unix::fs::FileExt;

        // ローカルのログファイルへの pread は実質ブロックしないため同期呼び出しでよい。
        let n = loop {
            match self.file.read_at(buf, self.pos) {
                Ok(n) => break n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        };
        self.pos += n as u64;
        Ok(n)
    }
}

#[cfg(target_os = "macos")]
impl std::io::Read for FdReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.read_at(buf)
    }
}

#[cfg(target_os = "macos")]
impl tokio::io::AsyncRead for FdReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let dst = buf.initialize_unfilled();
        match this.read_at(dst) {
            Ok(n) => {
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

/// `follow` 対応のログリーダー。
///
/// `follow = false` では末尾 (0 バイト) で EOF。`follow = true` では追記をポーリングし、
/// init プロセス終了または `stop` フラグで EOF する。
#[cfg(target_os = "macos")]
struct FollowFdReader {
    inner: FdReader,
    follow: bool,
    stop: Arc<AtomicBool>,
    wait_state: Arc<std::sync::Mutex<WaitState>>,
    /// async の追記待ち用 sleep。sync では使わない。
    delay: Option<Pin<Box<tokio::time::Sleep>>>,
}

#[cfg(target_os = "macos")]
impl FollowFdReader {
    fn new(
        inner: FdReader,
        follow: bool,
        stop: Arc<AtomicBool>,
        wait_state: Arc<std::sync::Mutex<WaitState>>,
    ) -> Self {
        Self {
            inner,
            follow,
            stop,
            wait_state,
            delay: None,
        }
    }

    /// follow を打ち切って EOF にしてよいか。
    fn should_stop_follow(&self) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return true;
        }
        self.wait_state
            .lock()
            .expect("wait state mutex must not be poisoned while following logs")
            .exit_code()
            .is_some()
    }
}

#[cfg(target_os = "macos")]
impl std::io::Read for FollowFdReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Read トレイトの契約: 空バッファでは即座に Ok(0) を返す。
        // これがないと follow + プロセス生存中に 100ms スリープの無限ループになる。
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            match self.inner.read_at(buf)? {
                0 if self.follow && !self.should_stop_follow() => {
                    // 呼び出しスレッドをブロックする。async 文脈からは使わないこと。
                    std::thread::sleep(Duration::from_millis(100));
                }
                n => return Ok(n),
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl tokio::io::AsyncRead for FollowFdReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        // AsyncRead の契約: 空バッファでは即座に Ready(Ok(())) を返す。
        // これがないと follow + プロセス生存中にスリープの無限ループになる。
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let dst = buf.initialize_unfilled();
            match this.inner.read_at(dst) {
                Ok(0) if this.follow && !this.should_stop_follow() => {
                    let delay = this.delay.get_or_insert_with(|| {
                        Box::pin(tokio::time::sleep(Duration::from_millis(100)))
                    });
                    match delay.as_mut().poll(cx) {
                        Poll::Ready(()) => {
                            this.delay = None;
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                Ok(n) => {
                    this.delay = None;
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Err(e) => {
                    this.delay = None;
                    return Poll::Ready(Err(e));
                }
            }
        }
    }
}

/// FD から独立オフセットのバッファ付きリーダーを作る。FD が無い / dup 失敗時は空リーダー。
#[cfg(target_os = "macos")]
fn fd_reader_or_empty(
    fd: Option<RawFd>,
    follow: bool,
    stop: Arc<AtomicBool>,
    wait_state: Arc<std::sync::Mutex<WaitState>>,
) -> Pin<Box<dyn AsyncBufRead + Send>> {
    match fd.and_then(FdReader::dup_from) {
        Some(reader) => Box::pin(tokio::io::BufReader::new(FollowFdReader::new(
            reader, follow, stop, wait_state,
        ))),
        None => Box::pin(tokio::io::BufReader::new(tokio::io::empty())),
    }
}

/// FD から独立オフセットの同期バッファ付きリーダーを作る。
///
/// FD が無い、または dup に失敗した場合は空リーダーを返す。
#[cfg(target_os = "macos")]
fn fd_reader_or_empty_sync(
    fd: Option<RawFd>,
    follow: bool,
    stop: Arc<AtomicBool>,
    wait_state: Arc<std::sync::Mutex<WaitState>>,
) -> Box<dyn std::io::BufRead + Send> {
    match fd.and_then(FdReader::dup_from) {
        Some(reader) => Box::new(std::io::BufReader::new(FollowFdReader::new(
            reader, follow, stop, wait_state,
        ))),
        None => Box::new(std::io::BufReader::new(std::io::empty())),
    }
}

/// LogConsumer へログ行を配信するタスクを起動する (macOS・FD ベース)。
///
/// 行長上限 (`line::MAX_LINE_LENGTH`) を超える行は先頭を切り捨てフレームとして配信し、
/// 残余を読み捨てる (改行を含まない巨大出力でも配信タスクのメモリが有界に保たれる)。
///
/// EOF は「ログの終端」ではなく「現時点の末尾」なので、停止指示 (`stop`) が来るまで
/// ポーリングで追記を読み続ける。以前は最初の EOF でタスクが終了してしまい、
/// それ以降のログが consumer に届かなかった。
///
/// コンテナが自然終了した場合も `stop` は立たないため、EOF 時に「stop フラグ OR
/// (exit code 記録を初めて観測してから `DRAIN_GRACE` 経過)」で終了判定する。
/// 終了直前に flush されるログを取りこぼさないための猶予である。
/// `stop` フラグは EOF 観測時に猶予を待たず即 break する (ログ配信中は EOF に達するまで
/// 停止しない。ドレインとして意図的)。macOS ではコンテナ終了後にログ追記が止まるため、
/// いずれ EOF に達して判定される。
///
/// exit code は `wait_blocking` の成功時のみ記録されるため、XPC 障害で記録が無い場合は
/// ポーリングが継続し得る (`FollowFdReader` と同じ制約。stop / rm / Drop 経路で解消される)。
#[cfg(target_os = "macos")]
fn spawn_log_consumer_task(
    fd: Option<RawFd>,
    stop: Arc<AtomicBool>,
    wait_state: Arc<std::sync::Mutex<WaitState>>,
    consumers: Arc<Vec<Box<dyn crate::core::logs::consumer::LogConsumer + 'static>>>,
    to_frame: fn(Vec<u8>) -> crate::core::logs::LogFrame,
) {
    let Some(fd) = fd else { return };
    let Some(reader) = FdReader::dup_from(fd) else {
        return;
    };
    tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(reader);
        // exit code を初めて観測した時刻。未観測の間は None。
        // 一度観測したら保持し続ける (リセットしない)。exit code が Some → None に戻るのは
        // 再 start 時の世代バンプのみで、その時点で旧コンテナは停止済みであり、旧タスクが
        // 読む dup FD は死んだファイルを指す。リセットすると refresh_log_streams 失敗時に
        // 旧タスクが新世代の exit 記録まで残り続けるため、アンカーは保持して必ず
        // 「観測 + 猶予」で終了させる。なお refresh 失敗時は start 側の巻き戻し
        // (stop_with_timeout) が stop_log_delivery 経由で stop フラグを立てるため、
        // タスクは EOF 観測時に即 break する (アンカー保持は保険の役割)。
        let mut exit_observed_at: Option<std::time::Instant> = None;
        loop {
            match deliver_line_to_consumers(&mut reader, consumers.as_ref(), to_frame).await {
                Ok(true) => {}
                Ok(false) => {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // コンテナ自然終了 (exit code 記録) を観測したら、猶予期間の経過で
                    // タスクと dup FD を解放する。猶予のアンカーは初回観測時点で固定し、
                    // ポーリングが長引いても猶予が伸びないようにする。
                    let exited = wait_state
                        .lock()
                        .expect(
                            "wait state mutex must not be poisoned while checking container exit",
                        )
                        .exit_code()
                        .is_some();
                    if exited {
                        let anchor = exit_observed_at.get_or_insert_with(std::time::Instant::now);
                        if anchor.elapsed() >= crate::core::wait::log_strategy::DRAIN_GRACE {
                            break;
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(()) => break,
            }
        }
    });
}

impl<I: Image + fmt::Debug> fmt::Debug for ContainerAsync<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContainerAsync")
            .field("id", &self.id)
            .field("image", &self.image)
            .field("dropped", &self.dropped)
            .finish()
    }
}

/// copy_out 先の一時パスを削除する。ファイルでもディレクトリでも消す。
///
/// macOS ではディレクトリへの `remove_file` が `EISDIR` ではなく `EPERM` になることが
/// あるため、ErrorKind では分岐せず失敗したら常に `remove_dir_all` を試す。
#[cfg(target_os = "macos")]
async fn remove_copy_out_temp(path: &std::path::Path) {
    if tokio::fs::remove_file(path).await.is_ok() {
        return;
    }
    if let Err(e) = tokio::fs::remove_dir_all(path).await {
        tracing::warn!(
            "failed to remove copy_file_from temp path {}: {e}",
            path.display()
        );
    }
}

/// Runtime 内 Drop で削除スレッドの完了を待つ上限時間。
/// コンテナ削除は Docker Desktop 負荷時や XPC 混雑時に 1 秒を超え得るため、
/// 既存のログタスク polling (最大 1 秒) より余裕を持たせた 5 秒を初期値とする。
const DROP_REMOVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Drop 時のコンテナ削除の契約。
///
/// # 完了保証
///
/// - Runtime 内 Drop (`Handle::try_current()` が `Ok`): 削除を専用 std スレッドで
///   `remove_blocking` として実行し、`DROP_REMOVE_TIMEOUT` (5 秒) を上限に完了を待つ。
///   timeout 内に完了すれば `drop` 復帰時点で削除は終わっている。超過時は best-effort
///   (削除スレッドは裏で走り続けるが、`drop` 直後にプロセスが終了すると中断され得る)。
///   待機には `mpsc::recv_timeout` を使うため、呼び出しスレッド (tokio ワーカー) を
///   最大 timeout 値ブロックする。multi-threaded Runtime では他のワーカーが肩代わり
///   するが、current-thread Runtime では Runtime 全体が停止する。テストライブラリの
///   Drop 場面 (テスト末尾) では実害が小さい
/// - Runtime 外 Drop: 呼び出しスレッドで `remove_blocking` を同期実行し、試行の
///   終了までは待つ。成功は保証しない
///
/// いずれの経路でも失敗は `tracing::error` にのみ記録され、呼び出し側には届かない。
/// 完了を待ちたい、または成否を `Result` で扱いたい場合は明示 `rm().await` を使うこと。
/// `TESTCONTAINERS_COMMAND=keep` のときは Drop は削除しない (明示 `rm` は `keep` でも
/// 削除する。ゲートは非対称)。
impl<I: Image> Drop for ContainerAsync<I> {
    fn drop(&mut self) {
        // 共通の前置き: ログ配信を停止する (macOS: 停止フラグ、Linux: shutdown)。
        self.stop_log_delivery();

        // macOS: containerLogs の FD はここで close する。
        // close する者がいないと、コンテナ 1 つにつき 2 fd がリークする。
        #[cfg(target_os = "macos")]
        {
            let mut source = self
                .log_source
                .lock()
                .expect("log source mutex must not be poisoned while dropping container");
            if let ContainerLogSource::Fd { stdout, stderr } = *source {
                unsafe { libc::close(stdout) };
                unsafe { libc::close(stderr) };
            }
            *source = ContainerLogSource::None;
        }

        // Linux: Runtime 外ならログタスク (demux / consumer) の完了を最大 1 秒 polling する。
        // Runtime 内では削除を DROP_REMOVE_TIMEOUT 付きで待つが、ログタスク完了は待たない。
        #[cfg(target_os = "linux")]
        {
            let handle = {
                let source = self
                    .log_source
                    .lock()
                    .expect("log source mutex must not be poisoned while dropping container");
                match &*source {
                    ContainerLogSource::DockerStream(handle) => Some(handle.clone()),
                    ContainerLogSource::None => None,
                }
            };
            if let Some(handle) = handle
                && tokio::runtime::Handle::try_current().is_err()
            {
                // 各周は「フラグ確認 → sleep」の順。既に完了済みなら 0 回 sleep で即抜ける。
                for _ in 0..20 {
                    if handle.all_done() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }

        if self.dropped {
            return;
        }
        if !matches!(
            crate::core::env::command(),
            crate::core::env::Command::Remove
        ) {
            return;
        }

        let id = self.id.clone();
        let client = self.client.clone();
        let remove = move || -> Result<()> {
            match &client {
                #[cfg(target_os = "macos")]
                Client::MacOs(_) => {
                    crate::core::client::xpc_client::XpcClient::remove_blocking(&id, true)
                }
                #[cfg(target_os = "linux")]
                Client::Linux(c) => c.remove_blocking(&id, true),
            }
        };

        match tokio::runtime::Handle::try_current() {
            Ok(_) => {
                // Runtime 内ではユーザー Runtime に依存しない専用スレッドで削除する。
                // async spawn だと Runtime 終了でタスクが破棄されコンテナが孤立し得る。
                // mpsc::channel + recv_timeout で完了を DROP_REMOVE_TIMEOUT まで待つ。
                // remove_blocking は Runtime 非依存のため deadlock にはならない。
                // recv_timeout は呼び出しスレッド (tokio ワーカー) をブロックするが、
                // テストライブラリの Drop 場面 (テスト末尾) では実害が小さい。
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = remove();
                    let _ = tx.send(result);
                });
                match rx.recv_timeout(DROP_REMOVE_TIMEOUT) {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::error!("failed to remove container on drop: {e}");
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        tracing::error!(
                            "timed out waiting for container removal on drop ({}s)",
                            DROP_REMOVE_TIMEOUT.as_secs()
                        );
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        tracing::error!("container removal thread disconnected unexpectedly");
                    }
                }
            }
            Err(_) => {
                // Runtime 外 (sync Container の drop や Runtime 破棄後)。
                // 呼び出しスレッドで同期実行し、試行終了まで待つ。契約詳細は Drop の rustdoc 参照。
                if let Err(e) = remove() {
                    tracing::error!("failed to remove container on drop: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WaitState;

    #[test]
    fn ready_conditions_require_log_detects_log_strategy() {
        // WaitFor::Log があれば true、無ければ false になること。
        // start 側 / exec 側で共用する共通述語の動作を確認する。
        use crate::core::wait::WaitFor;

        assert!(super::ready_conditions_require_log(&[
            WaitFor::message_on_stdout("ready")
        ]));
        assert!(super::ready_conditions_require_log(&[
            WaitFor::seconds(1),
            WaitFor::message_on_stderr("err"),
        ]));
        assert!(super::ready_conditions_require_log(&[
            WaitFor::message_on_either_std("either")
        ]));
        assert!(!super::ready_conditions_require_log(&[]));
        assert!(!super::ready_conditions_require_log(&[WaitFor::Nothing]));
        assert!(!super::ready_conditions_require_log(&[WaitFor::seconds(1)]));
        assert!(!super::ready_conditions_require_log(&[
            WaitFor::healthcheck()
        ]));
    }

    /// `ContainerLogSource::None` で構築した `ContainerAsync` に対して、
    /// `WaitFor::Log` を含む ready_conditions 付き exec が明示エラーを返すこと。
    ///
    /// ログ取得元が無いコンテナに `WaitFor::Log` を待機させると、空リーダー +
    /// `exit_code_hint` 未観測のままポーリングが永久に回るため、コンテナ内コマンドの
    /// 実行前に明示エラーで打ち切る。`Client::detect` は接続しないため、exec 経路は
    /// XPC / Docker を呼ばずに検証できる (テスト終了時の Drop はコンテナ削除を試みるが、
    /// エラーは無視されテスト結果には影響しない)。
    #[tokio::test]
    async fn exec_log_wait_without_log_source_returns_error() {
        use crate::core::client::Client;
        use crate::core::containers::async_container::{ContainerAsync, ContainerLogSource};
        use crate::core::image::ExecCommand;
        use crate::core::image::image_ext::ImageExt;
        use crate::core::wait::WaitFor;
        use crate::images::GenericImage;

        let req: crate::ContainerRequest<GenericImage> =
            GenericImage::new("alpine", "latest").with_cmd(["sleep", "30"]);
        let wait_state = crate::core::containers::async_container::new_wait_state();
        let client = Client::detect().expect("クライアント生成に失敗した");
        #[cfg(target_os = "linux")]
        let container: ContainerAsync<GenericImage> = ContainerAsync::new(
            "test-id".to_string(),
            client,
            req,
            wait_state,
            ContainerLogSource::None,
            None,
        );
        #[cfg(target_os = "macos")]
        let container: ContainerAsync<GenericImage> = ContainerAsync::new(
            "test-id".to_string(),
            client,
            req,
            wait_state,
            ContainerLogSource::None,
        );

        let err = container
            .exec(
                ExecCommand::new(["echo", "hello"])
                    .with_container_ready_conditions(vec![WaitFor::message_on_stdout("ready")]),
            )
            .await
            .expect_err("ログ取得元なし + Log 待機は明示エラーになること");
        assert!(
            err.to_string().contains("log wait requires a log source"),
            "ログ取得元欠如の明示エラーであること: {err}"
        );
    }

    #[test]
    fn store_if_current_accepts_matching_generation() {
        // 一致する世代の書き込みは受理されること。
        let mut state = WaitState::default();
        assert_eq!(state.generation(), 0);
        assert!(state.store_if_current(0, 42));
        assert_eq!(state.exit_code(), Some(42));
    }

    #[test]
    fn store_if_current_rejects_stale_generation() {
        // bump 後の旧世代書き込みは破棄されること。
        let mut state = WaitState::default();
        assert!(state.store_if_current(0, 1));
        let new_gen = state.bump();
        assert_eq!(new_gen, 1);
        assert_eq!(state.exit_code(), None);
        assert!(!state.store_if_current(0, 99));
        assert_eq!(state.exit_code(), None);
        assert!(state.store_if_current(1, 7));
        assert_eq!(state.exit_code(), Some(7));
    }

    /// 中身ありの一時ディレクトリを `remove_copy_out_temp` が削除できること。
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn remove_copy_out_temp_removes_nonempty_directory() {
        let dir = std::env::temp_dir().join(format!(
            "container-rs-copy-out-helper-test-{}",
            crate::core::util::unique_suffix()
        ));
        tokio::fs::create_dir(&dir)
            .await
            .expect("一時ディレクトリの作成に失敗した");
        tokio::fs::write(dir.join("child.txt"), b"x")
            .await
            .expect("子ファイルの作成に失敗した");
        assert!(
            tokio::fs::metadata(&dir)
                .await
                .expect("一時ディレクトリのメタデータ取得に失敗した")
                .is_dir(),
            "作成直後はディレクトリであること"
        );

        super::remove_copy_out_temp(&dir).await;

        assert!(
            tokio::fs::metadata(&dir).await.is_err(),
            "ヘルパー呼び出し後にパスが存在しないこと"
        );
    }

    /// FollowFdReader の同期 Read が空バッファで即座に Ok(0) を返すこと。
    /// 修正前は follow + プロセス生存中に 100ms スリープの無限ループになっていた。
    #[cfg(target_os = "macos")]
    #[test]
    fn follow_fd_reader_sync_empty_buffer_returns_zero() {
        use std::io::Read;
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        // 一時ファイルの FD で FdReader を作る。
        let dir = std::env::temp_dir().join(format!(
            "container-rs-follow-fd-test-{}",
            crate::core::util::unique_suffix()
        ));
        std::fs::create_dir(&dir).expect("一時ディレクトリの作成に失敗した");
        let path = dir.join("test.log");
        std::fs::write(&path, b"hello").expect("ファイルの書き込みに失敗した");
        let file = std::fs::File::open(&path).expect("ファイルを開けること");
        use std::os::fd::AsRawFd;
        let reader = super::FdReader::dup_from(file.as_raw_fd()).expect("dup に成功すること");
        let stop = Arc::new(AtomicBool::new(false));
        let wait_state = Arc::new(std::sync::Mutex::new(WaitState::default()));
        let mut follow_reader = super::FollowFdReader::new(reader, true, stop, wait_state);

        // 空バッファで即座に Ok(0) が返ること (ハングしないこと)。
        // ハング防止のため別スレッドで実行し、タイムアウトで検出する。
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut empty: [u8; 0] = [];
            let result = follow_reader.read(&mut empty);
            let _ = tx.send(result);
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("空バッファの read が 2 秒以内に返ること (無限ループしていないこと)");
        assert_eq!(
            result.expect("read が成功すること"),
            0,
            "空バッファでは Ok(0) であること"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// FollowFdReader の非同期 AsyncRead が空バッファで即座に Ready(Ok(())) を返すこと。
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn follow_fd_reader_async_empty_buffer_returns_ready() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use tokio::io::AsyncRead;

        // 一時ファイルの FD で FdReader を作る。
        let dir = std::env::temp_dir().join(format!(
            "container-rs-follow-fd-async-test-{}",
            crate::core::util::unique_suffix()
        ));
        tokio::fs::create_dir(&dir)
            .await
            .expect("一時ディレクトリの作成に失敗した");
        let path = dir.join("test.log");
        tokio::fs::write(&path, b"hello")
            .await
            .expect("ファイルの書き込みに失敗した");
        let file = tokio::fs::File::open(&path)
            .await
            .expect("ファイルを開けること");
        use std::os::fd::AsRawFd;
        let reader = super::FdReader::dup_from(file.as_raw_fd()).expect("dup に成功すること");
        let stop = Arc::new(AtomicBool::new(false));
        let wait_state = Arc::new(std::sync::Mutex::new(WaitState::default()));
        let mut follow_reader = super::FollowFdReader::new(reader, true, stop, wait_state);

        // 空の ReadBuf で即座に Ready(Ok(())) が返ること。
        // ハング防止のため tokio::time::timeout で保護する。
        let mut buf = tokio::io::ReadBuf::new(&mut []);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            std::future::poll_fn(|cx| {
                std::pin::Pin::new(&mut follow_reader).poll_read(cx, &mut buf)
            }),
        )
        .await
        .expect("空バッファの poll_read が 2 秒以内に返ること (無限ループしていないこと)");
        assert!(result.is_ok(), "空バッファの poll_read が成功すること");
        assert_eq!(
            buf.filled().len(),
            0,
            "空バッファでは読み込みバイト数 0 であること"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
