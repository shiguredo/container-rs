# バグ: LogConsumer 配信タスクが行長無制限でメモリを消費し続ける

- Created: 2026-08-12
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-log-consumer-line-length-limit
- Polished: 2026-08-12

## 目的

改行を含まない巨大出力 (バイナリ・単一行ダンプ等) をコンテナが出力したときに、LogConsumer 配信タスクのメモリ使用量が無制限に成長しないようにする。

## 現状

macOS の配信タスク (`src/core/containers/async_container.rs` の `spawn_log_consumer_task`) は `read_until(b'\n')` で 1 行ずつ読み、`Vec` を再利用しながら行を蓄積する。

```rust
buf.clear();
match reader.read_until(b'\n', &mut buf).await {
```

この経路には行長の上限がなく、改行を一切含まない出力が続くと `buf` が無制限に成長し OOM し得る。

- Linux 側の配信タスク (`src/core/client/docker_log_stream.rs` の `spawn_log_consumer_task`) も同じ `read_until` 構造で行長無制限。8 MiB リングバッファ (drop-oldest) は共有バッファ (demux 側) のメモリを有界にするだけで、配信タスクの行蓄積は制限しない。つまり両プラットフォームとも同じ OOM 経路を持つ
- macOS の exec 出力には 64 MiB 上限 (`src/core/client/xpc_client.rs` の `read_file_to_vec_cancellable`) があるが、1-shot ログと配信タスクの経路には上限が無い
- コンテナが終了しても EOF 観測 + 猶予期間 (`DRAIN_GRACE`) 経過まで読み続ける構造のため、稼働中の巨大な単一行は `buf` に蓄積され続ける

## 設計方針

- 行長上限を 8 MiB に確定する (Linux の共有バッファ上限 `DEFAULT_BUFFER_LIMIT` と同じ値)。境界は「ちょうど 8 MiB は超過としない」(0064 の 64 MiB 境界と同じ定義)。上限は行コンテンツ (改行を除く) で数える
- 上限値を macOS / Linux で共有できる場所に定数として定義し、rustdoc にも明記する (0064 が `DOCKER_RESPONSE_BODY_LIMIT` を複数経路で共有した先例と同じ方針。macOS / Linux の 2 重定義は将来のドリフトを生むため行わない)
- 上限を超えた行は「先頭 8 MiB を 1 フレームとして配信し、残余は次の改行まで読み捨てる」方式に確定し、超過時に warn ログを出す (タスク打ち切りは不採用: 同一セッション中は再 spawn されない構造のため、1 行超過で以後の配信が全て失われる)
- 切り捨てフレームは行末ではないため、既存の末尾 `\n` / `\r` 除去 (配信直前の `buf.pop()`) は適用しない (適用すると 8 MiB 境界の `\r` で 1 バイト欠損する)
- `read_until` は行長上限を持たないため、`fill_buf` / `consume` ベースの読み取りに変更する。EOF 時の挙動は従来どおりを維持する: 改行なしの最終行は残余バイトを 1 フレームとして配信し、macOS は EOF 後もポーリングを継続する (EOF は終端ではなく「現時点の末尾」。Linux の EOF = break を macOS に適用すると 0062 で修正した追記ログ配信が壊れる)。読み捨て中に EOF へ達した場合は捨て残りを配信しない
- macOS / Linux の両配信タスクを対象とする (行分割ロジックは同一の修正を適用する)
- Linux の drop-oldest とは行の扱いが揃わない (Linux はバイト単位で先頭を捨て末尾が残るのに対し、macOS の順方向読みでは先頭 8 MiB を残す切り捨てになる)。メモリ有界という目的は両方で達成され、この非対称は許容する
- macOS の 1-shot ログ (`stdout_to_vec` / `stderr_to_vec` 等) への上限導入は対象外

## 完了条件

- 改行なしの巨大出力で配信タスクのメモリ使用量が行長上限 (8 MiB) を超えないこと
- 上限を超える行は先頭 8 MiB が 1 フレームとして配信され、残余が読み捨てられること (warn ログ出力つき)
- 通常の行配信・`LogConsumer` への配信が従来どおり動作すること (`WaitFor::Log` は配信タスクと独立した経路のため回帰確認のみ。macOS の EOF 後のポーリング継続と改行なし最終行の配信も維持されること)
- 行長上限超過の切り捨てと残余の読み捨て、ちょうど 8 MiB の境界を検証する単体テストがあること (macOS の配信タスクは private かつ FD 入力のため、行分割ロジックをテスト可能な形に分離して検証する)
- `CHANGES.md` に `[FIX]` エントリが記載されること
