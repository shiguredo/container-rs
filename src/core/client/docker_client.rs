//! Ubuntu 用 Docker Engine API クライアント。
//!
//! Docker Engine API は HTTP/1.1 REST API であるため、`shiguredo_http11` でリクエストを送信する。
//! Unix ドメインソケット `/var/run/docker.sock` を介して通信する。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use shiguredo_http11::{Request, Response};

use crate::core::client::http_decode::BodyLimit;
use crate::core::client::{ContainerConfig, ContainerSnapshot, HealthProbe, HealthStatus};
use crate::core::containers::request::PortMapping;
use crate::core::error::{ClientError, Result};
use crate::core::healthcheck::Healthcheck;
use crate::core::ports::{ContainerPort, Ports};

const DEFAULT_DOCKER_SOCKET: &str = "/var/run/docker.sock";
/// exec の exit code 取得リトライにおける、失敗ごとのバックオフ間隔列。
///
/// 初回試行は即時で、失敗するたびにこの列の値を sleep して再試行する。
/// ループは `0..=len` で回るため、試行回数は列の長さ + 1 (最大 6 回)、
/// sleep は列の合計 (約 1.76 秒) がすべて実行される。
/// Docker daemon は exec ストリーム閉塞直後に `Running` / `ExitCode` をまだ
/// 記録していない場合があるため、指数的に伸ばすバックオフで待つ。
const EXEC_EXIT_CODE_BACKOFF_MILLIS: &[u64] = &[10, 50, 200, 500, 1000];

/// `GET /exec/{id}/json` の応答 (JSON 文字列) から exec の実行状態を読み取る。
///
/// 戻り値は `(running, exit_code)`。`Running` フィールドのパース失敗 (欠落・型不一致)
/// は終了扱い (`false`) として扱う (現行挙動の維持)。`ExitCode` は `Running == false`
/// のときのみ意味を持つ。ボディ全体が非 UTF-8 / JSON として不正な場合は
/// `ClientError::Json` を返す (デーモン異常の診断情報を失わないため、従来どおり即エラー)。
fn parse_exec_inspect_state(body: &[u8]) -> Result<(bool, Option<i64>)> {
    let text = std::str::from_utf8(body).map_err(|e| ClientError::Json(e.to_string()))?;
    let parsed = nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))?;
    let value = parsed.value();
    let running = value
        .to_member("Running")
        .ok()
        .and_then(|m| m.optional())
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false);
    let exit_code = value
        .to_member("ExitCode")
        .ok()
        .and_then(|m| m.optional())
        .and_then(|v| i64::try_from(v).ok());
    Ok((running, exit_code))
}

/// パース済みの exec 状態列を先頭から順に消化し、最初に `Running == false` になった
/// 状態の `ExitCode` を返す (純粋関数)。
///
/// `Running == true` の状態は無視して次へ進む。すべて `Running == true` のまま
/// 打ち切られた場合は `None` を返す。`Running` のパース失敗を終了扱い (`false`) に
/// した結果は、`(false, exit_code)` の状態としてこの列に現れる。
fn resolve_exec_exit_code(states: &[(bool, Option<i64>)]) -> Option<i64> {
    states
        .iter()
        .find(|(running, _)| !running)
        .and_then(|(_, exit_code)| *exit_code)
}

/// `remove_blocking` 専用の UnixStream 読み書きタイムアウト。
/// Drop 経路から呼ばれるため、デーモン無応答時に呼び出しスレッドが恒久ブロックするのを防ぐ。
/// exec start や stop?t=N 等、正当に長時間ブロックする経路には適用しない。
const DOCKER_STREAM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Docker Engine API レスポンスボディの蓄積上限。macOS 経路 (`read_file_to_vec_cancellable`) と同じ 64 MiB の値。
///
/// 適用対象は以下の 6 経路で共有する (将来の上限変更で一部だけが変わる非対称を防ぐ):
/// - exec start の出力: demux 前の multiplexed stream 全体 (stdout + stderr の合計、
///   フレームヘッダ込み) で、macOS の stdout / stderr 各ストリーム別 64 MiB より実効上限が
///   厳しい (この非対称は許容する)。超過時は切り詰めずエラーにする。multiplexed stream を
///   切り詰めるとフレーム途中で切断され `demux_exec_stream` が不完全フレームを静かに捨てて
///   出力が欠損するため。判定は macOS 側と同じ `>` 境界 (ちょうど 64 MiB は成功)
/// - 1-shot ログ取得 (`?follow=false`) の各ストリーム蓄積
/// - `copy_from` の tar 全体 (ヘッダ + データ + トレーラ)
/// - `copy_to` の tar 全体 (ヘッダ + データ + トレーラ、`UstarBuilder` の蓄積)
/// - `copy_to` の per-file 読み込み (コピー対象 1 ファイルあたり)
/// - イメージ pull の進捗ストリーム (JSON Lines)
pub(crate) const DOCKER_RESPONSE_BODY_LIMIT: usize = 64 * 1024 * 1024;

/// Docker exec の生結果。
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
pub(crate) struct DockerClient {
    socket_path: String,
}

impl DockerClient {
    /// Unix ドメインソケット経由で Docker API に接続するクライアントを返す。
    pub(crate) fn detect() -> Result<Self> {
        Ok(Self {
            socket_path: DEFAULT_DOCKER_SOCKET.to_string(),
        })
    }

