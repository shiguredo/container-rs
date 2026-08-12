# バグ: macOS の exit_code() が世代不一致で旧コンテナの exit code を返す

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-exit-code-generation-stale
- Polished: {YYYY-MM-DD}

## 目的

再 start と並行して `exit_code()` が呼ばれたときに、旧コンテナの exit code を「現在の」exit code として返す競合をなくす。

## 現状

`src/core/containers/async_container.rs` の `exit_code()` (macOS 分岐) は、停止済みかつバックグラウンド wait が未完了の場合に、5 秒タイムアウトで都度 `containerWait` を呼ぶ。

```rust
Ok(Ok(code)) => {
    let mut guard = wait_state.lock().expect(...);
    let _ = guard.store_if_current(generation, code);
    Ok(Some(code))
}
```

`store_if_current` は世代が一致しない場合に記録を棄却するが、返り値の `Ok(Some(code))` は棄却の有無に関係なく旧コンテナの exit code を返す。5 秒 wait の間に再 start (`reset_wait_state_and_respawn` の世代バンプ) が挟まると、呼び出し側は旧プロセスのコードを現在のコードとして受け取る。

- 競合経路: `start()` → `reset_wait_state_and_respawn` の世代バンプ → その間に `exit_code()` の 5 秒 wait が完了
- `exit_code()` は公開 API (`sync_container.rs` の `Container::exit_code` 経由) のためユーザー可視
- 世代バンプの仕組みは `WaitState::generation()` / `store_if_current` に既に実装済みで、記録側は正しくガードされている

## 設計方針

- 世代が不一致で記録を棄却した場合は、旧コードを返さず `Ok(None)` に倒す (または世代が一致するまで再試行する)
- `store_if_current` の返り値 (記録が採用されたか) を利用する
- バックグラウンド wait (`spawn_exit_code_waiter`) が正常なら本経路自体が走らないため、正常系への影響はない

## 完了条件

- `exit_code()` が「世代不一致で記録棄却」時に旧コンテナの exit code を返さないこと
- 既存の exit code 取得テスト (macOS) が従来どおり通ること
