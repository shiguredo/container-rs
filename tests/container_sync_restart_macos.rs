//! 同期 API の stop → start 再起動を検証する統合テスト。
//!
//! `test_container_sync_drop_macos.rs` は「同期 API を使う他のテストを置かない」制約があるため、
//! 再起動シナリオは本ファイルに分離する。

#![cfg(all(target_os = "macos", feature = "blocking"))]

use shiguredo_container::{
    ExecCommand, GenericImage, ImageExt, SyncRunner, core::wait::CmdWaitFor,
};

mod helpers;

/// 同期 API で stop → start 後に running かつ exec 疎通できること。
#[test]
fn sync_stop_start_restarts_container() {
    if helpers::skip_if_ci() {
        return;
    }

    let container = GenericImage::new("alpine", "latest")
        .with_cmd(["sleep", "30"])
        .start()
        .expect("起動に失敗した");

    // 再起動シナリオの検証が目的なので、SIGTERM 猶予待ちは避ける。
    container
        .stop_with_timeout(Some(0))
        .expect("停止に失敗した");
    assert!(
        !container
            .is_running()
            .expect("停止後の実行状態取得に失敗した"),
        "stop 後は running でないこと"
    );

    container.start().expect("再 start に失敗した");
    assert!(
        container
            .is_running()
            .expect("再起動後の実行状態取得に失敗した"),
        "再 start 後は running であること"
    );

    container
        .exec(ExecCommand::new(["true"]).with_cmd_ready_condition(CmdWaitFor::exit_code(0)))
        .expect("再 start 後の exec に失敗した");

    container.stop_with_timeout(Some(0)).ok();
    container.rm().ok();
}
