# バグ: Linux のログ 1-shot 取得・ copy_from ・ pull 進捗にメモリ上限が無い

- Created: 2026-08-02
- Completed: {YYYY-MM-DD}
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

- `fetch_logs_oneshot_blocking` の 1-shot 蓄積を各ストリーム 64 MiB 上限・超過時エラーにする。`SharedLogBuffer` は follow 経路で drop-oldest (先頭から切り捨て、8 MiB) として使われるため、既存の drop-oldest 挙動は変えず、1-shot 経路専用の「超過を検知してエラーを返す」モードを追加する (0055 が既存の上限挙動を変えず exec 専用のエラーモードを追加したのと同じ配慮)。上限超過を検知した時点で即時エラーを返す (早期アボート。exec と同じ方針)。エラーは `output exceeds ... bytes limit` の文言 (macOS と同じ) にする
- `copy_from` を `BodyLimit::Error(64 MiB)` で受信する (`request_with_body_limit` を使う。同関数の「exec start の出力読み出し専用」というコメントは本変更で古くなるため更新する)。上限は tar 全体 (ヘッダ + データ + トレーラ) に掛かるため、ファイル内容 64 MiB ちょうどでも tar オーバーヘッド分でエラーになり得る。利用者向けの `copy_file_from` rustdoc には「tar 形式のオーバーヘッド分を考慮する」旨を添える
- `pull_image` を `BodyLimit::Error(64 MiB)` で受信する。`request_with_extra_headers` は BodyLimit パラメータを持たないため、body_limit パラメータを追加するか同等の経路を設ける。エラー文言はフレーミング依存になり得る (Content-Length 宣言の超過はデコーダ側の `body too large` が先に返る)。早期アボートでソケットを切った場合、デーモン側のプルは継続してイメージがローカルに残り得る (0055 の exec と同じトレードオフとして許容する)
- 境界テストは、上限値をテストから差し替え可能にして単体テストで小さな上限 (例: 8 バイト) で検証する (既存の `shared_buffer_drops_oldest_over_limit` と同じ手法)。1-shot は加えて「上限超過を検知した時点で読み出しを止めてエラーを返す (ハングしない)」ことの回帰テストも追加する。pull / copy_from の `BodyLimit::Error` の超過時エラー自体は `http_decode.rs` の単体テストで検証済みのため、配線 (正しい上限が渡ること) はコードレビューで検証する
- 上限値の定数は既存の `EXEC_OUTPUT_BODY_LIMIT` (64 MiB) と同じ値になるため、定数を共有するか各ファイルに定義するかを実装時に統一する (将来の上限変更で一部だけが変わる非対称を防ぐ)
- 1-shot ログの follow 経路 (8 MiB・切り詰め) との値・挙動の差 (64 MiB・エラー) は、1-shot が「決定的に全ログを取得する」契約を持つことによる設計判断として維持する
- macOS のログ 1-shot 経路は無制限のまま残る (本 issue の対象外として許容する。Linux 側にだけ上限が入る非対称が生じる)
- `docker_client.rs` と `docker_log_stream.rs` を変更するため、同じ `DockerClient::copy_from` を変更する 0065 とマージ順に注意する (0065 の 404 分岐と 0064 の BodyLimit 変更は領域が別だが、同一関数のためコンフリクト時は両方の変更を保持する)
- `CHANGES.md` に `[FIX]` エントリを追加する
