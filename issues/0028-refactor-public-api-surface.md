# リファクタリング: 公開 API 面の semver 固定を回避する

- Priority: High
- Created: 2026-07-22
- Completed:
- Model: Claude Sonnet 4

## 目的

現在の公開 API 面には、内部実装のモジュールパスや未使用の型が公開されており、一度ユーザーが依存すると将来の整理が破壊的変更になる。2026.0.0 の間に公開面を最小化し、semver 上の自由度を確保する。

## 優先度根拠

公開 API の縮小はユーザーが存在する今しかできない。後から `pub(crate)` に変更すると破壊的変更になる。OSS 公開のブロッカー。

## 現状

### 空の公開モジュール `pub mod client` (src/core.rs:3)

`client.rs` 内の全アイテム（`Client`, `ContainerSnapshot`, `ContainerConfig`, `XpcClient`, `DockerClient`）は全て `pub(crate)`。公開アイテムがゼロのモジュールパスが docs.rs に表示される。

### 内部実装を公開する `pub mod env` (src/core.rs:6)

`Config` / `Command` は `async_runner.rs` と `async_container.rs` の内部でのみ使用。ユーザーが直接使う型ではない。

### 公開サブモジュールパスの過剰露出

型は `pub use` で再エクスポート済みだが、サブモジュールの `pub mod` によりモジュールパスも公開 API になっている:

- `src/core/image.rs:4-5`: `pub mod exec`, `pub mod image_ext`
- `src/core/logs.rs:6`: `pub mod consumer`
- `src/core/logs/consumer.rs:3`: `pub mod logging_consumer`
- `src/core/wait/mod.rs:9-14`: `pub mod cmd_wait`, `pub mod exit_strategy`, `pub mod health_strategy`, `pub mod http_strategy`, `pub mod log_strategy`

### pub フィールド (src/core/mounts.rs:18-21, src/core/copy.rs:37-44)

`MountTmpfsOptions` の `size_bytes` / `mode`、`CopyTargetOptions` の `path` / `mode` / `uid` / `gid` が `pub`。`Mount` の他フィールドはプライベート + アクセサ pattern で不統一。

## 設計方針

- `pub mod client` → `pub(crate) mod client` に変更
- `pub mod env` → `pub(crate) mod env` に変更
- 公開サブモジュールを `pub(crate) mod` に変更し、`pub use` による再エクスポートだけを残す
- `MountTmpfsOptions` のフィールドを `pub(crate)` にし、`size_bytes()` / `mode()` アクセサを追加
- `CopyTargetOptions` のフィールドを `pub(crate)` にし、`path()` / `uid()` / `gid()` アクセサを追加（`mode()` は既存）
- re-export 自体は本家互換のため維持する（shiguredo-rust 規約の「許可を得ること」に該当。本家 testcontainers-rs の API 互換が目的）

## 完了条件

- [ ] `pub mod client` / `pub mod env` が `pub(crate)` になっていること
- [ ] 公開サブモジュールが `pub(crate) mod` になり、`pub use` 経由でのみ型が到達可能になっていること
- [ ] `MountTmpfsOptions` / `CopyTargetOptions` のフィールドが `pub(crate)` になり、アクセサメソッドが追加されていること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
