# その他: canary.py の使い勝手とテストを改善する

- Priority: Low
- Created: 2026-07-12
- Completed: 2026-07-31
- Model: Kimi
- Branch: feature/update-canary-py
- Polished: 2026-07-29

## 目的

canary.py は canary リリース用にバージョンの bump、`cargo update`、git commit、tag、push までを一括で行うスクリプトである。コードレビューで確認した問題を修正し、バージョン変換ロジックにテストを追加する。

本 issue はカテゴリ混在 (bug / test / refactor) のため、以下の 3 issue に分割した:

- `issues/0046-bug-canary-prompt-confirm.md`: 確認プロンプトの挙動修正 + dry-run 非対話化 (bug)
- `issues/0047-test-canary-version-conversion.md`: バージョン変換ロジックの純粋関数抽出 + テスト追加 (test)
- `issues/0048-refactor-canary-comments.md`: 不正確なコメントと関数名の修正 (refactor)

## 優先度根拠

いずれも実害が出にくい問題のため Low。

## 現状

上記 3 issue に分割済み。本 issue に残存する作業はない。

## 設計方針

分割先の各 issue を参照すること。

## 完了条件

- [x] 0046 / 0047 / 0048 のすべてが closed になること

## 解決方法

分割先の 3 issue がすべて closed になったため、本 issue も closed とする。

- 0046: 確認プロンプトの挙動修正 + dry-run 非対話化（PR #8 で squash merge 済み）
- 0047: バージョン変換ロジックの純粋関数抽出 + テスト追加（PR #9 で squash merge 済み）
- 0048: 不正確なコメントと関数名の修正（PR #10 で squash merge 済み）
