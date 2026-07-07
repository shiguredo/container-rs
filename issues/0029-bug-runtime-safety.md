# バグ: FD リーク・タイムアウト欠如・無限ループ等のランタイム安全性修正

- Priority: Medium
- Created: 2026-07-22
- Completed:
- Model: Claude Sonnet 4

## 目的

コードレビューで発見したランタイム安全性の問題を修正する。FD リーク、タイムアウト欠如によるスレッド枯渇、無限ループ、バリデーション不足が含まれる。

## 優先度根拠

いずれも特定の条件下でリソースリークやハングを引き起こすが、テスト用途では発生頻度が低い。ただし OSS 公開後に多様な環境で使われると顕在化する可能性があるため Medium。

## 現状

### FD リーク (src/core/client/xpc_client.rs:195-207)

`logs()` で `fds.len() < 2` のエラーパスが、dup 済みの FD を close せずに `Err` を返す。

### Docker UnixStream タイムアウトなし (src/core/client/docker_client.rs:467-480)

`request()` の `UnixStream` に読み書きタイムアウト未設定。Docker デーモンが応答を止めると `spawn_blocking` ワーカーが恒久的にブロックする。

### read_file_to_vec の OOM リスク (src/core/client/xpc_client.rs:645-648)

パイプ出力をサイズ上限なく `Vec` に読み込む。大量出力で OOM になり得る。

### FollowFdReader の空バッファ無限ループ (src/core/containers/async_container.rs:878-886)

同期 `Read::read` で、空バッファ + follow=true + プロセス生存中に 100ms スリープの無限ループ。`Read` トレイトの契約（空バッファに `Ok(0)` 即時返却）に違反。

### with_times(0) の未検証 (src/core/wait/log_strategy.rs:100-102)

`times = 0` でログを 1 バイトも読まずに ready と判定される。

## 設計方針

- FD リーク: `fds.len() < 2` の分岐で有効な FD を close してから Err を返す
- タイムアウト: `UnixStream::connect` 直後に `set_read_timeout` / `set_write_timeout` を設定する（DEFAULT_TIMEOUT 60 秒を流用）
- OOM: `read_file_to_vec` に `take()` で上限（64 MiB）を設け、超過時はエラーを返す
- 無限ループ: `read_at` / `read` の冒頭で `if buf.is_empty() { return Ok(0); }` を入れる
- with_times(0): `with_times` で `max(1, times)` でガードする

## 完了条件

- [ ] `logs()` のエラーパスで FD が close されること
- [ ] Docker UnixStream にタイムアウトが設定されること
- [ ] `read_file_to_vec` にサイズ上限があること
- [ ] `FollowFdReader` が空バッファで即座に `Ok(0)` を返すこと
- [ ] `with_times(0)` が 1 として扱われること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
