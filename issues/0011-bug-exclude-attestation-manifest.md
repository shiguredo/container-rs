# バグ修正: manifest 選択フォールバックで attestation manifest を除外する

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: Qwen Code
- Branch: feature/fix-exclude-attestation-manifest
- Polished:

## 目的

OCI image index からの manifest 選択ロジック (`src/core/client/image_config.rs` の `select_manifest_digest`) において、フォールバック経路が attestation manifest (platform.architecture が `"unknown"`) を選び得る問題を修正する。

## 優先度根拠

フォールバック経路限定の内部挙動であり、通常のイメージ (arm64 / amd64 の manifest を含む index) では主経路または preferred フォールバックで正しい manifest が選ばれる。attestation manifest が index の先頭にあり、かつ主経路・preferred フォールバックの両方が不一致になる特殊な構成でのみ発生する理論上のリスクであるため Low。

## 現状

`select_manifest_digest` には 4 段階の選定経路がある:

1. **主経路**: os と architecture の両方が一致する manifest を選ぶ
2. **preferred フォールバック**: 同じ os で `"arm64"` / `"amd64"` の順に選ぶ
3. **soft 先頭**: os 一致のみで選び、architecture は問わない
4. **hard 先頭**: `manifests.first()` を無条件に返す

主経路と preferred フォールバックは architecture の完全一致で選ぶため、`"unknown"` は自然にスキップされる。しかし:

- **soft 先頭**: `platform: { os: "linux", architecture: "unknown" }` の attestation manifest が `os == "linux"` でマッチし得る
- **hard 先頭**: `manifests.first()` が attestation manifest を無条件に選び得る

## 設計方針

`soft 先頭` と `hard 先頭` のフォールバック経路で、architecture が `"unknown"` の manifest を候補から除外する。

- 除外した結果候補がゼロになった場合はエラー (`ClientError::Json("index has no valid manifests")` 等) を返す
- attestation manifest の判定は `architecture == "unknown"` とする
- 主経路・preferred フォールバックは変更不要 (architecture 完全一致で `"unknown"` は自然にスキップされる)

## 完了条件

- [ ] `soft 先頭` / `hard 先頭` のフォールバック経路で architecture が `"unknown"` の manifest が選択されないこと
- [ ] 単体テストが追加されていること (最低限: attestation manifest が先頭にある index で正常な manifest が選ばれること、全 manifest が attestation の場合にエラーになること、soft 先頭経路で attestation manifest がスキップされること)
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
