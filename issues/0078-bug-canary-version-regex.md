# バグ: canary.py のバージョン抽出正規表現が `rust-version` を巻き込み MSRV を黙って破壊し得る

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-canary-version-regex
- Polished: {YYYY-MM-DD}

## 目的

canary.py のバージョン更新が、`Cargo.toml` の `[package]` セクション内のフィールド順によって `rust-version` を誤って書き換え、MSRV を黙って破壊する潜伏バグを修正する。

## 現状

- `canary.py` の `update_version` は `\[package\].*?version\s*=\s*"([\d\.\w-]+)"` (DOTALL) でバージョンを抽出する
- `[package]` セクション内で `rust-version = "1.93.0"` が `version = "..."` より**前に**並ぶと、この正規表現は `rust-version` の値を拾う (`.*?` が最初の `version\s*=` に一致するため。`rust-version` の末尾 `version` が含まれる)
- すると `next_canary_version("1.93.0")` がマイナーバンプ付き canary として成立し、置換 `package_content.replace('version = "1.93.0"', 'version = "1.94.0-canary.0"', 1)` が `rust-version = "1.93.0"` の**内部**の部分文字列に一致して、`rust-version` だけが書き換わり本物の `version` は無変更のまま成功扱いになる
- 置換後の整合性チェック (`updated_package == package_content` で raise) も部分文字列一致のため検出不能
- 現在の `Cargo.toml` は `version` が先頭 (3 行目) のため発症しないが、フィールド順の並べ替えで即発症する

## 設計方針

- 正規表現を「行頭の `version` キー」に固定する (例: `^version\s*=\s*"..."` を `re.MULTILINE` で使う、または `\nversion\s*=` アンカー)
- マッチの span を使って置換し、リテラル一致 (`replace`) に依存しない

## 完了条件

- `rust-version` が `version` より前に並ぶ `Cargo.toml` に対しても、`update_version` が正しい `version` のみを更新すること
- `rust-version` の値が変更されないこと (単体テスト)

## 解決方法

- `canary.py` の `update_version` の抽出正規表現を修正し、マッチ span で置換する
- `test_canary.py` に「`rust-version` が `version` より先に並ぶ Cargo.toml フィクスチャ」のテストを追加する
- あわせて `test_canary.py` が実行されない問題 (CI / Makefile / prek に未配線) がある場合は、本 issue の検証で必要なため CI への配線を検討する (配線自体は別 issue の範囲とし、本 issue ではテストの追加のみ)
