//! macOS の LogConsumer 配信タスクの dup FD 解放を、プロセス全体の FD 数を基準に
//! 検証する統合テスト。
//!
//! プロセス全体の FD 数を並列実行テストで検証するのは本質的に不安定である
//! (他テストのコンテナ start / teardown が FD 数を増減させる) ため、この検証を
//! 専用バイナリに分離して他テストの FD 干渉を排除する。cargo test はテスト
//! バイナリごとに別プロセスで実行されるため、バイナリを分けると FD 表の干渉が
//! 構造的に消える。
//! コンテナを start / teardown するテストは、FD 数検証を行わない場合でも FD 数を
//! 増減させて干渉するため、このバイナリにテストを追加しないこと。
//! (container_sync_drop_macos.rs の「同期 API を使う他のテストはこのバイナリに
//! 置かないこと」と同じ規約)

#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use shiguredo_container::core::logs::LogFrame;
use shiguredo_container::{AsyncRunner, GenericImage, ImageExt};

mod helpers;

/// 現在プロセスが開いている FD の数を数える。
///
/// LogConsumer 配信タスクの dup FD 解放の検証に使う。`/dev/fd` はプロセス自身の
/// FD 一覧なので、読んでいる最中の /dev/fd 自身も数える (相対比較のみに使う)。
/// このバイナリは FD 数検証専用で、他テストのコンテナ start / teardown による
/// FD 数の干渉を受けない。
fn open_fd_count() -> usize {
    std::fs::read_dir("/dev/fd")
        .expect("FD 一覧の取得に失敗した (検証を無言で無効化しないこと)")
        .count()
}

/// コンテナ自然終了後に LogConsumer 配信タスクが停止し、dup FD が解放されること。
#[tokio::test]
async fn xpc_alpine_log_consumer_stops_after_natural_exit() {
    if helpers::skip_if_ci() {
        return;
    }

    let marker = "log-consumer-exit-marker";
    let cmd = format!("echo {marker}; exit 0");
    let logs = Arc::new(Mutex::new(Vec::new()));
    let logs_clone = logs.clone();
    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["sh", "-c", &cmd])
        .with_log_consumer(move |record: &LogFrame| {
            logs_clone
                .lock()
                .expect("処理に失敗しないこと")
                .push((record.source(), record.bytes().to_vec()));
        })
        .start()
        .await
        .expect("alpine コンテナの起動に失敗した");

    // 配信タスクが spawn 済みの時点の FD 数を記録する。
    // 以降の検証はこの値を基準に「タスクの dup FD が解放された」ことを確認する。
    // タスクは stdout / stderr で各 1 dup の計 2 FD を持つため、基準から 2 減る。
    // コンテナ終了時には wait スレッドの XPC 接続も閉じるが、XPC 接続が FD を
    // 消費するかは libxpc 実装依存のため、判定の根拠にしない (FD を消費しても
    // 1 減にとどまり、タスク break なしでは 2 減に届かず誤成功しない)。
    let fd_after_start = open_fd_count();

    // マーカーが配信されるまで待つ (ポーリング。固定待ちにしない)。
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let delivered = {
            let captured = logs.lock().expect("処理に失敗しないこと");
            let delivered = captured
                .iter()
                .any(|(_, bytes)| String::from_utf8_lossy(bytes).contains(marker));
            assert!(
                std::time::Instant::now() < deadline,
                "マーカーが配信されること: {captured:?}"
            );
            delivered
        };
        if delivered {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // 自然終了 (exit code 記録) 後、猶予期間を過ぎると配信タスクが停止し、
    // dup FD が解放されて FD 数が spawn 時より減ることを確認する。
    // 停止は「EOF 観測 + exit code 初観測から 2 秒」で、wait_blocking 応答遅延と
    // EOF 観測周期 (最大 100ms) の分だけ遅れ得る。
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let fd_now = open_fd_count();
        if fd_now <= fd_after_start.saturating_sub(2) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "配信タスクの dup FD が解放されること (after_start={fd_after_start}, now={fd_now})"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    container.stop_with_timeout(Some(0)).await.ok();
    container.rm().await.ok();
}
