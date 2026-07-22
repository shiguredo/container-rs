//! Ubuntu 用 Docker Engine API クライアント。
//!
//! Docker Engine API は HTTP/1.1 REST API であるため、`shiguredo_http11` でリクエストを送信する。
//! Unix ドメインソケット `/var/run/docker.sock` を介して通信する。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use shiguredo_http11::{Request, Response};

use crate::core::client::{ContainerConfig, ContainerSnapshot};
use crate::core::containers::request::PortMapping;
use crate::core::error::{ClientError, Result};
use crate::core::ports::{ContainerPort, Ports};

const DEFAULT_DOCKER_SOCKET: &str = "/var/run/docker.sock";

/// Docker exec の生結果。stdout / stderr は取得しないため常に空。
pub(crate) struct DockerExecResult {
    pub(crate) exit_code: Option<i64>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// shiguredo_http11 のエラーを `ClientError::Other` に変換する。
fn http11_err(e: impl std::fmt::Display) -> crate::core::error::Error {
    ClientError::Other(e.to_string()).into()
}

/// Docker Engine API クライアント。
#[derive(Debug, Clone)]
pub struct DockerClient {
    socket_path: String,
}

impl DockerClient {
    /// Unix ドメインソケット経由で Docker API に接続するクライアントを返す。
    pub fn detect() -> Result<Self> {
        Ok(Self {
            socket_path: DEFAULT_DOCKER_SOCKET.to_string(),
        })
    }

