# テスト: xpc_alpine_with_ready_conditions_override がフレークするのを安定化する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-flaky-ready-conditions-override-test
- Polished: {YYYY-MM-DD}

## 目的

`tests/container_macos.rs` の `xpc_alpine_with_ready_conditions_override` 統合テストが、Apple container (XPC) の応答遅延で起動タイムアウトによりフレークする。テストの安定化でローカル・CI の信頼性を上げる。

## 現状

- テストは `WaitFor::seconds(20)` が `with_ready_conditions(vec![WaitFor::seconds(1)])` で上書きされ、起動が 10 秒未満で完了することを検証する (`tests/container_macos.rs` の `xpc_alpine_with_ready_conditions_override`。`elapsed < 10 秒` を assert)
- 2026-08-02 にローカルで 2 回、起動タイムアウトで失敗した (テストスイート全体が 95.65 秒要し、単体再実行は 2.26 秒で pass)。`start()` が Apple container 実機の応答遅延で 10 秒を超えると失敗する。失敗は本変更と無関係のフレークであり、ドキュメント変更のみの作業中にも発生した

## 設計方針

- フレークの原因 (タイムアウト値・検証方法・並列実行時の Apple container の応答性) を特定して安定化する
- テストの検証意図 (`with_ready_conditions` が `with_wait_for` を上書きすること) は変えない

## 完了条件

- [ ] `xpc_alpine_with_ready_conditions_override` が連続実行 (複数回) で安定して pass すること
- [ ] テストの検証意図 (`with_ready_conditions` による上書き) が維持されていること
- [ ] `cargo test --all-features` が pass すること
