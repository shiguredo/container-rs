//! Linux: ContainerAsync ライフサイクルの統合テスト。
//!
//! macOS ではコンパイル対象外。Linux / CI (`test-linux-docker`) で必ず実行する。

#![cfg(target_os = "linux")]

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use shiguredo_container::core::CmdWaitFor;
use shiguredo_container::core::error::ClientError;
use shiguredo_container::core::image::ExecCommand;
use shiguredo_container::core::logs::{LogFrame, consumer::LogConsumer};
use shiguredo_container::{AsyncRunner, Error, GenericImage, ImageExt, WaitFor};

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

/// `haystack` に `needle` が部分一致で含まれるか。空の `needle` は常に true。
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// 受信フレームを記録する `LogConsumer` 実装 (テスト用観測)。
///
/// 本番と同じ `LogConsumer` トレイトを実装し、実 Docker Engine 越しに動作する。
/// `DockerClient` や `AsyncRunner` を差し替えないため、モック / スタブではない。
#[derive(Clone, Default)]
struct RecordingConsumer {
    frames: Arc<Mutex<Vec<LogFrame>>>,
}

impl RecordingConsumer {
    fn frames(&self) -> Vec<LogFrame> {
        self.frames
            .lock()
            .expect("フレーム記録用 Mutex は poison しないこと")
            .clone()
    }
}

impl LogConsumer for RecordingConsumer {
    fn accept<'a>(&'a self, record: &'a LogFrame) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.frames
                .lock()
                .expect("フレーム記録用 Mutex は poison しないこと")
                .push(record.clone());
        })
    }
}

/// `WaitFor::message_on_stdout` が Linux で成立すること。
#[tokio::test]
async fn log_wait_message_on_stdout_succeeds() {
    let container = GenericImage::new("alpine", "latest")
        .with_wait_for(WaitFor::message_on_stdout("READY_OUT"))
        .with_cmd(["sh", "-c", "echo READY_OUT; tail -f /dev/null"])
        .with_startup_timeout(Duration::from_secs(15))
        .start()
        .await
        .expect("stdout ログ待機付き alpine の起動に失敗した");
    container.rm().await.expect("rm に失敗した");
}

/// `WaitFor::message_on_stderr` が Linux で成立すること。
///
/// Docker Engine API は STREAM_TYPE で stderr を分離するため、本当に stderr のみに反応する。
#[tokio::test]
async fn log_wait_message_on_stderr_succeeds() {
    let container = GenericImage::new("alpine", "latest")
        .with_wait_for(WaitFor::message_on_stderr("READY_ERR"))
        .with_cmd(["sh", "-c", "echo READY_ERR >&2; tail -f /dev/null"])
        .with_startup_timeout(Duration::from_secs(15))
        .start()
        .await
        .expect("stderr ログ待機付き alpine の起動に失敗した");
    container.rm().await.expect("rm に失敗した");
}

/// `WaitFor::message_on_either_std` が Linux で成立すること。
#[tokio::test]
async fn log_wait_message_on_either_std_succeeds() {
    let container = GenericImage::new("alpine", "latest")
        .with_wait_for(WaitFor::message_on_either_std("READY_EITHER"))
        .with_cmd(["sh", "-c", "echo READY_EITHER >&2; tail -f /dev/null"])
        .with_startup_timeout(Duration::from_secs(15))
        .start()
        .await
        .expect("either ログ待機付き alpine の起動に失敗した");
    container.rm().await.expect("rm に失敗した");
}

/// `stdout_to_vec` / `stderr_to_vec` が multiplex demux で stdout / stderr を分離すること。
#[tokio::test]
async fn stdout_to_vec_separates_stdout_stderr() {
    // stderr のマーカーを待機する (stdout の後に出力されるため、両方 flushed 済み)。
    let container = GenericImage::new("alpine", "latest")
        .with_wait_for(WaitFor::message_on_stderr("SEP_ERR"))
        .with_cmd([
            "sh",
            "-c",
            "echo SEP_OUT; echo SEP_ERR >&2; tail -f /dev/null",
        ])
        .with_startup_timeout(Duration::from_secs(15))
        .start()
        .await
        .expect("起動に失敗した");

    let stdout = container
        .stdout_to_vec()
        .await
        .expect("stdout_to_vec に失敗した");
    let stderr = container
        .stderr_to_vec()
        .await
        .expect("stderr_to_vec に失敗した");

    assert!(
        contains(&stdout, b"SEP_OUT"),
        "stdout に SEP_OUT を含むこと: {stdout:?}"
    );
    assert!(
        !contains(&stdout, b"SEP_ERR"),
        "stdout に SEP_ERR を含まないこと (demux 分離): {stdout:?}"
    );
    assert!(
        contains(&stderr, b"SEP_ERR"),
        "stderr に SEP_ERR を含むこと: {stderr:?}"
    );
    assert!(
        !contains(&stderr, b"SEP_OUT"),
        "stderr に SEP_OUT を含まないこと (demux 分離): {stderr:?}"
    );

    container.rm().await.expect("rm に失敗した");
}

