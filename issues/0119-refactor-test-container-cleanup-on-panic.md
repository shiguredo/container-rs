# リファクタリング: テスト失敗 (panic) 時にコンテナの後始末を保証する仕組みを導入する

- Created: 2026-08-18
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-test-container-cleanup-on-panic
- Polished: {YYYY-MM-DD}

## 目的

統合テストの失敗 (expect の panic) 時に明示的な後始末 (`rm`) が実行されないため、コンテナの削除が `ContainerAsync` の Drop の best-effort 削除に依存し、削除に失敗した場合にコンテナが残り続ける。孤立コンテナの蓄積を防ぐ仕組みをテスト基盤に導入する。

## 現状

- tests/ 配下の統合テストは「コンテナ生成 → 検証 → `stop_with_timeout(Some(0))` → `rm()`」のパターンで、各ステップの expect による panic 時には明示 `rm` が実行されない
- `ContainerAsync` の Drop はコンテナ削除を試行するが、失敗は `tracing::error` にのみ記録され、呼び出し側には届かない (Runtime 内 Drop は `DROP_REMOVE_TIMEOUT` (5 秒) の best-effort、Runtime 外 Drop は成功非保証)
- `TESTCONTAINERS_COMMAND=keep` 設定時は Drop は削除しない
- watchdog (macOS) はテストプロセスの終了 (reaper の pipe EOF) 時にのみ発動する。panic によるテスト失敗ではテストプロセスが継続するため発動しない

## 設計方針

- tests/helpers/ に、スコープ脱出 (正常終了・panic の両方) 時にコンテナの明示削除を試みるテストヘルパー (RAII ガード) を導入する
- 既存テストの書き換えは対象を絞って段階的に行う (例: 新規テストと更新対象のテストから適用し、スイート全体の書き換えは別途判断)
- 通常の後始末 (`stop_with_timeout` + `rm`) の挙動は変えない
- ヘルパー自体の検証は実コンテナを使った統合テストで行う (モックやスタブは使わない)

## 完了条件

- スコープ脱出時にコンテナの明示削除を試みるテストヘルパーが tests/helpers/ に追加されている
- このヘルパーが統合テスト (tests/nginx_http11.rs 等) に適用されている
- ヘルパー適用後も `cargo test --all-features` が通ること
- `TESTCONTAINERS_COMMAND=keep` の扱い (明示 `rm` は `keep` でも削除する既存契約) が維持されていること
