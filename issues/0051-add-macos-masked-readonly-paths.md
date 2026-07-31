# 機能追加: macOS で OCI maskedPaths / readonlyPaths を対応する

- Priority: Medium
- Created: 2026-07-31
- Completed:
- Branch: feature/add-macos-masked-readonly-paths
- Polished:

## 目的

Apple container 1.2.0 で `ContainerConfiguration` に追加された OCI `maskedPaths` / `readonlyPaths` を、本クレートの macOS (XPC) 経路から設定できるようにする。

## 現状

- Apple container 1.2.0 ([#1996](https://github.com/apple/container/pull/1996)) で `ContainerConfiguration.maskedPaths` / `readonlyPaths` (`[String]?`) が追加された
- 意味論は次のとおり
  - `nil` (キー省略): ランタイム既定セットを使う
  - `[]`: 既定を無効化する
  - 明示リスト: 既定を完全に上書きする
- 本クレートの `ContainerCfg` (`src/core/client/container_cfg.rs`) の `DisplayJson` は `stopSignal` / `creationDate` までを出力し、`maskedPaths` / `readonlyPaths` を持たない
- CLI フラグは未提供。設定口は `containerCreate` に渡す JSON のみ
- testcontainers-rs の `with_security_opt` とは別物。`issues/pending/0006-add-macos-security-opt-xpc.md` の対象外

## 設計方針

- shiguredo 拡張として `ImageExt` に setter を追加する (本家 testcontainers-rs に同名 API は無い)
  - 例: `with_masked_paths(self, paths: impl IntoIterator<Item = impl Into<String>>)`
  - 例: `with_readonly_paths(self, paths: impl IntoIterator<Item = impl Into<String>>)`
- 「未指定 (`None`)」と「空リスト (`Some([])`)」を区別するため、`ContainerRequest` 側は `Option<Vec<String>>` で保持する
- `ContainerCfg` の JSON 出力は次のとおり
  - `None`: キー自体を出さない (Apple 側の decodeIfPresent → nil → ランタイム既定)
  - `Some(list)`: `maskedPaths` / `readonlyPaths` キーを配列として出す (空配列も出す)
- Linux (Docker Engine) への配線は本 issue の範囲外。macOS のみ
- 互換破壊を許容する (新規 API 追加のみで既存呼び出しは壊れないが、公開面の拡張として扱う)

## 完了条件

- [ ] macOS で `with_masked_paths` / `with_readonly_paths` が `containerCreate` JSON の `maskedPaths` / `readonlyPaths` に反映されること
- [ ] 未指定時は JSON キーが省略され、空リスト指定時は `[]` が出力されること
- [ ] 統合テストが追加されていること (最低限: 空リストまたは明示パスを渡して create が成功し、設定が観測できること)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の関連箇所が更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
