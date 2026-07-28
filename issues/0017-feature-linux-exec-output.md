# 機能追加: Linux で exec の stdout/stderr 取得を実装する

- Priority: Medium
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-exec-output
- Polished: 2026-07-29
- Reporter: @voluntas

## 目的

Linux (Docker Engine API) バックエンドで exec の stdout/stderr 取得と `CmdWaitFor::StdOutMessage` / `StdErrMessage` を実装する。`ExecCommand::with_env_vars` は issue 0050 で対応する。

## 優先度根拠

exec の出力取得はコンテナ内のコマンド結果を検証する基本的な機能であり、`CmdWaitFor::StdOutMessage` / `StdErrMessage` の前提でもある。現状は exit code のみ取得できており、出力が必要なテストは代替手段 (ファイル書き出し + copy) が必要。Medium。

利用者フィードバック (mqtt-rs の移行) でも具体的な需要が確認されている:

- `exec_before_ready` で「コンテナの中からポーリングして成功するまで待つ」という本家どおりの使い方ができる
- Mosquitto の起動待ちシェルや SCRAM 用の API リトライをテスト側に抱えずに済む道が増える
- 失敗時にコンテナ内コマンドの出力で原因を追える

## 現状

- `DockerClient::exec` は `AttachStdout: false, AttachStderr: false` で exec を作成し、`Detach: true` で起動している (`docker_client.rs:221-261`)
- `Detach: true` のため `POST /exec/{id}/start` のレスポンスボディは空。完了待ちは `GET /exec/{id}/json` の inspect ポーリングループ (30 秒タイムアウト付き, `docker_client.rs:274-318`) で行っている
- stdout/stderr は常に空の `Vec<u8>`
- `CmdWaitFor::StdOutMessage` / `StdErrMessage` は Linux で明示エラー (`async_container.rs:381-387, 399-405`)
- `tests/container_linux.rs:239-264` の `exec_unsupported_options_return_err` テストが上記の Err を明示的にアサートしている
- demux ロジックは `docker_log_stream.rs:151-246` の `FrameDemuxer` (8 バイトヘッダの multiplex フレームを stdout/stderr に分離) が既に存在するが、プライベートであり `SharedLogBuffer` を引数に取る

## 設計方針

- `DockerClient::exec` で `AttachStdout: true, AttachStderr: true, Detach: false` を設定する。`Detach: true` ではレスポンスボディが空になるため、`false` への変更が必須
- `Detach: false` に変更すると `POST /exec/{id}/start` のレスポンスボディが multiplexed stream になる。既存の inspect ポーリングループ (`docker_client.rs:274-318`) は不要になるため削除し、ストリーム読み取りが完了待ちを兼ねる。exit code はストリーム EOF 後の inspect (`GET /exec/{id}/json`) で取得する
- ストリーム読み取りは既存の `DockerClient::request()` で全ボディを蓄積し、返却後に demux する。exec の出力はログと異なり有界 (プロセス終了で EOF) なため、全蓄積で十分。`ResponseDecoder` による增量読み取りは不要
- タイムアウトは既存の 30 秒ポーリングタイムアウトに代わり、`startup_timeout` 相当の機構を適用するか、ストリーム読み取りにタイムアウトを設定する。具体的な方針は実装時に決定するが、無期限ブロックにはしない
- Docker の exec ストリームは 8 バイトヘッダ (stream type + size) + payload の multiplexed 形式。既存の `FrameDemuxer` (`docker_log_stream.rs:151-246`) は `SharedLogBuffer` 依存のためそのまま再利用できない。exec 用に `Vec<u8>` に書き込む demux を新規実装するか、`FrameDemuxer` からヘッダパース部分を抽出して共用する
- `CmdWaitFor::StdOutMessage` / `StdErrMessage` の Linux 分岐を有効にする

## 完了条件

- [ ] Linux で exec の stdout/stderr が取得できること
- [ ] Linux で `CmdWaitFor::StdOutMessage` / `StdErrMessage` が動作すること
- [ ] `ExecResult::stdout_to_vec()` / `ExecResult::stderr_to_vec()` が exec 出力を返すこと
- [ ] `tests/container_linux.rs` の `exec_unsupported_options_return_err` から `StdOutMessage` / `StdErrMessage` の Err 期待を削除すること
- [ ] 統合テストが追加されていること
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の exec 出力関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