    /// イメージをプルする。
    pub(crate) async fn pull_image(&self, descriptor: &str) -> Result<()> {
        let (image, tag) = split_pull_reference(descriptor);
        // Docker Engine API は query 値の `/` 等を percent-encode する必要がある。
        let path = format!(
            "/images/create?fromImage={}&tag={}",
            percent_encode_component(image),
            percent_encode_component(tag)
        );
        let response = self.request("POST", &path, None).await?;
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to pull image {descriptor}: {}",
                response.status_code()
            ))
            .into());
        }
        check_pull_stream_errors(response.body_bytes().unwrap_or(&[]))?;
        Ok(())
    }

    /// イメージの descriptor を解決する。
    /// ローカルに存在しなければプルして再試行する。
    pub(crate) async fn resolve_image_descriptor(&self, descriptor: &str) -> Result<String> {
        // path セグメントの `/` を生のまま埋め込むとルートが壊れる
        // (例: `ghcr.io/org/app:tag` → `/images/ghcr.io/org/...`)。
        let path = format!("/images/{}/json", percent_encode_path_segment(descriptor));
        let response = self.request("GET", &path, None).await?;
        if response.status_code() == 200 {
            Ok(descriptor.to_string())
        } else if response.status_code() == 404 {
            self.pull_image(descriptor).await?;
            let response = self.request("GET", &path, None).await?;
            if response.status_code() == 200 {
                Ok(descriptor.to_string())
            } else {
                Err(ClientError::ImageNotFound(descriptor.to_string()).into())
            }
        } else {
            Err(ClientError::Other(format!(
                "failed to resolve image {descriptor}: {}",
                response.status_code()
            ))
            .into())
        }
    }

    /// コンテナを作成する。
    pub(crate) async fn create_container(&self, config: ContainerConfig) -> Result<String> {
        let query = config
            .name
            .as_ref()
            .map(|name| format!("?name={}", percent_encode_component(name)))
            .unwrap_or_default();
        let path = format!("/containers/create{query}");
        let body = CreateContainerBody::from_config(config)?;
        let body_json = body.to_json_string()?;
        let response = self
            .request("POST", &path, Some(body_json.into_bytes()))
            .await?;
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to create container: {}",
                response.status_code()
            ))
            .into());
        }
        let body = response
            .body_bytes()
            .ok_or_else(|| ClientError::Other("empty response body".into()))?;
        let text = std::str::from_utf8(body).map_err(|e| ClientError::Json(e.to_string()))?;
        let parsed = nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))?;
        let id: String = parsed
            .value()
            .to_member("Id")
            .map_err(|e| ClientError::Json(e.to_string()))?
            .required()
            .map_err(|e| ClientError::Json(e.to_string()))?
            .try_into()
            .map_err(|e: nojson::JsonParseError| ClientError::Json(e.to_string()))?;
        Ok(id)
    }

    /// コンテナを起動する。
    pub(crate) async fn start_container(&self, id: &str) -> Result<()> {
        let path = format!("/containers/{}/start", percent_encode_path_segment(id));
        let response = self.request("POST", &path, None).await?;
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to start container: {}",
                response.status_code()
            ))
            .into());
        }
        Ok(())
    }

    /// コンテナを停止する。
    ///
    /// `timeout_seconds`:
    /// - `None` または負値 → `t=30`
    /// - `Some(t)` (`t >= 0`) → `t={t}` (Docker の `t=0` は grace 0)
    ///
    /// コンテナが存在しない (404) ときは冪等に成功とする。
    pub(crate) async fn stop(&self, id: &str, timeout_seconds: Option<i32>) -> Result<()> {
        let t = match timeout_seconds {
            Some(t) if t >= 0 => t,
            _ => 30,
        };
        let path = format!("/containers/{}/stop?t={t}", percent_encode_path_segment(id));
        let response = self.request("POST", &path, None).await?;
        if response.status_code() == 404 {
            return Ok(());
        }
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to stop container: {}",
                response.status_code()
            ))
            .into());
        }
        Ok(())
    }

    /// コンテナを削除する。
    ///
    /// コンテナが存在しない (404) ときは冪等に成功とする。
    pub(crate) async fn remove(&self, id: &str, force: bool) -> Result<()> {
        let path = format!(
            "/containers/{}?force={force}",
            percent_encode_path_segment(id)
        );
        let response = self.request("DELETE", &path, None).await?;
        if response.status_code() == 404 {
            return Ok(());
        }
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to remove container: {}",
                response.status_code()
            ))
            .into());
        }
        Ok(())
    }

    /// `remove` の同期版。Drop などランタイム外から呼ぶ。
    ///
    /// コンテナが存在しない (404) ときは冪等に成功とする。
    pub(crate) fn remove_blocking(&self, id: &str, force: bool) -> Result<()> {
        let path = format!(
            "/containers/{}?force={force}",
            percent_encode_path_segment(id)
        );
        let request_bytes = encode_docker_api_request("DELETE", &path, None)?;
        let mut stream = UnixStream::connect(&self.socket_path)?;
        stream.write_all(&request_bytes)?;
        // 書き込み半閉じは dockerd / Docker Desktop が 500 を返すため行わない。
        // 対向の接続保持は `Connection: close` とボディ完了時の即リターンで防ぐ。
        let response = read_http11_response(&mut stream, "DELETE")?;
        if response.status_code() == 404 {
            return Ok(());
        }
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to remove container: {}",
                response.status_code()
            ))
            .into());
        }
        Ok(())
    }

    /// コンテナ内でコマンドを実行し、終了コードを取得する。
    /// stdout / stderr は取得しない。
    pub(crate) async fn exec(&self, id: &str, cmd: &[String]) -> Result<DockerExecResult> {
        let exec_path = format!("/containers/{}/exec", percent_encode_path_segment(id));
        let exec_config = ExecConfig {
            cmd: cmd.to_vec(),
            // stdout / stderr は読まない。完了待ちは inspect の Running ポーリングで行う。
            attach_stdout: false,
            attach_stderr: false,
        };
        let exec_json = exec_config.to_json_string()?;
        let response = self
            .request("POST", &exec_path, Some(exec_json.into_bytes()))
            .await?;
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to create exec: {}",
                response.status_code()
            ))
            .into());
        }
        let body = response
            .body_bytes()
            .ok_or_else(|| ClientError::Other("empty exec response body".into()))?;
        let text = std::str::from_utf8(body).map_err(|e| ClientError::Json(e.to_string()))?;
        let parsed = nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))?;
        let exec_id: String = parsed
            .value()
            .to_member("Id")
            .map_err(|e| ClientError::Json(e.to_string()))?
            .required()
            .map_err(|e| ClientError::Json(e.to_string()))?
            .try_into()
            .map_err(|e: nojson::JsonParseError| ClientError::Json(e.to_string()))?;

        // Detach=true で即時復帰し、完了は inspect で待つ。
        // Detach=false でも attach 無しだと Engine はプロセス完了を待たず、
        // 直後の inspect で ExitCode がまだ null になり得る。
        let start_path = format!("/exec/{}/start", percent_encode_path_segment(&exec_id));
        let start_config = ExecStartConfig {
            detach: true,
            tty: false,
        };
        let start_json = start_config.to_json_string()?;
        let response = self
            .request("POST", &start_path, Some(start_json.into_bytes()))
            .await?;
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to start exec: {}",
                response.status_code()
            ))
            .into());
        }

        let inspect_path = format!("/exec/{}/json", percent_encode_path_segment(&exec_id));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let response = self.request("GET", &inspect_path, None).await?;
            if response.status_code() >= 400 {
                return Err(ClientError::Other(format!(
                    "failed to inspect exec: {}",
                    response.status_code()
                ))
                .into());
            }
            let body = response
                .body_bytes()
                .ok_or_else(|| ClientError::Other("empty exec inspect body".into()))?;
            let text = std::str::from_utf8(body).map_err(|e| ClientError::Json(e.to_string()))?;
            let parsed =
                nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))?;
            let running = parsed
                .value()
                .to_member("Running")
                .ok()
                .and_then(|m| m.optional())
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false);
            if !running {
                let exit_code = parsed
                    .value()
                    .to_member("ExitCode")
                    .ok()
                    .and_then(|m| m.optional())
                    .and_then(|v| i64::try_from(v).ok());
                return Ok(DockerExecResult {
                    exit_code,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                });
            }
            if std::time::Instant::now() >= deadline {
                return Err(ClientError::Other(format!(
                    "exec {exec_id} did not finish within 30s"
                ))
                .into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// コンテナのポートマッピングを取得する。
    pub(crate) async fn ports(&self, id: &str) -> Result<Ports> {
        let snapshot = self.container_state(id).await?;
        Ok(snapshot.ports)
    }

    /// コンテナの状態を取得する。
    pub(crate) async fn container_state(&self, id: &str) -> Result<ContainerSnapshot> {
        let path = format!("/containers/{}/json", percent_encode_path_segment(id));
        let response = self.request("GET", &path, None).await?;
        if response.status_code() == 404 {
            return Err(ClientError::ContainerNotFound(id.to_string()).into());
        }
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to inspect container: {}",
                response.status_code()
            ))
            .into());
        }
        let body = response
            .body_bytes()
            .ok_or_else(|| ClientError::Other("empty inspect body".into()))?;
        let text = std::str::from_utf8(body).map_err(|e| ClientError::Json(e.to_string()))?;
        let parsed = nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))?;

        let state = parsed
            .value()
            .to_member("State")
            .map_err(|e| ClientError::Json(e.to_string()))?
            .required()
            .map_err(|e| ClientError::Json(e.to_string()))?;
        let running = state
            .to_member("Running")
            .ok()
            .and_then(|m| m.optional())
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false);
        let ports = parse_ports(&parsed);

        Ok(ContainerSnapshot { running, ports })
    }

    /// コンテナのログストリーム (`?follow=true`) を起動し、ハンドルを返す。
    ///
    /// `docker_log_stream::spawn_log_session` をこのクライアントのソケットパスで呼ぶ。
    /// ログの demux / 共有バッファ / LogConsumer 配信は呼び出し側 (`AsyncRunner::start`) が組み立てる。
    pub(crate) async fn spawn_log_session(
        &self,
        id: &str,
    ) -> Result<std::sync::Arc<crate::core::client::docker_log_stream::DockerLogsHandle>> {
        crate::core::client::docker_log_stream::spawn_log_session(
            self.socket_path.clone(),
            id.to_string(),
        )
        .await
    }

    /// Docker Engine API に HTTP リクエストを送信する。
    async fn request(&self, method: &str, path: &str, body: Option<Vec<u8>>) -> Result<Response> {
        let socket_path = self.socket_path.clone();
        let method = method.to_string();
        let path = path.to_string();

        tokio::task::spawn_blocking(move || -> Result<Response> {
            // encode 失敗時にソケットを開かないよう、connect より先にエンコードする。
            let request_bytes = encode_docker_api_request(&method, &path, body.as_deref())?;

            let mut stream = UnixStream::connect(&socket_path)?;
            stream.write_all(&request_bytes)?;
            // 書き込み半閉じは dockerd / Docker Desktop が create / start 等で
            // 500 Internal Server Error を返すため行わない。
            // 対向の接続保持は `Connection: close` とボディ完了時の即リターンで防ぐ。

            read_http11_response(&mut stream, &method)
        })
        .await
        .map_err(|e| ClientError::Other(format!("spawn_blocking failed: {e}")))?
    }
}

