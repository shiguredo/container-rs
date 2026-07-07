# 機能追加: Linux で pause/unpause を対応する

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-pause-unpause
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで `pause` / `unpause` を実装する。Docker Engine API には `POST /containers/{id}/pause` / `POST /containers/{id}/unpause` が存在する。

## 優先度根拠

pause/unpause はコンテナのプロセスを一時停止・再開する機能で、テストでの需要は限定的。macOS (Apple Container) では XPC に pause 系 route が無いため実装不可であり、Linux 限定の機能になる。Low。

## 現状

- macOS では XPCRoute に pause 系が無いため、シグネチャ自体を削除済み
- Docker Engine API には `POST /containers/{id}/pause` / `POST /containers/{id}/unpause` がある
- 本家 testcontainers-rs 0.27 には `ContainerAsync::pause` / `unpause` がある

## 設計方針

- `DockerClient` に `pause(id)` / `unpause(id)` メソッドを追加する
- `ContainerAsync` に Linux 限定 (`#[cfg(target_os = "linux")]`) で `pause` / `unpause` メソッドを追加する
- macOS ではシグネチャ無しを維持する (XPC 制約)
- 本家とシグネチャを揃えるかは要検討 (本家は OS 共通シグネチャ)

## 完了条件

- [ ] Linux で `pause()` がコンテナを一時停止すること
- [ ] Linux で `unpause()` がコンテナを再開すること
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