    /// イメージをプルする。
    pub(crate) async fn pull_image(&self, descriptor: &str, platform: Option<&str>) -> Result<()> {
        let (image, tag) = split_pull_reference(descriptor);
        // Docker Engine API は query 値の `/` 等を percent-encode する必要がある。
        let mut path = format!(
            "/images/create?fromImage={}&tag={}",
            percent_encode_component(image),
            percent_encode_component(tag)
        );
        if let Some(platform) = platform {
            path.push_str(&format!("&platform={}", percent_encode_component(platform)));
        }
        // プライベートレジストリ認証があれば X-Registry-Auth ヘッダを付与する。
        let auth_header = super::registry_auth::x_registry_auth(descriptor);
        let extra_headers: Vec<(&'static str, String)> = auth_header
            .map(|v| vec![("X-Registry-Auth", v)])
            .unwrap_or_default();
        // プル進捗ストリーム (JSON Lines) は無制限にバッファリングしない。
        // 実用上 64 MiB 未満に収まるため、超過時はエラーにする (OOM 防止)。
        // 上限超過時は進捗ストリーム末尾の daemon エラー (errorDetail) が読めないため、
        // 文脈を付けて包む (エラーバリアントは ClientError::Other のまま維持する)。
        let response = self
            .request_with_extra_headers(
                "POST",
                &path,
                None,
                extra_headers,
                BodyLimit::Error(DOCKER_RESPONSE_BODY_LIMIT),
            )
            .await
            .map_err(|e| {
                crate::core::error::Error::Client(ClientError::Other(format!(
                    "failed to receive pull progress for image {descriptor}: {e}"
                )))
            })?;
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
    pub(crate) async fn resolve_image_descriptor(
        &self,
        descriptor: &str,
        platform: Option<&str>,
    ) -> Result<String> {
        // path セグメントの `/` を生のまま埋め込むとルートが壊れる
        // (例: `ghcr.io/org/app:tag` → `/images/ghcr.io/org/...`)。
        let path = format!("/images/{}/json", percent_encode_path_segment(descriptor));
        let response = self.request("GET", &path, None).await?;
        if response.status_code() == 200 {
            Ok(descriptor.to_string())
        } else if response.status_code() == 404 {
            self.pull_image(descriptor, platform).await?;
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
        let mut params = Vec::new();
        if let Some(name) = &config.name {
            params.push(format!("name={}", percent_encode_component(name)));
        }
        if let Some(platform) = &config.platform {
            params.push(format!("platform={}", percent_encode_component(platform)));
        }
        let query = if params.is_empty() {
            String::new()
        } else {
            format!("?{}", params.join("&"))
        };
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

    /// コンテナを一時停止する。
    ///
    /// 既に一時停止済み (304) は冪等に成功とする。
    pub(crate) async fn pause(&self, id: &str) -> Result<()> {
        let path = format!("/containers/{}/pause", percent_encode_path_segment(id));
        let response = self.request("POST", &path, None).await?;
        if response.status_code() == 304 || response.status_code() == 404 {
            return Ok(());
        }
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to pause container: {}",
                response.status_code()
            ))
            .into());
        }
        Ok(())
    }

