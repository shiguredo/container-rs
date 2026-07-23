# バグ修正: fallback_fd の生 fd フォールバックに残る use-after-close 競合を解消する

- Priority: Low
- Created: 2026-07-23
- Completed: {YYYY-MM-DD}
- Model: qwen3.8-max-preview
- Branch: feature/fix-fallback-fd-race
- Polished: {YYYY-MM-DD}

## 目的

Linux ログストリームの `DockerLogsHandle::stop` は、`try_clone` 失敗時に demux タスク所有の `UnixStream` から借用した生 fd を `libc::shutdown` するフォールバックを持つ。demux 終了後に fd が close（再利用され得る）ため、`stop()` が fd を取得した直後に demux 終了 + fd 再利用が重なると、無関係な接続を shutdown し得る微小な競合窓が残る。現状は `demux_done` ガードと `terminate_all` での fd クリアで窓を狭め文書化しているが、根本解消は未。この残差競合を解消する。

## 優先度根拠

発火条件が「`try_clone` (=dup) 失敗（EMFILE 等の稀な経路）」かつ「stop と demux 終了 + fd 再利用が微小窓で重なる」と二重に稀で、影響も無関係接続の一時的な shutdown（データ破壊・機密漏洩にはならない）に留まるため Low。

## 現状

`src/core/client/docker_log_stream.rs`:

- `fallback_fd: std::sync::Mutex<Option<RawFd>>` に、`start_and_demux` で `try_clone` 失敗時に `stream.as_raw_fd()`（借用した生 fd、所有権なし）を格納する。
- `stop()` は `shutdown_socket`（複製）が無ければ `demux_done` を確認のうえ `fallback_fd` を take し `unsafe { libc::shutdown(fd, SHUT_RDWR) }` する。
- `terminate_all()` は demux 終了時に `fallback_fd` を `None` にクリアする。

残差窓: `stop()` が `demux_done=false` を読んで `fallback_fd` を take した直後、demux タスクが終了して `stream` が drop（fd close）され、OS がその fd 番号を別接続に再利用したうえで `stop()` が `libc::shutdown(fd)` を発行すると、無関係な接続が shutdown される。`RawFd` にはライフタイム保証が無いため、`demux_done` ガードではこの窓を完全には塞げない。

## 設計方針

選択肢を検討する:

1. demux タスクのソケット drop と `stop()` の shutdown を同一 Mutex で直列化し、fd 生存を保証する。
2. フォールバックを廃止し、`try_clone` 失敗時は `log_stop` フラグと後続の remove（デーモン側が接続を閉じる）による停止に頼る（dup はほぼ失敗しないためフォールバックの実益は小さい）。
3. fd 所有権をハンドル側に移し、shutdown 後に close する。

いずれを採用するかは実装時に判断する（2 が最も単純で安全、1/3 は停止の即時性を保てる）。

## 完了条件

- fallback fd の use-after-close 競合が解消される（またはフォールバック廃止で競合自体が無くなる）
- `try_clone` 失敗時でもログストリームが停止できる（廃止案の場合は log_stop + remove で停止すること）
- 既存の Linux 統合テスト・単体テストが pass する

## 解決方法

`src/core/client/docker_log_stream.rs` の `fallback_fd` 周辺を、採用した方針に従って再設計する。`stop()` の doc コメントの残差窓の注記を実態に合わせて更新する。
