//! `Container` — 同期 API のコンテナハンドル。
//! 元の 0.27 の `core::containers::sync_container` に相当。
//!
//! macOS (XPC) では `ContainerAsync` を内部で `block_on` する同期ラッパー。

use std::net::IpAddr;
use std::sync::Arc;

use crate::{ContainerAsync, Image, core::error::Result};

/// 既存 tokio ランタイムコンテキスト内から呼ばれた場合は別スレッドで `block_on` し、
/// そうでなければ与えられたランタイム上で直接 `block_on` する。
///
/// 共有 Runtime への再入を検出した場合は即座にエラーを返す。
/// LogConsumer コールバックや async コンテキスト内から同期 API を呼び出すと、
/// 唯一のワーカースレッドが塞がれて timer 依存の処理が進まなくなり deadlock するため、
/// fail-fast で防ぐ。
pub(crate) fn block_on_runtime<F>(runtime: &tokio::runtime::Runtime, f: F) -> Result<F::Output>
where
    F: std::future::Future + Send,
    F::Output: Send,
{
    match tokio::runtime::Handle::try_current() {
        Ok(current) if current.id() == runtime.handle().id() => Err(crate::Error::other(
            "cannot call sync API from within the shared runtime context (LogConsumer callback or async context): this would deadlock",
        )),
        Ok(_) => {
            // 既存ランタイム内からの呼び出しは `Runtime::block_on` が panic するため、
            // 一時的に別スレッドに移してから実行する。
            Ok(std::thread::scope(|s| {
                s.spawn(|| runtime.block_on(f))
                    .join()
                    .expect("thread executing the future panicked; re-raising here")
            }))
        }
        Err(_) => Ok(runtime.block_on(f)),
    }
}

/// 共有ランタイムへの強参照を drop する。
///
/// 最後の強参照の drop は Runtime 本体の drop になるが、tokio ランタイムコンテキスト内での
/// Runtime drop は「Cannot drop a runtime in a context where blocking is not allowed」の
/// panic になるため、コンテキスト内から呼ばれた場合は drop を別スレッドへ移して join する。
///
/// join するのは、呼び出し復帰時点で Runtime 解放済みという同期的性質を維持するため
/// (解放が遅延すると、直後の `SyncRunner::start` が旧 Runtime を掴むか新 Runtime を作るかが
/// 非決定になる)。最終 drop かどうかの判定 (`Arc::strong_count` の確認) はしない。
/// 判定と drop の間に他スレッドが drop すると自分が最終 drop になる競合があるため、
/// コンテキスト内なら常に別スレッドへ移す (非最終 drop でも安全で低コスト)。
///
/// ガードは `Handle::try_current()` の成否で判定する。panic の正確な条件は
/// 「ランタイムに entered しているスレッド上での drop」であり `Handle` の有無は
/// その上位集合だが、過剰に別スレッドへ逃がすだけで安全側に倒れる。
///
/// 既知の限界: 共有ランタイム自身のワーカースレッド上から呼ばれた場合
/// (LogConsumer コールバック内で最後の `Container` を drop した場合等) は、
/// `join()` とランタイム shutdown のワーカー join が循環待ちになりハングし得る。
/// この再入ケースは LogConsumer コールバック起点の再入問題として別途対処する。
pub(crate) fn drop_shared_runtime(runtime: Arc<tokio::runtime::Runtime>) {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => {
            std::thread::spawn(move || drop(runtime))
                .join()
                .expect("thread dropping the shared runtime panicked");
        }
        Err(_) => drop(runtime),
    }
}

/// 同期コンテナハンドル。`ContainerAsync` を包む。
pub struct Container<I: Image> {
    /// 共有ランタイムへの強参照。`Drop` で take して `drop_shared_runtime` に渡すため
    /// `Option` で保持する。`Drop` 以外では常に `Some`。
    runtime: Option<Arc<tokio::runtime::Runtime>>,
    inner: Option<ContainerAsync<I>>,
}

impl<I: Image> Container<I> {
    pub(crate) fn new(runtime: Arc<tokio::runtime::Runtime>, inner: ContainerAsync<I>) -> Self {
        Self {
            runtime: Some(runtime),
            inner: Some(inner),
        }
    }

    fn runtime(&self) -> &tokio::runtime::Runtime {
        self.runtime
            .as_ref()
            .expect("runtime is only taken in Drop")
    }

