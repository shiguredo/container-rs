# バグ: HttpWaitStrategy の「1 MiB で切り詰め」契約が 10 MiB 超の応答でエラーに化ける

- Created: 2026-08-04
- Completed: 2026-08-05
- Branch: feature/fix-http-wait-truncate-limit
- Polished: 2026-08-04

## 目的

`HttpWaitStrategy` のレスポンスボディが「1 MiB で切り詰めて続行」する契約になっているのに、10 MiB 超の応答でデコーダ側の上限によりエラーになる乖離を修正する。

## 現状

- `src/core/wait/http_strategy.rs` の `send_request_inner` は `ResponseAccumulator::new(..., BodyLimit::Truncate(MAX_HTTP_RESPONSE_BODY_BYTES))` (1 MiB) でレスポンスを蓄積する
- 一方 `src/core/client/http_decode.rs` の `ResponseAccumulator::new` は、`BodyLimit::Truncate` の場合 `ResponseDecoder::new()` (デコーダ既定の `max_body_size` = 10 MiB、`shiguredo_http11` の `limits.rs` で確認) を使う
- そのため 10 MiB 超のレスポンスは「切り詰めて続行」ではなく、Content-Length 宣言の超過はヘッダ解析時点で、close-delimited は蓄積超過で、chunked はチャンクサイズ行解析時点でエラーになる
- `HttpResponse::body()` の rustdoc は「保持量は最大 1 MiB であり、それを超えた部分は切り詰められる」と契約しており、実挙動と乖離している (1 MiB 〜 10 MiB の間は切り詰めが効くため、10 MiB 超でのみ顕在化する)

## 設計方針

- `shiguredo_http11` の `max_body_size` は受信総量 (`body_consumed`) の制限であり、超過は常に `BodyTooLarge` エラーになる (切り詰めではない)。Content-Length フレーミングはヘッダ解析時点 (宣言長のチェック) で、close-delimited は蓄積時で、chunked はチャンクサイズ行解析時にエラーになる
- 切り詰め (保持量の制限) は既存の `drain_body` の Truncate 分岐 (`http_decode.rs`) が担っている
- よって `BodyLimit::Truncate(max)` ではデコーダの `max_body_size` を無効化 (`DecoderLimits { max_body_size: u64::MAX, ..Default::default() }`。`DecoderLimits::unlimited()` は `max_buffer_size` / `max_headers_count` / `max_header_line_size` / `max_chunk_line_size` も無制限にするため使わない) し、切り詰めは `drain_body` に任せる。同一パターンはログストリームのデコーダ構築 (`docker_log_stream.rs`) に既存
- `BodyLimit::Unlimited` (全量保持) は対象外とし、従来どおりデコーダ既定の 10 MiB 上限のままにする (現行の `_ => ResponseDecoder::new()` の catch-all を分割して `Truncate` のみ `u64::MAX` 化する)
- `BodyLimit::Error` と同じ揃え方は誤り (`Error` は「デコーダ先出エラーとアプリ後出エラーで結果が一致する」ための揃えであり、`Truncate` は「エラー vs 切り詰め」で結果が異なる)
- メモリ面: `read_buf_size` は 8192 にキャップされ、各 feed で consume されるためデコーダ内バッファは肥大しない。受信総量のガードは失われるが、`request_timeout` (10 秒) と保持量 1 MiB で実質 bounded になる (修正前は 10 MiB 超でエラーになっていた巨大応答が、修正後は読み切るまで継続し、完了しなければ `request_timeout` で打ち切られる挙動変化がある。Content-Length / close-delimited / chunked のすべてで)

## 完了条件

- 10 MiB 超 (修正前はデコーダ既定の 10 MiB 上限でエラーになっていたサイズ) のレスポンスに対して `HttpWaitStrategy` がエラーにならず、1 MiB で切り詰めてマッチ判定できること (単体テスト。修正はデコーダ上限の無効化 1 点で全フレーミング (Content-Length / close-delimited / chunked) に効く。テストは Content-Length フレーミングで検証する)
- 既存の切り詰めテスト (`MAX_HTTP_RESPONSE_BODY_BYTES + 1`) が引き続き通ること

## 解決方法

- `src/core/client/http_decode.rs` の `ResponseAccumulator::new` で、`BodyLimit::Truncate(_)` に `DecoderLimits { max_body_size: u64::MAX, ..Default::default() }` を適用した (デコーダの受信総量チェックを無効化し、切り詰めは既存の `drain_body` の Truncate 分岐に任せる。`docker_log_stream` の非有界ログデコーダと同じ方針)
- `BodyLimit::Unlimited` は従来どおり `ResponseDecoder::new()` (デコーダ既定 10 MiB 上限) のままとした (Docker Engine API 経路は小さい JSON 応答のみで、受信総量の安全弁として維持)。`BodyLimit::Error` は既存の上限値揃えを維持
- `DecoderLimits::unlimited()` は使わず `max_body_size` のみ `u64::MAX` にした (`max_buffer_size` / `max_headers_count` 等のガードを維持するため)
- 受信総量ガード喪失のトレードオフ (呼び出し側の `request_timeout` と保持量上限で実質 bounded) をコメントに明記した
- テスト: `http_strategy.rs` に 10 MiB 超レスポンス (Content-Length フレーミング) の切り詰めテストを追加し、1 MiB に切り詰められて status 200 でマッチ判定できることを検証した。既存の 1 MiB 切り詰めテストも通ることを確認した
