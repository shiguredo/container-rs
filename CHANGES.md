# 変更履歴

- UPDATE
  - 後方互換がある変更
- ADD
  - 後方互換がある追加
- CHANGE
  - 後方互換のない変更
- FIX
  - バグ修正

## develop

- [FIX] macOS でイメージ config の `Entrypoint` が `[""]` のとき executable が空文字になりコンテナが起動できない問題と、`with_entrypoint("")` がイメージ entrypoint にフォールバックする問題を修正する (単一要素の空文字列を entrypoint クリアとして扱い、ユーザー cmd、無ければイメージ cmd の先頭を executable にする)
  - @voluntas

### misc
- [UPDATE] PBT を proptest から noprop に切り替える
  - @voluntas

## 2026.1.0

**リリース日**: 2026-08-18

**公開**
