# バグ: LogConsumer の `accept` panic で `active_consumers` が減少せず、Drop の完了待ちが毎回無駄になる

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-log-consumer-panic-counter
- Polished: {YYYY-MM-DD}

## 目的

Linux の LogConsumer 配信タスクで、ユーザーコールバック (`LogConsumer::accept`) が panic した場合に `active_consumers` カウンタが減少せず、`all_done()` が永久に false のままになる問題を修正する。

## 現状

- `src/core/client/docker_log_stream.rs` の `spawn_log_consumer_task` は `handle.register_consumer()` でカウンタを増やしてから `tokio::spawn` し、配信ループの終了時に `handle.consumer_finished()` で減らす
- ループ内の `consumer.accept(&frame).await` が panic するとタスクは abort され、`consumer_finished()` が呼ばれず `active_consumers` は減少しない
- `all_done()` は `active_consumers == 0` を要求するため、以後永久に false になり、`ContainerAsync::drop` の完了待ちポーリング (`async_container.rs` の Drop。20 回 × 50ms) が毎回最大 1 秒を無駄にする
- 直接の誤動作ではないが、カウンタの不変条件が壊れる構造

## 設計方針

- panic 経路でも `consumer_finished()` が必ず呼ばれるようにする (scopeguard 相当、または `catch_unwind` / `AssertUnwindSafe` でラップ)
- panic 自体は tokio の既定どおりログに残す (握り潰さない)

## 完了条件

- `accept` が panic するコールバックを登録しても、`all_done()` が true になり Drop の完了待ちが無駄にならないこと (単体テスト)
- 正常系の配信挙動が変わらないこと

## 解決方法

- `src/core/client/docker_log_stream.rs` の `spawn_log_consumer_task` で、配信ループを `catch_unwind` で包むか、終了処理をガード構造に変更して `consumer_finished()` の呼び出しを保証する
- 単体テストに panic するコールバックのケースを追加する
