# バグ: DOCKER_CONFIG 指定時に ~/.docker/config.json へフォールバックする

- Created: 2026-08-12
- Completed: 2026-08-18
- Branch: feature/fix-registry-auth-config-fallback
- Polished: 2026-08-12

## 目的

`DOCKER_CONFIG` 環境変数が設定されているのにその場所に config.json が無い場合に、docker CLI と異なる場所 (`~/.docker/config.json`) を参照してしまうのをなくす。

## 現状

`src/core/client/registry_auth.rs` の `load_config_json` は、`DOCKER_CONFIG` で指定された config.json が読めない場合に `~/.docker/config.json` へフォールバックする (優先順位は `DOCKER_AUTH_CONFIG` > `DOCKER_CONFIG` > `~/.docker` の 3 段階)。

```rust
if let Ok(dir) = std::env::var("DOCKER_CONFIG") {
    let path = std::path::Path::new(&dir).join("config.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        return Some(content);
    }
}
// ~/.docker/config.json
```

- docker CLI は `DOCKER_CONFIG` が設定されているとき、そのディレクトリのみを参照し `~/.docker` へフォールバックしない (docker CLI の `config.Dir()` は DOCKER_CONFIG 非空ならそのディレクトリのみ。config.json 不在時は認証なしで返す。testcontainers-rs 0.27.3 の `read_docker_auth_config` も同様で、0034 の「本家に揃える」方針からも現在のフォールバックは乖離している)
- `DOCKER_CONFIG` が設定済みなのにその場所にファイルが無い場合、`~/.docker/config.json` の認証情報が使われ、意図しないレジストリ認証が行われる

## 設計方針

- `DOCKER_CONFIG` が設定されている (非空) 場合は、その場所で読めなければ (ファイル不在・パーミッション等) フォールバックせず認証なし (`None`) にする
- `DOCKER_AUTH_CONFIG` が設定されている (非空) 場合は従来どおり最優先で返す (今回の修正対象外。修正は `DOCKER_CONFIG` 設定時にファイルが読めない場合のフォールバック停止のみ)
- 空文字 `DOCKER_CONFIG=""` は docker CLI と同じく未設定として扱い、`~/.docker/config.json` へフォールバックする (現行の「空文字でカレントディレクトリの config.json を読む」挙動も修正される。なお testcontainers-rs 0.27.3 の `read_docker_auth_config` は空文字でカレントディレクトリを読むため、この空文字の扱いは本家と異なる docker CLI 準拠の判断)
- docker CLI の挙動に合わせる旨をコメントで明記する

## 完了条件

- `DOCKER_CONFIG` 指定 (非空) 時に `~/.docker/config.json` が参照されないこと
- `DOCKER_CONFIG` 指定 (非空) + その場所に config.json がある場合は、その場所が参照されること
- `DOCKER_CONFIG` 指定 (非空) + その場所に config.json が無い (または読めない) 場合は `None` になること
- `DOCKER_CONFIG` 未設定時の `~/.docker/config.json` 参照は従来どおり動作すること
- 上記の検証は子プロセス分離方式の単体テストで行うこと (HOME を一時ディレクトリへ差し替え、親環境の `DOCKER_AUTH_CONFIG` は `env_remove` で排除する)
- 修正で陳腐化する `skills/shiguredo-container/SKILL.md` の認証情報探索の記述が更新されること
- `CHANGES.md` に `[FIX]` エントリが記載されること

## 解決方法

- `src/core/client/registry_auth.rs` の `load_config_json` を修正し、`DOCKER_CONFIG` が非空で設定されている場合はそのディレクトリのみを参照し、`read_to_string` が失敗しても `~/.docker/config.json` へフォールバックしないようにした (docker CLI の `config.Dir()` 相当の挙動)。`DOCKER_CONFIG=""` (空文字) は未設定と同じ扱いで `~/.docker/config.json` を読む
- 単体テストは子プロセス分離方式で 4 ケースを検証: DOCKER_CONFIG 指定 (非空) + config.json 存在 → その config.json / DOCKER_CONFIG 指定 (非空) + config.json 不在 → None (~/.docker へフォールバックしない) / DOCKER_CONFIG 未設定 → ~/.docker/config.json / DOCKER_CONFIG="" → 未設定と同じ扱い
- 子プロセス起動時に親環境の `DOCKER_AUTH_CONFIG` を `env_remove` で必ず排除する。子側 stdout にマーカー (`__LOAD_CONFIG_CHILD_RAN__`) を出力し親側で検出することで、libtest の `--exact` 0 マッチ silent-pass を防ぐ (子起動 args に `--nocapture` を付けて libtest の stdout capture を回避)
- `DOCKER_CONFIG` の 3 状態 (未設定 / 空文字 / 非空) を `DockerConfigArg` enum で扱い、テストヘルパ `spawn_load_config_child` に統合した
- 一時ディレクトリの後始末は `TempDirGuard` (Drop 実装) で管理し、テスト panic 時も `/tmp` に残骸を残さない
- `skills/shiguredo-container/SKILL.md` の認証情報探索の記述に、`DOCKER_CONFIG` 指定時のフォールバック挙動と空文字の扱いを追記した
- `CHANGES.md` の develop セクションに `[FIX]` エントリを追加した

### 未対応 (別 issue 化候補)

- 既存の `x_registry_auth_docker_hub_child` テストも同じ silent-pass 問題を抱える。共通ヘルパ化 or marker 対応の水平展開は本 PR の範囲外
