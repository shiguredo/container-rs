# 機能追加: macOS で OCI maskedPaths / readonlyPaths に対応する

- Priority: Medium
- Created: 2026-07-31
- Completed: 2026-08-01
- Branch: feature/add-macos-masked-readonly-paths
- Polished: 2026-08-01

## 目的

Apple container 1.2.0 で `ContainerConfiguration` に追加された OCI `maskedPaths` / `readonlyPaths` を、本クレートの macOS (XPC) 経路から設定できるようにする。

## 現状

- Apple container 1.2.0 ([#1996](https://github.com/apple/container/pull/1996)) で `ContainerConfiguration.maskedPaths` / `readonlyPaths` (`[String]?`) が追加された。1.2.0 未満のランタイムでは未知キーが Codable で黙って無視されるため、本機能は 1.2.0 以上が前提（macOS ランタイム要件の 1.2.0 化は 0052 で対応する）
- 意味論は次のとおり
  - `nil` (キー省略): ランタイム既定セットを使う
  - `[]`: 既定を無効化する
  - 明示リスト: 既定を完全に上書きする
- 本クレートの `ContainerCfg` (`src/core/client/container_cfg.rs`) の `DisplayJson` は `stopSignal` / `creationDate` までを出力し、`maskedPaths` / `readonlyPaths` を持たない
- testcontainers-rs の `with_security_opt` の対象外（`issues/pending/0006-add-macos-security-opt-xpc.md`。seccomp / apparmor 系で XPC に設定口が無い）

## 設計方針

- shiguredo 拡張として `ImageExt` に setter を追加する (本家 testcontainers-rs に同名 API は無い)
  - 例: `with_masked_paths(self, paths: impl IntoIterator<Item = impl Into<String>>)`
  - 例: `with_readonly_paths(self, paths: impl IntoIterator<Item = impl Into<String>>)`
  - 複数回呼び出しは「上書き」とする（Apple container の意味論が「明示リストで既定を完全に上書き」のため。`with_ready_conditions` と同じ上書きパターン）
  - 空リストの指定は `with_masked_paths(std::iter::empty::<String>())` のように `Some(vec![])`（既定の無効化）として渡す。rustdoc に使用例を書く
  - 既存 API は setter と accessor が対のため、`ContainerRequest` に `masked_paths()` / `readonly_paths()` accessor も追加する
  - `with_readonly_paths` は OCI の個別パス読み取り専用化であり、`with_readonly_rootfs`（ルート FS 全体の `readOnly`）や `Mount` の `AccessMode::ReadOnly` とは別物である旨を rustdoc に明記する
- 「未指定 (`None`)」と「空リスト (`Some([])`)」を区別するため、`ContainerRequest` 側は `Option<Vec<String>>` で保持する
- `ContainerCfg` の JSON 出力は次のとおり
  - `None`: null を出力する（既存の Option フィールド (`shmSize` / `stopSignal` 等) と同じパターン。Apple 側は decodeIfPresent のためキー省略でも null でも nil になり動作は同じ）
  - `Some(list)`: `maskedPaths` / `readonlyPaths` キーを配列として出す (空配列も出す)
- Linux (Docker Engine) への配線は本 issue の範囲外。macOS のみ。ただし Linux で `with_masked_paths` / `with_readonly_paths` を使った場合は `with_ssh` と同じ扱いで start 時に明示エラーを返す（黙って無視しない方針。`linux_unsupported_request_reason` に追加する）
- 新規 API 追加のみで既存呼び出しは壊れない。公開面の拡張として扱い、CHANGES.md は `[ADD]` とする

## 完了条件

- [ ] macOS で `with_masked_paths` / `with_readonly_paths` が `containerCreate` JSON の `maskedPaths` / `readonlyPaths` に反映されること
- [ ] 未指定時は null が出力され、空リスト指定時は `[]` が出力されること（JSON 出力の検証は `container_cfg.rs` のユニットテストで行う。既存の `shm_size_is_null_by_default_in_container_cfg_json` と同じ方式）
- [ ] 統合テストが追加されていること (最低限: 空リストと明示パスの両方を渡して create が成功し、設定が観測できること)。観測はコンテナ内の挙動で行う: `readonlyPaths` は指定パスへの書き込みが失敗すること、`maskedPaths: []` は既定マスクが解除された挙動になること（既存の `alpine_with_readonly_rootfs` と同じ exec プローブ方式）。統合テストの実行には Apple container 1.2.0 以上のローカルランタイムが必要（0052 でランタイム要件を 1.2.0 に揃えるまでは、実行環境側で 1.2.0 を用意する）
- [ ] `masked_paths()` / `readonly_paths()` accessor の追加とテストが完了していること
- [ ] `docs/TESTCONTAINERS.md`（ImageExt 対応表への 2 行追加・「`with_ssh` のみ Linux で start 時明示エラー」記述の更新・accessor 一覧への 2 行追加・shiguredo 拡張の件数 (16 → 20、setter 2 + accessor 2) と API 集計の更新）と `skills/shiguredo-container/SKILL.md` の関連箇所が更新されること
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

`src/core/image/image_ext.rs` の `ImageExt` に shiguredo 拡張として `with_masked_paths` / `with_readonly_paths` (引数 `impl IntoIterator<Item = impl Into<String>>`) を追加した。複数回呼び出しは上書き (`with_ready_conditions` と同じパターン)。rustdoc には「個別パスの読み取り専用化であり、`with_readonly_rootfs` (ルート FS 全体の `readOnly`) や `Mount` の `AccessMode::ReadOnly` とは別物」を明記した。

`src/core/containers/request.rs` の `ContainerRequest` に `masked_paths` / `readonly_paths` を `Option<Vec<String>>` で追加し、`masked_paths()` / `readonly_paths()` accessor を実装した。未指定 (`None`) と空リスト (`Some(vec![])`) を区別する。

`src/core/client/container_cfg.rs` の `ContainerCfg` に両フィールドを追加し、`DisplayJson` で `maskedPaths` / `readonlyPaths` として出力する。`None` は null、`Some(list)` は配列 (空配列も出力) で、既存の `shmSize` / `stopSignal` と同じ Option パターン。

Linux では `src/runners/async_runner.rs` の `linux_unsupported_request_reason` に追加し、`with_ssh` と同じ扱いで start 時に明示エラー (`with_masked_paths() is not implemented on Linux` / `with_readonly_paths() is not implemented on Linux`) を返す。

テストは次を追加した: `container_cfg.rs` の JSON 単体テスト (既定 null・空リスト `[]`・明示配列・上書き意味論)、`src/runners/async_runner.rs` の Linux 単体テスト (空リスト含む `Some` 時に明示エラー)、`tests/container_macos.rs` の統合テスト (`readonlyPaths` で `/etc` への touch が失敗し `/tmp` への touch が成功する対比・`maskedPaths` 空リストで既定マスク解除・明示リストで既定マスクの上書き)。

ドキュメントは次を更新した: `docs/TESTCONTAINERS.md` (ImageExt 対応表・accessor 一覧・Linux fail-fast 記述・shiguredo 拡張の件数 16 → 20 と API 集計 409 → 413・フィールド一覧)、`skills/shiguredo-container/SKILL.md` (ImageExt 表・既知の制限事項・API 件数)、`README.md` の Linux 注意書き。

`CHANGES.md` に `[ADD]` エントリを追加した。
