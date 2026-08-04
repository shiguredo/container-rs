# リファクタリング: 同一実装の重複を統合する

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-merge-duplicate-implementations
- Polished: {YYYY-MM-DD}

## 目的

ほぼ同一の実装が複数箇所に重複しており、片方の修正が他方に反映されないリスクがあるのを解消する。

## 現状

以下の重複実装が確認されている (コードベース全体のレビュー結果):

- **`XpcClient::wait_blocking` と `wait_blocking_with_timeout`** (`src/core/client/xpc_client.rs`): タイムアウト引数以外は完全に同一。1 関数に統合できる
- **`bridge_ip_address` と `gateway_ip_address`** (`src/core/client/xpc_client.rs`): 取得キー (`ipv4Address` / `ipv4Gateway`) とエラーメッセージの一部だけが違う約 60 行 × 2。キーを引数に取る 1 関数に畳める
- **LogConsumer の行配信ループ** (`src/core/containers/async_container.rs` の macOS 実装と `src/core/client/docker_log_stream.rs` の Linux 実装): `read_until(b'\n')` + 末尾 `\n` / `\r` 除去 + `to_frame` + consumer 配信のループが完全に同一 (コメントで「両プラットフォームで揃えている」と自認)
- **exec の env マージ** (`src/core/containers/async_container.rs` の `exec` 内 macOS / Linux 分岐): BTreeMap に積んで `KEY=VALUE` にする処理が同一形。ベース (コンテナ env vs リクエスト env) だけが違い、`split_once('=')` の展開と `format!("{k}={v}")` の収束が重複
- **Linux の pull の二重構造** (`src/runners/async_runner.rs` の `resolve_or_pull_linux` と `src/core/client/docker_client.rs` の `resolve_image_descriptor`): どちらも 404 → pull → 再解決を行う (コメントで「二重構造は維持する」と自認)。正常系で pull が必要なケースに pull が 2 回走り得る
- **`xpc::Filters.labels`** (`src/core/client/xpc_client.rs`): 常に空の `HashMap` で渡され、設定箇所が皆無。フィールドごと削除するか、実際に使うまで消す

## 設計方針

- 各重複を共通関数・共通ヘルパーに統合する (挙動は変更しない)
- ログ配信ループはプラットフォーム共通のヘルパーに抽出する
- `resolve_or_pull_linux` は macOS 側と同じく「`ImageNotFound` のときのみ pull」に揃え、二重構造を解消する
- `Filters.labels` は未使用のため削除する

## 完了条件

- 各重複が 1 実装になること
- 全テストが従来どおり通ること (挙動の変化がないこと)

## 解決方法

- `src/core/client/xpc_client.rs` の `wait_blocking` 系 2 関数と `bridge_ip_address` / `gateway_ip_address` を統合する
- LogConsumer の行配信ループを共通ヘルパーに抽出する (両プラットフォームから呼ぶ)
- `exec` の env マージを共通処理にまとめる
- `src/runners/async_runner.rs` の `resolve_or_pull_linux` を macOS 側と同じ条件 (404 限定) に揃える
- `xpc::Filters` の `labels` フィールドを削除する
