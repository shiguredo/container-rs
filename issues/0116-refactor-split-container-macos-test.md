# リファクタリング: tests/container_macos.rs の巨大モジュールを分割する

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/refactor-split-container-macos-test
- Polished: {YYYY-MM-DD}

## 目的

4031 行に達した `tests/container_macos.rs` のうち、単独で 2100 行を超えるモジュールを分割し、テストの可読性と保守性を上げる。

## 現状

`tests/container_macos.rs` (4031 行) は mod 単位で構造化されているが、内訳が偏っている (コードベース全体のレビュー結果):

- `test_container_macos`: 45-732 (約 690 行)
- `test_container_xpc`: 733-2840 (約 2100 行) ← 突出
- `test_container_sync`: 2841-3254 (約 410 行)
- `test_container_http_direct`: 3255-3359 (約 100 行)
- `test_container_http_wait`: 3366-3472 (約 100 行)
- `test_drop_removal_race`: 3478-3582 (約 100 行)
- `test_exec_after_start`: 3589-3760 (約 170 行)
- `test_container_restart`: 3764-4031 (約 270 行)

- テストシナリオの重複は確認されていない (mod 内で同一シナリオの重複なし)
- 統合テストのバイナリ分割には制約がある: `tests/container_macos_log_consumer_fd.rs` に「このバイナリにテストを追加しないこと」と明記されており、並列実行による FD 数干渉を避ける設計のため、バイナリ分割ではなく mod 分割のみが安全

## 設計方針

- 新規ファイル (例: `tests/container_macos_xpc.rs`) を追加し、`container_macos.rs` から `test_container_xpc` モジュールを移す
- 統合テストは各ファイルが独立したバイナリになるため、`mod` の参照方法 (`#[path]` や `mod common` パターン) を既存の構成に合わせて決める
- テストの内容は変更しない (移動のみ)

## 完了条件

- 1 ファイルあたりの行数が 1500 行以下になること
- テストの内容・実行環境 (ゲート・CI) が変わらないこと
- 全テストが通ること
