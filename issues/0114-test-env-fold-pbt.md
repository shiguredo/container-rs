# テスト: env 畳み込みロジックの PBT を追加する

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/add-env-fold-pbt
- Polished: {YYYY-MM-DD}

## 目的

環境変数の BTreeMap 畳み込み (同名キーの後勝ち・キーソート・`KEY=VALUE` 整形) を PBT で検証し、固定 1 例の単体テストのみに依存するのをなくす。

## 現状

env の畳み込み処理は 4 箇所に実装されている (重複の統合は `issues/0096-refactor-merge-duplicate-implementations.md` の対象):

- `src/core/client/container_cfg.rs` の init env (macOS)
- `src/runners/async_runner.rs` の Config.Env (Linux)
- `src/core/containers/async_container.rs` の exec 内 macOS / Linux 分岐

検証はそれぞれ固定 1 例の単体テストのみ (`container_cfg.rs` / `async_runner.rs` の env 関連テスト) で、`container_linux.rs` の統合テストは「畳み込み規則そのものは単体テストが検証する」と単体テストに押し付けている。

- プロパティ (任意の (k, v) 列 → キー一意・同名キーは後勝ち・キーソート済み出力) は PBT で実現可能
- `docker_tar` の src 内 PBT 前例 (closed issue 0013 で承認済み) に倣い、`pub(crate)` の畳み込み関数も PBT 化できる
- 関連: `issues/0071-test-add-container-request-unit-tests.md` の完了条件「ラウンドトリップ性質の検証を PBT で行う別 issue が起票されていること」が本 issue の起票で満たされる

## 設計方針

- 畳み込み処理を共通関数 (0096 の統合先) として切り出し、その関数に対する PBT を `pbt/tests/` に追加する
- 0096 の統合を待たずに PBT だけ先行する場合も可 (対象関数は実装時に特定する)

## 完了条件

- 任意の (k, v) 列に対する畳み込みの property (キー一意・後勝ち・ソート) が PBT で検証されること
- `issues/0071-test-add-container-request-unit-tests.md` の完了条件が満たされること
