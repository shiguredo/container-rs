# バグ: macOS exec の成功経路で読み取りスレッドの join にタイムアウトが無く永久ハングし得る

- Created: 2026-08-12
- Completed: 2026-08-18
- Branch: feature/fix-exec-reader-join-timeout
- Polished: 2026-08-12

## 目的

exec の出力読み取りスレッドが、デーモン側が pipe の書き込み端を閉じない異常系で join されず、`exec()` が永久ハングする経路をなくす。

## 現状

`src/core/client/xpc_client.rs` の exec 処理は、読み取りスレッド 2 本を spawn し、正常系では「デーモンが書き込み端を閉じると EOF になり join が返る」ことを期待して join する。

- コメントは「デーモンが書き込み端を閉じると EOF になり join が返る」を期待するのみで、join 自体にタイムアウトはない
- EOF が来ない異常系では `exec()` が永久ハングする (デーモンが dup 済みの write FD を閉じないケース、または exec プロセスの子プロセスが write FD を継承したまま残るケース。クライアントからは区別不能で修正は同一)
- キャンセルフラグは `containerWait` 失敗時 (エラーパス) のみで立てられ、この成功経路には効かない
- 関連: `issues/closed/0083-bug-macos-exec-fd-leak.md` で FD 回収は修正済みだが、join の打ち切りは対象外だった (0083 は「正常系の読み取り契約 (EOF までブロックして読み切る) は変えない」と明記)

## 設計方針

- `containerWait` 成功後の join 待ちに上限時間 (5 秒) を設け、超過時は読み取りスレッドを打ち切ってエラーにする。上限は「プロセス終了後にデーモンが write FD を閉じるまでの猶予」であり、exec 自体の実行時間は制限しない (join は `containerWait` 成功後にしか実行されず、その時点でプロセスは終了済みのため。「exec の実行時間上限」で打ち切ると長時間 exec の正常系が壊れる。5 秒は大出力の読み取りが `containerWait` と並行に進むため join 後に残るのはデーモンの FD クローズ遅延のみ、という分析に基づく固定値)
- `std::thread::join` にはタイムアウトがないため、読み取りスレッドの結果を mpsc チャネル (stdout / stderr を識別する enum を送る 1 本) で受け取り `recv_timeout(5 秒)` で待つ構造に変更する (2 本のチャネルを直列に待つと合計最長 10 秒になるため 1 本にまとめる)
- 打ち切り時はキャンセルフラグを立ててから join し、スレッドと FD を確実に回収する (0083 のパターンを踏襲。フラグ観測から poll 間隔 100ms 以内に終了する)。片方のストリームだけが打ち切られた場合も他方のスレッドをフラグで打ち切って回収する。フラグが立つとスレッドは `Ok(None)` を返すため、既存の `expect("正常系の exec では読み取りが打ち切られないため Some になること")` を除去し、`Ok(None)` を打ち切りエラーに変換する
- 打ち切り時は読み切れた分の出力と exit code を捨ててエラーを返す (正常終了したプロセスでも打ち切られる。異常系でハングさせるより望ましいと判断)。エラーは `ClientError::Other` で「`read stdout/stderr timed out after 5s`」形式の英語メッセージにする (ストリーム識別つき)
- 0083 の「フラグが立たなければ時間で打ち切らない」契約は維持されるが、「正常系の読み取り契約 (EOF までブロックして読み切る)」は EOF が来ない異常系では 5 秒で打ち切られる (キャンセルフラグの用途を「エラーパス」から「エラーパス + 正常系の打ち切り」に拡張する)
- エラーパス (`containerWait` 失敗時) は 0083 どおりデタッチのまま維持する (mpsc 化は成功経路のみ)

## 完了条件

- EOF が来ない異常系で `exec()` が上限時間 (5 秒 + スレッド終了までの猶予 (poll 間隔 100ms + read 1 回)) 内にエラーを返すこと (recv_timeout 待ち部分を関数として抽出し、write 端を開いたままの pipe で単体テストする。exec() 全体は spawn_blocking + XPC 接続のため単体テスト対象外。タイムアウト値は定数化しテストから短縮注入できるようにする)
- 正常系の exec (stdout / stderr 取得) が従来どおり動作すること (既存の exec 統合テストが引き続き通ること)
- 修正で陳腐化するコメント・`expect` (`read_file_to_vec_cancellable` の doc の「エラーパス専用」・「正常系の exec では読み取りが打ち切られないため Some になること」) が更新されること
- `CHANGES.md` に `[FIX]` エントリが記載されること

## 解決方法

- `src/core/client/xpc_client.rs` の `XpcClient::exec` から、読み取りスレッドの結果を 1 本の mpsc チャネル (`ExecReaderMsg` enum で stdout / stderr を識別) で受け取り `recv_timeout` で待つ構造に変更した
- 待ち上限を求めるロジックを新設した `join_exec_readers` 関数に抽出し、`EXEC_READER_JOIN_TIMEOUT` (5 秒固定) を渡して単体テストからは短縮注入できるようにした
- タイムアウト時はキャンセルフラグを立て (0083 のパターン踏襲)、残っているスレッドの結果を 2 段目 recv で回収してから、どのストリームが応答しなかったかを含めたエラー (`read stdout/stderr timed out after 5000ms` 相当) を返す。読み切れた分の出力と exit code は捨てる
- 打ち切り時のエラー種別は `ClientError::Other`、メッセージは英語で統一 (未受信ストリーム名は共通ヘルパ `describe_missing` で組み立て)
- Disconnected (読み取りスレッドが結果を送らず終了) も同ヘルパで未受信ストリーム名を含めたエラーに変換する。読み取りエラーは `read stdout failed: ...` / `read stderr failed: ...` に変換して伝播する
- `containerWait` 失敗時のエラーパスは 0083 の設計 (デタッチ + キャンセルフラグ) を維持する。mpsc 化は成功経路のみ
- `read_file_to_vec_cancellable` の doc を更新し、キャンセル契機の 2 通り (`XpcClient::exec` の containerWait 失敗パス、`join_exec_readers` のタイムアウト分岐) と目的を明記した。従来の「エラーパス専用」の記述と `expect("正常系の exec では読み取りが打ち切られないため Some になること")` を除去した
- テスト: `join_exec_readers` の単体テスト 6 件 (両ストリーム受信・両方タイムアウト・片方タイムアウトのストリーム名報告・Disconnected 両方 missing・Disconnected 片方 missing・読み取りエラー伝播) と、実 pipe + 実読み取りスレッド 2 本で EOF 未着信の異常系を再現しキャンセルフラグでスレッドを回収する統合的な単体テスト 1 件を追加した
- `CHANGES.md` の develop セクションに `[FIX]` エントリを追加した
