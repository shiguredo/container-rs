# バグ: LogConsumer の `accept` panic で `active_consumers` が減少せず、Drop の完了待ちが毎回無駄になる

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
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

- `src/core/client/docker_log_stream.rs` の `spawn_log_consumer_task` で、配信ループの終了処理を `TerminateOnDrop` と同型の Drop ガードに変更し、`consumer_finished()` の呼び出しを保証する (ガードは async block 内・配信ループの前で生成する。正常系の明示呼び出しは削除する)
- `tokio::spawn` は失敗を返さない (Runtime コンテキスト不在時は panic する) ため、spawn 失敗時のカウンタ戻しは不要
- 単体テストに panic するコールバックのケースを追加する (テストは配信対象の行を共有バッファに追記して通知し、`terminate_all()` で `demux_done` を立てたうえで、`all_done()` をタイムアウト付きでポーリングして検証する。`all_done()` は `demux_done && active_consumers == 0` を要求するため、`terminate_all()` の前に `active_consumers == 0` をポーリングして「EOF を経ずにタスクが終了した = panic 経由で減算された」ことを独立に検証する。正常終了テスト (完了条件の「ちょうど 1 回だけ減算されること」) でも `terminate_all()` が必要。`spawn_log_consumer_task` は JoinHandle を返さないため)
- 注意: 0096 (refactor) は両プラットフォームの LogConsumer 行配信ループの共通化を予定しており、実装順序によっては干渉し得る
