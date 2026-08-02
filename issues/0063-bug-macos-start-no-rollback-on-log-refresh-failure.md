# バグ: macOS start() の再起動フローがログ再取得失敗時に巻き戻らない

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-start-log-refresh-rollback
- Polished: {YYYY-MM-DD}

## 目的

macOS で停止済みコンテナを再起動する `ContainerAsync::start` が、ログ FD 再取得に失敗した場合にコンテナを実行中のまま残す不整合を解消する (Linux 分岐と非対称)。

## 現状

- `src/core/containers/async_container.rs` の `ContainerAsync::start` (macOS 分岐) は `bootstrap_container` + `start_process` 成功後に `refresh_log_streams` を呼ぶ
- `refresh_log_streams` が失敗すると (再 bootstrap 後は旧ログ FD が死ぬため) ログ系 API (`stdout` / `stderr` / ログ待機戦略) が機能しない実行中コンテナが残ったまま `Err` が返る
- 同じファイルの Linux 分岐 (`refresh_log_streams`) は新規ログセッション起動失敗時に `stop_container` で巻き戻す設計で、macOS に巻き戻しが無い

## 設計方針

macOS も Linux と同じく、ログ再取得失敗時はコンテナを停止 (SIGKILL) してから `Err` を返す。巻き戻し失敗時は `tracing::warn` で記録する。

## 完了条件

- macOS の再起動フローでログ再取得が失敗した場合、コンテナが実行中のまま残らず停止されている
- 呼び出し側には `Err` が返る

## 解決方法

- macOS の `start` で `refresh_log_streams` が `Err` を返した場合に `stop(Some(0))` (SIGKILL) で巻き戻し、巻き戻しの失敗は warn ログに記録する
