# テスト: docker_tar の PBT が ustar の prefix 分割ブランチを生成しないのを修正する

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/add-docker-tar-pbt-prefix-split
- Polished: {YYYY-MM-DD}

## 目的

`split_ustar_path` (99 バイト超パスの prefix 分割) のロジックが PBT で一度も実行されないのを修正し、規約「PBT で実現できるものは PBT で書く」に適合させる。

## 現状

`src/core/client/docker_tar.rs` の PBT (`round_trip_single_file`) の name strategy は `[a-zA-Z0-9_.-]{1,99}` (最大 99 バイト・スラッシュなし)。

```rust
fn round_trip_single_file(
    name in "[a-zA-Z0-9_.-]{1,99}",
```

- `split_ustar_path` は name が 99 バイト超のとき初めて prefix 分割ループに入るため、PBT ではその分岐が 1 回も実行されない
- 分割ループ (name と prefix の配分を最長 name 優先で決める) は微妙なアルゴリズムで、検証は単体テスト 3 本 (`split_prefers_longest_name` / `split_directory_keeps_trailing_slash_in_name` / `builder_uses_prefix_for_long_path`) の手選び例のみ
- directory エントリ (`append_directory`) も PBT の入力に現れない
- 統合テスト (copy_to 系) も長パスの分割正しさは検証しない

## 設計方針

- PBT の name strategy を「スラッシュ・末尾スラッシュを含む 255 バイト以下の相対パス」に拡張する
- property として「分割成功時は `prefix + "/" + name` が元パスと一致する」「`name.len() <= 99`」「`prefix.len() <= 155`」を検証する (長すぎて分割不能なパスは明示エラーになる性質も PBT で検証する)
- エラー系 (分割不能パス) は単体テストとの住み分けを保つ

## 完了条件

- PBT で 99 バイト超パスの分割ブランチが実行されること
- 分割の round-trip 性質 (name / prefix の制約・再合成) が PBT で検証されること
- 全テストが通ること
