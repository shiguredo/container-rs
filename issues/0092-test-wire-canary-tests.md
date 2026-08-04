# テスト: test_canary.py を CI に配線し、canary バージョン変換ロジックを検証可能にする

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/test-wire-canary-tests
- Polished: {YYYY-MM-DD}

## 目的

CHANGES.md で「unittest テストを追加」と宣言済みの `test_canary.py` が、CI / Makefile / prek のどこからも実行されておらず、バージョン変換ロジックの回帰検出の穴になっているのを塞ぐ。

## 現状

- `canary.py` のバージョン変換 (`next_canary_version`) と `test_canary.py` は存在する (CHANGES.md:115 に [UPDATE] として記載済み)
- しかし `.github/workflows/ci.yml` に Python テストの実行ステップが無く (python3 の使用はネットワーク診断のみ)、`Makefile` にターゲットが無く、`prek.toml` にも Python フックが無い
- そのため canary バージョン変換のバグ (例: 0078 の `rust-version` 巻き込み) を CI で検出できない

## 設計方針

- `test_canary.py` を CI の lint ジョブ (または専用ジョブ) に `python3 -m unittest test_canary` として追加する
- `Makefile` に `test-canary` ターゲットを追加する (既存の Makefile の流儀に合わせる)

## 完了条件

- CI で `test_canary.py` が実行され、失敗すると CI が落ちること
- ローカルでも `make test-canary` (または同等) で実行できること

## 解決方法

- `.github/workflows/ci.yml` の lint ジョブに `run: python3 -m unittest test_canary` を追加する
- `Makefile` に `test-canary` ターゲットを追加する
- 必要なら `prek.toml` に Python テストフックを追加する
