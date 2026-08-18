//! shiguredo_container — macOS / Linux 両対応のテスト用コンテナ管理ライブラリ。
//!
//! macOS では Apple Container、Linux では Docker Engine API を利用してコンテナを管理する。
//! ユーザーは `use shiguredo_container::*` で両 OS 向けに同じ API を書ける。
//!
//! # クイックスタート
//!
//! alpine コンテナを起動してコマンドを実行し、終了後に削除する例。
//!
//! ```rust,no_run
//! use shiguredo_container::{AsyncRunner, ExecCommand, GenericImage, ImageExt};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // コンテナを起動し、`tail` で生存させ続ける
//!     let container = GenericImage::new("alpine", "latest")
//!         .with_cmd(["tail", "-f", "/dev/null"])
//!         .start()
//!         .await?;
//!
//!     // コンテナ内でコマンドを実行する
//!     let mut result = container.exec(ExecCommand::new(["echo", "hello"])).await?;
//!     assert_eq!(result.exit_code().await?, Some(0));
//!     let stdout = result.stdout_to_vec().await?;
//!     assert_eq!(String::from_utf8_lossy(&stdout).trim(), "hello");
//!
//!     // コンテナを削除する
//!     container.rm().await?;
//!     Ok(())
//! }
//! ```
//!
//! # Feature 一覧
//!
//! - `blocking`: 同期 API ([`Container`] / [`SyncRunner`]) を有効化する。
//! - `http_wait_plain`: [`WaitFor::http`] ([`core::wait::HttpWaitStrategy`]) を有効化する。plain HTTP のみ対応 (TLS 非対応)。
//! - `watchdog`: テストプロセスのクラッシュ (SIGKILL 含む) 時に孤立コンテナを掃除する。macOS のみ。
//!
//! # プラットフォーム差
//!
//! - macOS: Apple Container の XPC 経由でコンテナを管理する。pause / unpause は未対応。
//! - Linux: Docker Engine API 経由でコンテナを管理する。`with_ssh` は未対応。
//!
//! 対応範囲の詳細は `docs/TESTCONTAINERS.md` と README の WARNING を参照すること。
//!
//! # 環境変数
//!
//! - `TESTCONTAINERS_COMMAND`: `keep` を設定すると Drop 時にコンテナを削除しない (デバッグ用)。

#![warn(missing_docs)]

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
