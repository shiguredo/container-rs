# ドキュメントの Keep / `keep` 表記を単一方針に統一する

- Priority: Low
- Created: 2026-07-22
- Completed: 2026-08-01
- Model: Opus 4.7
- Branch: feature/update-unify-keep-notation
- Polished: 2026-07-29

## 目的

`TESTCONTAINERS_COMMAND=keep` に関するドキュメントの表記が、ファイル間・ファイル内で「Keep（大文字裸）」と「`keep`（バッククォート付き小文字）」で揺れている。方針を単一に決めて統一し、CODEBASE.md に短く明文化することで、以降の書き手 (人間・LLM) が迷わないようにする。

## 現状

- `src/core/env.rs`
  - Rust enum バリアント: `Command::Keep`（大文字始まり、Rust 命名規則どおり）
  - 環境変数値の実リテラル: 小文字 `"keep"`（`Ok("keep") => Command::Keep`）
- `README.md` の「コンテナの掃除契約」節
  - `` `keep` ゲート`` / `` `TESTCONTAINERS_COMMAND=keep` `` は小文字 backtick 付き。`(keep 除く)` のみ小文字 backtick 無し
- `skills/shiguredo-container/SKILL.md`
  - 機能対応表の `Drop` 行: `対応 (削除。Keep ゲートあり)` — 大文字裸表記
  - 環境変数表・掃除契約の節: `` `keep` `` の小文字 backtick が主。`(keep 除く)` は小文字 backtick 無し
- `docs/TESTCONTAINERS.md`
  - `AsyncRunner::start` 行・`Drop` impl 行: `Keep 尊重` / `Keep ゲート無し` / `Keep ゲートは Drop のみ` / `明示 rm は Keep でも削除する` — 大文字裸表記
  - watchdog の仕組み節: `` `TESTCONTAINERS_COMMAND=keep` `` — 小文字 backtick
- `src/core/containers/async_container.rs` / `sync_container.rs` の rustdoc
  - `` `keep` ゲートとの非対称 `` / `` `TESTCONTAINERS_COMMAND=keep` `` — 小文字 backtick で統一済み
- `src/runners/async_runner.rs` のコードコメント
  - `// TESTCONTAINERS_COMMAND=keep 指定時はコンテナを残す意図なので登録しない。` — 小文字 backtick 無し

同一ファイル内で表記が割れている箇所（TESTCONTAINERS.md、SKILL.md）と、ファイル間で表記が割れている状態が混在している。

## 設計方針

**方針は案 B に固定する。**

- **案 B: すべて小文字 backtick に統一する**
  - `` `keep` ゲート `` / `` `keep` モード `` / `` `keep` 尊重 `` のように、概念名でも小文字 + backtick を使う
  - Rust enum との対応が消える代わりに、環境変数値との一貫性が上がる。既存の rustdoc / README / SKILL 本文の主流に揃う
  - rustdoc が既に小文字 backtick で統一済みであり、完了条件で rustdoc を凍結する方針と整合する

方針決定後、以下を反映する:

1. `docs/TESTCONTAINERS.md`、`skills/shiguredo-container/SKILL.md`、`README.md` 内の Keep / `keep` 関連表記をすべて小文字 backtick に統一する（対象ファイル内の Keep / `keep` 出現箇所すべて。大文字裸の `Keep` を `` `keep` `` に、backtick 無しの `keep` を `` `keep` `` に置換する）
2. `src/core/env.rs` の Rust enum バリアント名 (`Command::Keep`) は変更しない（Rust 命名規則で決定的）
3. `CODEBASE.md` に「Keep / `keep` の書き分け方針」を 2〜3 行で追記する（CODEBASE.md のコミットは shiguredo-git の特別ルールに従い、develop 直接コミット・メッセージ「整備」・他変更を混ぜない）

制約:

- コード実装 (`src/core/env.rs` の値パース、`Command::Keep` バリアント、ゲート判定ロジック) は変更しない
- コードコメント (`src/runners/async_runner.rs` 等) はドキュメント表記統一の対象外とする
- `TESTCONTAINERS_COMMAND` の環境変数値の受理形式（現状小文字 `keep` のみ）は変更しない
- rustdoc (`async_container.rs` / `sync_container.rs`) は既に統一済みのため変更しない
- 0038（macOS logs() 失敗時ロールバックの Keep ゲート欠如修正）が TESTCONTAINERS.md の同一行を変更する可能性がある。実施順序に注意

## 完了条件

- [ ] `docs/TESTCONTAINERS.md`、`skills/shiguredo-container/SKILL.md`、`README.md` 内の Keep / `keep` 関連表記が、すべて小文字 backtick に統一されていること
- [ ] `CODEBASE.md` に「Keep / `keep` の書き分け方針」を追記していること
- [ ] `src/core/env.rs` および rustdoc の実装名は変更していないこと
- [ ] `CHANGES.md` には載せないこと（`.md` 変更・rustdoc・CODEBASE / AGENTS の記述整備は changelog 規約の非対象）
- [ ] `cargo fmt --all -- --check` / `cargo clippy --all-targets --all-features -- -D warnings` / `cargo test --all-features` が pass すること

## 解決方法

方針決定後、対象ファイル群の表記を一括で置換する。差分は文書変更のみで、コード・テストには手を入れない。方針の明文化を `CODEBASE.md` に追加して、以降の書き手が迷わないようにする。
