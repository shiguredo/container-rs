# 仕様変更: Apple container 1.2.0 のコンテナ ID 制約に合わせて検証する

- Priority: Medium
- Created: 2026-07-31
- Completed:
- Branch: feature/change-macos-container-id-validation
- Polished:

## 目的

Apple container 1.2.0 で XPC リクエストに対するコンテナ ID 検証が強化された。本クレート側でも同じ規則で fail-fast し、不正な `with_container_name` が create 直前で落ちるのを防ぐ。あわせて macOS ランタイム要件を 1.2.0 以上に揃える。

互換破壊を許容する。

## 現状

- Apple container 1.2.0 ([#1956](https://github.com/apple/container/pull/1956)) で `ContainersHarness` が create / delete / logs 等の ID に対し `ManagedContainer.nameValid` を enforce する
- `nameValid` の規則
  - 長さが 63 以下
  - 正規表現 `^[a-zA-Z0-9][a-zA-Z0-9_.-]+$` (先頭は英数字、以降は英数字 / `_` / `.` / `-`、実質 2 文字以上)
- 本クレートの ID 決定 (`src/runners/async_runner.rs`)
  - `with_container_name` があればそれを ID にする
  - 無ければ `c-{unique_suffix()}` (`src/core/util.rs` の `unique_suffix`)
- `with_container_name` (`src/core/image/image_ext.rs`) は文字列をそのまま保存するだけで検証しない
- watchdog の `is_valid_container_id` (`src/watchdog.rs`) は空でない・`[A-Za-z0-9._-]` のみを見る。長さ上限も「2 文字以上」も無い
- 自動生成 ID (`c-...`) は通常 63 文字以内に収まるが、利用者が長い名前を渡すと 1.2.0 の XPC で create が失敗する
- README の要件表は `container` のバージョン下限を書いていない
- 1.2.0 の TCP/UDP port forward バッファ修正 ([#2027](https://github.com/apple/container/pull/2027)) はランタイム側のみで、本クレートのコード変更は不要。要件・ドキュメントで 1.2.0 以上を明示する材料になる

## 設計方針

- Apple の `nameValid` と同じ規則をクレート内の共通関数 (例: `src/core/util.rs` または専用モジュール) に実装する
- macOS の `AsyncRunner::start` で ID 確定直後に検証し、不正なら create 前に明示エラーを返す (破壊的変更)
- watchdog の `is_valid_container_id` も同じ規則に揃える (1 文字 ID を許可していた挙動は捨てる)
- 自動生成 ID は現行形式を維持し、検証を通ることを単体テストで担保する
- README の要件表・`docs/TESTCONTAINERS.md`・`skills/shiguredo-container/SKILL.md` に次を記載する
  - macOS ランタイムは Apple container 1.2.0 以上
  - コンテナ ID / `with_container_name` の文字種・長さ制約
- Linux 経路のコンテナ名検証は本 issue の必須範囲外 (macOS 1.2.0 対応が主目的)。共通関数化した場合に Linux でも使うかは実装時判断でよいが、完了条件は macOS とする

## 完了条件

- [ ] 不正な `with_container_name` (63 文字超、1 文字、禁止文字、先頭が非英数字) が macOS の start で create 前に明示エラーになること
- [ ] 自動生成 ID (`c-{unique_suffix()}`) が検証を通ること
- [ ] watchdog の ID 検証が Apple `nameValid` 相当に揃っていること
- [ ] README の要件に Apple container 1.2.0 以上が記載されていること
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` に ID 制約と 1.2.0 前提が記載されていること
- [ ] `CHANGES.md` に `[CHANGE]` (破壊的変更である旨を含む) エントリが記載されること
- [ ] 単体テストまたは統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
