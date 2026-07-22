# 機能追加: Runtime 内でも削除完了を待てるクリーンアップ契約を提供する

- Priority: Medium
- Created: 2026-07-22
- Completed:
- Model: Cursor Grok 4.5
- Branch: feature/add-awaitable-container-cleanup
- Polished:
- Reporter: @voluntas

## 目的

`ContainerAsync` / `Container` の Drop は、tokio Runtime 内では削除を専用スレッドへ投げっぱなしにし、呼び出し復帰時点で削除完了を保証しない。Runtime 内でも削除完了を待てる Drop / `rm` 契約、または Drop 相当の同期クリーンアップ API を提供し、利用者がポーリングで補完しなくてよいようにする。

## 優先度根拠

利用者フィードバック。現状でも明示 `rm().await` で削除完了を待てる一方、Drop 任せの掃除では完了タイミングが不定で、テストや後続処理が残存コンテナと衝突し得る。ライフサイクル契約の穴であり Medium。

## 現状

`ContainerAsync` の `Drop` (`src/core/containers/async_container.rs`):

- Runtime 内 (`Handle::try_current()` が `Ok`): `std::thread::spawn` で `remove_blocking` を実行し、**join しない**（async Drop からの join は deadlock し得る、というコメントあり）
- Runtime 外: 同期的に `remove_blocking` を実行し、復帰までに削除完了を保証する
- `TESTCONTAINERS_COMMAND=keep` または既に `rm` 済み (`dropped`) のときは削除しない

明示 `rm()` は async で `client.remove` を await するため完了を待てる。一方 Drop 任せの経路では完了待ち API が無い。

Linux 統合テスト (`tests/container_linux.rs`) は、ランタイム内 Drop のあとに `wait_until_absent` で `docker inspect` をポーリングして削除完了を確認している。これは契約の穴をテスト側で補っている状態である。

`skills/shiguredo-container/SKILL.md` の掃除契約も「専用スレッド上のブロッキング API で行い、ユーザーの tokio Runtime 終了に依存しない」と述べるだけで、Runtime 内 Drop の完了待ちは約束していない。

## 設計方針

次のいずれか（または組み合わせ）で、Runtime 内でも削除完了を待てる契約を定義する。

1. **Drop 相当の明示クリーンアップ API**（推奨候補）
   - 例: async の `cleanup()` / 既存 `rm()` の利用を推奨し、ドキュメントで「完了保証が必要なら Drop ではなくこの API」と明記する
   - Drop 自体の fire-and-forget は維持し、deadlock リスクを増やさない
2. **Drop の同期化**
   - Runtime 内でも join する案。async Drop からの blocking join は deadlock し得るため、採用するなら条件と実装を厳密に設計する（安易な join は不可）
3. **完了待ちハンドル**
   - Drop が起動した削除スレッドの完了を、後から await / join できる仕組み

方針選定の制約:

- async コンテキストからの blocking join で deadlock しないこと
- `keep` ゲートと `rm` 済み (`dropped`) の既存意味を壊さないこと
- macOS / Linux 両方で同じ契約にすること

## 完了条件

- [ ] Runtime 内でも削除完了を待てる公開 API、または Drop 契約の変更が提供されていること
- [ ] ドキュメント / `skills/shiguredo-container/SKILL.md` の掃除契約が、完了保証の有無を明示していること
- [ ] Runtime 内 Drop（または代替 API）後に、削除完了を外部ポーリングなしで検証できる統合テストがあること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
