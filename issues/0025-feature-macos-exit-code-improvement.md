# 変更: macOS で exit_code の都度 containerWait 取得を対応する

- Priority: Low
- Created: 2026-07-21
- Completed:
- Model: qwen3.8-max-preview
- Branch: feature/change-macos-exit-code-improvement
- Polished: 2026-07-29

## 目的

macOS (Apple Container) バックエンドで `ContainerAsync::exit_code` の精度を改善する。現状はバックグラウンド wait の観測済みキャッシュのみを返すため、コンテナが停止済みでもバックグラウンド wait が未完了の場合は `None` を返し得る。

## 現状

- `exit_code()` (`src/core/containers/async_container.rs`) は `WaitState` のキャッシュを参照し、未観測時は `None` を返す
- コンテナが停止済みでも、バックグラウンドの `containerWait` スレッド (`spawn_exit_code_waiter`) がまだ完了していない場合は `None` になる
- 停止済みかつ未観測の場合に都度 `containerWait` を呼ぶ経路は無い
- バックグラウンドの `spawn_exit_code_waiter` は `std::thread::spawn` を使う (`spawn_blocking` はランタイム drop 時に `LONG_TIMEOUT` (24 時間) の `containerWait` を join しようとしてハングするため、コードベースが明示的に回避している設計判断)
- `XpcClient::wait_blocking` (`src/core/client/xpc_client.rs`) は `LONG_TIMEOUT` (24 時間) 固定で `send_with_timeout` を呼ぶ。`send_with_timeout` (`src/xpc/conn.rs`) は任意の `Duration` を受け付ける

## 設計方針

- 停止済みかつ未観測の場合に、**XPC レベルの短いタイムアウト** (例: 5 秒。停止済みコンテナへの `containerWait` は通常即座に返るため十分) で `containerWait` を呼ぶ経路を追加する
- タイムアウトは XPC レベル (`send_with_timeout` に 5 秒を渡す) で適用する。tokio レベル (`tokio::time::timeout`) では `wait_blocking` 内部の XPC 呼び出しが 24 時間走り続け、ランタイム drop 時にハングする既存の問題を再導入するため使わない
- `XpcClient` (`src/core/client/xpc_client.rs`) にタイムアウト付きの wait メソッド (例: `wait_blocking_with_timeout`) を追加する。既存の `wait_blocking` は `LONG_TIMEOUT` 固定のためそのままでは使えない
- 都度取得は `spawn_blocking` で実行する。XPC レベルのタイムアウト (5 秒) により blocking タスクが短時間で完了するため、バックグラウンド常駐スレッド (24 時間) とは異なりランタイム drop 時の join 問題は限定される
- runtime client が解放済みで `containerWait` が失敗した場合は `None` を返す (エラーにしない)
- 都度取得で exit code を取得できた場合は `WaitState` に保存する (後続の `exit_code()` 呼び出しがキャッシュヒットする)
- `exit_code_hint()` (`ExitWaitStrategy` / ログ EOF 判定から呼び出し) への都度取得経路の追加は行わない (スコープ外。必要なら別途 issue を起票する)

## 完了条件

- [ ] macOS でコンテナ停止後に `exit_code()` が exit code を返すこと (バックグラウンド wait 未完了でも都度取得で対応)
- [ ] タイムアウト内に取得できなかった場合、または runtime client 解放済みの場合は `None` を返すこと (エラーにしない)
- [ ] 都度取得で取得した exit code が `WaitState` に保存され、後続の `exit_code()` 呼び出しがキャッシュヒットすること
- [ ] テストが追加されていること (XPC 呼び出しを伴うため統合テストとして追加。モック・スタブは使用しない)
- [ ] 実装後に陳腐化する `exit_code()` の rustdoc (`src/core/containers/async_container.rs`) が更新されること
- [ ] `CHANGES.md` に `[UPDATE]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
