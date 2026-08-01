# バグ: XpcClient::logs 成功パスで余剰 FD がリークする

- Created: 2026-07-31
- Completed: 2026-08-01
- Branch: feature/fix-xpc-logs-surplus-fd-leak
- Polished: {YYYY-MM-DD}

## 目的

`XpcClient::logs` の成功パスで `fds[2..]` の余剰 FD が close されずリークするのを修正する。

## 現状

`src/core/client/xpc_client.rs` の `logs` メソッドは `reply.log_fds()` から FD 配列を取得し、`fds.len() < 2` と `fds[0] < 0 || fds[1] < 0` のエラーパスでは `close_valid_fds` で全 FD を close する。しかし成功パスの `Ok((fds[0], fds[1]))` では `fds[2..]` が close されない。

XPC `containerLogs` は通常 2 個の FD を返すため実害はほぼ無いが、エラーパスだけ塞いで成功パスを開けたままにするのは FD リーク修正の意図と非対称である。

## 設計方針

成功パスの `Ok((fds[0], fds[1]))` の直前で `close_valid_fds(&fds[2..])` を呼び、余剰 FD を close する。

## 完了条件

- [ ] `logs` の成功パスで `fds[2..]` が close されること
- [ ] `cargo test --all-features` と `cargo clippy --all-targets --all-features -- -D warnings` が pass すること

## 解決方法

修正対象のバグは存在しないため closed にする。

`XpcReply::log_fds` (`src/xpc/conn.rs`) は `[i32; 2]` の固定バッファで `xpc_bridge_get_log_fds` を呼び、C 側 (`src/xpc_bridge.c` の `xpc_bridge_get_log_fds`) が `count > max_fds` でクランプする。したがって `log_fds()` の戻り値の長さは最大 2 で、`fds[2..]` は常に空スライスになる。XPC 配列が 3 個目以降の FD を含んでいても、`xpc_array_dup_fd` で dup されず XPC reply オブジェクトが所有したままになり、`RawReply` の Drop (`xpc_bridge_release` → `xpc_release`) で配列ごと解放されるため、成功パスの余剰 FD リークは構造的に発生しない。

提案の `close_valid_fds(&fds[2..])` は常に空スライスへの呼び出しになり、恒久的 no-op である。
