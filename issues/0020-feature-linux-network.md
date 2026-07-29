# 機能追加: Linux で with_network を対応する

- Priority: Medium
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-network
- Polished: 2026-07-29

## 目的

Linux (Docker Engine API) バックエンドで `with_network` を Docker の NetworkingConfig に反映する。

## 現状

- `with_network`: `ContainerRequest` には保存されるが、`linux_unsupported_request_reason` (`src/runners/async_runner.rs`) が `build_container_config` の呼び出しより前に**明示エラーで拒否**している ("with_network() is not implemented on Linux")
- Linux 用 `build_container_config` (`src/runners/async_runner.rs`) は `ContainerConfig` に network をマッピングしていない
- `CreateContainerBody` (`src/core/client/docker_client.rs`) に `NetworkingConfig` は存在しない
- macOS では XPC `containerCreate` の `networks[0].network` に反映されている。未指定時は `"default"` を明示指定する (自動作成はしない)

## 設計方針

- `linux_unsupported_request_reason` (`src/runners/async_runner.rs`) から `network` のガードを削除する
- `ContainerConfig` (`src/core/client.rs`) に `network: Option<String>` を追加する
- `build_container_config` (`src/runners/async_runner.rs`) で `ContainerRequest` から値をマッピングする
- `CreateContainerBody` (`src/core/client/docker_client.rs`) に `network: Option<String>` フィールドを追加し、`from_config` で `ContainerConfig` から値を受け渡し、`to_json_string` で `NetworkingConfig.EndpointsConfig.{network}: {}` を JSON に出力する。`network` が `None` の場合は `NetworkingConfig` 自体を省略する (Docker がデフォルト bridge に接続する。macOS の `"default"` 明示指定とは異なる)
- `ContainerConfig` のフィールド追加に伴い、テストヘルパー `config_with_mounts` (`src/core/client/docker_client.rs`) にも `network` フィールドのデフォルト値を追加する
- macOS と同様にネットワークの自動作成は行わない (事前に `docker network create` が必要)
- 存在しないネットワークを指定した場合は Docker 側がエラーを返す (現状の `create_container` のエラー伝播で十分)

## 完了条件

- [ ] Linux で `with_network` が Docker の NetworkingConfig に反映されること
- [ ] 存在しないネットワーク指定時に Docker のエラーが伝播すること
- [ ] 統合テストが追加されていること (最低限: `build_container_config` の出力に network が反映されること、`to_json_string` の出力に network 指定時は `NetworkingConfig` が含まれ `None` 時は省略されることの検証)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
