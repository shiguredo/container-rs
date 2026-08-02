# バグ: macOS の LogConsumer 配信タスクがコンテナ終了後も 100ms ポーリングを継続する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-log-consumer-polling
- Polished: {YYYY-MM-DD}

## 目的

macOS でコンテナが自然終了した後も LogConsumer 配信タスクが 100ms 間隔の空ポーリングを永久に継続する資源リークを解消する。

## 現状

- `src/core/containers/async_container.rs` の `spawn_log_consumer_task` は EOF (`Ok(0)`) 時に stop フラグだけで終了判定する。ログ FD は通常ファイルのため EOF が永久に続き、stop フラグが立つまでは 100ms 間隔でポーリングし続ける
- 同じファイルの `FollowFdReader::should_stop_follow` は `wait_state` の exit code 記録 (`exit_code().is_some()`) でも EOF にするため、終了判定が非対称
- `TESTCONTAINERS_COMMAND=keep` で失敗コンテナを残してハンドルを保持し続けると、stdout / stderr の 2 タスク + dup 済み FD 2 本 + 10 wakeups/秒 がハンドル生存中ずっと残る

## 設計方針

配信タスクの終了条件を `FollowFdReader::should_stop_follow` と同じ「stop フラグ OR exit code 記録」に揃える。プロセス終了直後に flush されるログを取りこぼさないよう、`src/core/wait/log_strategy.rs` の `DRAIN_GRACE` 相当の猶予を設ける。

## 完了条件

- コンテナ自然終了後、猶予期間を過ぎると配信タスクが停止し、タスクと dup FD が残らない
- 終了直前に出力されたログが consumer に配信される

## 解決方法

- `spawn_log_consumer_task` に `wait_state` (`Arc<std::sync::Mutex<WaitState>>`) を渡し、EOF 時に exit code 記録からの経過時間で終了判定する
- 猶予時間の経過は既存のポーリング周期 (100ms) と `DRAIN_GRACE` の関係を確認して設定する
