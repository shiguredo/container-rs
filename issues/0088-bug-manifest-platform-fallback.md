# バグ: 明示 platform 指定時の manifest 選択フォールバックが Docker と異なり、要求と異なるアーキテクチャを黙って選ぶ

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-manifest-platform-fallback
- Polished: 2026-08-04

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
- フォールバックは platform 未指定時に実行ホストのアーキテクチャに応じた選択を可能にするためにある

## 設計方針

- `platform` が明示指定 (`Some`) の場合は (os, arch) 完全一致のみ許可し、一致しない場合はエラーにする。完全一致の判定は `target_os_and_architecture` による正規化後の値で行う (許可外 arch はホスト arch に正規化される。`select_manifest_digest` は private 関数で単体テストから直接呼ばれるため、`Some("linux/s390x")` の直接指定はホスト arch と一致し得るが、実経路では `normalize_platform` が許可外を `None` にするため顕在化しない)
- エラーは `ClientError::Other` で `no matching manifest for {platform}` 相当の文言を返す (メッセージに含める platform は引数で渡された文字列をそのまま使う。`ClientError::ImageNotFound` は pull 再試行の意味論 (イメージ自体の不存在) を持つため使わない)
- preferred / soft / hard フォールバックは `platform == None` の場合のみ適用する
- `None` は「未指定」または「許可外 platform の明示指定」を意味する (`normalize_platform` が許可外も `None` に正規化する)。許可外指定の `None` 正規化は pull / create が arm64 扱いで一貫するため矛盾しない (フォールバックが選ぶ arch 自体は保証されない。未指定 + stale index では preferred / soft / hard が任意の arch を選び得るのは従来どおり)。許可外の明示指定は本修正の対象外 (本修正の対象は `normalize_platform` で許可される `linux/amd64` / `linux/arm64` の明示指定)
- 注意: 明示 platform 指定でローカル index に一致が無い場合はエラーになる (index 経路の話。descriptor が manifest 直結 (single-arch) の場合は `resolve_image_config` が platform を参照せず config を読むため、この保証は index 経路のみ)。amd64 は強制 pull (`async_runner.rs`) で救済されるが、arm64 は `ImageNotFound` 時のみ pull のため、amd64 のみの stale index に対して arm64 を指定するとエラーになる (Docker はレジストリから manifest を取得して成功する)。本 issue ではこの挙動を許容する

## 完了条件

- `select_manifest_digest` が `Some("linux/amd64")` で index に amd64 が無い場合、フォールバックせず `no matching manifest` エラーを返すこと (単体テスト)
- `None` (platform 未指定) の場合は従来どおりフォールバックすること
- 既存のフォールバックテストのうち `Some` 指定でフォールバックを期待するもの (`select_manifest_digest_matches_os_on_preferred_fallback` / `select_manifest_digest_soft_first_matches_os` / `select_manifest_digest_soft_first_skips_unknown_arch` / `select_manifest_digest_hard_first_skips_attestation` / `select_manifest_digest_soft_first_keeps_missing_arch`) と、`Some` 指定で `no valid manifests` を検証している `select_manifest_digest_errors_when_all_attestation` は `None` 指定のケースに書き換えられること (`select_manifest_digest_unknown_arch_stays_on_linux` は許可外 arch の正規化検証のため `Some` のまま。`None` 書き換え後の検証はホスト arch 依存のため、CI の arm64 ホスト前提であることに注意)

## 解決方法

- `src/core/client/image_config.rs` の `select_manifest_digest` で、`platform.is_some()` のときは主経路のみで判定し、不一致なら `ClientError::Other` (`no matching manifest for {platform}` 相当の文言) を返す
- 単体テストを「明示 platform は完全一致のみ」「未指定はフォールバック」の 2 系統に整理する
