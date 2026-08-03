//! XPC 通信の低レベルラッパ。FFI バインディングとコネクション管理。
//!
//! `conn` モジュールで実装し、`XpcClient` から利用される。

pub mod conn;

pub(crate) use conn::member_opt_string;
pub(crate) use conn::{
    DEFAULT_TIMEOUT, Filters, IMAGE_SERVICE, KeyValue, LONG_TIMEOUT, SERVICE_NAME, XpcConn, id_key,
    j, k, s,
};
