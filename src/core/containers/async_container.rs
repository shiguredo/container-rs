//! `ContainerAsync` — 実行中コンテナのハンドル。
//! 元の 0.27 の `core::containers::async_container` と同一シグネチャ。
//!
//! macOS (XPC) では内部で `XpcClient` を使い、Apple Container の XPC API を叩く。

pub mod exec;

use std::{
    fmt,
    future::Future,
    net::IpAddr,
    os::fd::{FromRawFd, RawFd},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, ReadBuf};

use crate::core::client::Client;
use crate::core::host::Host;
use crate::{
    ContainerRequest, Image,
    core::{
        WaitFor,
        copy::CopyFileFromContainer,
        error::{Error, Result},
        image::{ContainerState, ExecCommand},
        ports::{ContainerPort, Ports},
    },
};

#[cfg(target_os = "macos")]
use crate::core::copy::CopyFromContainerError;
use crate::core::error::ExecError;

/// 実行中コンテナ。Drop で削除される。
pub struct ContainerAsync<I: Image> {
    id: String,
    image: ContainerRequest<I>,
    client: Client,
    dropped: bool,
    /// macOS (XPC) で init プロセスの exit code を保持する。
    /// `containerWait` を別スレッドで待ち、終了時に値が入る。
    /// 世代番号で再 start 後の旧スレッド書き込みを破棄する。
    wait_state: Arc<std::sync::Mutex<WaitState>>,
    /// macOS (XPC) で `containerLogs` から取得した stdout / stderr の FD。
    /// 再 start 時に差し替えるため Mutex で保持し、Drop で close する。
    stdout_fd: std::sync::Mutex<Option<RawFd>>,
    stderr_fd: std::sync::Mutex<Option<RawFd>>,
    /// LogConsumer 配信タスクへの停止指示。再 start 時に新旧を差し替える。
    log_stop: std::sync::Mutex<Arc<AtomicBool>>,
    /// LogConsumer 再 spawn 用に保持する。未登録なら None。
    /// Linux では再 start 経路が未実装のため保持しない (初期 spawn の Arc はタスク側が持つ)。
    #[cfg(target_os = "macos")]
    log_consumers: Option<Arc<Vec<Box<dyn crate::core::logs::consumer::LogConsumer + 'static>>>>,
}

/// `containerWait` スレッドと共有する終了コード状態。
#[derive(Debug, Default)]
pub(crate) struct WaitState {
    /// 再 start のたびに進む世代。旧スレッドの書き込み判定に使う。
    /// Linux では再 start / wait スレッドが未接続のため lib 本体からは未使用 (単体テストでは使う)。
    #[cfg_attr(all(not(target_os = "macos"), not(test)), expect(dead_code))]
    generation: u64,
    exit_code: Option<i64>,
}

impl WaitState {
    /// 世代が一致する場合のみ exit_code を記録する。
    /// 一致して書き込んだら true、旧世代で破棄したら false。
    #[cfg_attr(all(not(target_os = "macos"), not(test)), expect(dead_code))]
    pub(crate) fn store_if_current(&mut self, generation: u64, code: i64) -> bool {
        if self.generation == generation {
            self.exit_code = Some(code);
            true
        } else {
            false
        }
    }

    /// 世代を進め、exit_code をクリアする。新しい世代番号を返す。
    #[cfg_attr(all(not(target_os = "macos"), not(test)), expect(dead_code))]
    pub(crate) fn bump(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.exit_code = None;
        self.generation
    }

    pub(crate) fn exit_code(&self) -> Option<i64> {
        self.exit_code
    }

