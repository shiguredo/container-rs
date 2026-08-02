# バグ: macOS stop_with_timeout がグレース 60 秒超で XPC タイムアウトの誤エラーを返す

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-stop-grace-timeout
- Polished: {YYYY-MM-DD}

## 目的

`ContainerAsync::stop_with_timeout` の公開引数範囲の一部 (グレース 60 秒超) で、停止は成功するのに誤ったタイムアウトエラーが返るのを修正する。

## 現状

- `src/core/client/xpc_client.rs` の `XpcClient::stop` は `XpcConn::send` (DEFAULT_TIMEOUT = 60 秒、`src/xpc/conn.rs` の `send`) で `containerStop` を送る
- `timeout_seconds` は `stopOptions.timeoutInSeconds` にのみ反映され、XPC 呼び出し自体のタイムアウトには影響しない
- apiserver は停止処理の完了を待って reply を返すため、グレース 60 秒超の指定 (`Some(120)` や負値 = `i32::MAX` 秒) では XPC 呼び出しが先にタイムアウトし `ClientError::XpcTimeout` が返る。デーモン側の停止は裏で続行される
- 実機確認: SIGTERM を無視する init に対して `stop_with_timeout(Some(120))` を実行すると 60.01 秒後に `client error: XPC request timed out` が返った
- `stop_with_timeout` の統合テストは `Some(0)` (SIGKILL) のみで、SIGTERM 経路 (None / 正のグレース / 負値) は全 OS で未検証

## 設計方針

XPC 送信タイムアウトをグレース + 余裕に拡張する。グレース自体の上限は設けない (負値 = 極めて長い SIGTERM の仕様を維持する)。

## 完了条件

- グレース 60 秒超の `stop_with_timeout` がエラーを返さず、SIGTERM → グレース経過 → SIGKILL の経路でコンテナが停止する
- SIGTERM 経路 (None / 正のグレース / 負値) の統合テストが追加される

## 解決方法

- `XpcClient::stop` で `XpcConn::send_with_timeout` に「グレース + 余裕」を渡す (例: `timeout_seconds + 30 秒` 相当、負値は `LONG_TIMEOUT` 相当)
- `tests/container_macos.rs` に `trap '' TERM` の init を使った graceful 停止の統合テストを追加する
