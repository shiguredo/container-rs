//! OCI イメージ設定の解決。
//!
//! Apple container の XPC はイメージ既定の CMD/ENTRYPOINT を解決しないため、
//! クライアント側で index → manifest → config の blob を辿って取得する。

use crate::core::{
    client::xpc_client::XpcClient,
    error::{ClientError, Result},
};

/// イメージ config から抽出した CMD/ENTRYPOINT。
#[derive(Debug, Clone, Default)]
pub(crate) struct ImageConfig {
    pub(crate) entrypoint: Option<Vec<String>>,
    pub(crate) cmd: Option<Vec<String>>,
}

impl ImageConfig {
    /// ユーザー指定の entrypoint/cmd とイメージ既定値をマージし、
    /// init プロセスの (executable, arguments) を決める。
    ///
    /// Docker の意味論に従う:
    /// - ユーザー entrypoint 指定時はそれを先頭にし、ユーザー cmd があれば引数、
    ///   無ければイメージ cmd を引数にする。
    /// - ユーザー entrypoint 無し、イメージ entrypoint 有りの場合も同様。
    /// - entrypoint が無く cmd がある場合、cmd の先頭が executable、残りが引数。
    pub(crate) fn effective_command(
        &self,
        entrypoint_override: Option<&str>,
        cmd_override: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<(String, Vec<String>)> {
        let cmd_override: Vec<String> = cmd_override.into_iter().map(Into::into).collect();

        // ユーザー指定 entrypoint が空でなければそれを優先する。
        let entrypoint: Option<Vec<String>> = match entrypoint_override {
            Some(ep) if !ep.is_empty() => Some(vec![ep.to_string()]),
            _ => self.entrypoint.clone(),
        };

        // ユーザー指定 cmd が空でなければそれを優先する。
        let cmd: Option<Vec<String>> = if !cmd_override.is_empty() {
            Some(cmd_override)
        } else {
            self.cmd.clone()
        };

        match (entrypoint, cmd) {
            (Some(ep), Some(cmd)) if !ep.is_empty() && !cmd.is_empty() => {
                let mut args = ep;
                args.extend(cmd);
                let exe = args.remove(0);
                Ok((exe, args))
            }
            (Some(ep), _) if !ep.is_empty() => {
                let mut args = ep;
                let exe = args.remove(0);
                Ok((exe, args))
            }
            (_, Some(cmd)) if !cmd.is_empty() => {
                let mut c = cmd;
                let exe = c.remove(0);
                Ok((exe, c))
            }
            _ => Err(crate::Error::other(
                "no command specified: Apple container XPC does not resolve the image's default \
                 CMD/ENTRYPOINT and none was provided, use with_cmd() or an Image that provides \
                 cmd()",
            )),
        }
    }
}

/// 解決済み descriptor (raw JSON) から OCI image config を取得する。
///
/// `platform` は `linux/arm64` 等。`None` の場合は実行ホストのアーキテクチャに応じる。
pub(crate) async fn resolve_image_config(
    client: &XpcClient,
    desc_raw: &str,
    platform: Option<&str>,
) -> Result<ImageConfig> {
    let desc = parse_descriptor(desc_raw)?;

    let config_digest = match desc.media_type.as_str() {
        "application/vnd.oci.image.index.v1+json"
        | "application/vnd.docker.distribution.manifest.list.v2+json" => {
            let index_bytes = client.content_get(&desc.digest).await?;
            let manifest_digest = select_manifest_digest(&index_bytes, platform)?;
            let manifest_bytes = client.content_get(&manifest_digest).await?;
            config_digest_from_manifest(&manifest_bytes)?
        }
        "application/vnd.oci.image.manifest.v1+json"
        | "application/vnd.docker.distribution.manifest.v2+json" => {
            let manifest_bytes = client.content_get(&desc.digest).await?;
            config_digest_from_manifest(&manifest_bytes)?
        }
        "application/vnd.oci.image.config.v1+json" => desc.digest,
        other => {
            return Err(ClientError::Other(format!(
                "unsupported image descriptor mediaType: {other}"
            ))
            .into());
        }
    };

    let config_bytes = client.content_get(&config_digest).await?;
    parse_image_config(&config_bytes)
}

/// OCI descriptor JSON から mediaType と digest を取り出す。
fn parse_descriptor(desc_raw: &str) -> Result<Descriptor> {
    let parsed = nojson::RawJson::parse(desc_raw)
        .map_err(|e| ClientError::Json(format!("invalid descriptor JSON: {e}")))?;
    let value = parsed.value();
    let media_type: String = value
        .to_member("mediaType")
        .and_then(|m| m.required())
        .and_then(TryInto::<String>::try_into)
        .map_err(|e| ClientError::Json(format!("descriptor mediaType: {e}")))?;
    let digest: String = value
        .to_member("digest")
        .and_then(|m| m.required())
        .and_then(TryInto::<String>::try_into)
        .map_err(|e| ClientError::Json(format!("descriptor digest: {e}")))?;
    Ok(Descriptor { media_type, digest })
}

struct Descriptor {
    media_type: String,
    digest: String,
}

/// index JSON から対象 platform の manifest digest を選ぶ。
///
/// 選定順:
/// 1. 主経路: `os` と `architecture` がともに一致
/// 2. preferred フォールバック: 同じ `os` で arm64 → amd64
/// 3. soft 先頭: `os` だけ一致する先頭 (architecture 欠落も可、attestation 除外)
/// 4. hard 先頭: 先頭の非 attestation manifest (os 不問の最後の手段)
///
/// `platform` が明示指定 (`Some`) の場合は主経路のみで判定し、一致しなければ
/// `no matching manifest for {platform}` エラーを返す (Docker Engine と同じ挙動。
/// フォールバックで要求と異なるアーキテクチャを黙って選ぶのを防ぐ)。
/// フォールバック 2〜4 は `platform` 未指定 (`None`) の場合のみ適用する
/// (実行ホストのアーキテクチャに応じた選択のため)。
///
/// 照合は `target_os_and_architecture` による正規化後の (os, arch) で行う。許可外の
/// arch (例: `linux/arm/v7`) はホスト arch に正規化されるため、`Some` 指定でも
/// ホスト arch の manifest があれば主経路で一致して成功し得る (実経路では
/// `normalize_platform` が許可外を `None` にするため顕在化しない)。
///
/// attestation manifest (`architecture == "unknown"`) は 3・4 のフォールバック経路で
/// 候補から除外する。architecture 欠落は attestation とみなさない。
fn select_manifest_digest(index_bytes: &[u8], platform: Option<&str>) -> Result<String> {
    let text = std::str::from_utf8(index_bytes)
        .map_err(|e| ClientError::Json(format!("index is not UTF-8: {e}")))?;
    let parsed = nojson::RawJson::parse(text)
        .map_err(|e| ClientError::Json(format!("invalid index JSON: {e}")))?;
    let manifests: Vec<nojson::RawJsonValue<'_, '_>> = parsed
        .value()
        .to_member("manifests")
        .and_then(|m| m.required())
        .and_then(|v| v.to_array())
        .map_err(|e| ClientError::Json(format!("index manifests: {e}")))?
        .collect();

    let (target_os, target_arch) = target_os_and_architecture(platform);

    // 主経路: architecture と os の両方が一致する manifest。
    for item in &manifests {
        let Some((os, arch)) = manifest_platform_os_arch(item) else {
            continue;
        };
        if os == target_os && arch == target_arch {
            return manifest_digest(item);
        }
    }

    // 明示 platform 指定時は主経路のみで判定し、一致しなければエラー (フォールバックしない)。
    if let Some(p) = platform {
        return Err(ClientError::Other(format!("no matching manifest for {p}")).into());
    }

    // preferred フォールバック: 同じ os 条件で arm64 → amd64。
    for preferred in ["arm64", "amd64"] {
        for item in &manifests {
            let Some((os, arch)) = manifest_platform_os_arch(item) else {
                continue;
            };
            if os == target_os && arch == preferred {
                return manifest_digest(item);
            }
        }
    }

    // soft 先頭: os が一致する先頭 (architecture 欠落でも候補にする)。
    // attestation manifest (architecture == "unknown") は除外する。
    for item in &manifests {
        let Some(os) = manifest_platform_os(item) else {
            continue;
        };
        if os == target_os && !is_attestation_manifest(item) {
            return manifest_digest(item);
        }
    }

    // hard 先頭: os 不問の最後の手段。attestation manifest は除外する。
    let first = manifests
        .iter()
        .find(|m| !is_attestation_manifest(m))
        .ok_or_else(|| ClientError::Json("index has no valid manifests".into()))?;
    manifest_digest(first)
}

/// manifest エントリの `digest` を取り出す。
fn manifest_digest(item: &nojson::RawJsonValue<'_, '_>) -> Result<String> {
    let digest: String = item
        .to_member("digest")
        .and_then(|m| m.required())
        .and_then(TryInto::<String>::try_into)
        .map_err(|e| ClientError::Json(format!("manifest digest: {e}")))?;
    Ok(digest)
}

/// `platform.os` と `platform.architecture` の両方を取り出す。欠落なら `None`。
fn manifest_platform_os_arch(item: &nojson::RawJsonValue<'_, '_>) -> Option<(String, String)> {
    let platform = item.to_member("platform").ok()?.optional()?;
    let os = platform
        .to_member("os")
        .ok()?
        .required()
        .ok()
        .and_then(|v| TryInto::<String>::try_into(v).ok())?;
    let arch = platform
        .to_member("architecture")
        .ok()?
        .required()
        .ok()
        .and_then(|v| TryInto::<String>::try_into(v).ok())?;
    Some((os, arch))
}

/// `platform.os` だけを取り出す。欠落なら `None`。
fn manifest_platform_os(item: &nojson::RawJsonValue<'_, '_>) -> Option<String> {
    let platform = item.to_member("platform").ok()?.optional()?;
    platform
        .to_member("os")
        .ok()?
        .required()
        .ok()
        .and_then(|v| TryInto::<String>::try_into(v).ok())
}

/// attestation manifest (architecture == "unknown") かどうかを判定する。
///
/// Docker buildx が生成する provenance attestation manifest の platform は
/// `{ "architecture": "unknown", "os": "unknown" }` である。architecture フィールドの
/// 欠落は attestation とみなさない (通常の manifest として候補に残す)。
fn is_attestation_manifest(item: &nojson::RawJsonValue<'_, '_>) -> bool {
    manifest_platform_architecture(item).as_deref() == Some("unknown")
}

/// `platform.architecture` だけを取り出す。欠落なら `None`。
fn manifest_platform_architecture(item: &nojson::RawJsonValue<'_, '_>) -> Option<String> {
    let platform = item.to_member("platform").ok()?.optional()?;
    platform
        .to_member("architecture")
        .ok()?
        .required()
        .ok()
        .and_then(|v| TryInto::<String>::try_into(v).ok())
}

/// manifest JSON から config descriptor の digest を取り出す。
fn config_digest_from_manifest(manifest_bytes: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(manifest_bytes)
        .map_err(|e| ClientError::Json(format!("manifest is not UTF-8: {e}")))?;
    let parsed = nojson::RawJson::parse(text)
        .map_err(|e| ClientError::Json(format!("invalid manifest JSON: {e}")))?;
    let digest: String = parsed
        .value()
        .to_member("config")
        .and_then(|m| m.required())
        .and_then(|c| c.to_member("digest"))
        .and_then(|m| m.required())
        .and_then(TryInto::<String>::try_into)
        .map_err(|e| ClientError::Json(format!("manifest config.digest: {e}")))?;
    Ok(digest)
}

/// image config JSON から Entrypoint / Cmd を取り出す。
fn parse_image_config(config_bytes: &[u8]) -> Result<ImageConfig> {
    let text = std::str::from_utf8(config_bytes)
        .map_err(|e| ClientError::Json(format!("image config is not UTF-8: {e}")))?;
    let parsed = nojson::RawJson::parse(text)
        .map_err(|e| ClientError::Json(format!("invalid image config JSON: {e}")))?;
    let config = parsed
        .value()
        .to_member("config")
        .ok()
        .and_then(|m| m.optional());

    let entrypoint = config.and_then(|c| string_array(&c, "Entrypoint"));
    let cmd = config.and_then(|c| string_array(&c, "Cmd"));

    Ok(ImageConfig { entrypoint, cmd })
}

/// JSON オブジェクトの配列メンバーを `Vec<String>` として取り出す。
fn string_array(value: &nojson::RawJsonValue, key: &str) -> Option<Vec<String>> {
    let arr = value.to_member(key).ok()?.optional()?;
    arr.to_array()
        .ok()?
        .map(|v| TryInto::<String>::try_into(v).ok())
        .collect()
}

/// platform 文字列 (`linux/arm64` / `linux/arm64/v8` / `amd64` など) から
/// `(os, OCI architecture)` を取り出す。
///
/// - 要素 2 以上: `os = parts[0]`、`arch = parts[1]` (variant 以降は無視)
/// - 要素 1: `os = "linux"`、`arch = parts[0]`
/// - `None`: `os = "linux"`、arch はホスト
///
/// arch の正規化は `"arm64"|"aarch64"` → `"arm64"`、`"amd64"|"x86_64"` → `"amd64"`。
/// 既知 alias 外はホスト arch へフォールバックする。
fn target_os_and_architecture(platform: Option<&str>) -> (&str, &'static str) {
    let (os, raw_arch) = platform_os_and_raw_arch(platform);
    let arch = match raw_arch {
        "" => host_architecture(),
        "arm64" | "aarch64" => "arm64",
        "amd64" | "x86_64" => "amd64",
        _ => host_architecture(),
    };
    (os, arch)
}

/// platform 文字列を `(os, raw_arch)` に分解する。正規化はしない。
///
/// `None` のとき raw_arch は空文字 (呼び出し側でホスト arch にフォールバックする)。
fn platform_os_and_raw_arch(platform: Option<&str>) -> (&str, &str) {
    match platform {
        None => ("linux", ""),
        Some(p) => {
            let mut parts = p.split('/');
            let first = parts.next().unwrap_or("");
            match parts.next() {
                Some(arch) => (first, arch),
                None => ("linux", first),
            }
        }
    }
}

/// 実行ホストの OCI architecture 値。
fn host_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        _ => "arm64",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_command_uses_user_entrypoint_and_user_cmd() {
        // ユーザー指定 entrypoint と cmd がある場合、entrypoint が先頭に入り cmd は引数になる。
        let cfg = ImageConfig {
            entrypoint: Some(vec!["/bin/sh".into(), "-c".into()]),
            cmd: Some(vec!["echo".into(), "image".into()]),
        };
        let (exe, args) = cfg
            .effective_command(Some("/bin/bash"), ["-c", "user"])
            .expect("処理に失敗しないこと");
        assert_eq!(exe, "/bin/bash");
        assert_eq!(args, vec!["-c", "user"]);
    }

