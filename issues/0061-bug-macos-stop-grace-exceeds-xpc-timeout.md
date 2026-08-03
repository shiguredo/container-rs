# バグ: macOS stop_with_timeout がグレース 60 秒超で XPC タイムアウトの誤エラーを返す

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-stop-grace-timeout
- Polished: 2026-08-02

## 目的

`ContainerAsync::stop_with_timeout` の公開引数範囲のうち、グレース + apiserver の停止処理時間が XPC 呼び出しのタイムアウト (60 秒) を超える指定で、停止は成功するのに誤ったタイムアウトエラーが返るのを修正する。グレース 60 秒超の指定は確実にタイムアウトする。

## 現状

- `src/core/client/xpc_client.rs` の `XpcClient::stop` は `XpcConn::send` (DEFAULT_TIMEOUT = 60 秒、`src/xpc/conn.rs` の `send`) で `containerStop` を送る
- `timeout_seconds` は `stopOptions.timeoutInSeconds` にのみ反映され、XPC 呼び出し自体のタイムアウトには影響しない
- apiserver は停止処理の完了を待って reply を返すため、グレース 60 秒超の指定 (`Some(120)` や負値 = `i32::MAX` 秒) では XPC 呼び出しが先にタイムアウトし `ClientError::XpcTimeout` が返る。デーモン側の停止は裏で続行される
- 実機確認 (macOS 26.4 / Apple container 1.2.0): `with_cmd(["sh", "-c", "trap '' TERM; exec tail -f /dev/null"])` の init に対して `stop_with_timeout(Some(120))` を実行すると 60.01 秒後に `client error: XPC request timed out` が返り、その後 120 秒時点で SIGKILL によりコンテナが停止した (タイムアウトは誤エラーであり、停止自体は成功する)
- 実行中コンテナに対する SIGTERM 経路 (graceful 停止) の統合テストは全 OS で未検証 (Linux の `Some(1)` は既停止コンテナへの冪等検証のみ)

## 設計方針

XPC 送信タイムアウトを「グレース + 余裕 (30 秒)」に拡張する。計算式は `max(DEFAULT_TIMEOUT, min(グレース + 30 秒, LONG_TIMEOUT))` とし、u64 で飽和計算する:

- `None` (グレース 30 秒) → 60 秒 (現行の DEFAULT_TIMEOUT を維持)
- `Some(0)` (SIGKILL) → 60 秒 (現行を下回らない)
- `Some(t)` (t > 0) → t + 30 秒
- 負値 (グレース `i32::MAX` 秒) → `LONG_TIMEOUT` (24 時間) で飽和

グレース自体の上限は設けない (負値 = `i32::MAX` 秒の SIGTERM 仕様を維持する)。負値や、グレース + 余裕が `LONG_TIMEOUT` に達する正のグレース (約 24 時間 - 30 秒以上) を SIGTERM 無視のコンテナに指定した場合は 24 時間後に `XpcTimeout` が返り得るが、これは実用上の上限として許容する (グレース (最大約 68 年) に完全に合わせると `spawn_blocking` の drop ハングリスクが事実上無限になるため、XPC タイムアウトは 24 時間で飽和する)。

## 完了条件

- 正のグレース 60 秒超 (例: `Some(120)`) の `stop_with_timeout` がエラーを返さず、SIGTERM → グレース経過 → SIGKILL の経路でコンテナが停止する (macOS 実機。SIGTERM 経路 (None / 正のグレース / 負値) の網羅テストは 0069 が担当する)
- `CHANGES.md` に `[FIX]` エントリが追加されている

## 解決方法

- `XpcClient::stop` で `XpcConn::send_with_timeout` に設計方針の計算式のタイムアウトを渡す (u64 で飽和計算し、i32 境界のオーバーフローを避ける。`conn.rs` の `send_with_timeout` 内の `u64::try_from(...).unwrap_or(u64::MAX)` と同じ発想)
- `XpcClient::stop` の rustdoc に、XPC 送信タイムアウトが「グレース + 30 秒」になる旨 (負値は 24 時間で飽和) を追記する。あわせて公開 API 側 (`ContainerAsync::stop_with_timeout` / `SyncContainer` 相当) の rustdoc に、負値指定時に呼び出し側が最大 24 時間ブロックされ得る旨を追記する
- `tests/container_macos.rs` に `trap '' TERM` (SIGTERM 無視) の init を使った統合テストを追加する (テスト名: `xpc_alpine_stop_with_timeout_grace_ignores_sigterm`)。グレースは 60 秒超の最小値 (61 秒) を使い、テスト所要時間を最小化する。「エラーなしで停止し、グレース経過後に SIGKILL で止まること」を検証する (経過時間がグレース値以上であることと、`is_running()` が false になることを確認する。`trap ''` が効かず SIGTERM で即死する誤実装を検出するため)。このテストは SIGTERM 経路のシナリオ検証であり、`tests/helpers/mod.rs` の後始末規約 (停止は `stop_with_timeout(Some(0))`) の例外であることをコメントで注記する。実装時に SIGKILL 後の reply 到着遅延が 30 秒以内であることを実機で確認する
- macOS のグレース 60 秒超の回帰テストは本 issue が担当する。SIGTERM 経路 (None / 正のグレース / 負値) の網羅テストは 0069 が担当する (0069 の解決方法の graceful 停止テストと重複しないようにする)
- `tests/container_macos.rs` を変更するため、同一ファイルを変更する 0058 / 0059 / 0060 / 0068 / 0069 とマージ順に注意する
- `CHANGES.md` に `[FIX]` エントリを追加する
