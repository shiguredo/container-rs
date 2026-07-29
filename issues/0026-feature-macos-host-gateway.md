# 機能追加: macOS で with_host の HostGateway を対応する

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/add-macos-host-gateway
- Polished: 2026-07-29

## 目的

macOS (Apple Container) バックエンドで `with_host(..., ExtraHost::HostGateway)` を実装する。現状は `"with_host(..., HostGateway) is not supported on macOS"` の明示エラー。

## 現状

- `ExtraHost::HostGateway` は `apply_extra_hosts` (`src/runners/async_runner.rs`) で明示エラーを返す。doc コメントにも「`HostGateway` はホストゲートウェイ IP が環境依存なため未対応とする」と記載されている
- Docker では `host-gateway` を `ExtraHosts` に渡すと、Docker がホストのゲートウェイ IP に解決する
- Apple Container のネットワークでは、ホストへの到達 IP は VM のゲートウェイになるが、環境依存で固定ではない
- `containerList` の `networks[0]` には `ipv4Gateway` フィールドがスキーマ上存在する (`docs/TESTCONTAINERS.md`)。ただし `src/` 配下で `ipv4Gateway` を読み取るコードは現在存在しない (未検証)
- 既存のテスト (`tests/container_macos.rs` の `async_host_gateway_removes_container`、`tests/container_sync_drop_macos.rs` の `sync_host_gateway_removes_container` / `keep_on_startup_failure_victim`) は HostGateway が macOS で**必ずエラーになること**を前提に構築されている。特に `keep_on_startup_failure_victim` は HostGateway のエラーをテストインフラとして利用している

## 設計方針

- `XpcClient` (`src/core/client/xpc_client.rs`) に `containerList` の `networks[0].ipv4Gateway` を読み取るメソッドを追加する (既存の `bridge_ip_address` が `ipv4Address` を読むパターンに倣い、`IpAddr` を返す)
- **実装前に `ipv4Gateway` が実環境で値を返すか実測で確認する**。空文字や null だった場合は、代替手段 (例: `ifconfig` 相当の exec、VM 設定の読み取り) を検討するか、本 issue を pending に移す
- `apply_extra_hosts` (`src/runners/async_runner.rs`) の `HostGateway` アームを、ゲートウェイ IP を取得して `/etc/hosts` に追記する処理に変更する (既存の `ExtraHost::Addr` と同じ exec 経路)
- ゲートウェイ IP が取得できない場合は明示エラーを維持する
- `apply_extra_hosts` は `ContainerAsync::new` の後に呼ばれるため、コンテナは running 状態であり `containerList` からゲートウェイ IP を取得可能

## 完了条件

- [ ] `with_host("myhost", ExtraHost::HostGateway)` がホストのゲートウェイ IP を `/etc/hosts` に追記すること
- [ ] ゲートウェイ IP が取得できない場合は明示エラーを返すこと
- [ ] 既存の HostGateway エラー前提テスト (`async_host_gateway_removes_container` / `sync_host_gateway_removes_container` / `keep_on_startup_failure_victim`) を実装後の挙動に合わせて修正すること (`keep_on_startup_failure_victim` は別のエラートリガに差し替える)
- [ ] `apply_extra_hosts` の doc コメントを実態に合わせて更新すること
- [ ] 統合テストが追加されていること
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が実装済みに更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
