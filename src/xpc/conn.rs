//! XPC 接続とメッセージ送受信の低レベルラッパ。
//!
//! `xpc_bridge.c` の FFI を包み、`XpcConn::send` で XPC サービスにメッセージを送る。

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::time::Duration;

use nojson::DisplayJson;

use crate::core::error::{ClientError, Result};

#[expect(non_camel_case_types)]
type xpc_object_t = *mut c_void;
#[expect(non_camel_case_types)]
type xpc_connection_t = *mut c_void;

unsafe extern "C" {
    fn xpc_bridge_create_connection(name: *const c_char) -> xpc_connection_t;
    fn xpc_bridge_connection_cancel(conn: xpc_connection_t);
    fn xpc_bridge_create_dictionary() -> xpc_object_t;
    fn xpc_bridge_dictionary_set_string(d: xpc_object_t, k: *const c_char, v: *const c_char);
    fn xpc_bridge_dictionary_set_data(
        d: xpc_object_t,
        k: *const c_char,
        v: *const c_void,
        l: usize,
    );
    fn xpc_bridge_dictionary_set_int64(d: xpc_object_t, k: *const c_char, v: i64);
    fn xpc_bridge_dictionary_set_uint64(d: xpc_object_t, k: *const c_char, v: u64);
    fn xpc_bridge_dictionary_set_bool(d: xpc_object_t, k: *const c_char, v: bool);
    // true = 設定成功。xpc_fd_create 失敗時は false。
    fn xpc_bridge_dictionary_set_fd(d: xpc_object_t, k: *const c_char, v: i32) -> bool;
    fn xpc_bridge_release(obj: xpc_object_t);
    fn xpc_bridge_send_message_with_reply_sync(
        c: xpc_connection_t,
        m: xpc_object_t,
        timeout_ns: u64,
    ) -> xpc_object_t;
    fn xpc_bridge_copy_data(d: xpc_object_t, k: *const c_char, l: *mut usize) -> *const c_void;
    fn xpc_bridge_get_string(d: xpc_object_t, k: *const c_char) -> *const c_char;
    // キー存在かつ XPC_TYPE_INT64 のときだけ *out に書き出して true。
    fn xpc_bridge_get_int64_checked(d: xpc_object_t, k: *const c_char, out: *mut i64) -> bool;
    fn xpc_bridge_is_error(obj: xpc_object_t) -> bool;
    fn xpc_bridge_get_error_description(obj: xpc_object_t, b: *mut c_char, bl: usize) -> bool;
    fn xpc_bridge_get_log_fds(reply: xpc_object_t, fds: *mut i32, max_fds: usize) -> i32;
}

/// XPC のルートキー・エラーキー・サービス名。
const ROUTE_KEY: &CStr = c"com.apple.container.xpc.route";
const ERROR_KEY: &CStr = c"com.apple.container.xpc.error";
pub(crate) const SERVICE_NAME: &CStr = c"com.apple.container.apiserver";
pub(crate) const IMAGE_SERVICE: &CStr = c"com.apple.container.core.container-core-images";

/// 通常の XPC 呼び出しのタイムアウト。
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// `containerWait` など、プロセス終了を待つ呼び出しのタイムアウト。
pub(crate) const LONG_TIMEOUT: Duration = Duration::from_secs(86400);

/// XPC の応答を包むラッパ。Drop で `xpc_bridge_release` する。
pub(crate) struct RawReply {
    obj: xpc_object_t,
}

// `xpc_object_t` は `*mut c_void` だが、XPC オブジェクトはスレッドセーフに解放できる。
unsafe impl Send for RawReply {}

impl RawReply {
    /// 指定キーのデータを `Vec<u8>` で取り出す。
    pub(crate) fn data(&self, key: &CStr) -> Option<Vec<u8>> {
        unsafe {
            let mut len = 0;
            let p = xpc_bridge_copy_data(self.obj, key.as_ptr(), &mut len);
            if p.is_null() || len == 0 {
                return None;
            }
            let v = std::slice::from_raw_parts(p as *const u8, len).to_vec();
            libc::free(p as *mut c_void);
            Some(v)
        }
    }

    /// 指定キーの文字列を `String` で取り出す。
    pub(crate) fn string(&self, key: &CStr) -> Option<String> {
        unsafe {
            let p = xpc_bridge_get_string(self.obj, key.as_ptr());
            if p.is_null() {
                return None;
            }
            CStr::from_ptr(p).to_str().ok().map(|s| s.to_string())
        }
    }

