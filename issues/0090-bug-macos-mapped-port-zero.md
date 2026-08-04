# バグ: macOS の `with_mapped_port(0, ...)` が自動割当されず、ホストポート 0 のまま XPC に送られる

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-mapped-port-zero
- Polished: {YYYY-MM-DD}

## 目的

macOS で `with_mapped_port(0, container_port)` (Docker ではランダム割当の慣用) を指定した場合の挙動を、expose 経路と同じ自動割当 (または明示エラー) に統一する。

## 現状

- `src/core/client/container_cfg.rs` の `build_config` は、`with_exposed_port` / `Image::expose_ports` 由来のポートには `allocate_free_host_port` で空きホストポートを割り当てる
- 一方、`with_mapped_port` の明示マッピングは `host_port` をそのまま `PortCfg` に載せるため、`with_mapped_port(0, 80.tcp())` は `"hostPort":0` のまま XPC に送られる (expose 経路と非対称)
- Apple container が `hostPort: 0` をどう解決するかは未検証で、結果不定 (エラー or ポート 0 バインド)
- 加えて `parse_published_ports` (`src/core/client/xpc_client.rs`) はホストポート 0 のエントリをそのまま `Ports` に登録するため、`get_host_port_ipv4` が `Ok(0)` を返し得る

## 設計方針

- macOS の `build_config` で `host_port == 0` の明示マッピングを `allocate_free_host_port` 経由の自動割当に流す (expose 経路と統一)
- または現行の素通しをやめ、start 時に明示エラーにする (仕様の決定を要する)
- 自動割当にする場合は `parse_published_ports` 側の 0 ポート混入も併せて対処する (欠落・0 エントリのスキップ)

## 完了条件

- macOS で `with_mapped_port(0, port)` を指定した場合に、実際に割り当てられたホストポートで接続できること (統合テスト)、または明示エラーになること
- `parse_published_ports` がホストポート 0 / 欠落エントリを `Ports` に登録しないこと (単体テスト)

## 解決方法

- `src/core/client/container_cfg.rs` の `build_config` で `host_port == 0` の明示マッピングを自動割当に変更する
- `src/core/client/xpc_client.rs` の `parse_published_ports` で 0 / 欠落のホストポートエントリをスキップする
- それぞれ単体テストと、macOS の統合テスト (`tests/container_macos.rs`) を追加する
