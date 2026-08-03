# 規約: async_runner.rs の Keep 表記を `keep` に統一し英語語句を日本語化する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-unify-keep-comments
- Polished: 2026-08-02

## 目的

CODEBASE.md の「概念名は `` `keep` `` (小文字 + backtick) と表記する」規約に、`src/runners/async_runner.rs` のコードコメントを合わせる。あわせて AGENTS.md の「コメントは全て日本語」違反 (英語語句「Keep-gated」「force 削除」) を解消する。

## 現状

`src/runners/async_runner.rs` のコメントに、`` `keep` `` 表記に反する表記が 8 箇所残っている (所属関数と逐語引用):

- `AsyncRunner::start` (macOS 分岐): 「失敗時も Keep ゲート付き明示 rm でロールバックする。」
- `AsyncRunner::start` (Linux 分岐): 「失敗時は Keep-gated 明示 rm でロールバックする (未 start)。」— 英語語句「Keep-gated」は AGENTS.md の「コメントは全て日本語」に違反
- `run_ready_sequence`: 「失敗時は Keep ゲート付きで明示 `rm` してから Err を返す。」
- `cleanup_on_ready_failure`: 「ready シーケンス失敗時の Keep ゲート付き明示 `rm`。」と「Keep 指定時は Drop のガードと同じく削除しない (失敗したコンテナを残して調査する)。」
- `rollback_remove`: 「Keep ゲート付きロールバック。Remove コマンド時のみ force 削除を試みる。」— 英語語句「force 削除」は AGENTS.md の「コメントは全て日本語」に違反
- `start_linux_log_stream`: 「起動失敗時は `log_required` なら Keep ゲート付き明示 rm でロールバックして `Err` を返し、」
- watchdog 登録箇所: 「TESTCONTAINERS_COMMAND=keep 指定時はコンテナを残す意図なので登録しない。」— 小文字だが backtick 無しで規約違反 (0037 の現状でも「小文字 backtick 無し」として認識済み)

closed 0037 でドキュメント (README / docs/TESTCONTAINERS.md / SKILL.md) は `` `keep` `` に統一済みだが、async_runner.rs のコードコメントは対象外だった。

## 設計方針

既に統一済みの `src/core/containers/async_container.rs` / `sync_container.rs` の rustdoc と同じ表記 (`` `keep` `` 小文字 + backtick) に合わせる。英語語句の日本語化の対象は日本語に置き換え可能な語句 (「Keep-gated」「force 削除」) のみとし、技術用語の英字 (`rm` / `start` / `log_required` / `Err` 等) は対象外とする。

## 完了条件

- `src/runners/async_runner.rs` のコメントで、`` `keep` `` (小文字 + backtick) 表記に反する表記 (大文字「Keep」・backtick なしの `keep`・日本語化対象の英語語句「Keep-gated」「force 削除」) が残らない
- `rg 'Keep' src/runners/async_runner.rs` でヒットするのが enum バリアント (`Command::Keep`) のみになり、`rg 'keep' src/runners/async_runner.rs` のヒットがすべて `` `keep` `` (backtick 付き) またはコード識別子 (テスト関数名等) になること
- `cargo test --all-features` が pass すること
- `CHANGES.md` の `### misc` に `[UPDATE]` エントリ (担当者行つき) が追加されている

## 解決方法

- 上記 8 箇所を `` `keep` `` 表記に書き換える:
  - 「Keep ゲート付き」→「`` `keep` `` ゲート付き」(`rollback_remove` の「Keep ゲート付きロールバック」もこのパターンで置換される)
  - 「Keep 指定時」→「`` `keep` `` 指定時」
  - 「Keep-gated 明示 rm」→「`` `keep` `` ゲート付き明示 rm」
  - 「TESTCONTAINERS_COMMAND=keep 指定時」→「`TESTCONTAINERS_COMMAND=keep` 指定時」(`async_container.rs` の rustdoc と同じく環境変数名ごと backtick で囲む)
- 「Remove コマンド」は `Command::Remove` バリアントへの参照のため置換しない。「force 削除」は「強制削除」に日本語化する
- コメント以外のコード・テスト・挙動は変更しない
- `CHANGES.md` の `### misc` に `[UPDATE]` エントリ (例: 「async_runner.rs のコメントの `` `keep` `` 表記に統一し英語語句を日本語化する」) を追加する
