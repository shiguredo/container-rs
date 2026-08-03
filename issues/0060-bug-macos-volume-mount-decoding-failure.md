# バグ: macOS の Mount::volume_mount が apiserver でデコード失敗して必ず起動に失敗する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
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

- `src/core/client/container_cfg.rs` の `VolType` で `cache` / `sync` を単一キーオブジェクト形式で出力する (nojson の DisplayJson で表現。既存の `EmptyObj` と同じパターン)。`format` は設計方針に従い、実機確認の結果に応じて変更する
- `container_cfg.rs` の単体テストで、`cache` / `sync` が単一キーオブジェクト形式であり、`name` / `format` が文字列のままであることを JSON 文字列で検証する (contains 検証で十分)
- `tests/container_macos.rs` に volume マウントの統合テストを追加する (テスト名: `xpc_alpine_with_volume_mount`。起動 → コンテナ内でマウント確認 → 掃除)。`skip_if_ci` ガードと `with_startup_timeout` は既存の macOS 統合テストに合わせる。マウント確認は df の出力にマウントポイントが含まれることで行い、df に現れない場合は exec での読み書き確認に切り替える。volume は指定時に自動作成される (Apple container の CLI で確認済み。テスト実装時に library (XPC) 経由でも自動作成されることを確認し、事前作成が必要ならテストの前処理で CLI (`container volume create`) を呼ぶ)。テスト用ボリューム名は Apple のボリューム名制約 (英数字始まり・255 文字以内) に適合する `container_vol_{pid}_{nanos}` 形式でユニーク化し、テスト冒頭で同名ボリュームの事前削除を試みて冪等化する (前回実行の assert 失敗で残存したボリュームを除去して再実行可能にする)。クレートに volume 削除 API は無いため、掃除ではクレートの `container.rm()` に加えて Apple container CLI (`container volume rm`) を直接呼んで volume も削除する (名前付きボリュームはコンテナ削除後も残存するため)
- macOS の volume 統合テストは本 issue が担当する。Linux 側の `volume_mount` 統合テストは 0069 が担当する (0069 の解決方法の「別の既知バグ」は本 issue を指す)
- `tests/container_macos.rs` を変更するため、同一ファイルを変更する 0058 / 0059 / 0061 / 0068 / 0069 とマージ順に注意する
- `CHANGES.md` に `[FIX]` エントリを追加する
