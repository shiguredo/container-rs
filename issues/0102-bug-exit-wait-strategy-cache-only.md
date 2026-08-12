# バグ: ExitWaitStrategy が exit code キャッシュのみ参照し StartupTimeout と誤診断する

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-exit-wait-strategy-cache-only
- Polished: {YYYY-MM-DD}

## 目的

`WaitFor::Exit` で期待 exit code を指定したとき、コンテナが期待コードで正常終了していても、バックグラウンド wait の記録が無いだけで `StartupTimeout` と誤診断される経路をなくす。

## 現状

`src/core/wait/exit_strategy.rs` の `wait_until_ready` は exit code の判定を `container.exit_code_hint()` (キャッシュのみ) で行う。

```rust
if let Some(actual) = container.exit_code_hint() {
    if let Some(expected_code) = self.expected_code && actual != expected_code {
        return Err(...UnexpectedExitCode...);
    }
    return Ok(());
}
...
if !state.running && self.expected_code.is_none() {
    return Ok(());
}
```

- `expected_code` 指定時は「コンテナ停止を観測しても hint 待ちを継続」するため、バックグラウンド wait (`spawn_exit_code_waiter`) が XPC 障害等で記録に失敗した環境では、期待コードで正常終了していても exit code が永遠に立たず、外側の `startup_timeout` で `StartupTimeout` と誤診断される
- 一方 `ContainerAsync::exit_code()` (async_container.rs) には「停止済みかつ未観測なら 5 秒タイムアウトで都度 `containerWait` を呼ぶ」フォールバックが実装済みで、戦略はそれを使っていない
- バックグラウンド wait が正常なら本問題は発生しない (障害経路限定) が、コードベース内に既存の取得手段があるのに使われていない非対称が実在する

## 設計方針

- コンテナ停止 (`state.running == false`) を観測した時点で、`container.exit_code()` の都度取得フォールバックを 1 回試す
- 取得できなければ `UnexpectedExitCode { actual: None }` として明示エラーにする (StartupTimeout に化けさせない)
- 期待コードが `None` (exit code 不問) のときの挙動は現状どおり

## 完了条件

- 期待 exit code 指定時にコンテナ停止を観測したら、`StartupTimeout` ではなく明示エラー (または正しい exit code 判定) が返ること
- `WaitFor::Exit` の既存テスト (macOS / Linux) が従来どおり通ること