    /// 指定キーの `int64` を取り出す。キー欠落・型不一致はエラー。
    pub(crate) fn try_int64(&self, key: &CStr) -> Result<i64> {
        let mut out = 0i64;
        let ok = unsafe { xpc_bridge_get_int64_checked(self.obj, key.as_ptr(), &mut out) };
        if ok {
            Ok(out)
        } else {
            Err(ClientError::Xpc(format!(
                "missing or non-int64 key: {}",
                key.to_string_lossy()
            ))
            .into())
        }
    }

    /// `"logs"` 配列から FD を複製して取り出す。
    pub(crate) fn log_fds(&self) -> Vec<std::os::fd::RawFd> {
        let mut fds = [0i32; 2];
        let n = unsafe { xpc_bridge_get_log_fds(self.obj, fds.as_mut_ptr(), fds.len()) };
        if n <= 0 {
            return Vec::new();
        }
        fds[..n as usize].to_vec()
    }

    /// 応答が XPC エラーオブジェクトかどうか。
    pub(crate) fn is_error(&self) -> bool {
        unsafe { xpc_bridge_is_error(self.obj) }
    }

    /// エラー説明を取り出す。
    pub(crate) fn error_desc(&self) -> Option<String> {
        let mut buf = vec![0u8; 1024];
        if unsafe {
            xpc_bridge_get_error_description(self.obj, buf.as_mut_ptr() as *mut c_char, buf.len())
        } {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            Some(String::from_utf8_lossy(&buf[..end]).into_owned())
        } else {
            None
        }
    }

    /// `com.apple.container.xpc.error` キーのデータを JSON としてパースし、
    /// `code` / `message` を取り出して `Xpc` エラーを作る。
    pub(crate) fn json_error(&self) -> Option<ClientError> {
        let ek = ERROR_KEY;
        self.data(ek).map(|d| parse_xpc_error_response(&d))
    }
}

/// XPC エラー応答のバイト列から `code` / `message` を取り出して `Xpc` エラーを作る。
///
/// - 非 UTF-8 → `"XPC error unparseable (response is not UTF-8)"`
/// - JSON パース失敗 → `"XPC error unparseable"`
/// - パース成功 → `"XPC error {code}: {message}"`
fn parse_xpc_error_response(data: &[u8]) -> ClientError {
    let Ok(text) = std::str::from_utf8(data) else {
        return ClientError::Xpc("XPC error unparseable (response is not UTF-8)".into());
    };
    let Ok(j) = nojson::RawJson::parse(text) else {
        return ClientError::Xpc("XPC error unparseable".into());
    };
    let code = j
        .value()
        .to_member("code")
        .and_then(|m| m.required())
        .and_then(|v| {
            let s: String = v.try_into()?;
            Ok(s)
        })
        .unwrap_or_default();
    let msg = j
        .value()
        .to_member("message")
        .and_then(|m| m.required())
        .and_then(|v| {
            let s: String = v.try_into()?;
            Ok(s)
        })
        .unwrap_or_default();
    ClientError::Xpc(format!("XPC error {code}: {msg}"))
}

impl Drop for RawReply {
    fn drop(&mut self) {
        unsafe {
            xpc_bridge_release(self.obj);
        }
    }
}

/// XPC 接続。Drop で `xpc_connection_cancel` する。
pub(crate) struct XpcConn {
    conn: xpc_connection_t,
}

impl XpcConn {
    /// 指定サービスに接続する。
    pub(crate) fn connect(service: &CStr) -> Result<XpcConn> {
        let conn = unsafe { xpc_bridge_create_connection(service.as_ptr()) };
        if conn.is_null() {
            return Err(ClientError::XpcConnect.into());
        }
        Ok(XpcConn { conn })
    }

    /// メッセージを送り、同期で応答を受け取る。
    ///
    /// `route` は XPC のルート名（例: `"containerCreate"`）。`entries` はキーと値のリスト。
    /// タイムアウトは `DEFAULT_TIMEOUT` (60 秒)。
    pub(crate) fn send(&self, route: &str, entries: &[(CString, KeyValue)]) -> Result<RawReply> {
        self.send_with_timeout(route, entries, DEFAULT_TIMEOUT)
    }

