//! `XpcClient` — Apple Container の XPC API を叩くクライアント。
//!
//! 本家 testcontainers-rs の `Client`（Docker Engine API ラッパ）に相当する。
//! `ContainerAsync` / `AsyncRunner` / `WaitStrategy` から利用される。
//!
//! macOS バックエンドが必要とする XPC 操作を実装する。
//! `pause` / `unpause` に相当する XPC 操作は未実装。

use std::collections::HashMap;
use std::io::Read;
use std::net::IpAddr;
use std::os::fd::FromRawFd;
use std::time::Duration;

use nojson::DisplayJson;

use crate::core::{
    client::ContainerSnapshot,
    error::{ClientError, Result},
    ports::{ContainerPort, Ports},
};
use crate::xpc::{self, IMAGE_SERVICE, KeyValue, SERVICE_NAME, XpcConn, id_key, j, k, s};

/// XPC クライアント。毎回 `connect()` で新規接続を張る（本家の `Client` と違い接続プール無し）。
#[derive(Clone)]
pub(crate) struct XpcClient;

/// exec の `containerWait` 成功後、読み取りスレッドから mpsc 経由で結果が届くのを
/// 待つ上限時間。
///
/// この時点でプロセスは終了済み (containerWait が成功で返っているため)。
/// 上限は「プロセス終了後にデーモンが write FD を閉じるまでの猶予」であり、
/// exec 自体の実行時間を制限するものではない (この上限が課されるのは containerWait
/// 応答受信後のみ)。大出力の読み取りは containerWait と並行に進むため、この上限で
/// 待つのはデーモンの FD クローズ遅延のみ、という分析に基づく固定値。
const EXEC_READER_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// 停止処理の XPC 送信タイムアウトを求める。
///
/// apiserver は停止処理の完了 (グレース経過 + SIGKILL) を待って reply を返すため、
/// グレース秒 + 余裕 30 秒を XPC 送信タイムアウトにする。`DEFAULT_TIMEOUT` (60 秒) を
/// 下回らず、`LONG_TIMEOUT` (24 時間) で飽和する (u64 の飽和計算)。
/// 負値のグレース (`i32::MAX` 秒) は 24 時間で飽和し、`XpcTimeout` が返り得る。
fn xpc_timeout_for_grace(grace_seconds: u64) -> Duration {
    crate::xpc::DEFAULT_TIMEOUT
        .max(Duration::from_secs(grace_seconds.saturating_add(30)).min(crate::xpc::LONG_TIMEOUT))
}

/// ホスト側パスを絶対パス化する。
///
/// copy_in / copy_out のホストパスは XPC 経由で apiserver に渡るが、apiserver は
/// 呼び出し側と別プロセス (gui/501 の launchd エージェント) で cwd も異なる。
/// 相対パスをそのまま渡すと apiserver の cwd 基準で解決されてしまうため、
/// 呼び出し側の cwd 基準で絶対パスへ変換してから渡す。
fn absolutize_host_path(path: &std::path::Path) -> Result<std::path::PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir()
        .map_err(|e| ClientError::Other(format!("failed to resolve current directory: {e}")))?;
    Ok(cwd.join(path))
}

/// ホスト側パスを XPC 文字列用の UTF-8 `&str` に変換する。
///
/// 非 UTF-8 パスを `to_string_lossy` で黙って置換すると、別パスとして送信されるため拒否する。
fn host_path_for_xpc(path: &std::path::Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        ClientError::Other(format!("host path is not valid UTF-8: {}", path.display())).into()
    })
}

impl XpcClient {
    /// コンテナ内のプロセスが終了するのを待ち、その exit code を返す (同期)。
    ///
    /// `AsyncRunner::start` の常駐 exit code 監視はランタイム非管理の std スレッドから
    /// これを呼ぶ。`spawn_blocking` 経由だと、コンテナ実行中にランタイムを drop した際に
    /// tokio が LONG_TIMEOUT (24 時間) の containerWait を join しようとしてハングする。
    pub(crate) fn wait_blocking(id: &str, process_id: &str) -> Result<i64> {
        let conn = XpcConn::connect(SERVICE_NAME)?;
        let reply = conn.send_with_timeout(
            "containerWait",
            &[(id_key(), s(id)), (k("processIdentifier"), s(process_id))],
            crate::xpc::LONG_TIMEOUT,
        )?;
        reply.try_int64(&k("exitCode"))
    }

    /// タイムアウト付きでコンテナの exit code を待つ。
    ///
    /// 停止済みコンテナへの都度取得用。通常は即座に返る。
    pub(crate) fn wait_blocking_with_timeout(
        id: &str,
        process_id: &str,
        timeout: Duration,
    ) -> Result<i64> {
        let conn = XpcConn::connect(SERVICE_NAME)?;
        let reply = conn.send_with_timeout(
            "containerWait",
            &[(id_key(), s(id)), (k("processIdentifier"), s(process_id))],
            timeout,
        )?;
        reply.try_int64(&k("exitCode"))
    }

