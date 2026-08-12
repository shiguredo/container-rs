# バグ: macOS exec の成功経路で読み取りスレッドの join にタイムアウトが無く永久ハングし得る

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-exec-reader-join-timeout
- Polished: {YYYY-MM-DD}

## 目的

exec の出力読み取りスレッドが、デーモン側が pipe の書き込み端を閉じない異常系で join されず、`exec()` が永久ハングする経路をなくす。

## 現状

`src/core/client/xpc_client.rs` の exec 処理は、読み取りスレッド 2 本を spawn し、正常系では「デーモンが書き込み端を閉じると EOF になり join が返る」ことを期待して join する。

- コメントは「デーモンが書き込み端を閉じると EOF になり join が返る」を期待するのみで、join 自体にタイムアウトはない
- デーモン側が dup 済みの write FD を閉じない異常系では EOF が来ず、`exec()` が永久ハングする
- キャンセルフラグは `containerWait` 失敗時 (エラーパス) のみで立てられ、この成功経路には効かない
- 関連: `issues/closed/0083-bug-macos-exec-fd-leak.md` で FD 回収は修正済みだが、join の打ち切りは対象外だった

## 設計方針

- 読み取りスレッドの join にタイムアウト (例: exec の実行時間上限) を設け、超過時は読み取りスレッドを打ち切ってエラーにする
- 打ち切り時はキャンセルフラグを立ててから join し、スレッドと FD を確実に回収する (0083 のパターンを踏襲)

## 完了条件

- デーモンが pipe を閉じない異常系で `exec()` が有限時間内にエラーを返すこと
- 正常系の exec (stdout / stderr 取得) が従来どおり動作すること
