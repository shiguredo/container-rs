//! ログコンシューマ。元の 0.27 の `core::logs::consumer` と同一シグネチャ。

pub(crate) mod logging_consumer;

pub use logging_consumer::LoggingConsumer;

use std::future::Future;
use std::pin::Pin;

use crate::core::logs::LogFrame;

/// ログフレームを消費するトレイト。
/// コンテナのライフサイクル全期間で各ログフレームについて呼ばれる。
///
/// 本クレートは行単位 (改行除去後の `Vec<u8>`) で配信する。
/// 本家 testcontainers-rs 0.27 のチャンク単位配信とは異なる。
///
/// # 制約
///
/// `blocking` feature 使用時、コールバック内で同期 API (`SyncRunner::start` 等) を
/// 呼び出すと共有 Runtime への再入によって deadlock する。実装側は再入を検出して
/// 即座にエラーにする (fail-fast) が、コールバック内では同期 API の呼び出しを避けること。
///
/// コールバック内で `Container::stdout` / `stderr` (同期ログリーダー) を取得して読む
/// 場合も同様に再入検出が働き、読み取り時点で `io::Error` を返す。コールバック外で
/// 取得したリーダーをコールバック内で読むケースは検出されない点に注意すること。
pub trait LogConsumer: Send + Sync {
    /// ログフレームを 1 件受け取って処理する。
    fn accept<'a>(&'a self, record: &'a LogFrame) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

impl<F> LogConsumer for F
where
    F: Fn(&LogFrame) + Send + Sync,
{
    fn accept<'a>(&'a self, record: &'a LogFrame) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self(record);
        })
    }
}
