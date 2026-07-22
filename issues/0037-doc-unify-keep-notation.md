# ドキュメントの Keep / `keep` 表記を単一方針に統一する

- Priority: Low
- Created: 2026-07-22
- Completed:
- Model: Opus 4.7
- Branch: feature/update-unify-keep-notation
- Polished: {YYYY-MM-DD}

## 目的

`TESTCONTAINERS_COMMAND=keep` に関するドキュメントの表記が、ファイル間・ファイル内で「Keep（大文字裸）」と「`keep`（バッククォート付き小文字）」で揺れている。方針を単一に決めて統一し、CODEBASE.md または AGENTS.md に短く明文化することで、以降の書き手 (人間・LLM) が迷わないようにする。

## 優先度根拠

Low。機能・削除ロジック・公開 API 挙動には影響しない。読みやすさと保守性の改善のみ。ただし、直近の 0035 で掃除契約を複数ドキュメントに分散させたことで表記揺れが目立つようになり、複数ファイルを横断して読む LLM やレビュアーが混乱しやすくなっている。方針を確定させて 1 度反映しておくと将来の diff で揺り戻しが起きない。

## 現状

- `src/core/env.rs`
  - Rust enum バリアント: `Command::Keep`（大文字始まり、Rust 命名規則どおり）
  - 環境変数値の実リテラル: 小文字 `"keep"`（`Ok("keep") => Command::Keep`）
- `README.md` L141-143
  - `` `keep` ゲート`` / `TESTCONTAINERS_COMMAND=keep` / `(keep 除く)` の小文字（後者 2 つは backtick 無し）
- `skills/shiguredo-container/SKILL.md`
  - L94: `対応 (削除。Keep ゲートあり)` — 大文字裸表記
  - L238-241, L247: `` `keep` `` の小文字 backtick が主。L240 は `(keep 除く)` の小文字 backtick 無し
- `docs/TESTCONTAINERS.md`
  - L154: `Keep 尊重` / `Keep ゲート無し` — 大文字裸表記
  - L214: `Keep ゲートは Drop のみ` / `明示 `rm` は Keep でも削除する` — 大文字裸表記
  - L783: `` `TESTCONTAINERS_COMMAND=keep` `` — 小文字 backtick
- `src/core/containers/async_container.rs` / `sync_container.rs` の rustdoc
  - `` `keep` ゲートとの非対称 `` / `` `TESTCONTAINERS_COMMAND=keep` `` — 小文字 backtick で統一済み

同一ファイル内で表記が割れている箇所（TESTCONTAINERS.md、SKILL.md）と、ファイル間で表記が割れている状態が混在している。

## 設計方針

**方針はこれだけに固定する。**

以下の 2 案のいずれかから 1 つを選択し、全ドキュメントで統一する。

- **案 A: 「概念名」と「リテラル」で書き分ける**
  - 環境変数値としてのリテラルは常に小文字 + backtick: `` `keep` `` （例: `` `TESTCONTAINERS_COMMAND=keep` ``、`` `keep` 指定時 ``）
  - 概念・機能名として文中で参照するときは大文字裸: Keep モード / Keep ゲート / Keep 尊重
  - Rust enum `Command::Keep` の PascalCase と概念名の大文字が対応するため、既存の `src/core/env.rs` との親和性が高い
- **案 B: すべて小文字 backtick に統一する**
  - `` `keep` ゲート `` / `` `keep` モード `` / `` `keep` 尊重 `` のように、概念名でも小文字 + backtick を使う
  - Rust enum との対応が消える代わりに、環境変数値との一貫性が上がる。既存の rustdoc / README / SKILL 本文の主流に揃う

**推奨は案 A**（既存 rustdoc の小文字 backtick と、既存 TESTCONTAINERS.md / SKILL の大文字裸表記の両方に居場所を与える）。ただし採用は起票者が polish で判断する。

方針決定後、以下を反映する:

1. `docs/TESTCONTAINERS.md` L154, L214, L783 と `skills/shiguredo-container/SKILL.md` L94, L238-241, L247、`README.md` L141-143 の表記を選択した方針で統一する
2. `src/core/env.rs` の Rust enum バリアント名 (`Command::Keep`) は変更しない（Rust 命名規則で決定的）
3. `CODEBASE.md`（無ければ `AGENTS.md`）に「Keep / `keep` の書き分け方針」を 2〜3 行で追記する

制約:

- コード実装 (`src/core/env.rs` の値パース、`Command::Keep` バリアント、ゲート判定ロジック) は変更しない
- `TESTCONTAINERS_COMMAND` の環境変数値の受理形式（現状小文字 `keep` のみ）は変更しない

## 完了条件

- [ ] `docs/TESTCONTAINERS.md`、`skills/shiguredo-container/SKILL.md`、`README.md` 内の Keep / `keep` 関連表記が、選択した単一方針で統一されていること
- [ ] `CODEBASE.md` または `AGENTS.md` に「Keep / `keep` の書き分け方針」を追記していること
- [ ] `src/core/env.rs` および rustdoc の実装名は変更していないこと
- [ ] `CHANGES.md` には載せないこと（`.md` 変更・rustdoc・CODEBASE / AGENTS の記述整備は changelog 規約の非対象）
- [ ] `cargo fmt --all -- --check` / `cargo clippy --all-targets --all-features -- -D warnings` / `cargo test --all-features` が pass すること

## 解決方法

方針決定後、対象ファイル群の表記を一括で置換する。差分は文書変更のみで、コード・テストには手を入れない。方針の明文化を `CODEBASE.md`（無ければ `AGENTS.md`）に追加して、以降の書き手が迷わないようにする。
