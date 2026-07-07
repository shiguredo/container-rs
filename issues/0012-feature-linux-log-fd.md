# 機能追加: Linux でログ FD を取得し stdout/stderr/LogConsumer/WaitFor::Log を有効化する

- Priority: High
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-log-fd
- Polished:

## 目的

Linux (Docker Engine API) バックエンドでコンテナのログを取得できるようにする。現状は `AsyncRunner::start` がログ FD を渡さないため、`stdout()` / `stderr()` が空リーダーになり、`WaitFor::Log` が成立しない。

## 優先度根拠

`WaitFor::Log` は testcontainers の中核的な ready 条件であり、これが使えないと多くのイメージで startup timeout まで待つか、`WaitFor::Nothing` で妥協する必要がある。Linux バックエンドの実用性を大きく損なっているため High。

## 現状

- `AsyncRunner::start` の Linux 分岐は `ContainerAsync::new` に `stdout_fd: None, stderr_fd: None` を渡している
- `stdout()` / `stderr()` は FD が無いため `tokio::io::empty()` を返す
- `WaitFor::Log` は空リーダーに対してポーリングし、startup timeout まで待つ
- `LogConsumer` は FD が無いため配信タスクが起動しない
- `stdout_to_vec()` / `stderr_to_vec()` は常に空の `Vec<u8>` を返す

## 設計方針

Docker Engine API の `/containers/{id}/logs` エンドポイント (HTTP stream) を利用してログを取得する。macOS の FD ベースとは異なり、HTTP ストリームを `tokio::io::AsyncRead` に変換するアダプタが必要。

- `DockerClient` に `logs(id, follow, stdout, stderr)` メソッドを追加する
- Docker のログストリームは multiplexed (8 バイトヘッダ + payload) のため、demux アダプタを実装する
- `ContainerAsync` の Linux 分岐でログストリームを構築し、`stdout()` / `stderr()` / `LogConsumer` / `WaitFor::Log` が動作するようにする
- TTY 有効時はヘッダ無しの raw ストリームになる点に注意する

## 完了条件

- [ ] Linux で `stdout()` / `stderr()` がコンテナのログを返すこと
- [ ] Linux で `WaitFor::Log` (message_on_stdout / message_on_stderr / message_on_either_std) が動作すること
- [ ] Linux で `LogConsumer` がログフレームを受信すること
- [ ] Linux で `stdout_to_vec()` / `stderr_to_vec()` がログ内容を返すこと
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
