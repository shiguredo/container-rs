# テスト: restart_rearms_log_stream が「再武装」を検証できていないのを修正する

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/test-fix-restart-log-rearm-verification
- Polished: {YYYY-MM-DD}

## 目的

`tests/container_linux.rs` の `restart_rearms_log_stream` が、テストコメントの主張 (「新規リーダーが新バッファに接続されることを検証する」) を実際には検証できていないため、再武装の有無で結果が変わらない観測に修正する。

## 現状

- `tests/container_linux.rs` の `restart_rearms_log_stream` は、stop → start 後に `stdout_to_vec()` で marker A と異なる marker B が読めることを条件にする
- しかし `stdout_to_vec()` は `stdout(false)` → 1-shot リーダー (`follow=false`) で、`docker_log_stream.rs` の `DockerLogsHandle` のコメントどおり**新規 HTTP セッションを張って全ログを取得する**
- 新規セッションはコンテナのログ全体を返すため、`refresh_log_streams` (再武装) が**実行されなくても** marker B は読める。Docker daemon のログは restart 後も同一バッファに追記されるため、旧ハンドル経由でも条件は成立する
- つまり再武装の有無でテスト結果が変わらず、回帰を検出できない

## 設計方針

- 再武装の有無で結果が変わる観測にする: 再起動前に取得した follow リーダー (`stdout(true)`) が再起動後に新バッファへ接続されない (または stop で EOF になる) ことを検証する
- 既存の `stop_terminates_log_stream_for_new_reader` と組み合わせて、再武装経路を直接観測する

## 完了条件

- 再武装 (`refresh_log_streams`) が失敗する実装では失敗するテストになること
- 正常系では通ること

## 解決方法

- `tests/container_linux.rs` の `restart_rearms_log_stream` を、1-shot 取得ではなく follow リーダー (`stdout(true)`) を使う観測に書き換える (再起動前に取得したリーダーが stop で EOF になることを確認し、再起動後の新規 follow リーダーが新ログを読めることを確認する)