    /// コンテナを停止する。
    ///
    /// `timeout_seconds`:
    /// - `None` → SIGTERM、30 秒タイムアウト（既存 `stop()` の挙動を維持）。
    /// - `Some(0)` → 即時 SIGKILL。
    /// - `Some(t)` (`t < 0`) → 無限待ちに近い長いタイムアウトで SIGTERM。
    ///   Apple container は負のタイムアウトをサポートしないため、`i32::MAX` 秒で代用。
    /// - `Some(t)` (`t > 0`) → SIGTERM、`t` 秒タイムアウト。
    ///
    /// XPC 呼び出し自体のタイムアウトは「グレース + 余裕 30 秒」で、`DEFAULT_TIMEOUT`
    /// (60 秒) を下回らず `LONG_TIMEOUT` (24 時間) で飽和する。グレース + 30 秒が
    /// `LONG_TIMEOUT` を超える指定は 24 時間後に `XpcTimeout` が返り得る
    /// (実用上の上限として許容)。呼び出し中にランタイムを drop すると、tokio が
    /// 最大 24 時間の XPC 待ちを join しようとしてハングし得るため注意する。
    pub(crate) async fn stop(&self, id: &str, timeout_seconds: Option<i32>) -> Result<()> {
        let (signal, timeout) = match timeout_seconds {
            Some(0) => ("SIGKILL".to_string(), 0),
            Some(t) if t < 0 => ("SIGTERM".to_string(), i32::MAX as u64),
            Some(t) => ("SIGTERM".to_string(), t as u64),
            None => ("SIGTERM".to_string(), 30),
        };
        // XPC 送信タイムアウト: グレース + 余裕 30 秒 (u64 で飽和計算)。
        // apiserver は停止処理の完了を待って reply を返すため、グレースが
        // DEFAULT_TIMEOUT (60 秒) を超える指定では 60 秒では足りず誤タイムアウトになる。
        let xpc_timeout = xpc_timeout_for_grace(timeout);
        let id = id.to_string();
        let stop_options = j(&StopOptions { signal, timeout });
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(SERVICE_NAME)?;
            let result = conn.send_with_timeout(
                "containerStop",
                &[
                    (id_key(), s(&id)),
                    (k("stopOptions"), KeyValue::Data(stop_options)),
                ],
                xpc_timeout,
            );
            match result {
                Ok(_) => Ok(()),
                // 既に存在しないコンテナへの stop は冪等に成功扱いにする (本家の 304/404 無視相当)。
                Err(e) if is_not_found_error(&e) => Ok(()),
                Err(e) => Err(e),
            }
        })
        .await?
    }

    /// ホストからコンテナへファイルをコピーする。
    pub(crate) async fn copy_in(
        &self,
        id: &str,
        source: &std::path::Path,
        destination: &str,
        mode: u32,
    ) -> Result<()> {
        let id = id.to_string();
        // ホスト側の相対パスは、XPC の受け手 (apiserver) の cwd ではなく
        // 呼び出し側の cwd 基準で解決する。apple/container の cp と同じ基準に揃える。
        let source = absolutize_host_path(source)?;
        let source = host_path_for_xpc(&source)?.to_string();
        let destination = destination.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(SERVICE_NAME)?;
            conn.send(
                "containerCopyIn",
                &[
                    (id_key(), s(&id)),
                    (k("sourcePath"), s(&source)),
                    (k("destinationPath"), s(&destination)),
                    (k("fileMode"), KeyValue::UInt64(u64::from(mode))),
                    (k("createParents"), KeyValue::Bool(true)),
                ],
            )?;
            Ok(())
        })
        .await?
    }

    /// コンテナからホストへファイルをコピーする。
    pub(crate) async fn copy_out(
        &self,
        id: &str,
        source: &std::path::Path,
        destination: &std::path::Path,
    ) -> Result<()> {
        let id = id.to_string();
        let source = host_path_for_xpc(source)?.to_string();
        // コピー先はホスト側パスなので、相対指定を呼び出し側の cwd 基準で解決する。
        let destination = absolutize_host_path(destination)?;
        let destination = host_path_for_xpc(&destination)?.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(SERVICE_NAME)?;
            conn.send(
                "containerCopyOut",
                &[
                    (id_key(), s(&id)),
                    (k("sourcePath"), s(&source)),
                    (k("destinationPath"), s(&destination)),
                    (k("createParents"), KeyValue::Bool(true)),
                ],
            )?;
            Ok(())
        })
        .await?
    }

    /// コンテナを削除する。
    pub(crate) async fn remove(&self, id: &str, force: bool) -> Result<()> {
        let id = id.to_string();
        tokio::task::spawn_blocking(move || Self::remove_blocking(&id, force)).await?
    }

    /// `remove` の同期版。Drop などランタイム外から呼ぶ。
    ///
    /// 既に存在しないコンテナへの削除は冪等に成功扱いにする (本家の 404/409 無視相当)。
    /// 外部で削除された後の `rm()` や、rm 済みハンドルの Drop がエラーにならないため。
    pub(crate) fn remove_blocking(id: &str, force: bool) -> Result<()> {
        let conn = XpcConn::connect(SERVICE_NAME)?;
        let result = conn.send(
            "containerDelete",
            &[(id_key(), s(id)), (k("forceDelete"), KeyValue::Bool(force))],
        );
        match result {
            Ok(_) => Ok(()),
            Err(e) if is_not_found_error(&e) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// コンテナのログ FD を取得する。
    ///
    /// 返り値は `(stdout_fd, stderr_fd)`。XPC `containerLogs` が 2 個の FD を返す前提。
    pub(crate) async fn logs(&self, id: &str) -> Result<(std::os::fd::RawFd, std::os::fd::RawFd)> {
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(SERVICE_NAME)?;
            let reply = conn.send("containerLogs", &[(id_key(), s(&id))])?;
            let fds = reply.log_fds();
            if fds.len() < 2 {
                // 取得済みの有効な FD を close してからエラーにする (FD リーク防止)。
                close_valid_fds(&fds);
                return Err(
                    ClientError::Other("containerLogs did not return enough fds".into()).into(),
                );
            }
            // dup 失敗 (fd 枯渇等) は -1 が入る。有効な方だけ close してエラーにする。
            if fds[0] < 0 || fds[1] < 0 {
                close_valid_fds(&fds);
                return Err(
                    ClientError::Other("containerLogs returned an invalid fd".into()).into(),
                );
            }
            Ok((fds[0], fds[1]))
        })
        .await?
    }

    /// 名前付きボリュームを解決し、その実体パスとファイルシステム形式を返す。
    ///
    /// Apple container は volume マウントを block デバイスとして扱い、
    /// `Filesystem.volume` の `source` にボリューム実体の絶対パスを要求する
    /// (Apple の CLI 実装 `Utility.containerConfigFromFlags` は `getOrCreateVolume` で
    /// 実パスを解決して渡している。ボリューム名のままではマウントできない)。
    /// CLI と同様に `volumeCreate` (既存なら `volumeInspect`) で解決する。
    /// 実装は Apple container 1.2.0 で検証している。将来のバージョンで
    /// XPC のエラー形式が変わると already exists 判定がずれる可能性がある。
    /// 自動作成されたボリュームは、コンテナの起動失敗時も削除されず残る
    /// (Docker の名前付きボリューム自動作成と同じ挙動)。
    pub(crate) async fn resolve_volume(&self, name: &str) -> Result<VolumeResolution> {
        let name = name.to_string();
        // Apple のボリューム名制約 (英数字始まり・255 文字以内) を事前検証する。
        // 不正名は volumeCreate が分かりにくい XPC エラーを返すため、ここで落とす。
        if !is_valid_volume_name(&name) {
            return Err(ClientError::Other(format!(
                "invalid volume name {name:?}: must match ^[A-Za-z0-9][A-Za-z0-9_.-]*$ and be at most 255 characters"
            ))
            .into());
        }
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(SERVICE_NAME)?;
            // まず自動作成を試みる。既に存在する場合は inspect にフォールバックする。
            let reply = conn.send(
                "volumeCreate",
                &[(k("volumeName"), s(&name)), (k("volumeDriver"), s("local"))],
            );
            let data = match reply {
                Ok(r) => r.data(&k("volume")),
                Err(e) if is_volume_already_exists_error(&e) => conn
                    .send("volumeInspect", &[(k("volumeName"), s(&name))])?
                    .data(&k("volume")),
                Err(e) => return Err(e),
            };
            let data = data.ok_or_else(|| {
                crate::core::error::Error::Client(ClientError::Other(format!(
                    "volume '{name}' resolution did not return volume"
                )))
            })?;
            parse_volume_configuration(&data)
        })
        .await?
    }

    /// コンテナの状態を取得する。
    ///
    /// `containerList` のレスポンスから `status` と `configuration.publishedPorts` を取得する。
    pub(crate) async fn container_state(&self, id: &str) -> Result<ContainerSnapshot> {
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            with_first_container(&id, |item| {
                let state: String = item
                    .to_member("status")
                    .and_then(|m| m.required())
                    .and_then(|v| v.try_into())
                    .unwrap_or_else(|_| "unknown".into());
                let running = state == "running";
                let ports = parse_published_ports(item);
                Ok(ContainerSnapshot { running, ports })
            })
        })
        .await?
    }

    /// コンテナのブリッジ IP アドレスを取得する。
    ///
    /// `containerList` のレスポンスから `networks[0].ipv4Address` を取得する。
    /// ネットワークが無い場合はエラー。
    pub(crate) async fn bridge_ip_address(&self, id: &str) -> Result<IpAddr> {
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            with_first_container(&id, |item| {
                let networks = item
                    .to_member("networks")
                    .ok()
                    .and_then(|m| m.optional())
                    .and_then(|v| v.to_array().ok())
                    .ok_or_else(|| ClientError::Other("no network attachments".into()))?;
                let first = networks
                    .into_iter()
                    .next()
                    .ok_or_else(|| ClientError::Other("no network attachments".into()))?;
                let addr_str: String = first
                    .to_member("ipv4Address")
                    .and_then(|m| m.required())
                    .and_then(|v| v.try_into())
                    .map_err(|e| ClientError::Json(e.to_string()))?;
                // CIDR 表記 "xxx.xxx.xxx.xxx/yy" からアドレス部分のみを取り出す。
                let addr_only = addr_str
                    .split_once('/')
                    .map_or(addr_str.as_str(), |(addr, _)| addr);
                addr_only.parse::<IpAddr>().map_err(|e| {
                    ClientError::Other(format!("invalid bridge ip address: {e}")).into()
                })
            })
        })
        .await?
    }

    /// `containerList` のレスポンスから `networks[0].ipv4Gateway` を取得する。
    /// ゲートウェイが取得できない場合はエラー。
    pub(crate) async fn gateway_ip_address(&self, id: &str) -> Result<IpAddr> {
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            with_first_container(&id, |item| {
                let networks = item
                    .to_member("networks")
                    .ok()
                    .and_then(|m| m.optional())
                    .and_then(|v| v.to_array().ok())
                    .ok_or_else(|| ClientError::Other("no network attachments".into()))?;
                let first = networks
                    .into_iter()
                    .next()
                    .ok_or_else(|| ClientError::Other("no network attachments".into()))?;
                let addr_str: String = first
                    .to_member("ipv4Gateway")
                    .and_then(|m| m.required())
                    .and_then(|v| v.try_into())
                    .map_err(|e| ClientError::Json(e.to_string()))?;
                // CIDR 表記の場合に備えてアドレス部分のみを取り出す。
                let addr_only = addr_str
                    .split_once('/')
                    .map_or(addr_str.as_str(), |(addr, _)| addr);
                addr_only.parse::<IpAddr>().map_err(|e| {
                    ClientError::Other(format!("invalid gateway ip address: {e}")).into()
                })
            })
        })
        .await?
    }

    /// コンテナの公開ポートを取得する。
    ///
    /// `container_state` から `publishedPorts` を取得する。
    pub(crate) async fn ports(&self, id: &str) -> Result<Ports> {
        let snapshot = self.container_state(id).await?;
        Ok(snapshot.ports)
    }

    /// コンテナ内でコマンドを実行し、終了コード・stdout・stderr を取得する。
    ///
    /// `environment` は `KEY=VALUE` 形式の配列。コンテナ作成時の env
    /// (`with_env_var` / `Image::env_vars`) をそのまま渡す。
    pub(crate) async fn exec(
        &self,
        id: &str,
        cmd: &[String],
        environment: Vec<String>,
    ) -> Result<XpcExecResult> {
        if cmd.is_empty() {
            return Err(ClientError::Other("exec requires at least one argument".into()).into());
        }
        let id = id.to_string();
        let executable = cmd[0].clone();
        let arguments: Vec<String> = if cmd.len() > 1 {
            cmd[1..].to_vec()
        } else {
            vec![]
        };
        // processIdentifier はタイムスタンプだけでは並行 exec で衝突するため、
        // プロセス内カウンタを組み合わせる。
        let pid = format!("exec-{}", crate::core::util::unique_suffix());
        tokio::task::spawn_blocking(move || {
            // exec 結果の stdout / stderr 用 pipe を作成する。
            // fd は File として所有し、エラーパスでも Drop で確実に close する。
            let (out_read, out_write) = create_pipe()?;
            let (err_read, err_write) = create_pipe()?;

            let conn = XpcConn::connect(SERVICE_NAME)?;
            let cfg = ProcCfg {
                executable,
                arguments,
                environment,
                // exec 用 ProcessConfiguration では作業ディレクトリを公開 API から受けずルート固定。
                working_directory: "/".into(),
            };
            use std::os::fd::AsRawFd;
            conn.send(
                "containerCreateProcess",
                &[
                    (id_key(), s(&id)),
                    (k("processIdentifier"), s(&pid)),
                    (k("processConfig"), KeyValue::Data(j(&cfg))),
                    (k("stdout"), KeyValue::Fd(out_write.as_raw_fd())),
                    (k("stderr"), KeyValue::Fd(err_write.as_raw_fd())),
                ],
            )?;
            // XPC が dup 済みなので、書き込み端はこちらで close してよい。
            // close しないと読み取り側に EOF が届かない。
            drop(out_write);
            drop(err_write);

            conn.send(
                "containerStartProcess",
                &[(id_key(), s(&id)), (k("processIdentifier"), s(&pid))],
            )?;

            // stdout / stderr は containerWait と並行に読む。
            // 終了待ちを先にすると、出力がパイプバッファ (64KB) を超えた時点で
            // プロセスが write でブロックし、永遠に終了しないデッドロックになる。
            // 読み取りスレッドはキャンセルフラグ付きで起動する。containerWait
            // 失敗時、および containerWait 成功後の join 待ちがタイムアウトした
            // 際にフラグを立てると、poll の確認間隔以内に FD を閉じて終了する
            // (FD を握り続けるリークを防ぐ)。
            //
            // 結果は 1 本の mpsc チャネルで受け取る。std::thread::join には
            // タイムアウトが無いため、`recv_timeout` で上限時間を課す。
            // 2 本のチャネルを直列に待つと最悪合計 2 倍の時間がかかるため、
            // ストリーム識別付き enum を 1 本のチャネルで受ける構造にする。
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (tx, rx) = std::sync::mpsc::channel::<ExecReaderMsg>();
            let out_cancel = cancel.clone();
            let err_cancel = cancel.clone();
            let out_tx = tx.clone();
            let err_tx = tx;
            std::thread::spawn(move || {
                let r = read_file_to_vec_cancellable(out_read, out_cancel);
                let _ = out_tx.send(ExecReaderMsg::Stdout(r));
            });
            std::thread::spawn(move || {
                let r = read_file_to_vec_cancellable(err_read, err_cancel);
                let _ = err_tx.send(ExecReaderMsg::Stderr(r));
            });

            let reply = match conn.send_with_timeout(
                "containerWait",
                &[(id_key(), s(&id)), (k("processIdentifier"), s(&pid))],
                crate::xpc::LONG_TIMEOUT,
            ) {
                Ok(r) => r,
                Err(e) => {
                    // wait 失敗時は読み取りスレッドに終了指示を出してから戻る。
                    // join するとデーモン側が書き込み端を閉じるまで戻れない可能性が
                    // あるため、join はせずデタッチのままにする。フラグが立つと
                    // 読み取りスレッドは poll 間隔以内に FD を閉じて終了する
                    // (FD を握り続けるリークを防ぐ)。
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    return Err(e);
                }
            };

            // プロセス終了後、デーモンが書き込み端を閉じると EOF になり読み取り
            // スレッドから結果が届く。EOF が来ない異常系 (デーモンが dup 済みの
            // write FD を閉じない、exec 子プロセスが write FD を継承したまま残る)
            // では EXEC_READER_JOIN_TIMEOUT を超えた時点でキャンセルフラグを立てて
            // 打ち切り、エラーを返す。打ち切り時は読み切れた分の出力と exit code を捨てる。
            let (stdout, stderr) = join_exec_readers(&rx, &cancel, EXEC_READER_JOIN_TIMEOUT)?;

            let exit_code = reply.try_int64(&k("exitCode"))?;
            Ok(XpcExecResult {
                exit_code: Some(exit_code),
                stdout,
                stderr,
            })
        })
        .await?
    }

    /// イメージをプルする。
    ///
    /// Apple container はホスト部の無い参照を拒否するため、送信前に正規化する。
    /// プルはダウンロードを伴う長時間操作なので `LONG_TIMEOUT` で待つ。
    ///
    /// `platform_arch` が `Some` のときだけ `ociPlatform` に
    /// `{"os":"linux","architecture":...}` を載せる。未指定時はキーごと省略する。
    pub(crate) async fn pull_image(&self, image: &str, platform_arch: Option<&str>) -> Result<()> {
        let image = normalize_image_reference(image);
        let oci_platform = platform_arch.map(oci_platform_json);
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(IMAGE_SERVICE)?;
            let mut args = vec![
                (k("imageReference"), s(&image)),
                (k("insecureFlag"), KeyValue::Bool(false)),
                (k("maxConcurrentDownloads"), KeyValue::Int64(3)),
            ];
            if let Some(pf) = oci_platform {
                args.push((k("ociPlatform"), KeyValue::Data(pf)));
            }
            conn.send_with_timeout("imagePull", &args, crate::xpc::LONG_TIMEOUT)?;
            Ok(())
        })
        .await?
    }

    /// デフォルトカーネルを取得する。
    ///
    /// Apple Silicon 上ではゲストが amd64 (Rosetta) でもホスト既定の arm64 カーネルを使う。
    /// `getDefaultKernel` に amd64 を渡すと未インストール時に notFound になる一方、
    /// CLI の `container run --arch amd64` は arm64 カーネルのみの環境でも動作する。
    pub(crate) async fn get_default_kernel(&self) -> Result<Vec<u8>> {
        let pf = oci_platform_json("arm64");
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(SERVICE_NAME)?;
            let reply = conn.send(
                "getDefaultKernel",
                &[(k("systemPlatform"), KeyValue::Data(pf))],
            )?;
            reply
                .data(&k("kernel"))
                .ok_or_else(|| ClientError::Other("no kernel".into()).into())
        })
        .await?
    }

    /// イメージの descriptor を解決する。
    pub(crate) async fn resolve_image_descriptor(&self, image: &str) -> Result<String> {
        let image = image.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(IMAGE_SERVICE)?;
            let reply = conn.send("imageList", &[])?;
            let data = reply.data(&k("imageDescriptions")).unwrap_or_default();
            match_image_descriptor(&data, &image).map_err(Into::into)
        })
        .await?
    }

    /// 指定 digest の blob を content store から読み出す。
    ///
    /// XPC `contentGet` はローカル content store 上のファイルパスを返す。
    /// 返り値はそのファイルの内容。
    pub(crate) async fn content_get(&self, digest: &str) -> Result<Vec<u8>> {
        let digest = digest.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(IMAGE_SERVICE)?;
            let reply = conn.send("contentGet", &[(k("digest"), s(&digest))])?;
            let path = reply.string(&k("contentPath")).ok_or_else(|| {
                ClientError::Other("contentGet did not return contentPath".into())
            })?;
            std::fs::read(&path).map_err(|e| {
                ClientError::Other(format!(
                    "failed to read content for {digest} at {path}: {e}"
                ))
                .into()
            })
        })
        .await?
    }

    /// コンテナを XPC で作成する。
    ///
    /// `container_cfg` は呼び出し側が構築した DisplayJson ペイロード
    /// (典型的には `container_cfg::ContainerCfg`)。シリアライズ (`j`) は
    /// `spawn_blocking` の前に行い、閉包へはバイト列だけを move する。
    /// `kernel` は `get_default_kernel` の戻り値。
    pub(crate) async fn create_container(
        &self,
        container_cfg: &impl DisplayJson,
        kernel: Vec<u8>,
    ) -> Result<()> {
        // spawn_blocking に DisplayJson を持ち込まないため、先にバイト列化する。
        let container_cfg = j(container_cfg);
        let opts = j(&CreateOpts { auto_remove: false });
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(SERVICE_NAME)?;
            conn.send(
                "containerCreate",
                &[
                    (k("containerConfig"), KeyValue::Data(container_cfg)),
                    (k("kernel"), KeyValue::Data(kernel)),
                    (k("containerOptions"), KeyValue::Data(opts)),
                ],
            )?;
            Ok(())
        })
        .await?
    }

    /// コンテナをブートストラップする。
    pub(crate) async fn bootstrap_container(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(SERVICE_NAME)?;
            conn.send(
                "containerBootstrap",
                &[
                    (id_key(), s(&id)),
                    (k("dynamicEnv"), KeyValue::Data(b"{}".to_vec())),
                ],
            )?;
            Ok(())
        })
        .await?
    }

    /// コンテナの初期プロセスを開始する。
    pub(crate) async fn start_process(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(SERVICE_NAME)?;
            conn.send(
                "containerStartProcess",
                &[(id_key(), s(&id)), (k("processIdentifier"), s(&id))],
            )?;
            Ok(())
        })
        .await?
    }
}

