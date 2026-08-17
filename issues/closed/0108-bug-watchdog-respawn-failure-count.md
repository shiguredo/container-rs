# バグ: watchdog の respawn 後の write 失敗が失敗回数に加算されず再試行が無制限になる

- Created: 2026-08-12
- Completed: 2026-08-14
- Branch: feature/fix-watchdog-respawn-failure-count
- Polished: 2026-08-12

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
// 引用は register の再 spawn 経路 (watchdog.rs:106-111) の断片で、
// 続く `else if SPAWN_FAILURES >= MAX_SPAWN_FAILURES { warn_exhausted_once(); }` 分岐は省略している。
```

- 失敗回数 `SPAWN_FAILURES` に加算されるのは `try_spawn_reaper` 内の spawn 失敗のみで、spawn 成功後の `write_id` 失敗 (reaper が即死) は加算されない
- spawn は成功するが即死する reaper (プロセスは起動するが読み取り前に死ぬ環境) では、`register` のたびに「spawn → write 失敗 → 次回も spawn」が繰り返され、上限 3 に到達しない
- コメントに「pipe 死亡そのものは失敗回数に加算しない」と意図は明記されているが、spawn 成功 + 即死の組み合わせで回数無制限になる点は考慮されていない
- 実害はコンテナ登録ごとに最大 2 回の spawn でホットループではないが、write 失敗ごとに登録が放棄され (guard = None)、クラッシュ時の孤立コンテナ掃除の保護が効かない (病理的環境のみ顕在化)

## 設計方針

- 再 spawn 直後の write 失敗 (register 内の 2 箇所目の write) のみを失敗回数に加算し、spawn 成功 + 即死を spawn 失敗と同列に扱う (加算時に上限到達ならその場で `warn_exhausted_once` を呼ぶ)
- 既存 reaper への write 失敗 (register 内の 1 箇所目の write。初回 spawn 直後と再 spawn 前の既存 reaper への write の両方) は加算しない (外部 kill 等の一時的な reaper 死亡で上限を消費して watchdog がプロセス生涯で無効化されないようにする。0077 で実在が認められた外部 kill シナリオの自動回復を維持する)
- 上限到達後の挙動 (以後 spawn しない) は現状どおり
- 本修正は既存実装のコメント (watchdog.rs:100) に明記された「pipe 死亡そのものは失敗回数に加算しない」設計判断を拡張する (spawn 直後の write 失敗のみ加算対象にする)

## 完了条件

- spawn 成功 + write 失敗の繰り返しが回数上限で止まること (spawn 成功 + 即死の再現はモック・スタブ禁止下で困難なため、コードレビューで担保する)
- 正常系 (reaper が生存する) の登録フローが変わらないこと
- 既存 reaper への write 失敗 (外部 kill 等) で失敗回数が加算されないこと (reaper の stdin を drop して外部 kill 相当を再現するテストで検証できる)
- 修正で陳腐化する watchdog.rs のコメント (`SPAWN_FAILURES` の doc・register 内の「pipe 死亡そのものは失敗回数に加算しない」・`EXHAUSTED_WARNED` の doc・`warn_exhausted_once` のメッセージ文言) が更新されること
- `CHANGES.md` に `[FIX]` エントリが記載されること

## 解決方法

`src/watchdog.rs` の `register` を修正した。

- 再 spawn 直後の write 失敗 (spawn 成功 + 即死) を `SPAWN_FAILURES` に加算するようにした。これにより spawn は成功するが即死する reaper で register のたびに respawn が繰り返されても、上限 `MAX_SPAWN_FAILURES` (3) で停止する
- 既存 reaper への write 失敗 (外部 kill 等の一時的な死亡) は引き続き加算しない (外部 kill 後の自動回復を維持。設計方針どおり)
- `SPAWN_FAILURES` / `EXHAUSTED_WARNED` の doc と、`register` 内のコメント・`warn_exhausted_once` のメッセージ文言を、加算対象 (spawn 失敗 + 再 spawn 直後の write 失敗) と非加算対象 (既存 reaper への write 失敗) を含む形に更新した
- テスト 2 本を追加: 既存 reaper への write 失敗で失敗回数が加算されないこと (kill → REAPER へのセットで外部 kill 相当を再現)、生存する reaper への登録が失敗回数を消費しないこと。グローバル状態 (REAPER / SPAWN_FAILURES) を触るためテスト用直列化ロック (`TEST_LOCK`) を導入し、テスト用 reaper は cat で代用して `container rm` の副作用を回避した
- spawn 成功 + 即死の加算経路はモック・スタブ禁止のため決定論的に再現できないとしてコードレビューで担保した (issue の指示どおり。テストにその旨をコメント明記)
- `CHANGES.md` の `## develop` に `[FIX]` エントリを追記した
