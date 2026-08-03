# バグ: macOS start() の再起動フローがログ再取得失敗時に巻き戻らない

- Created: 2026-08-02
- Completed: 2026-08-03
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

- `src/core/containers/async_container.rs` の `ContainerAsync::start` (macOS 分岐) で、`bootstrap_container` + `start_process` + `reset_wait_state_and_respawn` の後に呼ぶ `refresh_log_streams` が `Err` を返した場合、`self.stop_with_timeout(Some(0))` (SIGKILL) で巻き戻してから元のエラーを返すように変更する
- `stop_with_timeout` は `stop_log_delivery` を経由して現役の LogConsumer タスクも停止するため、`client.stop` 直接呼びより望ましい (Linux 側は `refresh_log_streams` 内部で `client.stop` を呼ぶが、macOS は「FD 差し替え」という単一責務のため呼び出し側の `start` が巻き戻しを担う)
- 巻き戻し失敗時は Linux と同じ文言 (`failed to stop container after log refresh failure: {stop_err}`) で warn 記録し、元の `Err` を返す。巻き戻し失敗時は実行中コンテナ + 死んだログ FD が残るため、先に `stop()` を呼んでから `start()` すると回復する旨をコメントで明記する
- 巻き戻しは Keep ゲート無し (Linux と同様)
- 自動テストは追加しない (XPC `logs` の失敗誘発はモック禁止の制約下で困難。issue の解決方法に記載のとおり)。既存の macOS 再起動テスト 3 本 (`stop_start_restarts_and_resets_wait_state` / `generic_image_stop_start_is_running` / `log_consumer_receives_lines_after_restart`) と全 macOS 統合テスト 86 本が実機で通過することを確認済み
- `CHANGES.md` に `[FIX]` エントリを追加する