    #[test]
    fn effective_command_appends_image_cmd_to_image_entrypoint() {
        // イメージ entrypoint があり cmd もある場合、cmd は entrypoint の引数になる。
        let cfg = ImageConfig {
            entrypoint: Some(vec!["/bin/sh".into(), "-c".into()]),
            cmd: Some(vec!["echo".into(), "hello".into()]),
        };
        let (exe, args) = cfg
            .effective_command(None, std::iter::empty::<String>())
            .expect("処理に失敗しないこと");
        assert_eq!(exe, "/bin/sh");
        assert_eq!(args, vec!["-c", "echo", "hello"]);
    }

    #[test]
    fn effective_command_uses_image_cmd_when_no_entrypoint() {
        // entrypoint が無く cmd だけある場合、cmd の先頭が executable になる。
        let cfg = ImageConfig {
            entrypoint: None,
            cmd: Some(vec!["nginx".into(), "-g".into(), "daemon off;".into()]),
        };
        let (exe, args) = cfg
            .effective_command(None, std::iter::empty::<String>())
            .expect("処理に失敗しないこと");
        assert_eq!(exe, "nginx");
        assert_eq!(args, vec!["-g", "daemon off;"]);
    }

    #[test]
    fn effective_command_prefers_user_cmd_over_image_cmd() {
        // ユーザー cmd があればイメージ cmd は上書きされる。
        let cfg = ImageConfig {
            entrypoint: None,
            cmd: Some(vec!["image-cmd".into()]),
        };
        let (exe, args) = cfg
            .effective_command(None, ["user-cmd"])
            .expect("処理に失敗しないこと");
        assert_eq!(exe, "user-cmd");
        assert_eq!(args, Vec::<String>::new());
    }

