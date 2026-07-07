# 機能追加: Linux で get_bridge_ip_address を実装する

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-bridge-ip
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで `get_bridge_ip_address` を実装する。

## 優先度根拠

`get_bridge_ip_address` はコンテナのブリッジネットワーク IP を取得する API で、ホストからの直接接続に使う。published port 経由の接続が主流であり、必須度は低いため Low。

## 現状

- `ContainerAsync::get_bridge_ip_address` の Linux 分岐は `"get_bridge_ip_address is not supported on Linux"` の明示エラー
- Docker Engine API の `GET /containers/{id}/json` (inspect) の `NetworkSettings.Networks.{network}.IPAddress` から取得可能

## 設計方針

- `DockerClient::container_state` の inspect レスポンスから `NetworkSettings.Networks.bridge.IPAddress` (または接続済みネットワークの IP) をパースする
- `ContainerSnapshot` に `bridge_ip: Option<IpAddr>` フィールドを追加するか、別途メソッドを設ける

## 完了条件

- [ ] Linux で `get_bridge_ip_address` がコンテナの IP アドレスを返すこと
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
