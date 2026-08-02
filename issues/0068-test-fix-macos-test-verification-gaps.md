# テスト: macOS 統合テストの検証の食い違いを修正する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-test-verification-gaps
- Polished: {YYYY-MM-DD}

## 目的

macOS 統合テストの「検証対象とテスト名・意図の食い違い」と「スキップ条件の不整合」を修正し、誤検知と環境依存の失敗を解消する。

## 現状

- `tests/container_macos.rs` の `xpc_alpine_stdout_stderr_echo` は、コンテナが `/dev/stderr` に書いた `STDERR_MSG` を一切検証せず、Apple container 内部 (vminitd) のログ文言 (`setting up relay for StandardIO stderr`) を検証している。stderr 捕捉が壊れても pass する誤検知構造で、テスト名 (`echo`) とも食い違う。また vminitd の内部文言への依存はバージョン差で壊れるフレーク要因でもある
- `tests/container_macos.rs` の `nginx_starts_without_with_cmd` は、同じファイルの published port 依存テスト群 (`test_container_http_wait` モジュール) や `tests/nginx_http11.rs` が `skip_unless_host_network` (RUN_HOST_NETWORK_TESTS ゲート) を使うのに対し、本テストのみ `GITHUB_ACTIONS` 環境変数の判定で、Local Network Privacy 未許可のローカルで必ず既定 startup_timeout (60 秒) 待って失敗する

## 設計方針

- stderr 検証は「コンテナ自身の stderr が捕捉されること」を検証する形に改める。捕捉できない仕様 (bootlog 混流) ならその旨をテスト名とコメントに明記する
- published port 依存テストのスキップ条件を `skip_unless_host_network` に統一する

## 完了条件

- 両テストがテスト名・コメントの意図どおりの性質を検証する
- LNP 未許可のローカル環境で `nginx_starts_without_with_cmd` が即スキップされる

## 解決方法

- `xpc_alpine_stdout_stderr_echo` の stderr 検証を実態に合わせる (STDERR_MSG の捕捉検証、または仕様上の制約の明記とテスト名の修正)
- `nginx_starts_without_with_cmd` のスキップ判定を `skip_unless_host_network()` に置き換える
