//! 環境設定。本家 testcontainers-rs 0.27 の `core::env` に相当するが、
//! macOS (XPC) バックエンドでは最小限の実装にとどめる。

/// コンテナ終了時のコマンド。本家と同じ。
#[derive(Debug, Clone, Copy, Default)]
pub enum Command {
    #[default]
    Remove,
    Keep,
}

/// `TESTCONTAINERS_COMMAND` 環境変数からコンテナ終了コマンドを返す。
pub fn command() -> Command {
    // `TESTCONTAINERS_COMMAND=keep` 環境変数で挙動を変える（本家と同じ）。
    match std::env::var("TESTCONTAINERS_COMMAND").as_deref() {
        Ok("keep") => Command::Keep,
        _ => Command::Remove,
    }
}
