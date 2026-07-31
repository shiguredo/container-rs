# 機能追加: Linux で exit_code を実装する

- Priority: Medium
- Created: 2026-07-21
- Completed: 2026-07-31
- Model: qwen3.8-max-preview
- Branch: feature/add-linux-exit-code
- Polished: 2026-07-29

## 目的

Linux (Docker Engine API) バックエンドで `ContainerAsync::exit_code` を実装する。

## 優先度根拠

exit code の取得はコンテナの正常終了検証に必要であり、`ExitWaitStrategy` の前提でもある。ただし `exec` の exit code は既に取得できており、コンテナ全体の exit code は補助的な用途が多いため Medium。

## 現状

- `ContainerAsync::exit_code` の Linux 分岐は `"exit_code() is not implemented on Linux"` の明示エラー (`src/core/containers/async_container.rs:742`)
- `tests/container_linux.rs:232-234` の `unimplemented_boundaries_return_err` テストがこの Err を明示的にアサートしている
- macOS では `spawn_exit_code_waiter` (`async_container.rs:123`, `#[cfg(target_os = "macos")]` 限定) が `std::thread::spawn` で `XpcClient::wait_blocking` を呼び、`WaitState` に exit code をキャッシュしている
- `AsyncRunner::start` の Linux 分岐 (`async_runner.rs:326-333`) は空の `new_wait_state()` を渡すだけで waiter スレッドを起動しない
- `WaitState` の `generation` / `store_if_current` / `bump` / `generation()` には `#[cfg_attr(all(not(target_os = "macos"), not(test)), expect(dead_code))]` が付いており、Linux で使用すると clippy が通らない
- `reset_wait_state_and_respawn` (`async_container.rs:478`) も `#[cfg(target_os = "macos")]` 限定で、Linux の `start()` 分岐 (`async_container.rs:461-467`) には再武装の呼び出しがない
- Docker Engine API には `POST /containers/{id}/wait` がある (condition: `not-running` / `next-exit` / `removed`、レスポンス: `{"StatusCode": N}`)

## 設計方針

- macOS の `spawn_exit_code_waiter` は `#[cfg(target_os = "macos")]` 限定で `XpcClient::wait_blocking` を呼ぶため、Linux では再利用できない。Linux 用の waiter 関数を新規作成し、`DockerClient` の blocking な wait メソッドを呼ぶ
- `DockerClient` に `wait_blocking(&self, id: &str) -> Result<i64>` のような同期メソッドを追加する。`DockerClient::remove_blocking` (`docker_client.rs:195-217`) の前例に従い、`std::thread::spawn` から直接呼べる同期関数にする。`POST /containers/{id}/wait?condition=not-running` を呼び、レスポンスの `StatusCode` をパースする
- `AsyncRunner::start` の Linux 分岐で macOS と同様に `std::thread::spawn` によるバックグラウンド wait スレッドを起動する (`spawn_blocking` はランタイム drop 時にハングするため使わない。macOS と同じ設計判断)。**wait スレッドの起動は必ず `start_container` 成功後に行うこと**。`condition=not-running` は停止中コンテナに対して即座に旧 exit code を返すため、created (non-running) 状態で spawn すると旧コードが新世代として記録され、`exit_code_hint()` が running 中に `Some` を返し `LogWaitStrategy` の EOF 判定が誤動作する
- `WaitState` の世代管理は macOS と共通の仕組みを使う。`cfg_attr(dead_code)` 属性は Linux で使用するため除去する
- 再 start 時は macOS の `reset_wait_state_and_respawn` と同様に、Linux の `start()` 分岐からも wait スレッドの再武装を行う。`reset_wait_state_and_respawn` の `#[cfg(target_os = "macos")]` を外すか、Linux 用の再武装経路を追加する。**Linux では再起動が `refresh_log_streams` 内部 (`async_container.rs:579` の `start_container`) で行われるため、再武装は `refresh_log_streams(c).await?` 成功後に行うこと**。macOS のように `refresh_log_streams` より前に再武装すると、停止中のコンテナに対して waiter が旧 exit code を即取得し、新世代として記録してしまう
- waiter スレッドは `wait_blocking` のエラー (削除後の 404 等) を macOS と同様に無視する (`if let Ok(code) = ...` 相当)
- `exit_code()` は `WaitState` のキャッシュを参照し、未観測時は `None` を返す (macOS と同じ挙動)
- 本実装により Linux でも `exit_code_hint()` が `Some` を返すようになり、`log_strategy.rs:170` の EOF 判定に `exit_code_hint` 経路が追加で成立する。`log_strategy.rs:169` のコメント「macOS は exit_code_hint 経路、Linux は logs_terminated (demux 終端) 経路で EOF 判定する。」は実装後に虚偽になるため更新する
- `ExitWaitStrategy` の Linux 有効化は本 issue のスコープ外 (issue 0016 で対応)

## 完了条件

- [ ] Linux で `exit_code()` がコンテナ終了後に exit code を返すこと
- [ ] Linux で `exit_code()` がコンテナ実行中に `None` を返すこと
- [ ] 再 start 後に世代管理が正しく動作すること
- [ ] `tests/container_linux.rs` の `unimplemented_boundaries_return_err` から `exit_code` の Err 期待 (231-234 行) を削除すること (他の Err 期待は残す)
- [ ] 統合テストが追加されていること (最低限: コンテナ終了後に exit code が取得できること、再 start 直後の running 中は `exit_code()` が `None` を返すこと)
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` の `exit_code` 関連箇所が実装済みに更新されること (README.md には exit_code の言及が無いため対象外)
- [ ] 実装後に陳腐化する rustdoc / コメントが更新されること (`async_container.rs:60-62` の `wait_state` フィールド doc、`async_container.rs:77-78` の `generation` フィールド doc、`async_container.rs:714-716` の `exit_code()` rustdoc、`log_strategy.rs:169` の EOF 判定コメント)
- [ ] `CHANGES.md` に `[ADD]` エントリが記載されること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

`DockerClient` に `wait_blocking` メソッドを追加し、Linux でバックグラウンド wait スレッドによる exit code 取得を実装した。

1. `DockerClient::wait_blocking` を新規追加し、`POST /containers/{id}/wait?condition=not-running` を同期で呼び `StatusCode` をパースして返す
2. `spawn_exit_code_waiter` の Linux 版を追加し、`Arc<DockerClient>` を受け取って別スレッドで `wait_blocking` を呼ぶ。wait のエラーは macOS と同様に無視する
3. `AsyncRunner::start` の Linux 分岐で `start_container` 成功後に wait スレッドを起動する
4. `WaitState` の `cfg_attr(dead_code)` 属性を除去し、Linux で世代管理を有効化した
5. `reset_wait_state_and_respawn` の Linux 版を追加し、`refresh_log_streams` 成功後に wait スレッドを再武装する
6. `exit_code()` の Linux 分岐を macOS と同じ WaitState キャッシュ参照に更新した
7. `log_strategy.rs` の EOF 判定コメントを実態に合わせて更新した
8. 統合テスト 3 件（終了後 exit code 取得、実行中 None、再 start 後 None）を追加した
9. TESTCONTAINERS.md / SKILL.md の exit_code 関連箇所を実装済みに更新した
10. CHANGES.md に `[ADD]` エントリを追加した

変更ファイル: `src/core/client/docker_client.rs`、`src/core/containers/async_container.rs`、`src/runners/async_runner.rs`、`src/core/wait/log_strategy.rs`、`tests/container_linux.rs`、`docs/TESTCONTAINERS.md`、`skills/shiguredo-container/SKILL.md`、`CHANGES.md`
