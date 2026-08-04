# バグ: `with_mapped_port` の同一コンテナポートへの重複マッピングが検証されず、サイレントに 1 本へ潰れる

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-mapped-port-duplicate
- Polished: 2026-08-04

## 目的

`with_mapped_port(8080, 80.tcp()).with_mapped_port(8081, 80.tcp())` のような同一コンテナポートへの重複マッピングを検出し、黙って片方が消える挙動をなくす。

## 現状

- `src/core/image/image_ext.rs` の `with_mapped_port` は `PortMapping` を Vec に push するだけで、同一 `container_port` の重複を検証しない
- Linux の `build_port_bindings` (`src/core/client/docker_client.rs`) は `"80/tcp"` をキーに `BTreeMap::insert` するため、**後勝ちでサイレントに 1 本だけ**が送信される (先の 8080 が黙って消える)
- macOS の `build_config` (`src/core/client/container_cfg.rs`) は重複 `PortCfg` をそのまま XPC に送る (挙動は Apple container 側に依存)
- `build_container_config` (`src/runners/async_runner.rs`) の expose 合成は「mapped vs expose_ports」間の重複排除のみ行い、mapped 同士の重複はそのまま ports に残る (片方が静かに消えるのは送信側の後勝ちによる)

## 設計方針

- 重複検出は **pull 前** (`with_mapped_port` は公開 API のシグネチャ (`ContainerRequest<I>` を返す。本家 testcontainers-rs 互換) を維持するため変更しない。`ContainerRequest` にエラーを返せる「構築時」の地点は存在しない) に行う。検出は Linux の `linux_unsupported_request_reason` / macOS の ID 検証と同じ地点 (pull 前) に置き、不要な pull を避ける
- 検出ロジックは純粋関数 (`&[PortMapping] -> Result<()>`) として OS 非依存モジュールに切り出し、Linux / macOS の両方の pull 前検証で呼ぶ (`docker_client` は Linux ゲート、`container_cfg` は macOS ゲートのため、どちらかに置くと他 OS でコンパイル不能になる)。既存の `reject_sctp_ports` / `linux_unsupported_request_reason` と同じ fail-fast パターン。macOS 側の `reject_sctp_ports` は既存どおり重複検出より先に実行する (SCTP 未対応エラーを優先)
- 重複の判定は `ContainerPort` 完全一致 (proto 込み) とする (同番号・異プロトコルは別エントリとして共存させる。既存テスト `different_protocol_same_number_keeps_both` を維持する)
- エラーメッセージには重複したコンテナポートと競合する 2 つのホストポートを含める
- 先勝ち・複数 binding は不採用 (先勝ちは後勝ちと同じ「黙って 1 本に潰れる」挙動を仕様化するだけ。複数 binding は `Ports` 型 (`BTreeMap<ContainerPort, u16>`) の構造変更・公開 API (`get_host_port_ipv4`) の変更・macOS 側の未検証を伴うため別 issue の範囲とする)
- mapped vs expose の同一コンテナポート重複は 0036 で確定した方針 (mapped 優先・expose スキップ) を維持する (本 issue の対象外)。expose 同士の重複も 0036 の確定仕様 (冪等宣言としてサイレント 1 本化) を維持する (mapped はユーザーの明示マッピングのためエラーにするという非対称は意図的)
- 対象はコンテナポート単位の重複のみ。同一ホストポート・異コンテナポートの重複は対象外 (Linux は Docker が `port is already allocated` で明示エラーにする。macOS は Apple container 依存のまま)
- 注意: 0090 (bug) は `build_config` の変更を予定しており、macOS の pull 前検証 (async_runner.rs) とは対象が異なるが、ポート関連の変更が同居するため実装順序に注意する

## 完了条件

- 同一 `ContainerPort` (proto 込み) への重複マッピングが、黙って潰れず明示エラーになること (pull 前検証。単体テスト)
- 既存の mapped vs expose 優先テスト (`async_runner.rs` の mapped 優先) と同番号・異プロトコル共存テストが引き続き通ること

## 解決方法

- 重複検出の純粋関数を切り出し、Linux / macOS の pull 前検証 (`src/runners/async_runner.rs` の `linux_unsupported_request_reason` / ID 検証と同地点) で呼ぶ
- 単体テストに重複マッピングのエラー検証を追加する (純粋関数の単体テストで検証する)
