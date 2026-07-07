//! クレート内で共通に使うユーティリティ関数。

/// プロセスユニークなサフィックスを生成する。
///
/// ナノ秒タイムスタンプだけでは実解像度 (macOS はマイクロ秒程度) の範囲で
/// 並列 `start()` が同じ値になり、コンテナ ID が衝突する。
/// 同一プロセス内はアトミックカウンタで、プロセス間はプロセス ID で一意性を担保する。
pub(crate) fn unique_suffix() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}-{}", std::process::id(), nanos, count)
}
