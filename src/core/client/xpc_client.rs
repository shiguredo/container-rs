//! `XpcClient` — Apple Container の XPC API を叩くクライアント。
//!
//! 元の crate の `Client`（bollard ラッパ）に相当する。
//! `ContainerAsync` / `AsyncRunner` / `WaitStrategy` から利用される。
//!
//! macOS バックエンドが必要とする XPC 操作を実装する。
//! `pause` / `unpause` に相当する XPC 操作は未実装。

use std::collections::HashMap;
use std::io::Read;
use std::net::IpAddr;
use std::os::fd::FromRawFd;

use nojson::DisplayJson;

use crate::core::{
    client::ContainerSnapshot,
    error::{ClientError, Result},
    ports::{ContainerPort, Ports},
};
use crate::xpc::{self, IMAGE_SERVICE, KeyValue, SERVICE_NAME, XpcConn, id_key, j, k, s};

/// XPC クライアント。毎回 `connect()` で新規接続を張る（本家の `Client` と違い接続プール無し）。
pub struct XpcClient;

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

    /// コンテナを停止する。
    ///
    /// `timeout_seconds`:
    /// - `None` → SIGTERM、30 秒タイムアウト（既存 `stop()` の挙動を維持）。
    /// - `Some(0)` → 即時 SIGKILL。
    /// - `Some(t)` (`t < 0`) → 無限待ちに近い長いタイムアウトで SIGTERM。
    ///   Apple container は負のタイムアウトをサポートしないため、`i32::MAX` 秒で代用。
    /// - `Some(t)` (`t > 0`) → SIGTERM、`t` 秒タイムアウト。
    pub(crate) async fn stop(&self, id: &str, timeout_seconds: Option<i32>) -> Result<()> {
        let (signal, timeout) = match timeout_seconds {
            Some(0) => ("SIGKILL".to_string(), 0),
            Some(t) if t < 0 => ("SIGTERM".to_string(), i32::MAX as u64),
            Some(t) => ("SIGTERM".to_string(), t as u64),
            None => ("SIGTERM".to_string(), 30),
        };
        let id = id.to_string();
        let stop_options = j(&StopOptions { signal, timeout });
        tokio::task::spawn_blocking(move || {
            let conn = XpcConn::connect(SERVICE_NAME)?;
            let result = conn.send(
                "containerStop",
                &[
                    (id_key(), s(&id)),
                    (k("stopOptions"), KeyValue::Data(stop_options)),
                ],
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
    ) -> Result<ExecResult> {
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
            let out_handle = std::thread::spawn(move || read_file_to_vec(out_read));
            let err_handle = std::thread::spawn(move || read_file_to_vec(err_read));

            let reply = match conn.send_with_timeout(
                "containerWait",
                &[(id_key(), s(&id)), (k("processIdentifier"), s(&pid))],
                crate::xpc::LONG_TIMEOUT,
            ) {
                Ok(r) => r,
                // wait 失敗時に join するとデーモン側が書き込み端を閉じるまで
                // 戻れない可能性があるため、読み取りスレッドはデタッチする。
                Err(e) => return Err(e),
            };

            // プロセス終了後、デーモンが書き込み端を閉じると EOF になり join が返る。
            let stdout = out_handle
                .join()
                .map_err(|_| ClientError::Other("stdout reader thread panicked".into()))??;
            let stderr = err_handle
                .join()
                .map_err(|_| ClientError::Other("stderr reader thread panicked".into()))??;

            let exit_code = reply.try_int64(&k("exitCode"))?;
            Ok(ExecResult {
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
    if data.is_empty() {
        return Err(ClientError::ContainerNotFound(id.to_string()).into());
    }
    let text = std::str::from_utf8(&data).unwrap_or("");
    let parsed = nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))?;
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

/// `File` を EOF まで読み出す。64 MiB を超える場合はエラーを返す (OOM 防止)。
fn read_file_to_vec(mut f: std::fs::File) -> std::io::Result<Vec<u8>> {
    // 上限なしの read_to_end は大量出力で OOM になり得るため take で制限する。
    const MAX_OUTPUT_SIZE: u64 = 64 * 1024 * 1024;
    let mut buf = Vec::new();
    let mut limited = std::io::Read::take(&mut f, MAX_OUTPUT_SIZE + 1);
    limited.read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_OUTPUT_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::OutOfMemory,
            format!("output exceeds {MAX_OUTPUT_SIZE} bytes limit"),
        ));
    }
    Ok(buf)
}

/// `exec` の結果。
pub(crate) struct ExecResult {
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
    fn read_file_to_vec_reads_small_file() {
        // 小さなファイルはそのまま読み出せること。
        let dir = std::env::temp_dir().join(format!(
            "container-rs-read-file-test-{}",
            crate::core::util::unique_suffix()
        ));
        std::fs::create_dir(&dir).expect("一時ディレクトリの作成に失敗した");
        let path = dir.join("small.txt");
        std::fs::write(&path, b"hello").expect("ファイルの書き込みに失敗した");
        let f = std::fs::File::open(&path).expect("ファイルを開けること");
        let result = read_file_to_vec(f).expect("読み出しに成功すること");
        assert_eq!(result, b"hello", "ファイルの内容が一致すること");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_file_to_vec_rejects_over_limit() {
        // 64 MiB を超えるファイルはエラーになること (OOM 防止)。
        // 実際の 64 MiB ファイルは大きすぎるため、set_len でスパースファイルを作る。
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
        let result = read_file_to_vec(f);
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
    fn read_file_to_vec_accepts_exactly_at_limit() {
        // ちょうど 64 MiB のファイルは成功すること (境界値: > であり >= ではない)。
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
        let result = read_file_to_vec(f);
        assert!(
            result.is_ok(),
            "ちょうど 64 MiB は成功すること: {:?}",
            result.err()
        );
        assert_eq!(
            result.unwrap().len(),
            size as usize,
            "読み込みバイト数が 64 MiB であること"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