    #[cfg_attr(all(not(target_os = "macos"), not(test)), expect(dead_code))]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

/// 空の `WaitState` を共有用に作る。
pub(crate) fn new_wait_state() -> Arc<std::sync::Mutex<WaitState>> {
    Arc::new(std::sync::Mutex::new(WaitState::default()))
}

/// init プロセスの exit code を待つ std スレッドを起動する。
///
/// spawn 時点の世代を保持し、書き込み時に世代が一致する場合のみ記録する。
#[cfg(target_os = "macos")]
pub(crate) fn spawn_exit_code_waiter(
    id: String,
    wait_state: Arc<std::sync::Mutex<WaitState>>,
    generation: u64,
) {
    std::thread::spawn(move || {
        if let Ok(code) = crate::core::client::xpc_client::XpcClient::wait_blocking(&id, &id) {
            let mut guard = wait_state
                .lock()
                .expect("wait state mutex must not be poisoned while recording exit code");
            let _ = guard.store_if_current(generation, code);
        }
    });
}

impl<I: Image> ContainerAsync<I> {
    /// `AsyncRunner::start` から呼ばれる構築子。
    pub(crate) fn new(
        id: String,
        client: Client,
        mut image: ContainerRequest<I>,
        wait_state: Arc<std::sync::Mutex<WaitState>>,
        stdout_fd: Option<RawFd>,
        stderr_fd: Option<RawFd>,
    ) -> Self {
        let log_consumers = std::mem::take(&mut image.log_consumers);
        let has_consumers = !log_consumers.is_empty();
        let log_stop = Arc::new(AtomicBool::new(false));
        // consumer には独立オフセットのリーダーを渡す。
        // 以前は FD 自体を consumer に奪わせていたため、`stdout()` / ログ待機戦略が
        // 空リーダーになり、`with_log_consumer` + `message_on_stdout` の併用が必ず失敗していた。
        #[cfg(target_os = "macos")]
        let log_consumers = if has_consumers {
            let consumers = Arc::new(log_consumers);
            spawn_log_consumer_task(
                stdout_fd,
                log_stop.clone(),
                consumers.clone(),
                crate::core::logs::LogFrame::StdOut,
            );
            spawn_log_consumer_task(
                stderr_fd,
                log_stop.clone(),
                consumers.clone(),
                crate::core::logs::LogFrame::StdErr,
            );
            // 再 start 時の再 spawn 用に保持する。
            Some(consumers)
        } else {
            None
        };
        #[cfg(target_os = "linux")]
        if has_consumers {
            let consumers = Arc::new(log_consumers);
            spawn_log_consumer_task(
                stdout_fd,
                log_stop.clone(),
                consumers.clone(),
                crate::core::logs::LogFrame::StdOut,
            );
            spawn_log_consumer_task(
                stderr_fd,
                log_stop.clone(),
                consumers,
                crate::core::logs::LogFrame::StdErr,
            );
        }

        Self {
            id,
            image,
            client,
            dropped: false,
            wait_state,
            stdout_fd: std::sync::Mutex::new(stdout_fd),
            stderr_fd: std::sync::Mutex::new(stderr_fd),
            log_stop: std::sync::Mutex::new(log_stop),
            #[cfg(target_os = "macos")]
            log_consumers,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn image(&self) -> &I {
        self.image.image()
    }

    pub async fn ports(&self) -> Result<Ports> {
        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => c.ports(&self.id).await,
            #[cfg(target_os = "linux")]
            Client::Linux(c) => c.ports(&self.id).await,
        }
    }

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

    pub async fn get_bridge_ip_address(&self) -> Result<IpAddr> {
        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => c.bridge_ip_address(&self.id).await,
            #[cfg(target_os = "linux")]
            Client::Linux(_) => Err(Error::other(
                "get_bridge_ip_address is not supported on Linux",
            )),
        }
    }

