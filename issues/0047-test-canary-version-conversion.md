# テスト: canary.py のバージョン変換ロジックにテストを追加する

- Priority: Low
- Created: 2026-07-29
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/add-canary-version-test

## 目的

canary.py のバージョン書き換えロジック (`canary.py:32-49`) にはテストが無い。`-canary.N` のインクリメントと、canary サフィックスが無い場合の次マイナーバージョン + `-canary.0` 付与の 2 経路がある。正規表現のグループ構成を誤ると壊れたバージョン文字列のまま commit / tag / push まで進むリスクがあるため、バージョン変換部を純粋関数として抽出しテストを追加する。

## 優先度根拠

Low。リリース担当者が確認プロンプトで目視する機会があるため実害は出にくい。ただし変換ロジックにテストが無いため、正規表現を誤って変更した場合に検知手段がない。

## 現状

- バージョン書き換えロジック (`canary.py:32-49`) にテストが無い: `-canary.N` のインクリメントと、canary サフィックスが無い場合の次マイナーバージョン + `-canary.0` 付与の 2 経路がある
- 変換ロジックは TOML 対象の正規表現 (`version\s*=\s*"..."`) で動いており、裸のバージョン文字列を受け取る関数への抽出には正規表現の書き直しと TOML 差し込み処理の再構成が伴う

## 設計方針

- バージョン変換部を純粋な文字列変換の関数として抽出する。抽出する関数は `def next_canary_version(version: str) -> str` (バージョン文字列を受け取り次の canary バージョンを返す) とし、ファイル I/O や `input()` を含まない純粋関数にする。既存の TOML 対象正規表現はバージョン文字列のみに作用する形に書き換え、TOML への差し戻しは `update_version` 側に残す
- テストは標準ライブラリ `unittest` で書く。AGENTS.md は Python コードに shiguredo-python スキル (pytest 必須) の参照を求めているが、canary.py は pyproject.toml も uv 環境も持たない単発のリリーススクリプトであり、pytest + uv のツールチェーンを導入する規模ではないため、意図的に逸脱する。テストファイルは `test_canary.py` をリポジトリ直下に置き、`python3 -m unittest test_canary` で実行する。CI / prek への統合は今回は行わない (手動実行のみ)
- テストケースは最低限、canary バージョンのインクリメント (`2026.1.0-canary.3` → `2026.1.0-canary.4`)、通常バージョンからの変換 (`2026.1.0` → `2026.2.0-canary.0`) を含める。バージョン形式はリポジトリの CalVer (`YYYY.M.P`) に合わせる

## 完了条件

- [ ] バージョン変換ロジックが純粋関数 `next_canary_version` として抽出され、ファイル I/O や `input()` を含まないこと
- [ ] 上記のテストケースを含む `unittest` テストが追加され、`python3 -m unittest test_canary` で pass すること
