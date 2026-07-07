# リファクタリング: shiguredo-rust 規約違反の修正と設計改善

- Priority: Low
- Created: 2026-07-22
- Completed:
- Model: Claude Sonnet 4

## 目的

shiguredo-rust 規約違反とコードレビューで発見した設計・可読性の問題を修正する。

## 優先度根拠

規約違反ではあるが機能・安全性には影響しないため Low。コード品質の向上として対応する。

## 現状

### shiguredo-rust 規約違反

- `#[allow(clippy::infallible_destructuring_match)]` が 2 箇所（src/runners/async_runner.rs:55, :244）。規約は `#[expect(...)]` を要求
- `ExtraHost` に `Copy` がない（src/core/containers/request.rs:61）。全 variant が Copy 可能
- `ContainerPort::as_u16(&self)` / `as_str(&self)` が `&self` を取っている（src/core/ports.rs:82-89）。Copy な Enum のメソッドは `self` で受けるべき

### 設計改善

- `DockerClient` / `XpcClient` が `pub` 宣言だが実効可視性は `pub(crate)`（docker_client.rs:36, xpc_client.rs:28）
- `ExecResult` の名前衝突（xpc_client.rs:685 vs containers/async_container/exec.rs:14）
- `ExecCommand` の `env_vars` が `HashMap`、`ContainerRequest` は `BTreeMap` で混在（image/exec.rs:6）
- `with_cmd_ready_condition` の引数名が `ready_conditions`（複数形）だが単数を受け取る（image/exec.rs:30）
- `LogSource` / `LogFrame` に `PartialEq, Eq` がない（logs.rs:10, :17）
- `LoggingConsumer` の `BothStd` 分岐が到達不能（logging_consumer.rs:79-82）
- ロールバックパターンが async_runner.rs で 4 回重複
- `XpcClient` は ZST だが `Arc<XpcClient>` で包まれている（client.rs:56-57）

## 設計方針

- `#[allow(...)]` → `#[expect(...)]` に変更
- `ExtraHost` に `Copy` を derive
- `ContainerPort::as_u16` / `as_str` を `self` 受けに変更
- `DockerClient` / `XpcClient` を `pub(crate)` 宣言に変更
- XPC 側の `ExecResult` を `XpcExecResult` に改名
- `ExecCommand` の `env_vars` を `BTreeMap` に統一
- 引数名を `ready_condition`（単数形）に修正
- `LogSource` / `LogFrame` に `PartialEq, Eq` を derive
- `BothStd` 分岐を削除
- ロールバックパターンをヘルパ関数に抽出
- `Arc<XpcClient>` を `XpcClient` の直接保持に変更

## 完了条件

- [ ] `#[allow(...)]` が `#[expect(...)]` になっていること
- [ ] `ExtraHost` に `Copy` があること
- [ ] `ContainerPort` のメソッドが `self` 受けになっていること
- [ ] 上記の設計改善がすべて適用されていること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