    /// コンテナからホストへファイルをコピーする。
    ///
    /// apple/container の `containerCopyOut` はディレクトリもコピーできるが、
    /// 本 API はファイル専用のため、ディレクトリなら `IsDirectory` で拒否する。
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
            Client::Linux(_) => {
                let _ = (source.into(), target);
                Err(Error::other("copy_file_from() is not implemented on Linux"))
            }
        }
    }

    pub async fn get_host(&self) -> Result<Host> {
        // macOS / Linux ともホストは localhost。
        Ok(Host::parse("localhost"))
    }

    /// コンテナ内でコマンドを実行する。
    ///
    /// `ExecCommand::container_ready_conditions` には start 時の `startup_timeout` が
    /// 適用されない。http / log 待機などを置くと無期限に待ち得る。
    ///
    /// # Linux
    ///
    /// Docker Engine API の exec は Env を送らず、stdout / stderr も取得しない。
    /// `CmdWaitFor::StdOutMessage` / `StdErrMessage` は明示エラーになる。
    pub async fn exec(&self, cmd: ExecCommand) -> Result<exec::ExecResult> {
        let ExecCommand {
            cmd,
            container_ready_conditions,
            cmd_ready_condition,
            env_vars: _env_vars,
        } = cmd;

        let cmd_owned: Vec<String> = cmd;
        let raw = match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => {
                // XPC は environment が空だとコンテナ env を継承しないため、
                // ContainerRequest の env を基底にし、ExecCommand の env で上書きする。
                let mut merged: std::collections::BTreeMap<String, String> = self
                    .image
                    .env_vars()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                for (k, v) in _env_vars {
                    merged.insert(k, v);
                }
                let environment: Vec<String> = merged
                    .into_iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                c.exec(&self.id, &cmd_owned, environment).await?
            }
            #[cfg(target_os = "linux")]
            Client::Linux(c) => {
                // Docker は Env 省略時にコンテナ env を継承する。stdout / stderr は取得しない。
                // with_env_vars の Linux 本対応は未実装のため、非空なら明示エラーにする。
                if !_env_vars.is_empty() {
                    return Err(Error::other(
                        "ExecCommand::with_env_vars is not supported on Linux",
                    ));
                }
                c.exec(&self.id, &cmd_owned).await?
            }
        };

        // container_ready_conditions を待機
        self.block_until_ready(container_ready_conditions).await?;

        // cmd_ready_condition の処理。
        match cmd_ready_condition {
            crate::core::CmdWaitFor::StdOutMessage { message } => {
                #[cfg(target_os = "macos")]
                {
                    // XPC の exec はコマンド完走後に stdout / stderr 全体を取得済みなので、
                    // メッセージ待ちは取得済みバッファに対する部分一致で判定する。
                    if !contains_bytes(&raw.stdout, &message) {
                        return Err(crate::core::error::Error::other(format!(
                            "expected message not found in stdout: {}",
                            String::from_utf8_lossy(&message)
                        )));
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    let _ = message;
                    return Err(crate::core::error::Error::other(
                        "CmdWaitFor::StdOutMessage is not supported on Linux (stdout is not captured)",
                    ));
                }
            }
            crate::core::CmdWaitFor::StdErrMessage { message } => {
                #[cfg(target_os = "macos")]
                {
                    if !contains_bytes(&raw.stderr, &message) {
                        return Err(crate::core::error::Error::other(format!(
                            "expected message not found in stderr: {}",
                            String::from_utf8_lossy(&message)
                        )));
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    let _ = message;
                    return Err(crate::core::error::Error::other(
                        "CmdWaitFor::StdErrMessage is not supported on Linux (stderr is not captured)",
                    ));
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
            self.refresh_log_fds(c).await?;
        }
        #[cfg(target_os = "linux")]
        if let Client::Linux(c) = &self.client
            && !c.container_state(&self.id).await?.running
        {
            c.start_container(&self.id).await?;
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

    /// 再 start 後にログ FD を再取得し、旧 FD を close、LogConsumer を再 spawn する。
    ///
    /// `logs()` が成功するまで旧 `log_stop` / FD / consumer は維持する。
    /// 失敗時は部分更新せず `Err` を返す (呼び出し側の `start` が失敗する)。
    #[cfg(target_os = "macos")]
    async fn refresh_log_fds(
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
            let mut stdout = self
                .stdout_fd
                .lock()
                .expect("stdout file descriptor mutex must not be poisoned while refreshing logs");
            if let Some(old) = stdout.replace(new_out) {
                unsafe { libc::close(old) };
            }
        }
        {
            let mut stderr = self
                .stderr_fd
                .lock()
                .expect("stderr file descriptor mutex must not be poisoned while refreshing logs");
            if let Some(old) = stderr.replace(new_err) {
                unsafe { libc::close(old) };
            }
        }

        if let Some(consumers) = &self.log_consumers {
            let out = *self.stdout_fd.lock().expect(
                "stdout file descriptor mutex must not be poisoned while restarting consumers",
            );
            let err = *self.stderr_fd.lock().expect(
                "stderr file descriptor mutex must not be poisoned while restarting consumers",
            );
            spawn_log_consumer_task(
                out,
                new_stop.clone(),
                consumers.clone(),
                crate::core::logs::LogFrame::StdOut,
            );
            spawn_log_consumer_task(
                err,
                new_stop,
                consumers.clone(),
                crate::core::logs::LogFrame::StdErr,
            );
        }

        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        self.stop_with_timeout(None).await
    }

    pub async fn stop_with_timeout(&self, timeout_seconds: Option<i32>) -> Result<()> {
        // LogConsumer 配信を止める (Drop を待たない)。
        self.log_stop
            .lock()
            .expect("log stop mutex must not be poisoned while stopping container")
            .store(true, Ordering::Relaxed);

        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => c.stop(&self.id, timeout_seconds).await,
            #[cfg(target_os = "linux")]
            Client::Linux(c) => c.stop(&self.id, timeout_seconds).await,
        }
    }

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

    /// バックグラウンドの `containerWait` が観測した exit code を返す (未終了なら `None`)。
    ///
    /// # macOS
    ///
    /// 取得経路はバックグラウンド wait がキャッシュした値だけである。
    /// バックグラウンド wait が未完了、または wait 自体が失敗した場合は、コンテナが
    /// 停止済みでも `Ok(None)` を返し得る。`container_state` からの再取得経路は無い。
    ///
    /// # Linux
    ///
    /// 未実装のためエラーを返す。
    pub async fn exit_code(&self) -> Result<Option<i64>> {
        match &self.client {
            #[cfg(target_os = "macos")]
            Client::MacOs(c) => {
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
                if c.container_state(&self.id).await?.running {
                    Ok(None)
                } else {
                    // 停止済みだがまだバックグラウンド wait が完了していない。
                    // ここで containerWait を呼ぶと runtime client が解放済みの場合に
                    // ブロック・失敗するため、取得できない場合は None を返す。
                    Ok(None)
                }
            }
            #[cfg(target_os = "linux")]
            Client::Linux(_) => Err(Error::other("exit_code() is not implemented on Linux")),
        }
    }

    #[cfg(target_os = "macos")]
    pub async fn rm(mut self) -> Result<()> {
        match &self.client {
            Client::MacOs(c) => c.remove(&self.id, true).await?,
        }
        self.dropped = true;
        Ok(())
    }

    /// コンテナを削除する。
    #[cfg(target_os = "linux")]
    pub async fn rm(mut self) -> Result<()> {
        match &self.client {
            Client::Linux(c) => c.remove(&self.id, true).await?,
        }
        self.dropped = true;
        Ok(())
    }

    /// stdout のリーダーを返す。macOS (XPC) では `containerLogs` から取得した FD を読む。
    ///
    /// リーダーは独立した読み取り位置を持ち、常にログ先頭から読む。
    /// `follow = true` のときは末尾到達後も追記をポーリングする。init プロセス終了または
    /// コンテナ Drop / ログ FD 差し替えで EOF する。
    pub fn stdout(&self, follow: bool) -> Pin<Box<dyn AsyncBufRead + Send>> {
        fd_reader_or_empty(
            *self
                .stdout_fd
                .lock()
                .expect("stdout file descriptor mutex must not be poisoned while reading logs"),
            follow,
            self.log_stop_flag(),
            self.wait_state.clone(),
        )
    }

    /// stderr のリーダーを返す。macOS (XPC) では `containerLogs` から取得した FD を読む。
    ///
    /// リーダーは独立した読み取り位置を持ち、常にログ先頭から読む。
    /// `follow = true` のときは末尾到達後も追記をポーリングする。init プロセス終了または
    /// コンテナ Drop / ログ FD 差し替えで EOF する。
    ///
    /// 注意: Apple container の `containerLogs` が返す 2 本目の FD は VM の bootlog であり、
    /// アプリケーションの stderr は stdout 側のログに混流する。
    pub fn stderr(&self, follow: bool) -> Pin<Box<dyn AsyncBufRead + Send>> {
        fd_reader_or_empty(
            *self
                .stderr_fd
                .lock()
                .expect("stderr file descriptor mutex must not be poisoned while reading logs"),
            follow,
            self.log_stop_flag(),
            self.wait_state.clone(),
        )
    }

    /// stdout の同期リーダーを返す。
    ///
    /// リーダーは独立した読み取り位置を持ち、常にログ先頭から読む。
    /// `follow = true` のときは末尾到達後も追記をポーリングする。呼び出しスレッドをブロックするため、
    /// tokio ランタイムワーカー上や LogConsumer コールバックからは呼ばないこと。
    pub(crate) fn stdout_sync(&self, follow: bool) -> Box<dyn std::io::BufRead + Send> {
        fd_reader_or_empty_sync(
            *self
                .stdout_fd
                .lock()
                .expect("stdout file descriptor mutex must not be poisoned while reading logs"),
            follow,
            self.log_stop_flag(),
            self.wait_state.clone(),
        )
    }

    /// stderr の同期リーダーを返す。
    ///
    /// リーダーは独立した読み取り位置を持ち、常にログ先頭から読む。
    /// `follow = true` のときは末尾到達後も追記をポーリングする。呼び出しスレッドをブロックするため、
    /// tokio ランタイムワーカー上や LogConsumer コールバックからは呼ばないこと。
    ///
    /// 注意: Apple container の `containerLogs` が返す 2 本目の FD は VM の bootlog であり、
    /// アプリケーションの stderr は stdout 側のログに混流する。
    pub(crate) fn stderr_sync(&self, follow: bool) -> Box<dyn std::io::BufRead + Send> {
        fd_reader_or_empty_sync(
            *self
                .stderr_fd
                .lock()
                .expect("stderr file descriptor mutex must not be poisoned while reading logs"),
            follow,
            self.log_stop_flag(),
            self.wait_state.clone(),
        )
    }

    /// 現在の LogConsumer / follow 停止フラグを返す。
    fn log_stop_flag(&self) -> Arc<AtomicBool> {
        self.log_stop
            .lock()
            .expect("log stop mutex must not be poisoned while creating log readers")
            .clone()
    }

    pub async fn stdout_to_vec(&self) -> Result<Vec<u8>> {
        let mut stdout = Vec::new();
        self.stdout(false).read_to_end(&mut stdout).await?;
        Ok(stdout)
    }

    pub async fn stderr_to_vec(&self) -> Result<Vec<u8>> {
        let mut stderr = Vec::new();
        self.stderr(false).read_to_end(&mut stderr).await?;
        Ok(stderr)
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
#[cfg(target_os = "macos")]
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// pread ベースの独立オフセットリーダー。
///
/// `dup(2)` はオープンファイル記述 (ファイルオフセット) を共有するため、複数のリーダーが
/// 互いの読み取り位置を進めてしまい「後から読んだ側にログの前半が見えない」誤動作になる。
/// `read_at` (pread) は共有オフセットを使わず動かさないので、各リーダーが独立に読める。
/// `containerLogs` の FD は通常ファイルなので pread が使える。
struct FdReader {
    file: std::fs::File,
    pos: u64,
}

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

impl std::io::Read for FdReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.read_at(buf)
    }
}

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
struct FollowFdReader {
    inner: FdReader,
    follow: bool,
    stop: Arc<AtomicBool>,
    wait_state: Arc<std::sync::Mutex<WaitState>>,
    /// async の追記待ち用 sleep。sync では使わない。
    delay: Option<Pin<Box<tokio::time::Sleep>>>,
}

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

impl std::io::Read for FollowFdReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
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

impl tokio::io::AsyncRead for FollowFdReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
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

/// LogConsumer へログ行を配信するタスクを起動する。
///
/// EOF は「ログの終端」ではなく「現時点の末尾」なので、停止指示 (`stop`) が来るまで
/// ポーリングで追記を読み続ける。以前は最初の EOF でタスクが終了してしまい、
/// それ以降のログが consumer に届かなかった。
fn spawn_log_consumer_task(
    fd: Option<RawFd>,
    stop: Arc<AtomicBool>,
    consumers: Arc<Vec<Box<dyn crate::core::logs::consumer::LogConsumer + 'static>>>,
    to_frame: fn(Vec<u8>) -> crate::core::logs::LogFrame,
) {
    let Some(fd) = fd else { return };
    let Some(reader) = FdReader::dup_from(fd) else {
        return;
    };
    tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) => {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Ok(_) => {
                    if buf.last() == Some(&b'\n') {
                        buf.pop();
                    }
                    if buf.last() == Some(&b'\r') {
                        buf.pop();
                    }
                    let frame = to_frame(std::mem::take(&mut buf));
                    for consumer in consumers.as_ref() {
                        consumer.accept(&frame).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("log consumer read failed; stopping delivery: {e}");
                    break;
                }
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

impl<I: Image> Drop for ContainerAsync<I> {
    fn drop(&mut self) {
        // LogConsumer 配信タスクへ停止を通知する。
        self.log_stop
            .lock()
            .expect("log stop mutex must not be poisoned while dropping container")
            .store(true, Ordering::Relaxed);

        // containerLogs の FD はここで close する。
        // close する者がいないと、コンテナ 1 つにつき 2 fd がリークする。
        if let Some(fd) = self
            .stdout_fd
            .lock()
            .expect("stdout file descriptor mutex must not be poisoned while dropping container")
            .take()
        {
            unsafe { libc::close(fd) };
        }
        if let Some(fd) = self
            .stderr_fd
            .lock()
            .expect("stderr file descriptor mutex must not be poisoned while dropping container")
            .take()
        {
            unsafe { libc::close(fd) };
        }

        if self.dropped {
            return;
        }
        if !matches!(
            crate::core::env::Config.command(),
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
                // ランタイム内ではユーザー Runtime に依存しない専用スレッドで削除する。
                // async spawn だと Runtime 終了でタスクが破棄されコンテナが孤立し得る。
                // join しない (async Drop からの join は deadlock し得る)。
                std::thread::spawn(move || {
                    if let Err(e) = remove() {
                        tracing::error!("failed to remove container on drop: {e}");
                    }
                });
            }
            Err(_) => {
                // ランタイム外 (sync Container の drop や Runtime 破棄後)。
                // 呼び出し復帰までに削除完了を保証するため同期的に実行する。
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
}
