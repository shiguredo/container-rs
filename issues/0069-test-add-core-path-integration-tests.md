# テスト: stop_with_timeout の SIGTERM 経路・CmdWaitFor 未検証バリアント・volume_mount / ReadOnly の統合テストを追加する

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/add-core-path-integration-tests
- Polished: 2026-08-02

## 目的

「実装はあるが一度も実機を通っていない」主要経路の統合テストを追加し、バックエンドごとの実挙動の欠陥を検出できるようにする。

## 現状

- 実行中コンテナに対する `stop_with_timeout` の SIGTERM 経路 (None / 正のグレース / 負値) は全 OS で未検証 (既存は `Some(0)` のみ。Linux の `Some(1)` は既停止コンテナへの冪等検証のみ。`tests/helpers/mod.rs` の規約が `Some(0)` を強制した結果、SIGTERM の graceful 経路が一度も実機検証されていない)
- `CmdWaitFor::Exit { code: None }` と `Duration` 系 (`seconds` / `millis`) が全 OS 未テスト。macOS の exec 経路 (`XpcClient::exec`) の `StdErrMessage` も未テスト (Linux はテスト済み)
- `Mount::volume_mount` と `AccessMode::ReadOnly` の実ランタイム経由の統合テストが皆無 (macOS には bind / tmpfs のテストがあるが、volume_mount / ReadOnly は両 OS とも未テスト。Linux は mount 系テスト自体皆無)。`ImageExt::with_copy_to` の rustdoc (macOS の virtiofs bind の文脈) が ReadOnly 併用を推奨しているのに実機検証が無い

## 設計方針

各バックエンドの実装が完全に分離 (XPC vs Docker Engine API) しているため、片 OS の検証はもう片方の保障にならない。必要な経路を macOS と Linux の両方で検証する (OS ごとの担当は各 issue で分担する)。

- SIGTERM 経路: 0061 が macOS のグレース 60 秒超を担当する。本 issue は None / 正のグレース (60 秒以内) / 負値の graceful 停止を両 OS で検証する
- volume_mount / ReadOnly: macOS の volume_mount 統合テストは 0060 が担当する。本 issue は Linux で volume_mount と ReadOnly bind を検証する (macOS の ReadOnly bind は本 issue の対象外として許容する)

## 完了条件

- `stop_with_timeout` の SIGTERM 経路 (None / 正のグレース (60 秒以内) / 負値) が Linux と macOS の両方で検証される (グレース 60 秒超は 0061 が担当)
- `CmdWaitFor` の未検証バリアント (`Exit { code: None }`・`Duration` 系・macOS の `StdErrMessage`) が両 OS で検証される (`Duration` 系は `millis` で代表して検証する。`seconds` は同一バリアントのため別途不要)
- Linux で `volume_mount` と `AccessMode::ReadOnly` の bind が実機で検証される (macOS の `volume_mount` は 0060 が担当)
- `RUN_CONTAINER_TESTS=1 cargo test --all-features` が pass すること (Linux は CI (test-linux-docker)、macOS は `RUN_CONTAINER_TESTS=1` で実行される)

## 解決方法

- `trap 'exit 0' TERM` を仕込んだ init (例: `sh -c "trap 'exit 0' TERM; while :; do sleep 1; done"`) で graceful 停止を検証するテストを Linux と macOS の両方に追加する。None / 正のグレース (例: 10 秒) / 負値の 3 ケースで「SIGTERM が届き、コンテナが exit code 0 で終了すること」を検証する (trap 経由の graceful 終了は exit code 0、SIGKILL なら 137 になるため区別できる。137 の報告形式は macOS では未実機確認のため、実装時に確認し食い違う場合は注記する。exit code の取得は停止後にバックグラウンド wait の記録をポーリングする (既存の `alpine_exit_code_after_exit` と同型))。グレース値の経過自体は検証しない (即応答 init ではグレースが消費されない。グレース経過の検証は 0061 が担当)。負値は Linux では None と同値 (30 秒) に正規化され、macOS では `i32::MAX` 秒になるが、どちらの OS でも graceful 終了 (exit code 0) を検証する。macOS の負値テストは即応答 init のため 0061 の修正前後で成功するが、SIGTERM 不達のバグで挙動が変わる (0061 修正前: 60 秒で `XpcTimeout` / 修正後: 24 時間飽和) ため、stop 呼び出しを `tokio::time::timeout` (例: 5 分) で包んで打ち切れるようにする。このテストは `tests/helpers/mod.rs` の後始末規約 (停止は `stop_with_timeout(Some(0))`) の例外であることをコメントで注記する。テスト名は既存の命名に合わせて `alpine_stop_with_timeout_graceful_sigterm` (Linux) / `xpc_alpine_stop_with_timeout_graceful_sigterm` (macOS) とする
- `CmdWaitFor::exit()` (終了コード不問)・`CmdWaitFor::millis` のテストを追加し、macOS の exec で `message_on_stderr` のテストを追加する (exec は `XpcClient::exec` が専用 pipe で stderr を取得するため、0068 の bootlog 混流仕様 (containerLogs の 2 本目 FD) とは無関係に成立する)。`millis` の検証は所要時間計測で行う (例: `millis(500)` が 500ms 以上かかること)
- Linux で `volume_mount` と ReadOnly bind を各 1 本追加する (macOS の volume_mount 統合テストは 0060 が担当する)。Docker でも名前付きボリュームはコンテナ削除後も残存するため、テスト用ボリューム名はユニーク化する (例: `volume_{pid}_{nanos}`。ユニーク名のため事前削除は不要)。検証は exec の結果 (exit code / stdout) で行う (例: マウントポイントにファイルを書き込んで読めること)。ReadOnly bind は書き込みが拒否されること (sh -c での touch が失敗し exit code が 0 以外になること) を検証する。掃除ではコンテナの `rm` に加えてボリュームの削除 (`docker volume rm`) を行う
- `tests/container_macos.rs` と `tests/container_linux.rs` を変更するため、同一ファイルを変更する 0058 / 0059 / 0060 / 0061 / 0062 / 0065 / 0066 / 0068 / 0070 とマージ順に注意する
