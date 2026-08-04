# バグ: Linux exec の ExitCode 取得リトライが短く、デーモンの状態記録が遅れると exit code を取得できない

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-exec-exit-code-retry
- Polished: 2026-08-04

## 目的

Linux の `ContainerAsync::exec` で、ストリーム EOF 後に exit code を取得するリトライが短すぎるため、デーモンの状態記録が遅れる環境で exit code を取得できず `None` になる問題を修正する。

## 現状

- `src/core/client/docker_client.rs` の `exec` は、`POST /exec/{id}/start` のストリーム EOF 後に `GET /exec/{id}/json` を **最大 5 回の試行・失敗ごとに 10ms の sleep (最大 40ms)** だけポーリングし、`Running == false` にならないと `ExitCode` を取得できない (リトライは 0017 で導入された)
- デーモンが `Running` フラグの記録を遅らせる環境 (高負荷・システムの一時的な遅延等) では 40ms では足りず、`tracing::warn!("exec ... still running after stream EOF, exit code unavailable")` に落ちて `DockerExecResult.exit_code` が `None` になる潜在リスクがある (観測事例ではなく潜在リスクとしての堅牢化)
- exec の exit code はコンテナテストの成否判定に使われる主要な契約であり、`CmdWaitFor::Exit { code: Some(...) }` は**期待コードが指定され、取得した `exit_code` が `None` の場合**に「確認できない = エラー」になる (`async_container.rs` の `exec`)

## 設計方針

- リトライ間隔を指数的に伸ばす (初回試行は即時、失敗ごとに 10ms → 50ms → 200ms → 500ms → 1s の sleep を挟んで再試行。sleep は最大 5 回・合計約 1.76 秒で、試行は最大 6 回になる) 方式に確定する。`POST /containers/{id}/wait?condition=not-running` は**コンテナの終了**を待つ API であり exec プロセスの終了には使えないため不採用
- リトライ全体はバックオフ上限 (合計約 1.76 秒) で打ち切る (無限待ちにしない)。打ち切り時は現行どおり `warn` ログ + `exit_code: None` を返す
- リトライ中の HTTP エラー (status >= 400) は現行どおり即エラーにする。`Running == false` になった時点の `ExitCode` パース失敗もリトライ継続せず `None` のまま打ち切る (現行どおり)

## 完了条件

- EOF 直後の inspect が `Running == true` を返しても、バックオフ上限 (合計約 1.76 秒) 内に `Running == false` を観測して exit code を取得できること (単体テスト)
- 単体テストでリトライのバックオフシーケンス (間隔列・回数・上限) が仕様どおりであることが検証されること
- `src/core/client/docker_client.rs` の exec のリトライに関するコメントが実装に合わせて更新されること

## 解決方法

- `src/core/client/docker_client.rs` の `exec` のリトライ間隔・回数を拡大する (指数的バックオフ)
- バックオフの間隔列 (10ms → 50ms → 200ms → 500ms → 1s) は定数として切り出し、リトライの判定ロジック (パース済みの `(Running, ExitCode)` の応答列を順に消化し、`Running == false` で `ExitCode` を返す) は純粋関数として切り出し、それぞれ単体テストで検証できるようにする。`Running` のパース失敗 (フィールド欠落・型不一致) は現行どおり終了扱い (`false`) として扱う。「EOF 直後は `Running`、数回後に `Running == false` になる」応答列を入力として、exit code が取得できることを検証する (モックやスタブは使わない)