/// `stdout(true)` の follow リーダーがログを読むこと。
#[tokio::test]
async fn stdout_follow_stream_reads_marker() {
    use tokio::io::AsyncReadExt;

    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["sh", "-c", "echo FOLLOW_MARKER; tail -f /dev/null"])
        .with_startup_timeout(Duration::from_secs(15))
        .start()
        .await
        .expect("起動に失敗した");

    let mut reader = container.stdout(true);
    let mut buf = [0u8; 4096];
    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !contains(&acc, b"FOLLOW_MARKER") {
        let n = tokio::time::timeout(Duration::from_secs(5), reader.read(&mut buf))
            .await
            .expect("follow 読み取りがタイムアウトした")
            .expect("follow 読み取りに失敗した");
        if n == 0 {
            break;
        }
        acc.extend_from_slice(&buf[..n]);
        assert!(
            tokio::time::Instant::now() < deadline,
            "FOLLOW_MARKER が現れない: {acc:?}"
        );
    }
    assert!(
        contains(&acc, b"FOLLOW_MARKER"),
        "follow リーダーが FOLLOW_MARKER を読むこと: {acc:?}"
    );

    container.rm().await.expect("rm に失敗した");
}

/// `with_log_consumer` が stdout / stderr フレームを行単位で受信すること。
#[tokio::test]
async fn log_consumer_receives_stdout_and_stderr_frames() {
    let consumer = RecordingConsumer::default();
    let recorded = consumer.clone();
    let container = GenericImage::new("alpine", "latest")
        .with_wait_for(WaitFor::message_on_stderr("CONS_ERR"))
        .with_cmd([
            "sh",
            "-c",
            "echo CONS_OUT; echo CONS_ERR >&2; tail -f /dev/null",
        ])
        .with_log_consumer(consumer)
        .with_startup_timeout(Duration::from_secs(15))
        .start()
        .await
        .expect("起動に失敗した");

    // consumer 配信は非同期なので、両フレームが届くまでポーリングする。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frames = recorded.frames();
        let has_out = frames
            .iter()
            .any(|f| matches!(f, LogFrame::StdOut(b) if contains(b, b"CONS_OUT")));
        let has_err = frames
            .iter()
            .any(|f| matches!(f, LogFrame::StdErr(b) if contains(b, b"CONS_ERR")));
        if has_out && has_err {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "CONS_OUT / CONS_ERR フレームが届かない: {frames:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    container.rm().await.expect("rm に失敗した");
}

/// メッセージが出ないままコンテナが終了すると、Log 待機が EOF (EndOfStream) で失敗すること。
///
/// Linux は `logs_terminated` (demux 終端) 経路で EOF を判定する。
#[tokio::test]
async fn log_wait_end_of_stream_when_message_never_appears() {
    let err = GenericImage::new("alpine", "latest")
        .with_wait_for(WaitFor::message_on_stdout("NEVER_APPEARS"))
        .with_cmd(["sh", "-c", "echo something; exit 0"])
        .with_startup_timeout(Duration::from_secs(15))
        .start()
        .await
        .expect_err("メッセージが出ずコンテナが終了するため失敗すること");
    assert!(
        err.to_string().contains("end of stream"),
        "EndOfStream エラーであること: {err}"
    );
}

/// stdout から `RESTART_MARKER-` 行の最後 (最新) を抽出する。
fn last_restart_marker(stdout: &[u8]) -> Option<Vec<u8>> {
    let mut found = None;
    for line in stdout.split(|&b| b == b'\n') {
        if line.starts_with(b"RESTART_MARKER-") {
            found = Some(line.to_vec());
        }
    }
    found
}

/// 再 start (stop → start) 後にログストリームが再武装され、新実行のログが読めること。
///
/// Docker Engine の `POST /containers/{id}/start` は create 時の cmd を再実行する。
/// UUID marker を使い、初回 start の marker A と再起動後の marker B が
/// 異なることを通じて、新規リーダーが新バッファに接続されることを検証する。
#[tokio::test]
async fn restart_rearms_log_stream() {
    // marker は kernel の UUID で一意化する。BusyBox の date は %N (nanosecond) 非対応で
    // 秒単位になり、同一秒内の再起動で marker が一致して flaky になるため。
    let container = GenericImage::new("alpine", "latest")
        .with_cmd([
            "sh",
            "-c",
            "echo RESTART_MARKER-$(cat /proc/sys/kernel/random/uuid); tail -f /dev/null",
        ])
        .with_startup_timeout(Duration::from_secs(15))
        .start()
        .await
        .expect("起動に失敗した");

    // 初回実行の marker A を取得する。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let marker_a = loop {
        let stdout = container
            .stdout_to_vec()
            .await
            .expect("stdout_to_vec に失敗した");
        if let Some(marker) = last_restart_marker(&stdout) {
            break marker;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "初回実行の marker が読めない: {stdout:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    container
        .stop_with_timeout(Some(0))
        .await
        .expect("stop に失敗した");
    container.start().await.expect("再起動に失敗した");

    // 再起動後は cmd が再実行され、marker A とは異なる marker B が新規リーダーで読めること。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let stdout = container
            .stdout_to_vec()
            .await
            .expect("stdout_to_vec に失敗した");
        if let Some(marker_b) = last_restart_marker(&stdout)
            && marker_b != marker_a
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "再起動後に異なる marker が読めない: {stdout:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    container.rm().await.expect("rm に失敗した");
}

/// stop 後に新規 follow リーダーを開くと EOF に達すること (demux 終了の間接観測)。
///
/// demux / consumer タスクの完了フラグは `pub(crate)` で統合テストからは読めないため、
/// 新規リーダーがハングせず EOF (`read_to_end` 完了) になることで終了を観測する。
#[tokio::test]
async fn stop_terminates_log_stream_for_new_reader() {
    use tokio::io::AsyncReadExt;

    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["sh", "-c", "echo BEFORE_STOP; tail -f /dev/null"])
        .with_startup_timeout(Duration::from_secs(15))
        .start()
        .await
        .expect("起動に失敗した");

    container
        .stop_with_timeout(Some(0))
        .await
        .expect("stop に失敗した");

    // stop 後の新規 follow リーダーはハングせず EOF に達すること。
    let mut reader = container.stdout(true);
    let mut all = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_to_end(&mut all))
        .await
        .expect("stop 後の新規リーダーが EOF に達しない (タイムアウト)")
        .expect("読み取りに失敗した");

    container.rm().await.expect("rm に失敗した");
}

