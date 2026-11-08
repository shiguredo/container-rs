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

- [CHANGE] 最小 Rust バージョン (MSRV) を 1.95 に引き上げる
  - @voluntas
- [ADD] macOS で `with_kernel` によりコンテナ単位でカスタムカーネルを指定できるようにする
  - @voluntas
- [ADD] `with_network_alias` / `with_network_aliases` でコンテナのネットワークエイリアスを指定できるようにする
  - @voluntas

### misc

- [UPDATE] モジュール定義を `mod.rs` から `<module>.rs` 形式に統一する
  - @voluntas

## 2026.1.1

**リリース日**: 2026-10-07

- [FIX] macOS でイメージ config の `Entrypoint` が `[""]` のとき executable が空文字になりコンテナが起動できない問題と、`with_entrypoint("")` がイメージ entrypoint にフォールバックする問題を修正する (単一要素の空文字列を entrypoint クリアとして扱い、ユーザー cmd、無ければイメージ cmd の先頭を executable にする)
  - @voluntas

### misc

- [UPDATE] PBT を proptest から noprop に切り替える
  - @voluntas
- [UPDATE] ツールチェーンを `rust-toolchain.toml` で MSRV の 1.93 に固定する
  - `Cargo.toml` の `rust-version` を `1.93` 表記に統一する
  - CI で `rustup show` により MSRV のツールチェーンを導入する
  - @voluntas

## 2026.1.0

**リリース日**: 2026-08-18

**公開**
