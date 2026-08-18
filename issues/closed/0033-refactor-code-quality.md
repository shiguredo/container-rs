# リファクタリング: コードレビューで発見した設計・可読性の問題修正

- Priority: Low
- Created: 2026-07-22
- Completed: 2026-08-01
- Model: Claude Sonnet 4
- Branch: feature/refactor-code-quality
- Polished: 2026-07-29

## 目的

コードレビューで発見した設計・可読性の問題を修正する。

## 現状

### 設計改善

- `DockerClient` / `XpcClient` が `pub` 宣言だが実効可視性は `pub(crate)` (`src/core/client/docker_client.rs` の `DockerClient` struct、`src/core/client/xpc_client.rs` の `XpcClient` struct)。なお 0028（公開 API 面の semver 固定回避）で `pub mod client` → `pub(crate) mod client` が実施されれば実効的に冗長になる（0028 未実施時のみ対応）
- XPC 側の `ExecResult` (`src/core/client/xpc_client.rs`) と `ExecResult` (`src/core/containers/async_container/exec.rs`) の名前衝突。Docker 側は `DockerExecResult` で命名不整合
- `ExecCommand` の `env_vars` が `HashMap` (`src/core/image/exec.rs`)、`ContainerRequest` は `BTreeMap` (`src/core/containers/request.rs`) で混在。なお 0050（Linux exec env_vars 実装）が `ExecCommand.env_vars` を触るため、実施順序に注意
- `with_cmd_ready_condition` の引数名が `ready_conditions`（複数形）だが単数を受け取る (`src/core/image/exec.rs`)
- `LogSource` / `LogFrame` に `PartialEq, Eq` がない (`src/core/logs.rs`)
- `LoggingConsumer` の `BothStd` 分岐が到達不能 (`src/core/logs/consumer/logging_consumer.rs`)。ただし `LogSource::BothStd` は `log_strategy.rs` や `wait/mod.rs` で実際に構築・使用されているため、variant 自体の削除は不可。`LoggingConsumer::accept` の match arm を `_ => unreachable!()` にするか、`LogFrame::source()` の返り値の型を 2 variant に絞る方針を検討する
- ロールバックパターンが `src/runners/async_runner.rs` で 8 箇所重複（macOS 側 4 箇所 + Linux 側 3 箇所 + log FD 取得失敗パス 1 箇所。log FD 取得失敗パスは `matches!(Config.command(), Command::Remove)` ゲートがなく他とパターンが異なる）
- `XpcClient` は ZST だが `Arc<XpcClient>` で包まれている (`src/core/client.rs` の `Client` enum)。`DockerClient` は非 ZST のため `Arc<DockerClient>` は妥当だが、`XpcClient` の直接保持にすると `Client` enum が非対称になる

## 設計方針

- `DockerClient` / `XpcClient` を `pub(crate)` 宣言に変更する（`pub fn detect()` も `pub(crate) fn` に変更）。0028 実施時は対応不要
- XPC 側の `ExecResult` を `XpcExecResult` に改名する（`src/core/client/xpc_client.rs` 内の参照箇所と呼び出し側 `src/core/containers/async_container.rs` の `exec` メソッド内が影響）
- `ExecCommand` の `env_vars` を `BTreeMap` に統一する
- 引数名を `ready_condition`（単数形）に修正する
- `LogSource` / `LogFrame` に `PartialEq, Eq` を derive する
- `BothStd` 分岐を `LogSource::BothStd => unreachable!()` に置き換える（`LogSource::BothStd` variant 自体は log_strategy 等で使用中のため削除しない。ワイルドカード `_` にすると将来 variant 追加時に網羅性チェックが効かなくなるため、明示的に指定する）
- ロールバックパターンをヘルパ関数に抽出する（macOS 側・Linux 側とも対象。log FD 取得失敗パスのゲート差異はヘルパ関数の引数で吸収する。log FD パスの Keep ゲート欠落は現状維持とし、リファクタリングでは行動を変えない。バグであれば別途 issue で対応する）
- `Arc<XpcClient>` を `XpcClient` の直接保持に変更する（`Client` enum の非対称性は許容する）

## 完了条件

- [ ] 上記の設計改善がすべて適用されていること
- [ ] `CHANGES.md` の `### misc` セクションに内部リファクタリングの記載をすること（公開 API の変更は伴わない）
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

- `DockerClient` / `XpcClient` を `pub(crate)` 宣言に変更した
- XPC 側の `ExecResult` を `XpcExecResult` に改名した
- `ExecCommand` の `env_vars` を `HashMap` から `BTreeMap` に統一した
- `with_cmd_ready_condition` の引数名を `ready_condition`（単数形）に修正した
- `LogSource` / `LogFrame` に `PartialEq, Eq` を derive した
- `LoggingConsumer` の `BothStd` 分岐を `unreachable!()` に置き換えた
- ロールバックパターンを `rollback_remove` ヘルパ関数に抽出し 8 箇所の重複を解消した
- `Arc<XpcClient>` を `XpcClient` の直接保持に変更した（ZST のため Arc 不要）
- `CHANGES.md` の `### misc` セクションに `[UPDATE]` エントリを追加した
