# テスト: macOS で bind mount の起動前可視化を検証する自動テストを追加する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/add-macos-bind-mount-prestart-visible-test
- Polished: {YYYY-MM-DD}

## 目的

macOS (Apple container) で `Mount::bind_mount` を使った起動前ファイル可視化の自動テストを追加する。0057 で「起動前に見えるようになる」を公開契約として rustdoc に明記したが、リポジトリ内の自動テストでは検証されていないため、回帰検証の手段を確保する。

## 現状

- 既存の `xpc_alpine_with_bind_mount` (`tests/container_macos.rs`) は start 後に exec で `cat` 検証する型であり、初期プロセス起動時のタイミング保証を検証しない
- 0045 の PoC 実測 (`WaitFor::message_on_stdout(marker)` パターンで初期プロセスが起動時に bind mount 内容を読めたことを確認) のみで、リポジトリ内の自動テストには存在しない
- Linux 側には `copy_to_visible_before_initial_process` 相当の起動前可視化テストがあり、macOS 側に無い非対称がある

## 設計方針

- 0045 の候補 1 の実測手順 (`WaitFor::message_on_stdout(marker)` パターン) をテスト化する。起動コマンドが bind mount したファイルの内容を stdout に出す形で、初期プロセスが起動時に読めたことを検証する
- `xpc_alpine_with_bind_mount` (`tests/container_macos.rs`) の形式に合わせて追加する

## 完了条件

- [ ] `Mount::bind_mount` の内容を初期プロセスが起動時に読めることを検証する統合テストが追加されていること
- [ ] `cargo test --all-features` が pass すること