/// 同期 API (`blocking` feature) の `stdout_to_vec` / `stderr_to_vec` が Linux で動作すること。
#[cfg(feature = "blocking")]
#[test]
fn sync_stdout_to_vec_returns_logs() {
    use shiguredo_container::SyncRunner;

    // AsyncRunner::start と同名のため、UFCS で SyncRunner::start を明示する。
    let request = GenericImage::new("alpine", "latest")
        .with_wait_for(WaitFor::message_on_stderr("SYNC_ERR"))
        .with_cmd([
            "sh",
            "-c",
            "echo SYNC_OUT; echo SYNC_ERR >&2; tail -f /dev/null",
        ])
        .with_startup_timeout(Duration::from_secs(15));
    let container = SyncRunner::start(request).expect("同期起動に失敗した");

    let stdout = container.stdout_to_vec().expect("stdout_to_vec に失敗した");
    let stderr = container.stderr_to_vec().expect("stderr_to_vec に失敗した");
    assert!(
        contains(&stdout, b"SYNC_OUT"),
        "同期 stdout が SYNC_OUT を含むこと: {stdout:?}"
    );
    assert!(
        contains(&stderr, b"SYNC_ERR"),
        "同期 stderr が SYNC_ERR を含むこと: {stderr:?}"
    );

    container.rm().expect("rm に失敗した");
}

/// `with_copy_to` (Data ソース) で投入したファイルを `copy_file_from` (Vec<u8>) で回収できること。
#[tokio::test]
async fn copy_to_data_and_copy_file_from_round_trip() {
    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .with_copy_to("/tmp/hello.txt", b"hello copy".to_vec())
        .start()
        .await
        .expect("起動に失敗した");

    let content: Vec<u8> = container
        .copy_file_from("/tmp/hello.txt", Vec::new())
        .await
        .expect("copy_file_from に失敗した");
    assert_eq!(
        content, b"hello copy",
        "Data コピーの往復で内容が一致すること"
    );

    container.rm().await.expect("rm に失敗した");
}