    fn inner(&self) -> &ContainerAsync<I> {
        self.inner.as_ref().expect("container already removed")
    }

    /// コンテナ ID を返す。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn id(&self) -> &str {
        self.inner().id()
    }

    /// イメージを返す。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn image(&self) -> &I {
        self.inner().image()
    }

    /// コンテナを停止する。デフォルトのタイムアウトで SIGTERM を送信する。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn stop(&self) -> Result<()> {
        self.stop_with_timeout(None)
    }

    /// タイムアウトを指定してコンテナを停止する。
    ///
    /// `timeout_seconds` が `None` の場合はデフォルト (SIGTERM + 30 秒)、
    /// `Some(0)` の場合は即時 SIGKILL。負値は macOS では無限待ちに近いタイムアウト
    /// (`i32::MAX` 秒) で SIGTERM、Linux では 30 秒に変換される。
    ///
    /// macOS では、負値 (またはグレース + 30 秒が 24 時間を超える正の値) を指定すると、
    /// SIGTERM を無視するコンテナでこの呼び出しが最大 24 時間ブロックされた後、
    /// XPC タイムアウトのエラーが返り得る。Linux では指定グレース時間のまま待つ。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn stop_with_timeout(&self, timeout_seconds: Option<i32>) -> Result<()> {
        block_on_runtime(
            self.runtime(),
            self.inner().stop_with_timeout(timeout_seconds),
        )?
    }

    /// コンテナが実行中かどうかを返す。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn is_running(&self) -> Result<bool> {
        block_on_runtime(self.runtime(), self.inner().is_running())?
    }

    /// コンテナの exit code を返す (未終了なら `None`)。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn exit_code(&self) -> Result<Option<i64>> {
        block_on_runtime(self.runtime(), self.inner().exit_code())?
    }

    /// コンテナを削除する (同期版)。
    ///
    /// 共有ランタイム上で `ContainerAsync::rm` に委譲する。
    ///
    /// # 完了保証
    ///
    /// `Ok` を返した時点で削除処理は終わっており、成否は返り値の `Result` として
    /// 呼び出し側に届く。`Drop` は `DROP_REMOVE_TIMEOUT` (5 秒) 内で完了を待つが、
    /// 超過時は best-effort であり成否の `Result` も返さないため、確実な完了保証と
    /// 成否が必要ならこのメソッドを使うこと。`Err` の場合は削除は完了しておらず
    /// (共有ランタイムへの再入検出では削除試行自体が未実行、バックエンドからの
    /// エラーでは試行済み失敗)、いずれも `Drop` の削除経路
    /// (Runtime 内なら timeout 付き待機、Runtime 外なら同期試行) に委ねられる。
    ///
    /// # `keep` ゲートとの非対称
    ///
    /// このメソッドは `TESTCONTAINERS_COMMAND=keep` でも削除する。`keep` ゲートは
    /// `Drop` の削除のみを抑止する仕様であり、明示 `rm` には効かない。
    pub fn rm(mut self) -> Result<()> {
        if let Some(inner) = self.inner.take() {
            block_on_runtime(self.runtime(), inner.rm())??;
        }
        Ok(())
    }

    /// コンテナを同期的に削除する。tokio Runtime 内外のどちらから呼んでも安全。
    ///
    /// 内部の `ContainerAsync::rm_blocking` に委譲する。`block_on` を使わず
    /// `remove_blocking` (同期 I/O) を直接呼び出すため、tokio Runtime 内の同期
    /// コンテキスト (Drop ガードや `spawn_blocking` 内) から呼んでも deadlock しない。
    ///
    /// # `rm()` との使い分け
    ///
    /// - 共有ランタイム上で async 削除を待てるなら `rm()` を使う
    /// - 同一共有 Runtime への再入で `rm()` が `Err` を返す場合や、Runtime 内の
    ///   同期コンテキストから削除完了を待ちたい場合はこのメソッドを使う
    ///
    /// # `keep` ゲートとの非対称
    ///
    /// このメソッドは `TESTCONTAINERS_COMMAND=keep` でも削除する。`keep` ゲートは
    /// `Drop` の削除のみを抑止する仕様であり、明示 `rm` / `rm_blocking` には効かない。
    pub fn rm_blocking(mut self) -> Result<()> {
        if let Some(inner) = self.inner.take() {
            inner.rm_blocking()?;
        }
        Ok(())
    }

    /// コンテナの公開ポートマッピングを取得する。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn ports(&self) -> Result<crate::core::ports::Ports> {
        block_on_runtime(self.runtime(), self.inner().ports())?
    }

    /// コンテナポートに対応するホストの IPv4 ポートを取得する。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn get_host_port_ipv4(
        &self,
        internal_port: impl Into<crate::core::ports::ContainerPort>,
    ) -> Result<u16> {
        let port = internal_port.into();
        block_on_runtime(self.runtime(), self.inner().get_host_port_ipv4(port))?
    }

    /// コンテナポートに対応するホストの IPv6 ポートを取得する。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn get_host_port_ipv6(
        &self,
        internal_port: impl Into<crate::core::ports::ContainerPort>,
    ) -> Result<u16> {
        let port = internal_port.into();
        block_on_runtime(self.runtime(), self.inner().get_host_port_ipv6(port))?
    }

    /// stdout を全量読み出して `Vec<u8>` で返す (follow なし)。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn stdout_to_vec(&self) -> Result<Vec<u8>> {
        block_on_runtime(self.runtime(), self.inner().stdout_to_vec())?
    }

    /// stderr を全量読み出して `Vec<u8>` で返す (follow なし)。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn stderr_to_vec(&self) -> Result<Vec<u8>> {
        block_on_runtime(self.runtime(), self.inner().stderr_to_vec())?
    }

    /// コンテナの状態 snapshot を取得する。
    ///
    /// macOS (Apple container) では最新の host と port mapping を返す。
    /// Linux (Docker) では未実装エラーを返す。
    pub fn container_state(&self) -> Result<crate::core::image::ContainerState> {
        block_on_runtime(self.runtime(), self.inner().container_state())?
    }

    /// stdout の同期リーダーを返す。
    ///
    /// リーダーは独立した読み取り位置を持ち、常にログ先頭から読む。
    /// `follow = true` のときは末尾到達後も追記をポーリングする。呼び出しスレッドをブロックするため、
    /// tokio ランタイムワーカー上や LogConsumer コールバックからは呼ばないこと。
    ///
    /// # Linux
    ///
    /// `follow = true` は demux 開始時点以降のログを共有バッファから `park_timeout(50ms)` の
    /// 周期起床で読む。8 MiB 上限で先頭が drop された場合は取りこぼした旨を `warn` ログに
    /// 出力し、読み進みは継続する。再 start (`refresh_log_streams`) で demux 開始時点が更新
    /// されると、以前に取得した古いリーダーは新バッファに接続されない。`follow = false` は
    /// 呼び出しごとに新規 HTTP セッションを張って現時点までの全ログを取得する。
    pub fn stdout(&self, follow: bool) -> Box<dyn std::io::BufRead + Send> {
        self.inner().stdout_sync(follow)
    }

    /// stderr の同期リーダーを返す。
    ///
    /// リーダーは独立した読み取り位置を持ち、常にログ先頭から読む。
    /// `follow = true` のときは末尾到達後も追記をポーリングする。呼び出しスレッドをブロックするため、
    /// tokio ランタイムワーカー上や LogConsumer コールバックからは呼ばないこと。
    ///
    /// 注意: macOS の stderr は Apple container の bootlog であり、
    /// アプリケーションの stderr は stdout 側のログに混流する。
    ///
    /// # Linux
    ///
    /// Docker Engine API は STREAM_TYPE で stdout / stderr を分離するため、`follow = true`
    /// では本当に stderr のみのログを読む。8 MiB 上限で先頭が drop された場合は取りこぼした
    /// 旨を `warn` ログに出力し、読み進みは継続する。`follow = false` は呼び出しごとに新規
    /// HTTP セッションを張って現時点までの全ログを取得する。
    pub fn stderr(&self, follow: bool) -> Box<dyn std::io::BufRead + Send> {
        self.inner().stderr_sync(follow)
    }

    /// 停止済みなら再起動し、`Image::exec_after_start` を実行する。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn start(&self) -> Result<()> {
        block_on_runtime(self.runtime(), self.inner().start())?
    }

    /// コンテナ内でコマンドを実行する。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn exec(&self, cmd: crate::core::ExecCommand) -> Result<SyncExecResult> {
        match block_on_runtime(self.runtime(), self.inner().exec(cmd)) {
            Ok(Ok(inner)) => Ok(SyncExecResult { inner }),
            Ok(Err(e)) | Err(e) => Err(e),
        }
    }

    /// コンテナのブリッジ IP アドレスを取得する。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn get_bridge_ip_address(&self) -> Result<IpAddr> {
        block_on_runtime(self.runtime(), self.inner().get_bridge_ip_address())?
    }

    /// コンテナに接続するためのホストを返す。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn get_host(&self) -> Result<crate::core::Host> {
        block_on_runtime(self.runtime(), self.inner().get_host())?
    }

    /// コンテナからホストへファイルをコピーする。
    ///
    /// # Feature
    ///
    /// この API は `blocking` feature が必要です。
    pub fn copy_file_from<T: crate::core::copy::CopyFileFromContainer>(
        &self,
        source: impl Into<String> + Send,
        target: T,
    ) -> Result<T::Output> {
        block_on_runtime(self.runtime(), self.inner().copy_file_from(source, target))?
    }
}

