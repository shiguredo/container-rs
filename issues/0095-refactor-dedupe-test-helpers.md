# リファクタリング: tests のヘルパー複製を `tests/helpers/mod.rs` に集約する

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-dedupe-test-helpers
- Polished: {YYYY-MM-DD}

## 目的

統合テスト間で完全に同一のヘルパー関数が複製されており、ゲート条件等の変更時の反映漏れでゲート不一致が再発し得る構造を解消する。

## 現状

以下のヘルパーが複製されている (コードベース全体のレビュー結果):

- `container_id_listed`: **3 コピー** (`tests/container_macos.rs` と `tests/container_sync_drop_macos.rs` に完全同一実装)
- `cleanup_container`: **2 コピー** (`tests/container_macos.rs` と `tests/container_sync_drop_macos.rs`。前者のコメントが「`test_container_sync_drop_macos.rs` の同名ヘルパーの複製」と自認)
- `skip_unless_host_network`: **2 コピー** (`tests/container_macos.rs` と `tests/nginx_http11.rs`。doc も含めて完全同一)

`tests/helpers/mod.rs` には現在 `skip_if_ci` しか置かれていない。

## 設計方針

- 各ヘルパーを `tests/helpers/mod.rs` に移動し、各テストファイルから `mod helpers;` + `helpers::xxx` で使う
- 既存の `skip_if_ci` と同じ公開形式に合わせる

## 完了条件

- 同一ヘルパーの複製が 0 になること
- 全テストが従来どおり通ること

## 解決方法

- `tests/helpers/mod.rs` に `container_id_listed` / `cleanup_container` / `skip_unless_host_network` を移動する
- `tests/container_macos.rs` / `tests/container_sync_drop_macos.rs` / `tests/nginx_http11.rs` の複製を削除して参照に置き換える