/// `containerList` を取得し、先頭のコンテナエントリに対して `f` を適用する。
///
/// `container_state` / `bridge_ip_address` の共通部分 (XPC 接続・filters 構築・
/// 送信・空チェック・JSON パース・先頭要素取得) を集約する。
fn with_first_container<F, R>(id: &str, f: F) -> Result<R>
where
    F: FnOnce(&nojson::RawJsonValue<'_, '_>) -> Result<R>,
{
    let conn = XpcConn::connect(SERVICE_NAME)?;
    let filters = j(&xpc::Filters {
        ids: vec![id.to_string()],
        labels: HashMap::new(),
    });
    let reply = conn.send(
        "containerList",
        &[(k("listFilters"), KeyValue::Data(filters))],
    )?;
    let data = reply.data(&k("containers")).unwrap_or_default();
    let parsed = parse_container_list(&data, id)?;
    let arr = parsed
        .value()
        .to_array()
        .map_err(|e| ClientError::Json(e.to_string()))?;
    let item = arr
        .into_iter()
        .next()
        .ok_or_else(|| ClientError::ContainerNotFound(id.to_string()))?;
    f(&item)
}

/// `containerList` 応答の `containers` バイト列をパースして返す。
///
/// - 空データ → `ContainerNotFound`
/// - 非 UTF-8・パース失敗 → `Json`
fn parse_container_list<'a>(
    data: &'a [u8],
    id: &str,
) -> std::result::Result<nojson::RawJson<'a>, ClientError> {
    if data.is_empty() {
        return Err(ClientError::ContainerNotFound(id.to_string()));
    }
    let text = std::str::from_utf8(data)
        .map_err(|e| ClientError::Json(format!("containerList response is not UTF-8: {e}")))?;
    nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))
}

/// エラーが「コンテナが存在しない」ことを表すか。
///
/// Apple container の XPC エラーは `ClientError::Xpc("XPC error <code>: <message>")` の
/// 文字列として届くため、notFound コードはプレフィクスで判定する。
fn is_not_found_error(e: &crate::core::error::Error) -> bool {
    match e {
        crate::core::error::Error::Client(ClientError::ContainerNotFound(_)) => true,
        crate::core::error::Error::Client(ClientError::Xpc(msg)) => {
            msg.starts_with("XPC error notFound")
        }
        _ => false,
    }
}

/// エラーが「同名ボリュームが既に存在する」ことを表すか。
///
/// 既存ボリュームへの `volumeCreate` は `VolumeError.volumeAlreadyExists` が
/// XPC エラーのメッセージに `volume '<name>' already exists` の形で含まれて返る
/// (message 部分は Apple container 1.2.0 で実測。`xpc_alpine_with_existing_volume_mount`
/// 統合テストで固定済み。code 部分の値は未検証のため判定には使わない)。
/// 将来のバージョンで文面が変わると判定がずれる可能性がある。
fn is_volume_already_exists_error(e: &crate::core::error::Error) -> bool {
    match e {
        crate::core::error::Error::Client(ClientError::Xpc(msg)) => msg.contains("already exists"),
        _ => false,
    }
}

/// ボリューム名が Apple container の制約を満たすか。
///
/// `VolumeStorage.volumeNamePattern` (`^[A-Za-z0-9][A-Za-z0-9_.-]*$`) と
/// 255 文字以下の制約 (Apple container 1.2.0 の `isValidVolumeName`)。
fn is_valid_volume_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// ボリューム解決の結果。`Filesystem.volume` の `source` / `format` に使う。
#[derive(Debug)]
pub(crate) struct VolumeResolution {
    /// ボリューム実体の絶対パス (block デバイスイメージ)。
    pub(crate) source: String,
    /// ボリュームのファイルシステム形式 (例: `"ext4"`)。
    pub(crate) format: String,
}

