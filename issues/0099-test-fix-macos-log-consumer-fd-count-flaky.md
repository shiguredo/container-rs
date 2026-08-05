# バグ: macOS の LogConsumer FD 解放検証テストが並列実行で失敗する

- Created: 2026-08-05
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-log-consumer-fd-count-flaky
- Polished: {YYYY-MM-DD}

## 目的

CI (test-apple-container) の cargo test 並列実行時に `xpc_alpine_log_consumer_stops_after_natural_exit` が失敗するのを止める。

## 現状

- `tests/container_macos.rs` の `test_container_xpc::xpc_alpine_log_consumer_stops_after_natural_exit` は、プロセス全体の FD 数 (`open_fd_count`) を基準に「配信タスクの dup FD が解放された」ことを検証する
- 検証は「基準値から 2 以上減る」ことだが、cargo test は同一バイナリ内のテストを並列実行するため、他テスト (XPC 接続・exec・copy 等) が FD を握っていると減らずにタイムアウトする
- 実際の CI 失敗実績:
  - 2026-08-04: `after_start=53, now=57` (4 増)
  - 2026-08-05: `after_start=53, now=56` (3 増)
- テスト単独実行では失敗しない (コメントにも単独実行を想定した記述がある)

## 設計方針

- 検証対象は「コンテナ自然終了後に LogConsumer 配信タスクが停止し dup FD が解放されること」であり、FD 数の減少はその観測手段
- プロセス全体の FD 数を並列実行テストで検証するのは本質的に不安定であるため、テストを単独バイナリに分離して他テストの FD 干渉を排除する (`tests/container_sync_drop_macos.rs` の「最終 drop 検証専用バイナリ」と同様のパターン)
- あるいは、FD 数全体ではなくタスク固有の FD のみを追跡する方法 (検証対象の絞り込み) に変更する

## 完了条件

- develop の CI (test-apple-container) が安定して通過する (2 回以上連続で成功する)

## 解決方法

- `xpc_alpine_log_consumer_stops_after_natural_exit` を専用テストバイナリに分離する
- 分離後も FD 解放検証の意味 (タスク break で 2 減) が保たれること