    /// コンテナの一時停止を解除する。
    ///
    /// 既に実行中 (304) は冪等に成功とする。
    pub(crate) async fn unpause(&self, id: &str) -> Result<()> {
        let path = format!("/containers/{}/unpause", percent_encode_path_segment(id));
        let response = self.request("POST", &path, None).await?;
        if response.status_code() == 304 || response.status_code() == 404 {
            return Ok(());
        }
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to unpause container: {}",
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
        stream.set_read_timeout(Some(DOCKER_STREAM_TIMEOUT))?;
        stream.set_write_timeout(Some(DOCKER_STREAM_TIMEOUT))?;
        stream.write_all(&request_bytes)?;
        // 書き込み半閉じは dockerd / Docker Desktop が 500 を返すため行わない。
        // 対向の接続保持は `Connection: close` とボディ完了時の即リターンで防ぐ。
        let response = read_http11_response(&mut stream, "DELETE", BodyLimit::Unlimited)?;
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

    /// コンテナの終了を同期で待つ。`POST /containers/{id}/wait?condition=not-running` を呼び、
    /// レスポンスの `StatusCode` を返す。tokio Runtime に依存しないため std スレッドから
    /// 直接呼べる。コンテナ削除後の 404 等はエラーとして返す (呼び出し側で無視する)。
    ///
    /// 注意: コンテナ終了待ちは正当に無制限にブロックし得るため、
    /// `remove_blocking` とは異なりタイムアウトを意図的に設定しない。
    pub(crate) fn wait_blocking(&self, id: &str) -> Result<i64> {
        let path = format!(
            "/containers/{}/wait?condition=not-running",
            percent_encode_path_segment(id)
        );
        let request_bytes = encode_docker_api_request("POST", &path, None)?;
        let mut stream = UnixStream::connect(&self.socket_path)?;
        stream.write_all(&request_bytes)?;
        let response = read_http11_response(&mut stream, "POST", BodyLimit::Unlimited)?;
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to wait for container: {}",
                response.status_code()
            ))
            .into());
        }
        let body = response
            .body_bytes()
            .ok_or_else(|| ClientError::Other("empty wait response body".into()))?;
        let text = std::str::from_utf8(body).map_err(|e| ClientError::Json(e.to_string()))?;
        let parsed = nojson::RawJson::parse(text).map_err(|e| ClientError::Json(e.to_string()))?;
        let status_code = parsed
            .value()
            .to_member("StatusCode")
            .map_err(|e| ClientError::Json(e.to_string()))?
            .required()
            .map_err(|e| ClientError::Json(e.to_string()))?;
        let code: i64 = status_code
            .try_into()
            .map_err(|e: nojson::JsonParseError| ClientError::Json(e.to_string()))?;
        Ok(code)
    }

    /// コンテナ内でコマンドを実行し、終了コードと stdout / stderr を取得する。
    ///
    /// `AttachStdout: true, AttachStderr: true, Detach: false` で exec を作成・起動し、
    /// `POST /exec/{id}/start` のレスポンスボディ (multiplexed stream) を全蓄積して
    /// demux する。ストリーム EOF 後に inspect で exit code を取得する。
    ///
    /// `env` が非空の場合は `ExecConfig.Env` に設定する。Docker Engine API は
    /// `Env` 指定時にコンテナ env を置換するため、呼び出し側でコンテナ env との
    /// マージ済みリストを渡すこと。空の場合は `Env` を送信せず Docker の継承に任せる。
    pub(crate) async fn exec(
        &self,
        id: &str,
        cmd: &[String],
        env: Vec<String>,
    ) -> Result<DockerExecResult> {
        let exec_path = format!("/containers/{}/exec", percent_encode_path_segment(id));
        let exec_config = ExecConfig {
            cmd: cmd.to_vec(),
            attach_stdout: true,
            attach_stderr: true,
            env,
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

        // Detach=false で起動し、レスポンスボディの multiplexed stream を全蓄積する。
        // exec の出力はプロセス終了で EOF するが、任意のコマンドが実行可能なため
        // 大量出力 (例: `yes | head -c 1G`) で OOM になり得る。蓄積上限は 64 MiB で、
        // 超過時は切り詰めずエラーにする (フレーム途中切断で出力欠損するため)。
        let start_path = format!("/exec/{}/start", percent_encode_path_segment(&exec_id));
        let start_config = ExecStartConfig {
            detach: false,
            tty: false,
        };
        let start_json = start_config.to_json_string()?;
        let response = self
            .request_with_body_limit(
                "POST",
                &start_path,
                Some(start_json.into_bytes()),
                BodyLimit::Error(DOCKER_RESPONSE_BODY_LIMIT),
            )
            .await?;
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to start exec: {}",
                response.status_code()
            ))
            .into());
        }

        // multiplexed stream を demux して stdout / stderr に分離する。
        // ボディ無しは Docker daemon 側の異常のため明示エラーにする。
        let stream_body = response
            .body_bytes()
            .ok_or_else(|| ClientError::Other("empty exec start response body".into()))?;
        let (stdout, stderr) = demux_exec_stream(stream_body);

        // ストリーム EOF 後に inspect で exit code を取得する。
        // Docker daemon はストリーム閉塞直後に ExitCode をまだ記録していない場合があるため、
        // Running == false になるまで指数的バックオフで再試行する。
        // 初回は即時、失敗ごとに 10ms → 50ms → 200ms → 500ms → 1s の sleep を挟む
        // (試行最大 6 回・合計約 1.76 秒)。超過時は warn ログ + exit_code: None に倒す。
        let inspect_path = format!("/exec/{}/json", percent_encode_path_segment(&exec_id));
        let mut states: Vec<(bool, Option<i64>)> = Vec::new();
        // 初回試行は即時。バックオフ列の長さ分の再試行 (sleep) を挟むため、
        // 試行回数はバックオフ列の長さ + 1 (最大 6 回) になる。列のイテレートでは
        // 初回 (sleep 前) の試行を表現できないため、範囲ループでインデックス参照する。
        #[expect(clippy::needless_range_loop)]
        for attempt in 0..=EXEC_EXIT_CODE_BACKOFF_MILLIS.len() {
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
            let state = parse_exec_inspect_state(body)?;
            let (running, _) = state;
            states.push(state);
            if !running {
                break;
            }
            // 最終試行の後は sleep しない。
            if attempt < EXEC_EXIT_CODE_BACKOFF_MILLIS.len() {
                tokio::time::sleep(std::time::Duration::from_millis(
                    EXEC_EXIT_CODE_BACKOFF_MILLIS[attempt],
                ))
                .await;
            }
        }
        let exit_code = resolve_exec_exit_code(&states);
        if exit_code.is_none() {
            tracing::warn!("exec {exec_id} still running after stream EOF, exit code unavailable");
        }

        Ok(DockerExecResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    /// コンテナのポートマッピングを取得する。
    pub(crate) async fn ports(&self, id: &str) -> Result<Ports> {
        let snapshot = self.container_state(id).await?;
        Ok(snapshot.ports)
    }

    /// コンテナのブリッジネットワーク IP アドレスを取得する。
    ///
    /// inspect (`GET /containers/{id}/json`) の `NetworkSettings.Networks` から
    /// 先頭ネットワークの `IPAddress` を取得する。ネットワーク名のハードコードは
    /// しない (カスタムネットワーク対応のため)。Docker の `Networks` は JSON オブジェクト
    /// であり、「先頭」はキーのアルファベット順で決まる (macOS の配列順とは異なる)。
    /// `IPAddress` が空文字列の場合 (host ネットワークモード等) や `Networks` が空・欠落
    /// の場合はエラーを返す。
    pub(crate) async fn bridge_ip_address(&self, id: &str) -> Result<std::net::IpAddr> {
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

        // NetworkSettings.Networks の先頭エントリの IPAddress を取得する
        let ip_str = parsed
            .value()
            .to_member("NetworkSettings")
            .ok()
            .and_then(|m| m.optional())
            .and_then(|ns| ns.to_member("Networks").ok())
            .and_then(|m| m.optional())
            .and_then(|networks| networks.to_object().ok())
            .and_then(|mut obj| obj.next())
            .and_then(|(_, network)| {
                network
                    .to_member("IPAddress")
                    .ok()
                    .and_then(|m| m.optional())
                    .and_then(|v| String::try_from(v).ok())
            })
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ClientError::Other("no network IP address found for container".into())
            })?;

        ip_str
            .parse::<std::net::IpAddr>()
            .map_err(|e| ClientError::Other(format!("invalid IP address '{ip_str}': {e}")).into())
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

    /// コンテナの環境変数を inspect (`GET /containers/{id}/json`) の `Config.Env` から取得する。
    ///
    /// 返り値は `["KEY=VALUE", ...]` 形式の文字列ベクトル。exec の `Env` 指定時に
    /// コンテナ env を置換する Docker Engine API のセマンティクスに対応するため、
    /// 呼び出し側で exec 分の env を上書きマージしてから `exec` に渡す。
    pub(crate) async fn container_env(&self, id: &str) -> Result<Vec<String>> {
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

        let mut env = Vec::new();
        if let Ok(config) = parsed.value().to_member("Config")
            && let Some(config) = config.optional()
            && let Ok(env_member) = config.to_member("Env")
            && let Some(env_arr) = env_member.optional()
            && let Ok(arr) = env_arr.to_array()
        {
            for item in arr {
                if let Ok(s) = String::try_from(item) {
                    env.push(s);
                }
            }
        }
        Ok(env)
    }

    /// コンテナの running と `State.Health.Status` を取得する。
    pub(crate) async fn container_health(&self, id: &str) -> Result<HealthProbe> {
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

        // State.Health が無い / Status が none・空・未知なら health = None。
        let health = state
            .to_member("Health")
            .ok()
            .and_then(|m| m.optional())
            .and_then(|health| {
                health
                    .to_member("Status")
                    .ok()
                    .and_then(|m| m.optional())
                    .and_then(|v| String::try_from(v).ok())
            })
            .and_then(|status| match status.as_str() {
                "starting" => Some(HealthStatus::Starting),
                "healthy" => Some(HealthStatus::Healthy),
                "unhealthy" => Some(HealthStatus::Unhealthy),
                "none" | "" => None,
                _ => None,
            });

        Ok(HealthProbe { running, health })
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
        self.request_with_body_limit(method, path, body, BodyLimit::Unlimited)
            .await
    }

    /// ボディ蓄積上限を指定して Docker Engine API に HTTP リクエストを送信する。
    ///
    /// ボディ上限は exec start の出力読み出し・`copy_from` (archive) ・プル進捗受信の
    /// 早期アボート用 (OOM 防止)。上限を指定しない経路は `request` 経由で
    /// `BodyLimit::Unlimited` を使い、挙動を変えない。
    async fn request_with_body_limit(
        &self,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
        body_limit: BodyLimit,
    ) -> Result<Response> {
        let socket_path = self.socket_path.clone();
        let method = method.to_string();
        let path = path.to_string();

        tokio::task::spawn_blocking(move || -> Result<Response> {
            // encode 失敗時にソケットを開かないよう、connect より先にエンコードする。
            let request_bytes = encode_docker_api_request(
                &method,
                &path,
                body.as_deref().map(|b| (b, "application/json")),
            )?;

            let mut stream = UnixStream::connect(&socket_path)?;
            stream.write_all(&request_bytes)?;
            // 書き込み半閉じは dockerd / Docker Desktop が create / start 等で
            // 500 Internal Server Error を返すため行わない。
            // 対向の接続保持は `Connection: close` とボディ完了時の即リターンで防ぐ。
            // 注意: exec start (Detach: false) や stop?t=N 等は正当に長時間ブロックするため、
            // この共有メソッドにはタイムアウトを設定しない。タイムアウトは remove_blocking 等、
            // 短時間で完了すべき同期専用経路に個別に設定する。

            read_http11_response(&mut stream, &method, body_limit)
        })
        .await
        .map_err(|e| ClientError::Other(format!("spawn_blocking failed: {e}")))?
    }

    /// 追加ヘッダ付きの HTTP リクエスト (レジストリ認証用)。
    ///
    /// `body_limit` は受信ボディの蓄積上限 (プル進捗ストリームの OOM 防止に使う)。
    async fn request_with_extra_headers(
        &self,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
        extra_headers: Vec<(&'static str, String)>,
        body_limit: BodyLimit,
    ) -> Result<Response> {
        let socket_path = self.socket_path.clone();
        let method = method.to_string();
        let path = path.to_string();

        tokio::task::spawn_blocking(move || -> Result<Response> {
            let request_bytes = encode_docker_api_request_with_headers(
                &method,
                &path,
                body.as_deref().map(|b| (b, "application/json")),
                &extra_headers,
            )?;

            let mut stream = UnixStream::connect(&socket_path)?;
            stream.write_all(&request_bytes)?;
            read_http11_response(&mut stream, &method, body_limit)
        })
        .await
        .map_err(|e| ClientError::Other(format!("spawn_blocking failed: {e}")))?
    }

    /// 任意の Content-Type でボディを送る HTTP リクエスト (tar 送信用)。
    ///
    /// 既存の JSON 固定 `request` は変更せず、`copy_to` 専用に分ける (既存呼び出しへの影響を最小化)。
    async fn request_with_content_type(
        &self,
        method: &str,
        path: &str,
        body: Vec<u8>,
        content_type: &'static str,
    ) -> Result<Response> {
        let socket_path = self.socket_path.clone();
        let method = method.to_string();
        let path = path.to_string();

        tokio::task::spawn_blocking(move || -> Result<Response> {
            let request_bytes =
                encode_docker_api_request(&method, &path, Some((body.as_slice(), content_type)))?;

            let mut stream = UnixStream::connect(&socket_path)?;
            stream.write_all(&request_bytes)?;
            read_http11_response(&mut stream, &method, BodyLimit::Unlimited)
        })
        .await
        .map_err(|e| ClientError::Other(format!("spawn_blocking failed: {e}")))?
    }

    /// コンテナからファイルを取り出す (`GET /containers/{id}/archive`)。
    ///
    /// レスポンスボディの生 tar を返す。404 はボディの daemon メッセージで
    /// 「コンテナ不存在」と「コンテナ内パス不存在」を区別する (区別できない場合は
    /// `ContainerNotFound` に寄せる)。
    ///
    /// 蓄積は tar 全体 (ヘッダ + データ + トレーラ) で 64 MiB 上限・超過時エラー
    /// (OOM 防止。ファイル内容がちょうど 64 MiB でも tar オーバーヘッド分でエラーになり得る)。
    pub(crate) async fn copy_from(&self, id: &str, path: &str) -> Result<Vec<u8>> {
        let api_path = format!(
            "/containers/{}/archive?path={}",
            percent_encode_path_segment(id),
            percent_encode_component(path)
        );
        let response = self
            .request_with_body_limit(
                "GET",
                &api_path,
                None,
                BodyLimit::Error(DOCKER_RESPONSE_BODY_LIMIT),
            )
            .await?;
        if response.status_code() == 404 {
            return Err(
                classify_archive_404(id, path, response.body_bytes().unwrap_or(&[])).into(),
            );
        }
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to copy from container: {}",
                response.status_code()
            ))
            .into());
        }
        Ok(response.body_bytes().unwrap_or(&[]).to_vec())
    }

    /// コンテナへ tar を投入する (`PUT /containers/{id}/archive`)。
    ///
    /// `dir` は投入先ディレクトリ。`copyUIDGID=true` で tar ヘッダの uid / gid を反映する。
    /// 404 は `ContainerNotFound` に寄せる。
    pub(crate) async fn copy_to(&self, id: &str, dir: &str, tar: Vec<u8>) -> Result<()> {
        let api_path = format!(
            "/containers/{}/archive?path={}&copyUIDGID=true",
            percent_encode_path_segment(id),
            percent_encode_component(dir)
        );
        let response = self
            .request_with_content_type("PUT", &api_path, tar, "application/x-tar")
            .await?;
        if response.status_code() == 404 {
            return Err(ClientError::ContainerNotFound(id.to_string()).into());
        }
        if response.status_code() >= 400 {
            return Err(ClientError::Other(format!(
                "failed to copy to container: {}",
                response.status_code()
            ))
            .into());
        }
        Ok(())
    }
}