/// `ContainerRequest` の volume マウントをまとめて解決する。
///
/// 同一名の重複マウントは 1 回の解決にまとめ、決定的な順序で 1 つずつ解決する。
/// 途中で失敗した場合は、それまでに自動作成されたボリュームは残る
/// (Docker の名前付きボリューム自動作成と同じ挙動)。
pub(crate) async fn resolve_volumes<I: crate::Image>(
    client: &XpcClient,
    req: &crate::ContainerRequest<I>,
) -> Result<std::collections::HashMap<String, VolumeResolution>> {
    let mut names: Vec<String> = req
        .mounts()
        .filter(|m| matches!(m.mount_type(), crate::core::mounts::MountType::Volume))
        .filter_map(|m| m.source().map(str::to_owned))
        .collect();
    names.sort();
    names.dedup();
    let mut resolutions = std::collections::HashMap::new();
    for name in names {
        // どのボリュームで失敗したか分かるように、ボリューム名を添えて変換する。
        let resolution = client
            .resolve_volume(&name)
            .await
            .map_err(|e| crate::Error::other(format!("failed to resolve volume '{name}': {e}")))?;
        resolutions.insert(name.clone(), resolution);
    }
    Ok(resolutions)
}

/// `VolumeConfiguration` の JSON から `source` / `format` を取り出す。
///
/// `volumeCreate` / `volumeInspect` のレスポンス (`volume` キー) の形式。
/// `source` / `format` が欠落した JSON はエラーにする。
fn parse_volume_configuration(data: &[u8]) -> Result<VolumeResolution> {
    let text = std::str::from_utf8(data)
        .map_err(|e| ClientError::Json(format!("volume configuration is not UTF-8: {e}")))?;
    let parsed = nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))?;
    let v = parsed.value();
    let source: String = v
        .to_member("source")
        .and_then(|m| m.required())
        .and_then(|v| v.try_into())
        .map_err(|e| {
            ClientError::Json(format!(
                "volume configuration missing or invalid source: {e}"
            ))
        })?;
    let format: String = v
        .to_member("format")
        .and_then(|m| m.required())
        .and_then(|v| v.try_into())
        .map_err(|e| {
            ClientError::Json(format!(
                "volume configuration missing or invalid format: {e}"
            ))
        })?;
    Ok(VolumeResolution { source, format })
}

/// イメージ参照を Apple container が受け付ける完全修飾形式に正規化する。
///
/// Apple container の image service はホスト部の無い参照を
/// `invalidArgument: no host specified in image reference` で拒否するため、
/// Docker の参照正規化 (`ParseNormalizedNamed`) と同じ規則で補完する:
/// - `/` を含まない (`alpine:latest`) → `docker.io/library/` を前置
/// - 先頭コンポーネントがレジストリホスト (`.` か `:` を含む、または `localhost`) → そのまま
/// - それ以外 (`user/repo`) → `docker.io/` を前置
///
/// タグ区切りの `:` は最終コンポーネントにしか現れないため、
/// 先頭コンポーネントの判定には影響しない。
pub(crate) fn normalize_image_reference(image: &str) -> String {
    match image.split_once('/') {
        None => format!("docker.io/library/{image}"),
        Some((first, _)) => {
            if first.contains('.') || first.contains(':') || first == "localhost" {
                image.to_string()
            } else {
                format!("docker.io/{image}")
            }
        }
    }
}

/// `imageList` の `imageDescriptions` バイト列から参照一致の descriptor JSON を取り出す。
///
/// - 空データ / 一致無し → `ImageNotFound`
/// - 非 UTF-8・パース失敗・非配列 → `Json`
/// - 参照一致だが `descriptor` 欠落 → `Other` (後続エントリは探さない)
fn match_image_descriptor(data: &[u8], image: &str) -> std::result::Result<String, ClientError> {
    if data.is_empty() {
        return Err(ClientError::ImageNotFound(image.to_string()));
    }

    let text = std::str::from_utf8(data)
        .map_err(|e| ClientError::Json(format!("imageDescriptions is not UTF-8: {e}")))?;
    let parsed = nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))?;
    let arr = parsed
        .value()
        .to_array()
        .map_err(|e| ClientError::Json(e.to_string()))?;

    let docker_ref = normalize_image_reference(image);
    for item in arr {
        let ref_str = xpc::member_opt_string(&item, "reference");
        let matched = ref_str.as_deref() == Some(image) || ref_str.as_deref() == Some(&docker_ref);
        if !matched {
            continue;
        }
        match item.to_member("descriptor").and_then(|m| m.required()) {
            Ok(desc) => return Ok(desc.extract().text().to_string()),
            Err(_) => {
                return Err(ClientError::Other(format!(
                    "image {image}: matched reference but descriptor is missing"
                )));
            }
        }
    }

    Err(ClientError::ImageNotFound(image.to_string()))
}

/// pipe(2) を作り、(読み取り端, 書き込み端) を所有権付きの `File` で返す。
///
/// 両端に `FD_CLOEXEC` を立てる。本プロセスが後続で fork/exec した子へ
/// パイプが継承されると、読み取りスレッドに EOF が届かず join が永久に返らないため。
/// `File` の Drop で close されるため、エラーパスでも fd がリークしない。
fn create_pipe() -> Result<(std::fs::File, std::fs::File)> {
    let mut fds = [-1i32; 2];
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return Err(ClientError::Other("failed to create pipe".into()).into());
        }
        for &fd in &fds {
            if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == -1 {
                // fcntl 失敗時は両端を閉じてから Err (fd リーク禁止)。
                let _ = libc::close(fds[0]);
                let _ = libc::close(fds[1]);
                return Err(ClientError::Other("failed to set FD_CLOEXEC on pipe".into()).into());
            }
        }
        Ok((
            std::fs::File::from_raw_fd(fds[0]),
            std::fs::File::from_raw_fd(fds[1]),
        ))
    }
}

/// 有効な FD (0 以上) だけを close する。エラーパスでの FD リーク防止用。
fn close_valid_fds(fds: &[std::os::fd::RawFd]) {
    for fd in fds {
        if *fd >= 0 {
            unsafe { libc::close(*fd) };
        }
    }
}

/// exec の読み取りスレッドから結果を受け取る際のメッセージ。
///
/// stdout / stderr のどちらのストリームからの結果かを識別する。
/// 1 本の mpsc チャネルで両方を受け取るために enum で束ねる。
enum ExecReaderMsg {
    Stdout(std::io::Result<Option<Vec<u8>>>),
    Stderr(std::io::Result<Option<Vec<u8>>>),
}

/// 未受信のストリーム名を人間可読な形で返す。
///
/// エラーメッセージ用の共通ヘルパ。「少なくとも片方が未受信」の状況でのみ使う契約で、
/// `has_stdout` と `has_stderr` の両方が `true` の場合は呼び出し規約違反として
/// `unreachable!` にする。
fn describe_missing(has_stdout: bool, has_stderr: bool) -> &'static str {
    match (has_stdout, has_stderr) {
        (false, false) => "stdout/stderr",
        (false, true) => "stdout",
        (true, false) => "stderr",
        (true, true) => unreachable!("stdout / stderr の少なくとも一方が未受信"),
    }
}

/// exec の読み取りスレッド 2 本の結果を上限時間付きで待ち、stdout / stderr のバイト列を返す。
///
/// `containerWait` が成功で返った時点でプロセスは終了済みなので、通常はデーモンが
/// write FD を閉じて即座に EOF が届き、この関数はほぼブロックせずに返る。
/// EOF が来ない異常系 (デーモンが dup 済みの write FD を閉じない、exec 子プロセスが
/// write FD を継承したまま残る等) では `timeout` を超えた時点で `cancel` フラグを
/// 立てて両方のスレッドを打ち切る。フラグ観測から poll 間隔 100ms + read 1 回以内に
/// スレッドは終了し、FD を回収する。
///
/// 打ち切り時は読み切れた分の出力を捨て、どのストリームが応答しなかったかを含めた
/// エラーを返す。読み取りスレッドが結果を送らずに終了した (panic 等) 場合も
/// エラーになる (通常は起こらない)。
fn join_exec_readers(
    rx: &std::sync::mpsc::Receiver<ExecReaderMsg>,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    timeout: Duration,
) -> Result<(Vec<u8>, Vec<u8>)> {
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Instant;

    let deadline = Instant::now() + timeout;
    let mut stdout: Option<std::io::Result<Option<Vec<u8>>>> = None;
    let mut stderr: Option<std::io::Result<Option<Vec<u8>>>> = None;

    // 2 通のメッセージ (stdout / stderr) を上限時間内に受け取る。
    // 片方が届いても、もう一方の deadline 残り時間で recv_timeout を続ける。
    while stdout.is_none() || stderr.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(ExecReaderMsg::Stdout(r)) => stdout = Some(r),
            Ok(ExecReaderMsg::Stderr(r)) => stderr = Some(r),
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => {
                // 送信側 (両方の読み取りスレッド) が結果を送らずに全て終了した状態
                // (panic 等)。通常は起こらないが、片方だけ受信済みで残りが panic した
                // 場合も含めて、どのストリームが応答しなかったかを明示する。
                let missing = describe_missing(stdout.is_some(), stderr.is_some());
                return Err(ClientError::Other(format!(
                    "exec reader thread for {missing} terminated without sending result"
                ))
                .into());
            }
        }
    }

    if stdout.is_none() || stderr.is_none() {
        // 待ち上限を超過。キャンセルフラグを立てて、残っているスレッド (両方未着なら
        // 2 本、片方だけ未着なら残り 1 本) の終了を待ち、FD を回収する。
        // フラグ観測後は poll 間隔 (100ms) + read 1 回以内に終了する契約
        // (read_file_to_vec_cancellable の doc 参照) のため無限待ちしてよい。
        // 未受信ストリーム名は 2 段目 recv 前にスナップショットする
        // (期限内に届かなかった、という判定は 2 段目の結果で変わらない)。
        cancel.store(true, Ordering::Relaxed);
        let missing = describe_missing(stdout.is_some(), stderr.is_some());
        while stdout.is_none() || stderr.is_none() {
            match rx.recv() {
                Ok(ExecReaderMsg::Stdout(r)) => stdout = Some(r),
                Ok(ExecReaderMsg::Stderr(r)) => stderr = Some(r),
                Err(std::sync::mpsc::RecvError) => break,
            }
        }
        return Err(ClientError::Other(format!(
            "read {missing} timed out after {}ms",
            timeout.as_millis()
        ))
        .into());
    }

    // 両方のストリームが期限内に届いた。
    // 読み取り自体のエラーはそのまま伝播する。フラグを立てていない正常系では
    // `Ok(None)` は返らないため、`Ok(None)` は打ち切りエラーに変換する
    // (通常はここには到達しない: cancel フラグを立てるのは上のタイムアウト分岐のみ)。
    let stdout_bytes = match stdout.expect("上のループでチェック済み") {
        Ok(Some(b)) => b,
        Ok(None) => {
            return Err(
                ClientError::Other("exec stdout reader was cancelled unexpectedly".into()).into(),
            );
        }
        Err(e) => return Err(ClientError::Other(format!("read stdout failed: {e}")).into()),
    };
    let stderr_bytes = match stderr.expect("上のループでチェック済み") {
        Ok(Some(b)) => b,
        Ok(None) => {
            return Err(
                ClientError::Other("exec stderr reader was cancelled unexpectedly".into()).into(),
            );
        }
        Err(e) => return Err(ClientError::Other(format!("read stderr failed: {e}")).into()),
    };
    Ok((stdout_bytes, stderr_bytes))
}