/// Docker Engine API 向け HTTP/1.1 リクエストをエンコードする。
///
/// ログストリーム (`docker_log_stream`) でも再利用するため `pub(crate)` で公開する。
pub(crate) fn encode_docker_api_request(
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<Vec<u8>> {
    // Method は動的文字列なので Method::new で構築する。
    let method_obj = shiguredo_http11::Method::new(method).map_err(http11_err)?;
    let mut request = Request::new(method_obj, path)
        .map_err(http11_err)?
        .header("Host", "localhost")
        .map_err(http11_err)?
        // keep-alive で対向が接続を保持しないよう明示的に閉じる。
        .header("Connection", "close")
        .map_err(http11_err)?;
    if let Some(body) = body {
        request = request
            .header("Content-Type", "application/json")
            .map_err(http11_err)?;
        request = request
            .header("Content-Length", &body.len().to_string())
            .map_err(http11_err)?;
        request = request.body(body.to_vec());
    }
    request.encode().map_err(http11_err)
}

/// ソケットから HTTP/1.1 レスポンスを読み取りデコードする。
fn read_http11_response(stream: &mut impl Read, method: &str) -> Result<Response> {
    use crate::core::client::http_decode::ResponseAccumulator;

    let mut acc = ResponseAccumulator::new(method, None);

    loop {
        let want = acc.read_buf_size();
        if want == 0 {
            return Err(ClientError::Other("decoder buffer full".into()).into());
        }
        let buf = acc.mut_buf(want).map_err(ClientError::Other)?;
        let n = stream.read(buf)?;
        if acc.feed(n).map_err(ClientError::Other)? {
            break;
        }
    }

    let decoded = acc.finish().map_err(ClientError::Other)?;
    let mut response = Response::with_version(
        decoded.head.version(),
        decoded.head.status_code(),
        decoded.head.reason_phrase(),
    )
    .map_err(http11_err)?;
    for (name, value) in decoded.head.headers() {
        // ヘッダー名は動的文字列 (非 'static) のため HeaderName::new で構築する。
        let name = shiguredo_http11::HeaderName::new(name.as_str()).map_err(http11_err)?;
        response = response.header(name, value).map_err(http11_err)?;
    }
    response = response.body(decoded.body);
    Ok(response)
}

/// Docker API 用のコンテナ作成ボディ。
struct CreateContainerBody {
    image: String,
    entrypoint: Option<Vec<String>>,
    cmd: Vec<String>,
    env: Vec<String>,
    labels: BTreeMap<String, String>,
    working_dir: Option<String>,
    user: Option<String>,
    host_config: HostConfig,
    exposed_ports: Vec<String>,
}

impl CreateContainerBody {
    fn from_config(config: ContainerConfig) -> Result<Self> {
        let port_bindings = build_port_bindings(&config.ports);
        let exposed_ports = build_exposed_ports(&config.ports);
        let mut binds = Vec::new();
        for m in &config.mounts {
            match m.mount_type() {
                crate::core::mounts::MountType::Bind => {
                    let source = m.source().ok_or_else(|| {
                        ClientError::Configuration("bind mount source is required".into())
                    })?;
                    let target = m.target().ok_or_else(|| {
                        ClientError::Configuration("bind mount target is required".into())
                    })?;
                    binds.push(format!("{source}:{target}:{}", m.access_mode()));
                }
                crate::core::mounts::MountType::Volume => {
                    return Err(ClientError::Configuration(
                        "volume mount is not implemented on Linux".into(),
                    )
                    .into());
                }
                crate::core::mounts::MountType::Tmpfs => {
                    return Err(ClientError::Configuration(
                        "tmpfs mount is not implemented on Linux".into(),
                    )
                    .into());
                }
            }
        }

        Ok(Self {
            image: config.image,
            entrypoint: config.entrypoint,
            cmd: config.cmd,
            env: config.env,
            labels: config.labels,
            working_dir: config.working_dir,
            user: config.user,
            host_config: HostConfig {
                port_bindings,
                binds,
                privileged: config.privileged,
                init: config.init,
            },
            exposed_ports,
        })
    }

    fn to_json_string(&self) -> Result<String> {
        // DisplayJson を使わずに簡易的に JSON を組み立てる。
        let mut json = String::new();
        json.push_str("{\"Image\":");
        json.push_str(&escape_json(&self.image));
        if let Some(entrypoint) = &self.entrypoint
            && !entrypoint.is_empty()
        {
            json.push_str(",\"Entrypoint\":");
            json.push_str(&json_array(entrypoint));
        }
        if !self.cmd.is_empty() {
            json.push_str(",\"Cmd\":");
            json.push_str(&json_array(&self.cmd));
        }
        if !self.env.is_empty() {
            json.push_str(",\"Env\":");
            json.push_str(&json_array(&self.env));
        }
        if !self.labels.is_empty() {
            json.push_str(",\"Labels\":");
            json.push_str(&json_object(&self.labels));
        }
        if let Some(wd) = &self.working_dir {
            json.push_str(",\"WorkingDir\":");
            json.push_str(&escape_json(wd));
        }
        if let Some(user) = &self.user {
            json.push_str(",\"User\":");
            json.push_str(&escape_json(user));
        }
        if !self.exposed_ports.is_empty() {
            json.push_str(",\"ExposedPorts\":");
            json.push('{');
            for (i, key) in self.exposed_ports.iter().enumerate() {
                if i > 0 {
                    json.push(',');
                }
                json.push_str(&escape_json(key));
                json.push_str(":{}");
            }
            json.push('}');
        }
        json.push_str(",\"HostConfig\":");
        json.push_str(&self.host_config.to_json_string()?);
        json.push('}');
        Ok(json)
    }
}

struct HostConfig {
    port_bindings: BTreeMap<String, Vec<PortBinding>>,
    binds: Vec<String>,
    privileged: bool,
    init: bool,
}

impl HostConfig {
    fn to_json_string(&self) -> Result<String> {
        let mut json = String::new();
        json.push_str("{\"Privileged\":");
        json.push_str(if self.privileged { "true" } else { "false" });
        json.push_str(",\"Init\":");
        json.push_str(if self.init { "true" } else { "false" });
        if !self.binds.is_empty() {
            json.push_str(",\"Binds\":");
            json.push_str(&json_array(&self.binds));
        }
        if !self.port_bindings.is_empty() {
            json.push_str(",\"PortBindings\":");
            json.push_str(&json_port_bindings(&self.port_bindings));
        }
        json.push('}');
        Ok(json)
    }
}

struct PortBinding {
    host_ip: String,
    host_port: String,
}

struct ExecConfig {
    cmd: Vec<String>,
    attach_stdout: bool,
    attach_stderr: bool,
}

impl ExecConfig {
    fn to_json_string(&self) -> Result<String> {
        let mut json = String::new();
        json.push_str("{\"AttachStdout\":");
        json.push_str(if self.attach_stdout { "true" } else { "false" });
        json.push_str(",\"AttachStderr\":");
        json.push_str(if self.attach_stderr { "true" } else { "false" });
        if !self.cmd.is_empty() {
            json.push_str(",\"Cmd\":");
            json.push_str(&json_array(&self.cmd));
        }
        json.push('}');
        Ok(json)
    }
}

struct ExecStartConfig {
    detach: bool,
    tty: bool,
}

impl ExecStartConfig {
    fn to_json_string(&self) -> Result<String> {
        Ok(format!(
            "{{\"Detach\":{},\"Tty\":{}}}",
            if self.detach { "true" } else { "false" },
            if self.tty { "true" } else { "false" }
        ))
    }
}

/// Docker Engine API の path セグメント用 percent-encode。
///
/// `/` を含むイメージ参照を `/images/{name}/json` に埋め込むとき、生の `/` は
/// ルート区切りになるため `%2F` に変換する。RFC 3986 unreserved 以外を符号化す
/// る (Go の `PathEscape` に近い挙動)。
///
/// ログストリーム (`docker_log_stream`) でも再利用するため `pub(crate)` で公開する。
pub(crate) fn percent_encode_path_segment(s: &str) -> String {
    percent_encode(s)
}

/// Docker Engine API の query 値用 percent-encode。
///
/// `fromImage` / `tag` / `name` など。スペースは `%20` (form の `+` は使わない)。
fn percent_encode_component(s: &str) -> String {
    percent_encode(s)
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0xf) as usize] as char);
            }
        }
    }
    out
}

