//! Docker レジストリ認証。
//!
//! `DOCKER_AUTH_CONFIG` / `DOCKER_CONFIG` / `~/.docker/config.json` から
//! 静的エントリ (`auths`) のみを読む。credential helper は対象外。

/// イメージ参照から auths の参照キーを抽出する。
///
/// Docker Hub の場合は `https://index.docker.io/v1/`、
/// それ以外は先頭コンポーネント (レジストリホスト) を返す。
pub(crate) fn auths_key(descriptor: &str) -> String {
    let first = descriptor.split('/').next().unwrap_or("");
    if first.contains('.') || first.contains(':') || first == "localhost" {
        first.to_string()
    } else {
        "https://index.docker.io/v1/".to_string()
    }
}

/// Docker の認証設定を読み込み、指定イメージの X-Registry-Auth ヘッダ値を返す。
///
/// 認証が見つからない場合は `None` を返す (公開レジストリ)。
pub(crate) fn x_registry_auth(descriptor: &str) -> Option<String> {
    let config_json = load_config_json()?;
    let key = auths_key(descriptor);
    let auth_entry = extract_auth_entry(&config_json, &key)?;
    Some(auth_entry)
}

/// 認証設定の JSON 文字列を読み込む。
///
/// 優先順: DOCKER_AUTH_CONFIG > DOCKER_CONFIG/config.json > ~/.docker/config.json
fn load_config_json() -> Option<String> {
    // 環境変数 DOCKER_AUTH_CONFIG (config.json 相当の JSON 文字列)
    if let Ok(json) = std::env::var("DOCKER_AUTH_CONFIG")
        && !json.is_empty()
    {
        return Some(json);
    }

    // DOCKER_CONFIG ディレクトリ配下の config.json
    if let Ok(dir) = std::env::var("DOCKER_CONFIG") {
        let path = std::path::Path::new(&dir).join("config.json");
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some(content);
        }
    }

    // ~/.docker/config.json
    if let Some(home) = home_dir() {
        let path = home.join(".docker").join("config.json");
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some(content);
        }
    }

    None
}

/// ホームディレクトリを取得する。
fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// config.json の auths から指定キーのエントリを抽出し、
/// X-Registry-Auth ヘッダ用の base64 エンコード JSON を返す。
fn extract_auth_entry(config_json: &str, key: &str) -> Option<String> {
    use base64ct::{Base64, Encoding};

    // config.json をパースして auths.{key}.auth または auths.{key}.identitytoken を取得する。
    let parsed = nojson::RawJson::parse(config_json).ok()?;
    let value = parsed.value();

    // auths オブジェクトを取得する。
    let auths_value = value.to_member("auths").and_then(|m| m.required()).ok()?;

    // 指定キーのエントリを取得する。
    let entry_value = auths_value.to_member(key).and_then(|m| m.required()).ok()?;

    // identitytoken がある場合はそれを優先する。
    if let Ok(token_value) = entry_value
        .to_member("identitytoken")
        .and_then(|m| m.required())
        && let Ok(token_str) = TryInto::<String>::try_into(token_value)
        && !token_str.is_empty()
    {
        let header_json = format!(
            "{{\"identitytoken\":\"{}\"}}",
            escape_json_value(&token_str)
        );
        let encoded = Base64::encode_string(header_json.as_bytes());
        return Some(encoded);
    }

    // auth フィールド (base64 エンコードされた username:password)
    let auth_value = entry_value
        .to_member("auth")
        .and_then(|m| m.required())
        .ok()?;
    let auth_str: String = TryInto::<String>::try_into(auth_value).ok()?;
    if auth_str.is_empty() {
        return None;
    }

    // base64 デコードして username:password に分割する。
    let decoded_bytes = Base64::decode_vec(&auth_str).ok()?;
    let decoded = String::from_utf8(decoded_bytes).ok()?;
    let (username, password) = decoded.split_once(':')?;

    // X-Registry-Auth ヘッダ用の JSON を構築する。
    let serveraddress = if key == "https://index.docker.io/v1/" {
        "https://index.docker.io/v1/"
    } else {
        key
    };
    let header_json = format!(
        "{{\"username\":\"{}\",\"password\":\"{}\",\"serveraddress\":\"{}\"}}",
        escape_json_value(username),
        escape_json_value(password),
        escape_json_value(serveraddress)
    );
    let encoded = Base64::encode_string(header_json.as_bytes());
    Some(encoded)
}

/// JSON 文字列値をエスケープする (最小限: バックスラッシュとダブルクォート)。
fn escape_json_value(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auths_key_docker_hub_for_plain_image() {
        // Docker Hub のイメージは https://index.docker.io/v1/ を返すこと。
        assert_eq!(auths_key("alpine:latest"), "https://index.docker.io/v1/");
        assert_eq!(auths_key("library/nginx"), "https://index.docker.io/v1/");
    }

    #[test]
    fn auths_key_private_registry() {
        // プライベートレジストリは先頭コンポーネントを返すこと。
        assert_eq!(auths_key("ghcr.io/org/app:tag"), "ghcr.io");
        assert_eq!(auths_key("localhost:5000/myimage"), "localhost:5000");
        assert_eq!(
            auths_key("registry.example.com/img"),
            "registry.example.com"
        );
    }

    #[test]
    fn extract_auth_entry_decodes_base64_auth() {
        // auth フィールドの base64 デコードとヘッダ構築を検証する。
        use base64ct::{Base64, Encoding};

        let credentials = Base64::encode_string(b"user:pass");
        let config =
            format!(r#"{{"auths":{{"https://index.docker.io/v1/":{{"auth":"{credentials}"}}}}}}"#);
        let result = extract_auth_entry(&config, "https://index.docker.io/v1/");
        assert!(result.is_some(), "認証エントリが取得できること");

        // 結果を base64 デコードして JSON 構造を検証する。
        let decoded = Base64::decode_vec(&result.expect("Some")).expect("デコードできること");
        let json_str = String::from_utf8(decoded).expect("UTF-8 であること");
        assert!(
            json_str.contains("\"username\":\"user\""),
            "username が含まれること: {json_str}"
        );
        assert!(
            json_str.contains("\"password\":\"pass\""),
            "password が含まれること: {json_str}"
        );
    }

    #[test]
    fn extract_auth_entry_returns_none_for_missing_key() {
        // 存在しないキーは None を返すこと。
        let config = r#"{"auths":{"ghcr.io":{"auth":"dXNlcjpwYXNz"}}}"#;
        assert!(extract_auth_entry(config, "https://index.docker.io/v1/").is_none());
    }

    #[test]
    fn extract_auth_entry_returns_none_for_empty_auths() {
        // auths が空の場合は None を返すこと。
        let config = r#"{"auths":{}}"#;
        assert!(extract_auth_entry(config, "https://index.docker.io/v1/").is_none());
    }
}
