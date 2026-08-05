# バグ: canary.py のバージョン抽出正規表現が `rust-version` を巻き込み MSRV を黙って破壊し得る

- Created: 2026-08-04
- Completed: 2026-08-05
- Branch: feature/fix-canary-version-regex
- Polished: 2026-08-04

## 目的

canary.py のバージョン更新が、`Cargo.toml` の `[package]` セクション内のフィールド順によって `rust-version` を誤って書き換え、MSRV を黙って破壊する潜伏バグを修正する。

## 現状

- `canary.py` の `update_version` は `\[package\].*?version\s*=\s*"([\d\.\w-]+)"` (DOTALL) でバージョンを抽出する
- `[package]` セクション内で `rust-version = "1.93.0"` が `version = "..."` より**前に**並ぶと、この正規表現は `rust-version` の値を拾う (`.*?` が最初の `version\s*=` に一致するため。`rust-version` の末尾 `version` が含まれる)
- すると `next_canary_version("1.93.0")` がマイナーバンプ付き canary として成立し、置換 `package_content.replace('version = "1.93.0"', 'version = "1.94.0-canary.0"', 1)` が `rust-version = "1.93.0"` の**内部**の部分文字列に一致して、`rust-version` だけが書き換わり本物の `version` は無変更のまま成功扱いになる
- 置換後の整合性チェック (`updated_package == package_content` で raise) も部分文字列一致のため検出不能
- 現在の `Cargo.toml` は `version` が `rust-version` より先に並ぶため発症しない

## 設計方針

- 正規表現を「`[package]` セクション内の行頭の `version` キー」に固定する (例: `(?m)^[ \t]*version\s*=` を使い、`rust-version` の末尾 `version` に一致しないようにする)
- 抽出・置換は package セクション文字列 (`package_content`) に対して完結させる。これにより他セクションの行頭 `version` キーを拾わず、マッチ span の座標変換 (`content` 座標 ↔ `package_content` 座標) も不要になる (`package_start` 自体は `update_version` 側の切り出し・スプライスに残る)
- マッチの span を使って置換し、リテラル一致 (`replace`) に依存しない
- span 置換でマッチが必ず存在するようになるため、現行の整合性チェック (raise) は到達不能になる。抽出失敗時は既存の `ValueError` が代替するため、チェックは削除する

## 完了条件

- `rust-version` が `version` より前に並ぶ `Cargo.toml` に対しても、切り出した純粋関数 (package セクション更新ロジック) が正しい `version` のみを更新すること
- `rust-version` の値が変更されないこと (単体テスト)
- 正常順序 (`version` が `rust-version` より先) の `Cargo.toml` でも従来どおり更新されること (単体テスト)
- ローカルで `python3 -m unittest test_canary` が通ること (CI 配線は 0092 の範囲)

## 解決方法

- `canary.py` の `update_version` の抽出正規表現を、行頭アンカー `(?m)^[ \t]*version\s*=` に修正し、マッチ span で値だけを置換する方式に変更した (`rust-version` の末尾 `version` への誤マッチを防ぎ、リテラル一致の `replace` に依存しない)
- package セクション更新ロジックを純粋関数 `update_package_section` (入力: package セクション文字列 / 出力: 更新後セクション・現在・新バージョン) に切り出し、セクション切り出しも `split_package_section` (入力: Cargo.toml 全体 / 出力: package セクション・前方・後方) として分離した
- テストは `test_canary.py` を廃止し、各純粋関数の docstring に doctest として最小限組み込んだ (`python3 -m doctest canary.py` で実行可能。ユーザー指示により単体テストファイルは作らない方針)
- 注意: 本 issue のテストが回帰検出の実効力を持つのは 0092 (CI 配線) の完了後である。0092 は本 issue のバグを検出動機として参照しており、実装順序の依存がある
