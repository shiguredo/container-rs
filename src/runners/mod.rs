//! `runners` — コンテナ起動のトレイト。元の 0.27 の `runners` と同一シグネチャ。

pub(crate) mod async_runner;

pub use async_runner::AsyncRunner;

#[cfg(feature = "blocking")]
pub(crate) mod sync_runner;

#[cfg(feature = "blocking")]
pub use sync_runner::SyncRunner;
