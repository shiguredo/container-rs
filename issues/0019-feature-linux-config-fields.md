# 機能追加: Linux で Config 未反映項目を対応する (hostname/open_stdin)

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-config-fields
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで `ContainerRequest` に保存されているが Docker の create JSON に未反映の Config 項目を対応する。

## 優先度根拠

`hostname` はネットワークテスト、`open_stdin` はインタラクティブなコンテナで必要。ただし published port 経由の接続が主流であり、`open_stdin` のユースケースは限定的なため Low。

## 現状

- `with_hostname`: `ContainerRequest` には保存されるが `build_container_config` で無視される
- `with_open_stdin`: 同上
- Docker Engine API の create JSON では `Config.Hostname` / `Config.OpenStdin` に相当する

## 設計方針

- `ContainerConfig` に `hostname: Option<String>` / `open_stdin: Option<bool>` を追加する
- `build_container_config` で `ContainerRequest` から値を映射する
- `CreateContainerBody` の JSON に `Hostname` / `OpenStdin` を追加する

## 完了条件

- [ ] Linux で `with_hostname` が Docker の Config.Hostname に反映されること
- [ ] Linux で `with_open_stdin` が Docker の Config.OpenStdin に反映されること
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
