# バグ: macOS の LogConsumer 配信タスクがコンテナ終了後も 100ms ポーリングを継続する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
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

- `spawn_log_consumer_task` に `wait_state` を渡し、EOF 時に「stop フラグ OR (exit code 記録を初めて観測してから 2 秒経過)」で終了判定する。観測時刻はタスク内のローカル `Instant` で記録する (WaitState には記録時刻が無いため。`log_strategy.rs` の `exited_at` パターンと同じ固定アンカー方式)。stop フラグは猶予より優先する (明示 stop が猶予で遅延しない)
- 影響範囲は macOS の `spawn_log_consumer_task` (async_container.rs の 4 箇所の呼び出し) と `log_strategy.rs` の `DRAIN_GRACE` の `pub(crate)` 化に限定される。Linux 側の `docker_log_stream::spawn_log_consumer_task` は別実装で無関係。再 start 時は世代機構で exit code がクリアされ、新タスクはポーリングを継続する (従来どおり)
- `wait_blocking` 失敗で exit code が記録されない場合はポーリングが継続し得る (FollowFdReader と同じ制約。stop / rm / Drop 経路で解消される) ことを許容する
- macOS 統合テスト (`tests/container_macos.rs`) に回帰テストを追加する: テスト用 LogConsumer を登録し、`sh -c "echo <マーカー>; exit 0"` 相当の cmd (マーカーはテスト固有の一意な文字列、例: `log-consumer-exit-marker`) で最終行 (マーカー文字列で特定) 出力後に自然終了させる。(1) 最終行が配信されること、(2) 配信停止をポーリングで検出し (固定待ちにしない。停止は「EOF 観測 + exit code 初観測から 2 秒」で、`wait_blocking` 応答遅延と EOF 観測周期 (最大 100ms) の分だけ遅れ得る)、FD が解放されることを検証する。停止確認の上限時間は「`wait_blocking` 応答遅延 + 最大 100ms + 2 秒 + ポーリング間隔 + マージン」の合計を上回る値にする
- `async_container.rs` と `tests/container_macos.rs` を変更するため、同一ファイルを変更する 0058 / 0059 / 0060 / 0061 / 0063 / 0068 / 0069 とマージ順に注意する (特に 0063 は同じ `refresh_log_streams` の macOS 分岐を変更する)
- `CHANGES.md` に `[FIX]` エントリを追加する
