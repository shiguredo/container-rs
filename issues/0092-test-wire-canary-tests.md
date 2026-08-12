# テスト: canary.py の doctest を CI に配線し、canary バージョン変換ロジックを検証可能にする

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/add-canary-doctest-wiring
- Polished: 2026-08-12
- Updated: 2026-08-05

## 目的

`canary.py` に組み込まれた doctest が、CI / Makefile / prek のどこからも実行されておらず、バージョン変換ロジックの回帰検出の穴になっているのを塞ぐ。

## 現状

- `canary.py` のバージョン変換 (`next_canary_version` / `update_package_section` / `split_package_section`) には doctest が組み込まれており、`python3 -m doctest canary.py` で実行できる (0078 で `test_canary.py` は廃止され、テストは doctest に集約された)
- しかし `.github/workflows/ci.yml` に Python テストの実行ステップが無く (python3 の使用はネットワーク診断のみ)、`Makefile` にターゲットが無く、`prek.toml` にも Python フックが無い
- そのため canary バージョン変換の回帰 (例: 0078 で修正した `rust-version` 巻き込み) を CI で検出できない

## 設計方針

- 0078 の方針 (ユーザー指示により単体テストファイルは作らない。テストは doctest に集約) に従い、配線のみを行う。`canary.py` のコードは変更しない
- 実行先は CI の lint ジョブに確定する (fmt / clippy と同じく軽量な静的検証の場であり、専用ジョブを立てるほどの規模ではない)。lint ジョブは ubuntu-24.04 / macos-26 の両方で実行されるが、両 OS の GitHub-hosted ランナーには python3 がプリインストールされている
- `Makefile` に `test-canary` ターゲットを追加する (既存の流儀に合わせ、`.PHONY` への追記と日本語コメント付き)
- `prek.toml` への Python フック追加は**行わない** (CI と Makefile で検証経路が確保できるため。canary.py の変更頻度は低く、コミット時のフック追加は過剰)

## 完了条件

- CI で `python3 -m doctest canary.py` が実行され、失敗すると CI が落ちること (検証: 意図的に doctest を一時的に壊して lint ジョブの失敗を確認し、確認後に元へ戻す。この検証では Slack に failure → fixed の通知が実際に飛ぶことを想定しておく)
- ローカルでも `make test-canary` で実行できること
- `CHANGES.md` の `### misc` に `[UPDATE]` エントリ (担当者行つき) が追加されていること

## 解決方法

- `.github/workflows/ci.yml` の lint ジョブに `run: python3 -m doctest canary.py` を追加する
- `Makefile` に `test-canary` ターゲット (`python3 -m doctest canary.py` を実行) を追加し、`.PHONY` にも追記する
- `CHANGES.md` の `### misc` に `[UPDATE]` エントリを追加する
