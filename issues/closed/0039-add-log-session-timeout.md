# 機能追加: ログセッションにタイムアウトを導入する

- Priority: Low
- Created: 2026-07-23
- Completed: 2026-08-01
- Model: qwen3.8-max-preview
- Branch: feature/add-log-session-timeout
- Polished: 2026-07-29

## 目的

Linux ログストリーム (`src/core/client/docker_log_stream.rs`) の HTTP セッションにタイムアウトが無い。Docker デーモンが接続受理後に無応答になると、該当スレッド / 呼び出し元が無限にブロックし得る。タイムアウトを導入してハングを抑止する。

## 現状

タイムアウトが無い箇所は 2 系統:

1. **oneshot (`follow=false`) 経路**: `fetch_logs_oneshot_blocking` が `UnixStream::connect(socket_path)` と `stream.read(buf)` / `stream.write_all` をタイムアウト無しで呼ぶ。`SyncOneshotReader` (`Container::stdout(false)` が返す) は呼び出しスレッドが、`OneshotReader` は `spawn_blocking` スレッドがブロックする。
2. **follow (`follow=true`) 起動経路**: `spawn_log_session` → `run_log_session` → `start_and_demux` → `read_and_validate_head` がヘッダ検証までタイムアウト無しで `stream.read` / `stream.write_all` する。デーモン無応答時、`AsyncRunner::start` / `ContainerAsync::start` が無期限ハングし得る（`startup_timeout` は ready 待機のみに掛かり、ログセッション起動は対象外）。

なお既存の `DockerClient::request` (spawn_blocking 内の同期 HTTP) も同様にタイムアウト無し（本 issue はログセッションに限定する。0029 が補完関係として引き受け）。

## 設計方針

- oneshot 経路: `UnixStream` に `set_read_timeout` / `set_write_timeout` を設定する。`std::io::ErrorKind::WouldBlock` / `TimedOut` を適切に `Err` として伝播する
- follow 起動経路: 接続直後に `set_read_timeout` / `set_write_timeout` を設定し、起動検証 (`read_and_validate_head` の `stream.read` / `stream.write_all`) にタイムアウトを適用する。起動成功後に `demux_loop` に移行する前に `set_read_timeout(None)` / `set_write_timeout(None)` でタイムアウトを解除する（`follow=true` ストリームは legitimately 長時間無ログになり得るため、`demux_loop` にタイムアウトが残ると誤終了する）。あるいは async 側で `tokio::time::timeout(started_rx.await)` を使い、超過時は `handle.stop()` でソケット shutdown して cleanup する
- タイムアウト値はログセッション用の新規定数を定義する（`DEFAULT_STARTUP_TIMEOUT` (60 秒) を参照値とする。`POLL_INTERVAL` (100ms) はポーリング間隔でありソケットタイムアウトとしては不適切）
- `demux_loop` の `stream.read` は本 issue のスコープ外だが、`set_read_timeout` の波及に影響を受けるため、起動成功後の解除が必須

## 完了条件

- [ ] oneshot 経路と follow 起動経路にタイムアウトが導入され、デーモン無応答時にハングせず `Err` を返す
- [ ] follow 経路の `demux_loop` がタイムアウトの影響を受けないこと（起動成功後にタイムアウト解除）
- [ ] 正常系（タイムアウト内に応答あり）の挙動が変わらない
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

`src/core/client/docker_log_stream.rs` の `fetch_logs_oneshot_blocking` と `spawn_log_session` / `read_and_validate_head` にタイムアウトを導入する。タイムアウト値の定数化と、タイムアウトエラーの伝播を実装する。デーモン無応答を模すテスト（応答しない Unix listener + テスト側タイムアウト付き）でハングしないことを検証する。
