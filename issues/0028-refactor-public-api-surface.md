# リファクタリング: 公開 API 面の semver 固定を回避する

- Priority: High
- Created: 2026-07-22
- Completed:
- Model: Claude Sonnet 4
- Branch: feature/refactor-public-api-surface
- Polished: 2026-07-29

## 目的

現在の公開 API 面には、内部実装のモジュールパスや未使用の型が公開されており、一度ユーザーが依存すると将来の整理が破壊的変更になる。2026.0.0 の間に公開面を最小化し、semver 上の自由度を確保する。

## 現状

### 空の公開モジュール `pub mod client` (`src/core.rs`)

`client.rs` 内の全アイテム（`Client`, `ContainerSnapshot`, `ContainerConfig`, `XpcClient`, `DockerClient`）は全て `pub(crate)`。公開アイテムがゼロのモジュールパスが docs.rs に表示される。

### 内部実装を公開する `pub mod env` (`src/core.rs`)

`Config` / `Command` は `async_runner.rs` と `async_container.rs` の内部でのみ使用。ユーザーが直接使う型ではない。

### 公開サブモジュールパスの過剰露出

型は `pub use` で再エクスポート済みのものがあるが、サブモジュールの `pub mod` によりモジュールパスも公開 API になっている:

- `src/core/image.rs`: `pub mod exec`, `pub mod image_ext`（`ExecCommand`, `ImageExt` は再エクスポート済み）
- `src/core/logs.rs`: `pub mod consumer`（**`LogConsumer` には `pub use` 再エクスポートが存在しない**。モジュールパス経由でのみ到達可能）
- `src/core/logs/consumer.rs`: `pub mod logging_consumer`（**`LoggingConsumer` にも `pub use` 再エクスポートが存在しない**）
- `src/core/wait/mod.rs`: `pub mod cmd_wait`, `pub mod exit_strategy`, `pub mod health_strategy`, `pub mod log_strategy`（`CmdWaitFor` 等は再エクスポート済み）。`pub mod http_strategy` は `#[cfg(feature = "http_wait_plain")]` でゲートされている

### pub フィールド (`MountTmpfsOptions`, `CopyTargetOptions`)

`MountTmpfsOptions` (`src/core/mounts.rs`) の `size_bytes` / `mode`、`CopyTargetOptions` (`src/core/copy.rs`) の `path` / `mode` / `uid` / `gid` が `pub`。`Mount` の他フィールドはプライベート + アクセサ pattern で不統一。

## 設計方針

- `pub mod client` → `pub(crate) mod client` に変更
- `pub mod env` → `pub(crate) mod env` に変更
- 公開サブモジュールを `pub(crate) mod` に変更し、`pub use` による再エクスポートだけを残す。既存の `pub use` がない `LogConsumer` / `LoggingConsumer` については、`src/core/logs/consumer.rs` に `pub use logging_consumer::LoggingConsumer;` を中継として追加し、`src/core/logs.rs` に `pub use consumer::LogConsumer;` と `pub use consumer::LoggingConsumer;` を**新規追加**する（`consumer` モジュールが `pub(crate)` になるため、`logs.rs` からの再エクスポートがなければ外部から到達不能になる。CODEBASE.md の「testcontainers-rs 互換のため、クレート公開面の `pub use` re-export を許可する」に該当）
- `MountTmpfsOptions` のフィールドを `pub(crate)` にし、`size_bytes()` / `mode()` アクセサを追加する（戻り値は `Option<i64>`）
- `CopyTargetOptions` のフィールドを `pub(crate)` にし、`path()` / `uid()` / `gid()` アクセサを追加する（`mode()` は既存。`path()` の戻り値は `&str`、`uid()` / `gid()` の戻り値は `u32`）
- re-export 自体は本家互換のため維持する（CODEBASE.md の許可に該当。本家 testcontainers-rs の API 互換が目的）
- `containers::async_container::exec` の `pub mod exec` は `async_container` 自体が `pub(crate) mod` のため公開面に影響しない（対応不要）
- 0033 の `DockerClient` / `XpcClient` の `pub(crate)` 化は、本 issue で `pub mod client` → `pub(crate) mod client` にすれば実効的に冗長になる（実施順序の注記）

## 完了条件

- [ ] `pub mod client` / `pub mod env` が `pub(crate)` になっていること
- [ ] 公開サブモジュールが `pub(crate) mod` になり、`pub use` 経由でのみ型が到達可能になっていること（`LogConsumer` / `LoggingConsumer` の `pub use` 新規追加を含む）
- [ ] `MountTmpfsOptions` / `CopyTargetOptions` のフィールドが `pub(crate)` になり、アクセサメソッドが追加されていること
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が実態に合わせて更新されること
- [ ] `CHANGES.md` に `[CHANGE]` エントリが記載されること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