/// `File` を EOF まで読み出すが、キャンセルフラグが立つと読み取りを打ち切って
/// `Ok(None)` を返す。
///
/// `libc::poll` で FD の読み取り可能性を監視しつつ、poll のタイムアウトごとに
/// フラグを確認する。フラグが立っているのを観測したら即座に読み取りを打ち切って
/// 終了する (poll のタイムアウト 100ms 以内に観測される)。64 MiB を超える場合は
/// エラーを返す (OOM 防止)。
///
/// フラグが立たない正常系では EOF まで読み切って `Ok(Some(bytes))` を返す。
/// 時間による打ち切りはこの関数自体には無い (5 秒超の exec でも出力を失わない)。
///
/// 呼び出し元は 2 通りの場面でフラグを立て得る:
/// - `XpcClient::exec` の `containerWait` 失敗時のエラーパス。デタッチした読み取り
///   スレッドが読み取り端 FD を握り続けるのを防ぐ。
/// - `join_exec_readers` のタイムアウト分岐。`containerWait` 成功後、EOF が
///   来ない異常系 (デーモンが dup 済みの write FD を閉じない、exec 子プロセスが
///   write FD を継承したまま残る等) で `EXEC_READER_JOIN_TIMEOUT` を超えた場合。
///
/// フラグを立てると、継続実行中のプロセスが書き込み端への書き込みで EPIPE を
/// 受け得るが、いずれの場合も exec は既に失敗扱いのため許容する
/// (containerWait 成功後の打ち切りではプロセスは終了済み、エラーパスでは exec
/// 呼び出し自体が失敗を返すため)。
fn read_file_to_vec_cancellable(
    mut f: std::fs::File,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::io::Result<Option<Vec<u8>>> {
    use std::os::fd::AsRawFd;
    use std::sync::atomic::Ordering;

    const MAX_OUTPUT_SIZE: u64 = 64 * 1024 * 1024;
    const POLL_TIMEOUT_MS: i32 = 100;

    let fd = f.as_raw_fd();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        // poll のタイムアウトごとにフラグを確認する。フラグが立っていれば
        // 読み取りを打ち切って終了する (poll のタイムアウト以内に観測される)。
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ret = unsafe { libc::poll(&mut pfd, 1, POLL_TIMEOUT_MS) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if ret == 0 {
            // タイムアウト。フラグ確認に戻る。
            continue;
        }
        // POLLERR / POLLNVAL (読み取り不可能なエラー状態) は明示的に返す。
        if pfd.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(std::io::Error::other(format!(
                "poll on exec output pipe failed with revents {:#x}",
                pfd.revents
            )));
        }
        // 読み取り可能または HUP (書き込み端が全て閉じた = EOF)。
        if pfd.revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            match f.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() as u64 > MAX_OUTPUT_SIZE {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::OutOfMemory,
                            format!("output exceeds {MAX_OUTPUT_SIZE} bytes limit"),
                        ));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
    Ok(Some(buf))
}

/// `exec` の結果。
pub(crate) struct XpcExecResult {
    pub(crate) exit_code: Option<i64>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// `containerList` レスポンスの `configuration.publishedPorts` から `Ports` を構築する。
fn parse_published_ports(item: &nojson::RawJsonValue<'_, '_>) -> Ports {
    let mut ports = Ports::default();
    let config = match item
        .to_member("configuration")
        .ok()
        .and_then(|m| m.optional())
    {
        Some(c) => c,
        None => return ports,
    };
    let published_ports = match config
        .to_member("publishedPorts")
        .ok()
        .and_then(|m| m.optional())
    {
        Some(p) => p,
        None => return ports,
    };
    let arr = match published_ports.to_array() {
        Ok(a) => a,
        Err(_) => return ports,
    };
    for p in arr {
        let container_port: u16 = p
            .to_member("containerPort")
            .ok()
            .and_then(|m| m.optional())
            .and_then(|v| TryInto::<u16>::try_into(v).ok())
            .unwrap_or(0);
        let host_port: u16 = p
            .to_member("hostPort")
            .ok()
            .and_then(|m| m.optional())
            .and_then(|v| TryInto::<u16>::try_into(v).ok())
            .unwrap_or(0);
        let proto: String = p
            .to_member("proto")
            .ok()
            .and_then(|m| m.optional())
            .and_then(|v| TryInto::<String>::try_into(v).ok())
            .unwrap_or_else(|| "tcp".into());
        let host_address: String = p
            .to_member("hostAddress")
            .ok()
            .and_then(|m| m.optional())
            .and_then(|v| TryInto::<String>::try_into(v).ok())
            .unwrap_or_default();
        let container_port = match proto.as_str() {
            "udp" => ContainerPort::Udp(container_port),
            "sctp" => ContainerPort::Sctp(container_port),
            _ => ContainerPort::Tcp(container_port),
        };
        // ホストポート 0 / コンテナポート 0 のエントリは Ports に登録しない。
        // - ホストポート 0: 自動割当 (build_config の事前割当) 後は到達不能な防御
        //   (デーモン応答の異常系のみ)。
        // - コンテナポート 0: そのまま登録すると Ports の最小キーになり、
        //   ポート未指定フォールバックが接続を試みてしまうため必須。
        // この防御は macOS 側のみ。Linux の Docker Engine は正常応答で 0 を返さない
        // ため実害がない (構造上は first_container_port のフォールバックで同問題を持ち得る)。
        if host_port == 0 || container_port.as_u16() == 0 {
            tracing::debug!(
                "skipping published port with zero host_port or container_port: \
                 host_port={host_port}, container_port={container_port}"
            );
            continue;
        }
        // hostAddress のアドレスファミリで IPv4 / IPv6 マッピングを分ける。
        // (以前は全部 IPv4 側に入り、ipv6_mapping は永遠に空だった)
        if host_address.parse::<std::net::Ipv6Addr>().is_ok() {
            ports.add_ipv6_mapping(container_port, host_port);
        } else {
            ports.add_mapping(container_port, host_port);
        }
    }
    ports
}

// ── DisplayJson 型 ──

struct StopOptions {
    signal: String,
    timeout: u64,
}
impl DisplayJson for StopOptions {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("signal", &self.signal)?;
            f.member("timeoutInSeconds", self.timeout)
        })
    }
}

struct CreateOpts {
    auto_remove: bool,
}
impl DisplayJson for CreateOpts {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("autoRemove", self.auto_remove)?;
            f.member("rootFsOverride", &Option::<String>::None)
        })
    }
}

struct Platform {
    os: String,
    architecture: String,
}
impl DisplayJson for Platform {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("os", &self.os)?;
            f.member("architecture", &self.architecture)
        })
    }
}

/// `ContainerizationOCI.Platform` 相当の JSON バイト列を組み立てる。
///
/// `imagePull` の `ociPlatform` と `getDefaultKernel` の `systemPlatform` で共用する。
fn oci_platform_json(architecture: &str) -> Vec<u8> {
    j(&Platform {
        os: "linux".into(),
        architecture: architecture.into(),
    })
}

struct ProcCfg {
    executable: String,
    arguments: Vec<String>,
    /// `KEY=VALUE` 形式の環境変数。呼び出し元から渡された値をそのまま送る。
    environment: Vec<String>,
    working_directory: String,
}
impl DisplayJson for ProcCfg {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("executable", &self.executable)?;
            f.member("arguments", &self.arguments)?;
            f.member("environment", &self.environment)?;
            f.member("workingDirectory", &self.working_directory)?;
            // 非インタラクティブ exec 前提。
            f.member("terminal", false)?;
            // exec 経路では user 上書きせず root 固定（`with_user` は init 側）。
            f.member("user", &UserId { uid: 0, gid: 0 })?;
            // 常に空配列を送る。
            f.member("supplementalGroups", Vec::<u32>::new())?;
            f.member("rlimits", Vec::<u32>::new())
        })
    }
}

struct UserId {
    uid: u32,
    gid: u32,
}
impl DisplayJson for UserId {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member(
                "id",
                &InnerId {
                    uid: self.uid,
                    gid: self.gid,
                },
            )
        })
    }
}

struct InnerId {
    uid: u32,
    gid: u32,
}
impl DisplayJson for InnerId {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("uid", self.uid)?;
            f.member("gid", self.gid)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tag_with_dot_still_gets_library_prefix() {
        // タグに `.` を含んでも非修飾参照は docker.io/library/ が前置されること (回帰)。
        assert_eq!(
            normalize_image_reference("alpine:3.19"),
            "docker.io/library/alpine:3.19"
        );
    }

    #[test]
    fn normalize_image_reference_is_idempotent_for_samples() {
        // 代表参照で正規化が冪等であること。
        for image in [
            "alpine:latest",
            "user/repo:1",
            "ghcr.io/org/app:tag",
            "localhost/foo",
            "registry.example.com:5000/ns/name:v1",
        ] {
            let once = normalize_image_reference(image);
            let twice = normalize_image_reference(&once);
            assert_eq!(once, twice, "冪等であること: {image}");
        }
    }

    #[test]
    fn normalize_unqualified_gets_library_prefix() {
        // `/` を含まない参照には docker.io/library/ が前置されること。
        let normalized = normalize_image_reference("nginx:1.25");
        assert!(
            normalized.starts_with("docker.io/library/"),
            "非修飾参照は docker.io/library/ で始まること: {normalized}"
        );
        assert!(
            normalized.ends_with("nginx:1.25"),
            "元の参照が末尾に残ること: {normalized}"
        );
    }

