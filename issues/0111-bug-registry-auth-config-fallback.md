# バグ: DOCKER_CONFIG 指定時に ~/.docker/config.json へフォールバックする

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-registry-auth-config-fallback
- Polished: {YYYY-MM-DD}

## 目的

`DOCKER_CONFIG` 環境変数が設定されているのにその場所に config.json が無い場合に、docker CLI と異なる場所 (`~/.docker/config.json`) を参照してしまうのをなくす。

## 現状

`src/core/client/registry_auth.rs` の認証情報解決は、`DOCKER_CONFIG` で指定された config.json が読めない場合に `~/.docker/config.json` へフォールバックする。

```rust
if let Ok(dir) = std::env::var("DOCKER_CONFIG") {
    let path = std::path::Path::new(&dir).join("config.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        return Some(content);
    }
}
// ~/.docker/config.json
```

- docker CLI は `DOCKER_CONFIG` が設定されているとき、そのディレクトリのみを参照し `~/.docker` へフォールバックしない
- `DOCKER_CONFIG` が設定済み (意図的に別ディレクトリを指定) なのにその場所にファイルが無い場合、`~/.docker/config.json` の認証情報が使われ、意図しないレジストリ認証が行われる (または逆に、無視されるべき場所の情報で認証される)

## 設計方針

- `DOCKER_CONFIG` が設定されている場合は、その場所で読めなければフォールバックせず認証なし (`None`) にする
- docker CLI の挙動に合わせる旨をコメントで明記する

## 完了条件

- `DOCKER_CONFIG` 指定時に `~/.docker/config.json` が参照されないこと
- `DOCKER_CONFIG` 未指定時の `~/.docker/config.json` 参照は従来どおり動作すること
