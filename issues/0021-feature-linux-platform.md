# 機能追加: Linux で with_platform を対応する

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-platform
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで `with_platform` を Docker の create/pull に反映する。

## 優先度根拠

Linux 上で異なるアーキテクチャのコンテナを動かすには QEMU/binfmt_misc のセットアップが必要であり、CI 環境での需要は限定的。macOS の Rosetta のような透過的なエミュレーションが Linux には無いため、実用性は環境に大きく依存する。Low。

## 現状

- `with_platform`: `ContainerRequest` には保存されるが `build_container_config` で無視される
- macOS では `normalize_platform` で正規化し、XPC の `rosetta` / `ociPlatform` / `platform.architecture` に反映している
- Docker Engine API では `POST /images/create` の `platform` パラメータと `POST /containers/create` の `platform` フィールドに相当する

## 設計方針

- `pull_image` 時に `platform` クエリパラメータを付与する
- `create_container` 時に `platform` フィールドを JSON に含める
- macOS の `normalize_platform` と同じ正規化ロジックを共有する

## 完了条件

- [ ] Linux で `with_platform("linux/amd64")` が pull / create に反映されること
- [ ] 不正な platform 文字列は無視されること (macOS と同じ挙動)
- [ ] 統合テストが追加されていること (amd64 ホスト上での arm64 指定等)
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
