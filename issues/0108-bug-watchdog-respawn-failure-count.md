# バグ: watchdog の respawn 後の write 失敗が失敗回数に加算されず再試行が無制限になる

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-watchdog-respawn-failure-count
- Polished: {YYYY-MM-DD}

## 目的

reaper プロセスの再生成 (`respawn`) が「spawn は成功するが即死する」状況で、失敗回数の上限に達せず再試行が永久に繰り返されるのを防ぐ。

## 現状

`src/watchdog.rs` の `register` は、reaper が死亡していた場合に再生成する。

```rust
*guard = try_spawn_reaper();
if let Some(w) = guard.as_mut() {
    if !write_id(w, id) {
        tracing::warn!(...);
        *guard = None;
    }
}
```

- 失敗回数 `SPAWN_FAILURES` に加算されるのは `try_spawn_reaper` 内の spawn 失敗のみで、spawn 成功後の `write_id` 失敗 (reaper が即死) は加算されない
- spawn は成功するが即死する reaper (例: `/bin/sh` が異常) の環境では、`register` のたびに「spawn → write 失敗 → 次回も spawn」が繰り返され、上限 3 に到達しない
- コメントに「pipe 死亡そのものは失敗回数に加算しない」と意図は明記されているが、spawn 成功 + 即死の組み合わせで回数無制限になる点は考慮されていない
- 実害はコンテナ登録ごとに最大 2 回の spawn で、ホットループではない (病理的環境のみ顕在化)

## 設計方針

- write 失敗も失敗回数として数えるか、spawn 直後に write が失敗した場合は spawn 失敗と同列に扱う
- 上限到達後の挙動 (以後 spawn しない) は現状どおり

## 完了条件

- spawn 成功 + write 失敗の繰り返しが回数上限で止まること
- 正常系 (reaper が生存する) の登録フローが変わらないこと
