//! watchdog — テストプロセスがクラッシュした際の孤立コンテナ掃除。
//!
//! 本家 0.27 の `watchdog` feature はシグナルハンドラ方式だが、SIGKILL や
//! SIGSEGV によるクラッシュでは動かない。shiguredo は外部 reaper プロセス方式を採る:
//! 起動したコンテナ ID を pipe 経由で reaper (`/bin/sh`) に登録し、reaper は
//! stdin の EOF (親プロセスの死。シグナル種別を問わず必ず起きる) を検知したら
//! 登録済みコンテナを `container rm --force` で削除する。
//! 通常終了時は Drop が先に削除しており、reaper の rm は無害に失敗する。

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

/// reaper 本体。stdin からコンテナ ID を行単位で受け取り、EOF 後に全削除する。
///
/// `set -f` で glob 展開を止め、改行区切り + `read -r` + `"$id"` 引用で
/// 単語分割・glob 展開を避ける。
const REAPER_SCRIPT: &str = r#"
set -f
ids=""
while IFS= read -r id; do
  case "$id" in
    "") ;;
    *) ids="${ids}${id}
" ;;
  esac
done
printf '%s' "$ids" | while IFS= read -r id; do
  container rm --force "$id" >/dev/null 2>&1
done
"#;

/// reaper の stdin。初期化時は spawn せず、`register` 経路でのみ起動する。
static REAPER: OnceLock<Mutex<Option<std::process::ChildStdin>>> = OnceLock::new();

/// spawn 失敗回数。成功した spawn は加算しない。pipe 死亡そのものも加算しない。
static SPAWN_FAILURES: AtomicU32 = AtomicU32::new(0);

/// spawn 失敗上限到達の warn を遷移時に 1 回だけ出すためのフラグ。
static EXHAUSTED_WARNED: AtomicBool = AtomicBool::new(false);

/// spawn 再試行の上限回数。
const MAX_SPAWN_FAILURES: u32 = 3;

/// コンテナ ID が reaper に安全に渡せる形式か検証する。
///
/// Apple container 1.2.0 の `nameValid` 相当の制約 (先頭は英数字・実質 2 文字以上・63 文字以下・
/// 文字種は英数字 / `_` / `.` / `-`) に委譲する。新規則の受理集合は従来の `[A-Za-z0-9._-]` の
/// 受理集合の部分集合であり、空文字・空白・改行・glob 文字の拒否による reaper スクリプトの
/// シェル安全性は維持される (1 文字 ID は `nameValid` が許さないため拒否する)。
fn is_valid_container_id(id: &str) -> bool {
    crate::core::util::is_valid_container_id(id)
}

/// コンテナ ID を reaper に登録する。
///
/// reaper が未起動または pipe が死んでいる場合は、失敗回数が上限未満なら再 spawn する。
/// ID は Apple container 1.2.0 の `nameValid` 相当の制約で検証し、不適合なら登録しない。
pub(crate) fn register(id: &str) {
    // 不適合な ID は reaper スクリプトの行分割を壊すため登録しない。
    if !is_valid_container_id(id) {
        tracing::warn!("watchdog refused invalid container id: {id:?}");
        return;
    }

    let mut guard = REAPER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("reaper mutex must not be poisoned while registering a container");

    // 未起動なら上限未満のあいだ再 spawn を試みる。
    if guard.is_none() {
        if SPAWN_FAILURES.load(Ordering::Relaxed) >= MAX_SPAWN_FAILURES {
            warn_exhausted_once();
            return;
        }
        *guard = try_spawn_reaper();
        if guard.is_none() {
            // try_spawn_reaper 内で失敗回数を加算済み。上限到達なら exhausted を出す。
            if SPAWN_FAILURES.load(Ordering::Relaxed) >= MAX_SPAWN_FAILURES {
                warn_exhausted_once();
            }
            return;
        }
    }

    // 登録を書き込む。pipe が死んでいれば warn して再 spawn → 当該 ID を 1 回再試行する。
    if write_id(
        guard.as_mut().expect("reaper stdin present after spawn"),
        id,
    ) {
        return;
    }

    tracing::warn!("watchdog reaper pipe closed; container {id} was not registered");
    *guard = None;

    // pipe 死亡そのものは失敗回数に加算しない。再 spawn の失敗だけを加算する。
    if SPAWN_FAILURES.load(Ordering::Relaxed) >= MAX_SPAWN_FAILURES {
        warn_exhausted_once();
        return;
    }

    *guard = try_spawn_reaper();
    if let Some(w) = guard.as_mut() {
        if !write_id(w, id) {
            tracing::warn!("watchdog reaper pipe closed; container {id} was not registered");
            *guard = None;
        }
    } else if SPAWN_FAILURES.load(Ordering::Relaxed) >= MAX_SPAWN_FAILURES {
        warn_exhausted_once();
    }
}

/// ID を 1 行書いて flush する。成功なら true。
fn write_id(w: &mut std::process::ChildStdin, id: &str) -> bool {
    writeln!(w, "{id}").and_then(|()| w.flush()).is_ok()
}

/// spawn 失敗上限到達の warn をプロセス生涯で 1 回だけ出す。
fn warn_exhausted_once() {
    if !EXHAUSTED_WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            "watchdog reaper spawn exhausted after {MAX_SPAWN_FAILURES} failures; further registration is disabled"
        );
    }
}

/// reaper プロセスを起動し、stdin を返す。失敗時は warn して失敗回数を加算する。
fn try_spawn_reaper() -> Option<std::process::ChildStdin> {
    let child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(REAPER_SCRIPT)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // テストプロセスのプロセスグループへのシグナル (Ctrl-C 等) で
        // reaper が巻き添えにならないよう、別プロセスグループで起動する。
        .process_group(0)
        .spawn();
    match child {
        Ok(mut c) => match c.stdin.take() {
            Some(stdin) => Some(stdin),
            None => {
                // spawn は成功したが stdin が取れない場合も失敗扱い。
                SPAWN_FAILURES.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("failed to spawn watchdog reaper: stdin pipe was not created");
                None
            }
        },
        Err(err) => {
            SPAWN_FAILURES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("failed to spawn watchdog reaper: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_valid_container_id;

    /// デフォルト生成 ID 形式と一般的な名前が適合すること。
    #[test]
    fn accepts_valid_container_ids() {
        assert!(
            is_valid_container_id("c-1-2-3"),
            "デフォルト形式の ID は適合するべき"
        );
        assert!(
            is_valid_container_id("my_container.test-01"),
            "許可文字のみの名前は適合するべき"
        );
        assert!(
            is_valid_container_id("A1"),
            "英数字 2 文字以上の名前は適合するべき"
        );
    }

    /// 空文字・空白・改行・glob 文字など不適合 ID を拒否すること。
    #[test]
    fn rejects_invalid_container_ids() {
        assert!(!is_valid_container_id(""), "空文字は拒否するべき");
        assert!(
            !is_valid_container_id("A"),
            "1 文字の ID は拒否するべき (nameValid は実質 2 文字以上)"
        );
        assert!(
            !is_valid_container_id("has space"),
            "空白を含む ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id("glob*"),
            "glob 文字を含む ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id("line\nbreak"),
            "改行を含む ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id("q?"),
            "クエスチョンマークを含む ID は拒否するべき"
        );
        assert!(
            !is_valid_container_id("a/b"),
            "スラッシュを含む ID は拒否するべき"
        );
    }
}
