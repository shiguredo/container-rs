# ドキュメント: Runtime 内 Drop は削除完了を保証しないことと明示 rm 契約を固定する

- Priority: Medium
- Created: 2026-07-22
- Completed:
- Model: Cursor Grok 4.5
- Branch: feature/update-clarify-awaitable-cleanup-contract
- Polished: 2026-07-22
- Reporter: @voluntas

## 目的

`ContainerAsync` / `Container` の掃除について、次の契約を利用者向けに固定する。

- Runtime 内の `Drop` は削除を専用スレッドへ依頼するが、呼び出し復帰時点で削除完了を保証しない（プロセスが直後に終了すると削除が途中で切れ、残コンテナになり得る）
- 削除の完了待ち、または成否の `Result` が必要なら明示 `rm()`（async は `rm().await`、sync は `rm()`）を使う

## 優先度根拠

利用者フィードバック。削除完了を待てる経路は既に `rm()` にある一方、掃除契約の文書は「専用スレッドで削除し Runtime 終了に依存しない」までで、Runtime 内 Drop の復帰時完了非保証と「完了が必要なら `rm`」が明示されていない。テストや後続処理が残存コンテナと衝突し得る。契約の穴であり Medium。

## 現状

### Drop (`src/core/containers/async_container.rs`)

- Runtime 内 (`Handle::try_current()` が `Ok`): `std::thread::spawn` で `remove_blocking` を実行し、**join しない**（async Drop からの join は deadlock し得る、というコメントあり）
- Runtime 外: 呼び出しスレッドで同期的に `remove_blocking` を実行し、**試行の終了**まで待つ。失敗は `tracing::error` のみで呼び出し側に届かない（成功は保証しない）。ソースコメントの「削除完了を保証」は契約上「試行終了」の意味であり、rustdoc では成功保証と書かないこと
- `TESTCONTAINERS_COMMAND=keep` または既に `rm` 済み (`dropped`) のときは削除しない
- **明示 `rm` は `keep` でも削除する**（`keep` ゲートは Drop のみ）
- `rm` / Drop とも常に `force=true`（running のまま削除できる前提）

### 明示 `rm`

- `ContainerAsync::rm`: async の `client.remove(..., true)` を await し、成功または 404 冪等のあと `dropped = true`
- `Container::rm` (`blocking`): `block_on` で上記を実行し、完了まで戻る。成否は `Result`
- Sync `Container` の `Drop` は `inner`（`ContainerAsync`）の `Drop` に委譲する。tokio コンテキスト内では同じ fire-and-forget
- `rm` は async `remove`、Drop は `remove_blocking`（Drop から async `remove` は呼ばれない）

### テスト

- Linux `alpine_drop_inside_runtime_removes_container`: Drop 後に `wait_until_absent`（ポーリング）で削除を確認。契約の穴をテスト側で補っている
- Linux `alpine_lifecycle_start_exec_stop_rm`: `rm().await` 後にも `wait_until_absent` している。`rm` が `Ok` を返した直後なら 1 ショット不在で足りる想定（ポーリングは防御的／ヘルパ流用）
- Linux `alpine_drop_outside_runtime_does_not_panic`: Runtime 外 Drop 後もポーリング。macOS 相当は既に 1 回 `container ls`
- macOS: Runtime 内 `ContainerAsync` Drop の削除完了テストは無い。sync Drop は no-panic のみ

### 文書の穴・誤り

- `README.md` / `skills/shiguredo-container/SKILL.md` の掃除契約: Runtime 内外を「専用スレッド」に畳んでおり不正確。完了非保証と明示 `rm` 推奨も未記載
- `docs/TESTCONTAINERS.md` 約 214 行: 「ランタイム内は async `remove`」とあるが、実装は **spawn + `remove_blocking`（非 join）**。誤記
- 本家 `AsyncDrop` は unstable。安定 Rust での Drop 内 await / 安易な join は採用しない

### 関連 issue

- `0029-bug-runtime-safety`: FD リーク・タイムアウト等。本 issue とは別
- `0032-docs-public-api`: 公開 API 全般の rustdoc。本 issue は掃除の完了保証契約と SKILL / README / TESTCONTAINERS Drop 行に限定する。一般 `missing_docs` は 0032 側

