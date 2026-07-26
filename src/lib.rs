//! shiguredo_container — macOS / Linux 両対応のテスト用コンテナ管理ライブラリ。
//!
//! macOS では Apple Container、Linux では Docker Engine API を利用してコンテナを管理する。
//!
//! ユーザーは `use shiguredo_container::*` で両 OS 向けに同じ API を書ける。
//! ただし Linux では一部 ImageExt・exec 出力等が未実装で、start 時に明示エラーになる。
//! 対応範囲は `docs/TESTCONTAINERS.md` と README の WARNING を参照すること。

pub mod core;
pub mod images;
pub mod runners;

#[cfg(all(target_os = "macos", feature = "watchdog"))]
pub(crate) mod watchdog;
#[cfg(target_os = "macos")]
mod xpc;

pub use crate::core::error::Error;
pub use crate::core::{
    ContainerAsync, ContainerRequest, ExecCommand, Healthcheck, Image, ImageExt, WaitFor,
};
pub use crate::images::GenericImage;
pub use crate::runners::AsyncRunner;

#[cfg(feature = "blocking")]
pub use crate::core::Container;

#[cfg(feature = "blocking")]
pub use crate::runners::SyncRunner;
