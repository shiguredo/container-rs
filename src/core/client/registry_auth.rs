//! Docker レジストリ認証。
//!
//! `DOCKER_AUTH_CONFIG` / `DOCKER_CONFIG` / `~/.docker/config.json` から
//! 静的エントリ (`auths`) のみを読む。credential helper は対象外。

// JSON 文字列値のエスケープは docker_client 側の共通実装を再利用する
// (実装とテストは docker_client に集約)。
use crate::core::client::docker_client::escape_json_value;

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
/// 優先順: `DOCKER_AUTH_CONFIG` > `DOCKER_CONFIG/config.json` > `~/.docker/config.json`。
///
/// `DOCKER_CONFIG` が非空で設定されている場合、docker CLI (`config.Dir()`) と同じく
/// そのディレクトリのみを参照し、`~/.docker/config.json` へフォールバックしない
/// (config.json が読めなければ認証なしで返す)。`DOCKER_CONFIG=""` は未設定と同じ扱い
/// (docker CLI 準拠)。
fn load_config_json() -> Option<String> {
    // 環境変数 DOCKER_AUTH_CONFIG (config.json 相当の JSON 文字列)
    if let Ok(json) = std::env::var("DOCKER_AUTH_CONFIG")
        && !json.is_empty()
    {
        return Some(json);
    }

    // DOCKER_CONFIG ディレクトリ配下の config.json。
    // 非空で設定されている場合のみ有効とし、そのディレクトリだけを見る
    // (~/.docker へフォールバックしない)。空文字は未設定と同じ扱い。
    if let Ok(dir) = std::env::var("DOCKER_CONFIG")
        && !dir.is_empty()
    {
        let path = std::path::Path::new(&dir).join("config.json");
        return std::fs::read_to_string(&path).ok();
    }

    // ~/.docker/config.json (DOCKER_CONFIG が未設定・空文字の場合のみ)
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

    // 親テストから子プロセスへ「load_config_json 子テストとして起動されたこと」を伝える。
    // 環境変数はプロセスグローバルなため、`DOCKER_CONFIG` / `HOME` の書き換えを親でやると
    // 並列実行中の他テストと競合する (Rust 2024 では UB)。既存の DOCKER_AUTH_CONFIG
    // テストと同じ子プロセス分離方式で、環境変数を子側にだけセットする。
    const LOAD_CONFIG_TEST_CHILD_ENV: &str = "SHIGUREDO_CONTAINER_LOAD_CONFIG_TEST_CHILD";

    // 子テストが実際に検証を実行したことを親に示すマーカー。
    // libtest の `--exact <name>` は該当テスト 0 件でも exit 0 を返すため、
    // 子テスト名にタイポがあると親 assert が silent-pass する。この文字列を
    // 子側 stdout に出力し、親がそれを検出することで silent-pass を防ぐ。
    const LOAD_CONFIG_CHILD_MARKER: &str = "__LOAD_CONFIG_CHILD_RAN__";

    /// テスト用の一時ディレクトリ名に付けるサフィックスを生成する。
    ///
    /// `core::util` モジュールは macOS 限定 (`src/core.rs` の `#[cfg(target_os = "macos")]`)
    /// のため、Linux 専用の本モジュールから `unique_suffix` を参照できない。並列テスト
    /// 間で名前が衝突しないよう、プロセス ID・ナノ秒・アトミックカウンタで一意性を
    /// 担保する (`unique_suffix` と同じ方針の再実装)。
    fn test_unique_suffix() -> String {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{}-{}-{}", std::process::id(), nanos, count)
    }

    /// テストの後始末を保証するための Drop ガード。
    ///
    /// 一時ディレクトリを作った直後に `TempDirGuard::new` で包むと、
    /// `spawn_load_config_child` の assert が panic した場合でも Drop で
    /// `remove_dir_all` を呼び、`/tmp` に残骸を蓄積させない。
    /// テスト内で明示的な remove を書かなくて済む。
    struct TempDirGuard(std::path::PathBuf);

    impl TempDirGuard {
        /// `tag` は一時ディレクトリ名に埋め込まれる識別子で、
        /// `container-rs-load-config-test-{tag}-{unique_suffix}` の形になる。
        /// テストを識別できる短い kebab-case を渡す (例: `"docker-over-home"`)。
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "container-rs-load-config-test-{tag}-{}",
                test_unique_suffix()
            ));
            std::fs::create_dir_all(&dir).expect("一時ディレクトリの作成に失敗した");
            Self(dir)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `DOCKER_CONFIG` 環境変数の状態を 3 通りで表す。
    ///
    /// `spawn_load_config_child` に渡して、子プロセスの環境をこの状態にセットする。
    /// `Option<&Path>` では表現できない「非空で空文字」を含める。
    enum DockerConfigArg<'a> {
        /// `env_remove("DOCKER_CONFIG")` (親環境から継承しない)。
        Unset,
        /// `env("DOCKER_CONFIG", "")` (空文字。docker CLI 準拠で未設定と同じ扱い)。
        Empty,
        /// `env("DOCKER_CONFIG", path)`。
        Set(&'a std::path::Path),
    }

    /// 子テストを起動して結果を検証する共通ヘルパ。
    ///
    /// - `docker_config` は `DockerConfigArg` の 3 状態 (Unset / Empty / Set(path))
    /// - `home` は `Some(path)` でセット、`None` で `env_remove` する
    /// - 親環境の `DOCKER_AUTH_CONFIG` は常に除去する (認証優先順位を壊さないため)
    /// - `--exact <name>` の 0 マッチ silent-pass を防ぐため、子側 stdout に
    ///   `LOAD_CONFIG_CHILD_MARKER` が出力されていることを親側で必ず検証する
    fn spawn_load_config_child(
        child_name: &str,
        docker_config: DockerConfigArg<'_>,
        home: Option<&std::path::Path>,
    ) {
        let executable = std::env::current_exe().expect("テストバイナリのパスを取得できること");
        let mut cmd = std::process::Command::new(executable);
        // `--nocapture` を必ず付ける: libtest はデフォルトで各テストの stdout を
        // 内部バッファに取り込み成功時は破棄するため、`println!` によるマーカーが
        // 親側の `output.stdout` に流れない。`--nocapture` で fd 1 に直接流す。
        cmd.args(["--exact", child_name, "--nocapture"])
            .env(LOAD_CONFIG_TEST_CHILD_ENV, "1")
            .env_remove("DOCKER_AUTH_CONFIG");
        match docker_config {
            DockerConfigArg::Unset => {
                cmd.env_remove("DOCKER_CONFIG");
            }
            DockerConfigArg::Empty => {
                cmd.env("DOCKER_CONFIG", "");
            }
            DockerConfigArg::Set(p) => {
                cmd.env("DOCKER_CONFIG", p);
            }
        }
        match home {
            Some(p) => {
                cmd.env("HOME", p);
            }
            None => {
                cmd.env_remove("HOME");
            }
        }
        let output = cmd.output().expect("子テストを起動できること");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "子テストが成功すること: status={}, stdout={stdout}, stderr={stderr}",
            output.status
        );
        // 子テスト名が --exact にマッチしなかった (タイポ等) 場合、libtest は
        // 0 件マッチでも exit 0 を返すため status.success() だけでは silent-pass する。
        // 子テストが実際に検証パスに入ったことをマーカーで確認する。
        assert!(
            stdout.contains(LOAD_CONFIG_CHILD_MARKER),
            "子テストが検証パスに入ったこと (0 マッチ silent-pass 防止): child={child_name}, stdout={stdout}"
        );
    }

    #[test]
    fn load_config_json_reads_docker_config_dir_over_home() {
        // DOCKER_CONFIG が指定されていれば ~/.docker/config.json より優先されること。
        let base = TempDirGuard::new("docker-over-home");
        let docker_config = base.path().join("docker_config");
        std::fs::create_dir(&docker_config).expect("docker_config ディレクトリの作成に失敗した");
        std::fs::write(docker_config.join("config.json"), b"from-docker-config")
            .expect("docker_config の config.json 書き込みに失敗した");
        let home = base.path().join("home");
        std::fs::create_dir_all(home.join(".docker")).expect("home/.docker の作成に失敗した");
        std::fs::write(home.join(".docker/config.json"), b"from-home")
            .expect("home 側の config.json 書き込みに失敗した");

        spawn_load_config_child(
            "core::client::registry_auth::tests::load_config_json_reads_docker_config_dir_over_home_child",
            DockerConfigArg::Set(&docker_config),
            Some(&home),
        );
    }

    #[test]
    fn load_config_json_reads_docker_config_dir_over_home_child() {
        if std::env::var_os(LOAD_CONFIG_TEST_CHILD_ENV).is_none() {
            return;
        }
        println!("{LOAD_CONFIG_CHILD_MARKER}");
        let actual = load_config_json();
        assert_eq!(
            actual.as_deref(),
            Some("from-docker-config"),
            "DOCKER_CONFIG 指定時はその config.json が優先されること (実際: {actual:?})"
        );
    }

    #[test]
    fn load_config_json_none_when_docker_config_missing_and_no_home_fallback() {
        // DOCKER_CONFIG 指定 (非空) + そこに config.json が無い場合、
        // ~/.docker/config.json にフォールバックせず None を返すこと (docker CLI 準拠)。
        let base = TempDirGuard::new("no-fallback");
        let docker_config = base.path().join("docker_config");
        // 意図的に config.json は作らない (空ディレクトリ)。
        std::fs::create_dir(&docker_config).expect("docker_config ディレクトリの作成に失敗した");
        let home = base.path().join("home");
        std::fs::create_dir_all(home.join(".docker")).expect("home/.docker の作成に失敗した");
        // ~/.docker/config.json は存在するが、フォールバックしないため読まれないはず。
        std::fs::write(home.join(".docker/config.json"), b"from-home")
            .expect("home 側の config.json 書き込みに失敗した");

        spawn_load_config_child(
            "core::client::registry_auth::tests::load_config_json_none_when_docker_config_missing_and_no_home_fallback_child",
            DockerConfigArg::Set(&docker_config),
            Some(&home),
        );
    }

    #[test]
    fn load_config_json_none_when_docker_config_missing_and_no_home_fallback_child() {
        if std::env::var_os(LOAD_CONFIG_TEST_CHILD_ENV).is_none() {
            return;
        }
        println!("{LOAD_CONFIG_CHILD_MARKER}");
        let actual = load_config_json();
        assert_eq!(
            actual, None,
            "DOCKER_CONFIG 指定 (非空) + config.json 不在なら None を返し ~/.docker/config.json にフォールバックしないこと (実際: {actual:?})"
        );
    }

    #[test]
    fn load_config_json_reads_home_when_docker_config_unset() {
        // DOCKER_CONFIG が未設定なら ~/.docker/config.json を読むこと (従来挙動)。
        let base = TempDirGuard::new("home-fallback");
        let home = base.path().join("home");
        std::fs::create_dir_all(home.join(".docker")).expect("home/.docker の作成に失敗した");
        std::fs::write(home.join(".docker/config.json"), b"from-home")
            .expect("home 側の config.json 書き込みに失敗した");

        spawn_load_config_child(
            "core::client::registry_auth::tests::load_config_json_reads_home_when_docker_config_unset_child",
            DockerConfigArg::Unset,
            Some(&home),
        );
    }

    #[test]
    fn load_config_json_reads_home_when_docker_config_unset_child() {
        if std::env::var_os(LOAD_CONFIG_TEST_CHILD_ENV).is_none() {
            return;
        }
        println!("{LOAD_CONFIG_CHILD_MARKER}");
        let actual = load_config_json();
        assert_eq!(
            actual.as_deref(),
            Some("from-home"),
            "DOCKER_CONFIG 未設定時は ~/.docker/config.json が読まれること (実際: {actual:?})"
        );
    }

    #[test]
    fn load_config_json_treats_empty_docker_config_as_unset() {
        // DOCKER_CONFIG="" は未設定と同じ扱い (docker CLI 準拠)。
        // ~/.docker/config.json にフォールバックする。
        let base = TempDirGuard::new("empty-docker-config");
        let home = base.path().join("home");
        std::fs::create_dir_all(home.join(".docker")).expect("home/.docker の作成に失敗した");
        std::fs::write(home.join(".docker/config.json"), b"from-home")
            .expect("home 側の config.json 書き込みに失敗した");

        spawn_load_config_child(
            "core::client::registry_auth::tests::load_config_json_treats_empty_docker_config_as_unset_child",
            DockerConfigArg::Empty,
            Some(&home),
        );
    }

    #[test]
    fn load_config_json_treats_empty_docker_config_as_unset_child() {
        if std::env::var_os(LOAD_CONFIG_TEST_CHILD_ENV).is_none() {
            return;
        }
        println!("{LOAD_CONFIG_CHILD_MARKER}");
        let actual = load_config_json();
        assert_eq!(
            actual.as_deref(),
            Some("from-home"),
            "DOCKER_CONFIG=\"\" は未設定と同じ扱いで ~/.docker/config.json が読まれること (実際: {actual:?})"
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
