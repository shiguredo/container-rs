# バグ: Mount::with_size_bytes / with_mode が負値・不正値を検証なしで素通しする

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-with-size-bytes-negative
- Polished: {YYYY-MM-DD}

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
- `with_mode` も同様に負値・不正値の検証がなく、XPC 側で意味不明な値が渡り得る
- `with_size` の doc は「パース失敗時は panic する」と明記しているが、`with_size_bytes` の doc には制約の記述がない

## 設計方針

- `with_size_bytes` で負値が指定されたら panic (または無視) にする。公開 API のシグネチャは testcontainers-rs 互換の `i64` のまま変えず、値検証のみ追加する
- `with_mode` も同様に負値の扱いを決める (mode は下位ビットのみ意味を持つため、負値は panic かマスク)
- 検証が入った旨を doc に明記する

## 完了条件

- `with_size_bytes` に負値を渡すと早期にエラー (panic または拒否) になること
- 通常値 (`with_size` / `with_size_bytes` の正値) は従来どおり動作すること
- 負値の拒否を検証するテストがあること