/// Drop 時のコンテナ削除は内部の `ContainerAsync` の `Drop` に委譲する。
///
/// # 完了保証
///
/// 委譲先の `ContainerAsync::drop` は Runtime 内 Drop で削除を専用 std スレッドで実行し、
/// `DROP_REMOVE_TIMEOUT` (5 秒) を上限に完了を待つ。timeout 内に完了すれば `drop` 復帰
/// 時点で削除は終わっている。超過時は best-effort。Runtime 外で drop されるときは
/// 呼び出しスレッドで削除試行が終わるまで待つが成功は保証されない。
/// 詳細な契約は `ContainerAsync` の `Drop` を参照すること。
///
/// 削除の完了待ち、または成否の `Result` が必要なら明示 `rm()` を使うこと。
/// `TESTCONTAINERS_COMMAND=keep` のときは Drop は削除しない (明示 `rm` は削除する)。
impl<I: Image> Drop for Container<I> {
    fn drop(&mut self) {
        // `ContainerAsync` の Drop に任せる。`rm` 呼出済みなら `inner` は None。
        // `ContainerAsync` の Drop は専用 std スレッド + mpsc::recv_timeout (Runtime 内)
        // または同期削除 (Runtime 外) で処理され、共有ランタイムの生存に依存しないため、
        // drop の順序に制約は無い。
        drop(self.inner.take());

        // 共有ランタイムへの最後の強参照になり得るため、async コンテキスト内での
        // 最終 drop による panic を避けるヘルパー経由で drop する。
        if let Some(runtime) = self.runtime.take() {
            drop_shared_runtime(runtime);
        }
    }
}

