# 機能追加: Linux で pause/unpause を対応する

- Priority: Low
- Created: 2026-07-21
- Completed: 2026-08-01
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-pause-unpause
- Polished: 2026-07-29

## 目的

Linux (Docker Engine API) バックエンドで `pause` / `unpause` を実装する。

## 現状

- macOS では XPC の route に pause 系が無いため、`ContainerAsync` に pause/unpause のシグネチャ自体が存在しない
- `DockerClient` (`src/core/client/docker_client.rs`) に pause/unpause メソッドは存在しない
- Docker Engine API には `POST /containers/{id}/pause` / `POST /containers/{id}/unpause` がある
- 本家 testcontainers-rs 0.27 には `ContainerAsync::pause` / `unpause` がある (OS 共通シグネチャ: `pub async fn pause(&self) -> Result<()>`)
- コードベースには OS 非対称メソッドの既存パターンがある (`rm` は `#[cfg]` で macOS 版と Linux 版を別定義)

## 設計方針

- `DockerClient` (`src/core/client/docker_client.rs`) に `pause(&self, id: &str)` / `unpause(&self, id: &str)` メソッドを追加する。Docker Engine API の `POST /containers/{id}/pause` / `POST /containers/{id}/unpause` を呼ぶ。304 (already paused/unpaused) は冪等成功として扱う (既存の `stop` / `remove` の 404 冪等パターンに準拠)
- `ContainerAsync` (`src/core/containers/async_container.rs`) に `#[cfg(target_os = "linux")]` で `pause` / `unpause` メソッドを追加する。macOS ではシグネチャ無しを維持する (XPC 制約。本家共通シグネチャへの統一は macOS 対応時に再検討する)
- 本家のシグネチャ (`pub async fn pause(&self) -> Result<()>`) に合わせる

## 完了条件

- [ ] Linux で `pause()` がコンテナを一時停止すること
- [ ] Linux で `unpause()` がコンテナを再開すること
- [ ] 単体テストが追加されていること (`tests/container_linux.rs` に追加。最低限: pause 後にコンテナが一時停止状態であること、unpause 後に再開することの検証)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

- `DockerClient` に `pause` / `unpause` メソッドを追加した (304 / 404 は冪等成功)
- `ContainerAsync` に `#[cfg(target_os = "linux")]` で `pause` / `unpause` メソッドを追加した
- docs/TESTCONTAINERS.md と skills/shiguredo-container/SKILL.md を更新した
- CHANGES.md に [ADD] エントリを追加した
