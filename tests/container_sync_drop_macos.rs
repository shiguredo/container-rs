//! tokio コンテキスト内で同期 `Container` を drop / rm した際に、共有ランタイムの
//! 最終 drop が「Cannot drop a runtime in a context where blocking is not allowed」で
//! panic しないことを検証する統合テスト。
//!
//! 共有ランタイムはプロセスグローバルに共有されるため、同一テストバイナリ内で
//! 並列実行される他テストが同期 API 経由で `Arc` の強参照を保持していると
//! 「最後の強参照の drop」を踏めず、検証対象を通らないまま pass してしまう。
//! このため同期 API を使う他のテストはこのバイナリに置かないこと。
//! バイナリ内のテスト同士は `TEST_LOCK` で直列化して最終 drop を保証する。

#![cfg(all(target_os = "macos", feature = "blocking"))]

use std::sync::{Mutex, MutexGuard};

use shiguredo_container::{GenericImage, ImageExt, SyncRunner};

mod helpers;

/// バイナリ内のテストを直列化するロック。並列実行で複数テストが同時に
/// 共有ランタイムの強参照を持つと「最終 drop」を踏めなくなるため。
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    // 先行テストの panic による poison はこのロックの整合性に影響しないため無視する。
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// drop 経路のテストで削除されずに残り得るコンテナを強制削除する (ベストエフォート)。
/// async コンテキスト内での drop では削除タスクがテストランタイムの破棄と競合して
/// 実行されない場合があり、コンテナがリークし得るため。
fn cleanup_container(id: &str) {
    let _ = std::process::Command::new("container")
        .args(["rm", "--force", id])
        .output();
}

/// tokio コンテキスト (current_thread) 内で最後の `Container` をそのまま drop しても
/// 共有ランタイムの最終 drop が panic しないこと。
#[tokio::test]
async fn sync_container_drop_in_current_thread_tokio_context() {
    if helpers::skip_if_ci() {
        return;
    }
    let _guard = lock();

    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .start()
        .expect("コンテナの起動に失敗した");
    let id = container.id().to_string();

    // そのまま drop する。panic せずテスト関数を抜けられれば成功。
    drop(container);

    cleanup_container(&id);
}

/// tokio コンテキスト (multi_thread) 内でも同様に panic しないこと。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_container_drop_in_multi_thread_tokio_context() {
    if helpers::skip_if_ci() {
        return;
    }
    let _guard = lock();

    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .start()
        .expect("コンテナの起動に失敗した");
    let id = container.id().to_string();

    drop(container);

    cleanup_container(&id);
}

/// tokio コンテキスト (current_thread) 内で `rm()` を呼んでも panic しないこと。
/// `rm(mut self)` は self を consume するため、return 時に共有ランタイムの
/// 最終 drop が同じ async コンテキスト内で走る。
#[tokio::test]
async fn sync_container_rm_in_current_thread_tokio_context() {
    if helpers::skip_if_ci() {
        return;
    }
    let _guard = lock();

    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .start()
        .expect("コンテナの起動に失敗した");

    container.rm().expect("コンテナの削除に失敗した");
}

/// tokio コンテキスト (multi_thread) 内でも `rm()` が panic しないこと。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_container_rm_in_multi_thread_tokio_context() {
    if helpers::skip_if_ci() {
        return;
    }
    let _guard = lock();

    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .start()
        .expect("コンテナの起動に失敗した");

    container.rm().expect("コンテナの削除に失敗した");
}

/// tokio コンテキスト内で `start` が Err を返す場合 (存在しないイメージ) でも、
/// 取得済みの共有ランタイムの強参照が Err 経路で捨てられる際に panic しないこと。
#[tokio::test]
async fn sync_start_error_in_tokio_context() {
    if helpers::skip_if_ci() {
        return;
    }
    let _guard = lock();

    let result = GenericImage::new("alpine", "no-such-tag-for-start-error-path")
        .with_cmd(["true"])
        .start();
    assert!(
        result.is_err(),
        "存在しないイメージの start は Err になること"
    );
}

/// 生存中の `Container` が無い状態で tokio コンテキスト内から `pull_image` を呼んでも
/// panic しないこと。`pull_image` は取得した共有ランタイムの強参照を関数末尾で捨てるため、
/// return 時点で最終 drop になる。
#[tokio::test]
async fn sync_pull_image_in_tokio_context() {
    if helpers::skip_if_ci() {
        return;
    }
    let _guard = lock();

    // 戻り値の `ContainerRequest` はこのテストでは使わないため明示的に捨てる。
    let _ = GenericImage::new("alpine", "latest")
        .pull_image()
        .expect("イメージの取得に失敗した");
}

