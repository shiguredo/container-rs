# バグ: Linux with_copy_to のターゲットパスが中間 .. を許容する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-linux-copy-to-path-validation
- Polished: {YYYY-MM-DD}

## 目的

`with_copy_to` (Linux 分岐) のターゲットパス検証が中間 `..` を通すため、意図した「明示エラー」ではなくデーモン由来の cryptic なエラーになる問題を解消する。

## 現状

- `src/runners/async_runner.rs` の `copy_to_sources_linux` のパス検証は、絶対パス・末尾スラッシュ・`file_name()` が `None` になるケース (末尾が `..` のパス) のみを検査する
- `/tmp/../etc/passwd` のような**中間** `..` は検証を通り、`make_path_relative` で `tmp/../etc/passwd` となり、`append_ancestor_directories` が `tmp/` や `tmp/../` のディレクトリエントリを tar に載せる
- 最終的に Docker daemon のパス保護に依存し、失敗時は daemon のエラーがそのまま返る (検証の意図である「ライブラリ側で明確なエラー」を満たしていない)

## 設計方針

パスをコンポーネント単位で検査し、`..` (ParentDir) を含むターゲットパスを拒否する。`PathNameError` で明示エラーを返す。

## 完了条件

- 中間 `..` を含むターゲットパス (`/tmp/../etc/passwd` 等) が `copy_to` の検証で拒否される
- 通常パスは従来どおり動作する

## 解決方法

- `copy_to_sources_linux` の検証で `std::path::Component::ParentDir` を含むパスを拒否する
- 境界のテスト (中間 `..`・末尾 `..`・通常パス) を追加する