    #[test]
    fn xpc_timeout_for_grace_keeps_default_for_short_grace() {
        // グレースが短い (30 秒以下) 場合は DEFAULT_TIMEOUT (60 秒) のままになること。
        assert_eq!(
            xpc_timeout_for_grace(0),
            crate::xpc::DEFAULT_TIMEOUT,
            "グレース 0 秒は 60 秒のまま"
        );
        assert_eq!(
            xpc_timeout_for_grace(30),
            crate::xpc::DEFAULT_TIMEOUT,
            "グレース 30 秒は 60 秒のまま (30 + 30 = 60)"
        );
    }

    #[test]
    fn xpc_timeout_for_grace_extends_over_default() {
        // グレースが 60 秒を超える場合は「グレース + 30 秒」になること。
        assert_eq!(xpc_timeout_for_grace(61), Duration::from_secs(61 + 30));
        assert_eq!(xpc_timeout_for_grace(120), Duration::from_secs(120 + 30));
    }

    #[test]
    fn xpc_timeout_for_grace_saturates_at_long_timeout() {
        // グレース + 30 秒が LONG_TIMEOUT を超える場合は 24 時間で飽和すること。
        // 負値のグレース相当 (i32::MAX) も同じ。
        assert_eq!(
            xpc_timeout_for_grace(i32::MAX as u64),
            crate::xpc::LONG_TIMEOUT
        );
        assert_eq!(
            xpc_timeout_for_grace(crate::xpc::LONG_TIMEOUT.as_secs() - 20),
            crate::xpc::LONG_TIMEOUT,
            "グレース + 30 秒が 24 時間を超える場合は飽和"
        );
        assert_eq!(
            xpc_timeout_for_grace(crate::xpc::LONG_TIMEOUT.as_secs() - 30),
            crate::xpc::LONG_TIMEOUT,
            "ちょうど 24 時間になる場合は飽和値"
        );
        assert_eq!(
            xpc_timeout_for_grace(crate::xpc::LONG_TIMEOUT.as_secs() - 31),
            Duration::from_secs(crate::xpc::LONG_TIMEOUT.as_secs() - 1),
            "グレース + 30 秒が 24 時間未満の場合は飽和しない"
        );
    }

    #[test]
    fn xpc_timeout_for_grace_never_overflows() {
        // u64::MAX を渡してもオーバーフローせず 24 時間で飽和すること。
        assert_eq!(xpc_timeout_for_grace(u64::MAX), crate::xpc::LONG_TIMEOUT);
    }

    #[test]
    fn is_valid_volume_name_accepts_allowed_chars() {
        // 英数字始まり・英数字と _ . - を含む名前は有効であること。
        for name in ["data", "Data-1", "a_b.c-d", "0", "a".repeat(255).as_str()] {
            assert!(is_valid_volume_name(name), "有効な名前であること: {name}");
        }
    }

    #[test]
    fn is_valid_volume_name_rejects_invalid_names() {
        // 空・英数字以外の先頭・256 文字超・禁止記号は無効であること。
        for name in [
            "",
            "-data",
            ".data",
            "_data",
            "da ta",
            "a/b",
            "a#b",
            "a".repeat(256).as_str(),
        ] {
            assert!(!is_valid_volume_name(name), "無効な名前であること: {name}");
        }
    }

    #[test]
    fn is_volume_already_exists_error_matches_existing_volume_message() {
        // 既存ボリュームへの volumeCreate が返す XPC エラーで true になること。
        let err = crate::Error::Client(ClientError::Xpc(
            "XPC error internalError: volume 'data' already exists".into(),
        ));
        assert!(is_volume_already_exists_error(&err));
    }

    #[test]
    fn is_volume_already_exists_error_rejects_other_errors() {
        // already exists を含まない XPC エラーと、Xpc 以外のエラーでは false になること。
        let xpc_err = crate::Error::Client(ClientError::Xpc(
            "XPC error internalError: storage error".into(),
        ));
        assert!(!is_volume_already_exists_error(&xpc_err));

        let other_err = crate::Error::Client(ClientError::ContainerNotFound("id".into()));
        assert!(!is_volume_already_exists_error(&other_err));
    }

