# バグ: fetch_logs_oneshot_blocking の UnixStream にタイムアウトが無い

- Created: 2026-07-31
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-oneshot-log-timeout
- Polished: {YYYY-MM-DD}

## 目的

`fetch_logs_oneshot_blocking` の `UnixStream` に読み書きタイムアウトが無く、Docker デーモンが無応答になると `spawn_blocking` ワーカーが恒久的にブロックするのを修正する。

## 現状

`src/core/client/docker_log_stream.rs` の `fetch_logs_oneshot_blocking` 関数は `UnixStream::connect` 後に `set_read_timeout` / `set_write_timeout` を設定していない。この関数は `stdout_to_vec` / `stderr_to_vec` 系の 1-shot ログ取得経路で `spawn_blocking` 内から呼ばれる。デーモンが無応答になるとワーカーが恒久ブロックし、tokio のブロッキングスレッドプールを枯渇させ得る。

`DockerClient::remove_blocking` には `DOCKER_STREAM_TIMEOUT` (60 秒) が設定済みだが、この関数は `docker_log_stream.rs` にあり同定数を参照していない。

## 設計方針

`fetch_logs_oneshot_blocking` の `UnixStream::connect` 直後に `set_read_timeout` / `set_write_timeout` を設定する。タイムアウト値は `DockerClient` の `DOCKER_STREAM_TIMEOUT` と整合させる (60 秒)。1-shot 取得は `follow=false` で即座にレスポンスが返るため、60 秒で十分。

## 完了条件

- [ ] `fetch_logs_oneshot_blocking` の UnixStream にタイムアウトが設定されること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