## 設計方針

**方針はこれだけに固定する。**

1. 新規公開 API（`cleanup()` 等）は追加しない。完了待ちの正は既存 `rm()`（常に `force=true`、404 は冪等成功）
2. Runtime 内 Drop の fire-and-forget（spawn + `remove_blocking`・非 join）は維持する。Drop 内 join や完了待ちハンドルは本 issue の範囲外
3. 利用者向け契約を次のとおり明記する（macOS / Linux 同一文言）。README / SKILL の既存「専用スレッド」一文は Runtime 内 / 外に分割して置換する
   - Runtime 内 Drop: 削除を専用スレッドに依頼するが、復帰時点の完了は保証しない。直後のプロセス終了で削除が途切れ得る
   - Runtime 外 Drop: 呼び出しスレッドで試行が終わるまで待つ。成功は保証しない（失敗はログのみ）。成否が必要なら明示 `rm`
   - 完了待ちまたは成否の `Result` が必要なら明示 `rm()`（`keep` でも削除する。Drop の `keep` ゲートとは非対称）
4. `docs/TESTCONTAINERS.md` の Drop 行を実装どおりに直す（両 OS とも Runtime 内は spawn + `remove_blocking`・非 join。`force` / 404 冪等も備考に含めてよい）
5. `async_container` / `sync_container` の `rm` / `Drop` に rustdoc で完了保証の差を書く。`ContainerAsync::rm` は macOS / Linux の `#[cfg]` 二重定義のため **両方** に同じ rustdoc を付ける。削除ロジック本体は変更しない

制約:

- `keep` ゲートと `dropped` の既存意味を壊さないこと
- macOS / Linux で同じ契約文言にすること

## 完了条件

- [ ] README / `skills/shiguredo-container/SKILL.md` の掃除契約が、上記設計方針 3 の Runtime 内 / 外 / 明示 `rm` / `keep` 非対称を述べていること（既存の「専用スレッド」一文化は分割・置換済みであること）
- [ ] `docs/TESTCONTAINERS.md` の Drop 行が実装どおりであること（「ランタイム内は async `remove`」誤記の解消）
- [ ] `ContainerAsync::rm` / `Drop`（macOS・Linux 両方）および sync 側の rustdoc に完了保証の差が書かれていること
- [ ] 公開 API シグネチャ・Drop 実装（spawn / join の有無）は変更していないこと
- [ ] 統合テスト: 明示 `rm` が `Ok` を返した直後に、`wait_until_absent` ループなしで削除済みを検証できること（`docker inspect` / 相当を 1 回）。既存 lifecycle からポーリングを外すことを推奨（専用テスト追加でも可）
- [ ] Runtime 内 Drop テストはポーリング維持でよい。コメントで「完了非保証のため最終確認にポーリングを使う」と明記すること（`wait_until_absent` ヘルパコメントの `spawn(remove)` も実装どおりに直す）
- [ ] Runtime 外 Drop: Linux も 1 ショット不在 assert に揃えること（macOS 既存の 1 回 `container ls` と同型）。ポーリング維持はしない
- [ ] 公開 API・削除ロジックの挙動は不変のため `CHANGES.md` に載せないこと（README / SKILL / TESTCONTAINERS の `.md` は changelog 規約どおり非対象。rustdoc・契約検証テストも機能変更ではない）
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

1. `README.md` と `skills/shiguredo-container/SKILL.md` の掃除契約節を、設計方針 3 どおりに書き換え（既存一文の分割・置換を含む）
2. `docs/TESTCONTAINERS.md` 約 214 行の Drop 備考を実装に合わせて書き換える
3. `async_container.rs` の `rm` / `Drop`（cfg 両方）と `sync_container.rs` の `rm` / `Drop` に rustdoc を足す（ロジック変更なし。Runtime 外 Drop は「試行終了／成功非保証」と書く）
4. `tests/container_linux.rs`: lifecycle（推奨）または専用テストで明示 `rm` 直後の 1 ショット不在検証にする。Drop テストと `wait_until_absent` ヘルパのコメントを契約どおりに直す。Runtime 外 Drop も 1 ショットに揃える
5. （任意）macOS にも明示 `rm` 直後の 1 ショット検証を足す
