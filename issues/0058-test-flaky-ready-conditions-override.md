# テスト: xpc_alpine_with_ready_conditions_override がフレークするのを安定化する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-flaky-ready-conditions-override-test
- Polished: 2026-08-02

## 目的

`tests/container_macos.rs` の `xpc_alpine_with_ready_conditions_override` 統合テストが、`elapsed < 10 秒` の assert 閾値超過によりフレークする。テストの安定化でローカル・CI の信頼性を上げる。

## 現状

- テストは `WaitFor::seconds(20)` が `with_ready_conditions(vec![WaitFor::seconds(1)])` で上書きされ、起動が 10 秒未満で完了することを検証する (`tests/container_macos.rs` の `xpc_alpine_with_ready_conditions_override`。`elapsed < 10 秒` を assert)
- `elapsed` は `start()` 全体を計測しており、ready 待機 (1 秒の sleep) と無関係な XPC 呼び出し群 (pull / create / bootstrap / start / logs 等) の応答遅延が混入する。`run_ready_sequence` のタイムアウト (`startup_timeout`、デフォルト 60 秒) は `block_until_ready` のみを包むため、このテストは「起動タイムアウト」ではなく assert 閾値超過で失敗する
- 2026-08-02 にローカルで 2 回、assert 閾値超過で失敗した (単体再実行は pass)。テストコードに触れていないドキュメント変更のみの作業中にも発生した
- 同日に CI の Apple container を最新化するコミット (git log `8553374`) が入っており、応答性の変化要因になりうる

## 設計方針

- 原因は、`elapsed < 10 秒` の assert が `start()` 全体の経過時間を計測しており、Apple container 実機の応答遅延 (並列実行時を含む) が閾値を超えることにある
- 安定化は、実機遅延の影響を受けない決定論的な検証方法に変更する方向で行う。前述のとおり `startup_timeout` は ready 待機のみを包むため、`with_startup_timeout` で ready 待機のタイムアウトを設定すれば、上書きが機能していれば ready 待機が 1 秒で完了して起動成功し、壊れていれば `WaitContainerError::StartupTimeout` で決定論的に失敗する。この方式なら `elapsed` の計測と `elapsed < 10 秒` の assert は不要になるため削除する。タイムアウト値は 20 秒未満かつ 1 秒の sleep に十分なマージンを持つ値 (例: 5 秒) にする
- しきい値の引き上げのみで済ませないこと。しきい値を 20 秒以上に上げると、上書きされていない場合 (20 秒待ち) でも pass してしまい検証能力を失う
- `with_wait_for(WaitFor::seconds(20))` は上書きが壊れた場合のフォールバック検出器であり、削除しないこと (削除すると ready 条件が空になり、テストが常に pass してしまう)
- テストの検証意図 (`with_ready_conditions` が `with_wait_for` を上書きすること) は変えない。`ready_conditions` のオーバーライド優先ロジックの単体テストは 0071 が対応するため、本 issue では統合テストの安定化のみ行う
- 原因がプロダクトコード側にあると判明した場合は、本 issue では対処せず bug カテゴリの別 issue として切り出す
- `tests/container_macos.rs` を変更するため、同一ファイルを変更する 0068 とマージ順に注意する

## 完了条件

- [ ] `xpc_alpine_with_ready_conditions_override` が `with_startup_timeout` を使用し、`elapsed` の計測と `elapsed < 10 秒` の assert が削除されていること
- [ ] `RUN_CONTAINER_TESTS=1 cargo test --all-features --test container_macos xpc_alpine_with_ready_conditions_override` が連続 3 回 pass すること
- [ ] 上書きが壊れた場合 (待機時間が 20 秒に戻った場合) に `WaitContainerError::StartupTimeout` でテストが失敗すること (一時的に `with_ready_conditions` を外して失敗を確認し、確認後に元へ戻す)
- [ ] `RUN_CONTAINER_TESTS=1 cargo test --all-features` が pass すること