/// `POST /images/create` 用に descriptor を `fromImage` / `tag` に分解する。
///
/// digest (`@` 以降) を優先し、tag の `:` は最後の `/` より後ろのパス要素だけを見る。
/// tag が空のときは `latest` に正規化する。
fn split_pull_reference(descriptor: &str) -> (&str, &str) {
    if let Some((left, digest)) = descriptor.split_once('@') {
        let tag = if digest.is_empty() { "latest" } else { digest };
        let (from_image, _) = split_name_and_tag(left);
        return (from_image, tag);
    }
    split_name_and_tag(descriptor)
}

/// 最後のパス要素に対する tag 分割。`:` が無いか右側が空なら `tag=latest`。
fn split_name_and_tag(descriptor: &str) -> (&str, &str) {
    let name_tag = match descriptor.rfind('/') {
        Some(i) => &descriptor[i + 1..],
        None => descriptor,
    };
    match name_tag.split_once(':') {
        None => (descriptor, "latest"),
        Some((name, "")) => {
            let from_image = &descriptor[..descriptor.len() - name_tag.len() + name.len()];
            (from_image, "latest")
        }
        Some((name, tag)) => {
            let from_image = &descriptor[..descriptor.len() - name_tag.len() + name.len()];
            (from_image, tag)
        }
    }
}

