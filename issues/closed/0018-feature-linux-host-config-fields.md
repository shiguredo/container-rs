# 機能追加: Linux で HostConfig 未反映項目を対応する (cap_add/cap_drop/shm_size/readonly_rootfs)

- Priority: Medium
- Created: 2026-07-21
- Completed: 2026-08-01
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-host-config-fields
- Polished: 2026-07-29

## 目的

Linux (Docker Engine API) バックエンドで `ContainerRequest` に保存されているが Docker の create JSON に未反映の HostConfig 項目を対応する。

## 優先度根拠

`cap_add` / `cap_drop` はセキュリティテスト、`shm_size` は共有メモリを使うアプリケーション (PostgreSQL 等)、`readonly_rootfs` はセキュリティ強化テストで必要。いずれも Docker Engine API の HostConfig に直接対応するフィールドがあり、実装は容易。Medium。

## 現状

- `with_cap_add` / `with_cap_drop` / `with_shm_size` / `with_readonly_rootfs`: `ContainerRequest` には保存されるが、`linux_unsupported_request_reason` (`src/runners/async_runner.rs:625-636`) が `build_container_config` の呼び出しより前にこれらを**明示エラーで拒否**している ("not implemented on Linux")
- Linux 用 `build_container_config` (`src/runners/async_runner.rs:499-513`) は `ContainerConfig` にこれらを映射していない
- 現在の `HostConfig` (`src/core/client/docker_client.rs:720-725`) は `port_bindings` / `binds` / `privileged` / `init` のみ
- macOS 側では `container_cfg.rs:160-198` で既に映射済み。macOS の `ContainerCfg` は `cap_add: Vec<String>` / `cap_drop: Vec<String>` (Option なし)、`read_only: bool` を使う

## 設計方針

- `linux_unsupported_request_reason` (`src/runners/async_runner.rs:625-636`) から `cap_add` / `cap_drop` / `shm_size` / `readonly_rootfs` の 4 ガードを削除する
- `ContainerConfig` (`src/core/client.rs:63`) に `cap_add: Vec<String>` / `cap_drop: Vec<String>` / `shm_size: Option<u64>` / `readonly_rootfs: bool` を追加する。`cap_add` / `cap_drop` は macOS 側 (`container_cfg.rs:266-267`) と同じ `Vec<String>` (Option なし) にする
- `build_container_config` (`src/runners/async_runner.rs:499-513`) で `ContainerRequest` から値を映射する
- `HostConfig` (`src/core/client/docker_client.rs:720-725`) に `cap_add` / `cap_drop` / `shm_size` / `readonly_rootfs` フィールドを追加し、`CreateContainerBody::from_config` (`docker_client.rs:655-660`) で `ContainerConfig` から値を受け渡し、`to_json_string` (`docker_client.rs:728`) で `CapAdd` / `CapDrop` / `ShmSize` / `ReadonlyRootfs` を JSON に出力する。出力条件は既存パターンに従い、bool (`ReadonlyRootfs`) は常に出力、collection (`CapAdd` / `CapDrop`) は非空時のみ出力、Option (`ShmSize`) は `Some` 時のみ出力する
- `ContainerConfig` のフィールド追加に伴い、テストヘルパー `config_with_mounts` (`docker_client.rs:1199-1216`) にも 4 フィールドのデフォルト値を追加する
- Docker API の `Privileged: true` は自動的に全 capability を付与するため、macOS 側のような `privileged` 時の `cap_add` 上書きは不要

## 完了条件

- [ ] Linux で `with_cap_add` が Docker の HostConfig.CapAdd に反映されること
- [ ] Linux で `with_cap_drop` が Docker の HostConfig.CapDrop に反映されること
- [ ] Linux で `with_shm_size` が Docker の HostConfig.ShmSize に反映されること
- [ ] Linux で `with_readonly_rootfs` が Docker の HostConfig.ReadonlyRootfs に反映されること
- [ ] 統合テストが追加されていること (最低限: `build_container_config` の出力に各フィールドが反映されることの検証)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

- `linux_unsupported_request_reason` から cap_add / cap_drop / shm_size / readonly_rootfs の 4 ガードを削除した
- `ContainerConfig` に 4 フィールドを追加し `build_container_config` で映射した
- `HostConfig` に 4 フィールドを追加し `from_config` で受け渡し、`to_json_string` で CapAdd / CapDrop / ShmSize / ReadonlyRootfs を JSON 出力した
- テストヘルパー `config_with_mounts` にデフォルト値を追加した
- docs/TESTCONTAINERS.md と skills/shiguredo-container/SKILL.md を更新した
- CHANGES.md に [ADD] エントリを追加した
