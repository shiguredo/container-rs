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

/// spawn / respawn の失敗回数。
///
/// - spawn 失敗と、再 spawn 直後の write 失敗 (spawn 成功 + 即死) を加算する
/// - 成功した spawn は加算しない
/// - 既存 reaper への write 失敗 (pipe 死亡) は加算しない。外部 kill 等の一時的な
///   reaper 死亡で上限を消費して watchdog 全体を無効化しないための設計判断
static SPAWN_FAILURES: AtomicU32 = AtomicU32::new(0);

/// spawn / respawn 失敗上限到達の warn を遷移時に 1 回だけ出すためのフラグ。
static EXHAUSTED_WARNED: AtomicBool = AtomicBool::new(false);

/// spawn 再試行の上限回数。
const MAX_SPAWN_FAILURES: u32 = 3;

/// コンテナ ID を reaper に登録する。
///
/// reaper が未起動または pipe が死んでいる場合は、失敗回数が上限未満なら再 spawn する。
/// ID は Apple container 1.2.0 の `nameValid` 相当の制約で検証し、不適合なら登録しない。
pub(crate) fn register(id: &str) {
    // 不適合な ID は reaper スクリプトの行分割を壊すため登録しない。
    if !crate::core::util::is_valid_container_id(id) {
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

    // 初回 spawn 直後と既存 reaper への write 失敗 (外部 kill 等の一時的な死亡) は
    // 失敗回数に加算しない。加算対象は respawn 失敗のみ (再 spawn 直後の write 失敗を
    // 含む。respawn の spawn 失敗と再 spawn 直後の write 失敗で最大 1 回加算する)。
    if SPAWN_FAILURES.load(Ordering::Relaxed) >= MAX_SPAWN_FAILURES {
        warn_exhausted_once();
        return;
    }

    *guard = try_spawn_reaper();
    if let Some(w) = guard.as_mut() {
        if !write_id(w, id) {
            // spawn 成功 + 即死 (write 失敗) は spawn 失敗と同列に失敗回数へ加算する。
            // 加算しないと register のたびに respawn → 即死が繰り返され上限に到達しない。
            SPAWN_FAILURES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("watchdog reaper died after respawn; container {id} was not registered");
            *guard = None;
            if SPAWN_FAILURES.load(Ordering::Relaxed) >= MAX_SPAWN_FAILURES {
                warn_exhausted_once();
            }
        }
    } else if SPAWN_FAILURES.load(Ordering::Relaxed) >= MAX_SPAWN_FAILURES {
        warn_exhausted_once();
    }
}

/// ID を 1 行書いて flush する。成功なら true。
fn write_id(w: &mut std::process::ChildStdin, id: &str) -> bool {
    writeln!(w, "{id}").and_then(|()| w.flush()).is_ok()
}

/// spawn / respawn 失敗上限到達の warn をプロセス生涯で 1 回だけ出す。
fn warn_exhausted_once() {
    if !EXHAUSTED_WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            "watchdog reaper failed to spawn or respawn {MAX_SPAWN_FAILURES} times; further registration is disabled"
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
    use super::*;

    /// REAPER / SPAWN_FAILURES 等のグローバル状態を触るテストを直列化するためのロック。
    ///
    /// アトミック数は並列でも壊れないが、REAPER (OnceLock<Mutex<Option<ChildStdin>>>) を
    /// 直接操作するテスト同士が並列で走ると finalized 状態や spawn の差し替えが
    /// 競合する。テストレベルで直列化して決定論的にする。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// テスト用に reaper 相当のプロセス (`/bin/cat`) を起動して stdin と Child を返す。
    ///
    /// 実 reaper (REAPER_SCRIPT) は EOF 後に `container rm` を実行するため、
    /// 単体テストでは外部コマンドの副作用を避ける目的で cat で代用する。
    /// cat は stdin を読み続け EOF で終了する点で reaper と挙動が揃うが、
    /// `process_group(0)` でのプロセスグループ分離は行わない (テスト中に
    /// テストプロセスへシグナルが送られる経路が無く、child.kill() で直接
    /// 終了させるため不要)。
    ///
    /// なお `register` の respawn 経路 (`try_spawn_reaper`) は実 REAPER_SCRIPT を
    /// 起動するため、respawn を伴うテストでは `container rm` が実行され得る。
    fn spawn_test_reaper() -> (std::process::Child, std::process::ChildStdin) {
        let mut child = std::process::Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("テスト用 reaper の起動に成功すること");
        let stdin = child
            .stdin
            .take()
            .expect("テスト用 reaper の stdin が取れること");
        (child, stdin)
    }

    /// テスト用にグローバルな失敗状態を初期化する。
    fn reset_global_failures() {
        SPAWN_FAILURES.store(0, Ordering::Relaxed);
        EXHAUSTED_WARNED.store(false, Ordering::Relaxed);
    }

    /// 既存 reaper への write 失敗 (外部 kill 相当) は失敗回数に加算されないこと。
    ///
    /// reaper の stdin を保持したままプロセスを kill して pipe を断ち、
    /// 次の `register` が既存 reaper への write に失敗しても `SPAWN_FAILURES` が
    /// 増えないことを確認する。
    ///
    /// 再 spawn 直後の write 失敗 (spawn 成功 + 即死) が失敗回数に加算される経路は、
    /// モック・スタブ禁止のため決定論的に再現できないためコードレビューで担保する。
    #[test]
    fn write_failure_to_existing_reaper_does_not_count_failure() {
        // グローバル状態 (REAPER / SPAWN_FAILURES) を触るため TEST_LOCK で直列化する。
        let _test_lock = TEST_LOCK
            .lock()
            .expect("テスト用ロックが poison していないこと");
        reset_global_failures();

        // reaper を起動して kill し、死んだプロセスの write fd を REAPER にセットする。
        // これは外部 kill 相当で、既存 reaper が死んでいる状態を表す。
        let (mut child, dead_stdin) = spawn_test_reaper();
        child.kill().expect("reaper を kill できること");
        let _ = child.wait();

        let reaper = REAPER.get_or_init(|| Mutex::new(None));
        {
            let mut guard = reaper
                .lock()
                .expect("reaper mutex はテスト中に poison していないこと");
            *guard = Some(dead_stdin);
        }

        // 死んだ reaper への write は失敗し respawn されるが、失敗回数は加算されない。
        register("c-1-2-3");
        assert_eq!(
            SPAWN_FAILURES.load(Ordering::Relaxed),
            0,
            "既存 reaper への write 失敗で失敗回数が加算されないこと"
        );

        // respawn 後の reaper は正常なので、以後の登録は失敗回数を消費しない。
        register("c-1-2-4");
        assert_eq!(
            SPAWN_FAILURES.load(Ordering::Relaxed),
            0,
            "正常な reaper への登録で失敗回数が加算されないこと"
        );

        // respawn された reaper は実 REAPER_SCRIPT を実行する (cat ではない)。
        // 標準入力を閉じると EOF を検知して、受領済みの ID に対して
        // `container rm --force` を実行し自ら終了する (存在しない ID なので
        // 削除は失敗して無害)。`try_spawn_reaper` が Child を保持しない設計のため
        // wait はできないが、EOF 後に自動終了し zombie はテストプロセス終了時に解消される。
        let mut guard = reaper
            .lock()
            .expect("reaper mutex はテスト中に poison していないこと");
        *guard = None;
        drop(guard);
    }

    /// 生存する reaper への登録が通常どおり完了し、失敗回数を消費しないこと。
    #[test]
    fn register_succeeds_with_alive_reaper() {
        // グローバル状態 (REAPER / SPAWN_FAILURES) を触るため TEST_LOCK で直列化する。
        let _test_lock = TEST_LOCK
            .lock()
            .expect("テスト用ロックが poison していないこと");
        reset_global_failures();

        // 生存する reaper を起動して REAPER にセットする。
        let (mut child, stdin) = spawn_test_reaper();
        let reaper = REAPER.get_or_init(|| Mutex::new(None));
        {
            let mut guard = reaper
                .lock()
                .expect("reaper mutex はテスト中に poison していないこと");
            *guard = Some(stdin);
        }

        // 生存する reaper への登録は失敗せず、失敗回数も消費しない。
        register("c-1-2-3");
        assert_eq!(
            SPAWN_FAILURES.load(Ordering::Relaxed),
            0,
            "生存する reaper への register で失敗回数が加算されないこと"
        );
        assert!(
            child
                .try_wait()
                .expect("reaper の状態を確認できること")
                .is_none(),
            "テスト用 reaper が生存していること"
        );

        // stdin を閉じて reaper を EOF で終了させ、プロセスを回収する。
        let mut guard = reaper
            .lock()
            .expect("reaper mutex はテスト中に poison していないこと");
        *guard = None;
        drop(guard);
        let _ = child.wait();
    }
}
