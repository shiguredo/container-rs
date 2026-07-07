# 機能追加: Linux で with_host を対応する (ExtraHosts)

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/linux-extra-hosts
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで `with_host` を Docker の HostConfig.ExtraHosts に反映する。

## 優先度根拠

`with_host` はコンテナ内の `/etc/hosts` にエントリを追加する機能で、特定のホスト名を特定の IP に解決させたいテストで使う。macOS では exec による `/etc/hosts` 追記で対応しているが、Linux では Docker の `ExtraHosts` が直接利用可能。ただしユースケースは限定的なため Low。

## 現状

- `with_host`: `ContainerRequest` には保存されるが `build_container_config` で無視される
- macOS では `ExtraHost::Addr` をコンテナ起動後に exec で `/etc/hosts` に追記している。`ExtraHost::HostGateway` は明示エラー
- Docker Engine API では `HostConfig.ExtraHosts` に `["hostname:ip"]` 形式で設定する

## 設計方針

- `ContainerConfig` に `extra_hosts: Vec<String>` を追加する
- `build_container_config` で `ContainerRequest::hosts()` から `"hostname:ip"` 形式の文字列を生成する
- `ExtraHost::HostGateway` は `"hostname:host-gateway"` として渡す (Docker は `host-gateway` を特殊値として扱う)
- `CreateContainerBody` の `HostConfig` JSON に `ExtraHosts` を追加する

## 完了条件

- [ ] Linux で `with_host("myhost", "1.2.3.4")` が Docker の HostConfig.ExtraHosts に反映されること
- [ ] Linux で `with_host("myhost", HostGateway)` が `host-gateway` として反映されること
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
