# 機能追加: 同期 `Container` に `pause` / `unpause` を追加する (Linux のみ)

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/add-sync-container-pause-unpause
- Polished: {YYYY-MM-DD}

## 目的

本家 testcontainers-rs 0.27.3 との互換性を高めるため、同期 API (`blocking` feature) の `Container` に `pause` / `unpause` を追加する。

## 現状

- 本家 0.27.3 の `core/containers/sync_container.rs` には `pub async fn pause` / `unpause` が存在する (内部で共有ランタイムの `block_on` により実行する同期実装。Docker Engine API の `POST /containers/{id}/pause` / `unpause` に対応)
- shiguredo の `src/core/containers/sync_container.rs` には `pause` / `unpause` が**存在しない**。非同期の `ContainerAsync` (`src/core/containers/async_container.rs`) には Linux 限定で実装済み (`docker_client.rs` の `pause` / `unpause` に配線済み、単体テストあり)
- 同期 API のみを使うユーザーは Linux で `pause` / `unpause` を呼べず、本家からの移行時にコンパイルエラーになる
- macOS は XPCRoute に pause 系が無いためシグネチャ自体を公開しない方針 (issues/pending/0001 で検討中) であり、本 issue では Linux のみを対象にする
- `docs/TESTCONTAINERS.md` 7 章には「Docker: 対応」と記載されているが実装が無いため、本対応で実装とドキュメントが一致する

## 設計方針

- 既存の同期 API と同じパターン (`block_on_runtime` 経由で `ContainerAsync::pause` / `unpause` に委譲) で実装する
- macOS ではコンパイルされないよう `#[cfg(target_os = "linux")]` でゲートする (macOS の同期 `Container` は pause / unpause を持たない)
- `docs/TESTCONTAINERS.md` 7 章の判定を実装に合わせて更新する (macOS: なし / Linux: 対応)

## 完了条件

- `Container::pause` / `unpause` が Linux で動作し、実行中コンテナの一時停止・再開ができること (統合テスト)
- macOS ビルドで `Container` に pause / unpause が追加されないこと (コンパイル確認)
- `docs/TESTCONTAINERS.md` 7 章の記述が実装と一致すること

## 解決方法

- `src/core/containers/sync_container.rs` に `#[cfg(target_os = "linux")]` 付きで `pause` / `unpause` を追加し、`block_on_runtime` 経由で `ContainerAsync::pause` / `unpause` に委譲する
- `tests/container_linux.rs` に同期 API での pause / unpause の統合テストを追加する (コンテナを起動し、`pause` 後にプロセスが停止していること・`unpause` 後に再開することを検証する)
- `docs/TESTCONTAINERS.md` 7 章を更新する