    /// メッセージを送り、同期で応答を受け取る。タイムアウトを指定できる版。
    pub(crate) fn send_with_timeout(
        &self,
        route: &str,
        entries: &[(CString, KeyValue)],
        timeout: Duration,
    ) -> Result<RawReply> {
        // 失敗しうる CString 変換は XPC 辞書を作る前に全部済ませる。
        // 辞書作成後に `?` で early return すると辞書 (と dup 済み fd) がリークするため。
        let rv = CString::new(route).map_err(|_| ClientError::Xpc("route contains NUL".into()))?;
        let mut cstrings = Vec::new();
        for (_, val) in entries {
            if let KeyValue::String(s) = val {
                cstrings.push(
                    CString::new(s.as_str())
                        .map_err(|_| ClientError::Xpc("string value contains NUL".into()))?,
                );
            }
        }
        let mut cstrings = cstrings.into_iter();

        let msg = unsafe { xpc_bridge_create_dictionary() };
        if msg.is_null() {
            // メモリ枯渇等で辞書作成が失敗した場合は NULL が返る。そのまま C 関数に
            // 渡すとクラッシュするため、最初の C 関数呼び出しの前にエラーで返す。
            // 失敗時は解放対象 (msg) が無いため `xpc_bridge_release` は呼ばない。
            return Err(ClientError::Xpc("failed to create XPC dictionary".into()).into());
        }
        let rk = ROUTE_KEY;
        unsafe {
            xpc_bridge_dictionary_set_string(msg, rk.as_ptr(), rv.as_ptr());
        }
        for (key, val) in entries {
            match val {
                KeyValue::String(_) => {
                    // cstrings は KeyValue::String と同順で積んであるため必ず存在する。
                    let c = cstrings.next().expect("cstring must exist");
                    unsafe {
                        xpc_bridge_dictionary_set_string(msg, key.as_ptr(), c.as_ptr());
                    }
                }
                KeyValue::Data(d) => unsafe {
                    xpc_bridge_dictionary_set_data(
                        msg,
                        key.as_ptr(),
                        d.as_ptr() as *const c_void,
                        d.len(),
                    );
                },
                KeyValue::Bool(b) => unsafe {
                    xpc_bridge_dictionary_set_bool(msg, key.as_ptr(), *b);
                },
                KeyValue::Int64(v) => unsafe {
                    xpc_bridge_dictionary_set_int64(msg, key.as_ptr(), *v);
                },
                KeyValue::UInt64(v) => unsafe {
                    xpc_bridge_dictionary_set_uint64(msg, key.as_ptr(), *v);
                },
                KeyValue::Fd(v) => {
                    let ok = unsafe { xpc_bridge_dictionary_set_fd(msg, key.as_ptr(), *v) };
                    if !ok {
                        // 途中失敗時も作成済み msg を解放してから Err を返す。
                        unsafe {
                            xpc_bridge_release(msg);
                        }
                        return Err(ClientError::Xpc(
                            "failed to set file descriptor in XPC dictionary".into(),
                        )
                        .into());
                    }
                }
            }
        }
        // 巨大な Duration で切り詰めないよう飽和させる。C 側でも INT64_MAX にクランプする。
        let timeout_ns = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
        let raw = unsafe { xpc_bridge_send_message_with_reply_sync(self.conn, msg, timeout_ns) };
        unsafe {
            xpc_bridge_release(msg);
        }
        if raw.is_null() {
            return Err(ClientError::XpcTimeout.into());
        }
        let reply = RawReply { obj: raw };
        if reply.is_error() {
            return Err(ClientError::Xpc(reply.error_desc().unwrap_or_default()).into());
        }
        if let Some(e) = reply.json_error() {
            return Err(e.into());
        }
        Ok(reply)
    }
}

impl Drop for XpcConn {
    fn drop(&mut self) {
        unsafe {
            xpc_bridge_connection_cancel(self.conn);
            // `xpc_connection_create_mach_service` は +1 参照を返すため、
            // cancel だけでは接続オブジェクトが解放されずリークする。
            xpc_bridge_release(self.conn);
        }
    }
}

/// XPC メッセージの値。
pub(crate) enum KeyValue {
    String(String),
    Data(Vec<u8>),
    Bool(bool),
    Int64(i64),
    UInt64(u64),
    Fd(std::os::fd::RawFd),
}

/// キー名から `CString` を作る。
pub(crate) fn k(name: &'static str) -> CString {
    CString::new(name).expect("key must not contain NUL")
}

/// `id` キーの `CString`。頻繁に使うので専用ヘルパ。
pub(crate) fn id_key() -> CString {
    k("id")
}

/// 文字列から `KeyValue::String` を作る。
pub(crate) fn s(v: &str) -> KeyValue {
    KeyValue::String(v.to_string())
}