/// Docker Engine API の `GET /containers/{id}/archive` の 404 応答を分類する。
///
/// Docker Engine は「コンテナ不存在」と「コンテナ内パス不存在」の両方で 404 を返す。
/// moby の実装ではボディの JSON `message` にそれぞれ
/// `No such container: <id>` / `Could not find the file <path> in container <id>` を含む。
///
/// - `message` が `Could not find the file ` で始まる → `ContainerPathNotFound` (パス不存在)
/// - それ以外 (コンテナ不存在・未知文言・空ボディ・非 JSON) → `ContainerNotFound`
///
/// マッチは `starts_with` で行う (パス名に `No such container:` 等を含む場合の誤分類を防ぐ)。
/// daemon メッセージからのパス切り出しは文言変更で壊れるため行わず、リクエスト引数の
/// `path` をそのまま保持する。区別できない場合は既存挙動 (`ContainerNotFound`) を維持する
/// (パス不存在の誤診断を増やさない安全側の設計)。
fn classify_archive_404(id: &str, path: &str, body: &[u8]) -> ClientError {
    let message = parse_daemon_error_message(body);
    match message {
        Some(msg) if msg.starts_with("Could not find the file ") => {
            ClientError::ContainerPathNotFound(path.to_string())
        }
        _ => ClientError::ContainerNotFound(id.to_string()),
    }
}

