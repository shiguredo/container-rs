# バグ: HttpWaitStrategy の「1 MiB で切り詰め」契約が 10 MiB 超の応答でエラーに化ける

- Created: 2026-08-04
- Completed: {YYYY-MM-DD}
- Branch: feature/fix-http-wait-truncate-limit
- Polished: {YYYY-MM-DD}

## 目的

`HttpWaitStrategy` のレスポンスボディが「1 MiB で切り詰めて続行」する契約になっているのに、10 MiB 超の応答でデコーダ側の上限によりエラーになる乖離を修正する。

## 現状

- `src/core/wait/http_strategy.rs` の `send_request_inner` は `ResponseAccumulator::new(..., BodyLimit::Truncate(MAX_HTTP_RESPONSE_BODY_BYTES))` (1 MiB) でレスポンスを蓄積する
- 一方 `src/core/client/http_decode.rs` の `ResponseAccumulator::new` は、`BodyLimit::Truncate` の場合 `ResponseDecoder::new()` (デコーダ既定の `max_body_size` = 10 MiB、`shiguredo_http11` の `limits.rs` で確認) を使う
- そのため 10 MiB 超のレスポンスは「切り詰めて続行」ではなく、Content-Length 宣言の超過はヘッダ解析時点で、chunked / close-delimited は蓄積超過でエラーになる
- `HttpResponse::body()` の rustdoc は「保持量は最大 1 MiB であり、それを超えた部分は切り詰められる」と契約しており、実挙動と乖離している (1 MiB 〜 10 MiB の間は切り詰めが効くため、10 MiB 超でのみ顕在化する)

## 設計方針

- `BodyLimit::Truncate(max)` のときも `DecoderLimits { max_body_size: max, .. }` を適用し、デコーダ側の上限を切り詰め上限と同じ 1 MiB に揃える (`BodyLimit::Error` と同じ揃え方)
- これにより 1 MiB 超の応答が確実に切り詰められ、契約が守られる

## 完了条件

- 10 MiB 超のレスポンスに対して `HttpWaitStrategy` がエラーにならず、1 MiB で切り詰めてマッチ判定できること (単体テスト)
- 既存の切り詰めテスト (`MAX_HTTP_RESPONSE_BODY_BYTES + 1`) が引き続き通ること

## 解決方法

- `src/core/client/http_decode.rs` の `ResponseAccumulator::new` で、`BodyLimit::Truncate(max)` にも `DecoderLimits { max_body_size: max as u64, .. }` を適用する
- `src/core/wait/http_strategy.rs` の単体テストに 10 MiB 超レスポンスの切り詰めケースを追加する
