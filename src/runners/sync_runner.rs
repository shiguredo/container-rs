//! `SyncRunner` — 同期 API のランナー。
//! 元の 0.27 の `runners::sync_runner` に相当。

use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::{
    Container, ContainerRequest, Image,
    core::{
        containers::sync_container::{block_on_runtime, drop_shared_runtime},
        error::{Error, Result},
    },
    runners::AsyncRunner,
};

static ASYNC_RUNTIME: OnceLock<Mutex<Weak<tokio::runtime::Runtime>>> = OnceLock::new();

/// 同期ランナー。
pub trait SyncRunner<I: Image> {
    /// コンテナを起動し `Container` を返す。
    fn start(self) -> Result<Container<I>>;
    /// イメージをプルする。
    fn pull_image(self) -> Result<ContainerRequest<I>>;
}

impl<T, I> SyncRunner<I> for T
where
    T: Into<ContainerRequest<I>> + Send,
    I: Image,
{
    fn start(self) -> Result<Container<I>> {
        let runtime = shared_runtime()?;
        match block_on_runtime(&runtime, AsyncRunner::start(self)) {
            Ok(Ok(async_container)) => Ok(Container::new(runtime, async_container)),
            Ok(Err(e)) | Err(e) => {
                // ここで捨てる `Arc` が最後の強参照だと、async コンテキスト内では
                // Runtime の drop が panic するためヘルパー経由で drop する。
                drop_shared_runtime(runtime);
                Err(e)
            }
        }
    }

    fn pull_image(self) -> Result<ContainerRequest<I>> {
        let runtime = shared_runtime()?;
        let result = block_on_runtime(&runtime, AsyncRunner::pull_image(self));
        // 生存中の `Container` が無い場合はこの `Arc` が最後の強参照であり、
        // async コンテキスト内では Runtime の drop が panic するためヘルパー経由で drop する。
        drop_shared_runtime(runtime);
        match result {
            Ok(Ok(req)) => Ok(req),
            Ok(Err(e)) | Err(e) => Err(e),
        }
    }
}

fn shared_runtime() -> Result<Arc<tokio::runtime::Runtime>> {
    let mut guard = ASYNC_RUNTIME
        .get_or_init(|| Mutex::new(Weak::new()))
        .lock()
        .map_err(|e| Error::other(format!("sync-runner runtime mutex poisoned: {e}")))?;

    match guard.upgrade() {
        Some(rt) => Ok(rt),
        None => {
            // current_thread ではなくワーカースレッド付きのランタイムにする。
            // current_thread だと LogConsumer 配信などのバックグラウンドタスクが
            // 誰かが block_on している間しか進まず、放置中はログが一切届かなかった。
            let rt = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                    .map_err(Error::other)?,
            );
            *guard = Arc::downgrade(&rt);
            Ok(rt)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::runtime::Runtime;

    use super::shared_runtime;

    /// 同一バイナリ内で `ASYNC_RUNTIME` に触るテストをこれ以外に追加しないこと。
    /// プロセスグローバルな static であるため、並列実行や他のテストからの干渉で
    /// flaky になる恐れがある。
    #[test]
    fn shared_runtime_shares_runtime_then_rebuilds_after_drop() {
        // (1) 2 回呼んで同一 Runtime が共有されること。
        let first: Arc<Runtime> = shared_runtime().expect("1 回目の Runtime 構築に失敗した");
        let second: Arc<Runtime> = shared_runtime().expect("2 回目の Runtime 取得に失敗した");
        assert!(
            Arc::ptr_eq(&first, &second),
            "2 回目の呼び出しで同一 Runtime が返されること"
        );

        // (2) 全 Arc の drop 後に Weak が upgrade できなくなること。
        let weak = Arc::downgrade(&first);
        drop(first);
        drop(second);
        assert!(
            weak.upgrade().is_none(),
            "全ての強参照が drop された後は Weak が None になること"
        );

        // (3) 再度呼んで返った新しい Runtime で block_on が動くこと。
        let third: Arc<Runtime> = shared_runtime().expect("3 回目の Runtime 再構築に失敗した");
        let value = third.block_on(async { 42 });
        assert_eq!(value, 42, "再構築した Runtime で block_on が動くこと");
    }
}
