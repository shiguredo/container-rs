# テスト: macOS で bind mount の起動前可視化を検証する自動テストを追加する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/add-macos-bind-mount-prestart-visible-test
- Polished: 2026-08-02

## 目的

macOS (Apple container) で `Mount::bind_mount` を使った起動前ファイル可視化の自動テストを追加する。0057 で「起動前に見えるようになる」を rustdoc に明記した (containerCreate 時点での materialize はコード構造からの推論であり、実測は 0045 の PoC のみ) が、リポジトリ内の自動テストでは検証されていないため、回帰検証の手段を確保する。

## 現状

- 既存の `xpc_alpine_with_bind_mount` (`tests/container_macos.rs`) は start 後に exec で `cat` 検証する型であり、初期プロセス起動時のタイミング保証を検証しない
- 0045 の PoC 実測 (`WaitFor::message_on_stdout(marker)` パターンで初期プロセスが起動時に bind mount 内容を読めたことを確認。新規パス・ディレクトリ全体・既存パス配下の子ファイル bind のいずれも成立) のみで、リポジトリ内の自動テストには存在しない
- Linux 側には `copy_to_visible_before_initial_process` 相当の起動前可視化テストがあり、macOS 側に無い非対称がある

## 設計方針

- Linux の `copy_to_visible_before_initial_process` (`tests/container_linux.rs`) と同型の marker パターンでテスト化する。`with_copy_to` を `with_mount(Mount::bind_mount(...))` に置き換え、起動コマンドが bind mount したファイルの内容を stdout に出して `WaitFor::message_on_stdout(marker)` で待つ形にする
- 追加するテストは次の 2 本 (Linux 側の 2 本構成に合わせる):
  - 新規パスへの単一ファイル bind (ホスト一時ファイル → `/data/payload.txt` 等)
  - 既存 non-empty ディレクトリ配下の既存ファイル bind (`/etc/motd` 等。0045 で成立確認済みのパターン。macOS の bind mount は virtiofs によりホスト側ファイルを共有するため、`with_copy_to` のスナップショット投入とは意味論が異なる。この違いはテストコメントにも明記する)
- 起動コマンドは marker を stdout に出した後にコンテナを生かし続ける形にする (例: `cat` の後に `sleep`)。marker 出力後に初期プロセスが exit する形は、ログ待機の EOF 処理に依存するため避ける
- テスト名は `xpc_alpine_bind_mount_visible_before_initial_process` / `xpc_alpine_bind_mount_overwrite_visible_before_initial_process` とし、`tests/container_macos.rs` の `xpc_alpine_with_bind_mount` の近くに追加する
- `with_startup_timeout` でタイムアウトを明示する (Linux 版と同じ 15 秒程度)
- `xpc_alpine_with_bind_mount` の前後処理 (skip_if_ci ガード・ホスト一時ファイルの作成と削除・stop と rm) に合わせる
- `tests/container_macos.rs` を変更するため、同一ファイルを変更する 0058 / 0060 / 0061 / 0068 / 0069 とマージ順に注意する

## 完了条件

- [ ] 新規パスへの単一ファイル bind の内容を初期プロセスが起動時に読めることを検証する統合テストが追加されていること
- [ ] 既存パス配下の既存ファイル bind の内容を初期プロセスが起動時に読めることを検証する統合テストが追加されていること
- [ ] テストコメントに、macOS の bind mount (virtiofs によるホスト側ファイル共有) が `with_copy_to` のスナップショット投入とは意味論が異なる旨が明記されていること
- [ ] `RUN_CONTAINER_TESTS=1 cargo test --all-features --test container_macos xpc_alpine_bind_mount_` が pass すること (テスト名の共通接頭辞で 2 本をフィルタする)
- [ ] `RUN_CONTAINER_TESTS=1 cargo test --all-features` が pass すること