/// `POST /images/create` の JSON Lines ボディに非空の `error` / `errorDetail.message` が無いか検査する。
fn check_pull_stream_errors(body: &[u8]) -> std::result::Result<(), ClientError> {
    let text = std::str::from_utf8(body).map_err(|e| ClientError::Json(e.to_string()))?;
    for line in text.lines() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        let parsed = nojson::RawJson::parse(line).map_err(|e| ClientError::Json(e.to_string()))?;
        if !parsed.value().kind().is_object() {
            return Err(ClientError::Json(format!(
                "expected object in pull stream, got {line}"
            )));
        }

        let error = parsed
            .value()
            .to_member("error")
            .ok()
            .and_then(|m| m.optional());
        let detail = parsed
            .value()
            .to_member("errorDetail")
            .ok()
            .and_then(|m| m.optional());
        let error_msg = error
            .and_then(|v| String::try_from(v).ok())
            .filter(|s| !s.is_empty());
        let detail_msg = detail
            .and_then(|d| d.to_member("message").ok())
            .and_then(|m| m.optional())
            .and_then(|v| String::try_from(v).ok())
            .filter(|s| !s.is_empty());

        if let Some(msg) = error_msg.or(detail_msg) {
            return Err(ClientError::Other(msg));
        }
    }
    Ok(())
}

fn build_port_bindings(ports: &[PortMapping]) -> BTreeMap<String, Vec<PortBinding>> {
    let mut map = BTreeMap::new();
    for p in ports {
        let key = format!(
            "{}/{}",
            p.container_port.as_u16(),
            p.container_port.as_str()
        );
        map.insert(
            key,
            vec![PortBinding {
                host_ip: "0.0.0.0".to_string(),
                host_port: p.host_port.to_string(),
            }],
        );
    }
    map
}

fn build_exposed_ports(ports: &[PortMapping]) -> Vec<String> {
    ports
        .iter()
        .map(|p| {
            format!(
                "{}/{}",
                p.container_port.as_u16(),
                p.container_port.as_str()
            )
        })
        .collect()
}

fn parse_ports(parsed: &nojson::RawJson<'_>) -> Ports {
    let mut ports = Ports::default();
    let network_settings = match parsed.value().to_member("NetworkSettings") {
        Ok(m) => match m.optional() {
            Some(v) => v,
            None => return ports,
        },
        Err(_) => return ports,
    };
    let ports_json = match network_settings.to_member("Ports") {
        Ok(m) => match m.optional() {
            Some(v) => v,
            None => return ports,
        },
        Err(_) => return ports,
    };
    let Ok(entries) = ports_json.to_object() else {
        return ports;
    };
    for (key, value) in entries {
        let key_str: String = match key.try_into() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let parts: Vec<&str> = key_str.split('/').collect();
        if parts.len() != 2 {
            continue;
        }
        let Ok(container_port) = parts[0].parse::<u16>() else {
            continue;
        };
        let container_port = match parts[1] {
            "tcp" => ContainerPort::Tcp(container_port),
            "udp" => ContainerPort::Udp(container_port),
            "sctp" => ContainerPort::Sctp(container_port),
            _ => continue,
        };
        if let Ok(bindings) = value.to_array() {
            for binding in bindings {
                if let Some(host_port) = binding
                    .to_member("HostPort")
                    .ok()
                    .and_then(|m| m.optional())
                    .and_then(|v| String::try_from(v).ok())
                    .and_then(|s| s.parse().ok())
                {
                    ports.add_mapping(container_port, host_port);
                }
            }
        }
    }
    ports
}

fn json_array(items: &[String]) -> String {
    let mut json = String::from("[");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&escape_json(item));
    }
    json.push(']');
    json
}

fn json_object(map: &BTreeMap<String, String>) -> String {
    let mut json = String::from("{");
    for (i, (k, v)) in map.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&escape_json(k));
        json.push(':');
        json.push_str(&escape_json(v));
    }
    json.push('}');
    json
}

