//! Docker レジストリ認証。
//!
//! `DOCKER_AUTH_CONFIG` / `DOCKER_CONFIG` / `~/.docker/config.json` から
//! 静的エントリ (`auths`) のみを読む。credential helper は対象外。

/// Docker Hub の auths 参照キー。docker login が config.json に書き込む形式。
const DOCKER_HUB_AUTH_KEY: &str = "https://index.docker.io/v1/";

/// イメージ参照から auths の参照キーを抽出する。
///
/// Docker Hub の場合は `https://index.docker.io/v1/`、
/// それ以外は先頭コンポーネント (レジストリホスト) を返す。
/// `/` を含まない参照 (例: `alpine:latest`) は常に Docker Hub。
///
/// 先頭コンポーネントが `docker.io` / `index.docker.io` の場合は
/// docker CLI の `getAuthConfigKey` と同じく Docker Hub のキーに正規化する
/// (この 2 ドメインのみが対象。`registry-1.docker.io` やポート付きホストは
/// 正規化しない)。
pub(crate) fn auths_key(descriptor: &str) -> String {
    // split は空文字でも常に 1 要素以上を返すため、ここは到達しない。
    let first = descriptor
        .split('/')
        .next()
        .expect("split は必ず 1 要素以上を返すため到達しない");
    // `/` で分割して 2 コンポーネント以上ある場合のみホスト判定する。
    if descriptor.contains('/')
        && (first.contains('.') || first.contains(':') || first == "localhost")
    {
        // docker login が config.json に書き込む Hub のキーは DOCKER_HUB_AUTH_KEY
        // であり、そのまま `docker.io` をキーにすると認証ヘッダに拾えないため。
        if first == "docker.io" || first == "index.docker.io" {
            DOCKER_HUB_AUTH_KEY.to_string()
        } else {
            first.to_string()
        }
    } else {
        DOCKER_HUB_AUTH_KEY.to_string()
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
    // serveraddress には照合に使ったキーをそのまま使う (auths_key が正規化済みの値を返す)。
    let header_json = format!(
        "{{\"username\":\"{}\",\"password\":\"{}\",\"serveraddress\":\"{}\"}}",
        escape_json_value(username),
        escape_json_value(password),
        escape_json_value(key)
    );
    let encoded = Base64::encode_string(header_json.as_bytes());
    Some(encoded)
}

/// JSON 文字列値をエスケープする (引用符なし。呼び出し側の `format!` で包む)。
///
/// バックスラッシュ・ダブルクォートに加え、JSON 文字列内でエスケープ必須の制御文字
/// (0x00-0x1F) を処理する。短縮エスケープ (`\b` / `\f` / `\n` / `\r` / `\t`) と、
/// それ以外の `< 0x20` は `\uXXXX` 化する。`docker_client::escape_json` と同じ
/// エスケープロジック (制御文字を含む)。
fn escape_json_value(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
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
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64ct::{Base64, Encoding};

    // 親テストから子プロセスへ「子として起動されたこと」を伝える環境変数。
    // wait モジュールの run_env_case と同じ分離方式で、環境変数を直接書き換えずに済ます。
    const DOCKER_HUB_TEST_CHILD_ENV: &str = "SHIGUREDO_CONTAINER_REGISTRY_AUTH_TEST_CHILD";

    #[test]
    fn auths_key_docker_hub_for_plain_image() {
        // Docker Hub のイメージは https://index.docker.io/v1/ を返すこと。
        assert_eq!(auths_key("alpine:latest"), "https://index.docker.io/v1/");
        assert_eq!(auths_key("library/nginx"), "https://index.docker.io/v1/");
    }

    #[test]
    fn auths_key_docker_hub_normalizes_docker_io_references() {
        // docker.io / index.docker.io 形式は Docker Hub のキーに正規化すること。
        assert_eq!(
            auths_key("docker.io/org/private"),
            "https://index.docker.io/v1/"
        );
        assert_eq!(
            auths_key("index.docker.io/org/private"),
            "https://index.docker.io/v1/"
        );
        assert_eq!(
            auths_key("docker.io/library/nginx:latest"),
            "https://index.docker.io/v1/"
        );
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
    fn auths_key_does_not_normalize_docker_hub_subdomains() {
        // doc コメントで正規化対象外と明記したケースを固定する。
        // registry-1.docker.io は Docker Hub の実 API ホストだが、
        // docker CLI と同じく正規化しない。
        assert_eq!(
            auths_key("registry-1.docker.io/org/img"),
            "registry-1.docker.io"
        );
        // ポート付きホストも正規化しない (プライベートレジストリ扱い)。
        assert_eq!(auths_key("docker.io:5000/img"), "docker.io:5000");
    }

    #[test]
    fn extract_auth_entry_decodes_base64_auth() {
        // auth フィールドの base64 デコードとヘッダ構築を検証する。
        let credentials = Base64::encode_string(b"user:pass");
        let config =
            format!(r#"{{"auths":{{"https://index.docker.io/v1/":{{"auth":"{credentials}"}}}}}}"#);
        let result = extract_auth_entry(&config, "https://index.docker.io/v1/");
        assert!(result.is_some(), "認証エントリが取得できること");

        // 結果を base64 デコードして JSON 構造を検証する。
        let decoded = Base64::decode_vec(&result.expect("認証エントリを取得できたこと"))
            .expect("デコードできること");
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
    fn x_registry_auth_picks_docker_hub_entry_for_docker_io_reference() {
        // 環境変数はプロセスグローバルなため、親プロセスで書き換えると
        // 並列実行中の他テストが読む environ と競合する (Rust 2024 では UB)。
        // 既存の wait モジュールと同じく、子プロセスに DOCKER_AUTH_CONFIG を
        // 渡してから子テストで検証する。
        let credentials = Base64::encode_string(b"user:pass");
        let config =
            format!(r#"{{"auths":{{"https://index.docker.io/v1/":{{"auth":"{credentials}"}}}}}}"#);
        let executable = std::env::current_exe().expect("テストバイナリのパスを取得できること");
        let status = std::process::Command::new(executable)
            .args([
                "--exact",
                "core::client::registry_auth::tests::x_registry_auth_docker_hub_child",
            ])
            .env(DOCKER_HUB_TEST_CHILD_ENV, "1")
            .env("DOCKER_AUTH_CONFIG", config)
            .status()
            .expect("環境変数を渡す子テストを起動できること");
        assert!(status.success(), "子テストが成功すること: {status}");
    }

    #[test]
    fn x_registry_auth_docker_hub_child() {
        // 親テストから専用フラグを渡された場合のみ検証する。
        // 通常のテスト実行ではスキップし、子プロセスとして起動されたときだけ
        // DOCKER_AUTH_CONFIG を読んで検証する。
        if std::env::var_os(DOCKER_HUB_TEST_CHILD_ENV).is_none() {
            return;
        }
        let result = x_registry_auth("docker.io/org/private")
            .expect("docker.io 形式でも Docker Hub エントリを取得できること");
        let decoded = Base64::decode_vec(&result).expect("デコードできること");
        let json_str = String::from_utf8(decoded).expect("UTF-8 であること");
        assert!(
            json_str.contains("\"username\":\"user\""),
            "username が含まれること: {json_str}"
        );
        assert!(
            json_str.contains("\"password\":\"pass\""),
            "password が含まれること: {json_str}"
        );
        assert!(
            json_str.contains("\"serveraddress\":\"https://index.docker.io/v1/\""),
            "serveraddress が含まれること: {json_str}"
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

    #[test]
    fn escape_json_value_escapes_control_characters() {
        // 制御文字が JSON 文字列内で合法なエスケープになること。
        // 短縮エスケープ (\n / \r / \t 等) と \uXXXX 化の両系統を検証する。
        assert_eq!(escape_json_value("a\nb"), "a\\nb");
        assert_eq!(escape_json_value("a\rb"), "a\\rb");
        assert_eq!(escape_json_value("a\tb"), "a\\tb");
        assert_eq!(escape_json_value("a\u{0008}b"), "a\\bb");
        assert_eq!(escape_json_value("a\u{000c}b"), "a\\fb");
        // 短縮エスケープの無い制御文字は \uXXXX になる。
        assert_eq!(escape_json_value("a\u{0001}b"), "a\\u0001b");
        // バックスラッシュとダブルクォートもエスケープされる。
        assert_eq!(escape_json_value("a\"b\\c"), "a\\\"b\\\\c");
        // 通常文字はそのまま。
        assert_eq!(escape_json_value("plain"), "plain");
    }

    #[test]
    fn escape_json_value_roundtrips_through_nojson() {
        // エスケープ済み文字列が JSON 文字列値としてパースできること (往復)。
        // 制御文字の代表ケースに加え、境界値 (\u0000 / \u001f)・短縮エスケープ
        // 全種・空文字列を検証する。
        for s in [
            "",
            "a\nb",
            "a\rb",
            "a\tb",
            "a\u{0008}b",
            "a\u{000c}b",
            "a\u{0000}b",
            "a\u{001f}b",
            "a\u{0001}b",
            "a\"b\\c",
            "plain",
        ] {
            let escaped = escape_json_value(s);
            let json = format!("\"{escaped}\"");
            let parsed = nojson::RawJson::parse(&json)
                .expect("escape_json_value の出力は JSON 文字列としてパースできること");
            let value = String::try_from(parsed.value()).expect("JSON 文字列値として読めること");
            assert_eq!(value, s, "往復で元の文字列が復元されること");
        }
    }

    #[test]
    fn extract_auth_entry_with_control_chars_in_credentials() {
        // auth フィールド経路: 資格情報 (username / password) に制御文字が含まれても、
        // 生成される X-Registry-Auth の JSON が構文として有効であること。
        // base64 経由で config.json に auth フィールドを埋め込む。
        let credentials = Base64::encode_string("user\nline:pa\tss\u{0001}".as_bytes());
        let config =
            format!(r#"{{"auths":{{"https://index.docker.io/v1/":{{"auth":"{credentials}"}}}}}}"#);
        let result = extract_auth_entry(&config, "https://index.docker.io/v1/")
            .expect("認証エントリが取得できること");

        // 生成されたヘッダ JSON がパース可能 (構文として有効) であること。
        let decoded = Base64::decode_vec(&result).expect("デコードできること");
        let parsed =
            nojson::RawJson::parse(std::str::from_utf8(&decoded).expect("UTF-8 であること"))
                .expect("制御文字入り資格情報でもヘッダ JSON が有効であること");
        let value = parsed.value();
        let username: String = value
            .to_member("username")
            .and_then(|m| m.required())
            .ok()
            .and_then(|v| TryInto::<String>::try_into(v).ok())
            .expect("username が復元できること");
        let password: String = value
            .to_member("password")
            .and_then(|m| m.required())
            .ok()
            .and_then(|v| TryInto::<String>::try_into(v).ok())
            .expect("password が復元できること");
        assert_eq!(
            username, "user\nline",
            "改行を含む username が復元されること"
        );
        assert_eq!(
            password, "pa\tss\u{0001}",
            "タブと制御文字を含む password が復元されること"
        );
    }

    #[test]
    fn extract_auth_entry_with_control_chars_in_identitytoken() {
        // identitytoken 経路: トークンに制御文字が含まれても、生成される
        // X-Registry-Auth の JSON が構文として有効であること。
        // config.json の identitytoken は JSON エスケープ表記 (\n 等) で書く
        // (nojson は生の制御文字を拒否するため)。
        let config =
            r#"{"auths":{"https://index.docker.io/v1/":{"identitytoken":"tok\nen\u0001"}}}"#;
        let result = extract_auth_entry(config, "https://index.docker.io/v1/")
            .expect("identitytoken が取得できること");

        // identitytoken 経路は {"identitytoken":"..."} 形式の JSON を返す。
        let decoded = Base64::decode_vec(&result).expect("デコードできること");
        let parsed =
            nojson::RawJson::parse(std::str::from_utf8(&decoded).expect("UTF-8 であること"))
                .expect("制御文字入り identitytoken でもヘッダ JSON が有効であること");
        let token: String = parsed
            .value()
            .to_member("identitytoken")
            .and_then(|m| m.required())
            .ok()
            .and_then(|v| TryInto::<String>::try_into(v).ok())
            .expect("identitytoken が復元できること");
        assert_eq!(
            token, "tok\nen\u{0001}",
            "改行と制御文字を含む identitytoken が復元されること"
        );
    }
}