    #[test]
    fn parse_volume_configuration_extracts_source_and_format() {
        // VolumeConfiguration JSON から source / format を取り出せること。
        let json = br#"{
            "name": "data",
            "driver": "local",
            "format": "ext4",
            "source": "/host/volumes/data/volume.img"
        }"#;
        let resolution = parse_volume_configuration(json).expect("パースに成功すること");
        assert_eq!(resolution.source, "/host/volumes/data/volume.img");
        assert_eq!(resolution.format, "ext4");
    }

    #[test]
    fn parse_volume_configuration_rejects_missing_source() {
        // source が無い JSON は Json エラーになること。
        let json = br#"{"name":"data","format":"ext4"}"#;
        let err = parse_volume_configuration(json).expect_err("source 欠落はエラーであること");
        assert!(matches!(err, crate::Error::Client(ClientError::Json(_))));
    }

    #[test]
    fn parse_volume_configuration_rejects_missing_format() {
        // format が無い JSON は Json エラーになること。
        let json = br#"{"name":"data","source":"/host/volumes/data/volume.img"}"#;
        let err = parse_volume_configuration(json).expect_err("format 欠落はエラーであること");
        assert!(matches!(err, crate::Error::Client(ClientError::Json(_))));
    }

    #[test]
    fn parse_volume_configuration_rejects_invalid_json() {
        // 不正 JSON は Json エラーになること。
        let err =
            parse_volume_configuration(b"not-json").expect_err("不正 JSON はエラーであること");
        assert!(matches!(err, crate::Error::Client(ClientError::Json(_))));
    }

    #[test]
    fn parse_volume_configuration_rejects_non_utf8() {
        // 非 UTF-8 バイト列は Json エラーになること。
        let err =
            parse_volume_configuration(&[0xff, 0xfe]).expect_err("非 UTF-8 はエラーであること");
        assert!(matches!(err, crate::Error::Client(ClientError::Json(_))));
    }

    #[test]
    fn parse_published_ports_splits_ipv4_and_ipv6_by_host_address() {
        // hostAddress のアドレスファミリで IPv4 / IPv6 マッピングが分かれること。
        // (旧実装は全部 IPv4 側に入り、ipv6_mapping が永遠に空だった)
        let json = r#"{"configuration":{"publishedPorts":[
            {"hostAddress":"0.0.0.0","hostPort":18080,"containerPort":80,"proto":"tcp"},
            {"hostAddress":"::","hostPort":18081,"containerPort":81,"proto":"tcp"}
        ]}}"#;
        let parsed = nojson::RawJson::parse(json).expect("テスト用 JSON の解析に成功すること");
        let ports = parse_published_ports(&parsed.value());
        assert_eq!(ports.map_to_host_port_ipv4(80u16), Some(18080));
        assert_eq!(ports.map_to_host_port_ipv6(80u16), None);
        assert_eq!(ports.map_to_host_port_ipv6(81u16), Some(18081));
        assert_eq!(ports.map_to_host_port_ipv4(81u16), None);
    }

    #[test]
    fn parse_published_ports_skips_zero_ports() {
        // ホストポート 0 / コンテナポート 0 のエントリは Ports に登録されないこと。
        // - ホストポート 0: 自動割当後は到達不能な防御 (デーモン応答の異常系のみ)。
        // - コンテナポート 0: そのまま登録すると Ports の最小キーになり、
        //   ポート未指定フォールバックが接続を試みてしまうため必須。
        let json = r#"{"configuration":{"publishedPorts":[
            {"hostAddress":"0.0.0.0","hostPort":0,"containerPort":80,"proto":"tcp"},
            {"hostAddress":"0.0.0.0","hostPort":18081,"containerPort":0,"proto":"tcp"},
            {"hostAddress":"0.0.0.0","hostPort":18082,"containerPort":82,"proto":"tcp"}
        ]}}"#;
        let parsed = nojson::RawJson::parse(json).expect("テスト用 JSON の解析に成功すること");
        let ports = parse_published_ports(&parsed.value());
        assert_eq!(ports.map_to_host_port_ipv4(80u16), None);
        assert_eq!(ports.map_to_host_port_ipv4(0u16), None);
        assert_eq!(ports.map_to_host_port_ipv4(82u16), Some(18082));
    }

    #[test]
    fn oci_platform_json_contains_os_and_architecture() {
        // imagePull の ociPlatform / getDefaultKernel の systemPlatform に載せる JSON。
        let amd64 = String::from_utf8(oci_platform_json("amd64")).expect("有効な UTF-8 であること");
        assert_eq!(amd64, r#"{"os":"linux","architecture":"amd64"}"#);
        let arm64 = String::from_utf8(oci_platform_json("arm64")).expect("有効な UTF-8 であること");
        assert_eq!(arm64, r#"{"os":"linux","architecture":"arm64"}"#);
    }

    #[test]
    fn match_image_descriptor_rejects_invalid_json() {
        // 不正 JSON は ImageNotFound ではなく Json になること。
        let err = match_image_descriptor(b"not-json", "alpine:latest").unwrap_err();
        assert!(
            matches!(err, ClientError::Json(_)),
            "不正 JSON は ClientError::Json であること: {err:?}"
        );
    }

    #[test]
    fn match_image_descriptor_rejects_non_array_json() {
        // JSON だが配列でない場合も Json になること。
        let err = match_image_descriptor(b"{}", "alpine:latest").unwrap_err();
        assert!(
            matches!(err, ClientError::Json(_)),
            "非配列 JSON は ClientError::Json であること: {err:?}"
        );
    }

    #[test]
    fn match_image_descriptor_empty_or_empty_array_is_not_found() {
        // 空バイト列と空配列はイメージ無しとして ImageNotFound になること。
        assert!(matches!(
            match_image_descriptor(b"", "alpine:latest"),
            Err(ClientError::ImageNotFound(_))
        ));
        assert!(matches!(
            match_image_descriptor(b"[]", "alpine:latest"),
            Err(ClientError::ImageNotFound(_))
        ));
    }

    #[test]
    fn match_image_descriptor_finds_normalized_reference() {
        // 短縮参照 alpine:latest が正規化後の reference と一致し descriptor を返すこと。
        let json = br#"[{"reference":"docker.io/library/alpine:latest","descriptor":{"mediaType":"application/vnd.oci.image.index.v1+json","digest":"sha256:dead","size":1}}]"#;
        let desc = match_image_descriptor(json, "alpine:latest").expect("一致するはず");
        assert!(
            desc.contains("sha256:dead"),
            "descriptor の digest が含まれること: {desc}"
        );
    }

    #[test]
    fn match_image_descriptor_missing_descriptor_is_other() {
        // 参照は一致するが descriptor 欠落なら Other で止まり、後続エントリを探さないこと。
        let json = br#"[
            {"reference":"docker.io/library/alpine:latest"},
            {"reference":"docker.io/library/alpine:latest","descriptor":{"mediaType":"application/vnd.oci.image.index.v1+json","digest":"sha256:later","size":1}}
        ]"#;
        let err = match_image_descriptor(json, "alpine:latest").unwrap_err();
        match err {
            ClientError::Other(msg) => {
                assert!(
                    msg.contains("descriptor is missing"),
                    "Other に欠落理由が含まれること: {msg}"
                );
            }
            other => panic!("ClientError::Other を期待したが {other:?}"),
        }
    }

    #[test]
    fn match_image_descriptor_rejects_non_utf8() {
        // 非 UTF-8 バイト列は Json になること。
        let err = match_image_descriptor(&[0xff, 0xfe], "alpine:latest").unwrap_err();
        assert!(
            matches!(err, ClientError::Json(_)),
            "非 UTF-8 は ClientError::Json であること: {err:?}"
        );
        assert!(
            err.to_string().contains("not UTF-8"),
            "UTF-8 失敗であることが分かること: {err}"
        );
    }

    #[test]
    fn parse_container_list_rejects_non_utf8() {
        // 非 UTF-8 の containerList 応答は ClientError::Json で UTF-8 失敗を明示すること。
        let err = parse_container_list(&[0xff, 0xfe], "test-id").unwrap_err();
        assert!(
            matches!(err, ClientError::Json(_)),
            "非 UTF-8 は ClientError::Json であること: {err:?}"
        );
        assert!(
            err.to_string()
                .contains("containerList response is not UTF-8"),
            "containerList 応答の UTF-8 失敗であることが分かること: {err}"
        );
    }

    #[test]
    fn parse_container_list_returns_container_not_found_for_empty_data() {
        // 空データはコンテナ不存在として扱うこと (従来挙動の維持)。
        let err = parse_container_list(&[], "test-id").unwrap_err();
        assert!(matches!(err, ClientError::ContainerNotFound(_)));
    }

    #[test]
    fn parse_container_list_rejects_invalid_json() {
        // UTF-8 だが JSON 構文が壊れている場合は Json になること。
        // 非 UTF-8 分岐 (文言に "is not UTF-8" を含む) とは文言で区別される。
        let err = parse_container_list(b"this is not json", "test-id").unwrap_err();
        assert!(matches!(err, ClientError::Json(_)));
        assert!(
            !err.to_string().contains("is not UTF-8"),
            "非 UTF-8 ではなく JSON 破損として報告されること: {err}"
        );
    }

    #[test]
    fn host_path_for_xpc_accepts_utf8() {
        // 通常の UTF-8 パスはそのまま通ること。
        let s = host_path_for_xpc(std::path::Path::new("/tmp/hello")).expect("成功すること");
        assert_eq!(s, "/tmp/hello");
    }

    #[test]
    fn host_path_for_xpc_rejects_non_utf8() {
        // 非 UTF-8 バイトを含むパスは Err になること (to_string_lossy 置換はしない)。
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let path = std::path::Path::new(OsStr::from_bytes(b"/tmp/\xff\xfe"));
        let err = host_path_for_xpc(path).expect_err("非 UTF-8 はエラーであること");
        let msg = err.to_string();
        assert!(
            msg.contains("not valid UTF-8"),
            "UTF-8 失敗であることが分かること: {msg}"
        );
    }

    #[test]
    fn create_pipe_sets_fd_cloexec() {
        // pipe 作成直後の両端に FD_CLOEXEC が立っていること。
        use std::os::fd::AsRawFd;
        let (r, w) = create_pipe().expect("pipe を作成できること");
        for (name, fd) in [("read", r.as_raw_fd()), ("write", w.as_raw_fd())] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            assert!(flags >= 0, "{name} の F_GETFD が成功すること");
            assert!(
                flags & libc::FD_CLOEXEC != 0,
                "{name} に FD_CLOEXEC が立っていること: flags={flags}"
            );
        }
    }

    #[test]
    fn cancellable_read_reads_small_file() {
        // 小さなファイルはそのまま読み出せること。
        use std::sync::atomic::AtomicBool;

        let dir = std::env::temp_dir().join(format!(
            "container-rs-read-file-test-{}",
            crate::core::util::unique_suffix()
        ));
        std::fs::create_dir(&dir).expect("一時ディレクトリの作成に失敗した");
        let path = dir.join("small.txt");
        std::fs::write(&path, b"hello").expect("ファイルの書き込みに失敗した");
        let f = std::fs::File::open(&path).expect("ファイルを開けること");
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let result = read_file_to_vec_cancellable(f, cancel)
            .expect("読み出しに成功すること")
            .expect("キャンセルされていないため Some になること");
        assert_eq!(result, b"hello", "ファイルの内容が一致すること");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancellable_read_rejects_over_limit() {
        // 64 MiB を超えるファイルはエラーになること (OOM 防止)。
        // 実際の 64 MiB ファイルは大きすぎるため、set_len でスパースファイルを作る。
        use std::sync::atomic::AtomicBool;

        let dir = std::env::temp_dir().join(format!(
            "container-rs-read-file-limit-test-{}",
            crate::core::util::unique_suffix()
        ));
        std::fs::create_dir(&dir).expect("一時ディレクトリの作成に失敗した");
        let path = dir.join("large.bin");
        // スパースファイル: 64 MiB + 1 バイト。
        let size = 64 * 1024 * 1024 + 1;
        {
            let f = std::fs::File::create(&path).expect("ファイルの作成に失敗した");
            f.set_len(size).expect("set_len に失敗した");
        }

        let f = std::fs::File::open(&path).expect("ファイルを開けること");
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let result = read_file_to_vec_cancellable(f, cancel);
        assert!(result.is_err(), "64 MiB 超過はエラーであること");
        let err = result.unwrap_err();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::OutOfMemory,
            "エラー種別が OutOfMemory であること: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancellable_read_accepts_exactly_at_limit() {
        // ちょうど 64 MiB のファイルは成功すること (境界値: > であり >= ではない)。
        use std::sync::atomic::AtomicBool;

        let dir = std::env::temp_dir().join(format!(
            "container-rs-read-file-boundary-test-{}",
            crate::core::util::unique_suffix()
        ));
        std::fs::create_dir(&dir).expect("一時ディレクトリの作成に失敗した");
        let path = dir.join("exact.bin");
        // スパースファイル: ちょうど 64 MiB。
        let size = 64 * 1024 * 1024;
        {
            let f = std::fs::File::create(&path).expect("ファイルの作成に失敗した");
            f.set_len(size).expect("set_len に失敗した");
        }

        let f = std::fs::File::open(&path).expect("ファイルを開けること");
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let result = read_file_to_vec_cancellable(f, cancel);
        assert!(
            result.is_ok(),
            "ちょうど 64 MiB は成功すること: {:?}",
            result.err()
        );
        assert_eq!(
            result
                .expect("ちょうど 64 MiB は読み出せること")
                .expect("キャンセルされていないため Some になること")
                .len(),
            size as usize,
            "読み込みバイト数が 64 MiB であること"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// キャンセルフラグを立てると、読み取りスレッドが有限時間内に終了し `Ok(None)`
    /// を返すこと (FD リーク防止)。
    ///
    /// 書き込み端を開いたまま (EOF を出さない) にして読み取りスレッドを起動し、
    /// フラグを立ててから終了を待つ。フラグが無ければ、読み取りスレッドは EOF が
    /// 来るまでブロックし続けるため、ここで検証できる。
    /// `recv_timeout` で包むことで、キャンセル機構が回帰した場合もテスト自体が
    /// ハングしない。
    #[test]
    fn cancellable_read_cancel_finishes_before_limit() {
        use std::sync::atomic::AtomicBool;
        use std::sync::atomic::Ordering;

        let (out_read, out_write) = super::create_pipe().expect("pipe の作成に成功すること");
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();

        let handle =
            std::thread::spawn(move || read_file_to_vec_cancellable(out_read, thread_cancel));

        // フラグを立てる前に少し待ち、読み取りスレッドが poll で待機中であることを
        // 確認してからキャンセルする。
        std::thread::sleep(std::time::Duration::from_millis(100));
        cancel.store(true, Ordering::Relaxed);

        // フラグ観測の最悪遅延は poll タイムアウト (100ms) 程度のため、
        // 2 秒以内に join が返ることを検証する。
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(handle.join());
        });
        let joined = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("キャンセル後の読み取りスレッドが 2 秒以内に終了すること");

        let result = joined
            .expect("読み取りスレッドが panic しないこと")
            .expect("read がエラーにならないこと");
        assert_eq!(result, None, "キャンセル時は None が返ること");

        drop(out_write);
    }

    /// フラグを立てない正常系では EOF まで読み切って `Ok(Some(bytes))` を返すこと。
    ///
    /// 時間による打ち切りは行わないため、5 秒超かかる exec の出力も失われない。
    /// ここでは書き込み端を閉じる (EOF) ことで即終了する。
    #[test]
    fn cancellable_read_reads_to_eof_without_cancel() {
        use std::io::Write;
        use std::sync::atomic::AtomicBool;

        let (out_read, mut out_write) = super::create_pipe().expect("pipe の作成に成功すること");
        let cancel = std::sync::Arc::new(AtomicBool::new(false));

        let handle = std::thread::spawn(move || read_file_to_vec_cancellable(out_read, cancel));

        // 書き込み端を閉じる (EOF) と、読み取りスレッドは読み切って返る。
        out_write
            .write_all(b"hello")
            .expect("書き込みに成功すること");
        drop(out_write);

        let result = handle
            .join()
            .expect("読み取りスレッドが panic しないこと")
            .expect("read がエラーにならないこと");
        assert_eq!(result, Some(b"hello".to_vec()), "EOF まで読み切ること");
    }

    /// mpsc 経由で両方のストリームが期限内に届いた場合、`(stdout, stderr)` として
    /// 正しく組み立てられて返ること。
    #[test]
    fn join_exec_readers_succeeds_when_both_streams_deliver_in_time() {
        use std::sync::atomic::AtomicBool;

        let (tx, rx) = std::sync::mpsc::channel::<ExecReaderMsg>();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        // 送信順が stderr → stdout の場合でも、stdout / stderr の組み立てが
        // 正しく行われることを確認する (mpsc は FIFO なので、この順序でも正しく
        // enum バリアントで振り分けられれば結果に反映される)。
        tx.send(ExecReaderMsg::Stderr(Ok(Some(b"err-body".to_vec()))))
            .expect("stderr 送信に成功すること");
        tx.send(ExecReaderMsg::Stdout(Ok(Some(b"out-body".to_vec()))))
            .expect("stdout 送信に成功すること");
        drop(tx);

        let (stdout, stderr) = join_exec_readers(&rx, &cancel, Duration::from_secs(1))
            .expect("両方のストリームが届いていれば成功すること");
        assert_eq!(stdout, b"out-body");
        assert_eq!(stderr, b"err-body");
        assert!(
            !cancel.load(std::sync::atomic::Ordering::Relaxed),
            "正常系ではキャンセルフラグは立たないこと"
        );
    }

    /// どちらのストリームも届かない状態で待ち上限を超過した場合、キャンセルフラグが
    /// 立てられ、エラーメッセージに stdout / stderr の両方が含まれること。
    #[test]
    fn join_exec_readers_times_out_when_both_streams_missing() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (tx, rx) = std::sync::mpsc::channel::<ExecReaderMsg>();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));

        // 上限超過後にスレッド側 (実際は無い) がフラグを見て終了する挙動を模す。
        // ここではフラグを見てチャネルを閉じるだけ (実スレッド無しでロジックを検証)。
        let bg_cancel = cancel.clone();
        std::thread::spawn(move || {
            while !bg_cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(tx);
        });

        let err = join_exec_readers(&rx, &cancel, Duration::from_millis(200))
            .expect_err("上限超過はエラーであること");
        let msg = err.to_string();
        assert!(
            msg.contains("stdout/stderr"),
            "両方のストリームが未受信であることが分かること: {msg}"
        );
        assert!(
            msg.contains("timed out"),
            "タイムアウトを示すメッセージであること: {msg}"
        );
        assert!(
            cancel.load(Ordering::Relaxed),
            "タイムアウト後にキャンセルフラグが立てられること"
        );
    }

    /// stdout だけ届いて stderr が上限内に届かなかった場合、エラーメッセージに
    /// `stderr` が含まれ、`stdout` は含まれないこと (どのストリームが応答しなかった
    /// かを識別できる)。
    #[test]
    fn join_exec_readers_times_out_reports_which_stream_missing() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (tx, rx) = std::sync::mpsc::channel::<ExecReaderMsg>();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));

        tx.send(ExecReaderMsg::Stdout(Ok(Some(b"partial".to_vec()))))
            .expect("stdout 送信に成功すること");

        let bg_cancel = cancel.clone();
        std::thread::spawn(move || {
            while !bg_cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(tx);
        });

        let err = join_exec_readers(&rx, &cancel, Duration::from_millis(200))
            .expect_err("上限超過はエラーであること");
        let msg = err.to_string();
        assert!(
            msg.contains("read stderr timed out"),
            "stderr がタイムアウトしたことが分かること: {msg}"
        );
        assert!(
            !msg.contains("stdout"),
            "届いた側のストリーム名 (stdout) は含めないこと: {msg}"
        );
    }

    /// スレッドが結果を送らずに全て終了した (Disconnected) 場合はエラーになること
    /// (通常は起こらないが、panic 対策として)。両方のストリームが未受信であることが
    /// エラーメッセージから分かること。
    #[test]
    fn join_exec_readers_returns_error_on_disconnect() {
        use std::sync::atomic::AtomicBool;

        let (tx, rx) = std::sync::mpsc::channel::<ExecReaderMsg>();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        drop(tx);

        let err = join_exec_readers(&rx, &cancel, Duration::from_secs(1))
            .expect_err("Disconnected はエラーであること");
        let msg = err.to_string();
        assert!(
            msg.contains("terminated without sending"),
            "Disconnected のエラーメッセージであること: {msg}"
        );
        assert!(
            msg.contains("stdout/stderr"),
            "両方のストリームが未受信であることがメッセージから分かること: {msg}"
        );
    }

    /// stdout だけ届いた状態で残りの送信端が drop される (もう片方のスレッドが結果を
    /// 送らずに終了した) と、`stderr` が未受信であることがエラーメッセージから
    /// 分かること。届いた側 (stdout) は「未受信」として報告しないこと。
    #[test]
    fn join_exec_readers_disconnect_reports_missing_stream() {
        use std::sync::atomic::AtomicBool;

        let (tx, rx) = std::sync::mpsc::channel::<ExecReaderMsg>();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        tx.send(ExecReaderMsg::Stdout(Ok(Some(b"partial".to_vec()))))
            .expect("stdout 送信に成功すること");
        drop(tx);

        let err = join_exec_readers(&rx, &cancel, Duration::from_secs(1))
            .expect_err("Disconnected はエラーであること");
        let msg = err.to_string();
        assert!(
            msg.contains("stderr"),
            "stderr が未受信であることが分かること: {msg}"
        );
        assert!(
            !msg.contains("stdout"),
            "届いた側 (stdout) の名前は未受信対象として現れないこと: {msg}"
        );
        assert!(
            msg.contains("terminated without sending"),
            "Disconnected のエラー種別であることが分かること: {msg}"
        );
    }

    /// 読み取りがエラーで終わった場合はそのエラーが伝播すること。
    #[test]
    fn join_exec_readers_returns_read_error() {
        use std::sync::atomic::AtomicBool;

        let (tx, rx) = std::sync::mpsc::channel::<ExecReaderMsg>();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        tx.send(ExecReaderMsg::Stdout(Err(std::io::Error::other(
            "broken pipe",
        ))))
        .expect("stdout エラー送信に成功すること");
        tx.send(ExecReaderMsg::Stderr(Ok(Some(Vec::new()))))
            .expect("stderr 送信に成功すること");
        drop(tx);

        let err = join_exec_readers(&rx, &cancel, Duration::from_secs(1))
            .expect_err("読み取りエラーは伝播すること");
        assert!(
            err.to_string().contains("read stdout failed"),
            "stdout の読み取り失敗であることが分かること: {err}"
        );
    }

    /// 実際の読み取りスレッドを 2 本 pipe に対して起動し、書き込み端を閉じない (EOF が
    /// 来ない) 異常系を再現する。`join_exec_readers` の上限内にキャンセルフラグを立てて
    /// スレッドを回収し、エラーを返すこと (統合的な検証)。
    #[test]
    fn join_exec_readers_reclaims_reader_threads_on_timeout() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (out_read, out_write) = super::create_pipe().expect("pipe の作成に成功すること");
        let (err_read, err_write) = super::create_pipe().expect("pipe の作成に成功すること");
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel::<ExecReaderMsg>();
        let out_cancel = cancel.clone();
        let err_cancel = cancel.clone();
        let out_tx = tx.clone();
        let err_tx = tx;
        let out_thread = std::thread::spawn(move || {
            let r = read_file_to_vec_cancellable(out_read, out_cancel);
            let _ = out_tx.send(ExecReaderMsg::Stdout(r));
        });
        let err_thread = std::thread::spawn(move || {
            let r = read_file_to_vec_cancellable(err_read, err_cancel);
            let _ = err_tx.send(ExecReaderMsg::Stderr(r));
        });

        // 書き込み端は開いたまま (EOF が来ない異常系)。300ms の上限で打ち切りを検証する。
        let err = join_exec_readers(&rx, &cancel, Duration::from_millis(300))
            .expect_err("EOF が来ない場合は上限で打ち切りエラーになること");
        assert!(
            err.to_string().contains("timed out"),
            "タイムアウトのエラーメッセージであること: {err}"
        );
        assert!(
            cancel.load(Ordering::Relaxed),
            "打ち切り時にキャンセルフラグが立てられること"
        );

        // スレッドが FD を握り続けないこと (poll 間隔 100ms + read 1 回以内に終了する
        // ため、余裕を持って 2 秒以内に join できること)。
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send((out_thread.join(), err_thread.join()));
        });
        let (out_join, err_join) = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("打ち切り後 2 秒以内に両方の読み取りスレッドが終了すること");
        out_join.expect("stdout スレッドが panic しないこと");
        err_join.expect("stderr スレッドが panic しないこと");

        // pipe の書き込み端は最後まで開いたままにしていたことを確認 (異常系の再現条件)。
        drop(out_write);
        drop(err_write);
    }

    /// キャンセルフラグを立てず、EOF も来ない状態が 5 秒を超えても、正常系の
    /// 読み取りは打ち切られず続くこと (時間打ち切りが無いことの検証)。
    ///
    /// 5 秒超かかる exec でも stdout / stderr の出力が失われないことを保証する。
    /// フラグを立てずに 5 秒超待ってから書き込むことで、打ち切られずに読み取れる
    /// ことを検証する。
    #[test]
    fn cancellable_read_does_not_timeout_without_cancel() {
        use std::io::Write;
        use std::sync::atomic::AtomicBool;

        let (out_read, mut out_write) = super::create_pipe().expect("pipe の作成に成功すること");
        let cancel = std::sync::Arc::new(AtomicBool::new(false));

        let handle = std::thread::spawn(move || read_file_to_vec_cancellable(out_read, cancel));

        // 5 秒を超えて待ち、その後書き込んで EOF にする。時間打ち切りが無ければ
        // 書き込んだ内容が読める。
        std::thread::sleep(std::time::Duration::from_millis(5100));
        out_write
            .write_all(b"late")
            .expect("書き込みに成功すること");
        drop(out_write);

        let result = handle
            .join()
            .expect("読み取りスレッドが panic しないこと")
            .expect("read がエラーにならないこと");
        assert_eq!(
            result,
            Some(b"late".to_vec()),
            "5 秒超の exec でも読み取りが打ち切られないこと"
        );
    }
}
