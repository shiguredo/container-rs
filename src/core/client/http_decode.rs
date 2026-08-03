//! HTTP/1.1 レスポンスの sans-io デコード状態機械。
//!
//! `docker_client.rs` (同期) と `http_strategy.rs` (非同期) で共通の
//! デコードロジックを集約する。I/O は呼び出し側が行い、本モジュールは
//! バッファ給餌と状態遷移のみを担う。

use shiguredo_http11::{BodyKind, BodyProgress, DecoderLimits, ResponseDecoder};

/// レスポンスボディの蓄積上限と超過時の挙動。
///
/// 既存の切り詰め (`Truncate`) は HTTP 待機戦略の 1 MiB 上限で使い続ける。
/// `Error` は exec 出力など超過時に切り詰めるとデータが欠損する経路専用で、
/// 上限を超えた時点で即座にエラーを返す (早期アボート)。
// macOS では `Unlimited` / `Error` を構築するのは Linux 側の Docker クライアントと
// 単体テストのみ。また Linux の既定ビルドでは `Truncate` を構築する HTTP 待機戦略が
// `http_wait_plain` feature 配下のため、いずれも lib ビルドでは未使用になり得る
// (単体テストでは使う)
#[cfg_attr(
    all(not(test), any(target_os = "macos", not(feature = "http_wait_plain"))),
    expect(dead_code)
)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum BodyLimit {
    /// 上限なし。
    Unlimited,
    /// 上限超過分を切り詰めて続行する。判定は `>` (ちょうど上限は成功)。
    Truncate(usize),
    /// 上限超過時に即座にエラーを返す。判定は `>` (ちょうど上限は成功)。
    Error(usize),
}

/// HTTP/1.1 レスポンスのデコード状態を管理する。
///
/// 呼び出し側は I/O で読み込んだバイト数を `feed` に渡し、
/// 完了判定とバッファ取得を繰り返す。
pub(crate) struct ResponseAccumulator {
    decoder: ResponseDecoder,
    /// 1xx スキップ後に decoder が method を消すため、再設定用に保持する。
    request_method: String,
    head: Option<shiguredo_http11::ResponseHead>,
    body_kind: Option<BodyKind>,
    body_buf: Vec<u8>,
    body_done: bool,
    body_limit: BodyLimit,
}

/// デコード完了後の中間結果。呼び出し側で用途の型に変換する。
pub(crate) struct DecodedResponse {
    pub(crate) head: shiguredo_http11::ResponseHead,
    pub(crate) body: Vec<u8>,
}

impl ResponseAccumulator {
    /// 新しいアキュムレータを作る。
    ///
    /// `body_limit`: ボディの蓄積上限と超過時の挙動 (詳細は [`BodyLimit`] 参照)。
    pub(crate) fn new(method: &str, body_limit: BodyLimit) -> Self {
        // exec 経路 (`BodyLimit::Error`) はデコーダの既定上限 (10 MiB) が先に発動すると
        // 指定した上限が実効しないため、デコーダの `max_body_size` を上限値に揃える。
        // 超過時の文言は、exec の実経路である close-delimited では `drain_body` 側の判定が
        // `consume_body` より先に走るため `output exceeds ... bytes limit` で一貫する。
        // Content-Length ヘッダで宣言された上限超過はヘッダ解析時点でデコーダ側の
        // `body too large` が先に返るため、文言はフレーミング依存になる
        // (copy_from / pull は Content-Length フレーミングで返るため `body too large` になり得る)。
        let mut decoder = match body_limit {
            BodyLimit::Error(max) => ResponseDecoder::with_limits(DecoderLimits {
                max_body_size: max as u64,
                ..Default::default()
            }),
            _ => ResponseDecoder::new(),
        };
        decoder.set_request_method(method);
        Self {
            decoder,
            request_method: method.to_string(),
            head: None,
            body_kind: None,
            body_buf: Vec::new(),
            body_done: false,
            body_limit,
        }
    }

    /// 次に読み込めるバッファサイズを返す。0 ならバッファフル。
    pub(crate) fn read_buf_size(&self) -> usize {
        self.decoder.available_buf().min(8192)
    }