/// `container list --all --quiet` の出力に対象 ID が行完全一致で含まれるか。
fn container_id_listed(id: &str) -> bool {
    let out = std::process::Command::new("container")
        .args(["list", "--all", "--quiet"])
        .output()
        .expect("container list の実行に失敗した");
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().any(|line| line.trim() == id)
}

/// 同期 API の構築後エラー (起動タイムアウト) で孤立コンテナが残らないこと。
///
/// 素の `#[test]` で最終 drop 条件 (生存中の他 Container が無い) を踏む。
#[test]
fn sync_startup_timeout_removes_container() {
    if helpers::skip_if_ci() {
        return;
    }
    let _guard = lock();

    let name = format!("drop-removal-sync-timeout-{}", std::process::id());
    cleanup_container(&name);

    let result = GenericImage::new("alpine", "latest")
        .with_wait_for(shiguredo_container::WaitFor::message_on_stdout(
            "NO_SUCH_MESSAGE_DROP_REMOVAL",
        ))
        .with_container_name(&name)
        .with_cmd(["sleep", "30"])
        .with_startup_timeout(std::time::Duration::from_secs(2))
        .start();

    match result {
        Err(shiguredo_container::Error::WaitContainer(
            shiguredo_container::core::error::WaitContainerError::StartupTimeout { id, timeout },
        )) => {
            assert_eq!(id, name, "コンテナ ID が指定値と一致すること");
            assert_eq!(
                timeout,
                std::time::Duration::from_secs(2),
                "timeout が指定値と一致すること"
            );
        }
        Ok(_) => panic!("起動タイムアウトは StartupTimeout になること"),
        Err(other) => panic!("起動タイムアウトは StartupTimeout になること: {other:?}"),
    }

    let listed = container_id_listed(&name);
    cleanup_container(&name);
    assert!(
        !listed,
        "構築後エラー後にコンテナが残っていないこと: {name}"
    );
}

/// 同期 API の構築後エラー (with_health_check 未対応) で孤立コンテナが残らないこと。
#[test]
fn sync_host_gateway_removes_container() {
    if helpers::skip_if_ci() {
        return;
    }
    let _guard = lock();

    let name = format!("drop-removal-sync-hostgw-{}", std::process::id());
    cleanup_container(&name);

    let result = GenericImage::new("alpine", "latest")
        .with_container_name(&name)
        .with_health_check(shiguredo_container::core::healthcheck::Healthcheck::none())
        .with_cmd(["sleep", "30"])
        .start();

    match result {
        Err(err) => {
            assert!(
                err.to_string().contains("with_health_check"),
                "with_health_check 未対応のメッセージを含むこと: {err}"
            );
        }
        Ok(_) => panic!("with_health_check は macOS で必ずエラーになること"),
    }

    let listed = container_id_listed(&name);
    cleanup_container(&name);
    assert!(
        !listed,
        "構築後エラー後にコンテナが残っていないこと: {name}"
    );
}

/// `TESTCONTAINERS_COMMAND=keep` 指定時は構築後エラーでもコンテナが残ること。
///
/// env はプロセスグローバルのため、親が subprocess で子テストを起動する。
#[test]
fn keep_on_startup_failure_preserves_container() {
    if helpers::skip_if_ci() {
        return;
    }
    let _guard = lock();

    let name = format!("drop-removal-keep-{}", std::process::id());
    cleanup_container(&name);

    let exe = std::env::current_exe().expect("テスト実行ファイルのパス取得に失敗した");
    let status = std::process::Command::new(exe)
        .args(["--exact", "keep_on_startup_failure_victim", "--nocapture"])
        .env("TESTCONTAINERS_COMMAND", "keep")
        .env("DROP_REMOVAL_KEEP_NAME", &name)
        .env("RUN_CONTAINER_TESTS", "1")
        .env("DROP_REMOVAL_KEEP_VICTIM", "1")
        .status()
        .expect("keep 検証用の子プロセス起動に失敗した");

    let listed = container_id_listed(&name);
    cleanup_container(&name);
    assert!(status.success(), "子テストが成功すること: {status}");
    assert!(
        listed,
        "子テスト終了時点で keep 指定のコンテナが残っていたこと: {name}"
    );
}

