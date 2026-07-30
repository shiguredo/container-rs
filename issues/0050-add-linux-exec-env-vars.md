# 機能追加: Linux で exec の env_vars を実装する

- Priority: Medium
- Created: 2026-07-29
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-exec-env-vars
- Polished: 2026-07-30

## 目的

Linux (Docker Engine API) バックエンドで `ExecCommand::with_env_vars` を実装する。

## 現状

- `ExecCommand::with_env_vars` は非空なら Linux で明示エラー (`async_container.rs` の `exec` 関数内 Linux 分岐の `if !_env_vars.is_empty()` ブロック)
- `ExecConfig` (`docker_client.rs` の `ExecConfig` 構造体) に `Env` フィールド自体が存在しない
- `DockerClient::exec` のシグネチャは `exec(&self, id: &str, cmd: &[String])` で env vars を受け取る引数がない
- macOS 実装 (`async_container.rs` の `exec` 関数内 macOS 分岐) は `ContainerRequest.env_vars()` と `ExecCommand.env_vars` を `BTreeMap` でマージしている (XPC がコンテナ env を継承しないため)
- `tests/container_linux.rs` の `exec_unsupported_options_return_err` テストが `with_env_vars` の Err を明示的にアサートしている

## 設計方針

- `DockerClient::exec` のシグネチャを `exec(&self, id: &str, cmd: &[String], env: &[(String, String)])` のように変更し、`ExecConfig` に `Env` フィールドを追加する
- Docker Engine API は `Env` 省略時にコンテナ env を継承するが、`Env` を指定した場合はコンテナ env を**置換**する。そのため、コンテナ env を inspect (`GET /containers/{id}/json`) の `Config.Env` から取得し、`ExecCommand.env_vars` で上書きマージした結果を `ExecConfig.Env` に設定する。`env_vars` が空の場合は現行どおり `Env` フィールドを送信しない (Docker の継承に任せる)
- macOS 経路の基底は `self.image.env_vars()`（`ContainerRequest` に設定したリクエスト時の env）であり、Linux 経路の基底は inspect の `Config.Env`（イメージの ENV 命令を含む実行時の全 env）である。データソースは異なるが、「コンテナの全環境変数 + exec 分の上書き」というセマンティクスは共通する
- inspect の呼び出しは `async_container.rs` 側で行い、マージ済み env を `DockerClient::exec` に渡す。既存の `DockerClient::container_state` は `Config.Env` をパースしないため、新規メソッド（例: `container_env`）の追加または `container_state` の拡張が必要
- `Env` の JSON 形式は `["KEY=VALUE", ...]` の文字列配列 (既存のコンテナ作成経路 `ContainerConfig::to_json_string` 内の `Env` 出力と同じ)
- 0017 (exec の stdout/stderr 取得) も同一関数 `DockerClient::exec` と `ExecConfig` 構造体を変更する。論理的依存はないが、実装順序によってはマージコンフリクト解消が必要

## 完了条件

- [ ] Linux で `ExecCommand::with_env_vars` が環境変数を exec に渡すこと
- [ ] `tests/container_linux.rs` の `exec_unsupported_options_return_err` から `with_env_vars` の Err 期待を削除すること
- [ ] 統合テストが追加されていること (最低限: `with_env_vars` で渡した変数が exec プロセスから見えること)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の env_vars 関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
