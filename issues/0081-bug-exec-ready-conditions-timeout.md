# バグ: macOS で exec の `container_ready_conditions` にタイムアウトが無く、ログ FD が無い場合に永久待ちになり得る

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-exec-ready-conditions-timeout
- Polished: {YYYY-MM-DD}

## 目的

`ContainerAsync::exec` の `ExecCommand::container_ready_conditions` 待機が `startup_timeout` の外側で実行され、特に macOS でログ FD が取得できていない場合に永久待ちになる経路を塞ぐ。

## 現状

- `src/core/containers/async_container.rs` の `exec` は `block_until_ready(container_ready_conditions)` をタイムアウトなしで呼ぶ (本家 testcontainers-rs 0.27.3 の `RawContainer::exec` も同じ構造のため、本家互換の挙動)
- ただし macOS で `containerLogs` 失敗時 (`log_source = None`) のコンテナに対して exec の ready_conditions に `WaitFor::Log` を置くと、空リーダー + `exit_code_hint` 未観測のまま `LogWaitStrategy` のポーリングが永久に回る (start 側は `ready_conditions_require_log_fds` + `log_fd_required_error` で事前に明示エラーにしているが、exec 経路はそのガードが無い)
- rustdoc には「無期限に待ち得る」と明記されているが、テストのハング温床であり、`startup_timeout` と非対称

## 設計方針

- exec の ready_conditions 待機にもタイムアウトを適用する (`startup_timeout` と同じ値、または exec 専用の既定値)
- タイムアウト時のエラーは `WaitContainerError::StartupTimeout` 相当に揃える
- macOS で `log_source = None` かつ `WaitFor::Log` を含む場合は start 側と同じ明示エラーにする

## 完了条件

- macOS でログ FD が無いコンテナへの exec (`container_ready_conditions` に `WaitFor::Log`) が永久待ちせず、明示エラーになること (統合テスト)
- exec の ready_conditions がタイムアウトで打ち切られること (統合テスト)

## 解決方法

- `src/core/containers/async_container.rs` の `exec` で ready_conditions 待機をタイムアウト付きに変更し、macOS のログ FD 欠如時は事前エラーを追加する
- `tests/container_macos.rs` にログ FD 失敗 + exec Log 待機のテストを追加する
