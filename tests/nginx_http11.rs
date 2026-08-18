//! README クイックスタート相当: nginx + `WaitFor::http` + 素の HTTP/1.1 GET。
//!
//! ```text
//! cargo test --test nginx_http11 --features http_wait_plain -- --nocapture
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

    /// 10 MiB の決定論的バイナリデータを生成する。
    ///
    /// 各バイトはオフセットを 251 (素数) で剰余した値。全バイト値 (0x00-0xFA) が
    /// 出現するため、パターンに偏りが無い。
    fn generate_10mb_binary() -> Vec<u8> {
        const SIZE: usize = 10 * 1024 * 1024;
        (0..SIZE).map(|i| (i % 251) as u8).collect()
    }

    /// レスポンスヘッダーから `Content-Length` の値をパースする。
    fn parse_content_length(head: &[u8]) -> Option<usize> {
        let head = std::str::from_utf8(head).ok()?;
        for line in head.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            if k.eq_ignore_ascii_case("content-length") {
                return v.trim().parse().ok();
            }
        }
        None
    }

    /// 遅い消費者 (8 KiB 読み + 読み合間に 1 ms 待機) として GET し、
    /// (`Content-Length`, 受信済みボディ長) を返す。
    ///
    /// サーバーの送信速度に追いつかない消費者を模擬する。読みの間に待機を
    /// 入れることで、フォワーダーのバッファが溢れた場合に背圧をかけず
    /// データを切り捨てる挙動を検出しやすくする。
    ///
    /// フォワーダーが切断せず接続を保持し続ける環境でもテストが無限に
    /// 待たないよう、全体に 60 秒のデッドラインを設ける。
    async fn slow_consumer_get(host: &str, port: u16) -> std::io::Result<(usize, usize)> {
        tokio::time::timeout(Duration::from_secs(60), async {
            let mut stream = tokio::net::TcpStream::connect((host, port)).await?;
            stream
                .write_all(
                    b"GET /10mb.bin HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .await?;

            let mut chunk = vec![0u8; 8192];
            let mut all = Vec::new();
            loop {
                let n = stream.read(&mut chunk).await?;
                if n == 0 {
                    break;
                }
                all.extend_from_slice(&chunk[..n]);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            let header_end = all
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| p + 4)
                .unwrap_or(0);
            let content_length = parse_content_length(&all[..header_end]).unwrap_or(0);
            let body_len = all.len() - header_end;
            Ok((content_length, body_len))
        })
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "slow consumer GET timed out")
        })?
    }

    /// macOS の published port フォワーダーが大容量レスポンスを途中切断する再現テスト。
    ///
    /// nginx を published port で起動し、10 MiB の静的ファイルを遅い消費者
    /// (8 KiB 読み + 読み合間に 1 ms 待機) で GET する。Apple 側のポートフォワーダーは
    /// バッファ溢れ時に背圧をかけずデータを切り捨てると推定されており、
    /// 消費者が遅いと Content-Length に満たない位置で EOF (FIN) になり、
    /// クライアントが不完全なレスポンスを正常な EOF として受信する。
    ///
    /// 切断は確率的 (環境により再現率は変わる) なため、このテストは切断を観測したら
    /// その旨を報告し、観測しなくても失敗にはしない (実行して切断の有無を確認する
    /// 道具として使う)。Apple 側で修正された場合は、完全受信を期待する検証
    /// (切断があれば失敗) に変更すること。
    ///
    /// 同一バイナリの他のテストと並列実行され得る。ホストポートの自動割当は
    /// bind(0) → 即 release のため衝突し得る。起動に失敗する場合は
    /// `--test-threads=1` で個別実行して確認すること。
    #[tokio::test]
    async fn published_port_large_response_truncates_for_slow_consumer() {
        if super::helpers::skip_if_ci() || skip_unless_host_network() {
            return;
        }

        // nginx を公開ポート 80 で起動し、10 MiB の静的ファイルを html ディレクトリへ投入する。
        // 投入は start 内で完了する (macOS は start_process 後の containerCopyIn)。
        let container = GenericImage::new("nginx", "latest")
            .with_exposed_port(80.tcp())
            .with_wait_for(WaitFor::http(
                HttpWaitStrategy::new("/")
                    .with_port(80.tcp())
                    .with_expected_status_code(200_u16),
            ))
            .with_startup_timeout(Duration::from_secs(120))
            .with_copy_to("/usr/share/nginx/html/10mb.bin", generate_10mb_binary())
            .start()
            .await
            .expect("nginx の起動に失敗した");

        let host_port = container
            .get_host_port_ipv4(80.tcp())
            .await
            .expect("公開ホストポートの解決に失敗した");

        // 遅い消費者で GET し、Content-Length と受信済みボディ長を比較する。
        let (content_length, body_len) = slow_consumer_get("127.0.0.1", host_port)
            .await
            .expect("HTTP/1.1 GET に失敗した");

        if content_length == 0 {
            eprintln!(
                "観測: Content-Length をパースできなかった (受信 {body_len} バイト)。\
                 レスポンスの形式を確認すること"
            );
        } else if content_length != 10 * 1024 * 1024 {
            eprintln!(
                "観測: 想定外のレスポンス (Content-Length {content_length}、受信 {body_len} バイト)。\
                 10mb.bin の配信状態を確認すること"
            );
        } else if content_length > body_len {
            eprintln!(
                "観測: published port で {body_len}/{content_length} バイト受信後に EOF \
                 (途中切断を再現した)"
            );
        } else {
            eprintln!(
                "観測: published port で {content_length} バイトを完全受信 \
                 (今回は再現しなかった。数回実行して確認すること)"
            );
        }

        container
            .stop_with_timeout(Some(0))
            .await
            .expect("nginx の停止に失敗した");
        container.rm().await.expect("nginx の削除に失敗した");
    }
}