    #[test]
    fn effective_command_errors_when_nothing_available() {
        let cfg = ImageConfig::default();
        assert!(
            cfg.effective_command(None, std::iter::empty::<String>())
                .is_err()
        );
    }

    #[test]
    fn parse_descriptor_extracts_media_type_and_digest() {
        let raw = r#"{"mediaType":"application/vnd.oci.image.index.v1+json","digest":"sha256:abc","size":123}"#;
        let desc = parse_descriptor(raw).expect("処理に失敗しないこと");
        assert_eq!(desc.media_type, "application/vnd.oci.image.index.v1+json");
        assert_eq!(desc.digest, "sha256:abc");
    }

    #[test]
    fn select_manifest_digest_matches_platform() {
        let index = r#"{"manifests":[
            {"digest":"sha256:amd64","platform":{"architecture":"amd64","os":"linux"}},
            {"digest":"sha256:arm64","platform":{"architecture":"arm64","os":"linux"}}
        ]}"#;
        let digest = select_manifest_digest(index.as_bytes(), Some("linux/arm64"))
            .expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:arm64");
    }

    #[test]
    fn select_manifest_digest_defaults_to_host_arch() {
        let index = r#"{"manifests":[
            {"digest":"sha256:amd64","platform":{"architecture":"amd64","os":"linux"}},
            {"digest":"sha256:arm64","platform":{"architecture":"arm64","os":"linux"}}
        ]}"#;
        // platform 未指定時はホスト arch。このテストは ARM64 ホスト (Apple Silicon /
        // CI の self-hosted runner) を前提とし、arm64 が主経路で選ばれることを検証する
        // (x86_64 ホストでは主経路で先頭の amd64 が選ばれるためこの期待値は成立しない)。
        let digest = select_manifest_digest(index.as_bytes(), None).expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:arm64");
    }

    #[test]
    fn platform_os_and_raw_arch_uses_second_component_as_arch() {
        // variant 付きでも arch は index 1
        assert_eq!(
            platform_os_and_raw_arch(Some("linux/arm64/v8")),
            ("linux", "arm64")
        );
        assert_eq!(
            target_os_and_architecture(Some("linux/arm64/v8")),
            ("linux", "arm64")
        );
        // 1 要素は os 既定 linux
        assert_eq!(platform_os_and_raw_arch(Some("amd64")), ("linux", "amd64"));
        // 未対応 arch はそのまま raw に残る
        assert_eq!(
            platform_os_and_raw_arch(Some("linux/arm/v7")),
            ("linux", "arm")
        );
    }

    #[test]
    fn select_manifest_digest_ignores_variant_suffix() {
        // variant 付きでも arch を誤らず arm64 側を選ぶ
        let index = r#"{"manifests":[
            {"digest":"sha256:amd64","platform":{"architecture":"amd64","os":"linux"}},
            {"digest":"sha256:arm64","platform":{"architecture":"arm64","os":"linux"}}
        ]}"#;
        let digest = select_manifest_digest(index.as_bytes(), Some("linux/arm64/v8"))
            .expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:arm64");
    }

    #[test]
    fn select_manifest_digest_matches_os_on_primary_path() {
        // windows/amd64 が先でも linux/amd64 を選ぶ
        let index = r#"{"manifests":[
            {"digest":"sha256:win-amd64","platform":{"architecture":"amd64","os":"windows"}},
            {"digest":"sha256:linux-amd64","platform":{"architecture":"amd64","os":"linux"}}
        ]}"#;
        let digest = select_manifest_digest(index.as_bytes(), Some("linux/amd64"))
            .expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:linux-amd64");
    }

    #[test]
    fn select_manifest_digest_matches_os_on_preferred_fallback() {
        // 主経路不一致 → preferred の arm64 は windows をスキップ → linux/amd64
        // platform 未指定 (None) のときだけフォールバックが適用されること。
        let index = r#"{"manifests":[
            {"digest":"sha256:win-arm64","platform":{"architecture":"arm64","os":"windows"}},
            {"digest":"sha256:linux-amd64","platform":{"architecture":"amd64","os":"linux"}}
        ]}"#;
        let digest = select_manifest_digest(index.as_bytes(), None).expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:linux-amd64");
    }

    #[test]
    fn select_manifest_digest_soft_first_matches_os() {
        // preferred 失敗後、linux 側の先頭 (s390x) を選ぶ (platform 未指定時のみ)。
        let index = r#"{"manifests":[
            {"digest":"sha256:win-arm64","platform":{"architecture":"arm64","os":"windows"}},
            {"digest":"sha256:win-amd64","platform":{"architecture":"amd64","os":"windows"}},
            {"digest":"sha256:linux-s390x","platform":{"architecture":"s390x","os":"linux"}}
        ]}"#;
        let digest = select_manifest_digest(index.as_bytes(), None).expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:linux-s390x");
    }

    #[test]
    fn select_manifest_digest_single_component_defaults_os_linux() {
        let index = r#"{"manifests":[
            {"digest":"sha256:amd64","platform":{"architecture":"amd64","os":"linux"}},
            {"digest":"sha256:arm64","platform":{"architecture":"arm64","os":"linux"}}
        ]}"#;
        let digest =
            select_manifest_digest(index.as_bytes(), Some("amd64")).expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:amd64");
    }

    #[test]
    fn select_manifest_digest_unknown_arch_stays_on_linux() {
        // linux/arm/v7 は raw arch=arm (既知外) でホストへフォールバックするが、
        // windows 先頭の fixture では linux 側のいずれかを選ぶこと。
        assert_eq!(
            platform_os_and_raw_arch(Some("linux/arm/v7")),
            ("linux", "arm")
        );
        let index = r#"{"manifests":[
            {"digest":"sha256:win-amd64","platform":{"architecture":"amd64","os":"windows"}},
            {"digest":"sha256:linux-arm64","platform":{"architecture":"arm64","os":"linux"}},
            {"digest":"sha256:linux-amd64","platform":{"architecture":"amd64","os":"linux"}}
        ]}"#;
        let digest = select_manifest_digest(index.as_bytes(), Some("linux/arm/v7"))
            .expect("処理に失敗しないこと");
        assert!(
            digest == "sha256:linux-arm64" || digest == "sha256:linux-amd64",
            "windows を選ばず linux 側であること: {digest}"
        );
    }

    #[test]
    fn select_manifest_digest_skips_entries_missing_os() {
        // architecture のみのエントリはスキップし、正常 linux/amd64 を選ぶ
        let index = r#"{"manifests":[
            {"digest":"sha256:arch-only","platform":{"architecture":"amd64"}},
            {"digest":"sha256:linux-amd64","platform":{"architecture":"amd64","os":"linux"}}
        ]}"#;
        let digest = select_manifest_digest(index.as_bytes(), Some("linux/amd64"))
            .expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:linux-amd64");
    }

    #[test]
    fn config_digest_from_manifest_extracts_config_digest() {
        let manifest = r#"{"schemaVersion":2,"config":{"mediaType":"application/vnd.oci.image.config.v1+json","digest":"sha256:cfg","size":1}}"#;
        let digest =
            config_digest_from_manifest(manifest.as_bytes()).expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:cfg");
    }

    #[test]
    fn parse_image_config_extracts_entrypoint_and_cmd() {
        let config = r#"{"config":{"Entrypoint":["/bin/sh","-c"],"Cmd":["echo","hi"]}}"#;
        let cfg = parse_image_config(config.as_bytes()).expect("処理に失敗しないこと");
        assert_eq!(cfg.entrypoint, Some(vec!["/bin/sh".into(), "-c".into()]));
        assert_eq!(cfg.cmd, Some(vec!["echo".into(), "hi".into()]));
    }

    #[test]
    fn parse_image_config_handles_missing_fields() {
        let config = r#"{"config":{}}"#;
        let cfg = parse_image_config(config.as_bytes()).expect("処理に失敗しないこと");
        assert_eq!(cfg.entrypoint, None);
        assert_eq!(cfg.cmd, None);
    }

    #[test]
    fn select_manifest_digest_skips_attestation_at_head() {
        // attestation manifest (os/arch ともに unknown) が先頭にあっても、
        // 正常な linux/amd64 manifest が選ばれること
        let index = r#"{"manifests":[
            {"digest":"sha256:attestation","platform":{"architecture":"unknown","os":"unknown"}},
            {"digest":"sha256:linux-amd64","platform":{"architecture":"amd64","os":"linux"}}
        ]}"#;
        let digest = select_manifest_digest(index.as_bytes(), Some("linux/amd64"))
            .expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:linux-amd64");
    }

    #[test]
    fn select_manifest_digest_errors_when_all_attestation() {
        // 全 manifest が attestation の場合はエラーになること (platform 未指定時のみ)。
        let index = r#"{"manifests":[
            {"digest":"sha256:att1","platform":{"architecture":"unknown","os":"unknown"}},
            {"digest":"sha256:att2","platform":{"architecture":"unknown","os":"unknown"}}
        ]}"#;
        let err = select_manifest_digest(index.as_bytes(), None)
            .expect_err("全 attestation はエラーであること");
        assert!(
            err.to_string().contains("no valid manifests"),
            "エラーメッセージに no valid manifests が含まれること: {err}"
        );
    }

    #[test]
    fn select_manifest_digest_soft_first_skips_unknown_arch() {
        // soft 先頭経路で os: "linux", architecture: "unknown" のエントリがスキップされ、
        // 正常な linux/s390x が選ばれること (platform 未指定時のみ)。
        let index = r#"{"manifests":[
            {"digest":"sha256:linux-unknown","platform":{"architecture":"unknown","os":"linux"}},
            {"digest":"sha256:linux-s390x","platform":{"architecture":"s390x","os":"linux"}}
        ]}"#;
        let digest = select_manifest_digest(index.as_bytes(), None).expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:linux-s390x");
    }

    #[test]
    fn select_manifest_digest_hard_first_skips_attestation() {
        // hard 先頭経路で attestation manifest がスキップされ、
        // 別 os の非 attestation manifest が選ばれること (platform 未指定時のみ)。
        let index = r#"{"manifests":[
            {"digest":"sha256:attestation","platform":{"architecture":"unknown","os":"unknown"}},
            {"digest":"sha256:win-amd64","platform":{"architecture":"amd64","os":"windows"}}
        ]}"#;
        let digest = select_manifest_digest(index.as_bytes(), None).expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:win-amd64");
    }

    #[test]
    fn select_manifest_digest_soft_first_keeps_missing_arch() {
        // soft 先頭経路で platform.architecture が欠落しているエントリは
        // attestation とみなされず、引き続き選択されること (platform 未指定時のみ)。
        // preferred フォールバック (arm64/amd64) に一致するエントリを置かず、
        // soft 先頭経路に到達させる。
        let index = r#"{"manifests":[
            {"digest":"sha256:linux-no-arch","platform":{"os":"linux"}},
            {"digest":"sha256:linux-s390x","platform":{"architecture":"s390x","os":"linux"}}
        ]}"#;
        let digest = select_manifest_digest(index.as_bytes(), None).expect("処理に失敗しないこと");
        assert_eq!(digest, "sha256:linux-no-arch");
    }

    #[test]
    fn select_manifest_digest_errors_when_explicit_platform_has_no_match() {
        // 明示 platform 指定 (Some) で主経路に一致する manifest が無い場合は、
        // フォールバックせず no matching manifest エラーになること。
        // Docker Engine と同じ挙動 (要求と異なるアーキテクチャを黙って選ばない)。
        let index = r#"{"manifests":[
            {"digest":"sha256:linux-arm64","platform":{"architecture":"arm64","os":"linux"}},
            {"digest":"sha256:linux-s390x","platform":{"architecture":"s390x","os":"linux"}}
        ]}"#;
        let err = select_manifest_digest(index.as_bytes(), Some("linux/amd64"))
            .expect_err("amd64 が無い明示指定はエラーになること");
        assert!(
            err.to_string()
                .contains("no matching manifest for linux/amd64"),
            "エラーメッセージに platform が含まれること: {err}"
        );

        // hard フォールバック (os 不問) も抑止されること。旧実装では win-amd64 が
        // os 不問の最後の手段として選ばれていた。
        let win_only = r#"{"manifests":[
            {"digest":"sha256:win-amd64","platform":{"architecture":"amd64","os":"windows"}}
        ]}"#;
        let err = select_manifest_digest(win_only.as_bytes(), Some("linux/amd64"))
            .expect_err("linux/amd64 が無い明示指定は os 不問でもエラーになること");
        assert!(
            err.to_string()
                .contains("no matching manifest for linux/amd64"),
            "hard フォールバック抑止のエラーであること: {err}"
        );
    }
}