/// exec の結果 (同期版)。元の 0.27 の `SyncExecResult` と同一シグネチャ。
///
/// 本家はランタイム上でストリームを読み出すが、shiguredo は exec 完了時点で
/// 全出力を取得済みのため、バッファ上のリーダーをそのまま返す。
pub struct SyncExecResult {
    inner: crate::core::containers::async_container::exec::ExecResult,
}

impl SyncExecResult {
    /// 終了コードを返す。コマンドがまだ終了していない場合は `None`。
    pub fn exit_code(&self) -> Result<Option<i64>> {
        Ok(self.inner.exit_code)
    }

    /// stdout の同期リーダーを返す。
    pub fn stdout<'b>(&'b mut self) -> Box<dyn std::io::BufRead + Send + 'b> {
        Box::new(&mut self.inner.stdout)
    }

    /// stderr の同期リーダーを返す。
    pub fn stderr<'b>(&'b mut self) -> Box<dyn std::io::BufRead + Send + 'b> {
        Box::new(&mut self.inner.stderr)
    }

    /// stdout を `Vec<u8>` で返す。
    pub fn stdout_to_vec(&mut self) -> Result<Vec<u8>> {
        let mut stdout = Vec::new();
        std::io::Read::read_to_end(&mut self.stdout(), &mut stdout)?;
        Ok(stdout)
    }

    /// stderr を `Vec<u8>` で返す。
    pub fn stderr_to_vec(&mut self) -> Result<Vec<u8>> {
        let mut stderr = Vec::new();
        std::io::Read::read_to_end(&mut self.stderr(), &mut stderr)?;
        Ok(stderr)
    }
}

