# 機能追加: macOS で with_host の HostGateway を対応する

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/macos-host-gateway
- Polished:

## 目的

macOS (Apple Container) バックエンドで `with_host(..., HostGateway)` を実装する。現状は `"with_host(..., HostGateway) is not supported on macOS"` の明示エラー。

## 優先度根拠

`HostGateway` はコンテナからホスト側のサービスに接続する際に使う。Apple container のネットワーク構成ではホストへの到達方法が異なる可能性があり、需要が確認できていない。Low。

## 現状

- `ExtraHost::HostGateway` は `apply_extra_hosts` で明示エラーを返す
- Docker では `host-gateway` を `ExtraHosts` に渡すと、Docker がホストのゲートウェイ IP に解決する
- Apple container のネットワークでは、ホストへの到達 IP は VM のゲートウェイ (例: `192.168.64.1`) になるが、環境依存で固定ではない

## 設計方針

- Apple container のデフォルトネットワークのゲートウェイ IP を検出する方法を調査する
- `containerList` の `networks[0].ipv4Gateway` からゲートウェイ IP を取得できる可能性がある
- 取得したゲートウェイ IP を `/etc/hosts` に追記する (既存の `ExtraHost::Addr` と同じ経路)
- ゲートウェイ IP が取得できない場合は明示エラーを維持する

## 完了条件

- [ ] `with_host("myhost", HostGateway)` がホストのゲートウェイ IP を `/etc/hosts` に追記すること
- [ ] ゲートウェイ IP が取得できない場合は適切なエラーを返すこと
- [ ] 統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
