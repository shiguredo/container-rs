# バグ修正: macOS の logs() 取得失敗時ロールバックが Keep ゲート無しで remove する

- Priority: Low
- Created: 2026-07-23
- Completed: {YYYY-MM-DD}
- Model: qwen3.8-max-preview
- Branch: feature/fix-macos-log-failure-keep-gate
- Polished: 2026-07-29

## 目的

macOS の `AsyncRunner::start` で `containerLogs` 取得が失敗し、かつ `WaitFor::Log` が登録されている経路のロールバックが、`TESTCONTAINERS_COMMAND=keep` ゲート無しで `remove` を呼んでいる。他のロールバック経路（create / bootstrap / start_process / copy 失敗時）や Linux の `start_linux_log_stream` はすべて `matches!(Config.command(), Command::Remove)` でゲートしており、挙動が非対称である。この非対称を解消し、`keep` 指定時は失敗したコンテナを残して調査できるようにする。

## 現状

`src/runners/async_runner.rs` の macOS 分岐（`logs()` 呼び出し部）:

```rust
Err(e) => {
    if ready_conditions_require_log_fds(&ready_conditions) {
        if let Err(rm_err) = client.remove(&id, true).await {
            tracing::warn!("failed to remove container {id} during rollback: {rm_err}");
        }
        return Err(log_fd_required_error(&e));
    }
    tracing::warn!("failed to get log fds: {e}");
    ContainerLogSource::None
}
```

`client.remove(&id, true)` が `Config.command()` の Keep ゲート無しで呼ばれている。同ファイルの他のロールバック（create / bootstrap / start_process / copy 失敗時）は `matches!(crate::core::env::Config.command(), crate::core::env::Command::Remove) &&` でゲートしている。Linux の `start_linux_log_stream` も同様にゲートしている。

## 設計方針

当該 `remove` を他のロールバック経路と同じく `matches!(Config.command(), Command::Remove)` でゲートする。`keep` 指定時は remove せず `Err` を返す（失敗したコンテナを残す）。`keep` 指定時は watchdog も未登録のため、コンテナは確実に残留する。

## 完了条件

- [ ] macOS の logs() 取得失敗 + Log 待機使用時のロールバックが Keep ゲート付きになる
- [ ] `TESTCONTAINERS_COMMAND=keep` 指定時、当該経路でコンテナが削除されない
- [ ] `docs/TESTCONTAINERS.md` の当該経路の記述が実態に合わせて更新されること（0037 が先に実施された場合は表記統一後の行を更新する）
- [ ] `CHANGES.md` に `[FIX]` エントリが記載されること
- [ ] 既存の macOS 統合テストが pass する
- [ ] `cargo fmt --all -- --check` / `cargo clippy --all-targets --all-features -- -D warnings` / `cargo test --all-features` が pass すること

## 解決方法

`src/runners/async_runner.rs` の macOS 分岐の当該 `remove` を `matches!(Config.command(), Command::Remove)` でゲートする。必要に応じて `keep` 時の挙動を検証する統合テストを追加する。
