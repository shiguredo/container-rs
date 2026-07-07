# 機能追加: Linux で exit_code を実装する

- Priority: Medium
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-exit-code
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで `ContainerAsync::exit_code` を実装する。

## 優先度根拠

exit code の取得はコンテナの正常終了検証に必要であり、`ExitWaitStrategy` の前提でもある。ただし `exec` の exit code は既に取得できており、コンテナ全体の exit code は補助的な用途が多いため Medium。

## 現状

- `ContainerAsync::exit_code` の Linux 分岐は `"exit_code() is not implemented on Linux"` の明示エラー
- macOS ではバックグラウンドの `containerWait` スレッドが exit code をキャッシュしている
- Docker Engine API には `POST /containers/{id}/wait` がある

## 設計方針

- macOS と同様に、`AsyncRunner::start` の Linux 分岐でバックグラウンドの wait スレッドを起動する
- `DockerClient` に `wait(id)` メソッドを追加し、`POST /containers/{id}/wait` を呼ぶ
- `WaitState` の世代管理は macOS と共通の仕組みを使う
- `exit_code()` は `WaitState` のキャッシュを参照し、未観測時は `None` を返す (macOS と同じ挙動)

## 完了条件

- [ ] Linux で `exit_code()` がコンテナ終了後に exit code を返すこと
- [ ] Linux で `exit_code()` がコンテナ実行中に `None` を返すこと
- [ ] 再 start 後に世代管理が正しく動作すること
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
