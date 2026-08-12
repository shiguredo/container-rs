# バグ: macOS の LogConsumer 配信タスクが行長無制限でメモリを消費し続ける

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-log-consumer-line-length-limit
- Polished: {YYYY-MM-DD}

## 目的

改行を含まない巨大出力 (バイナリ・単一行ダンプ等) をコンテナが出力したときに、LogConsumer 配信タスクのメモリ使用量が無制限に成長しないようにする。

## 現状

`src/core/containers/async_container.rs` の `spawn_log_consumer_task` (macOS) は `read_until(b'\n')` で 1 行ずつ読み、`Vec` を再利用しながら行を蓄積する。

```rust
buf.clear();
match reader.read_until(b'\n', &mut buf).await {
```

この経路には行長の上限がなく、改行を一切含まない出力が続くと `buf` が無制限に成長し OOM し得る。

- Linux 側 (docker_log_stream.rs) は 8 MiB リングバッファ (drop-oldest) が行長を暗黙に 8 MiB へ制限するため、プラットフォーム間で非対称
- macOS の 1-shot ログ・exec 出力には 64 MiB 上限があるが、この配信タスクの経路には上限が無い
- コンテナが終了しても FD が EOF になるまで読み続ける構造のため、巨大な単一行が残っている限り解放されない

## 設計方針

- 行長上限 (例: 8 MiB) を設け、超過した行は切り捨てて warn ログを出すか、タスクを打ち切ってエラーにする
- Linux 側のリングバッファ方式と挙動を揃えることが望ましい (過剰な行は drop-oldest で捨てる)
- 上限値をコード内の定数として定義し、ドキュメント (rustdoc) にも明記する

## 完了条件

- 改行なしの巨大出力でメモリ使用量が有界であること (上限値を超えないこと)
- 通常の行配信・`LogConsumer` への配信・`WaitFor::Log` が従来どおり動作すること
- 上限に関するテスト (単体または macOS 統合) があること