/// `DisplayJson` を JSON バイト列にする。
pub(crate) fn j(v: &impl DisplayJson) -> Vec<u8> {
    nojson::Json(v).to_string().into_bytes()
}

/// `nojson::RawJsonValue` からメンバーを文字列で取り出す（オプション）。
pub(crate) fn member_opt_string(item: &nojson::RawJsonValue, key: &str) -> Option<String> {
    item.to_member(key)
        .ok()
        .and_then(|m| m.required().ok())
        .and_then(|v| TryInto::<String>::try_into(v).ok())
}

/// フィルタ用の JSON を構築するヘルパ（`containerList` / `volumeList` / `networkList` 用）。
pub(crate) struct Filters {
    pub(crate) ids: Vec<String>,
    // Apple container の XPC デコーダは labels キーを必須として要求する (空でも送る)。
    pub(crate) labels: HashMap<String, String>,
}

impl DisplayJson for Filters {
    fn fmt(&self, f: &mut nojson::JsonFormatter) -> std::fmt::Result {
        f.object(|f| {
            f.member("ids", &self.ids)?;
            f.member("labels", &self.labels)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用に空の XPC 辞書を RawReply で包む。Drop で release される。
    fn empty_reply() -> RawReply {
        let obj = unsafe { xpc_bridge_create_dictionary() };
        assert!(!obj.is_null(), "辞書の作成に失敗しないこと");
        RawReply { obj }
    }

    #[test]
    fn try_int64_returns_value_when_key_is_int64() {
        // XPC_TYPE_INT64 のキーがあるとき値を返すこと。
        let reply = empty_reply();
        let key = CString::new("exitCode").expect("キーの作成に失敗しないこと");
        unsafe {
            xpc_bridge_dictionary_set_int64(reply.obj, key.as_ptr(), 42);
        }
        let v = reply.try_int64(&key).expect("int64 を取得できること");
        assert_eq!(v, 42, "設定した int64 が返ること");
    }

    #[test]
    fn try_int64_errors_when_key_missing() {
        // キー欠落はエラーになること (旧 int64 は 0 を返していた)。
        let reply = empty_reply();
        let key = CString::new("exitCode").expect("キーの作成に失敗しないこと");
        let err = reply
            .try_int64(&key)
            .expect_err("キー欠落はエラーであること");
        let msg = err.to_string();
        assert!(
            msg.contains("missing or non-int64"),
            "欠落を示すメッセージであること: {msg}"
        );
    }

    #[test]
    fn try_int64_errors_when_key_is_string() {
        // 型が string のときはエラーになること。
        let reply = empty_reply();
        let key = CString::new("exitCode").expect("キーの作成に失敗しないこと");
        let val = CString::new("整数でない値").expect("値の作成に失敗しないこと");
        unsafe {
            xpc_bridge_dictionary_set_string(reply.obj, key.as_ptr(), val.as_ptr());
        }
        let err = reply
            .try_int64(&key)
            .expect_err("型不一致はエラーであること");
        let msg = err.to_string();
        assert!(
            msg.contains("missing or non-int64"),
            "型不一致を示すメッセージであること: {msg}"
        );
    }

    #[test]
    fn parse_xpc_error_response_rejects_non_utf8() {
        // 非 UTF-8 のエラー応答は JSON パースエラーではなく UTF-8 失敗として
        // 明示されること (誤診断を防ぐ)。
        let err = parse_xpc_error_response(&[0xff, 0xfe]);
        assert!(
            err.to_string().contains("response is not UTF-8"),
            "UTF-8 失敗であることが分かること: {err}"
        );
    }

    #[test]
    fn parse_xpc_error_response_returns_unparseable_on_invalid_json() {
        // 非 UTF-8 ではなく JSON 構文自体が壊れている場合は従来どおり
        // `unparseable` になること (非 UTF-8 分岐と区別される)。
        let err = parse_xpc_error_response(b"this is not json");
        assert_eq!(err.to_string(), "XPC error: XPC error unparseable");
    }

    #[test]
    fn parse_xpc_error_response_extracts_code_and_message() {
        // 正常な JSON エラー応答から code / message を取り出すこと。
        // ClientError::Xpc の Display は "XPC error: " プレフィクスを付ける。
        let err =
            parse_xpc_error_response(br#"{"code":"notFound","message":"container is not found"}"#);
        assert_eq!(
            err.to_string(),
            "XPC error: XPC error notFound: container is not found"
        );
    }
}
