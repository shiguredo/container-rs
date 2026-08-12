# バグ: macOS の exit_code() が世代不一致で旧コンテナの exit code を返す

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-exit-code-generation-stale
- Polished: 2026-08-12

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
- 世代バンプの仕組みは `WaitState::bump()` / `store_if_current` に既に実装済みで、記録側は正しくガードされている
- 停止観測後に本経路を呼ぶ `WaitFor::Exit` の誤診断 issue (0102) があり、本修正の挙動 (世代不一致は `Ok(None)`) がその判定に影響する

## 設計方針

- 世代が不一致で記録を棄却した場合は、旧コードを返さず `Ok(None)` に倒す (新世代のバックグラウンド wait が後から新コードを記録するため、次の呼び出しで取得できる見込み。古い世代で現在のコードが棄却される場合も `Ok(None)` に倒れ、安全側の挙動になる)
- 世代が一致するまで再試行する案は不採用 (新世代のバックグラウンド wait が再試行と同機能を担うため、複雑さの割に得るものがない)
- `store_if_current` の返り値 (記録が採用されたか) を利用する
- 本経路はキャッシュ未記録かつ停止済みのときに走るため、バックグラウンド wait が正常でも停止直後は走り得る。ただし世代が一致する通常ケースでは従来どおり `Some(code)` を返すため、正常系の挙動は変わらない

## 完了条件

- `exit_code()` が「世代不一致で記録棄却」時に旧コンテナの exit code を返さず `Ok(None)` を返すこと (`store_if_current` の棄却契約は既存の `store_if_current_rejects_stale_generation` 単体テストで検証済み。`Ok(None)` への倒し込みはコードレビューで担保し、世代不一致の再現はタイミング依存のため統合テストは対象外)
- 既存の exit code 取得テスト (macOS / Linux) が従来どおり通ること
- 世代不一致時は `Ok(None)` を返す旨が `exit_code()` の rustdoc に追記されること
- `CHANGES.md` に `[FIX]` エントリが記載されること
