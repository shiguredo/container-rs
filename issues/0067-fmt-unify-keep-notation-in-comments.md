# 規約: async_runner.rs の Keep 表記と英語コメントを CODEBASE.md の `keep` 表記に統一する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-unify-keep-comments
- Polished: {YYYY-MM-DD}

## 目的

CODEBASE.md の「概念名は `` `keep` `` (小文字 + backtick) と表記する」規約に、`src/runners/async_runner.rs` のコードコメントを合わせる。あわせて AGENTS.md の「コメントは全て日本語」違反を解消する。

## 現状

- `src/runners/async_runner.rs` のコメントに大文字裸表記の「Keep ゲート付き」「Keep 指定時」「Keep ゲート付きロールバック」「Keep-gated」が 7 箇所残っている (`AsyncRunner::start` / `rollback_remove` / `run_ready_sequence` / `start_linux` 等の経路)
- 「Keep-gated」は英語コメントで、AGENTS.md の「コメントは全て日本語」に違反する
- closed 0037 でドキュメント (README / docs/TESTCONTAINERS.md / SKILL.md) は `` `keep` `` に統一済みだが、`async_runner.rs` のコードコメントは対象外だった

## 設計方針

既に統一済みの `src/core/containers/async_container.rs` / `sync_container.rs` の rustdoc と同じ表記 (`` `keep` `` 小文字 + backtick) に合わせる。Rust enum バリアント名 `Command::Keep` は Rust 命名規則どおり変更しない。

## 完了条件

- `src/runners/async_runner.rs` のコメントに大文字「Keep」表記と英語コメントが残らない

## 解決方法

- 7 箇所の「Keep ゲート」等を `` `keep` `` 表記に書き換え、「Keep-gated」を日本語で書き直す
