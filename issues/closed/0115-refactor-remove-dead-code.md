# リファクタリング: 未使用・不要なコードを削除する (死にコードの削除)

- Created: 2026-08-12
- Completed: 2026-08-18
- Branch: feature/refactor-remove-dead-code
- Polished: {YYYY-MM-DD}

## 目的

コードベース全体のレビューで確認された死にコード・不要な抽象化を削除し、保守コストを下げる。**削除対象はすべて「参照元ゼロ」を機械確認済み**であり、誤削除が起きないよう各項目の確認方法と根拠を明記する。

## 現状

以下の 6 項目が削除候補として確認されている。削除する前に、各項目について「呼び出し元ゼロ」を再度確認すること (本 issue の記述はレビュー時点の確認結果であり、実装時点のコードで再確認する):

### 1. `async_runner.rs` の `resolve_or_pull_linux` の戻り値が捨てられている

- 対象: `src/runners/async_runner.rs` の `resolve_or_pull_linux` の呼び出し側 (`let _desc_raw = ...` と受けて捨てている箇所)
- 現状: pull 後の再 resolve が返す descriptor (digest) を破棄し、`build_container_config` は `req.descriptor()` (name:tag) を使う。resolve は「存在確認 + ローカルへの pull」の副作用のみが効いており、戻り値は完全に無駄
- 確認方法: `rg "resolve_or_pull_linux"` で呼び出し 1 箇所のみ・受けた値を参照していないことを確認
- 対応: 戻り値を使う (digest を config の image に反映する) か、関数を `Result<()>` 化して受け捨てを消す
- 注: pull の二重構造そのものの解消は `issues/0096-refactor-merge-duplicate-implementations.md` の対象。本 issue は受け捨ての解消のみ

### 2. `xpc/conn.rs` の `unsafe impl Send for XpcConn`

- 対象: `src/xpc/conn.rs` の `unsafe impl Send for XpcConn {}`
- 現状: `XpcConn` は全使用箇所で単一スレッド内 (spawn_blocking クロージャ内 or wait_blocking の std スレッド内) に閉じており、スレッドを跨がない。`RawReply` の Send (spawn_blocking の戻り値として必須) とは異なり、この unsafe impl が無くてもコンパイルが通る
- 確認方法: `XpcConn` を保持する箇所 (conn.rs の呼び出し側) がスレッド境界を越えるかを確認。削除して `cargo check` が通ることを確認
- 対応: unsafe impl を削除する

### 3. `watchdog.rs` の `is_valid_container_id` 委譲ラッパーと重複テスト

- 対象: `src/watchdog.rs` の `is_valid_container_id` と、その検証テスト (`accepts_valid_container_ids` / `rejects_invalid_container_ids`)
- 現状: ラッパーは `crate::core::util::is_valid_container_id(id)` への 1 行委譲のみで、cfg ゲートも追加ロジックもない。`core::util` は macOS 専用ゲート、watchdog も macOS 専用のため、コンパイル上の必要性もない。テストは `src/core/util.rs` の検証テストと同じ入力の部分集合を再テストしている
- 確認方法: `rg "is_valid_container_id"` で util.rs の定義と watchdog.rs の委譲・テストのみがヒットすることを確認
- 対応: ラッパーと重複テストを削除し、watchdog 側は `crate::core::util::is_valid_container_id` を直接呼ぶ。util.rs 側のテストに一本化する

### 4. `env.rs` のユニット構造体 `Config`

- 対象: `src/core/env.rs` の `Config` (フィールドを持たず `command()` 1 メソッドのみ)
- 現状: env モジュールは `pub(crate)` のため本家互換の公開 API 制約がなく、フィールドなし構造体のメソッド呼び出しはフリー関数で置き換え可能。`Command` enum 自体は使用箇所を確認済み (async_runner.rs / async_container.rs) で削除しない
- 確認方法: `rg "Config::"` と `rg "env::Config"` で使用箇所を確認
- 対応: `Config` をフリー関数 `command()` に置き換えて構造体を削除する

