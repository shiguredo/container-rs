# バグ: `with_mapped_port` の同一コンテナポートへの重複マッピングが検証されず、サイレントに 1 本へ潰れる

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-mapped-port-duplicate
- Polished: {YYYY-MM-DD}

## 目的

`with_mapped_port(8080, 80.tcp()).with_mapped_port(8081, 80.tcp())` のような同一コンテナポートへの重複マッピングを検出し、黙って片方が消える挙動をなくす。

## 現状

- `src/core/image/image_ext.rs` の `with_mapped_port` は `PortMapping` を Vec に push するだけで、同一 `container_port` の重複を検証しない
- Linux の `build_port_bindings` (`src/core/client/docker_client.rs`) は `"80/tcp"` をキーに `BTreeMap::insert` するため、**後勝ちでサイレントに 1 本だけ**が送信される (先の 8080 が黙って消える)
- macOS の `build_config` (`src/core/client/container_cfg.rs`) は重複 `PortCfg` をそのまま XPC に送る (挙動は Apple container 側に依存)
- `build_container_config` (`src/runners/async_runner.rs`) の expose 合成は「コンテナポート一致」で重複排除するため、`80/tcp → 8080` と `80/tcp → 8081` の両方が ports に残り、片方が静かに消える

## 設計方針

- `with_mapped_port` (または start 時) で同一 `container_port` の重複マッピングを検出して明示エラーにする (本家 testcontainers-rs の挙動を確認し、エラーにするか先勝ちにするか決める)
- Docker Engine API は同一コンテナポートへの複数ホストポートマッピング (複数 binding) をサポートするため、複数 binding を送る方式も選択肢 (その場合は `Ports` 側の解決も必要)

## 完了条件

- 同一コンテナポートへの重複マッピングが、黙って潰れずに明示エラー (または複数 binding として送信) になること
- 単体テストで重複検出が検証されること

## 解決方法

- `src/core/image/image_ext.rs` の `with_mapped_port` (または `ContainerRequest` の構築時) で重複を検出してエラーにする
- `tests/container_linux.rs` に重複マッピングのエラー検証テストを追加する
