//! 環境設定。本家 testcontainers-rs 0.27 の `core::env` に相当するが、
//! macOS (XPC) バックエンドでは最小限の実装にとどめる。

use std::collections::BTreeMap;

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

/// `KEY=VALUE` 形式の env リストを生成する。
///
/// `base` に `overrides` を上書きしてから `KEY=VALUE` のリストに整形する。
/// BTreeMap に畳むことで、同名キーは後から来た値 (上書き) が勝ち・キーソート済みの
/// 出力になる。macOS / Linux の全経路 (build 側 2 箇所・exec 側 2 箇所) で同一規則を
/// 適用するため、ランタイム実装依存の重複エントリ解決に頼らない。
pub(crate) fn fold_env(
    base: impl IntoIterator<Item = (String, String)>,
    overrides: impl IntoIterator<Item = (String, String)>,
) -> Vec<String> {
    let mut merged: BTreeMap<String, String> = base.into_iter().collect();
    for (k, v) in overrides {
        merged.insert(k, v);
    }
    merged
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect()
}
