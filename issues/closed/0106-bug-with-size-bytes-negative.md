# バグ: Mount::with_size_bytes / with_mode が負値を検証なしで素通しする

- Created: 2026-08-12
- Completed: 2026-08-14
- Branch: feature/fix-tmpfs-options-negative-validation
- Polished: 2026-08-12

## 目的

tmpfs マウントのサイズ・mode に負値が指定されたときに、検証なしで OS 側 (XPC / Docker Engine) に送られるのを防ぐ。

## 現状

`src/core/mounts.rs` の `with_size_bytes` は `i64` をそのまま保持し、`with_mode` も同様に `i64` を素通しする。

```rust
pub fn with_size_bytes(mut self, size: i64) -> Self {
    self.tmpfs_options.get_or_insert_with(MountTmpfsOptions::default).size_bytes = Some(size);
    self
}
```

- `parse_size` (文字列形式の `with_size`) は負値を拒否する (`"Size cannot be negative"`) が、`with_size_bytes(-5)` はそのまま通る
- 消費側: macOS は `container_cfg.rs` で `size=-5` のオプション文字列が生成され、Linux は `docker_client.rs` で `"SizeBytes":-5` が生成される。どちらも検証しない
- `with_mode` も同様に負値の検証がなく、macOS では `mode={mode:o}` が負値の 2 の補数 8 進表現 (例: `-5` → `1777777777777777777773`) になり、Linux では `"Mode":-5` が生成される
- `with_size` の doc は「パース失敗時は panic する」と明記しているが、`with_size_bytes` / `with_mode` の doc には負値の制約の記述がない

## 設計方針

- `with_size_bytes` で負値が指定されたら `assert!` による負値チェックで panic にする (既存の `with_size` がパース失敗時に panic する流儀と一致。「無視」は設定意図を静かに落とすため不採用。シグネチャは testcontainers-rs 互換の `i64` のまま変えず、値検証のみ追加する)
- `with_mode` も同様に負値を panic にする (「マスク」は負値の下位ビットが意味のあるパーミッション値に化ける (例: `-1 & 0o7777 = 0o7777`) ため不採用)
- 検証が入った旨を `with_size_bytes` / `with_mode` の rustdoc に明記する

## 完了条件

- `with_size_bytes` に負値を渡すと panic になること
- `with_mode` に負値を渡すと panic になること
- 非負値 (`with_size` / `with_size_bytes` / `with_mode`) は従来どおり動作すること (既存の mounts.rs テストが従来どおり通ること。境界値 `0` が panic しないことのテストも追加する)
- 負値の panic を検証する単体テスト (`#[should_panic]` を 2 セッター分) があること
- 修正で陳腐化する `with_size_bytes` / `with_mode` の rustdoc が更新されること
- `CHANGES.md` に `[FIX]` エントリが記載されること

## 解決方法

`src/core/mounts.rs` の `Mount::with_size_bytes` / `Mount::with_mode` を修正した。

- 両セッターに `assert!(size >= 0)` / `assert!(mode >= 0)` を追加し、負値を panic で拒否するようにした (OS 側へ不正な size / mode が送信されるのを防ぐ)。シグネチャは互換の `i64` のまま
- rustdoc に負値の制約 (panic する旨・OS 側へ不正値が送られる理由) を明記した
- テスト 3 本を追加: `#[should_panic]` を `with_size_bytes` / `with_mode` の 2 本と、境界値 0 が panic しないテスト
- 既存の `with_size` (parse_size 経由) は負値を先に拒否するため、二重ガードは役割が異なる (多層防御)
- `CHANGES.md` の `## develop` に `[FIX]` エントリを追記した
