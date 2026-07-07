//! README クイックスタート相当: nginx + `WaitFor::http` + 素の HTTP/1.1 GET。
//!
//! ```text
//! cargo test --test test_nginx_http11 --features http_wait_plain -- --nocapture
//! ```
//!
//! published port 経由のため、macOS では Local Network Privacy の許可が必要。
//! ローカルでは `RUN_HOST_NETWORK_TESTS=1` を付けて実行する。

mod helpers;

#[cfg(all(target_os = "macos", feature = "http_wait_plain"))]
mod with_http_wait {
    use std::time::Duration;

    use shiguredo_container::{
        AsyncRunner, GenericImage, ImageExt, WaitFor,
        core::{IntoContainerPort, wait::HttpWaitStrategy},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// ホストからコンテナへのネットワーク接続に依存するテストをスキップする。
    fn skip_unless_host_network() -> bool {
        if std::env::var("RUN_HOST_NETWORK_TESTS").as_deref() == Ok("1") {
            return false;
        }
        eprintln!(
            "スキップ: ホストからコンテナへの接続は macOS Local Network Privacy に依存する。\
             許可済みホストでは RUN_HOST_NETWORK_TESTS=1 を指定すること"
        );
        true
    }

    /// nginx を起動し、公開ポートへ HTTP/1.1 GET して 200 と本文を確認する。
    #[tokio::test]
    async fn nginx_http11_get_returns_200() {
        if super::helpers::skip_if_ci() || skip_unless_host_network() {
            return;
        }

        // nginx を公開ポート 80 で起動し、HTTP 待機で準備完了を確認する。
        let container = GenericImage::new("nginx", "latest")
            .with_exposed_port(80.tcp())
            .with_wait_for(WaitFor::http(
                HttpWaitStrategy::new("/")
                    .with_port(80.tcp())
                    .with_expected_status_code(200_u16),
            ))
            .with_startup_timeout(Duration::from_secs(120))
            .start()
            .await
            .expect("nginx の起動に失敗した");

        let host_port = container
            .get_host_port_ipv4(80.tcp())
            .await
            .expect("公開ホストポートの解決に失敗した");

        // 依存クレートを増やさず、素の HTTP/1.1 で叩く。
        let (status, body) = http_get("127.0.0.1", host_port, "/")
            .await
            .expect("HTTP/1.1 GET に失敗した");

        assert_eq!(status, 200, "nginx は 200 を返すこと");
        assert!(
            body.to_ascii_lowercase().contains("nginx"),
            "レスポンス本文に nginx が含まれること: {body}"
        );

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("nginx の停止に失敗した");
        container.rm().await.expect("nginx の削除に失敗した");
    }

    /// `host:port` へ HTTP/1.1 GET し、(ステータスコード, レスポンス全文) を返す。
    async fn http_get(host: &str, port: u16, path: &str) -> std::io::Result<(u16, String)> {
        let mut stream = tokio::net::TcpStream::connect((host, port)).await?;
        let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await?;

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await?;
        let text = String::from_utf8_lossy(&buf).into_owned();
        let status = text
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .unwrap_or(0);
        Ok((status, text))
    }
}