/// keep 指定時の構築後エラーでコンテナが残ることを検証する子テスト。
///
/// 親テストが `TESTCONTAINERS_COMMAND=keep` と固定名を渡して起動する。
#[test]
fn keep_on_startup_failure_victim() {
    if std::env::var_os("DROP_REMOVAL_KEEP_VICTIM").is_none() {
        return;
    }
    let name = std::env::var("DROP_REMOVAL_KEEP_NAME")
        .expect("親テストが DROP_REMOVAL_KEEP_NAME を渡すこと");

    let result = GenericImage::new("alpine", "latest")
        .with_container_name(&name)
        .with_health_check(shiguredo_container::core::healthcheck::Healthcheck::none())
        .with_cmd(["sleep", "30"])
        .start();

    match result {
        Err(err) => {
            assert!(
                err.to_string().contains("with_health_check"),
                "with_health_check 未対応のメッセージを含むこと: {err}"
            );
        }
        Ok(_) => panic!("with_health_check は macOS で必ずエラーになること"),
    }

    assert!(
        container_id_listed(&name),
        "TESTCONTAINERS_COMMAND=keep では構築後エラーでもコンテナが残ること: {name}"
    );
}

/// 構築前失敗 (copy_to のホスト側ファイル欠落) の確定トリガ用パス。
///
/// 存在しない絶対パスを返し、create / bootstrap / start_process は成功したうえで
/// `copy_to_sources` の `copy_in` が失敗する経路に入る。
fn missing_copy_source_path() -> std::path::PathBuf {
    std::path::PathBuf::from(format!(
        "/tmp/shiguredo_container_missing_copy_{}.txt",
        std::process::id()
    ))
}

/// デフォルト (`Remove`) では構築前失敗 (copy_to) 後にコンテナが残らないこと。
///
/// env 変更は不要で、同一プロセス内の素の `#[test]` で検証する。
#[test]
fn early_rollback_copy_failure_removes_container() {
    if helpers::skip_if_ci() {
        return;
    }
    let _guard = lock();

    let name = format!("early-rollback-remove-{}", std::process::id());
    cleanup_container(&name);

    let missing = missing_copy_source_path();
    assert!(
        !missing.exists(),
        "コピー元は存在しないパスであること: {}",
        missing.display()
    );

    let result = GenericImage::new("alpine", "latest")
        .with_container_name(&name)
        .with_copy_to("/data/missing.txt", missing)
        .with_cmd(["sleep", "30"])
        .start();

    assert!(
        result.is_err(),
        "存在しないホスト側ファイルへの copy_to は Err になること"
    );

    let listed = container_id_listed(&name);
    cleanup_container(&name);
    assert!(
        !listed,
        "構築前ロールバック後にコンテナが残っていないこと: {name}"
    );
}

/// `TESTCONTAINERS_COMMAND=keep` 指定時は構築前失敗でもコンテナが残ること。
///
/// env はプロセスグローバルのため、親が subprocess で子テストを起動する。
#[test]
fn keep_on_early_rollback_preserves_container() {
    if helpers::skip_if_ci() {
        return;
    }
    let _guard = lock();

    let name = format!("early-rollback-keep-{}", std::process::id());
    cleanup_container(&name);

    let exe = std::env::current_exe().expect("テスト実行ファイルのパス取得に失敗した");
    let status = std::process::Command::new(exe)
        .args(["--exact", "keep_on_early_rollback_victim", "--nocapture"])
        .env("TESTCONTAINERS_COMMAND", "keep")
        .env("EARLY_ROLLBACK_KEEP_NAME", &name)
        .env("RUN_CONTAINER_TESTS", "1")
        .env("EARLY_ROLLBACK_KEEP_VICTIM", "1")
        .status()
        .expect("keep 構築前ロールバック検証用の子プロセス起動に失敗した");

    let listed = container_id_listed(&name);
    cleanup_container(&name);
    assert!(status.success(), "子テストが成功すること: {status}");
    assert!(
        listed,
        "子テスト終了時点で keep 指定のコンテナが残っていたこと: {name}"
    );
}

/// keep 指定時の構築前失敗でコンテナが残ることを検証する子テスト。
///
/// 親テストが `TESTCONTAINERS_COMMAND=keep` と固定名を渡して起動する。
#[test]
fn keep_on_early_rollback_victim() {
    if std::env::var_os("EARLY_ROLLBACK_KEEP_VICTIM").is_none() {
        return;
    }
    let name = std::env::var("EARLY_ROLLBACK_KEEP_NAME")
        .expect("親テストが EARLY_ROLLBACK_KEEP_NAME を渡すこと");

    let missing = missing_copy_source_path();
    assert!(
        !missing.exists(),
        "コピー元は存在しないパスであること: {}",
        missing.display()
    );

    let result = GenericImage::new("alpine", "latest")
        .with_container_name(&name)
        .with_copy_to("/data/missing.txt", missing)
        .with_cmd(["sleep", "30"])
        .start();

    assert!(
        result.is_err(),
        "存在しないホスト側ファイルへの copy_to は Err になること"
    );

    assert!(
        container_id_listed(&name),
        "TESTCONTAINERS_COMMAND=keep では構築前失敗でもコンテナが残ること: {name}"
    );
}
