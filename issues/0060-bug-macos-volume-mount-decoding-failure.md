# バグ: macOS の Mount::volume_mount が apiserver でデコード失敗して必ず起動に失敗する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-volume-mount-decoding
- Polished: {YYYY-MM-DD}

## 目的

macOS (Apple container) で `Mount::volume_mount` を使うと必ずコンテナ起動が失敗するのを修正する。統合テストで検出できるようにして再発を防ぐ。

## 現状

- `src/core/client/container_cfg.rs` の `VolType` (DisplayJson 実装) は `"cache":"auto"` / `"sync":"fsync"` を**文字列値**で JSON 出力する
- Apple container 1.2.0 の `Filesystem.FSType` (`CacheMode` / `SyncMode`) は raw value を持たない Swift enum で、合成 Codable は `{"auto":{}}` / `{"fsync":{}}` (単一キーオブジェクト) 形式しかデコードしない
- 実機確認: `Mount::volume_mount` 付き `start()` を実行すると `DecodingError.typeMismatch: Expected to decode Dictionary<String, Any> but found a string instead. Path: mounts[0].type.volume.cache` で失敗する
- 単体テストは JSON 文字列の contains 検証のみのため検出されない。macOS の統合テストに volume マウントの実機検証が皆無

## 設計方針

`VolType` の `cache` / `sync` を Apple container のエンコード形式 (`{"auto":{}}` / `{"fsync":{}}`) に合わせる。virtiofs / tmpfs の `{"type":{"virtiofs":{}}}` 形式は実測でデコード成功しており変更不要。

## 完了条件

- `Mount::volume_mount` 付きのコンテナが macOS 実機で起動でき、マウントがコンテナ内から確認できる
- 同経路の統合テストが追加され、CI (self-hosted macOS) で検証される

## 解決方法

- `src/core/client/container_cfg.rs` の `VolType` で `cache` / `sync` を単一キーオブジェクト形式で出力する (nojson の DisplayJson で表現)
- `tests/container_macos.rs` に volume マウントの統合テストを追加する (起動 → コンテナ内でマウント確認 → 掃除)
- 必要なら `docs/TESTCONTAINERS.md` のマウント対応表を実態に合わせる
