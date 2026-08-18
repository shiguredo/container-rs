# バグ: macOS の LogConsumer 配信タスクがコンテナ終了後も 100ms ポーリングを継続する

- Created: 2026-08-02
- Completed: 2026-08-03
- Branch: feature/fix-macos-log-consumer-polling
- Polished: 2026-08-02

## 目的

macOS でコンテナが自然終了した後も LogConsumer 配信タスクが 100ms 間隔の空ポーリングを永久に継続する資源リークを解消する。

## 現状

- `src/core/containers/async_container.rs` の `spawn_log_consumer_task` は EOF (`Ok(0)`) 時に stop フラグだけで終了判定する。ログ FD は通常ファイルのため EOF が永久に続き、stop フラグが立つまでは 100ms 間隔でポーリングし続ける
- 同じファイルの `FollowFdReader::should_stop_follow` は `wait_state` の exit code 記録 (`exit_code().is_some()`) でも EOF にするため、終了判定が非対称
- コンテナ自然終了後にハンドルを保持し続けると、stdout / stderr の 2 タスク + dup 済み FD 2 本 + 各タスク 10 wakeups/秒 (合計 20/秒) がハンドル生存中ずっと残る。`TESTCONTAINERS_COMMAND=keep` で失敗コンテナを残すとハンドルが長生きするため特に顕在化する

## 設計方針

配信タスクの終了条件を「stop フラグ OR (exit code 記録 AND 猶予経過)」にする。コンテナ終了直後に flush されるログを取りこぼさないよう、`log_strategy.rs` の `DRAIN_GRACE` と同じ 2 秒の猶予を設ける (定数は `pub(crate)` 化して共有する。ポーリング周期 100ms は 2 秒の猶予に対して十分小さい)。`FollowFdReader::should_stop_follow` は変更しない (即時 EOF のままで、LogWaitStrategy の DRAIN_GRACE 実装と干渉するため。配信タスクは `FollowFdReader` を経由せず、素の `FdReader` の EOF ループで動く)。

## 完了条件

- コンテナ自然終了 (exit code 記録) 後、猶予期間 (2 秒) を過ぎると配信タスクが停止し、タスクと dup FD が残らない (正常系が対象。テスト用 LogConsumer への配信が止まり、FD 数がタスク spawn 前の水準に戻る (dup FD 2 本の解放) ことで検証する。`wait_blocking` の応答遅延と EOF 観測周期 (最大 100ms) により停止が遅延し得る)
- 終了直前に出力されたログが consumer に配信される (自然終了が対象。明示 stop 経路は従来どおり `stop_log_delivery` で停止フラグが立ち、タスクが次の EOF 観測時に停止する)
- `CHANGES.md` に `[FIX]` エントリが追加されている

## 解決方法

- `src/core/containers/async_container.rs` の `spawn_log_consumer_task` (macOS) に `wait_state` を追加引数として渡し、EOF 時に「stop フラグ OR (exit code 記録を初めて観測してから 2 秒経過)」で終了判定するように変更する
- 猶予のアンカーはタスクローカルの `Instant` で初回観測時点に固定する (ポーリングが長引いても猶予が伸びない)。一度観測したアンカーはリセットしない (再 start の世代バンプ時点で旧コンテナは停止済みであり、リセットすると refresh 失敗時に新コンテナの exit までタスクが残るため)
- `src/core/wait/log_strategy.rs` の `DRAIN_GRACE` を `pub(crate)` 化して共有する (LogWaitStrategy と LogConsumer 配信タスクで「コンテナ終了後のドレイン猶予」を共有する意図をコメントで明記)
- `FollowFdReader::should_stop_follow` は変更しない (LogWaitStrategy の DRAIN_GRACE 実装と干渉するため)
- 単体テストへの影響なし (既存 158 本すべて通過)
- 統合テスト: `tests/container_macos.rs` に `xpc_alpine_log_consumer_stops_after_natural_exit` を追加する。`echo <マーカー>; exit 0` の cmd で自然終了させ、(1) マーカーが配信されること、(2) 配信タスクの dup FD (stdout / stderr で計 2 本) が解放されて FD 数が spawn 時より減ること、をポーリングで検証する。実機で通過確認済み (並列実行時の FD 検証の干渉リスクはコメントで明記)
- `CHANGES.md` に `[FIX]` エントリを追加する
