# バグ: ExitWaitStrategy が exit code キャッシュのみ参照し StartupTimeout と誤診断する

- Created: 2026-08-12
- Completed: 2026-08-14
- Branch: feature/fix-exit-wait-strategy-cache-only
- Polished: 2026-08-12

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

- `expected_code` 指定時は「コンテナ停止を観測しても hint 待ちを継続」するため、バックグラウンド wait (`spawn_exit_code_waiter`) が XPC 障害等で記録に失敗した環境では、期待コードで正常終了していても exit code が永遠に立たず、外側の `startup_timeout` で `StartupTimeout` と誤診断される (Linux でも `wait_blocking` の失敗で同じ誤診断が起き得るが、直接取得の手段が無いため本 issue の対象は macOS とする)
- 一方 `ContainerAsync::exit_code()` (async_container.rs) の macOS 分岐には「停止済みかつ未観測なら 5 秒タイムアウトで都度 `containerWait` を呼ぶ」フォールバックが実装済みで、戦略はそれを使っていない (Linux 分岐にはフォールバックが無く、停止済みでも未観測なら `None` を返す)
- バックグラウンド wait が正常なら本問題は発生しない (障害経路限定)

## 設計方針

- macOS: コンテナ停止 (`state.running == false`) を観測した時点で、`container.exit_code()` の都度取得フォールバックを 1 回試す。取得できた exit code で期待コードを比較して判定し、取得できなければ `UnexpectedExitCode { actual: None }` として明示エラーにする (StartupTimeout に化けさせない)。フォールバックはループの「hint チェック → 停止観測」の後に停止観測が成立した時点で 1 回だけ呼び、取得成功時も取得失敗時もその場で判定してループから抜ける
- Linux: フォールバックを試さず現状維持 (hint 待ちループ)。Linux の `exit_code()` には都度取得が無く、停止観測直後に `UnexpectedExitCode` へ倒すとバックグラウンド wait の記録との競合で正常終了コンテナを誤ってエラーにするため
- `UnexpectedExitCode { actual: None }` は「期待コードとの不一致」ではなく「確認できない」ことを示す用途として使う (専用エラー型は追加しない。フォールバックは 1 回で打ち切るため、一時的な XPC 障害・遅延でもバックグラウンド wait の後続成功を待たずにこのエラーへ倒れる)
- 期待コードが `None` (exit code 不問) のときの挙動は現状どおり
- フォールバックの 5 秒は `startup_timeout` の残り予算を消費する。残り予算が 5 秒未満の場合は外側の timeout が先に発火して `StartupTimeout` になるが、この残存リスクは許容する
- 本 issue は 0100 の完了後を前提とする (0100 修正後は世代不一致で `Ok(None)` に倒れるため、再 start と競合した場合は `UnexpectedExitCode { actual: None }` に倒れる。安全側の挙動)。再 start と競合し得るのは、ユーザーがハンドルを取得済みの exec の `container_ready_conditions` 経路で `WaitFor::Exit` を使う場合のみ (start の `run_ready_sequence` 経路ではハンドル未取得のため競合しない)

## 解決方法

`src/core/wait/exit_strategy.rs` の `ExitWaitStrategy::wait_until_ready` を修正した。

- macOS で期待コード指定時にコンテナ停止 (`state.running == false`) を観測したら、`ContainerAsync::exit_code()` の都度取得フォールバック (5 秒・1 回) を試すようにした
- 取得できた exit code で期待コードを比較して判定 (一致 → `Ok`、不一致 → `UnexpectedExitCode { actual: Some }`)。取得できなければ `UnexpectedExitCode { actual: None }` で明示エラーに倒し、StartupTimeout への誤診断を防ぐ
- 都度 wait の間に再 start で世代が進んだ場合は `exit_code()` が `Ok(None)` に倒れるため、キャッシュの再確認は行わない (再確認すると新世代の exit code で偽成功し得る。設計方針どおり安全側の挙動)
- Linux は設計方針どおり現状維持 (hint 待ちループ)
- `UnexpectedExitCode { actual: None }` の Display を「終了コードを確認できなかった」旨の文言に分岐させ、`src/core/error.rs` の診断性を改善した
- `docs/TESTCONTAINERS.md` の `ExitWaitStrategy` の記述を更新し、macOS のフォールバック挙動を明記した
- 障害経路 (バックグラウンド wait の記録失敗) は統合テストで決定論的に再現できないため、コードレビューで担保した (既存の macOS / Linux の `WaitFor::Exit` テストは従来どおり通過)
- `CHANGES.md` の `## develop` に `[FIX]` エントリを追記した

## 完了条件

- macOS で期待 exit code 指定時にコンテナ停止を観測したら、フォールバックが取得した exit code で期待コード比較の判定が行われること (取得できなければ `UnexpectedExitCode { actual: None }` になること。ただし `startup_timeout` の残り予算が 5 秒未満の場合は外側の timeout が先に発火して `StartupTimeout` になるため、この残存リスクは設計方針どおり許容する)
- `WaitFor::Exit` の既存テスト (macOS / Linux) が従来どおり通ること
- 障害経路 (バックグラウンド wait の記録失敗) は統合テストで決定論的に再現できないため、`UnexpectedExitCode { actual: None }` への倒し込みはコードレビューで担保すること
- 修正で陳腐化する `docs/TESTCONTAINERS.md` の ExitWaitStrategy 記述が更新されること
- `CHANGES.md` に `[FIX]` エントリが記載されること
