# 機能追加: Linux で ExitWaitStrategy を実装する

- Priority: Medium
- Created: 2026-07-21
- Completed: 2026-08-01
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-exit-wait-strategy
- Polished: 2026-07-29

## 目的

Linux (Docker Engine API) バックエンドで `ExitWaitStrategy` (`WaitFor::Exit`) を実装する。

## 優先度根拠

`WaitFor::Exit` はコンテナの終了を待つ ready 条件であり、ジョブ型コンテナのテストに必要。`exit_code` (0015) に依存するため、0015 の完了が前提。Medium。

## 現状

- `ExitWaitStrategy::wait_until_ready` の Linux 分岐は `"ExitWaitStrategy is not implemented on Linux"` の即時エラー (`src/core/wait/exit_strategy.rs:47-53`)
- macOS では `exit_code_hint()` (バックグラウンド wait のキャッシュ) と `container_state` (XPC `containerList` 呼び出し) で停止を検出している (`exit_strategy.rs:55-83`)
- macOS のループは `#[cfg(target_os = "macos")]` で隔離され、`match client { Client::MacOs(c) => ... }` で macOS 固有バリアントのみを扱う。`Client` enum は `#[cfg]` でプラットフォーム固有バリアントが切り替わるが、本コードベースでは match アームに `#[cfg]` を付ける単一ループのパターンが確立されている (`async_container.rs:210-217` 等)
- `src/core/wait/mod.rs:8-9` のモジュールドキュメントに「Linux では未実装エラーを返す」と記載されている

## 設計方針

- 0015 (exit_code) の完了を前提とする。0015 の実装により Linux でも `exit_code_hint()` が `Some` を返すようになる
- macOS の `#[cfg(target_os = "macos")]` ループの cfg ガードを削除し、`match client` のアームに `#[cfg]` を付ける単一ループに統合する。本コードベースの確立済みパターン (`async_container.rs:210-217` の `ports()`、`631-641` の `stop_with_timeout()`、`666-673` の `is_running()` 等) に従い、`#[cfg(target_os = "linux")] Client::Linux(c) => c.container_state(container.id()).await?` のアームを追加する。ループ本体の待機ロジック (`exit_code_hint()` 判定・`!state.running` 判定・`tokio::time::sleep`) は共通のため複製しない
- macOS と同じロジックを使う: `exit_code_hint()` で exit code を確認し、`container_state` の `running` フラグで停止を検出する。Docker inspect の `State.ExitCode` を直接使う代替案もあるが、macOS との対称性を優先し `exit_code_hint()` 経由に統一する
- `with_exit_code` で指定した期待コードとの照合も有効にする
- ポーリング間隔の既定 (100ms, `exit_strategy.rs:20` の `poll_interval`) と `startup_timeout` による打ち切りは macOS 経路と同じ

## 完了条件

- [ ] Linux で `WaitFor::Exit` がコンテナ終了まで待機すること
- [ ] Linux で `with_exit_code` 指定時に exit code の照合が動作すること
- [ ] 期待コードと異なる場合に `UnexpectedExitCode` エラーを返すこと
- [ ] 統合テストが追加されていること (最低限: exit 0 + `with_exit_code(0)` で成功すること、exit 3 + `with_exit_code(0)` で `UnexpectedExitCode` エラーになること、`with_exit_code` なしの単純な終了待機で `start()` 成功後に `!container.is_running()` であること。テストは `tests/container_linux.rs` に追加する)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の `WaitFor::Exit` / `ExitWaitStrategy` 関連箇所が実装済みに更新されること
- [ ] `src/core/wait/mod.rs` のモジュールドキュメントが実態に合わせて更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

- `exit_strategy.rs` の `#[cfg(target_os = "linux")]` 未実装エラーブロックを削除し、macOS の `#[cfg(target_os = "macos")]` ループガードを解除して共通ループに統合した
- `match client` に `#[cfg(target_os = "linux")] Client::Linux(c) => c.container_state(...)` アームを追加した
- 統合テスト 3 件を `tests/container_linux.rs` に追加した（exit code 照合成功・UnexpectedExitCode エラー・単純終了待機）
- `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の ExitWaitStrategy 関連箇所を「対応」に更新した
- `src/core/wait/mod.rs` のモジュールドキュメントを実態に合わせて更新した
- `CHANGES.md` に `[ADD]` エントリを追加した
