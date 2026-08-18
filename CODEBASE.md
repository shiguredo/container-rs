# container-rs

- testcontainers-rs 互換のため、次のトレイト定義を許可する:
  `Image` / `ImageExt` / `AsyncRunner` / `SyncRunner` / `LogConsumer` /
  `IntoContainerPort` / `CopyFileFromContainer`
- 上記互換のため、クレート公開面 (`lib.rs` / `core` 等) の `pub use` re-export を許可する
- `TESTCONTAINERS_COMMAND=keep` の概念名はドキュメント・コメント内で `` `keep` `` (小文字 + backtick) と表記する。
  Rust enum バリアント名 `Command::Keep` は Rust 命名規則に従い大文字のまま変更しない
