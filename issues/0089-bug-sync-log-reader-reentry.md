# バグ: 同期ログリーダーに再入検出が無く、LogConsumer コールバック内で読むと共有ランタイムの worker が凍結する

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-sync-log-reader-reentry
- Polished: {YYYY-MM-DD}

## 目的

`blocking` feature の `Container::stdout(true)` / `stderr(true)` が、共有ランタイムの worker 上 (LogConsumer コールバック内) から呼ばれた場合にランタイム全体を凍結させる問題を、`block_on_runtime` と同じ再入検出で防ぐ。

## 現状

- `src/core/containers/sync_container.rs` の `Container::stdout` / `stderr` は `ContainerAsync::stdout_sync` / `stderr_sync` に直接委譲する
- `block_on_runtime` は「唯一のワーカースレッドが塞がれて timer 依存の処理が進まなくなり deadlock するため、fail-fast で防ぐ」再入検出 (`Handle::try_current()` の id 一致) を持つが、**同期ログリーダーはこの検出を一切通らない**
- `stdout_sync` / `stderr_sync` (macOS: `FollowFdReader::read` の 100ms スリープ、Linux: `SyncLogReader::read` の `park_timeout(50ms)`) は、コンテナが生存し exit code も未観測の間ブロックし続ける
- 共有ランタイムは `worker_threads(1)` (`src/runners/sync_runner.rs`) のため、LogConsumer コールバック (worker 上で実行) から同期リーダーを読むとランタイム全体が無期限に近く停止する
- rustdoc には「呼ばないこと」とあるが、`block_on_runtime` と同じ設計意図の穴であり、検出・防護がない

## 設計方針

- `stdout_sync` / `stderr_sync` の入口で `block_on_runtime` と同じ再入判定 (共有ランタイムの worker 上かどうか) を追加し、一致したら fail-fast でエラーを返す
- 戻り値がリーダー型 (`Box<dyn BufRead>`) のため、エラーを返す API 形状にできない場合は、再入時に空リーダー + `tracing::error` にする等、API 互換を保つ形で検討する

## 完了条件

- 共有ランタイムの worker 上から同期リーダーを読んでも、ランタイムが凍結しないこと (単体テスト)
- 通常の呼び出し経路 (worker 外) の挙動が変わらないこと

## 解決方法

- `src/core/containers/async_container.rs` の `stdout_sync` / `stderr_sync` に再入検出を追加する (worker 判定は `sync_container.rs` の `block_on_runtime` と同じ id 比較)
- 再入時にエラーまたは空リーダーを返す分岐を追加し、`tests/container_sync_drop_macos.rs` 等に回帰テストを追加する
