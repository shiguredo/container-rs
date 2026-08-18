//! watchdog feature の統合テスト。
//!
//! 親テストが自分自身のテストバイナリを subprocess として起動し、
//! コンテナ起動後に SIGKILL で殺す。Drop は走らないため、掃除は
//! watchdog (reaper プロセス) だけが担う。

#![cfg(all(target_os = "macos", feature = "watchdog"))]

use std::io::BufRead;

mod helpers;

/// 親テストから subprocess として起動される犠牲プロセス。
/// コンテナ ID を stdout に出してから SIGKILL されるまで待つ。
#[tokio::test]
async fn watchdog_victim() {
    if std::env::var_os("WATCHDOG_VICTIM").is_none() {
        return;
    }
    use shiguredo_container::{GenericImage, ImageExt, runners::AsyncRunner};

    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["sleep", "300"])
        .start()
        .await
        .expect("犠牲プロセスのコンテナ起動に失敗した");

    println!("VICTIM_ID={}", container.id());
    use std::io::Write;
    std::io::stdout()
        .flush()
        .expect("標準出力の flush に失敗した");

    // SIGKILL されるまで待つ。container はスコープ内に生きたままなので
    // Drop による削除は走らない (クラッシュの再現)。
    std::thread::sleep(std::time::Duration::from_secs(120));
}

#[test]
fn watchdog_cleans_up_container_after_sigkill() {
    if helpers::skip_if_ci() {
        return;
    }

    // 自分自身のテストバイナリで犠牲テストだけを起動する。
    let exe = std::env::current_exe().expect("テスト実行ファイルのパス取得に失敗した");
    let mut child = std::process::Command::new(exe)
        .args(["--exact", "watchdog_victim", "--nocapture"])
        .env("WATCHDOG_VICTIM", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("犠牲プロセスの起動に失敗した");

    // 犠牲プロセスが起動したコンテナの ID を stdout から読む。
    let stdout = child
        .stdout
        .take()
        .expect("犠牲プロセスの標準出力を取得できない");
    let mut reader = std::io::BufReader::new(stdout);
    let mut id = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if let Some(rest) = line.trim().strip_prefix("VICTIM_ID=") {
            id = Some(rest.to_string());
            break;
        }
    }
    let id = match id {
        Some(id) => id,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("犠牲プロセスが VICTIM_ID を出力しなかった");
        }
    };

    // クラッシュを再現: SIGKILL。プロセス内のクリーンアップは一切走らない。
    child.kill().expect("犠牲プロセスへの SIGKILL に失敗した");
    let _ = child.wait();

    // reaper が pipe EOF で親の死を検知し、コンテナを削除するのを待つ。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let out = std::process::Command::new("container")
            .args(["ls", "--all", "--format", "json"])
            .output()
            .expect("container ls の実行に失敗した");
        let listing = String::from_utf8_lossy(&out.stdout).to_string();
        if !listing.contains(&id) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            // 掃除してから失敗させる。
            let _ = std::process::Command::new("container")
                .args(["rm", "--force", &id])
                .output();
            panic!("SIGKILL 後に watchdog がコンテナを削除すること: {id}");
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}
