# テスト: canary.py の doctest を CI に配線し、canary バージョン変換ロジックを検証可能にする

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/test-wire-canary-tests
- Polished: {YYYY-MM-DD}
- Updated: 2026-08-05

## 目的

`canary.py` に組み込まれた doctest が、CI / Makefile / prek のどこからも実行されておらず、バージョン変換ロジックの回帰検出の穴になっているのを塞ぐ。

## 現状

- `canary.py` のバージョン変換 (`next_canary_version` / `update_package_section` / `split_package_section`) には doctest が組み込まれており、`python3 -m doctest canary.py` で実行できる (当初計画していた `test_canary.py` は 0078 で廃止され、テストは doctest に集約された)
- しかし `.github/workflows/ci.yml` に Python テストの実行ステップが無く (python3 の使用はネットワーク診断のみ)、`Makefile` にターゲットが無く、`prek.toml` にも Python フックが無い
- そのため canary バージョン変換の回帰 (例: 0078 で修正した `rust-version` 巻き込み) を CI で検出できない

## 設計方針

- `canary.py` の doctest を CI の lint ジョブ (または専用ジョブ) に `python3 -m doctest canary.py` として追加する
- `Makefile` に `test-canary` ターゲットを追加する (既存の Makefile の流儀に合わせる)

## 完了条件

- CI で `python3 -m doctest canary.py` が実行され、失敗すると CI が落ちること
- ローカルでも `make test-canary` (または同等) で実行できること

## 解決方法

- `.github/workflows/ci.yml` の lint ジョブに `run: python3 -m doctest canary.py` を追加する
- `Makefile` に `test-canary` ターゲットを追加する
- 必要なら `prek.toml` に Python テストフックを追加する
