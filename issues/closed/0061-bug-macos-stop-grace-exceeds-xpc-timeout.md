# バグ: macOS stop_with_timeout がグレース 60 秒超で XPC タイムアウトの誤エラーを返す

- Created: 2026-08-02
- Completed: 2026-08-03
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

- `src/core/client/xpc_client.rs` の `XpcClient::stop` で `XpcConn::send` を `send_with_timeout` に変更し、XPC 送信タイムアウトを設計方針の計算式 `max(DEFAULT_TIMEOUT, min(グレース + 30 秒, LONG_TIMEOUT))` で求める。計算は純関数 `xpc_timeout_for_grace` に切り出し、u64 で飽和計算する
- `src/xpc/conn.rs` の `DEFAULT_TIMEOUT` を `pub(crate)` 化し、`src/xpc/mod.rs` の re-export に追加する
- `XpcClient::stop` の rustdoc に、XPC 送信タイムアウトが「グレース + 30 秒 (下限 60 秒・上限 24 時間)」になる旨と、24 時間後に `XpcTimeout` が返り得る旨、ランタイム drop 時のハング注意を追記する
- 公開 API 側 (`ContainerAsync::stop_with_timeout` / `SyncContainer::stop_with_timeout`) の rustdoc に、macOS では負値またはグレース + 30 秒が 24 時間を超える指定で最大 24 時間ブロックされた後に XPC タイムアウトのエラーが返り得る旨を追記する (負値は macOS では `i32::MAX` 秒、Linux では 30 秒に変換されることも明記)。`docs/TESTCONTAINERS.md` の機能対照表にも上限 24 時間の注記を追記する
- 単体テスト: `xpc_timeout_for_grace` の境界値テスト 4 本を追加する (60 秒下限維持・グレース + 30 秒・24 時間飽和・u64 オーバーフローなし)
- 統合テスト: `tests/container_macos.rs` に `xpc_alpine_stop_with_timeout_grace_ignores_sigterm` を追加する (`trap '' TERM` で SIGTERM を無視する init にグレース 61 秒を指定し、誤エラーなしで停止し、経過時間 61 秒以上・`is_running()` false を検証する)。実機で 64 秒で通過することを確認済み
- `CHANGES.md` に `[FIX]` エントリを追加する
