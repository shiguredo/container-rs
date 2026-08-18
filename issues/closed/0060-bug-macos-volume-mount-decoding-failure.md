# バグ: macOS の Mount::volume_mount が apiserver でデコード失敗して必ず起動に失敗する

- Created: 2026-08-02
- Completed: 2026-08-03
- Branch: feature/fix-macos-volume-mount-decoding
- Polished: 2026-08-02

## 目的

macOS (Apple container) で `Mount::volume_mount` を使うと必ずコンテナ起動が失敗するのを修正する。単体テストで JSON 形式を固定し、統合テストで実機検証できるようにして再発を防ぐ。

## 現状

- `src/core/client/container_cfg.rs` の `VolType` (DisplayJson 実装) は `"cache":"auto"` / `"sync":"fsync"` を**文字列値**で JSON 出力する
- Apple container 1.2.0 の `Filesystem.FSType` (`CacheMode` / `SyncMode`) は raw value を持たない Swift enum で、合成 Codable は `{"auto":{}}` / `{"fsync":{}}` (単一キーオブジェクト) 形式しかデコードしない。この形式は Swift の合成 Codable の標準挙動であり、`apple/container` の `Sources/ContainerResource/Container/Filesystem.swift` で `CacheMode` (`on` / `off` / `auto`)・`SyncMode` (`full` / `fsync` / `nosync`) が raw value なしの enum として定義されている
- 実機確認: `Mount::volume_mount("data-volume", "/data")` 付き `start()` を実行すると `DecodingError.typeMismatch: Expected to decode Dictionary<String, Any> but found a string instead. Path: mounts[0].type.volume.cache` で失敗する
- volume の JSON エンコード形式 (cache / sync) を検証する単体テストが無く、macOS の統合テストに volume マウントの実機検証も皆無のため検出されない

## 設計方針

`VolType` の `cache` / `sync` を Apple container のエンコード形式 (`{"auto":{}}` / `{"fsync":{}}`) に合わせる。修正後の形式は Apple 側の合成 Codable のエンコード形式 (`{"cache":{"auto":{}},"sync":{"fsync":{}}}`) と一致する。virtiofs / tmpfs の `{"type":{"virtiofs":{}}}` 形式は実測でデコード成功しており変更不要。

`format` は mount(2) の fstype に直結するため、現行の `"raw"` 固定のままではデコード修正後にマウント失敗する可能性がある。Apple の CLI 実装はボリューム実体の format (ext4) を送るため、`format` も実態に合わせて変更することを検討する。最終的な `format` の値は統合テストの実機確認で確定する。

## 完了条件

- `Mount::volume_mount` 付きのコンテナが macOS 実機で起動でき、マウントがコンテナ内から確認できる (df の出力にマウントポイントが含まれることを確認する。df に現れない場合は exec での読み書き確認に切り替える)
- 単体テストで `cache` / `sync` の JSON 形式が `{"auto":{}}` / `{"fsync":{}}` であり、`name` / `format` が文字列のままであることが検証されている
- macOS の volume マウント統合テストが追加され、`RUN_CONTAINER_TESTS=1 cargo test --all-features` で検証される
- `CHANGES.md` に `[FIX]` エントリが追加されている

## 解決方法

- `src/core/client/container_cfg.rs` の `VolType` で `cache` / `sync` を単一キーオブジェクト形式 (`{"auto":{}}` / `{"fsync":{}}`) で出力する。汎用の `KeyedEmptyObj` として実装し、`format` は XPC で解決したボリューム実体の値を渡す
- `src/core/client/xpc_client.rs` に `resolve_volume` / `resolve_volumes` を追加する。Apple container は volume マウントを block デバイスとして扱い `source` にボリューム実体の絶対パスを要求するため、`volumeCreate` (既存なら `volumeInspect`) で実パスと `format` (ext4) を解決する。CLI (`Utility.containerConfigFromFlags`) と同じ挙動。ボリューム名の事前検証 (`is_valid_volume_name`) と `parse_volume_configuration` も追加する
- `src/runners/async_runner.rs` の macOS 分岐で、`containerCreate` 前に `resolve_volumes` を呼び、解決結果を `build_config` に渡す
- 単体テスト: `container_cfg.rs` に JSON 形式の固定テスト (`volume_mount_uses_apple_container_enum_json_format` / `volume_mount_without_resolution_emits_empty_source`)、`xpc_client.rs` に `is_volume_already_exists_error` / `parse_volume_configuration` / `is_valid_volume_name` のテストを追加する
- 統合テスト: `tests/container_macos.rs` に `xpc_alpine_with_volume_mount` (新規ボリュームの自動作成 → df と読み書きでマウント確認 → 掃除) と `xpc_alpine_with_existing_volume_mount` (既存ボリュームの already exists → inspect フォールバックの実機固定) を追加する。実機で全 84 テストの通過を確認済み
- `CHANGES.md` に `[FIX]` エントリを追加する
