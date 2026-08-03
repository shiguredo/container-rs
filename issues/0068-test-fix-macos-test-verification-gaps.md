# テスト: macOS 統合テストの検証の食い違いを修正する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-test-verification-gaps
- Polished: 2026-08-02

## 目的

macOS 統合テストの「検証対象とテスト名・意図の食い違い」と「スキップ条件の不整合」を修正し、誤検知と環境依存の失敗を解消する。

## 現状

- `tests/container_macos.rs` の `xpc_alpine_stdout_stderr_echo` は、コンテナが `/dev/stderr` に書いた `STDERR_MSG` を一切検証せず、Apple container 内部 (vminitd) のログ文言 (`setting up relay for StandardIO stderr`) を検証している。コンテナ自身の stderr のルーティングが壊れても pass する誤検知構造で、テスト名 (`echo`) とも食い違う。また vminitd の内部文言への依存はバージョン差で壊れ得る失敗要因でもある
- `src/core/containers/async_container.rs` の rustdoc (stderr 系 API の注意) には「Apple container の `containerLogs` が返す 2 本目の FD は VM の bootlog であり、アプリケーションの stderr は stdout 側のログに混流する」という仕様が文書化済み (docs/TESTCONTAINERS.md と SKILL.md にも同旨)。つまり init プロセスの stderr を stderr 側 FD で検証することは仕様上できない
- `tests/container_macos.rs` の `nginx_starts_without_with_cmd` は、published port 依存テスト群 (`test_container_http_wait` モジュールや `tests/nginx_http11.rs`) が `skip_unless_host_network` (RUN_HOST_NETWORK_TESTS ゲート) を使うのに対し、published port 依存テスト群の中で本テストのみが `GITHUB_ACTIONS` 環境変数をスキップ判定に使う。LNP 未許可のローカルでは既定 startup_timeout (最大 60 秒) 待って失敗する

## 設計方針

- `xpc_alpine_stdout_stderr_echo`: 「アプリの stderr が stdout 側ログに混流する」仕様に合わせ、stdout に `STDOUT_MSG` と `STDERR_MSG` の両方が含まれること (正) と、stderr 側 FD (bootlog) に `STDERR_MSG` が含まれないこと (負) を検証する形に改める。テスト名は変更せず、コメントに bootlog 混流の仕様を明記する。実装の最初に実機で次を確認し、結果を現状セクションに記録する: (1) stdout に `STDOUT_MSG` と `STDERR_MSG` の両方が含まれること、(2) stderr 側 FD (bootlog) に `STDERR_MSG` が含まれないこと、(3) リレー確立前の書き込みによる flaky の有無。混流が観測されない (1) が成立しない場合は docs / rustdoc の記述との矛盾として別 issue で扱う。(2) が成立しない場合 (bootlog にも混流する場合) は負の検証を外すか、仕様の再確認として別 issue で扱う (負の検証を外した場合は完了条件もそれに合わせて修正する)
- `nginx_starts_without_with_cmd`: スキップ条件を `skip_if_ci() || skip_unless_host_network()` に統一する (置換対象は `GITHUB_ACTIONS` 判定ブロックのみで、`skip_if_ci()` は維持する)。テストのコメントに「with_cmd なし起動の検証に HttpWaitStrategy (published port 経由) を使うためホストネットワークに依存する。published port 非依存の検証は `nginx_starts_without_with_cmd_log_wait` が担う」と役割分担を明記する

## 完了条件

- `xpc_alpine_stdout_stderr_echo` が、stdout に `STDOUT_MSG` と `STDERR_MSG` の両方が含まれることと、stderr 側 FD (bootlog) に `STDERR_MSG` が含まれないことを検証し、コメントに bootlog 混流の仕様が明記されていること
- 混流の実機確認の結果 (上記 (1)〜(3)) が現状セクションに記録されていること
- `nginx_starts_without_with_cmd` が `skip_if_ci() || skip_unless_host_network()` でスキップされ、`RUN_HOST_NETWORK_TESTS` 未設定の環境で即スキップされること (60 秒待ちが発生しない。検証: `env -u RUN_HOST_NETWORK_TESTS RUN_CONTAINER_TESTS=1 cargo test --all-features --test container_macos nginx_starts_without_with_cmd -- --exact --nocapture` で「スキップ:」が即出力されることを確認する。`--exact` は部分一致で `nginx_starts_without_with_cmd_log_wait` が実走するのを防ぐため)
- 修正後のテストが docs/TESTCONTAINERS.md / SKILL.md の bootlog 混流の記述と矛盾しないこと

## 解決方法

- `xpc_alpine_stdout_stderr_echo`: stderr 側の vminitd 文言検証 (バージョン差で壊れ得るため) と `stderr_to_vec()` 呼び出しを削除し、stdout に `STDOUT_MSG` と `STDERR_MSG` の両方が含まれることと、stderr 側 FD に `STDERR_MSG` が含まれないことを検証する。bootlog の実体検証 (vminitd 文言の存在確認) は文言依存を避けるため行わない。コメントに「Apple container は 2 本目の FD が bootlog であり、アプリの stderr は stdout 側に混流する」仕様を明記する
- `nginx_starts_without_with_cmd`: `GITHUB_ACTIONS` 判定ブロックを `skip_unless_host_network()` に置き換え、`skip_if_ci() || skip_unless_host_network()` の形にする。コメントに published port 依存の理由と `nginx_starts_without_with_cmd_log_wait` との役割分担を明記する
- `tests/container_macos.rs` を変更するため、同一ファイルを変更する 0058 / 0059 / 0060 / 0061 / 0062 / 0069 (macOS の exec テスト追加のため) とマージ順に注意する (0063 は `async_container.rs` のみの変更のため対象外)
