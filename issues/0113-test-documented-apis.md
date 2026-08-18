# テスト: docs で「対応」宣言されている公開 API に統合テストを追加する

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/add-documented-api-tests
- Polished: {YYYY-MM-DD}

## 目的

`docs/TESTCONTAINERS.md` で「対応」と宣言されているのにテストが存在しない公開 API にテストを追加し、「対応宣言と検証の乖離」をなくす。

## 現状

docs では「対応」と宣言されているが、単体・統合ともテストが存在しない API が確認されている (コードベース全体のレビュー結果):

- **`with_ssh`**: macOS は XPC の ssh 設定反映 (`container_cfg.rs`)、Linux は start 時の明示エラー (`async_runner.rs` の `linux_unsupported_request_reason`)。どちらもテスト 0 件。Linux の fail-fast は単体テスト 1 本で塞げる
- **`with_init`**: macOS は useInit、Linux は HostConfig.Init に配線済み。テスト 0 件
- **`with_labels` / `with_label`** (`image_ext.rs`): テスト 0 件
- **`with_name` / `with_tag`**: テスト 0 件
- **`LoggingConsumer`** (`new` / `with_stdout_level` / `with_stderr_level` / `with_prefix`): tests/ はカスタム `LogConsumer` のみで `LoggingConsumer` の使用・検証 0 件
- **`get_host_port_ipv6`**: tests/ で呼び出し 0 件

統合テストなし (単体テストのみ) のもの:

- **Linux の `WaitFor::http` / `HttpWaitStrategy`**: `tests/nginx_http11.rs` は macOS 専用ゲート (`#[cfg(all(target_os = "macos", feature = "http_wait_plain"))]`) で、Linux に HTTP wait の統合テストが無い。`wait_until_ready` (http_strategy.rs) の実挙動が Linux で検証されたことがない (Docker はポートフォワードが素直に動くため CI で実行可能)
- **Linux の `Mount`** (bind / volume / tmpfs): `with_mount` の Linux 統合テストが無い

## 設計方針

- まず単体テストで塞げるもの (Linux の `with_ssh` fail-fast 等) から追加する
- 統合テストが必要なもの (Linux の `WaitFor::http`・`Mount`) は `issues/0070-test-add-linux-feature-integration-tests.md` のスコープと調整し、本 issue で追うものとの分担を明確にする
- カスタム `LogConsumer` のテストパターンを流用して `LoggingConsumer` の検証を追加する

## 完了条件

- 上記 API のうち、単体テストで検証可能なものすべてにテストがあること
- Linux の `WaitFor::http` の統合テストが 1 本以上あり、CI (Linux ジョブ) で実行されること
- 対応不能な項目がある場合は、docs の「対応」宣言を修正して整合させること
