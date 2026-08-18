//! Linux: 存在しないイメージの `pull_image` がストリーム内エラーを返すこと。
//!
//! macOS ではコンパイル対象外。Linux / CI (`test-linux-docker`) で必ず実行する。

#![cfg(target_os = "linux")]

use shiguredo_container::core::error::ClientError;
use shiguredo_container::{AsyncRunner, Error, GenericImage};

/// 存在しないイメージの pull が `ClientError::Other` になること。
#[tokio::test]
async fn pull_nonexistent_image_returns_stream_error() {
    let err = GenericImage::new("shiguredo-container-rs-nonexistent", "no-such-tag")
        .pull_image()
        .await
        .expect_err("存在しないイメージの pull は失敗すること");
    match err {
        Error::Client(ClientError::Other(msg)) => {
            assert!(!msg.is_empty(), "ストリームエラーメッセージが空でないこと");
        }
        other => panic!("ClientError::Other 以外のエラー: {other}"),
    }
}
