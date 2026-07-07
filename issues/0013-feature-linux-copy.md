# 機能追加: Linux で copy_file_from / copy_to を実装する

- Priority: Medium
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-copy
- Polished:

## 目的

Linux (Docker Engine API) バックエンドでコンテナとのファイルコピー (`copy_file_from` / `copy_to`) を実装する。

## 優先度根拠

ファイルコピーはテストのセットアップ (設定ファイルの投入) や検証 (生成物の取得) で頻繁に使う機能であるが、ライフサイクル系 (start/stop/exec) と比べると代替手段 (exec でのファイル操作、bind mount) があるため Medium。

## 現状

- `ContainerAsync::copy_file_from` の Linux 分岐は `"copy_file_from() is not implemented on Linux"` の明示エラー
- `AsyncRunner::start` の Linux 分岐は `copy_to_sources` が非空なら `"copy_to() is not implemented on Linux"` の明示エラー
- Docker Engine API には `GET /containers/{id}/archive` (copy from) と `PUT /containers/{id}/archive` (copy to) がある

## 設計方針

- `DockerClient` に `copy_from(id, path)` と `copy_to(id, path, data)` メソッドを追加する
- Docker の archive API は tar 形式のため、tar のエンコード/デコードが必要
- `copy_file_from`: `GET /containers/{id}/archive?path={path}` で tar を取得し、展開して `CopyFileFromContainer` に渡す
- `copy_to`: `PUT /containers/{id}/archive?path={target}` で tar を送信する
- tar の実装は `tar` crate の追加または自前実装のいずれか (依存最小方針を考慮)

## 完了条件

- [ ] Linux で `copy_file_from` がファイルを取得できること
- [ ] Linux で `with_copy_to` / `copy_to_sources` がファイルを投入できること
- [ ] ディレクトリのコピーは `IsDirectory` エラーで拒否すること (macOS と同じ挙動)
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
