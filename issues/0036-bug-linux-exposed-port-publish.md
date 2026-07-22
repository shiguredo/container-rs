# バグ修正: Linux で with_exposed_port をホストポート公開する

- Priority: High
- Created: 2026-07-22
- Completed: 2026-07-22
- Model: Grok 4.5
- Branch: feature/fix-linux-exposed-port-publish
- Polished: 2026-07-22

## 目的

Linux (Docker Engine API) バックエンドで、既存 API `GenericImage::with_exposed_port` / `Image::expose_ports` がホストポート公開に反映されない不具合を直す。

ユーザー可視の結果（起動後に `get_host_port_ipv4` 等が割当ホストポートを返すこと）を macOS および本家 testcontainers-rs と揃える。ホストポートの割当手段は OS ごとに異なる（後述）。

## 優先度根拠

README の本線例が `with_exposed_port` を使う。Linux では create 時の `HostConfig.PortBindings` が空のままになり、起動後および `WaitFor::http` など start 中の ready 待機で `get_host_port_ipv4` が `PortNotExposed` になる。

macOS では既に修正済み。他の Linux 未対応項目と違い `linux_unsupported_request_reason` でも fail-fast されないため、黙って壊れる。公開 API・docs は「使える」前提なのに Linux だけ欠落している。High。

## 現状

- macOS (`src/core/client/container_cfg.rs`): `expose_ports` をホスト公開済み（事前 `allocate_free_host_port`。重複判定は番号 + プロトコル）
- Linux (`src/runners/async_runner.rs` の `build_container_config`, 約 464 行): `req.ports()` だけを `ContainerConfig.ports` に渡す。`req.expose_ports()` は無視される
- Linux (`linux_unsupported_request_reason`, 約 497–533 行): expose 系は対象外
- Linux (`src/core/client/docker_client.rs`): `build_port_bindings` / `build_exposed_ports` は `ContainerConfig.ports` のみから組み立てる。`host_port` の `0` は文字列 `"0"` になる
- `docs/TESTCONTAINERS.md`: `GenericImage::with_exposed_port` は備考空のまま Docker「対応」。`Image` / `ContainerRequest` の `expose_ports` 備考「公開は `ports()` / mapped port 経由」は本修正後に嘘になる
- `tests/container_linux.rs` に `with_exposed_port` の検証は無い。macOS には `xpc_alpine_exposed_port_auto_mapping` 等がある
- `async_runner.rs` の既存 `mod tests` は `#[cfg(all(test, target_os = "macos"))]` のため、Linux ユニットは別モジュールが必要

## 設計方針

- 変更は Linux の `build_container_config` に閉じる。`docker_client` / macOS `container_cfg` / `GenericImage` / `ImageExt` / `get_host_port_*` のシグネチャは触らない
- `req.expose_ports()` のうち `req.ports()` に未登場の `ContainerPort`（番号 + プロトコル）だけを `PortMapping::new(0, exposed)` で追加する。二重 expose・`with_mapped_port` 併用（`host_port == 0` の明示マッピング含む）とも同判定で 1 本（macOS と同型）
- ホストポート `0` を渡す理由: Docker Engine API にランダム割当を任せる。結果は既存の `parse_ports`（start 後 inspect の `NetworkSettings.Ports`）で回収する。macOS は XPC 都合で事前割当しており、割当手段は意図的に異なる
- fail-fast にはしない。SCTP 拒否は macOS 専用のまま。IPv6 追加対応はスコープ外
- `WaitFor::http` 専用の Linux 統合テストはスコープ外とする。`HttpWaitStrategy` は ready 中に `get_host_port_ipv4` を 1 回解決するだけなので、PortBindings 配線と起動後ホストポート取得が正しければ同経路も直る

## 完了条件

- [ ] Linux で `with_exposed_port` のみのリクエストが、create 前の `ContainerConfig.ports` に `host_port == 0` の `PortMapping` として載ること
- [ ] 起動後に `get_host_port_ipv4`（または `ports().map_to_host_port_ipv4`）が非 0 の割当ホストポートを返すこと（`PortNotExposed` にならないこと）
- [ ] `with_mapped_port` と `with_exposed_port` を同番号・同プロトコルで併用したとき、明示マッピング側が優先され自動追加で上書きされないこと
- [ ] 同番号・異プロトコル（例: mapped `80.tcp` + expose `80.udp`）では両方載ること
- [ ] ユニットテスト（必須）: `src/runners/async_runner.rs` に `#[cfg(all(test, target_os = "linux"))] mod linux_tests` を新設し、`build_container_config` を直接検証する。既存の macOS 向け `mod tests` は触らない。ケース: expose のみ / mapped 優先 / 異 proto / 空 expose / 二重 expose は 1 本。create 前は `host_port == 0`
- [ ] 統合テスト（必須）: `tests/container_linux.rs` に追加する。セマンティクスは macOS `xpc_alpine_exposed_port_auto_mapping` 相当だが、`skip_if_ci` は使わない（Linux CI `test-linux-docker` で必ず実行）。命名に `xpc_` を付けない。起動後ホストポートが非 0
- [ ] Linux 向けテストの成立は Linux ホストまたは CI の `test-linux-docker` で確認すること（macOS 上の `cargo test` だけでは足りない）
- [ ] `docs/TESTCONTAINERS.md` の expose / `with_exposed_port` 該当行を実装後の実態に合わせること。少なくとも「公開は `ports()` / mapped port 経由」を書き換え、Linux は `HostPort=0` の `PortBindings`、macOS は事前割当と備考する
- [ ] `CHANGES.md` の `## develop` に `[FIX]` エントリと担当者行 (`- @ユーザー名`) を追記すること（公開 API シグネチャは不変。docs 変更は changelog に載せない）
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

1. `src/runners/async_runner.rs` の `build_container_config` で、`req.ports()` をベースに `req.expose_ports()` のうち未登場の `ContainerPort` を `PortMapping::new(0, exposed)` で追加した
2. `docker_client` / macOS / 公開 API シグネチャは変更していない（既存の `config.ports` → `PortBindings` 経路を利用）
3. `#[cfg(all(test, target_os = "linux"))] mod linux_tests` を新設し、expose のみ / mapped 優先 / mapped(0) 優先 / 異 proto / 空 / 二重 expose を検証した
4. `tests/container_linux.rs` に `alpine_exposed_port_auto_mapping` を追加した（`skip_if_ci` なし）
5. `docs/TESTCONTAINERS.md` の expose / `with_exposed_port` 備考を実態に合わせ、`CHANGES.md` に `[FIX]` を追記した
