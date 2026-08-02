# テスト: ContainerRequest / ImageExt の公開アクセサに単体テストを追加する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/add-container-request-unit-tests
- Polished: {YYYY-MM-DD}

## 目的

公開 API の純ロジック (コンテナ不要で検証できるアクセサ群) に単体テストが皆無なのを解消する。

## 現状

- `src/core/containers/request.rs` の `ContainerRequest` の公開アクセサ約 30 個 (`env_vars` / `hosts` / `mounts` / `copy_to_sources` / `cmd` / `descriptor` / `ready_conditions` / `expose_ports` / `init` / `ssh` / `masked_paths` / `readonly_paths` 等) に `#[test]` が 0 本
- `src/core/image/image_ext.rs` の `ImageExt` の各 `with_*` メソッドも同様に単体テスト 0 本
- マージ順序 (`env_vars` の image とリクエストの優先順位)・`descriptor` の name/tag 合成・`ready_conditions` のオーバーライド優先などは統合テスト経由でしか検証されておらず、純ロジックの回帰を統合テスト (コンテナ実機が必要・遅い) に依存している
- 一方で `ContainerState` (`src/core/image.rs`) や `ExecCommand` (`src/core/image/exec.rs`) には単体テストが存在し、非対称

## 設計方針

`GenericImage` + `with_*` で構築した `ContainerRequest` の accessor 検証テストを単体テストとして追加する。コンテナ実機を必要としないため高速で回帰検出も早い。

## 完了条件

- 主要な公開アクセサの単体テストが追加される
- `cargo test` (コンテナなし) で実行・通過する

## 解決方法

- `src/core/containers/request.rs` に `#[cfg(test)]` モジュールを追加し、`env_vars` のマージ順序・`descriptor` の合成・`ready_conditions` のオーバーライド・`cmd` のフォールバック等を検証する
- 同様のテストを `src/core/image/image_ext.rs` に追加する
