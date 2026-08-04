# バグ: 同期ログリーダーに再入検出が無く、LogConsumer コールバック内で呼ぶと共有ランタイムの worker が凍結する

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-sync-log-reader-reentry
- Polished: 2026-08-04

## 目的

`blocking` feature の `Container::stdout` / `stderr` (follow の値に関係なく) が、共有ランタイムの worker 上 (LogConsumer コールバック内) から呼ばれて read された場合にランタイム全体を凍結させる問題を、`block_on_runtime` と同じ再入検出で防ぐ。

## 現状

- `src/core/containers/sync_container.rs` の `Container::stdout` / `stderr` は `ContainerAsync::stdout_sync` / `stderr_sync` に直接委譲する
- `block_on_runtime` は「唯一のワーカースレッドが塞がれて timer 依存の処理が進まなくなり deadlock するため、fail-fast で防ぐ」再入検出 (`Handle::try_current()` の id 一致) を持つが、**同期ログリーダーはこの検出を一切通らない**
- 同期リーダーの read はブロックし続ける (macOS: `FollowFdReader::read` の 100ms スリープ (stop フラグ / exit code 未観測の間)、Linux follow=true: `SyncLogReader::read` の `park_timeout(50ms)` (terminated フラグが立つまで)、Linux follow=false: `SyncOneshotReader::read` の初回 read での blocking 取得 (30 秒のセッションタイムアウトあり))
- 共有ランタイムは `worker_threads(1)` (`src/runners/sync_runner.rs`) のため、LogConsumer コールバック (worker 上で実行) から同期リーダーを呼んで読むとランタイム全体が停止する (follow=true は無期限に近く、follow=false はセッションタイムアウトまで)
- rustdoc には「呼ばないこと」とあるが、`block_on_runtime` と同じ設計意図の穴であり、検出・防護がない

## 設計方針

- 再入検出は共有ランタイムの参照を持つ `Container::stdout` / `stderr` (`sync_container.rs`) 側に置く (`block_on_runtime` と同じ id 比較。判定ロジックは共通関数として切り出す。`stdout_sync` / `stderr_sync` は共有ランタイムの参照を持たないため、そこには置けない)
- 再入時の挙動は、**read 時に `io::Error` を返す専用リーダー**を返す方式に確定する (公開 API の形状 (`Box<dyn BufRead>`) は変えない。専用リーダーは `fill_buf` を含むすべての読み取り経路でエラーを返すこと。方式は `BufReader` ラップか直接 `BufRead` 実装のどちらでもよい。README / `consumer.rs` rustdoc の「再入を検出して即座にエラーにする (fail-fast)」方針と整合する。空リーダー + `tracing::error` は「ログが空」と区別できず、fail-fast 方針と矛盾するため不採用)
- 検出はリーダー取得時 (`Container::stdout` / `stderr` の呼び出し時) のみとし、worker 外で取得したリーダーを worker 上で読むケースは対象外とする (LogConsumer コールバック内の典型的な使い方 (コールバック内で取得して読む) は検出できる)

## 完了条件

- 共有ランタイムの worker 上 (LogConsumer コールバック内) から `Container::stdout` / `stderr` を呼んでも、ランタイムが凍結せず、read 時にエラーが返ること (統合テスト。worker 上でコードを動かす公開経路は LogConsumer コールバックのみのため。コールバックは start 前に登録され `Container` は start 後にしか得られないため、`Arc<Mutex<Option<Container>>>` 等の共有ハンドル経由で渡し、Container 未セット時は何もしない前置きを置く)
- 通常の呼び出し経路 (worker 外) の挙動が変わらないこと (既存の同期リーダーテストが引き続き通ること)
- README に「LogConsumer コールバック内で取得した同期ログリーダーは、読み取り時点でエラーになる (コールバック外で取得したリーダーをコールバック内で読むケースはエラーにならない)」旨の補足が追加されること
- `src/core/containers/sync_container.rs` の `stdout` / `stderr` の rustdoc (「呼ばないこと」) が新挙動に合わせて更新されること

## 解決方法

- `src/core/containers/sync_container.rs` の `Container::stdout` / `stderr` で、`block_on_runtime` と同じ再入判定 (共通関数化) を行い、再入時は read 時に `io::Error` を返す専用リーダーを返す
- 回帰テストを追加する (LogConsumer コールバック内から `Container::stdout` / `stderr` を呼び、read でエラーが返りランタイムが凍結しないことを検証する。`tests/container_sync_drop_macos.rs` は最終 drop 検証専用バイナリのため使わない。macOS は `tests/container_macos.rs` 等、Linux は `tests/container_linux.rs` 等の blocking feature ゲート付きテストに配置する)
