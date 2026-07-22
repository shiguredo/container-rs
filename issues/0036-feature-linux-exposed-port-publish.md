# 機能追加: Linux で with_exposed_port をホストポート公開する

- Priority: High
- Created: 2026-07-22
- Completed:
- Model: Grok 4.5
- Branch: feature/linux-exposed-port-publish
- Polished:

## 目的

Linux (Docker Engine API) バックエンドで `with_exposed_port` / `Image::expose_ports` をホストポート公開に反映し、macOS および本家 testcontainers-rs と同じセマンティクスにする。

## 優先度根拠

README と `tests/nginx_http11.rs` が `with_exposed_port` を本線 API として使っている。Linux では create 時に `PortBindings` へ載らず、起動後の `get_host_port_ipv4` が `PortNotExposed` になる。macOS では既に自動割当済みで、他の Linux 未対応項目と違い fail-fast もされないため、黙って壊れる。回避策 `with_mapped_port(0, …)` はあるが、本線 API が OS で分岐するのは許容できない。High。

## 現状

- macOS (`src/core/client/container_cfg.rs`): `with_mapped_port` に加え、`req.expose_ports()` に空きホストポートを自動割当して `publishedPorts` に載せる
- Linux (`src/runners/async_runner.rs` の `build_container_config`): `req.ports()`（=`with_mapped_port`）だけを `ContainerConfig.ports` に渡す。`req.expose_ports()` は無視される
- Linux (`src/core/client/docker_client.rs`): `build_port_bindings` / `build_exposed_ports` は `ContainerConfig.ports` のみから組み立てるため、`with_exposed_port` だけのリクエストは `HostConfig.PortBindings` が空になる
- Docker の `ExposedPorts`  alone はメタデータであり、ホスト公開には `PortBindings` が必要
- `docs/TESTCONTAINERS.md` は `with_exposed_port` / `expose_ports` を Docker 側「対応」と書いており、実態と食い違う

## 設計方針

- Linux の `build_container_config` で、`req.expose_ports()` のうち `req.ports()` に未登場のコンテナポートを `PortMapping` として追加する
- ホストポートは `0` を渡す（Docker Engine API のランダム割当。macOS の事前 `allocate_free_host_port` とは意図的に異なる）
- 明示マッピング (`with_mapped_port`) があるコンテナポートは自動追加しない（macOS と同じ重複回避）
- `docker_client` の `build_port_bindings` / `build_exposed_ports` は既存のまま `ContainerConfig.ports` を読めばよい
- 起動後の割当結果は既存の `parse_ports`（`NetworkSettings.Ports`）で取得できる前提を維持する
- `docs/TESTCONTAINERS.md` の「対応」記述を実装後の実態に合わせて直す（備考に Linux は host port `0` で公開、など）

## 完了条件

- [ ] Linux で `with_exposed_port` のみのリクエストが `HostConfig.PortBindings` に載ること
- [ ] 起動後に `get_host_port_ipv4` が割当ホストポートを返すこと（`PortNotExposed` にならないこと）
- [ ] `with_mapped_port` と併用時、明示マッピング側が優先され自動追加で上書きされないこと
- [ ] macOS の既存自動割当挙動を壊さないこと
- [ ] 統合テスト（または Linux 向けユニット相当）が追加されていること
- [ ] `docs/TESTCONTAINERS.md` の該当行が実態と一致すること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
