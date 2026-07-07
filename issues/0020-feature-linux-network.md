# 機能追加: Linux で with_network を対応する

- Priority: Medium
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-network
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで `with_network` を Docker の NetworkingConfig に反映する。

## 優先度根拠

ネットワーク分離はテスト間の干渉防止に重要であり、本家 testcontainers-rs でもネットワーク自動作成が行われる。Linux では Docker のユーザー定義ネットワークが利用可能であり、実装の障壁は低い。Medium。

## 現状

- `with_network`: `ContainerRequest` には保存されるが `build_container_config` で無視される
- macOS では XPC `containerCreate` の `networks[0].network` に反映されている (自動作成はしない)
- Docker Engine API では create JSON の `NetworkingConfig.EndpointsConfig.{network}` に相当する

## 設計方針

- `ContainerConfig` に `network: Option<String>` を追加する
- `CreateContainerBody` の JSON に `NetworkingConfig.EndpointsConfig.{network}: {}` を追加する
- macOS と同様にネットワークの自動作成は行わない (事前に `docker network create` が必要)
- 存在しないネットワークを指定した場合は Docker 側がエラーを返す

## 完了条件

- [ ] Linux で `with_network` が Docker の NetworkingConfig に反映されること
- [ ] 存在しないネットワーク指定時に適切なエラーが返ること
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
