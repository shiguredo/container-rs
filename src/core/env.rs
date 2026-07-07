//! 環境設定。元の 0.27 の `core::env` に相当するが、
//! macOS (XPC) では bollard を使わないため最小限の実装にとどめる。

/// コンテナ終了時のコマンド。本家と同じ。
#[derive(Debug, Clone, Copy, Default)]
pub enum Command {
    #[default]
    Remove,
    Keep,
}

/// 設定。macOS では実質的に空。
#[derive(Debug, Clone, Default)]
pub struct Config;

impl Config {
    pub fn command(&self) -> Command {
        // `TESTCONTAINERS_COMMAND=keep` 環境変数で挙動を変える（本家と同じ）。
        match std::env::var("TESTCONTAINERS_COMMAND").as_deref() {
            Ok("keep") => Command::Keep,
            _ => Command::Remove,
        }
    }
}
