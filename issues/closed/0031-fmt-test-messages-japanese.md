# 規約: テストの英語ログメッセージ日本語化と issue 番号言及の削除

- Priority: Medium
- Created: 2026-07-22
- Completed: 2026-08-01
- Model: Claude Sonnet 4
- Branch: feature/fix-test-messages-japanese
- Polished: 2026-07-29

## 目的

AGENTS.md の「テストのログメッセージは全て日本語にすること」と shiguredo-issues の「issue 番号をソースコードに持ち込まないこと」の規約違反を修正する。

## 現状

### 英語ログメッセージ (`tests/container_macos.rs`)

- `skip_unless_rosetta()` 関数の `eprintln!` が英語
- 英語の assert メッセージが約 30 件（`"container should be running"`, `"exit code should be 42"` 等）
- スキップメッセージのプレフィックスが `"SKIP:"` と `"スキップ:"` で混在（`"SKIP:"` は `skip_unless_rosetta()` 内ともう 1 箇所。後者はプレフィックスのみ英語で本文は日本語）
- 他のテストファイル（`container_linux.rs`、`nginx_http11.rs`、`helpers/mod.rs` 等）は既に日本語化済み

### issue 番号言及 (`src/core/containers/sync_container.rs`)

`block_on_runtime_inside_current_thread_runtime_runs_on_another_thread` テストの doc コメントに `(0019 の回帰テスト)` という issue 番号言及がある（1 箇所のみ）

## 設計方針

- `tests/container_macos.rs` の全 assert メッセージ・eprintln を日本語に統一する
- スキップメッセージのプレフィックスを `"スキップ:"` に統一する
- issue 番号言及を理由そのものに置き換える: `(0019 の回帰テスト)` → 「(既存 tokio ランタイム内で `block_on_runtime` を呼んだとき別スレッドで実行されることの回帰テスト)」等

## 完了条件

- [ ] tests/ 配下の全ログメッセージ・assert メッセージが日本語になっていること
- [ ] スキップメッセージのプレフィックスが `"スキップ:"` に統一されていること
- [ ] ソースコード内に issue 番号への言及がなくなること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

- tests/container_macos.rs の英語 assert メッセージ 35 箇所を日本語化した
- SKIP: プレフィックスを「スキップ:」に統一した
- sync_container.rs の issue 番号言及 (0019) を理由そのものに置き換えた
