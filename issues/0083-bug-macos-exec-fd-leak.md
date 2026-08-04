# バグ: macOS の exec で `containerWait` 失敗時に読み取りスレッド 2 本と FD 2 個がリークする

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-macos-exec-fd-leak
- Polished: {YYYY-MM-DD}

## 目的

macOS の `ContainerAsync::exec` で `containerWait` がエラーを返した場合に、stdout / stderr の読み取りスレッドと pipe FD がコンテナのプロセス終了まで回収されず蓄積する問題を修正する。

## 現状

- `src/core/client/xpc_client.rs` の `exec` は、stdout / stderr 用に pipe を作成し、`containerStartProcess` 後に読み取りスレッド 2 本 (`read_file_to_vec`) を起動してから `containerWait` を送信する
- `containerWait` がエラーを返すと、コメント (「join するとデーモンが書き込み端を閉じるまで戻れないためデタッチする」) の意図どおり読み取りスレッドを**デタッチしたまま即 `Err` を返す**
- デタッチされたスレッドは、コンテナのプロセスが生きている間はデーモン側が書き込み端を閉じないため、`read_file_to_vec` にブロックしたまま**読み取り端 FD 2 個を握り続ける** (書き込み端は close 済みだがデーモン側の dup が開いたまま)
- `containerWait` がエラー応答を返すのはプロセス起動済みのケース (プロセス ID 不整合等) であり、その場合プロセスは継続実行されるため、スレッドと FD はコンテナ終了まで回収されず、失敗ごとに蓄積する

## 設計方針

- エラーパスでも非ブロッキングに回収する手段を導入する (例: 読み取りに上限時間を設ける、短いタイムアウト付きの監視でスレッドを強制終了する、または失敗時に pipe を close して EOF を発生させる)
- デタッチの意図 (join によるハング回避) は維持する

## 完了条件

- `containerWait` 失敗後の exec で、読み取りスレッドと FD が有限時間内に回収されること
- 正常系の exec の挙動が変わらないこと (統合テスト)

## 解決方法

- `src/core/client/xpc_client.rs` の `exec` のエラーパスで、読み取りスレッドがブロックしないよう pipe の読み取り側を close する (EOF を発生させてスレッドを終了させる) か、タイムアウト付きの読み取りに変更する
- macOS の統合テストで、エラーパスの exec 後に FD 数が増加しないことを検証する
