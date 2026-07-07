# 機能追加: Linux で exec の stdout/stderr 取得と env_vars を実装する

- Priority: Medium
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-exec-output
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで exec の stdout/stderr 取得と `ExecCommand::with_env_vars` を実装する。

## 優先度根拠

exec の出力取得はコンテナ内のコマンド結果を検証する基本的な機能であり、`CmdWaitFor::StdOutMessage` / `StdErrMessage` の前提でもある。現状は exit code のみ取得できており、出力が必要なテストは代替手段 (ファイル書き出し + copy) が必要。Medium。

## 現状

- `DockerClient::exec` は `AttachStdout: false, AttachStderr: false` で exec を作成し、`Detach: true` で起動している
- stdout/stderr は常に空の `Vec<u8>`
- `CmdWaitFor::StdOutMessage` / `StdErrMessage` は Linux で明示エラー
- `ExecCommand::with_env_vars` は非空なら Linux で明示エラー

## 設計方針

- `DockerClient::exec` で `AttachStdout: true, AttachStderr: true` を設定し、`POST /exec/{id}/start` のレスポンスボディ (multiplexed stream) を読み取る
- Docker の exec ストリームは 8 バイトヘッダ (stream type + size) + payload の multiplexed 形式のため、demux して stdout/stderr に振り分ける
- `Env` フィールドに `ExecCommand::env_vars` を設定する (Docker は Env 省略時にコンテナ env を継承する)
- `CmdWaitFor::StdOutMessage` / `StdErrMessage` の Linux 分岐を有効にする

## 完了条件

- [ ] Linux で exec の stdout/stderr が取得できること
- [ ] Linux で `CmdWaitFor::StdOutMessage` / `StdErrMessage` が動作すること
- [ ] Linux で `ExecCommand::with_env_vars` が環境変数を exec に渡すこと
- [ ] `stdout_to_vec()` / `stderr_to_vec()` が exec 出力を返すこと
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
