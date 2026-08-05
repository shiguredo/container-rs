# バグ: LogConsumer の `accept` panic で `active_consumers` が減少せず、Drop の完了待ちが毎回無駄になる

- Created: 2026-08-04
- Completed: 2026-08-05
- Branch: feature/fix-log-consumer-panic-counter
- Polished: 2026-08-04

## 目的

Linux の LogConsumer 配信タスクで、ユーザーコールバック (`LogConsumer::accept`) が panic した場合に `active_consumers` カウンタが減少せず、`all_done()` が永久に false のままになる問題を修正する。

## 現状

- `src/core/client/docker_log_stream.rs` の `spawn_log_consumer_task` は `handle.register_consumer()` でカウンタを増やしてから `tokio::spawn` し、配信ループの終了時に `handle.consumer_finished()` で減らす
- ループ内の `consumer.accept(&frame).await` が panic すると panic によりタスクが終了し、`consumer_finished()` が呼ばれず `active_consumers` は減少しない (panic 時は残りの consumer と後続フレームへの配信も止まる)
- `all_done()` は `active_consumers == 0` を要求するため、以後永久に false になり、`ContainerAsync::drop` の完了待ちポーリング (`async_container.rs` の Drop。20 回 × 50ms = 最大 1 秒。Runtime 外の Drop 時のみ。Runtime 内の Drop では待たない) が毎回最大 1 秒を無駄にする
- 直接の誤動作ではないが、カウンタの不変条件が壊れる構造

## 設計方針

- panic 経路でも `consumer_finished()` が必ず呼ばれるようにする。方式は既存の `TerminateOnDrop` (`docker_log_stream.rs` の demux タスクで使用中の「正常終了・panic のいずれでも終了処理を保証する Drop ガード」) と同型の Drop ガードに確定する (`std::panic::catch_unwind` は使用しない。async ループに適用できず (標準の `catch_unwind` は同期クロージャのみ)、プロジェクトの Rust 規約でも禁止されているため)
- 正常終了時の明示的な `consumer_finished()` 呼び出しは削除してガードに一本化し、ちょうど 1 回だけ呼ばれるようにする (二重減算による `usize` アンダーフローを防ぐ)
- panic 自体は tokio の既定どおりログに残す (握り潰さない。panic による配信停止は現状どおり)
- 対象は Linux のみ (macOS 側の配信タスクは `active_consumers` カウンタを持たない。panic で配信が止まる構造は同一だがカウンタ不変条件は存在しない)

## 完了条件

- `accept` が panic するコールバックを登録しても、`all_done()` が true になり Drop の完了待ちが無駄にならないこと (単体テスト)
- 正常終了後も `all_done()` が true になること (カウンタがちょうど 1 回だけ減算されること。単体テスト)
- 正常系の配信挙動が変わらないこと

## 解決方法

- `src/core/client/docker_log_stream.rs` の `spawn_log_consumer_task` に、`TerminateOnDrop` と同型の Drop ガード `ConsumerFinishedOnDrop` を導入した。ガードは async ブロック内・配信ループの前に生成し、正常終了・panic・タスクキャンセルのいずれの経路でも future の drop とともに `consumer_finished()` をちょうど 1 回呼ぶ (`std::panic::catch_unwind` は使わない)
- 正常終了時の明示的な `consumer_finished()` 呼び出しを削除し、ガードに一本化した (二重減算による `usize` アンダーフローを防ぐ)
- panic 自体は tokio の既定どおり stderr にログ出力される (握り潰さない)
- `register_consumer()` は `tokio::spawn` の外で呼ぶため、spawn 自体の panic / 初回 poll 前の cancel ではカウンタが増えたままになるが、呼び出し元は全て async 文脈であり実害がない旨をガードの doc に明記した
- テスト: `consumer_panic_still_decrements_active_consumers` (panic 経路で `active_consumers` が 0 に戻り `all_done()` が true) と `consumer_normal_finish_decrements_active_consumers_once` (正常終了でちょうど 1 回だけ減算) を追加した
