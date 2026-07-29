# 機能追加: Linux で with_host を対応する (ExtraHosts)

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-extra-hosts
- Polished: 2026-07-29

## 目的

Linux (Docker Engine API) バックエンドで `with_host` を Docker の HostConfig.ExtraHosts に反映する。

## 現状

- `with_host`: `ContainerRequest` には保存されるが、`linux_unsupported_request_reason` (`src/runners/async_runner.rs`) が `build_container_config` の呼び出しより前に**明示エラーで拒否**している ("with_host() is not implemented on Linux")
- Linux 用 `build_container_config` (`src/runners/async_runner.rs`) は `ContainerConfig` に extra_hosts をマッピングしていない
- `HostConfig` (`src/core/client/docker_client.rs`) に `ExtraHosts` フィールドは存在しない
- macOS では `ExtraHost::Addr` をコンテナ起動後に exec で `/etc/hosts` に追記している (`apply_extra_hosts`, `src/runners/async_runner.rs`)。`ExtraHost::HostGateway` は明示エラー
- `ExtraHost` には `Display` 実装 (`src/core/containers/request.rs`) があり、`Addr` は IP アドレスを、`HostGateway` は `"host-gateway"` を出力する

## 設計方針

- `linux_unsupported_request_reason` (`src/runners/async_runner.rs`) から `hosts` のガードを削除する
- `ContainerConfig` (`src/core/client.rs`) に `extra_hosts: Vec<String>` を追加する
- `build_container_config` (`src/runners/async_runner.rs`) で `ContainerRequest::hosts()` から `ExtraHost` の `Display` 実装を使い `"hostname:ip"` / `"hostname:host-gateway"` 形式の文字列を生成する
- `HostConfig` (`src/core/client/docker_client.rs`) に `extra_hosts: Vec<String>` フィールドを追加し、`from_config` で `ContainerConfig` から値を受け渡し、`to_json_string` で `ExtraHosts` を JSON に出力する。空 Vec の場合は省略する
- `ContainerConfig` のフィールド追加に伴い、テストヘルパー `config_with_mounts` (`src/core/client/docker_client.rs`) にも `extra_hosts` フィールドのデフォルト値を追加する

## 完了条件

- [ ] Linux で `with_host("myhost", ExtraHost::Addr(...))` が Docker の HostConfig.ExtraHosts に反映されること
- [ ] Linux で `with_host("myhost", ExtraHost::HostGateway)` が `host-gateway` として反映されること
- [ ] 統合テストが追加されていること (最低限: `build_container_config` の出力に extra_hosts が反映されることの検証)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
