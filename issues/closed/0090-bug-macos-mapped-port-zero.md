# バグ: macOS の `with_mapped_port(0, ...)` が自動割当されず、ホストポート 0 のまま XPC に送られる

- Created: 2026-08-04
- Completed: 2026-08-05
- Branch: feature/fix-macos-mapped-port-zero
- Polished: 2026-08-04

## 目的

macOS で `with_mapped_port(0, container_port)` (Docker ではランダム割当の慣用) を指定した場合の挙動を、expose 経路と同じ自動割当に統一する。

## 現状

- `src/core/client/container_cfg.rs` の `build_config` は、`with_exposed_port` / `Image::expose_ports` 由来のポートには `allocate_free_host_port` で空きホストポートを割り当てる
- 一方、`with_mapped_port` の明示マッピングは `host_port` をそのまま `PortCfg` に載せるため、`with_mapped_port(0, 80.tcp())` は `"hostPort":0` のまま XPC に送られる (expose 経路と非対称)
- `parse_published_ports` (`src/core/client/xpc_client.rs`) はホストポート 0 のエントリ (欠落も `unwrap_or(0)` で 0 に正規化される) をそのまま `Ports` に登録するため、`get_host_port_ipv4` が `Ok(0)` を返し得る

## 設計方針

- macOS の `build_config` で `host_port == 0` の明示マッピングを `allocate_free_host_port` 経由の自動割当に流す (expose 経路と統一)。根拠: Apple container には `hostPort: 0` のランダム割当が無いため事前割当が必要。本家 testcontainers-rs は `host_port` をそのまま送り Docker Engine のランダム割当に任せる (Docker の慣用 `-p 0:port` 相当)。Linux 側 (`src/runners/async_runner.rs`) も 0 をそのまま Docker Engine に送ってランダム割当に任せている (0036 で確定した方針。ユーザー可視の結果を OS 間で揃える)。明示エラー案は Linux との OS 間非対称を生むため不採用
- `parse_published_ports` でホストポート 0 / コンテナポート 0 (どちらか一方でも 0) のエントリをスキップする。役割分担: ホストポート 0 のスキップは自動割当後は到達不能な防御 (デーモン応答の異常系のみ)。コンテナポート 0 のスキップは必須 (コンテナポート 0 は `Tcp(0)` として `Ports` の最小キーになり、ポート未指定フォールバックが接続を試みるため)
- 注意: `with_mapped_port(0, 80.tcp())` と `with_exposed_port(80.tcp())` を併用した場合、重複チェック (container_port + proto ベース) により expose 側がスキップされ二重割当は起きない (mapped のみ割り当て)
- 注意: 0091 (bug) は pull 前検証 (`async_runner.rs`) を予定しており、本 issue の変更対象 (`build_config`) とは異なるが、ポート関連の変更が同居するため実装順序に注意する

## 完了条件

- macOS で `with_mapped_port(0, port)` を指定した場合に、実際に割り当てられたホストポート (非 0) で接続できること (統合テスト。nginx 等、コンテナ内で listen するプロセスを使う。published port 経由の実接続は Local Network Privacy により CI で使えないため、`RUN_HOST_NETWORK_TESTS=1` ゲート付きで実行する。「実際に割り当てられた」は `build_config` の事前割当の値であり、`allocate_free_host_port` は bind → 即 release のため、Apple container の実 bind までの間にポートを奪われるレースがある。レースが実現した場合は start が失敗し得るが、稀なケースであり許容する)
- `parse_published_ports` がホストポート 0 / コンテナポート 0 のエントリを `Ports` に登録しないこと (単体テスト)
- `docs/TESTCONTAINERS.md` の `with_mapped_port` 該当行が実装後の実態に合わせて更新されること

## 解決方法

- `src/core/client/container_cfg.rs` の `build_config` で `host_port == 0` の明示マッピングを `allocate_unique_free_host_port` 経由の自動割当に変更した (SCTP は `reject_sctp_ports` が先に落とすため到達しない)。expose 経路も同じ関数を使う
- ホストポートの重複 (固定ポート同士・固定ポートと割当結果の衝突) は `duplicate host port mapping` の明示エラーにした
- `allocate_unique_free_host_port` は割当済み集合と衝突しないポートを最大 64 回再試行で確保する (bind(0) → 即 release の再利用防止)
- `src/core/client/xpc_client.rs` の `parse_published_ports` でホストポート 0 / コンテナポート 0 のエントリをスキップし、`tracing::debug` でログを残す
- テスト: `build_config` の単体テスト (mapped(0) の自動割当・expose 併用の重複スキップ・64 個の mapped(0) のユニーク検証・UDP・固定ポート重複エラー・expose 複数) と `parse_published_ports` の単体テスト (0 スキップ) を追加した
- macOS の統合テスト (`tests/container_macos.rs`) に、alpine の mapped(0) 自動割当検証と、nginx の mapped(0) → published port 経由 HTTP 200 検証 (`test_container_http_wait` モジュール、`RUN_HOST_NETWORK_TESTS=1` ゲート) を追加した
- `docs/TESTCONTAINERS.md` と `with_mapped_port` の rustdoc を新挙動に合わせて更新した
