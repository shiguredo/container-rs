# バグ: macOS start() の再起動フローがログ再取得失敗時に巻き戻らない

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-start-log-refresh-rollback
- Polished: 2026-08-02

## 目的

macOS で停止済みコンテナを再起動する `ContainerAsync::start` が、ログ FD 再取得に失敗した場合にコンテナを実行中のまま残す不整合を解消する (Linux 分岐と非対称)。

## 現状

- `src/core/containers/async_container.rs` の `ContainerAsync::start` (macOS 分岐) は `bootstrap_container` + `start_process` 成功後に `refresh_log_streams` を呼ぶ
- `refresh_log_streams` が失敗すると (再 bootstrap 後は旧ログ FD が死ぬため) `stdout` / `stderr` が読めないリーダー (死んだ FD への pread) になり、後続のログ照合も FD 依存のため機能しない実行中コンテナが残ったまま `Err` が返る
- 同じファイルの Linux 分岐 (`refresh_log_streams`) は新規ログセッション起動失敗時に `client.stop(&self.id, Some(0))` で巻き戻す設計で、macOS には巻き戻しが無い

## 設計方針

ログ再取得失敗時に実行中のコンテナを残すと、ログ系 API が機能しない「ちぐはぐな状態」になる (Linux の `refresh_log_streams` が stop で巻き戻すのと同じ理由)。macOS も同様に、ログ再取得失敗時はコンテナを停止 (SIGKILL) してから元の `Err` を返す。巻き戻し失敗時は `tracing::warn` で記録する (Linux と同じ文言)。巻き戻しは Keep ゲート無し (Linux と同様。`TESTCONTAINERS_COMMAND=keep` 指定時も巻き戻す)。

なお macOS の `refresh_log_streams` はログ利用の有無を問わず無条件に `logs()` を取得するため、ログを一切使わない利用者でも再起動時に `logs()` が失敗すると巻き戻しが発生する (Linux は旧ログハンドルが無い場合にログセッションを試行しないため非対称。この非対称は現行の macOS 実装の特性であり、本 issue では維持する)。

## 完了条件

- macOS の再起動フローでログ再取得が失敗した場合、コンテナが実行中のまま残らず停止されている (巻き戻しが成功した場合。巻き戻し失敗時は warn 記録のみで実行中が残り得る)
- 巻き戻しの成否にかかわらず、呼び出し側には `Err` が返る (元の refresh エラーを返す)
- 既存の macOS 統合テスト (特に再起動テスト) が pass すること
- `CHANGES.md` に `[FIX]` エントリが追加されている

## 解決方法

- macOS の `start` で `refresh_log_streams` が `Err` を返した場合に `self.stop_with_timeout(Some(0))` (SIGKILL) で巻き戻す。`stop_with_timeout` は `stop_log_delivery` を経由して現役の LogConsumer タスクも停止するため、`client.stop` 直接呼びよりも望ましい (直接呼びだと現役 consumer が死んだ FD をポーリングし続ける)。巻き戻し失敗は `failed to stop container after log refresh failure: {stop_err}` の warn ログに記録し、元の `Err` を返す
- 巻き戻しの配置は `start` 側とする。macOS の `refresh_log_streams` は「ログ FD の差し替え」という単一責務 (Linux の「コンテナ再起動 + ログセッション起動」とは構造が異なる) のため、失敗時の巻き戻しは呼び出し側の `start` が担う
- 巻き戻しで停止済みになったコンテナは、次回 `start()` で再 bootstrap され、`refresh_log_streams` 成功時に `log_stop` / FD / consumer が新しく差し替えられて回復する。巻き戻し後も旧 FD は `log_source` に残るが、次回 refresh 成功時または `rm` / Drop で close される
- 初回作成フロー (create / bootstrap / start_process の失敗時) は `rollback_remove` (削除) だが、再 start フローで削除すると次回 `start` が `ContainerNotFound` で失敗するため、停止 (SIGKILL) で巻き戻す
- 検証: ログ再取得失敗 (XPC `logs` の失敗) は自動テストでの再現が困難 (モック禁止の制約下では誘発手段が限られる。FD 枯渇 (setrlimit 等) での誘発を検討する場合、巻き戻しの `stop_with_timeout` も XPC 接続 (ソケット FD) を要するため、誘発条件の絞り込み (reply の FD 受信 / dup のみを枯渇させる等) が必要)。巻き戻し経路はコードレビューと実機・手動確認で検証する (Linux の巻き戻しにもテストが無い)
- `async_container.rs` を変更するため、同一ファイルを変更する 0061 / 0062 とマージ順に注意する (0061 は `stop_with_timeout` の rustdoc 更新、0062 は `spawn_log_consumer_task` のシグネチャ変更を伴う。本 issue はテストを追加しないため `tests/container_macos.rs` は変更しない)
- `CHANGES.md` に `[FIX]` エントリを追加する
