# 機能追加: ログセッションにタイムアウトを導入する

- Priority: Low
- Created: 2026-07-23
- Completed: {YYYY-MM-DD}
- Model: qwen3.8-max-preview
- Branch: feature/add-log-session-timeout
- Polished: {YYYY-MM-DD}

## 目的

Linux ログストリーム (`src/core/client/docker_log_stream.rs`) の HTTP セッションにタイムアウトが無い。Docker デーモンが接続受理後に無応答になると、該当スレッド / 呼び出し元が無限にブロックし得る。タイムアウトを導入してハングを抑止する。

## 優先度根拠

ローカル Unix ソケットの Docker デーモンが無応答になるケースは稀で、通常利用への影響は小さいため Low。ただしハング時は診断が難しいため、堅牢性向上として対応する価値はある。

## 現状

タイムアウトが無い箇所は 2 系統:

1. **oneshot (`follow=false`) 経路**: `fetch_logs_oneshot_blocking` が `UnixStream::connect(socket_path)` と `stream.read(buf)` をタイムアウト無しで呼ぶ。同期 API (`Container::stdout_to_vec` 等) は呼び出しスレッドが、非同期 API (`OneshotReader`) は `spawn_blocking` スレッドがブロックする。
2. **follow (`follow=true`) 起動経路**: `spawn_log_session` → `start_and_demux` → `read_and_validate_head` がヘッダ検証までタイムアウト無しで `stream.read` する。デーモン無応答時、`AsyncRunner::start` / `ContainerAsync::start` が無期限ハングし得る（`startup_timeout` は ready 待機のみに掛かり、ログセッション起動は対象外）。

なお既存の `DockerClient::request` (spawn_blocking 内の同期 HTTP) も同様にタイムアウト無し（本 issue はログセッションに限定する）。

## 設計方針

- `UnixStream` に `set_read_timeout` / `set_write_timeout` を設定するか、`connect` にタイムアウトを設ける。
- タイムアウト値は既存の `DEFAULT_STARTUP_TIMEOUT` (60 秒) や `POLL_INTERVAL` と整合させるか、ログセッション用の定数を定義する。
- oneshot 経路は `std::io::ErrorKind::WouldBlock` / `TimedOut` を適切に `Err` として伝播する。
- follow 起動経路は起動タイムアウトを `spawn_log_session` に設け、超過時は起動失敗として `Err` を返す（呼び出し側は fail-fast / フォールバックの既存分岐に乗る）。

## 完了条件

- oneshot 経路と follow 起動経路にタイムアウトが導入され、デーモン無応答時にハングせず `Err` を返す
- 既存の Linux 統合テスト・単体テストが pass する
- 正常系（タイムアウト内に応答あり）の挙動が変わらない

## 解決方法

`src/core/client/docker_log_stream.rs` の `fetch_logs_oneshot_blocking` と `spawn_log_session` / `read_and_validate_head` にタイムアウトを導入する。タイムアウト値の定数化と、タイムアウトエラーの伝播を実装する。デーモン無応答を模すテスト（応答しない Unix listener）でハングしないことを検証する。