impl std::fmt::Debug for SyncExecResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncExecResult")
            .field("exit_code", &self.inner.exit_code)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 共有ランタイムと同構成 (worker 1 の multi_thread) のランタイムを作る。
    /// プロセスグローバルな `ASYNC_RUNTIME` には触らない (テスト間干渉を避けるため)。
    fn build_runtime() -> Arc<tokio::runtime::Runtime> {
        Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("テスト用ランタイムの構築に失敗した"),
        )
    }

    /// tokio コンテキスト外では最後の強参照をそのまま drop しても panic しないこと。
    #[test]
    fn drop_shared_runtime_outside_async_context() {
        let runtime = build_runtime();
        drop_shared_runtime(runtime);
    }

    /// tokio コンテキスト内 (current_thread の外側ランタイムの block_on 中) で
    /// 最後の強参照を drop しても panic しないこと。ヘルパーを経由しない素の drop は
    /// 「Cannot drop a runtime in a context where blocking is not allowed」で落ちる経路。
    #[test]
    fn drop_shared_runtime_inside_current_thread_block_on() {
        let outer = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("外側ランタイムの構築に失敗した");
        let inner = build_runtime();
        outer.block_on(async move {
            drop_shared_runtime(inner);
        });
    }

    /// tokio コンテキスト内 (multi_thread の外側ランタイムの block_on 中) でも同様に
    /// panic しないこと。
    #[test]
    fn drop_shared_runtime_inside_multi_thread_block_on() {
        let outer = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("外側ランタイムの構築に失敗した");
        let inner = build_runtime();
        outer.block_on(async move {
            drop_shared_runtime(inner);
        });
    }

    /// 非最終 drop (他に強参照が残っている) でもヘルパーが安全に動き、
    /// 残った参照のランタイムが引き続き使えること。
    #[test]
    fn drop_shared_runtime_non_final_drop_keeps_runtime_usable() {
        let runtime = build_runtime();
        let kept = runtime.clone();
        let outer = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("外側ランタイムの構築に失敗した");
        outer.block_on(async move {
            drop_shared_runtime(runtime);
        });
        let value = kept.block_on(async { 42 });
        assert_eq!(value, 42, "非最終 drop 後もランタイムが使えること");
    }

    /// 既存 tokio ランタイムが無い場合は `block_on_runtime` が呼び出しスレッド上で
    /// future を実行すること。
    #[test]
    fn block_on_runtime_without_existing_runtime_runs_on_caller_thread() {
        let runtime = build_runtime();
        let caller = std::thread::current().id();
        let executed = block_on_runtime(&runtime, async move { std::thread::current().id() })
            .expect("block_on_runtime が成功すること");
        assert_eq!(
            executed, caller,
            "既存ランタイムが無い場合は呼び出しスレッド上で future が実行されること"
        );
    }

    /// 既存 tokio ランタイム (current_thread) 内から呼ばれた場合は別スレッドで
    /// future を実行し、panic しないこと (既存 tokio ランタイム内で block_on_runtime を呼んだとき別スレッドで実行されることの回帰テスト)。
    #[test]
    fn block_on_runtime_inside_current_thread_runtime_runs_on_another_thread() {
        let outer = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("外側ランタイムの構築に失敗した");
        let runtime = build_runtime();
        let caller = std::thread::current().id();
        let executed = outer.block_on(async {
            block_on_runtime(&runtime, async move { std::thread::current().id() })
                .expect("block_on_runtime が成功すること")
        });
        assert_ne!(
            executed, caller,
            "current_thread ランタイム内から呼ばれた場合は別スレッドで future が実行されること"
        );
    }

    /// 既存 tokio ランタイム (multi_thread) 内から呼ばれた場合も別スレッドで
    /// future を実行し、panic しないこと。
    #[test]
    fn block_on_runtime_inside_multi_thread_runtime_runs_on_another_thread() {
        let outer = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("外側ランタイムの構築に失敗した");
        let runtime = build_runtime();
        let caller = std::thread::current().id();
        let executed = outer.block_on(async {
            block_on_runtime(&runtime, async move { std::thread::current().id() })
                .expect("block_on_runtime が成功すること")
        });
        assert_ne!(
            executed, caller,
            "multi_thread ランタイム内から呼ばれた場合は別スレッドで future が実行されること"
        );
    }

    /// 同一 Runtime への再入呼び出しは即座に Err になること。
    #[test]
    fn block_on_runtime_detects_reentry_into_same_runtime() {
        let runtime = build_runtime();
        let result = runtime.block_on(async { block_on_runtime(&runtime, async { 42 }) });
        assert!(
            result.is_err(),
            "同一 Runtime への再入は fail-fast で Err になること"
        );
    }
}
