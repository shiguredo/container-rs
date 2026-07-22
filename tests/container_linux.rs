//! Linux: ContainerAsync ライフサイクルの統合テスト。
//!
//! macOS ではコンパイル対象外。Linux / CI (`test-linux-docker`) で必ず実行する。

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::time::Duration;

use shiguredo_container::core::CmdWaitFor;
use shiguredo_container::core::error::ClientError;
use shiguredo_container::core::image::ExecCommand;
use shiguredo_container::{AsyncRunner, Error, GenericImage, ImageExt};

/// 常駐 alpine を起動する。
async fn start_alpine() -> shiguredo_container::ContainerAsync<GenericImage> {
    GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .start()
        .await
        .expect("alpine コンテナの起動に失敗した")
}

/// `docker inspect` を 1 回実行してコンテナ不在を assert する。
///
/// Runtime 外 Drop や明示 `rm` の後で使う。両者とも呼び出し復帰時点で削除試行 (または
/// 削除処理) が終わっているため、ポーリング無しの 1 ショット判定で足りる。
fn assert_absent_once_blocking(id: &str) {
    let status = std::process::Command::new("docker")
        .args(["inspect", id])
        .status()
        .expect("docker inspect の実行に失敗した");
    assert!(
        !status.success(),
        "削除試行完了直後は 1 ショット inspect で不在であること: {id}"
    );
}

/// `docker inspect` を 1 回実行してコンテナ不在を assert する (async 版)。
///
/// 明示 `rm().await` 直後に使う。`rm().await` は削除完了まで戻らないため、
/// ポーリング無しの 1 ショット判定で足りる。
async fn assert_absent_once(id: &str) {
    let id_owned = id.to_string();
    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new("docker")
            .args(["inspect", &id_owned])
            .status()
            .expect("docker inspect の実行に失敗した")
    })
    .await
    .expect("docker inspect の spawn_blocking に失敗した");
    assert!(
        !status.success(),
        "rm 完了直後は 1 ショット inspect で不在であること: {id}"
    );
}