### 5. `Makefile` の `cover` ターゲット

- 対象: `Makefile` の `cover`
- 現状: `cargo llvm-cov` に依存するが、リポジトリ内で導入・バージョン固定されておらず、CI でも使用されない
- 確認方法: `rg "llvm-cov"` と `.github/workflows/` で未使用を確認
- 対応: ターゲットを削除する

### 6. `error.rs` の `WaitLogError::EndOfStream(Vec<Vec<u8>>)` の複数チャンク対応

- 対象: `src/core/error.rs` の `WaitLogError::EndOfStream` の型と Display 実装
- 現状: `log_strategy.rs` の `CollectedLogs::into_chunks()` は常に「空 or 1 要素」に連結して返すのに、型は `Vec<Vec<u8>>` (複数要素) で、Display は複数要素の末尾連結反復を実装している。実経路で複数要素が現れない以上、推測で追加された一般化
- 確認方法: `rg "EndOfStream"` で構築箇所 (log_strategy.rs) が常に 0 または 1 要素であることを確認。テスト (error.rs 内) が 2 要素で Display を検証している場合はテストも書き換える
- 対応: 型を `Option<Vec<u8>>` (または `Vec<u8>` 1 個) に縮め、Display を単純な末尾 1024 バイト取り出しにする
- 注: `ExecError::WaitLog` 等の未使用エラーバリアント削除は `issues/0072-refactor-remove-dead-error-variants.md` の対象であり、本 issue の対象外

## 設計方針

- 各項目について、削除前に必ず「参照元ゼロ」を再確認する (本 issue の現状記述はレビュー時点の確認結果)
- 削除後に `cargo check` / `cargo clippy --all-targets --all-features -- -D warnings` / 全テストが通ることを確認する
- 挙動の変更は行わない (削除のみ)

## 完了条件

- 上記 6 項目がすべて削除されていること
- 削除前後で公開 API の挙動が変わらないこと (テストの変更は項目 6 の Display 検証のみ)
- ビルド・clippy・全テストが通ること

## 解決方法

- 項目 1 (`resolve_or_pull_linux` の戻り値): 関数を `Result<()>` 化し、呼び出し側の `let _desc_raw =` 受け捨てを消した。pull 後の再 resolve の戻り値 (digest) は未使用のため破棄する旨をコメントに明記した
- 項目 2 (`unsafe impl Send for XpcConn`): `src/xpc/conn.rs` から削除した (削除後も `cargo check` が通ることを確認済み)
- 項目 3 (watchdog の委譲ラッパー): `src/watchdog.rs` の `is_valid_container_id` ラッパーと、`src/core/util.rs` の検証テストと同じ入力を再テストしていた 2 本のテスト (`accepts_valid_container_ids` / `rejects_invalid_container_ids`) を削除し、`crate::core::util::is_valid_container_id` を直接呼ぶようにした
- 項目 4 (`env.rs` の `Config`): ユニット構造体を削除し、フリー関数 `command()` に置き換えた。呼び出し側 4 箇所 (`async_runner.rs` 3 箇所・`async_container.rs` 1 箇所) を `crate::core::env::command()` に更新した
- 項目 5 (Makefile の `cover`): ターゲットと `.PHONY` のエントリを削除した
- 項目 6 (`WaitLogError::EndOfStream`): 型を `Vec<Vec<u8>>` から `Vec<u8>` に縮め、Display を「末尾 1024 バイトの単純なプレビュー」に置き換えた。`CollectedLogs::into_chunks` は `into_bytes` に改名して `Vec<u8>` を返すようにし、`log_strategy.rs` の単体テスト・`tests/container_macos.rs` の統合テスト・`docs/TESTCONTAINERS.md`・`skills/shiguredo-container/SKILL.md` の型記述を更新した
- 検証: `cargo fmt` / `cargo clippy --all-targets --all-features -- -D warnings` / `cargo test --all-features` (357 本) がすべて通ることを確認した
