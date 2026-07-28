# 機能追加: Linux で exec の env_vars を実装する

- Priority: Medium
- Created: 2026-07-29
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-exec-env-vars
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで `ExecCommand::with_env_vars` を実装する。

## 優先度根拠

exec への環境変数設定はコンテナ内コマンドの動作制御に必要。現状は非空なら明示エラーを返すため、環境変数が必要なテストは代替手段が必要。0017 (exec の stdout/stderr 取得) と同じ `DockerClient::exec` を触るが、論理的依存はない。Medium。

## 現状

- `ExecCommand::with_env_vars` は非空なら Linux で明示エラー (`src/core/containers/async_container.rs:354-359`)
- `ExecConfig` (`src/core/client/docker_client.rs:752-756`) に `Env` フィールド自体が存在しない
- `DockerClient::exec` のシグネチャは `exec(&self, id: &str, cmd: &[String])` で env vars を受け取る引数がない
- macOS 実装 (`async_container.rs:337-348`) は `ContainerRequest.env_vars()` と `ExecCommand.env_vars` を `BTreeMap` でマージしている (XPC がコンテナ env を継承しないため)
- `tests/container_linux.rs:239-264` の `exec_unsupported_options_return_err` テストが `with_env_vars` の Err を明示的にアサートしている

## 設計方針

- `DockerClient::exec` のシグネチャを `exec(&self, id: &str, cmd: &[String], env: &[(String, String)])` のように変更し、`ExecConfig` に `Env` フィールドを追加する
- Docker Engine API は `Env` 省略時にコンテナ env を継承するが、`Env` を指定した場合はコンテナ env を**置換**する。macOS との対称性を保つため、コンテナ env を inspect (`GET /containers/{id}/json`) の `Config.Env` から取得し、`ExecCommand.env_vars` で上書きマージした結果を `ExecConfig.Env` に設定する (macOS のマージ経路 `async_container.rs:337-348` と同じセマンティクス)
- `Env` の JSON 形式は `["KEY=VALUE", ...]` の文字列配列 (既存のコンテナ作成経路 `docker_client.rs:681-684` と同じ)

## 完了条件

- [ ] Linux で `ExecCommand::with_env_vars` が環境変数を exec に渡すこと
- [ ] `tests/container_linux.rs` の `exec_unsupported_options_return_err` から `with_env_vars` の Err 期待を削除すること
- [ ] 統合テストが追加されていること (最低限: `with_env_vars` で渡した変数が exec プロセスから見えること)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の env_vars 関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
