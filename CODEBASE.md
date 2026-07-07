# container-rs

- バージョンが 2026.0.0 の間は pull-request は経由せず develop -> branch -> develop (スカッシュマージ)
- バージョンが 2026.0.0 の間は CHANGES.md は更新しない
- testcontainers-rs 互換のため、次のトレイト定義を許可する:
  `Image` / `ImageExt` / `AsyncRunner` / `SyncRunner` / `LogConsumer` /
  `IntoContainerPort` / `CopyFileFromContainer`
- 上記互換のため、クレート公開面 (`lib.rs` / `core` 等) の `pub use` re-export を許可する
