# バグ: 明示 platform 指定時の manifest 選択フォールバックが Docker と異なり、要求と異なるアーキテクチャを黙って選ぶ

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-manifest-platform-fallback
- Polished: {YYYY-MM-DD}

## 目的

macOS のイメージ config 解決 (`resolve_image_config`) で、ユーザーが明示的に platform を指定した場合に、フォールバックが要求と異なるアーキテクチャの manifest を選んでしまう挙動を修正する。

## 現状

- `src/core/client/image_config.rs` の `select_manifest_digest` は、主経路 (os + arch 完全一致) の次に以下のフォールバックを持つ:
  1. preferred: 同じ os で arm64 → amd64 の順に選択
  2. soft: os 一致の先頭 (architecture 欠落も可)
  3. hard: os 不問の先頭 (attestation 除外)
- このフォールバックは `platform` 引数が `None` (未指定) の場合も `Some` (明示指定) の場合も**区別なく適用される**
- ユーザーが `with_platform("linux/amd64")` を明示しても、index に amd64 が無いと arm64 (preferred)、さらには s390x 等 (soft) まで選び得る。単体テスト `select_manifest_digest_soft_first_matches_os` は「linux/arm64 要求で linux-s390x を選ぶ」ことを正として固定している
- Docker Engine は明示 platform でマッチ無しなら `no matching manifest` エラーを返す (フォールバックしない)。マッチしないアーキテクチャの manifest から CMD/ENTRYPOINT を読むと、pull / create の platform と食い違い、起動失敗や誤設定になる
- フォールバック自体は platform 未指定時の救済として意図されたもの (doc コメントに明記)

## 設計方針

- `platform` が明示指定 (`Some`) の場合は (os, arch) 完全一致のみ許可し、一致しない場合はエラーにする
- preferred / soft / hard フォールバックは `platform == None` の場合のみ適用する
- `resolve_image_config` の呼び出し側 (`src/runners/async_runner.rs` の start) は正規化後の `resolve_platform` を渡しており、`None` は「未指定」を意味するため、区別は呼び出し側の変更なしで可能

## 完了条件

- `select_manifest_digest` が `Some("linux/amd64")` で index に amd64 が無い場合、フォールバックせずエラーを返すこと (単体テスト)
- `None` (platform 未指定) の場合は従来どおりフォールバックすること
- 既存のフォールバックテストは `None` 指定のケースに書き換えられること

## 解決方法

- `src/core/client/image_config.rs` の `select_manifest_digest` で、`platform.is_some()` のときは主経路のみで判定し、不一致なら `ClientError::Other` (または `ImageNotFound` 相当) を返す
- 単体テストを「明示 platform は完全一致のみ」「未指定はフォールバック」の 2 系統に整理する