    /// 書き込み可能なバッファを取得する。
    pub(crate) fn mut_buf(&mut self, want: usize) -> Result<&mut [u8], String> {
        self.decoder.mut_buf(want).map_err(|e| e.to_string())
    }

    /// `n` バイト読み込んだことを通知し、デコードを進める。
    ///
    /// 完了 (最終応答のボディまで読み切り) したら `true` を返す。
    /// `n == 0` は EOF を意味する。
    ///
    /// 1xx 中間応答は破棄して最終応答を待つ (RFC 9110)。
    /// Content-Length / chunked が未完了のまま EOF になった場合はエラー。
    pub(crate) fn feed(&mut self, n: usize) -> Result<bool, String> {
        if n == 0 {
            self.decoder.advance_buf(0);
            self.decoder.mark_eof();
        } else {
            self.decoder.advance_buf(n);
        }

        loop {
            if self.head.is_none() {
                match self.decoder.decode_headers().map_err(|e| e.to_string())? {
                    Some((h, bk)) => {
                        // 1xx は最終応答ではないため破棄し、次メッセージへ進む。
                        // ResponseDecoder は BodyKind::None で Complete になり、
                        // 再 decode_headers で次の start-line を読める。
                        let code = h.status_code();
                        if (100..200).contains(&code) {
                            self.head = None;
                            self.body_kind = None;
                            self.body_buf.clear();
                            self.body_done = false;
                            // Complete 遷移で decoder 側の method が消えるため戻す。
                            self.decoder.set_request_method(&self.request_method);
                            continue;
                        }
                        self.head = Some(h);
                        self.body_kind = Some(bk);
                    }
                    None if n == 0 => {
                        return Err("connection closed before headers complete".into());
                    }
                    None => return Ok(false),
                }
            }

            match self.body_kind {
                Some(BodyKind::None | BodyKind::Tunnel) => return Ok(true),
                Some(BodyKind::CloseDelimited) => {
                    self.drain_body()?;
                    if self.body_done {
                        return Ok(true);
                    }
                    if n == 0 {
                        // CloseDelimited は EOF が終端。
                        self.body_done = true;
                        return Ok(true);
                    }
                    return Ok(false);
                }
                Some(BodyKind::ContentLength(_) | BodyKind::Chunked) => {
                    self.drain_body()?;
                    if self.body_done {
                        return Ok(true);
                    }
                    if n == 0 {
                        return Err("connection closed before body complete".into());
                    }
                    return Ok(false);
                }
                None => return Err("missing body kind after headers".into()),
                // BodyKind は non_exhaustive のため将来 variant 用。
                Some(_) => return Err("unsupported body kind".into()),
            }
        }
    }

    fn drain_body(&mut self) -> Result<(), String> {
        loop {
            if let Some(data) = self.decoder.peek_body() {
                let len = data.len();
                match self.body_limit {
                    BodyLimit::Unlimited => {
                        self.body_buf.extend_from_slice(data);
                    }
                    BodyLimit::Truncate(max) => {
                        let remaining = max.saturating_sub(self.body_buf.len());
                        self.body_buf
                            .extend_from_slice(&data[..data.len().min(remaining)]);
                    }
                    BodyLimit::Error(max) => {
                        // 超過判定は macOS の read_file_to_vec と同じ `>` 境界。
                        // 蓄積が上限を超えた時点で即座にエラーを返す (早期アボート)。
                        // 切り詰めて続行すると multiplexed stream がフレーム途中で
                        // 切断され、出力が欠損するためエラーにする。
                        if self.body_buf.len().saturating_add(len) > max {
                            return Err(format!("output exceeds {max} bytes limit"));
                        }
                        self.body_buf.extend_from_slice(data);
                    }
                }
                match self.decoder.consume_body(len).map_err(|e| e.to_string())? {
                    BodyProgress::Complete { .. } => {
                        self.body_done = true;
                        return Ok(());
                    }
                    BodyProgress::Advanced | BodyProgress::NeedData => continue,
                }
            }
            match self.decoder.progress().map_err(|e| e.to_string())? {
                BodyProgress::Complete { .. } => {
                    self.body_done = true;
                    return Ok(());
                }
                BodyProgress::Advanced => continue,
                BodyProgress::NeedData => return Ok(()),
            }
        }
    }