/// daemon エラーボディ (`{"message": "..."}`) から `message` フィールドの値を取り出す。
///
/// 空・非 JSON・`message` 欠落は `None` を返す。
fn parse_daemon_error_message(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let parsed = nojson::RawJson::parse(text).ok()?;
    parsed
        .value()
        .to_member("message")
        .ok()
        .and_then(|m| m.required().ok())
        .and_then(|v| TryInto::<String>::try_into(v).ok())
}

/// Docker Engine API 向け HTTP/1.1 リクエストをエンコードする。
///
/// ログストリーム (`docker_log_stream`) と tar 送信 (`copy_to`) で再利用するため
/// `pub(crate)` で公開する。`body` は `(本体, Content-Type)` のタプル。
pub(crate) fn encode_docker_api_request(
    method: &str,
    path: &str,
    body: Option<(&[u8], &'static str)>,
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
    if let Some((body, content_type)) = body {
        request = request
            .header("Content-Type", content_type)
            .map_err(http11_err)?;
        request = request
            .header("Content-Length", &body.len().to_string())
            .map_err(http11_err)?;
        request = request.body(body.to_vec());
    }
    request.encode().map_err(http11_err)
}

/// 追加ヘッダ付きで Docker Engine API 向け HTTP/1.1 リクエストをエンコードする。
fn encode_docker_api_request_with_headers(
    method: &str,
    path: &str,
    body: Option<(&[u8], &'static str)>,
    extra_headers: &[(&'static str, String)],
) -> Result<Vec<u8>> {
    let method_obj = shiguredo_http11::Method::new(method).map_err(http11_err)?;
    let mut request = Request::new(method_obj, path)
        .map_err(http11_err)?
        .header("Host", "localhost")
        .map_err(http11_err)?
        .header("Connection", "close")
        .map_err(http11_err)?;
    for (name, value) in extra_headers {
        request = request.header(*name, value.as_str()).map_err(http11_err)?;
    }
    if let Some((body, content_type)) = body {
        request = request
            .header("Content-Type", content_type)
            .map_err(http11_err)?;
        request = request
            .header("Content-Length", &body.len().to_string())
            .map_err(http11_err)?;
        request = request.body(body.to_vec());
    }
    request.encode().map_err(http11_err)
}

/// ソケットから HTTP/1.1 レスポンスを読み取りデコードする。
fn read_http11_response(
    stream: &mut impl Read,
    method: &str,
    body_limit: BodyLimit,
) -> Result<Response> {
    use crate::core::client::http_decode::ResponseAccumulator;

    let mut acc = ResponseAccumulator::new(method, body_limit);

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
    healthcheck: Option<Healthcheck>,
    hostname: Option<String>,
    open_stdin: Option<bool>,
    network: Option<String>,
}

impl CreateContainerBody {
    fn from_config(config: ContainerConfig) -> Result<Self> {
        let port_bindings = build_port_bindings(&config.ports);
        let exposed_ports = build_exposed_ports(&config.ports);
        let mut binds = Vec::new();
        let mut mounts = Vec::new();
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
                    let source = m.source().ok_or_else(|| {
                        ClientError::Configuration("volume mount source is required".into())
                    })?;
                    let target = m.target().ok_or_else(|| {
                        ClientError::Configuration("volume mount target is required".into())
                    })?;
                    let read_only = m.access_mode() == crate::core::mounts::AccessMode::ReadOnly;
                    mounts.push(format!(
                        "{{\"Type\":\"volume\",\"Source\":{},\"Target\":{},\"ReadOnly\":{}}}",
                        escape_json(source),
                        escape_json(target),
                        read_only
                    ));
                }
                crate::core::mounts::MountType::Tmpfs => {
                    let target = m.target().ok_or_else(|| {
                        ClientError::Configuration("tmpfs mount target is required".into())
                    })?;
                    let read_only = m.access_mode() == crate::core::mounts::AccessMode::ReadOnly;
                    let mut entry = format!(
                        "{{\"Type\":\"tmpfs\",\"Target\":{},\"ReadOnly\":{}",
                        escape_json(target),
                        read_only
                    );
                    if let Some(opts) = m.tmpfs_options() {
                        let mut tmpfs_opts = String::new();
                        if let Some(size) = opts.size_bytes() {
                            tmpfs_opts.push_str(&format!("\"SizeBytes\":{size}"));
                        }
                        if let Some(mode) = opts.mode() {
                            if !tmpfs_opts.is_empty() {
                                tmpfs_opts.push(',');
                            }
                            tmpfs_opts.push_str(&format!("\"Mode\":{mode}"));
                        }
                        if !tmpfs_opts.is_empty() {
                            entry.push_str(&format!(",\"TmpfsOptions\":{{{tmpfs_opts}}}"));
                        }
                    }
                    entry.push('}');
                    mounts.push(entry);
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
                cap_add: config.cap_add,
                cap_drop: config.cap_drop,
                shm_size: config.shm_size,
                readonly_rootfs: config.readonly_rootfs,
                extra_hosts: config.extra_hosts,
                mounts,
            },
            exposed_ports,
            healthcheck: config.health_check,
            hostname: config.hostname,
            open_stdin: config.open_stdin,
            network: config.network,
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
        if let Some(hc_json) = self.healthcheck.as_ref().and_then(|hc| hc.to_docker_json()) {
            json.push_str(",\"Healthcheck\":");
            json.push_str(&hc_json);
        }
        if let Some(hostname) = &self.hostname {
            json.push_str(",\"Hostname\":");
            json.push_str(&escape_json(hostname));
        }
        if self.open_stdin == Some(true) {
            json.push_str(",\"OpenStdin\":true");
        }
        if let Some(network) = &self.network {
            json.push_str(",\"NetworkingConfig\":{\"EndpointsConfig\":{");
            json.push_str(&escape_json(network));
            json.push_str(":{}");
            json.push_str("}}");
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
    cap_add: Vec<String>,
    cap_drop: Vec<String>,
    shm_size: Option<u64>,
    readonly_rootfs: bool,
    extra_hosts: Vec<String>,
    /// Docker Mounts 配列の JSON エントリ (Volume / Tmpfs)。
    mounts: Vec<String>,
}

impl HostConfig {
    fn to_json_string(&self) -> Result<String> {
        let mut json = String::new();
        json.push_str("{\"Privileged\":");
        json.push_str(if self.privileged { "true" } else { "false" });
        json.push_str(",\"Init\":");
        json.push_str(if self.init { "true" } else { "false" });
        json.push_str(",\"ReadonlyRootfs\":");
        json.push_str(if self.readonly_rootfs {
            "true"
        } else {
            "false"
        });
        if !self.binds.is_empty() {
            json.push_str(",\"Binds\":");
            json.push_str(&json_array(&self.binds));
        }
        if !self.port_bindings.is_empty() {
            json.push_str(",\"PortBindings\":");
            json.push_str(&json_port_bindings(&self.port_bindings));
        }
        if !self.cap_add.is_empty() {
            json.push_str(",\"CapAdd\":");
            json.push_str(&json_array(&self.cap_add));
        }
        if !self.cap_drop.is_empty() {
            json.push_str(",\"CapDrop\":");
            json.push_str(&json_array(&self.cap_drop));
        }
        if let Some(shm_size) = self.shm_size {
            json.push_str(",\"ShmSize\":");
            json.push_str(&shm_size.to_string());
        }
        if !self.extra_hosts.is_empty() {
            json.push_str(",\"ExtraHosts\":");
            json.push_str(&json_array(&self.extra_hosts));
        }
        if !self.mounts.is_empty() {
            json.push_str(",\"Mounts\":[");
            json.push_str(&self.mounts.join(","));
            json.push(']');
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
    /// 環境変数。`["KEY=VALUE", ...]` 形式。空の場合は Docker の継承に任せる。
    env: Vec<String>,
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
        if !self.env.is_empty() {
            json.push_str(",\"Env\":");
            json.push_str(&json_array(&self.env));
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

/// Docker exec の multiplexed stream を demux して stdout / stderr に分離する。
///
/// ヘッダ 8 バイト (`stream_type (1B) + reserved (3B) + payload_len (4B big-endian)`)
/// と payload の繰り返し。stream_type 1 = stdout, 2 = stderr。
/// exec の出力はプロセス終了で EOF するため、全蓄積後に一括 demux する。
fn demux_exec_stream(data: &[u8]) -> (Vec<u8>, Vec<u8>) {
    const FRAME_HEADER_LEN: usize = 8;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut pos = 0;
    while pos + FRAME_HEADER_LEN <= data.len() {
        let stream_type = data[pos];
        let payload_len =
            u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
                as usize;
        pos += FRAME_HEADER_LEN;
        let end = (pos + payload_len).min(data.len());
        let chunk = &data[pos..end];
        match stream_type {
            // 1 = stdout, 2 = stderr。0 (stdin) と未知の種別は捨てる。
            1 => stdout.extend_from_slice(chunk),
            2 => stderr.extend_from_slice(chunk),
            _ => {}
        }
        pos = end;
    }
    (stdout, stderr)
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

pub(crate) fn json_array(items: &[String]) -> String {
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

pub(crate) fn escape_json(s: &str) -> String {
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
            health_check: None,
            cap_add: vec![],
            cap_drop: vec![],
            shm_size: None,
            readonly_rootfs: false,
            hostname: None,
            open_stdin: None,
            network: None,
            platform: None,
            extra_hosts: vec![],
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
    fn from_config_volume_mount_is_reflected_in_mounts() {
        // Volume マウントが HostConfig.Mounts に反映されること。
        use crate::core::mounts::Mount;

        let body = CreateContainerBody::from_config(config_with_mounts(vec![Mount::volume_mount(
            "data",
            "/container/data",
        )]))
        .expect("Volume マウントが成功すること");
        let json = body.to_json_string().expect("JSON 出力に失敗した");
        assert!(
            json.contains("\"Type\":\"volume\""),
            "Mounts に volume タイプが含まれること: {json}"
        );
        assert!(
            json.contains("\"Source\":\"data\""),
            "Mounts にソース名が含まれること: {json}"
        );
        assert!(
            json.contains("\"Target\":\"/container/data\""),
            "Mounts にターゲットが含まれること: {json}"
        );
    }

    #[test]
    fn from_config_tmpfs_mount_is_reflected_in_mounts() {
        // Tmpfs マウントが HostConfig.Mounts に反映されること。
        use crate::core::mounts::Mount;

        let body = CreateContainerBody::from_config(config_with_mounts(vec![
            Mount::tmpfs_mount("/tmpfs")
                .with_size_bytes(1_000_000)
                .with_mode(0o1777),
        ]))
        .expect("Tmpfs マウントが成功すること");
        let json = body.to_json_string().expect("JSON 出力に失敗した");
        assert!(
            json.contains("\"Type\":\"tmpfs\""),
            "Mounts に tmpfs タイプが含まれること: {json}"
        );
        assert!(
            json.contains("\"Target\":\"/tmpfs\""),
            "Mounts にターゲットが含まれること: {json}"
        );
        assert!(
            json.contains("\"SizeBytes\":1000000"),
            "TmpfsOptions にサイズが含まれること: {json}"
        );
        assert!(
            json.contains("\"Mode\":1023"),
            "TmpfsOptions にモードが含まれること: {json}"
        );
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
            let result = read_http11_response(&mut reader, "GET", BodyLimit::Unlimited);
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

    /// multiplex フレームのヘルパー: stream_type + payload から 8 バイトヘッダ付きフレームを構築する
    fn make_frame(stream_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![stream_type, 0, 0, 0];
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn demux_exec_stream_separates_stdout_and_stderr() {
        // stdout と stderr が正しく分離されること
        let mut data = make_frame(1, b"hello ");
        data.extend(make_frame(2, b"err "));
        data.extend(make_frame(1, b"world"));
        data.extend(make_frame(2, b"msg"));
        let (stdout, stderr) = demux_exec_stream(&data);
        assert_eq!(stdout, b"hello world", "stdout が正しく結合されること");
        assert_eq!(stderr, b"err msg", "stderr が正しく結合されること");
    }

    #[test]
    fn demux_exec_stream_handles_empty_input() {
        // 空入力は空の stdout / stderr を返すこと
        let (stdout, stderr) = demux_exec_stream(b"");
        assert!(stdout.is_empty(), "空入力では stdout が空であること");
        assert!(stderr.is_empty(), "空入力では stderr が空であること");
    }

    #[test]
    fn demux_exec_stream_ignores_stdin_and_unknown_types() {
        // stdin (0) と未知の種別 (3) は無視されること
        let mut data = make_frame(0, b"stdin");
        data.extend(make_frame(3, b"unknown"));
        data.extend(make_frame(1, b"out"));
        let (stdout, stderr) = demux_exec_stream(&data);
        assert_eq!(stdout, b"out", "stdout のみ取得されること");
        assert!(stderr.is_empty(), "stderr は空であること");
    }

    #[test]
    fn demux_exec_stream_handles_zero_length_payload() {
        // payload_len=0 のフレームはヘッダだけ消費して進むこと
        let mut data = make_frame(1, b"");
        data.extend(make_frame(1, b"data"));
        let (stdout, stderr) = demux_exec_stream(&data);
        assert_eq!(stdout, b"data", "payload_len=0 の後も正しく処理されること");
        assert!(stderr.is_empty());
    }

    #[test]
    fn demux_exec_stream_handles_truncated_frame() {
        // 末尾フレームが切れていても部分出力を返すこと (寛容動作)
        let mut data = make_frame(1, b"hello");
        // 2 フレーム目のヘッダだけ書いて payload を切る
        data.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 10]);
        data.extend_from_slice(b"par");
        let (stdout, stderr) = demux_exec_stream(&data);
        assert_eq!(stdout, b"hello", "完全なフレームは正しく処理されること");
        assert_eq!(stderr, b"par", "切断フレームは部分出力を返すこと");
    }

    #[test]
    fn classify_archive_404_path_not_found_message() {
        // daemon が「パス不存在」のメッセージを返す場合は ContainerPathNotFound になること。
        let body = br#"{"message":"Could not find the file /no/such in container abc123"}"#;
        let err = classify_archive_404("abc123", "/no/such", body);
        assert!(
            matches!(err, ClientError::ContainerPathNotFound(ref p) if p == "/no/such"),
            "ContainerPathNotFound にパスが保持されること: {err:?}"
        );
        assert_eq!(err.to_string(), "container path not found: /no/such");
    }

    #[test]
    fn classify_archive_404_container_not_found_message() {
        // daemon が「コンテナ不存在」のメッセージを返す場合は ContainerNotFound になること。
        let body = br#"{"message":"No such container: abc123"}"#;
        let err = classify_archive_404("abc123", "/etc/hostname", body);
        assert!(
            matches!(err, ClientError::ContainerNotFound(ref id) if id == "abc123"),
            "ContainerNotFound に ID が保持されること: {err:?}"
        );
    }

    #[test]
    fn classify_archive_404_falls_back_on_unknown_message() {
        // 未知文言・空ボディ・非 JSON・message 欠落は ContainerNotFound にフォールバック
        // すること (パス不存在の誤診断を増やさない安全側の設計)。
        for body in [
            &b""[..],
            b"not-json",
            br#"{"error":"something else"}"#,
            br#"{"message":"an unknown daemon message"}"#,
        ] {
            let err = classify_archive_404("abc123", "/etc/hostname", body);
            assert!(
                matches!(err, ClientError::ContainerNotFound(_)),
                "フォールバックは ContainerNotFound であること: {body:?} → {err:?}"
            );
        }
    }

    #[test]
    fn classify_archive_404_prefix_match_not_contains() {
        // パス名に "No such container:" が含まれても、message が "Could not find the file "
        // で始まる限り ContainerPathNotFound になること (contains 誤分類の回帰)。
        let body =
            br#"{"message":"Could not find the file /etc/No such container: x in container abc123"}"#;
        let err = classify_archive_404("abc123", "/etc/No such container: x", body);
        assert!(
            matches!(err, ClientError::ContainerPathNotFound(_)),
            "先頭一致で分類されること: {err:?}"
        );
    }

    #[test]
    fn parse_daemon_error_message_rejects_non_string_and_non_object() {
        // message が文字列でない・トップレベルがオブジェクトでない・非 UTF-8 は None に
        // フォールバックすること (分類の安全側の設計を直接検証)。
        for body in [
            &br#"{"message":123}"#[..],
            b"[1,2,3]",
            b"\"just a string\"",
            &[0xff, 0xfe][..],
        ] {
            assert!(
                parse_daemon_error_message(body).is_none(),
                "不正な message は None になること: {body:?}"
            );
        }
    }

    #[test]
    fn parse_daemon_error_message_extracts_message() {
        // 正常な daemon エラーボディから message が取り出せること。
        assert_eq!(
            parse_daemon_error_message(br#"{"message":"No such container: abc"}"#).as_deref(),
            Some("No such container: abc")
        );
    }

    #[test]
    fn resolve_exec_exit_code_picks_first_finished_state() {
        // パース済み状態列のうち、最初に Running == false になった状態の ExitCode を返すこと。
        // デーモンの状態記録が遅れ、EOF 直後は Running == true のままでも、
        // 後続の試行で Running == false を観測できれば exit code を取得できる。
        let states = [(true, None), (true, None), (false, Some(3))];
        assert_eq!(resolve_exec_exit_code(&states), Some(3));
    }

    #[test]
    fn resolve_exec_exit_code_returns_none_when_always_running() {
        // すべて Running == true のまま打ち切られた場合は None を返すこと。
        let states = [(true, None), (true, None), (true, None)];
        assert_eq!(resolve_exec_exit_code(&states), None);
    }

    #[test]
    fn resolve_exec_exit_code_handles_first_state_finished() {
        // 先頭の状態がすでに Running == false なら即座にその ExitCode を返すこと。
        let states = [(false, Some(0))];
        assert_eq!(resolve_exec_exit_code(&states), Some(0));
    }

    #[test]
    fn exec_exit_code_backoff_sequence_matches_spec() {
        // バックオフ間隔列が仕様どおりの値を持つことを検証する (間隔列・試行回数・
        // 合計 sleep 時間)。ループは `for attempt in 0..=EXEC_EXIT_CODE_BACKOFF_MILLIS.len()`
        // で回り、attempt が列長に達した最終試行の後だけ sleep しない。よって:
        // - 試行回数 = 列の長さ + 1 (初回即時 + 各列値で 1 回ずつの再試行)
        // - 合計 sleep = 列の全要素の合計 (末尾 1000ms も sleep される)
        // このテストは定数と、そこから導出した試行回数・合計のみを検証する
        // (ループ本体は HTTP を伴うためモック禁止規約により直接テストできない)。
        // ループ側は `0..=len` で列長に追従する構造になっており、列長の仕様を
        // ここで固定することで、off-by-one への回帰時に実装と仕様のずれを
        // コードレビューで検出しやすくする。
        assert_eq!(EXEC_EXIT_CODE_BACKOFF_MILLIS, &[10, 50, 200, 500, 1000]);
        let attempts = EXEC_EXIT_CODE_BACKOFF_MILLIS.len() + 1;
        let total_sleep: u64 = EXEC_EXIT_CODE_BACKOFF_MILLIS.iter().sum();
        assert_eq!(
            attempts, 6,
            "試行回数が 6 回であること (初回即時 + 5 回の再試行)"
        );
        assert_eq!(total_sleep, 1760, "合計 sleep が約 1.76 秒であること");
    }

    #[test]
    fn parse_exec_inspect_state_reads_running_and_exit_code() {
        // 正常な inspect 応答から Running / ExitCode が読み取れること。
        let (running, exit_code) = parse_exec_inspect_state(br#"{"Running":false,"ExitCode":7}"#)
            .expect("正常応答はパースできること");
        assert!(!running, "Running == false が読み取れること");
        assert_eq!(exit_code, Some(7), "ExitCode が読み取れること");

        let (running, _exit_code) = parse_exec_inspect_state(br#"{"Running":true,"ExitCode":0}"#)
            .expect("正常応答はパースできること");
        assert!(running, "Running == true が読み取れること");
    }

    #[test]
    fn parse_exec_inspect_state_treats_missing_running_as_finished() {
        // Running フィールド欠落・型不一致は終了扱い (false) として扱うこと (現行挙動)。
        // ExitCode が取れない場合は None になる。
        let (running, exit_code) = parse_exec_inspect_state(br#"{"Running":"yes"}"#)
            .expect("型不一致でも JSON として有効ならパースできること");
        assert!(!running, "型不一致の Running は false 扱いであること");
        assert_eq!(exit_code, None, "ExitCode が無い場合は None であること");

        let (running, _) =
            parse_exec_inspect_state(br#"{}"#).expect("空オブジェクトはパースできること");
        assert!(!running, "Running 欠落は false 扱いであること");
    }

    #[test]
    fn parse_exec_inspect_state_rejects_invalid_json() {
        // 非 UTF-8・JSON パース失敗は ClientError::Json で即エラーになること (従来挙動)。
        // デーモン異常の診断情報を失わないため、終了扱い (false) に倒さない。
        assert!(
            parse_exec_inspect_state(b"\xff\xfe").is_err(),
            "非 UTF-8 は Json エラーになること"
        );
        assert!(
            parse_exec_inspect_state(b"not json").is_err(),
            "JSON パース失敗は Json エラーになること"
        );
    }
}