/// `with_copy_to` (File ソース) と `copy_file_from` (PathBuf ターゲット) の往復が成立すること。
#[tokio::test]
async fn copy_to_file_source_and_copy_file_from_pathbuf_target() {
    // ホストに一時ファイルを用意し、File ソースとして投入する。
    let host_src =
        std::env::temp_dir().join(format!("container-rs-copy-src-{}.txt", std::process::id()));
    tokio::fs::write(&host_src, b"from host file")
        .await
        .expect("ホストファイル書き込みに失敗した");

    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .with_copy_to("/tmp/from_host.txt", host_src.clone())
        .start()
        .await
        .expect("起動に失敗した");

    // PathBuf ターゲットでホストへ回収する。
    let host_dst =
        std::env::temp_dir().join(format!("container-rs-copy-dst-{}.txt", std::process::id()));
    container
        .copy_file_from("/tmp/from_host.txt", host_dst.clone())
        .await
        .expect("copy_file_from (PathBuf) に失敗した");
    let recovered = tokio::fs::read(&host_dst)
        .await
        .expect("回収ファイル読み込みに失敗した");
    assert_eq!(
        recovered, b"from host file",
        "File ソース + PathBuf ターゲットの往復で内容が一致すること"
    );

    let _ = tokio::fs::remove_file(&host_src).await;
    let _ = tokio::fs::remove_file(&host_dst).await;
    container.rm().await.expect("rm に失敗した");
}

/// `CopyTargetOptions` の mode / uid / gid がコンテナ内ファイルに反映されること。
#[tokio::test]
async fn copy_to_applies_mode_uid_gid() {
    use shiguredo_container::core::copy::CopyTargetOptions;

    let target = CopyTargetOptions {
        mode: 0o755,
        uid: 1000,
        gid: 1000,
        ..CopyTargetOptions::new("/tmp/script.sh")
    };
    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .with_copy_to(target, b"#!/bin/sh\necho hi\n".to_vec())
        .start()
        .await
        .expect("起動に失敗した");

    // stat の結果をコンテナ内ファイルに書き出し、copy_file_from で回収して検証する
    // (Linux の exec は stdout を取得できないため)。
    container
        .exec(ExecCommand::new([
            "sh",
            "-c",
            "stat -c '%a %u %g' /tmp/script.sh > /tmp/stat.out",
        ]))
        .await
        .expect("stat の exec に失敗した");
    let stat: Vec<u8> = container
        .copy_file_from("/tmp/stat.out", Vec::new())
        .await
        .expect("stat 結果の回収に失敗した");
    assert_eq!(
        stat, b"755 1000 1000\n",
        "mode / uid / gid が tar ヘッダどおりに反映されること"
    );

    container.rm().await.expect("rm に失敗した");
}

/// ディレクトリの `copy_file_from` は `IsDirectory` で拒否されること。
#[tokio::test]
async fn copy_file_from_directory_is_is_directory() {
    let container = start_alpine().await;

    let err = container
        .copy_file_from("/etc", Vec::new())
        .await
        .expect_err("ディレクトリの copy_file_from は失敗すること");
    assert!(
        err.to_string().contains("is a directory"),
        "IsDirectory エラーであること: {err}"
    );

    container.rm().await.expect("rm に失敗した");
}

/// 存在しない (削除済み) コンテナへの `copy_file_from` が `ContainerNotFound` になること。
#[tokio::test]
async fn copy_file_from_nonexistent_container_is_not_found() {
    let container = start_alpine().await;
    let id = container.id().to_string();

    // 外部で削除してコンテナを無くす。
    let status = std::process::Command::new("docker")
        .args(["rm", "-f", &id])
        .status()
        .expect("docker rm -f の実行に失敗した");
    assert!(status.success(), "docker rm -f が成功すること");

    let err = container
        .copy_file_from("/etc/hostname", Vec::new())
        .await
        .expect_err("削除済みコンテナの copy_file_from は失敗すること");
    match err {
        Error::Client(ClientError::ContainerNotFound(_)) => {}
        other => panic!("ContainerNotFound 以外のエラー: {other}"),
    }
}
