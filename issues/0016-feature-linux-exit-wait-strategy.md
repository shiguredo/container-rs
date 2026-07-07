# 機能追加: Linux で ExitWaitStrategy を実装する

- Priority: Medium
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-exit-wait-strategy
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで `ExitWaitStrategy` (`WaitFor::Exit`) を実装する。

## 優先度根拠

`WaitFor::Exit` はコンテナの終了を待つ ready 条件であり、ジョブ型コンテナのテストに必要。`exit_code` (0015) に依存するため、0015 の完了が前提。Medium。

## 現状

- `ExitWaitStrategy::wait_until_ready` の Linux 分岐は `"ExitWaitStrategy is not implemented on Linux"` の即時エラー
- macOS では `exit_code_hint()` (バックグラウンド wait のキャッシュ) と `container_state` (list ポーリング) で停止を検出している

## 設計方針

- 0015 (exit_code) の完了を前提とする
- macOS と同じロジックを Linux 分岐で有効にする: `exit_code_hint()` で exit code を確認し、`container_state` で停止を検出する
- `with_exit_code` で指定した期待コードとの照合も有効にする

## 完了条件

- [ ] Linux で `WaitFor::Exit` がコンテナ終了まで待機すること
- [ ] Linux で `with_exit_code` 指定時に exit code の照合が動作すること
- [ ] 期待コードと異なる場合に `UnexpectedExitCode` エラーを返すこと
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
