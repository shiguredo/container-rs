# バグ: Linux のログ 1-shot 取得・copy_from・pull 進捗にメモリ上限が無い

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-linux-memory-accumulation-limits
- Polished: {YYYY-MM-DD}

## 目的

Linux (Docker Engine API) 経路の無制限メモリ蓄積 3 経路に上限を設定し、macOS 側 (64 MiB) や follow ログ (8 MiB) と上限方針を揃える。

## 現状

- `src/core/client/docker_log_stream.rs` の `fetch_logs_oneshot_blocking` は `SharedLogBuffer::new(usize::MAX)` で全ログを蓄積し、`drain_buffer` で全量 `Vec<u8>` 化する。`ContainerAsync::stdout_to_vec` / `stderr_to_vec` (follow なし) から到達し、巨大ログを吐くコンテナで OOM になり得る。macOS は `read_file_to_vec` に 64 MiB 上限があり非対称
- `src/core/client/docker_client.rs` の `DockerClient::copy_from` は `request` (BodyLimit::Unlimited) で `GET /containers/{id}/archive` のレスポンス (tar) を全蓄積する。`ContainerAsync::copy_file_from` から到達し、コンテナ内の巨大ファイルで OOM になり得る
- `DockerClient::pull_image` はプル進捗ストリーム (JSON Lines) を全蓄積してから `check_pull_stream_errors` で検査する。エラーは逐次検出可能なのにボディ全体をバッファリングする

## 設計方針

- 1-shot ログと copy_from: 蓄積に上限 (例: 64 MiB) を設け、超過時は切り詰めずエラーを返す (exec 出力と同じ方針)
- pull 進捗: ボディを逐次デコードして `error` 行を検出するか、上限付き受信に変更する

## 完了条件

- 3 経路とも上限超過時にエラーが返り、OOM 経路が無くなる
- 上限の境界を検証するテストが追加される

## 解決方法

- `fetch_logs_oneshot_blocking` の `SharedLogBuffer` に上限を設定し、超過をエラーにする
- `copy_from` を `BodyLimit::Error` (64 MiB) で受信する
- `pull_image` のボディ処理を逐次検出に変更する (または上限付き受信)
