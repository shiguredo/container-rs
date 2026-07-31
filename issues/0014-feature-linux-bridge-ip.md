# 機能追加: Linux で get_bridge_ip_address を実装する

- Priority: Low
- Created: 2026-07-21
- Completed: 2026-07-31
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-bridge-ip
- Polished: 2026-07-29

## 目的

Linux (Docker Engine API) バックエンドで `get_bridge_ip_address` を実装する。

## 優先度根拠

`get_bridge_ip_address` はコンテナのブリッジネットワーク IP を取得する API で、ホストからの直接接続に使う。published port 経由の接続が主流であり、必須度は低いため Low。

## 現状

- `ContainerAsync::get_bridge_ip_address` の Linux 分岐は `"get_bridge_ip_address is not supported on Linux"` の明示エラー (`src/core/containers/async_container.rs:246-248`)
- `tests/container_linux.rs:228-230` の `unimplemented_boundaries_return_err` テストがこの Err を明示的にアサートしている
- Docker Engine API の `GET /containers/{id}/json` (inspect) の `NetworkSettings.Networks.{network}.IPAddress` から取得可能
- macOS 実装 (`xpc_client.rs:238-267`) は `networks[0]` (先頭ネットワーク) の `ipv4Address` を CIDR から抽出して返す。ネットワーク不在時はエラー

## 設計方針

- closed issue 0044 (`issues/closed/0044-add-linux-healthcheck.md`) の前例に従い、`ContainerSnapshot` は変更しない。`DockerClient` に `bridge_ip_address(&self, id: &str) -> Result<IpAddr>` のような専用メソッドを新設し、inspect レスポンスの `NetworkSettings.Networks` から IP をパースする
- ネットワーク選択は macOS 実装に合わせて先頭ネットワーク (`Networks` マップの最初のエントリ) の `IPAddress` を取る。`bridge` ネットワーク名のハードコードはしない (カスタムネットワーク対応のため)
- `IPAddress` が空文字列の場合 (host ネットワークモード等) はエラーを返す (macOS 実装の「ネットワーク無し → エラー」と対称)。`Networks` マップが空または欠落している場合も同様にエラーを返す
- IPv4 (`IPAddress`) のみ取得する。IPv6 (`GlobalIPv6Address`) は対象外
- Docker の `IPAddress` はプレーンな IP アドレス文字列 (CIDR なし) であり、macOS の CIDR 表記とは異なる。パースは単純な `IpAddr::from_str` で十分

## 完了条件

- [ ] Linux で `get_bridge_ip_address` がコンテナの IP アドレスを返すこと
- [ ] `tests/container_linux.rs` の `unimplemented_boundaries_return_err` から `get_bridge_ip_address` の Err 期待 (228-230 行) を削除すること (他の Err 期待は残す)
- [ ] 統合テストが追加されていること
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` と `README.md` の `get_bridge_ip_address` 関連箇所が実装済みに更新されること (`README.md:27` の「bridge IP 取得...未対応」の記述を含む)
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

`DockerClient` に `bridge_ip_address` メソッドを追加し、inspect の `NetworkSettings.Networks` 先頭エントリの `IPAddress` から IP アドレスを取得するようにした。

1. `DockerClient::bridge_ip_address` を新規追加し、`GET /containers/{id}/json` の `NetworkSettings.Networks` を JSON オブジェクトとしてパースし、先頭エントリ（キーのアルファベット順）の `IPAddress` を `IpAddr::from_str` でパースして返す。空文字列・Networks 欠落時はエラーを返す
2. `ContainerAsync::get_bridge_ip_address` の Linux 分岐を明示エラーから `DockerClient::bridge_ip_address` への委譲に変更した
3. `unimplemented_boundaries_return_err` テストから `get_bridge_ip_address` の Err 期待を削除し、`alpine_bridge_ip_address` 統合テストを追加した
4. README / TESTCONTAINERS.md / SKILL.md の残ギャップ記述から bridge IP 取得を削除し、API 対応表を「対応」に更新した
5. CHANGES.md に `[ADD]` エントリを追加した

変更ファイル: `src/core/client/docker_client.rs`、`src/core/containers/async_container.rs`、`tests/container_linux.rs`、`README.md`、`docs/TESTCONTAINERS.md`、`skills/shiguredo-container/SKILL.md`、`CHANGES.md`