    /// デコード結果を取り出す。`feed` が `true` を返した後に呼ぶ。
    pub(crate) fn finish(self) -> Result<DecodedResponse, String> {
        let head = self.head.ok_or("no response head")?;
        Ok(DecodedResponse {
            head,
            body: self.body_buf,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(acc: &mut ResponseAccumulator, bytes: &[u8]) -> Result<bool, String> {
        let mut offset = 0;
        loop {
            let want = acc.read_buf_size();
            assert!(want > 0, "デコーダバッファが埋まっている");
            let n = (bytes.len() - offset).min(want);
            if n == 0 {
                return acc.feed(0);
            }
            let buf = acc.mut_buf(n)?;
            buf[..n].copy_from_slice(&bytes[offset..offset + n]);
            offset += n;
            if acc.feed(n)? {
                return Ok(true);
            }
        }
    }

    #[test]
    fn incomplete_content_length_body_at_eof_is_error() {
        // Content-Length: 5 なのにボディが "hi" で EOF。
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhi";
        let mut acc = ResponseAccumulator::new("GET", BodyLimit::Unlimited);
        let err = feed_all(&mut acc, raw).expect_err("未完了ボディの EOF はエラーであること");
        assert!(
            err.contains("before body complete"),
            "エラー文言に body complete を含むこと: {err}"
        );
    }

    #[test]
    fn informational_1xx_is_skipped_for_final_response() {
        // 103 のあと 200 が続く。最終は 200。
        let raw = b"HTTP/1.1 103 Early Hints\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
        let mut acc = ResponseAccumulator::new("GET", BodyLimit::Unlimited);
        assert!(
            feed_all(&mut acc, raw).expect("1xx スキップ後に最終応答を読めること"),
            "最終応答まで完了すること"
        );
        let decoded = acc.finish().expect("finish できること");
        assert_eq!(decoded.head.status_code(), 200);
        assert_eq!(decoded.body, b"ok");
    }

    #[test]
    fn close_delimited_accepts_eof_as_complete() {
        let raw = b"HTTP/1.0 200 OK\r\n\r\nhello";
        let mut acc = ResponseAccumulator::new("GET", BodyLimit::Unlimited);
        assert!(
            feed_all(&mut acc, raw).expect("CloseDelimited は EOF で完了すること"),
            "完了すること"
        );
        let decoded = acc.finish().expect("finish できること");
        assert_eq!(decoded.body, b"hello");
    }

    /// `Content-Length` とボディから raw レスポンスを組み立てる。
    fn content_length_response(body: &[u8]) -> Vec<u8> {
        let mut raw =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        raw.extend_from_slice(body);
        raw
    }

    /// `Content-Length` 無しの close-delimited レスポンス (exec start と同じフレーミング) を組み立てる。
    fn close_delimited_response(body: &[u8]) -> Vec<u8> {
        let mut raw = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        raw.extend_from_slice(body);
        raw
    }

    /// チャンクサイズを抑えて給餌する。`feed_all` はデコーダバッファの許す限り一度に
    /// 給餌するため、複数回の `feed` にまたがる蓄積判定の検証には使えない。
    fn feed_chunked(
        acc: &mut ResponseAccumulator,
        bytes: &[u8],
        chunk_size: usize,
    ) -> Result<bool, String> {
        let mut offset = 0;
        loop {
            let want = acc.read_buf_size();
            assert!(want > 0, "デコーダバッファが埋まっている");
            let n = (bytes.len() - offset).min(want).min(chunk_size);
            if n == 0 {
                return acc.feed(0);
            }
            let buf = acc.mut_buf(n)?;
            buf[..n].copy_from_slice(&bytes[offset..offset + n]);
            offset += n;
            if acc.feed(n)? {
                return Ok(true);
            }
        }
    }

    #[test]
    fn body_limit_error_accepts_exactly_at_limit() {
        // ちょうど上限 (判定は `>`) は成功すること。close-delimited (exec start と同型)。
        let body = b"hello";
        let raw = close_delimited_response(body);
        let mut acc = ResponseAccumulator::new("GET", BodyLimit::Error(5));
        assert!(
            feed_all(&mut acc, &raw).expect("ちょうど上限は成功であること"),
            "ボディが読み切れること"
        );
        let decoded = acc.finish().expect("finish できること");
        assert_eq!(decoded.body, body);
    }

    #[test]
    fn body_limit_error_rejects_one_over_limit() {
        // 上限 + 1 はエラーになること。エラー文言はデコーダの BodyTooLarge ではなく
        // BodyLimit::Error 側の文言 (output exceeds ... bytes limit) が出ること。
        let raw = close_delimited_response(b"helloo");
        let mut acc = ResponseAccumulator::new("GET", BodyLimit::Error(5));
        let err = feed_all(&mut acc, &raw).expect_err("上限 + 1 はエラーであること");
        assert!(
            err.contains("output exceeds 5 bytes limit"),
            "エラー文言に上限値と bytes limit を含むこと: {err}"
        );
    }

    #[test]
    fn body_limit_error_rejects_over_limit_with_multiple_chunks() {
        // チャンクに分割して給餌しても、蓄積が上限を超えた時点でエラーになること。
        // 早期アボートの核心は「複数回の feed にまたがる蓄積量の判定」のため、
        // feed_chunked で 3 バイトずつ給餌して跨ぎ判定を検証する。
        let raw = close_delimited_response(&[b'x'; 12]);
        let mut acc = ResponseAccumulator::new("GET", BodyLimit::Error(10));
        let err = feed_chunked(&mut acc, &raw, 3).expect_err("上限超過はエラーであること");
        assert!(
            err.contains("output exceeds 10 bytes limit"),
            "エラー文言に上限値を含むこと: {err}"
        );
    }

    #[test]
    fn body_limit_error_accepts_exactly_at_limit_across_chunks() {
        // 複数回の feed にまたがって累積がちょうど上限に達した場合も成功すること。
        let raw = close_delimited_response(&[b'x'; 9]);
        let mut acc = ResponseAccumulator::new("GET", BodyLimit::Error(9));
        assert!(
            feed_chunked(&mut acc, &raw, 3).expect("ちょうど上限は成功であること"),
            "ボディが読み切れること"
        );
        let decoded = acc.finish().expect("finish できること");
        assert_eq!(decoded.body.len(), 9);
    }

    #[test]
    fn body_limit_error_content_length_over_limit_is_rejected_at_header() {
        // Content-Length 宣言が上限を超える場合、ヘッダ解析時点でデコーダ側の
        // BodyTooLarge が先に返る (close-delimited の `output exceeds ...` とは文言が異なる)。
        let raw = content_length_response(b"helloo");
        let mut acc = ResponseAccumulator::new("GET", BodyLimit::Error(5));
        let err = feed_all(&mut acc, &raw).expect_err("Content-Length 上限超過はエラーであること");
        assert!(
            err.contains("body too large"),
            "デコーダ側の BodyTooLarge 文言が出ること: {err}"
        );
    }

    #[test]
    fn body_limit_error_accepts_no_content_body() {
        // ボディ無し応答 (204 No Content の BodyKind::None) と Error モードの組合せでも成功すること。
        let mut acc = ResponseAccumulator::new("GET", BodyLimit::Error(5));
        assert!(
            feed_all(&mut acc, b"HTTP/1.1 204 No Content\r\n\r\n").expect("204 は成功であること"),
            "BodyKind::None が読み切れること"
        );
        let decoded = acc.finish().expect("finish できること");
        assert!(decoded.body.is_empty());
    }

    #[test]
    fn body_limit_truncate_still_truncates_after_limit() {
        // 既存の切り詰めモードは上限超過後も続行し、上限分だけ保持すること。
        let raw = content_length_response(b"hello world");
        let mut acc = ResponseAccumulator::new("GET", BodyLimit::Truncate(5));
        assert!(
            feed_all(&mut acc, &raw).expect("切り詰めモードは続行できること"),
            "ボディが読み切れること"
        );
        let decoded = acc.finish().expect("finish できること");
        assert_eq!(decoded.body, b"hello");
    }
}
