# バグ: Linux exec の ExitCode 取得リトライが 5 回 × 10ms で打ち切り、高負荷時に exit code を恒久喪失する

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-exec-exit-code-retry
- Polished: {YYYY-MM-DD}

## 目的

Linux の `ContainerAsync::exec` で、ストリーム EOF 後に exit code を取得するリトライが短すぎるため、デーモンが遅い環境で exit code を取得できず `None` のまま落ちる問題を修正する。

## 現状

- `src/core/client/docker_client.rs` の `exec` は、`POST /exec/{id}/start` のストリーム EOF 後に `GET /exec/{id}/json` を **5 回 × 10ms 間隔 (合計約 50ms)** だけポーリングし、`Running == false` にならないと `ExitCode` を取得できない
- デーモンが `Running` フラグの記録を遅らせる高負荷環境では 50ms では足りず、`tracing::warn!("exec ... still running after stream EOF, exit code unavailable")` に落ち、`DockerExecResult.exit_code` が `None` になる
- exec の exit code はコンテナテストの成否判定に使われる主要な契約であり、`CmdWaitFor::Exit { code: Some(...) }` は `None` の場合に「確認できない = エラー」になる (`async_container.rs` の `exec`)。取得可能だったはずの exit code を恒久喪失する

## 設計方針

- リトライ間隔を指数的に伸ばす (例: 10ms → 50ms → 200ms → 500ms → 1s で合計数秒) か、`POST /containers/{id}/wait?condition=not-running` を使う
- 全体は呼び出し側の文脈で打ち切れる範囲に収める (無限待ちにしない)

## 完了条件

- 高負荷 (または遅延注入) 環境でストリーム EOF 直後に `Running == true` のままでも、リトライ期間内に exit code を取得できること
- 単体テストでリトライ回数・間隔の上限が検証されること

## 解決方法

- `src/core/client/docker_client.rs` の `exec` のリトライ間隔・回数を拡大する (指数的バックオフ)
- 単体テストで「EOF 直後は Running、数回後に Running == false になる」ケースを追加し、exit code が取得できることを検証する