fn json_port_bindings(map: &BTreeMap<String, Vec<PortBinding>>) -> String {
    let mut json = String::from("{");
    for (i, (k, bindings)) in map.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&escape_json(k));
        json.push_str(":[");
        for (j, b) in bindings.iter().enumerate() {
            if j > 0 {
                json.push(',');
            }
            json.push_str("{\"HostIp\":");
            json.push_str(&escape_json(&b.host_ip));
            json.push_str(",\"HostPort\":");
            json.push_str(&escape_json(&b.host_port));
            json.push('}');
        }
        json.push(']');
    }
    json.push('}');
    json
}

fn escape_json(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() + 2);
    escaped.push('"');
    for c in s.chars() {
        match c {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{0008}' => escaped.push_str("\\b"),
            '\u{000c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if (c as u32) < 0x20 => escaped.push_str(&format!("\\u{:04x}", c as u32)),
            c => escaped.push(c),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ports::IntoContainerPort;

    #[test]
    fn escape_json_escapes_special_characters() {
        let escaped = escape_json("a\"b\\c\nd\te\x01");
        assert_eq!(
            escaped, "\"a\\\"b\\\\c\\nd\\te\\u0001\"",
            "引用符・バックスラッシュ・改行・タブ・制御文字が正しくエスケープされること"
        );
    }

    #[test]
    fn escape_json_roundtrips_through_nojson_for_samples() {
        // 代表的な文字列が escape_json → nojson で往復すること。
        for s in ["", "plain", "a\"b", "line\n", "タブ\t", "\\"] {
            let escaped = escape_json(s);
            let parsed = nojson::RawJson::parse(&escaped)
                .expect("escape_json の出力は JSON としてパースできること");
            let value = String::try_from(parsed.value()).expect("JSON 文字列値として読めること");
            assert_eq!(value, s);
        }
    }

    #[test]
    fn json_array_and_object_roundtrip_samples() {
        // 配列・オブジェクトの簡易往復。
        let items = vec!["a".to_string(), "b\"c".to_string()];
        let json = json_array(&items);
        let parsed = nojson::RawJson::parse(&json).expect("json_array の出力はパースできること");
        let arr = parsed.value().to_array().expect("配列であること");
        let actual: Vec<String> = arr
            .map(|item| String::try_from(item).expect("配列要素は文字列であること"))
            .collect();
        assert_eq!(actual, items);

        let mut map = BTreeMap::new();
        map.insert("k".to_string(), "v".to_string());
        let json = json_object(&map);
        let parsed = nojson::RawJson::parse(&json).expect("json_object の出力はパースできること");
        let obj = parsed.value().to_object().expect("オブジェクトであること");
        let mut actual = BTreeMap::new();
        for (k, v) in obj {
            actual.insert(
                String::try_from(k).expect("キーは文字列であること"),
                String::try_from(v).expect("値は文字列であること"),
            );
        }
        assert_eq!(actual, map);
    }

    #[test]
    fn percent_encode_encodes_slash_in_image_refs() {
        // レジストリ付き参照の `/` と `:` が符号化されること。
        assert_eq!(
            percent_encode_path_segment("ghcr.io/org/app:1.0"),
            "ghcr.io%2Forg%2Fapp%3A1.0"
        );
        assert_eq!(
            percent_encode_component("ghcr.io/org/app"),
            "ghcr.io%2Forg%2Fapp"
        );
    }

    #[test]
    fn parse_ports_extracts_tcp_udp_sctp() {
        let json = r#"{"NetworkSettings":{"Ports":{"80/tcp":[{"HostPort":"8080"}],"53/udp":[{"HostPort":"5353"}],"5060/sctp":[{"HostPort":"5061"}]}}}"#;
        let parsed = nojson::RawJson::parse(json).expect("処理に失敗しないこと");
        let ports = parse_ports(&parsed);
        assert_eq!(
            ports.map_to_host_port_ipv4(80.tcp()),
            Some(8080),
            "tcp ポートが取得できること"
        );
        assert_eq!(
            ports.map_to_host_port_ipv4(53.udp()),
            Some(5353),
            "udp ポートが取得できること"
        );
        assert_eq!(
            ports.map_to_host_port_ipv4(5060.sctp()),
            Some(5061),
            "sctp ポートが取得できること"
        );
    }

    #[test]
    fn parse_ports_returns_empty_when_network_settings_missing() {
        let json = r#"{}"#;
        let parsed = nojson::RawJson::parse(json).expect("処理に失敗しないこと");
        let ports = parse_ports(&parsed);
        assert!(
            ports.map_to_host_port_ipv4(80.tcp()).is_none(),
            "NetworkSettings 欠落時は空の Ports になること"
        );
    }

    #[test]
    fn parse_ports_returns_empty_when_ports_missing() {
        let json = r#"{"NetworkSettings":{}}"#;
        let parsed = nojson::RawJson::parse(json).expect("処理に失敗しないこと");
        let ports = parse_ports(&parsed);
        assert!(
            ports.map_to_host_port_ipv4(80.tcp()).is_none(),
            "Ports 欠落時は空の Ports になること"
        );
    }

    #[test]
    fn parse_ports_ignores_invalid_port_strings() {
        let json = r#"{"NetworkSettings":{"Ports":{"abc/tcp":[{"HostPort":"8080"}],"80/xyz":[{"HostPort":"8080"}]}}}"#;
        let parsed = nojson::RawJson::parse(json).expect("処理に失敗しないこと");
        let ports = parse_ports(&parsed);
        assert!(
            ports.map_to_host_port_ipv4(80.tcp()).is_none(),
            "不正なポート文字列は無視されること"
        );
    }

    #[test]
    fn build_exposed_ports_formats_protocols() {
        let ports = vec![
            PortMapping {
                container_port: 80.into(),
                host_port: 8080,
            },
            PortMapping {
                container_port: 53.udp(),
                host_port: 5353,
            },
            PortMapping {
                container_port: 5060.sctp(),
                host_port: 5061,
            },
        ];
        let exposed = build_exposed_ports(&ports);
        assert_eq!(
            exposed,
            vec!["80/tcp", "53/udp", "5060/sctp"],
            "tcp/udp/sctp の書式が正しいこと"
        );
    }

    /// テスト用の最小 `ContainerConfig` を組み立てる。
    fn config_with_mounts(mounts: Vec<crate::core::mounts::Mount>) -> ContainerConfig {
        ContainerConfig {
            image: "alpine:latest".into(),
            entrypoint: None,
            cmd: vec![],
            env: vec![],
            ports: vec![],
            mounts,
            name: None,
            labels: BTreeMap::new(),
            privileged: false,
            working_dir: None,
            user: None,
            init: false,
        }
    }

    #[test]
    fn from_config_bind_readonly_gets_ro_suffix() {
        use crate::core::mounts::{AccessMode, Mount};

        let mount = Mount::bind_mount("/host", "/container").with_access_mode(AccessMode::ReadOnly);
        let body = CreateContainerBody::from_config(config_with_mounts(vec![mount]))
            .expect("Bind マウントは成功すること");
        assert_eq!(
            body.host_config.binds,
            vec!["/host:/container:ro".to_string()],
            "ReadOnly は :ro サフィックスになること"
        );
    }

    #[test]
    fn from_config_bind_readwrite_gets_rw_suffix() {
        use crate::core::mounts::{AccessMode, Mount};

        let mount =
            Mount::bind_mount("/host", "/container").with_access_mode(AccessMode::ReadWrite);
        let body = CreateContainerBody::from_config(config_with_mounts(vec![mount]))
            .expect("Bind マウントは成功すること");
        assert_eq!(
            body.host_config.binds,
            vec!["/host:/container:rw".to_string()],
            "ReadWrite は :rw サフィックスになること"
        );
    }

    #[test]
    fn from_config_volume_mount_returns_configuration_error() {
        use crate::core::mounts::Mount;

        let result =
            CreateContainerBody::from_config(config_with_mounts(vec![Mount::volume_mount(
                "vol",
                "/container",
            )]));
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("Volume マウントはエラーになること"),
        };
        match err {
            crate::core::error::Error::Client(ClientError::Configuration(msg)) => {
                assert_eq!(
                    msg, "volume mount is not implemented on Linux",
                    "Volume 未実装メッセージであること"
                );
            }
            other => panic!("Configuration 以外のエラー: {other}"),
        }
    }

    #[test]
    fn from_config_tmpfs_mount_returns_configuration_error() {
        use crate::core::mounts::Mount;

        let result =
            CreateContainerBody::from_config(config_with_mounts(vec![Mount::tmpfs_mount(
                "/tmpfs",
            )]));
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("Tmpfs マウントはエラーになること"),
        };
        match err {
            crate::core::error::Error::Client(ClientError::Configuration(msg)) => {
                assert_eq!(
                    msg, "tmpfs mount is not implemented on Linux",
                    "Tmpfs 未実装メッセージであること"
                );
            }
            other => panic!("Configuration 以外のエラー: {other}"),
        }
    }

    #[test]
    fn split_pull_reference_accepts_registry_port_and_digest() {
        // 公開 API 主再現。ポートを tag と誤認しない
        assert_eq!(
            split_pull_reference("localhost:5000/nginx:latest"),
            ("localhost:5000/nginx", "latest")
        );
        // ポート付き・ tag 無し
        assert_eq!(
            split_pull_reference("localhost:5000/nginx"),
            ("localhost:5000/nginx", "latest")
        );
        // 後方互換 (port 無し tag 有り)
        assert_eq!(split_pull_reference("nginx:1.25"), ("nginx", "1.25"));
        // 後方互換 (tag 無し。単体テスト用)
        assert_eq!(split_pull_reference("nginx"), ("nginx", "latest"));
        // 複数 `/`
        assert_eq!(
            split_pull_reference("docker.io/library/nginx:latest"),
            ("docker.io/library/nginx", "latest")
        );
        // digest。tag クエリに digest 全体
        assert_eq!(
            split_pull_reference("nginx@sha256:ab12cd"),
            ("nginx", "sha256:ab12cd")
        );
        // 公開 API で到達しうる digest 形
        assert_eq!(
            split_pull_reference("nginx:latest@sha256:ab12cd"),
            ("nginx", "sha256:ab12cd")
        );
        // port + tag + digest。digest 分岐でも port を切らない
        assert_eq!(
            split_pull_reference("localhost:5000/nginx:1.25@sha256:ab12cd"),
            ("localhost:5000/nginx", "sha256:ab12cd")
        );
        // tag に digest を載せた公開 API 形。`rsplit` 禁止の回帰防止
        assert_eq!(
            split_pull_reference("nginx:sha256:ab12cd"),
            ("nginx", "sha256:ab12cd")
        );
        // `:` あり・右側空を `latest` に正規化
        assert_eq!(split_pull_reference("nginx:"), ("nginx", "latest"));
        // `@` あり・右側空を `latest` に正規化
        assert_eq!(split_pull_reference("nginx@"), ("nginx", "latest"));
    }

    /// `ClientError::Other` であることを検証し、メッセージを返す。
    fn expect_other(err: ClientError) -> String {
        match err {
            ClientError::Other(msg) => msg,
            other => panic!("Other 以外のエラー: {other}"),
        }
    }

    /// `ClientError::Json` であることを検証する。
    fn expect_json(err: ClientError) {
        match err {
            ClientError::Json(_) => {}
            other => panic!("Json 以外のエラー: {other}"),
        }
    }

    #[test]
    fn check_pull_stream_errors_detects_error_and_detail() {
        // error + errorDetail.message 両方 (非空) → error 側を優先
        let err = check_pull_stream_errors(
            br#"{"error":"denied","errorDetail":{"message":"detail denied"}}"#,
        )
        .expect_err("error がある行は失敗すること");
        assert_eq!(expect_other(err), "denied");

        // error のみ
        let err = check_pull_stream_errors(br#"{"error":"only error"}"#)
            .expect_err("error のみでも失敗すること");
        assert_eq!(expect_other(err), "only error");

        // errorDetail.message のみ
        let err = check_pull_stream_errors(br#"{"errorDetail":{"message":"only detail"}}"#)
            .expect_err("errorDetail.message のみでも失敗すること");
        assert_eq!(expect_other(err), "only detail");

        // 空の error は無視し、非空の detail を採用する
        let err = check_pull_stream_errors(br#"{"error":"","errorDetail":{"message":"real"}}"#)
            .expect_err("空 error + 非空 detail は失敗すること");
        assert_eq!(expect_other(err), "real");
    }

    #[test]
    fn check_pull_stream_errors_ignores_empty_error_fields() {
        // 空の error のみ / 空の errorDetail のみは成功
        check_pull_stream_errors(br#"{"error":""}"#).expect("空 error は成功であること");
        check_pull_stream_errors(br#"{"errorDetail":{}}"#)
            .expect("空 errorDetail は成功であること");
    }

    #[test]
    fn check_pull_stream_errors_accepts_progress_and_aux() {
        // 進捗のみ複数行
        let body =
            b"{\"status\":\"Pulling from library/nginx\"}\n{\"status\":\"Download complete\"}\n";
        check_pull_stream_errors(body).expect("進捗のみは成功であること");

        // 成功終端の aux のみ
        check_pull_stream_errors(br#"{"aux":{"ID":"sha256:dead"}}"#)
            .expect("aux のみは成功であること");
    }

    #[test]
    fn check_pull_stream_errors_rejects_non_object_and_bad_json() {
        // オブジェクト以外
        expect_json(check_pull_stream_errors(b"[]").expect_err("配列行は Json エラーであること"));
        // 不正 JSON
        expect_json(
            check_pull_stream_errors(b"{not-json").expect_err("不正 JSON は Json エラーであること"),
        );
        // 非 UTF-8
        expect_json(
            check_pull_stream_errors(&[0xff, 0xfe]).expect_err("非 UTF-8 は Json エラーであること"),
        );
    }

    #[test]
    fn check_pull_stream_errors_fails_on_error_after_progress() {
        // 進捗のあとエラー行
        let body = b"{\"status\":\"Pulling\"}\n{\"error\":\"boom\"}\n";
        let err = check_pull_stream_errors(body).expect_err("進捗後の error は失敗すること");
        assert_eq!(expect_other(err), "boom");
    }

    #[test]
    fn check_pull_stream_errors_accepts_empty_body() {
        // 空ボディ / 空行のみ
        check_pull_stream_errors(b"").expect("空ボディは成功であること");
        check_pull_stream_errors(b"\n\n").expect("空行のみは成功であること");
    }

    #[test]
    fn encode_docker_api_request_sets_connection_close() {
        // Connection: close が付与されること
        let bytes =
            encode_docker_api_request("GET", "/version", None).expect("エンコードに失敗しないこと");
        let text = String::from_utf8(bytes).expect("UTF-8 であること");
        assert!(
            text.contains("Connection: close\r\n"),
            "Connection: close が含まれること: {text}"
        );
    }

    #[test]
    fn read_http11_response_returns_before_peer_closes() {
        use std::sync::mpsc;
        use std::time::Duration;

        // 書き込み側を判定完了まで drop しない (drop すると n==0 で抜け偽陽性になる)
        let (mut writer, mut reader) =
            UnixStream::pair().expect("UnixStream::pair に失敗しないこと");
        writer
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello")
            .expect("レスポンス書き込みに失敗しないこと");

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = read_http11_response(&mut reader, "GET");
            let _ = tx.send(result);
        });

        let response = rx
            .recv_timeout(Duration::from_millis(200))
            .expect("ボディ完了後に keep-alive 待ちでハングしないこと")
            .expect("デコードに失敗しないこと");
        assert_eq!(
            response.body_bytes(),
            Some(b"hello".as_slice()),
            "Content-Length ボディが取得できること"
        );
        // writer をここで明示的に保持し終える
        drop(writer);
    }
}
