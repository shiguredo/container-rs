# 仕様変更: Apple container 1.2.0 のコンテナ ID 制約に合わせて検証する

- Priority: Medium
- Created: 2026-07-31
- Completed:
- Branch: feature/change-macos-container-id-validation
- Polished: 2026-08-01

## 目的

Apple container 1.2.0 で XPC リクエストに対するコンテナ ID 検証が強化された。本クレート側でも同じ規則で fail-fast し、不正な `with_container_name` が create 直前で落ちるのを防ぐ。あわせて macOS ランタイム要件を 1.2.0 以上に揃える。不正 ID はロールバックの `containerDelete` も同じ検証で拒否されるため、create 前の検証で一括して防ぐ。

互換破壊を許容する。

## 現状

- Apple container 1.2.0 ([#1956](https://github.com/apple/container/pull/1956)) で `ContainersHarness` が bootstrap / create / delete / diskUsage / logs / export の ID に対し `ManagedContainer.nameValid` を enforce する（対象外は stop / wait / kill / dial / resize / createProcess / startProcess / copyIn / copyOut / stats / list。ただし本クレートは同一 ID を使うため start での 1 回の検証で全経路をカバーできる）
- `nameValid` の規則
  - 長さが 63 以下
  - 正規表現 `^[a-zA-Z0-9][a-zA-Z0-9_.-]+$` (先頭は英数字、以降は英数字 / `_` / `.` / `-`、実質 2 文字以上)
  - 注: Swift の `name.count` は文字数だが、正規表現が ASCII 限定のため、Rust 側をバイト数 (`len()`) で実装しても 1.2.0 と同値の判定になる（非 ASCII は文字種チェックで落ちる）
- 本クレートの ID 決定 (`src/runners/async_runner.rs`)
  - `with_container_name` があればそれを ID にする
  - 無ければ `c-{unique_suffix()}` (`src/core/util.rs` の `unique_suffix`)
- `with_container_name` (`src/core/image/image_ext.rs`) は文字列をそのまま保存するだけで検証しない
- watchdog の `is_valid_container_id` (`src/watchdog.rs`) は空でない・`[A-Za-z0-9._-]` のみを見る。長さ上限も「2 文字以上」も無い
- 自動生成 ID (`c-...`) は通常 63 文字以内に収まるが、利用者が長い名前を渡すと 1.2.0 の XPC で create が失敗する
- README の要件表は `container` のバージョン下限を書いていない
- 不正 ID で create が失敗する場合、`rollback_remove` の `containerDelete` も同じ `nameValid` で拒否される（warn のみで元エラーは隠蔽されないため実害は無いが、クレート側の create 前検証がこの無駄なロールバック呼び出しも防ぐ）
- 関連: 0051 は 1.2.0 の `maskedPaths` / `readonlyPaths` 対応で、同じく 1.2.0 前提。0051 は「macOS ランタイム要件の 1.2.0 化は 0052 で対応する」と明記しており、本 issue がその要件確定の受け皿になる

## 設計方針

- Apple の `nameValid` と同じ規則をクレート内の共通関数 (例: `src/core/util.rs` または専用モジュール) に実装する
- macOS の `AsyncRunner::start` で、**pull / resolve より前**（macOS ブロック冒頭）に ID を確定して検証し、不正なら `crate::Error::other` で create 前に明示エラーを返す (破壊的変更)。ID は pull と無関係に決定できるため、fail-fast の趣旨からネットワーク I/O の前に検証する。エラーメッセージは英語で、ID の値と規則の要約を含める（例: `invalid container id "abc/def": must match ^[a-zA-Z0-9][a-zA-Z0-9_.-]+$ and be at most 63 characters`）
- watchdog の `is_valid_container_id` も同じ規則に揃える (1 文字 ID を許可していた挙動は捨てる)。実装は共通関数への委譲とし、二重実装にしない（規則のドリフト防止）。`nameValid` の文字種は現行規則 (`[A-Za-z0-9._-]`) の部分集合であり、空文字・空白・改行・glob 文字の拒否は維持されるため、reaper スクリプトのシェル安全性は保たれる。あわせて `is_valid_container_id` と `register` の doc コメントも新規則の説明に更新する
- 自動生成 ID は現行形式を維持し、検証を通ることをテストで担保する
- README の要件表・`docs/TESTCONTAINERS.md`・`skills/shiguredo-container/SKILL.md`・`with_container_name` の rustdoc に次を記載する
  - macOS ランタイムは Apple container 1.2.0 以上
  - コンテナ ID / `with_container_name` の文字種・長さ制約
- Linux 経路のコンテナ名検証は本 issue の必須範囲外 (macOS 1.2.0 対応が主目的)。共通関数は macOS 専用モジュール (`src/core.rs` の `util` は `#[cfg(target_os = "macos")]`) に置く。将来 Linux でも使う場合は cfg 調整と、Docker Engine 側のコンテナ名規則（`nameValid` と完全には一致しない）との突き合わせが別途必要になる

## 完了条件

- [ ] 不正な `with_container_name` (63 文字超、空文字、1 文字、禁止文字、先頭が非英数字) が macOS の start で pull / resolve より前に明示エラーになること（テストではエラー内容 (ID 値・規則の要約) を検証し、「pull より前」の位置はコードレビューで担保する）
- [ ] 自動生成 ID (`c-{unique_suffix()}`) が `nameValid` の全条件 (長さ上限含む) を通ること
- [ ] watchdog の ID 検証が Apple `nameValid` 相当に揃っていること（`src/watchdog.rs` の `accepts_valid_container_ids` テストの 1 文字 ID 許可ケースの反転・更新を含む）
- [ ] 境界値: ちょうど 63 文字は許可、64 文字は拒否されること（単体テストまたは PBT で担保）
- [ ] README の要件に Apple container 1.2.0 以上が記載されていること
- [ ] `docs/TESTCONTAINERS.md` と `skills/shiguredo-container/SKILL.md` に ID 制約と 1.2.0 前提が記載されていること
- [ ] `with_container_name` の rustdoc に ID 制約が記載されていること
- [ ] `CHANGES.md` に `[CHANGE]` (破壊的変更である旨を含む) エントリが記載されること
- [ ] 単体テスト (PBT を含む) または統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` が pass すること
