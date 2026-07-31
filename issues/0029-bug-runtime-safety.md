# バグ: FD リーク・タイムアウト欠如・無限ループ・OOM リスク・バリデーション不足のランタイム安全性修正

- Priority: Medium
- Created: 2026-07-22
- Completed: 2026-07-31
- Model: Claude Sonnet 4
- Branch: feature/fix-runtime-safety
- Polished: 2026-07-29

## 目的

コードレビューで発見したランタイム安全性の問題を修正する。FD リーク、タイムアウト欠如によるスレッド枯渇、無限ループ、OOM リスク、バリデーション不足の 5 件が含まれる。

## 現状

### FD リーク (`XpcClient::logs`)

`src/core/client/xpc_client.rs` の `logs` メソッドで、`fds.len() < 2` のエラーパスが dup 済みの FD を close せずに `Err` を返す。直後の `fds[0] < 0 || fds[1] < 0` パスには close 処理があるが、`len < 2` パスにはない。

### Docker UnixStream タイムアウトなし (`DockerClient::request` / `request_with_content_type` / `remove_blocking`)

`src/core/client/docker_client.rs` の `request` メソッド、`request_with_content_type` メソッド、および `remove_blocking` メソッドの `UnixStream` に読み書きタイムアウト未設定。Docker デーモンが応答を止めると `spawn_blocking` ワーカーが恒久的にブロックする。`remove_blocking` は Drop 経路からも呼ばれるため、ハングすると呼び出しスレッドが直接ブロックする。なお issue 0039 (ログセッションタイムアウト) は「既存の `DockerClient::request` も同様にタイムアウト無し（本 issue はログセッションに限定する）」と明記しており、本 issue がそのスコープ外項目を引き受ける補完関係にある。

### read_file_to_vec の OOM リスク (`XpcClient::read_file_to_vec`)

`src/core/client/xpc_client.rs` の `read_file_to_vec` 関数がパイプ出力をサイズ上限なく `Vec` に読み込む。大量出力で OOM になり得る。

### FollowFdReader の空バッファ無限ループ (`FollowFdReader` の `Read` / `AsyncRead` impl)

`src/core/containers/async_container.rs` の `FollowFdReader` の同期 `Read::read` 実装で、空バッファ + follow=true + プロセス生存中に 100ms スリープの無限ループ。`Read` トレイトの契約（空バッファに `Ok(0)` 即時返却）に違反。非同期 `AsyncRead::poll_read` 実装にも同種の空バッファ問題がある。なお `FdReader::read_at` 自体は空バッファで正しく `Ok(0)` を返すため、ガードが必要なのは `FollowFdReader` の follow ループ側のみ。

### with_times(0) の未検証 (`LogWaitStrategy::with_times`)

`src/core/wait/log_strategy.rs` の `with_times` で `times = 0` を指定すると、`total >= self.times` が `0 >= 0` で即 true になり、読み取り結果を無視して即 ready と判定される。

## 設計方針

- FD リーク: `XpcClient::logs` の `fds.len() < 2` の分岐で有効な FD を close してから Err を返す
- タイムアウト: `DockerClient::request`、`request_with_content_type`、および `remove_blocking` の `UnixStream::connect` 直後に `set_read_timeout` / `set_write_timeout` を設定する。`DEFAULT_TIMEOUT` は `src/xpc/conn.rs` の private 定数 (macOS XPC 専用) のため流用不可。Docker クライアント用に新規定数を定義する (60 秒。issue 0039 のログセッションタイムアウトと整合)
- OOM: `read_file_to_vec` に `take()` で上限（64 MiB）を設け、超過時はエラーを返す。現状は `Vec<u8>` を返すシグネチャのため、`Result<Vec<u8>>` への変更と呼び出し側の追従が必要
- 無限ループ: `FollowFdReader` の `Read::read` と `AsyncRead::poll_read` の冒頭で `if buf.is_empty() { return Ok(0); }` (同期) / `Poll::Ready(Ok(()))` (非同期) を入れる
- with_times(0): `with_times` で `max(1, times)` でガードする

## 完了条件

- [x] `XpcClient::logs` のエラーパスで FD が close されること
- [x] `DockerClient::remove_blocking` の UnixStream にタイムアウトが設定されること（`request` / `request_with_content_type` は exec start 等の長時間操作のため意図的に除外）
- [x] `read_file_to_vec` にサイズ上限があること
- [x] `FollowFdReader` が空バッファで即座に `Ok(0)` / `Poll::Ready(Ok(()))` を返すこと (同期・非同期とも)
- [x] `with_times(0)` が 1 として扱われること
- [x] `CHANGES.md` に `[FIX]` エントリが記載されること
- [x] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

### FD リーク

`src/core/client/xpc_client.rs` の `logs` メソッドの `fds.len() < 2` エラーパスに FD close 処理を追加。重複する close ループを `close_valid_fds` ヘルパー関数に抽出。

### タイムアウト

`src/core/client/docker_client.rs` の `remove_blocking` に `DOCKER_STREAM_TIMEOUT` (60 秒) の読み書きタイムアウトを設定。`request` / `request_with_content_type` は exec start (Detach: false) や stop?t=N 等が正当に長時間ブロックするため一律タイムアウトを適用しない設計に変更。`wait_blocking` もコンテナ終了待ちのため意図的に除外し、rustdoc に理由を明記。

### OOM

`src/core/client/xpc_client.rs` の `read_file_to_vec` を `Result<Vec<u8>>` 返却に変更し、`take(64 MiB + 1)` で上限を設定。超過時は `ErrorKind::OutOfMemory` を返す。呼び出し側の exec スレッドも追従。

### 無限ループ

`src/core/containers/async_container.rs` の `FollowFdReader` の `Read::read` に `buf.is_empty()` ガード、`AsyncRead::poll_read` に `buf.remaining() == 0` ガードを追加。同期・非同期両方の回帰テスト（ハングガード付き）を追加。

### バリデーション

`src/core/wait/log_strategy.rs` の `with_times` に `max(1, times)` クランプを追加し、rustdoc に 0→1 クランプの挙動を明記。

### テスト

- `read_file_to_vec`: 小ファイル成功・64 MiB ちょうど成功（境界値）・64 MiB+1 失敗の 3 件
- `FollowFdReader`: 同期空バッファ・非同期空バッファの 2 件（ハングガード付き）
- `with_times`: 0 クランプ・正値保持の 2 件
