# バグ: Linux のログ 1-shot 取得・ copy_from ・ pull 進捗にメモリ上限が無い

- Created: 2026-08-02
- Completed: 2026-08-03
- Branch: feature/fix-linux-memory-accumulation-limits
- Polished: 2026-08-02

## 目的

Linux (Docker Engine API) 経路の無制限メモリ蓄積 3 経路に上限を設定し、macOS の exec 出力と同じ「64 MiB・超過時エラー」の方針に揃える (0055 で exec 出力に導入済みの方式)。

## 現状

- `src/core/client/docker_log_stream.rs` の `fetch_logs_oneshot_blocking` は `SharedLogBuffer::new(usize::MAX)` で全ログを蓄積し、`drain_buffer` で全量 `Vec<u8>` 化する。`ContainerAsync::stdout_to_vec` / `stderr_to_vec` (follow なし) から到達し、巨大ログを吐くコンテナで OOM になり得る。macOS のログ 1-shot 経路にも上限は無い (exec 出力の `read_file_to_vec` だけが 64 MiB 上限を持つ)
- `src/core/client/docker_client.rs` の `DockerClient::copy_from` は `request` (BodyLimit::Unlimited) で `GET /containers/{id}/archive` のレスポンス (tar) を全蓄積する。`ContainerAsync::copy_file_from` から到達し、コンテナ内の巨大ファイルで OOM になり得る
- `DockerClient::pull_image` はプル進捗ストリーム (JSON Lines) を無制限にバッファリングしてから `check_pull_stream_errors` で検査する。ボディ全体をバッファリングするため OOM になり得る

## 設計方針

- 1-shot ログと copy_from: 蓄積に 64 MiB の上限を設け、超過時は切り詰めずエラーを返す (exec 出力と同じ方針。上限値は macOS の exec 出力 (`read_file_to_vec`) と同じ 64 MiB)
- 1-shot ログの上限は stdout / stderr 各ストリーム 64 MiB とする (macOS の exec 出力と同じくストリーム別。Linux の exec が multiplexed 全体で 64 MiB なのとは非対称だが、demux 後のストリーム別蓄積という構造上の違いであり許容する)。境界は「ちょうど 64 MiB は成功、64 MiB 超はエラー」とする
- pull 進捗: 上限付き受信 (`BodyLimit::Error(64 MiB)`) に変更する (プル進捗は実用上 64 MiB 未満に収まる。ボディの逐次デコードへの変更は実装量が大きいため行わない)

## 完了条件

- 3 経路とも上限超過時にエラーが返り、OOM 経路が無くなる
- 上限の境界を検証するテストが追加される (統合テストで 64 MiB 超を流すのは CI 負荷が高いため行わない。方式は解決方法参照。1-shot は境界に加えて「超過後も読み続けず早期アボートすること (ハングしないこと)」も検証する。pull / copy_from は配線のコードレビューで検証する)
- 公開 API の rustdoc に上限と超過時の挙動が明記されている (`stdout_to_vec` / `stderr_to_vec` / `stdout(false)` / `stderr(false)` のリーダー (async と sync の両方) / `copy_file_from`。1-shot 経路は stdout / stderr を同一セッションで取得するため、片方のストリームの超過で両方の取得が失敗することも明記する。これらの API は macOS でも公開されており、macOS 側は上限なしのままであることも明記する)
- `CHANGES.md` に `[FIX]` エントリが追加されている

## 解決方法

- `src/core/client/docker_log_stream.rs` の 1-shot 蓄積を各ストリーム 64 MiB 上限・超過時エラーに変更する。`SharedLogBuffer` に strict モード (`new_strict`) を追加し、follow 経路の drop-oldest (8 MiB) は変更しない。`FrameDemuxer::feed` を `Result<(bool, bool)>` 化して上限超過を伝播し、検知した時点で即時エラー (早期アボート) する。エラー文言はストリーム識別子付きの `stdout output exceeds ... bytes limit` / `stderr output exceeds ... bytes limit`
- 1-shot 蓄積の上限はテストから差し替え可能にするため、`fetch_logs_oneshot_blocking_with_limit(socket_path, id, limit)` を内部関数として分離し、公開経路は `DOCKER_RESPONSE_BODY_LIMIT` (64 MiB) を渡す
- `src/core/client/docker_client.rs` の `copy_from` を `BodyLimit::Error(64 MiB)` で受信する (上限は tar 全体。ヘッダ + データ + トレーラ)。`pull_image` は `request_with_extra_headers` に `body_limit` パラメータを追加して `BodyLimit::Error(64 MiB)` で受信し、上限超過時は `failed to receive pull progress for image ...` の文脈で包む
- 上限定数は `EXEC_OUTPUT_BODY_LIMIT` を `DOCKER_RESPONSE_BODY_LIMIT` に改名して 4 経路 (exec / 1-shot ログ / copy_from / pull 進捗) で共有する (将来の上限変更で一部だけが変わる非対称を防ぐ)
- 公開 API の rustdoc (`stdout` / `stderr` / `stdout_to_vec` / `stderr_to_vec` / `copy_file_from`。async / sync 両方) に 64 MiB 上限・超過時の挙動・1-shot は stdout / stderr を同一セッションで取得するため片方の超過で両方が失敗し合計最大 128 MiB が一時保持され得ること・macOS 側に上限が無いことを明記する
- 単体テスト: strict モードの境界テスト 4 本 (ちょうど上限成功・累積超過・単発超過・stderr 超過) と、上限超過時に読み出しを止めてエラーを返す (ハングしない) ことの回帰テスト `oneshot_aborts_early_when_over_limit` を追加する (実 Unix ソケットのサーバが上限超フレームを送り続け、クライアントが早期に Err で戻ることを検証)。既存テストの `feed` 呼び出しは `.expect(...)` で不変条件 (非 strict では失敗しない) を固定する
- `copy_from` / `pull_image` の `BodyLimit::Error` の超過時エラー自体は `http_decode.rs` の単体テストで検証済みのため、配線はコードレビューで検証する。`http_decode.rs` のコメント (exec 以外で使われない旨) を現在の利用経路に合わせて更新する
- `CHANGES.md` に `[FIX]` エントリと rustdoc 更新分の `[UPDATE]` (misc) エントリを追加する
