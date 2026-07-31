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
/// 明示 `rm().await` 直後、または Runtime 内 Drop 直後に使う。
/// `rm().await` は削除完了まで戻り、Runtime 内 Drop は `DROP_REMOVE_TIMEOUT` (5 秒)
/// 内で完了を待つため、いずれもポーリング無しの 1 ショット判定で足りる。
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
        "削除完了直後は 1 ショット inspect で不在であること: {id}"
    );
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
/// Runtime 内 Drop は削除を専用 std スレッドで実行し、`DROP_REMOVE_TIMEOUT` (5 秒)
/// を上限に完了を待つ。timeout 内に完了すれば `drop` 復帰時点で削除は終わっているため、
/// 1 ショット inspect で不在を確認する。
#[tokio::test]
async fn alpine_drop_inside_runtime_removes_container() {
    let container = start_alpine().await;
    let id = container.id().to_string();
    drop(container);
    assert_absent_once(&id).await;
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

/// Runtime 内の同期コンテキストから rm_blocking でコンテナが削除されること。
///
/// `rm_blocking` は `block_on` を使わず `remove_blocking` (同期 I/O) を直接
/// 呼び出すため、tokio Runtime 内の `spawn_blocking` 内から呼んでも deadlock しない。
/// `Ok` を返した直後に 1 ショット inspect で不在を確認する。
#[tokio::test]
async fn alpine_rm_blocking_inside_runtime_removes_container() {
    let container = start_alpine().await;
    let id = container.id().to_string();
    // Runtime 内の同期コンテキスト (spawn_blocking) から rm_blocking を呼ぶ
    tokio::task::spawn_blocking(move || {
        container.rm_blocking().expect("rm_blocking に失敗した");
    })
    .await
    .expect("spawn_blocking に失敗した");
    assert_absent_once(&id).await;
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
        .exit_code()
        .await
        .expect_err("exit_code は Linux で未実装であること");

    container.rm().await.expect("rm に失敗した");
}

/// Linux で get_bridge_ip_address がコンテナの IP アドレスを返すこと。
#[tokio::test]
async fn alpine_bridge_ip_address() {
    let container = start_alpine().await;

    let ip = container
        .get_bridge_ip_address()
        .await
        .expect("get_bridge_ip_address に失敗した");

    // ブリッジネットワークの IP はループバックでも unspecified でもないこと
    assert!(
        !ip.is_loopback() && !ip.is_unspecified(),
        "ブリッジ IP はループバックでも unspecified でもないこと: {ip}"
    );

    container.rm().await.expect("rm に失敗した");
}

/// Linux で exec の with_env_vars が環境変数を exec プロセスに渡すこと。
///
/// コンテナ作成時の env (`with_env_var`) と exec 時の env (`with_env_vars`) が
/// マージされ、同名キーは exec 側が優先されることを検証する。
#[tokio::test]
async fn alpine_exec_with_env_vars() {
    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .with_env_var("BASE_VAR", "base_val")
        .with_env_var("OVERRIDE_VAR", "original")
        .start()
        .await
        .expect("alpine コンテナの起動に失敗した");

    let mut result = container
        .exec(
            ExecCommand::new(["sh", "-c", "echo $BASE_VAR $EXEC_TEST_VAR $OVERRIDE_VAR"])
                .with_env_vars([
                    ("EXEC_TEST_VAR", "hello_env"),
                    ("OVERRIDE_VAR", "overridden"),
                ]),
        )
        .await
        .expect("with_env_vars 付き exec に失敗した");

    let stdout = result
        .stdout_to_vec()
        .await
        .expect("stdout_to_vec に失敗した");
    let stdout_str = String::from_utf8_lossy(&stdout);
    // コンテナ env の BASE_VAR が保持され、exec 分の EXEC_TEST_VAR が追加され、
    // 同名キーの OVERRIDE_VAR は exec 側が優先されること
    assert!(
        stdout_str.contains("base_val"),
        "コンテナ env の BASE_VAR が保持されること: {stdout_str}"
    );
    assert!(
        stdout_str.contains("hello_env"),
        "exec env の EXEC_TEST_VAR が渡されること: {stdout_str}"
    );
    assert!(
        stdout_str.contains("overridden"),
        "同名キーは exec 側が優先されること: {stdout_str}"
    );

    container.rm().await.expect("rm に失敗した");
}

/// Linux で exec の stdout が取得できること。
#[tokio::test]
async fn alpine_exec_captures_stdout() {
    let container = start_alpine().await;

    let mut result = container
        .exec(ExecCommand::new(["echo", "hello_stdout"]))
        .await
        .expect("exec に失敗した");

    let stdout = result
        .stdout_to_vec()
        .await
        .expect("stdout_to_vec に失敗した");
    assert!(
        stdout.windows(12).any(|w| w == b"hello_stdout"),
        "stdout に hello_stdout が含まれること: {:?}",
        String::from_utf8_lossy(&stdout)
    );

    container.rm().await.expect("rm に失敗した");
}

/// Linux で exec の stderr が取得できること。
#[tokio::test]
async fn alpine_exec_captures_stderr() {
    let container = start_alpine().await;

    let mut result = container
        .exec(ExecCommand::new(["sh", "-c", "echo hello_stderr >&2"]))
        .await
        .expect("exec に失敗した");

    let stderr = result
        .stderr_to_vec()
        .await
        .expect("stderr_to_vec に失敗した");
    assert!(
        stderr.windows(12).any(|w| w == b"hello_stderr"),
        "stderr に hello_stderr が含まれること: {:?}",
        String::from_utf8_lossy(&stderr)
    );

    container.rm().await.expect("rm に失敗した");
}

/// Linux で CmdWaitFor::StdOutMessage が動作すること。
#[tokio::test]
async fn alpine_exec_wait_for_stdout_message() {
    let container = start_alpine().await;

    let mut result = container
        .exec(
            ExecCommand::new(["echo", "READY_TOKEN"])
                .with_cmd_ready_condition(CmdWaitFor::message_on_stdout("READY_TOKEN")),
        )
        .await
        .expect("StdOutMessage 待ちが成功すること");

    let stdout = result
        .stdout_to_vec()
        .await
        .expect("stdout_to_vec に失敗した");
    assert!(
        stdout.windows(11).any(|w| w == b"READY_TOKEN"),
        "stdout に READY_TOKEN が含まれること"
    );

    container.rm().await.expect("rm に失敗した");
}

/// Linux で CmdWaitFor::StdErrMessage が動作すること。
#[tokio::test]
async fn alpine_exec_wait_for_stderr_message() {
    let container = start_alpine().await;

    let mut result = container
        .exec(
            ExecCommand::new(["sh", "-c", "echo ERR_TOKEN >&2"])
                .with_cmd_ready_condition(CmdWaitFor::message_on_stderr("ERR_TOKEN")),
        )
        .await
        .expect("StdErrMessage 待ちが成功すること");

    let stderr = result
        .stderr_to_vec()
        .await
        .expect("stderr_to_vec に失敗した");
    assert!(
        stderr.windows(9).any(|w| w == b"ERR_TOKEN"),
        "stderr に ERR_TOKEN が含まれること"
    );

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

/// 初回 start 経路で、初期プロセスが start 前に投入された新規ファイルを読めること。
#[tokio::test]
async fn copy_to_visible_before_initial_process() {
    let marker = "COPY_BEFORE_START_OK";
    let container = GenericImage::new("alpine", "latest")
        .with_wait_for(WaitFor::message_on_stdout(marker))
        .with_cmd([
            "sh",
            "-c",
            "test -f /tmp/payload.txt && cat /tmp/payload.txt && exec tail -f /dev/null",
        ])
        .with_copy_to("/tmp/payload.txt", format!("{marker}\n").into_bytes())
        .with_startup_timeout(Duration::from_secs(15))
        .start()
        .await
        .expect("起動前コピーの可視性検証に失敗した");
    container.rm().await.expect("rm に失敗した");
}

/// 初回 start 経路で、初期プロセスが start 前に上書きされた既存ファイルの新内容を読むこと。
#[tokio::test]
async fn copy_to_overwrite_visible_before_initial_process() {
    let marker = "COPY_OVERWRITE_BEFORE_START_OK";
    let container = GenericImage::new("alpine", "latest")
        .with_wait_for(WaitFor::message_on_stdout(marker))
        .with_cmd([
            "sh",
            "-c",
            "test -f /etc/motd && cat /etc/motd && exec tail -f /dev/null",
        ])
        .with_copy_to("/etc/motd", format!("{marker}\n").into_bytes())
        .with_startup_timeout(Duration::from_secs(15))
        .start()
        .await
        .expect("既存パス上書きの起動前可視性検証に失敗した");
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

/// 存在しない親ディレクトリ配下への Data 投入が成功し、中間 dir の mode / uid / gid も反映されること。
#[tokio::test]
async fn copy_to_creates_missing_parents_for_data() {
    use shiguredo_container::core::copy::CopyTargetOptions;

    let target = CopyTargetOptions {
        mode: 0o640,
        uid: 1000,
        gid: 1000,
        ..CopyTargetOptions::new("/var/container-rs-copy-missing/a.txt")
    };
    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .with_copy_to(target, b"parent-ok\n".to_vec())
        .start()
        .await
        .expect("親ディレクトリ自動作成付きの起動に失敗した");

    let content: Vec<u8> = container
        .copy_file_from("/var/container-rs-copy-missing/a.txt", Vec::new())
        .await
        .expect("投入ファイルの回収に失敗した");
    assert_eq!(
        content, b"parent-ok\n",
        "親作成後のファイル内容が一致すること"
    );

    container
        .exec(ExecCommand::new([
            "sh",
            "-c",
            "stat -c '%a %u %g' /var/container-rs-copy-missing > /tmp/dir.stat && \
             stat -c '%a %u %g' /var/container-rs-copy-missing/a.txt > /tmp/file.stat",
        ]))
        .await
        .expect("stat の exec に失敗した");
    let dir_stat: Vec<u8> = container
        .copy_file_from("/tmp/dir.stat", Vec::new())
        .await
        .expect("中間 dir の stat 回収に失敗した");
    let file_stat: Vec<u8> = container
        .copy_file_from("/tmp/file.stat", Vec::new())
        .await
        .expect("ファイルの stat 回収に失敗した");
    assert_eq!(
        dir_stat, b"755 1000 1000\n",
        "中間ディレクトリの mode / uid / gid が規則どおりであること"
    );
    assert_eq!(
        file_stat, b"640 1000 1000\n",
        "ファイルの mode / uid / gid が target どおりであること"
    );

    container.rm().await.expect("rm に失敗した");
}

/// ホストディレクトリの再帰投入が、存在しない親配下でも成功すること。
#[tokio::test]
async fn copy_to_directory_source_with_missing_parents() {
    use shiguredo_container::core::copy::CopyTargetOptions;
    use std::fs;

    let host_dir =
        std::env::temp_dir().join(format!("container-rs-copy-dir-{}", std::process::id()));
    let nested = host_dir.join("nested");
    fs::create_dir_all(&nested).expect("ホスト一時ディレクトリの作成に失敗した");
    fs::write(host_dir.join("root.txt"), b"root\n").expect("root.txt の書き込みに失敗した");
    fs::write(nested.join("child.txt"), b"child\n").expect("child.txt の書き込みに失敗した");
    // 空サブディレクトリも残す。
    fs::create_dir_all(host_dir.join("empty")).expect("空ディレクトリの作成に失敗した");

    let target = CopyTargetOptions {
        mode: 0o600,
        uid: 1000,
        gid: 1000,
        ..CopyTargetOptions::new("/var/container-rs-copy-missing-dir")
    };
    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .with_copy_to(target, host_dir.clone())
        .start()
        .await
        .expect("ディレクトリ投入付きの起動に失敗した");

    let root: Vec<u8> = container
        .copy_file_from("/var/container-rs-copy-missing-dir/root.txt", Vec::new())
        .await
        .expect("root.txt の回収に失敗した");
    let child: Vec<u8> = container
        .copy_file_from(
            "/var/container-rs-copy-missing-dir/nested/child.txt",
            Vec::new(),
        )
        .await
        .expect("child.txt の回収に失敗した");
    assert_eq!(root, b"root\n");
    assert_eq!(child, b"child\n");

    container
        .exec(ExecCommand::new([
            "sh",
            "-c",
            "test -d /var/container-rs-copy-missing-dir/empty && \
             stat -c '%a %u %g' /var/container-rs-copy-missing-dir > /tmp/root.stat && \
             stat -c '%a %u %g' /var/container-rs-copy-missing-dir/root.txt > /tmp/file.stat",
        ]))
        .await
        .expect("空ディレクトリと stat の確認に失敗した");
    let root_stat: Vec<u8> = container
        .copy_file_from("/tmp/root.stat", Vec::new())
        .await
        .expect("投入ルート dir の stat 回収に失敗した");
    let file_stat: Vec<u8> = container
        .copy_file_from("/tmp/file.stat", Vec::new())
        .await
        .expect("投入ファイルの stat 回収に失敗した");
    assert_eq!(
        root_stat, b"755 1000 1000\n",
        "投入ルートディレクトリの mode / uid / gid が規則どおりであること"
    );
    assert_eq!(
        file_stat, b"600 1000 1000\n",
        "配下ファイルに target.mode / uid / gid が適用されること"
    );

    container.rm().await.expect("rm に失敗した");
    let _ = fs::remove_dir_all(&host_dir);
}

/// ディレクトリソース内の symlink は PathNameError で拒否されること。
#[tokio::test]
async fn copy_to_directory_with_symlink_is_rejected() {
    use std::fs;
    use std::os::unix::fs::symlink;

    let host_dir =
        std::env::temp_dir().join(format!("container-rs-copy-symlink-{}", std::process::id()));
    fs::create_dir_all(&host_dir).expect("ホスト一時ディレクトリの作成に失敗した");
    fs::write(host_dir.join("real.txt"), b"x\n").expect("real.txt の書き込みに失敗した");
    symlink("real.txt", host_dir.join("link.txt")).expect("symlink の作成に失敗した");

    let err = GenericImage::new("alpine", "latest")
        .with_cmd(["tail", "-f", "/dev/null"])
        .with_copy_to("/tmp/symlink-src", host_dir.clone())
        .start()
        .await
        .expect_err("symlink を含むディレクトリ投入は失敗すること");
    assert!(
        err.to_string().contains("symlink") || err.to_string().contains("copy path error"),
        "symlink 拒否のエラーであること: {err}"
    );

    let _ = fs::remove_dir_all(&host_dir);
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

/// `with_health_check` + `WaitFor::healthcheck` で healthy まで到達すること。
#[tokio::test]
async fn healthcheck_reaches_healthy() {
    use shiguredo_container::Healthcheck;

    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["sh", "-c", "touch /tmp/ok; while true; do sleep 60; done"])
        .with_health_check(
            Healthcheck::cmd_shell("test -f /tmp/ok")
                .with_interval(Duration::from_millis(500))
                .with_retries(3),
        )
        .with_ready_conditions(vec![WaitFor::healthcheck()])
        .with_startup_timeout(Duration::from_secs(30))
        .start()
        .await
        .expect("healthcheck 到達の起動に失敗した");

    container.rm().await.expect("rm に失敗した");
}

/// 同期 API でも healthcheck 到達が成立すること。
#[cfg(feature = "blocking")]
#[test]
fn sync_healthcheck_reaches_healthy() {
    use shiguredo_container::{Healthcheck, SyncRunner};

    let request = GenericImage::new("alpine", "latest")
        .with_cmd(["sh", "-c", "touch /tmp/ok; while true; do sleep 60; done"])
        .with_health_check(
            Healthcheck::cmd_shell("test -f /tmp/ok")
                .with_interval(Duration::from_millis(500))
                .with_retries(3),
        )
        .with_ready_conditions(vec![WaitFor::healthcheck()])
        .with_startup_timeout(Duration::from_secs(30));
    let container = SyncRunner::start(request).expect("同期 healthcheck 起動に失敗した");
    container.rm().expect("rm に失敗した");
}

/// 常に失敗する healthcheck が `Unhealthy` になること。
#[tokio::test]
async fn healthcheck_unhealthy_returns_error() {
    use shiguredo_container::Healthcheck;
    use shiguredo_container::core::error::WaitContainerError;

    let err = GenericImage::new("alpine", "latest")
        .with_cmd(["sh", "-c", "while true; do sleep 60; done"])
        .with_health_check(
            Healthcheck::cmd_shell("false")
                .with_interval(Duration::from_millis(500))
                .with_timeout(Duration::from_secs(1))
                .with_retries(1),
        )
        .with_ready_conditions(vec![WaitFor::healthcheck()])
        .with_startup_timeout(Duration::from_secs(30))
        .start()
        .await
        .expect_err("unhealthy はエラーになること");

    match err {
        Error::WaitContainer(WaitContainerError::Unhealthy(_)) => {}
        other => panic!("Unhealthy 以外のエラー: {other}"),
    }
}

/// healthcheck 未設定で `WaitFor::healthcheck` を指定すると `HealthCheckNotConfigured` になること。
#[tokio::test]
async fn healthcheck_not_configured_returns_error() {
    use shiguredo_container::core::error::WaitContainerError;

    let err = GenericImage::new("alpine", "latest")
        .with_cmd(["sh", "-c", "while true; do sleep 60; done"])
        .with_ready_conditions(vec![WaitFor::healthcheck()])
        .with_startup_timeout(Duration::from_secs(30))
        .start()
        .await
        .expect_err("healthcheck 未設定はエラーになること");

    match err {
        Error::WaitContainer(WaitContainerError::HealthCheckNotConfigured(_)) => {}
        other => panic!("HealthCheckNotConfigured 以外のエラー: {other}"),
    }
}

/// probe が長くかかる healthcheck で `startup_timeout` が発火すること。
#[tokio::test]
async fn healthcheck_startup_timeout() {
    use shiguredo_container::Healthcheck;
    use shiguredo_container::core::error::WaitContainerError;

    let err = GenericImage::new("alpine", "latest")
        .with_cmd(["sh", "-c", "while true; do sleep 60; done"])
        .with_health_check(
            Healthcheck::cmd_shell("sleep 120")
                .with_interval(Duration::from_secs(1))
                .with_timeout(Duration::from_secs(60))
                .with_retries(10)
                .with_start_period(Duration::from_secs(120)),
        )
        .with_ready_conditions(vec![WaitFor::healthcheck()])
        .with_startup_timeout(Duration::from_secs(10))
        .start()
        .await
        .expect_err("startup_timeout はエラーになること");

    match err {
        Error::WaitContainer(WaitContainerError::StartupTimeout { .. }) => {}
        other => panic!("StartupTimeout 以外のエラー: {other}"),
    }
}