/// `docker inspect` が非 0 になるまで待つ (コンテナ不在)。
///
/// Runtime 内 Drop は削除を専用 std スレッドで `remove_blocking` として実行し join
/// しないため、`drop` からの復帰時点で削除完了は保証されない。完了非保証のため、
/// 最終確認にはポーリングを使う。current_thread ランタイムのスタベーションを避ける
/// ため `thread::sleep` ではなく `tokio::time::sleep` で待つ。
async fn wait_until_absent(id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let id_owned = id.to_string();
        let status = tokio::task::spawn_blocking(move || {
            std::process::Command::new("docker")
                .args(["inspect", &id_owned])
                .status()
                .expect("docker inspect の実行に失敗した")
        })
        .await
        .expect("docker inspect の spawn_blocking に失敗した");
        if !status.success() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("コンテナが削除されるまで待機したが残っている: {id}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// start → is_running → ports → exec → stop → rm のハッピーパス。
#[tokio::test]
async fn alpine_lifecycle_start_exec_stop_rm() {
    let container = start_alpine().await;
    let id = container.id().to_string();

    assert!(
        container
            .is_running()
            .await
            .expect("is_running の取得に失敗した"),
        "起動直後は running であること"
    );
    container.ports().await.expect("ports の取得に失敗した");

    let exec = container
        .exec(ExecCommand::new(["true"]).with_cmd_ready_condition(CmdWaitFor::exit_code(0)))
        .await
        .expect("exec true に失敗した");
    assert_eq!(
        exec.exit_code().await.expect("exit_code の取得に失敗した"),
        Some(0)
    );

    container
        .stop_with_timeout(Some(0))
        .await
        .expect("stop に失敗した");
    assert!(
        !container
            .is_running()
            .await
            .expect("stop 後の is_running に失敗した"),
        "stop 後は running でないこと"
    );

    container
        .stop_with_timeout(Some(1))
        .await
        .expect("既停止への stop_with_timeout が冪等に成功すること");

    container.rm().await.expect("rm に失敗した");
    // 明示 rm() は削除完了まで戻るため、直後の inspect は 1 ショットで不在になる。
    assert_absent_once(&id).await;
}

/// stop → start 再起動で再び running になること。
#[tokio::test]
async fn alpine_stop_then_start_is_running() {
    let container = start_alpine().await;

    container
        .stop_with_timeout(Some(0))
        .await
        .expect("stop に失敗した");
    assert!(
        !container
            .is_running()
            .await
            .expect("stop 後の is_running に失敗した")
    );

    container.start().await.expect("再起動に失敗した");
    assert!(
        container
            .is_running()
            .await
            .expect("再起動後の is_running に失敗した"),
        "再起動後は running であること"
    );

    container.rm().await.expect("rm に失敗した");
}

/// Runtime 内 Drop でコンテナが削除されること。
///
/// Runtime 内 Drop は削除を専用 std スレッドに丸投げして join しないため、
/// `drop` 復帰時点の削除完了は保証されない (契約どおり)。
/// 最終確認にはポーリング (`wait_until_absent`) を使う。
#[tokio::test]
async fn alpine_drop_inside_runtime_removes_container() {
    let container = start_alpine().await;
    let id = container.id().to_string();
    drop(container);
    wait_until_absent(&id).await;
}

/// Runtime 外 Drop でパニックせずコンテナが削除されること。
///
/// Runtime 外 Drop は呼び出しスレッドで `remove_blocking` を同期実行するため、
/// `drop` 復帰時点で削除試行が終わっている。1 ショット inspect で不在を確認する
/// (macOS 側の `xpc_alpine_drop_outside_runtime_does_not_panic` と同型)。
#[test]
fn alpine_drop_outside_runtime_does_not_panic() {
    let rt = tokio::runtime::Runtime::new().expect("ランタイムの作成に失敗した");
    let container = rt.block_on(async { start_alpine().await });
    let id = container.id().to_string();
    rt.block_on(async {
        container.stop_with_timeout(Some(0)).await.ok();
    });
    drop(rt);
    drop(container);
    assert_absent_once_blocking(&id);
}

/// 外部で削除したあと、公開 API が ContainerNotFound を返すこと。
#[tokio::test]
async fn inspect_after_external_rm_returns_container_not_found() {
    let container = start_alpine().await;
    let id = container.id().to_string();

    let status = std::process::Command::new("docker")
        .args(["rm", "-f", &id])
        .status()
        .expect("docker rm -f の実行に失敗した");
    assert!(status.success(), "docker rm -f が成功すること");

    let err = container
        .is_running()
        .await
        .expect_err("削除済みコンテナの is_running は失敗すること");
    match err {
        Error::Client(ClientError::ContainerNotFound(found)) => {
            assert!(
                found.contains(&id) || id.starts_with(&found) || found.starts_with(&id),
                "ContainerNotFound の ID が一致すること: {found} vs {id}"
            );
        }
        other => panic!("ContainerNotFound 以外のエラー: {other}"),
    }
}

/// 未実装境界が Err を返すこと。
#[tokio::test]
async fn unimplemented_boundaries_return_err() {
    let container = start_alpine().await;

    container
        .get_bridge_ip_address()
        .await
        .expect_err("get_bridge_ip_address は Linux で未対応であること");
    container
        .copy_file_from(
            "/etc/hostname",
            PathBuf::from("/tmp/container-rs-copy-from-test"),
        )
        .await
        .expect_err("copy_file_from は Linux で未実装であること");
    container
        .exit_code()
        .await
        .expect_err("exit_code は Linux で未実装であること");

    container.rm().await.expect("rm に失敗した");
}

/// Linux では exec の stdout/stderr メッセージ待ちと with_env_vars が明示エラーになること。
#[tokio::test]
async fn exec_unsupported_options_return_err() {
    let container = start_alpine().await;

    container
        .exec(
            ExecCommand::new(["true"]).with_cmd_ready_condition(CmdWaitFor::message_on_stdout("x")),
        )
        .await
        .expect_err("StdOutMessage は Linux で未対応であること");

    container
        .exec(
            ExecCommand::new(["true"]).with_cmd_ready_condition(CmdWaitFor::message_on_stderr("x")),
        )
        .await
        .expect_err("StdErrMessage は Linux で未対応であること");

    container
        .exec(ExecCommand::new(["true"]).with_env_vars([("K", "V")]))
        .await
        .expect_err("with_env_vars は Linux で未対応であること");

    container.rm().await.expect("rm に失敗した");
}

/// Linux で未配線の ImageExt は start 時に明示エラーになること。
#[tokio::test]
async fn unsupported_image_ext_fails_fast_on_start() {
    let err = GenericImage::new("alpine", "latest")
        .with_cmd(["true"])
        .with_hostname("example")
        .start()
        .await
        .expect_err("with_hostname は Linux で未実装であること");
    assert!(
        err.to_string()
            .contains("with_hostname() is not implemented on Linux"),
        "明示メッセージであること: {err}"
    );
}

/// Linux で Log 待機は start 時に明示エラーになること。
#[tokio::test]
async fn log_wait_fails_fast_on_start() {
    use shiguredo_container::WaitFor;

    // with_wait_for は GenericImage のメソッドなので、ImageExt (with_cmd) より先に呼ぶ。
    let err = GenericImage::new("alpine", "latest")
        .with_wait_for(WaitFor::message_on_stdout("ready"))
        .with_cmd(["true"])
        .start()
        .await
        .expect_err("Log 待機は Linux で未対応であること");
    assert!(
        err.to_string()
            .contains("log wait is not supported on Linux"),
        "明示メッセージであること: {err}"
    );
}

/// `with_exposed_port` だけでホストポートが割当されること。
///
/// create 時は host_port 0 を Docker に渡し、起動後に非 0 の割当結果を回収する。
/// Linux CI (`test-linux-docker`) で必ず実行する。
#[tokio::test]
async fn alpine_exposed_port_auto_mapping() {
    use shiguredo_container::core::IntoContainerPort;

    let container = GenericImage::new("alpine", "latest")
        .with_exposed_port(80.tcp())
        .with_cmd(["tail", "-f", "/dev/null"])
        .start()
        .await
        .expect("公開ポート付き alpine コンテナの起動に失敗した");

    let host_port = container
        .get_host_port_ipv4(80.tcp())
        .await
        .expect("公開ホストポートの解決に失敗した");
    assert_ne!(host_port, 0, "ホストポートが割り当てられること");

    container.stop_with_timeout(Some(0)).await.ok();
    container.rm().await.ok();
}
