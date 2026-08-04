# リファクタリング: コメント重複の解消と stale コメントの修正を行う

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-dedupe-comments
- Polished: {YYYY-MM-DD}

## 目的

同一の説明が複数箇所に重複して貼られたコメント・rustdoc と、実装と食い違う stale コメントを整理し、保守コストを下げる。

## 現状

以下の重複・stale コメントが確認されている (コードベース全体のレビュー結果):

- **keep ゲート非対称の説明が 5 重複**: 「このメソッドは `TESTCONTAINERS_COMMAND=keep` でも削除する。`keep` ゲートは `Drop` の削除のみを抑止する仕様であり、明示 `rm` には効かない。」が `src/core/containers/async_container.rs` (macOS rm / Linux rm / rm_blocking) と `src/core/containers/sync_container.rs` (rm / rm_blocking) に同一文で 5 箇所
- **64 MiB 上限の説明が 4 重複**: 「`follow = false` (1-shot) は各ストリーム 64 MiB 上限で、超過時は読み出しを即座に止めてエラーを返す…合計最大 128 MiB が一時保持され得る)。macOS 側にこの上限は無い。」が `sync_container.rs` と `async_container.rs` の stdout / stderr 系 rustdoc に同一文で 4 箇所
- **リーダーの説明が 4 重複**: 「リーダーは独立した読み取り位置を持ち、常にログ先頭から読む。`follow = true` のときは…呼ばないこと。」が同様に 4 箇所
- **完了保証の説明が 4 重複**: 「`rm().await` の復帰時点で削除処理は終わっており…」が `async_container.rs` (rm / rm_blocking / Drop) と `sync_container.rs` に重複
- **stale コメント**:
  - `src/core/containers/sync_container.rs` の `container_state` の rustdoc「Linux (Docker) では未実装エラーを返す。」— 実装は Linux でも動作する (async_container.rs に Linux アームがある)
  - `src/core/wait/mod.rs` のモジュール doc「Linux ではログ FD が無く、非空メッセージ待ちは通常 startup timeout になる」— Linux は demux 済みログで Log 待機が対応済み (docs 10.1 とも自己矛盾)
- **「以前は〜」系の歴史説明**: `async_container.rs` (FD を consumer に奪わせていた) と `xpc_client.rs` (ipv6_mapping が永遠に空だった) の 2 箇所。git 履歴で残る情報

## 設計方針

- 共通文言は 1 箇所 (主要メソッドの rustdoc) に集約し、他は参照 (例: 「`rm` の rustdoc 参照」) に置き換える
- stale コメントは実装に合わせて修正する (削除ではなく正しい記述に置き換える)
- 歴史説明コメントは削除する (git 履歴が正)

## 完了条件

- 同一文の重複が 1 箇所以下になること
- stale コメント (container_state / wait/mod.rs) が実装と一致すること
- ビルド・テスト・clippy が通ること

## 解決方法

- `src/core/containers/async_container.rs` / `src/core/containers/sync_container.rs` の重複 rustdoc を整理する
- `src/core/containers/sync_container.rs` の `container_state` の doc を実装に合わせて修正する
- `src/core/wait/mod.rs` のモジュール doc を Linux 対応済みの記述に修正する
- 歴史説明コメントを削除する
