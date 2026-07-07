# 機能追加: Linux で HostConfig 未反映項目を対応する (cap_add/cap_drop/shm_size/readonly_rootfs)

- Priority: Medium
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-host-config-fields
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで `ContainerRequest` に保存されているが Docker の create JSON に未反映の HostConfig 項目を対応する。

## 優先度根拠

`cap_add` / `cap_drop` はセキュリティテスト、`shm_size` は共有メモリを使うアプリケーション (PostgreSQL 等)、`readonly_rootfs` はセキュリティ強化テストで必要。いずれも Docker Engine API の HostConfig に直接対応するフィールドがあり、実装は容易。Medium。

## 現状

- `with_cap_add` / `with_cap_drop`: `ContainerRequest` には保存されるが `build_container_config` で無視される
- `with_shm_size`: 同上
- `with_readonly_rootfs`: 同上
- 現在の `HostConfig` は `port_bindings` / `binds` / `privileged` / `init` のみ

## 設計方針

- `ContainerConfig` に `cap_add: Option<Vec<String>>` / `cap_drop: Option<Vec<String>>` / `shm_size: Option<u64>` / `readonly_rootfs: bool` を追加する
- `build_container_config` で `ContainerRequest` から値を映射する
- `CreateContainerBody` の `HostConfig` JSON に `CapAdd` / `CapDrop` / `ShmSize` / `ReadonlyRootfs` を追加する

## 完了条件

- [ ] Linux で `with_cap_add` が Docker の HostConfig.CapAdd に反映されること
- [ ] Linux で `with_cap_drop` が Docker の HostConfig.CapDrop に反映されること
- [ ] Linux で `with_shm_size` が Docker の HostConfig.ShmSize に反映されること
- [ ] Linux で `with_readonly_rootfs` が Docker の HostConfig.ReadonlyRootfs に反映されること
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
