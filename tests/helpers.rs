//! macOS 限定の統合テスト用ヘルパー。
//!
//! 各テストバイナリは macOS 専用の `#[cfg]` / `#![cfg]` 付きでこのヘルパーを
//! インクルードするため、Linux ビルドではコンパイルされない。

/// CI 環境ではコンテナ API サーバーが無いためテストをスキップする。
///
/// Apple container が使える self-hosted runner では `RUN_CONTAINER_TESTS=1` で実行する。
/// 値が `"1"` のときだけ有効化する（存在だけでは足りない）。
///
/// # コンテナ後始末について
///
/// 統合テストの停止は `stop_with_timeout(Some(0))` を使うこと。
/// `stop()` の既定は SIGTERM + 30 秒猶予で、Apple container 上の `sleep` 等は
/// SIGTERM にすぐ反応しないため、後始末のたびに約 30 秒待たされる。
/// `Some(0)` は即時 SIGKILL なので、シナリオ検証に不要な待ちを避けられる。
pub fn skip_if_ci() -> bool {
    if std::env::var("RUN_CONTAINER_TESTS").as_deref() == Ok("1") {
        return false;
    }
    if std::env::var("CI").is_ok() || std::env::var("GITHUB_ACTIONS").is_ok() {
        eprintln!("スキップ: CI 環境ではコンテナ API サーバーを利用できない");
        true
    } else if std::env::var("RUN_CONTAINER_TESTS").is_ok() {
        // 値が "1" 以外で設定されている場合もスキップする
        eprintln!("スキップ: RUN_CONTAINER_TESTS は 1 のときだけ有効");
        true
    } else {
        false
    }
}
