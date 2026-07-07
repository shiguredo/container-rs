//! HTTP/1.1 レスポンスの sans-io デコード状態機械。
//!
//! `docker_client.rs` (同期) と `http_strategy.rs` (非同期) で共通の
//! デコードロジックを集約する。I/O は呼び出し側が行い、本モジュールは
//! バッファ給餌と状態遷移のみを担う。

use shiguredo_http11::{BodyKind, BodyProgress, ResponseDecoder};

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
    max_body_bytes: Option<usize>,
}

/// デコード完了後の中間結果。呼び出し側で用途の型に変換する。
pub(crate) struct DecodedResponse {
    pub(crate) head: shiguredo_http11::ResponseHead,
    pub(crate) body: Vec<u8>,
}

impl ResponseAccumulator {
    /// 新しいアキュムレータを作る。
    ///
    /// `max_body_bytes`: ボディの最大蓄積バイト数。`None` は無制限。
    pub(crate) fn new(method: &str, max_body_bytes: Option<usize>) -> Self {
        let mut decoder = ResponseDecoder::new();
        decoder.set_request_method(method);
        Self {
            decoder,
            request_method: method.to_string(),
            head: None,
            body_kind: None,
            body_buf: Vec::new(),
            body_done: false,
            max_body_bytes,
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
                match self.max_body_bytes {
                    Some(max) => {
                        let remaining = max.saturating_sub(self.body_buf.len());
                        self.body_buf
                            .extend_from_slice(&data[..data.len().min(remaining)]);
                    }
                    None => {
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
        let mut acc = ResponseAccumulator::new("GET", None);
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
        let mut acc = ResponseAccumulator::new("GET", None);
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
        let mut acc = ResponseAccumulator::new("GET", None);
        assert!(
            feed_all(&mut acc, raw).expect("CloseDelimited は EOF で完了すること"),
            "完了すること"
        );
        let decoded = acc.finish().expect("finish できること");
        assert_eq!(decoded.body, b"hello");
    }
}
