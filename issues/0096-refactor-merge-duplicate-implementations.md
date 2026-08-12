# リファクタリング: 同一実装の重複を統合する

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-merge-duplicate-implementations
- Polished: {YYYY-MM-DD}
- Updated: 2026-08-05

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
- **`escape_json_value` と `escape_json`** (`src/core/client/registry_auth.rs` と `src/core/client/docker_client.rs`): 引用符の有無以外は完全に同一の制御文字エスケープロジック。片方の修正が他方に反映されないドリフト源 (0086 で `escape_json_value` が制御文字対応になり、両者がほぼ同一になった)
- **`demux_exec_stream` と `FrameDemuxer`** (`src/core/client/docker_client.rs` と `src/core/client/docker_log_stream.rs`): Docker Engine の 8 バイトヘッダ + payload の multiplex プロトコルの demux が 2 実装。exec 用は全蓄積後の一括処理、ログ用はストリーミング状態機械で形態は異なるが、ヘッダ定数・stream type の意味論・不完全フレームの扱いが二重管理になっており、片方の修正が他方に反映されない (現に docker_client 側は部分フレームを静かに破棄する挙動がログ側と微妙に異なる)
- **`percent_encode_path_segment` と `percent_encode_component`** (`src/core/client/docker_client.rs`): どちらも `percent_encode` を呼ぶだけの同一実装の別名 (パスセグメント / クエリ値の文脈を表すための別名)。テストも 2 つに分かれている
- **inspect エラー処理の 4 重複** (`src/core/client/docker_client.rs` の `bridge_ip_address` / `container_state` / `container_env` / `container_health`): `GET /containers/{id}/json` に対して 404 は `ContainerNotFound`・それ以外の 4xx/5xx は `Other("failed to inspect container: {status}")` を返す約 9 行のブロックが 4 箇所に逐語重複
- **`ContainerAsync::rm` の macOS / Linux 2 重複** (`src/core/containers/async_container.rs`): 本体は同一で `Client::MacOs(c) => c.remove(...)` / `Client::Linux(c) => c.remove(...)` のバリアント名だけが違い、rustdoc (完了保証・`keep` ゲートとの非対称の説明) まで完全に 2 重。同ファイル内の `rm_blocking` は 1 本の関数に cfg アームで統合済みなのに `rm` だけ 2 本化されており、方式が不統一
- **env の BTreeMap 畳み込みの build 側 2 箇所** (`src/core/client/container_cfg.rs` の init env と `src/runners/async_runner.rs` の Config.Env): exec 内の macOS / Linux 分岐 (上記) と同一の「BTreeMap に畳んで同名キーは後勝ち・`KEY=VALUE` に整形」処理。exec 側 2 箇所と合わせて 4 箇所の重複で、コメントもほぼ同一文
- **`XpcClient` の `spawn_blocking` + `XpcConn::connect` 定型パターン**: `src/core/client/xpc_client.rs` のほぼ全メソッドで「`spawn_blocking(move || { XpcConn::connect(SERVICE_NAME)?; ... })`」の冒頭 4 行を手書きしており、約 15 メソッドで繰り返し。共通の非同期ラッパー (`async fn call<T>(f: impl FnOnce() -> Result<T> + Send) -> Result<T>`) 1 つで置換できる

## 設計方針

- 各重複を共通関数・共通ヘルパーに統合する (挙動は変更しない)
- ログ配信ループはプラットフォーム共通のヘルパーに抽出する
- `resolve_or_pull_linux` は macOS 側と同じく「`ImageNotFound` のときのみ pull」に揃え、二重構造を解消する (amd64 明示時の強制 pull は macOS 側のみの分岐であり、Linux 側への導入有無を確認する)
- `Filters.labels` は未使用のため削除する
- `escape_json_value` と `escape_json` は共通の内部関数 (引用符なしのエスケープ本体) に集約し、`escape_json_value` は引用符なし・`escape_json` は引用符付きの薄いラッパーにする

## 完了条件

- 各重複が 1 実装になること
- 全テストが従来どおり通ること (挙動の変化がないこと)

## 解決方法

- `src/core/client/xpc_client.rs` の `wait_blocking` 系 2 関数と `bridge_ip_address` / `gateway_ip_address` を統合する
- LogConsumer の行配信ループを共通ヘルパーに抽出する (両プラットフォームから呼ぶ)
- `exec` の env マージを共通処理にまとめる
- `src/runners/async_runner.rs` の `resolve_or_pull_linux` を macOS 側と同じ条件 (404 限定) に揃える
- `xpc::Filters` の `labels` フィールドを削除する
- `escape_json_value` と `escape_json` を共通のエスケープ本体に集約する (引用符の有無は呼び出し側で付ける)
- `demux_exec_stream` と `FrameDemuxer` を共通の demux 処理に統合する
- `percent_encode_path_segment` / `percent_encode_component` を 1 関数に統合する (必要なら呼び出し側に文脈コメントを付ける)
- `docker_client.rs` の inspect エラー処理 4 箇所を共通ヘルパーに畳み込む
- `ContainerAsync::rm` の macOS / Linux 2 重複を `rm_blocking` と同じ 1 本化に揃える
- env の BTreeMap 畳み込み 4 箇所 (build 側 2 箇所 + exec 側 2 箇所) を共通処理にまとめる
- `xpc_client.rs` に `spawn_blocking` + `connect` の共通ラッパーを導入し、各メソッドの定型 4 行を置換する
