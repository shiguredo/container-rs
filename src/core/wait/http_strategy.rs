//! HTTP 待機戦略。元の 0.27 の `core::wait::http_strategy` に相当。
//!
//! 本家は reqwest を使うが、shiguredo は依存最小方針のため
//! `shiguredo_http11` + `tokio::net::TcpStream` で plain HTTP のみ対応する (TLS 非対応)。
//! そのため reqwest の型を受け取る API (`with_client` / `with_method(reqwest::Method)` /
//! `with_response_matcher(reqwest::Response)`) は shiguredo 独自の型に置き換えている。

use std::{fmt, sync::Arc, time::Duration};

use base64ct::{Base64, Encoding};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    ContainerAsync, Image,
    core::{
        client::Client,
        error::{Result, WaitContainerError},
        host::Host,
        ports::ContainerPort,
    },
};

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HTTP_RESPONSE_BODY_BYTES: usize = 1024 * 1024;

/// HTTP 待機のエラー。元の 0.27 の `HttpWaitError` に相当。
#[derive(Debug)]
pub enum HttpWaitError {
    /// コンテナに公開ポートが無い。
    NoExposedPortsForHttpWait,
    /// response matcher が未設定。
    /// 本家は待機ループ内で `TestcontainersError::other` を返すが、shiguredo は型付きにする。
    NoResponseMatcher,
    /// HTTP リクエストの構築に失敗した。
    RequestBuild(String),
}

impl fmt::Display for HttpWaitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpWaitError::NoExposedPortsForHttpWait => {
                write!(f, "container has no exposed ports")
            }
            HttpWaitError::NoResponseMatcher => {
                write!(
                    f,
                    "no response matcher provided for HTTP wait strategy (use with_expected_status_code or with_response_matcher)"
                )
            }
            HttpWaitError::RequestBuild(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for HttpWaitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

/// response matcher に渡す HTTP レスポンス。
///
/// 本家は `reqwest::Response` を渡すが、shiguredo は受信済みのレスポンスを保持する
/// 独自型を渡す (body 読み出しが不要なため matcher は同期関数でよい)。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    /// HTTP ステータスコードを返す。
    pub fn status(&self) -> u16 {
        self.status
    }

    /// 受信順のヘッダ一覧を返す。
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// 指定名のヘッダ値を返す (名前は大文字小文字を無視)。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// レスポンスボディを返す。
    ///
    /// 保持量は最大 1 MiB であり、それを超えた部分は切り詰められる。
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

type ResponseMatcher = Arc<dyn Fn(&HttpResponse) -> bool + Send + Sync + 'static>;

#[derive(Clone)]
enum Auth {
    Basic { username: String, password: String },
    Bearer(String),
}

/// HTTP レスポンスによる待機戦略。元の 0.27 の `HttpWaitStrategy` と同等の API。
#[derive(Clone)]
pub struct HttpWaitStrategy {
    path: String,
    port: Option<ContainerPort>,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    auth: Option<Auth>,
    response_matcher: Option<ResponseMatcher>,
    poll_interval: Duration,
    request_timeout: Duration,
}

impl HttpWaitStrategy {
    /// 指定パスに対する待機戦略を作る (デフォルトは GET)。
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            port: None,
            method: "GET".to_string(),
            headers: Vec::new(),
            body: None,
            auth: None,
            response_matcher: None,
            poll_interval: Duration::from_millis(100),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// リクエスト先のコンテナポートを指定する。
    ///
    /// 対応するホスト側ポートに接続する。未指定なら最初の公開ポートを使う。
    pub fn with_port(mut self, port: ContainerPort) -> Self {
        self.port = Some(port);
        self
    }

    /// HTTP メソッドを指定する。本家は `reqwest::Method` だが shiguredo は文字列。
    pub fn with_method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into();
        self
    }

    /// リクエストヘッダを追加する。
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// リクエストボディを設定する。
    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Basic 認証を設定する。既に設定済みの認証を上書きする。
    pub fn with_basic_auth(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.auth = Some(Auth::Basic {
            username: username.into(),
            password: password.into(),
        });
        self
    }

    /// Bearer トークンを設定する。既に設定済みの認証を上書きする。
    pub fn with_bearer_auth(mut self, token: impl Into<String>) -> Self {
        self.auth = Some(Auth::Bearer(token.into()));
        self
    }

    /// ポーリング間隔を設定する。
    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    /// HTTP リクエスト 1 回のタイムアウトを設定する。
    ///
    /// 既定値は 10 秒で、接続・送信・レスポンスヘッダおよびボディ受信全体に適用する。
    /// `Duration::ZERO` は即座にタイムアウトとして扱われ、待機ループでリトライする。
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// 期待するステータスコードを設定する。
    /// `with_response_matcher(|response| response.status() == status)` のショートカット。
    pub fn with_expected_status_code(self, status: impl Into<u16>) -> Self {
        let status = status.into();
        self.with_response_matcher(move |response| response.status() == status)
    }

    /// レスポンスの一致条件を設定する。`true` を返したら待機完了。
    pub fn with_response_matcher<Matcher>(mut self, matcher: Matcher) -> Self
    where
        Matcher: Fn(&HttpResponse) -> bool + Send + Sync + 'static,
    {
        self.response_matcher = Some(Arc::new(matcher));
        self
    }

    /// 戦略が制御するヘッダと衝突するユーザー指定ヘッダを判定する。
    fn is_reserved_header(&self, name: &str) -> bool {
        name.eq_ignore_ascii_case("Host")
            || name.eq_ignore_ascii_case("Connection")
            || (self.body.is_some() && name.eq_ignore_ascii_case("Content-Length"))
            || (self.auth.is_some() && name.eq_ignore_ascii_case("Authorization"))
    }

    /// Host ヘッダ値を組み立てる。
    fn host_header(host: &str, port: u16) -> String {
        if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        }
    }

    /// リトライ前に 1 回だけ HTTP リクエストを構築する。
    fn build_request_bytes(
        &self,
        host: &str,
        port: u16,
    ) -> std::result::Result<Vec<u8>, HttpWaitError> {
        use shiguredo_http11::Request;

        let method = shiguredo_http11::Method::new(&self.method)
            .map_err(|error| HttpWaitError::RequestBuild(error.to_string()))?;
        // Connection: close を明示しないと keep-alive でサーバーが接続を保持し、
        // EOF 待ちでポーリングが数十秒単位で停滞する。
        let mut request = Request::new(method, self.path.as_str())
            .and_then(|request| request.header("Host", Self::host_header(host, port)))
            .and_then(|request| request.header("Connection", "close"))
            .map_err(|error| HttpWaitError::RequestBuild(error.to_string()))?;
        let mut warned_headers = Vec::new();
        for (name, value) in &self.headers {
            if self.is_reserved_header(name) {
                let normalized_name = name.to_ascii_lowercase();
                if !warned_headers.contains(&normalized_name) {
                    tracing::warn!(
                        header = normalized_name.as_str(),
                        "skipping user-supplied reserved HTTP header"
                    );
                    warned_headers.push(normalized_name);
                }
                continue;
            }
            request = request
                .header(
                    shiguredo_http11::HeaderName::new(name).map_err(|_| {
                        HttpWaitError::RequestBuild(format!("invalid header name: {name}"))
                    })?,
                    value.as_str(),
                )
                .map_err(|_| {
                    // ヘッダ値はシークレットになり得るためエラー文字列に含めない。
                    HttpWaitError::RequestBuild(format!("failed to set header `{name}`"))
                })?;
        }
        match &self.auth {
            Some(Auth::Basic { username, password }) => {
                // RFC 4648 (パディングあり)。shiguredo-rust の base64ct 規約に従う。
                let credentials =
                    Base64::encode_string(format!("{username}:{password}").as_bytes());
                request = request
                    .header("Authorization", format!("Basic {credentials}"))
                    .map_err(|_| {
                        HttpWaitError::RequestBuild(
                            "failed to set Authorization header".to_string(),
                        )
                    })?;
            }
            Some(Auth::Bearer(token)) => {
                request = request
                    .header("Authorization", format!("Bearer {token}"))
                    .map_err(|_| {
                        HttpWaitError::RequestBuild(
                            "failed to set Authorization header".to_string(),
                        )
                    })?;
            }
            None => {}
        }
        if let Some(body) = &self.body {
            // Request がボディ長と一致する Content-Length を自動で付与する。
            request = request.body(body.clone());
        }
        request
            .encode()
            .map_err(|error| HttpWaitError::RequestBuild(error.to_string()))
    }

    /// リクエストを 1 回送信してレスポンスを受信する。
    /// 接続失敗・プロトコルエラーは呼び出し側でリトライする。
    async fn send_request(
        &self,
        host: &str,
        port: u16,
        request_bytes: &[u8],
    ) -> std::result::Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>> {
        if self.request_timeout.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "HTTP request timed out",
            )
            .into());
        }
        tokio::time::timeout(
            self.request_timeout,
            self.send_request_inner(host, port, request_bytes),
        )
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "HTTP request timed out"))?
    }

    async fn send_request_inner(
        &self,
        host: &str,
        port: u16,
        request_bytes: &[u8],
    ) -> std::result::Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>> {
        use crate::core::client::http_decode::ResponseAccumulator;

        let mut stream = tokio::net::TcpStream::connect((host, port)).await?;
        stream.write_all(request_bytes).await?;

        let mut acc = ResponseAccumulator::new(&self.method, Some(MAX_HTTP_RESPONSE_BODY_BYTES));

        loop {
            let want = acc.read_buf_size();
            if want == 0 {
                return Err("decoder buffer full".into());
            }
            let buf = acc.mut_buf(want)?;
            let n = stream.read(buf).await?;
            if acc.feed(n)? {
                break;
            }
        }

        let decoded = acc.finish()?;
        Ok(HttpResponse {
            status: decoded.head.status_code(),
            headers: decoded
                .head
                .headers()
                .iter()
                .map(|(name, value)| (name.as_str().to_string(), value.clone()))
                .collect(),
            body: decoded.body,
        })
    }
}

impl HttpWaitStrategy {
    pub(crate) async fn wait_until_ready<I: Image>(
        self,
        _client: &Client,
        container: &ContainerAsync<I>,
    ) -> Result<()> {
        // 本家は待機ループ内で matcher 未設定をエラーにするが、ループ前に検査する。
        let matcher = self
            .response_matcher
            .clone()
            .ok_or(WaitContainerError::HttpWait(
                HttpWaitError::NoResponseMatcher,
            ))?;

        let host = container.get_host().await?;

        // ポート未指定なら最初の公開ポートを使う (本家と同じ)。
        let container_port = match self.port {
            Some(port) => port,
            None => container.ports().await?.first_container_port().ok_or(
                WaitContainerError::HttpWait(HttpWaitError::NoExposedPortsForHttpWait),
            )?,
        };

        // ホスト側ポートを解決。IPv4 が無ければ IPv6 にフォールバック (本家と同じ)。
        let host_port = match host {
            Host::Addr(std::net::IpAddr::V6(_)) => {
                container.get_host_port_ipv6(container_port).await?
            }
            _ => match container.get_host_port_ipv4(container_port).await {
                Ok(port) => port,
                Err(_) => container.get_host_port_ipv6(container_port).await?,
            },
        };

        let host = host.to_string();
        let request_bytes = self
            .build_request_bytes(&host, host_port)
            .map_err(WaitContainerError::HttpWait)?;
        loop {
            match self.send_request(&host, host_port, &request_bytes).await {
                Ok(response) => {
                    if matcher(&response) {
                        return Ok(());
                    }
                    tracing::debug!("HTTP response condition not met");
                }
                Err(e) => {
                    // 起動前の接続拒否などはリトライする。
                    // 全体は AsyncRunner の startup_timeout で打ち切られる。
                    tracing::debug!("error while waiting for HTTP response: {e}");
                }
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

impl fmt::Debug for HttpWaitStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpWaitStrategy")
            .field("path", &self.path)
            .field("port", &self.port)
            .field("method", &self.method)
            .field("headers", &RedactedHeaders(&self.headers))
            .field("body_len", &self.body.as_ref().map(Vec::len))
            .field("auth", &self.auth)
            .field("poll_interval", &self.poll_interval)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Auth::Basic { username, .. } => f
                .debug_struct("Basic")
                .field("username", username)
                .field("password", &"***")
                .finish(),
            Auth::Bearer(_) => f.debug_tuple("Bearer").field(&"***").finish(),
        }
    }
}

struct RedactedHeaders<'a>(&'a [(String, String)]);

impl fmt::Debug for RedactedHeaders<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .0
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str(),
                    if name.eq_ignore_ascii_case("Authorization") {
                        "***"
                    } else {
                        value.as_str()
                    },
                )
            })
            .collect();
        headers.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encode_rfc4648_test_vectors() {
        // base64ct 置換後の回帰確認 (RFC 4648 / RFC 7617)。
        assert_eq!(Base64::encode_string(b""), "");
        assert_eq!(Base64::encode_string(b"f"), "Zg==");
        assert_eq!(Base64::encode_string(b"fo"), "Zm8=");
        assert_eq!(Base64::encode_string(b"foo"), "Zm9v");
        assert_eq!(Base64::encode_string(b"foob"), "Zm9vYg==");
        assert_eq!(Base64::encode_string(b"fooba"), "Zm9vYmE=");
        assert_eq!(Base64::encode_string(b"foobar"), "Zm9vYmFy");
        assert_eq!(
            Base64::encode_string(b"Aladdin:open sesame"),
            "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }

    #[test]
    fn expected_status_code_sets_matcher() {
        let strategy = HttpWaitStrategy::new("/health").with_expected_status_code(200_u16);
        let matcher = strategy
            .response_matcher
            .expect("matcher が設定されていること");
        let ok = HttpResponse {
            status: 200,
            headers: vec![],
            body: vec![],
        };
        let ng = HttpResponse {
            status: 503,
            headers: vec![],
            body: vec![],
        };
        assert!(matcher(&ok));
        assert!(!matcher(&ng));
    }

    #[test]
    fn http_response_header_is_case_insensitive() {
        let response = HttpResponse {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/html".to_string())],
            body: vec![],
        };
        assert_eq!(response.header("content-type"), Some("text/html"));
        assert_eq!(response.header("CONTENT-TYPE"), Some("text/html"));
        assert_eq!(response.header("x-missing"), None);
    }

    #[test]
    fn request_build_failure_is_returned_immediately() {
        let strategy = HttpWaitStrategy::new("/health").with_method("invalid method");

        let error = strategy
            .build_request_bytes("localhost", 8080)
            .expect_err("不正なメソッドはリクエスト構築に失敗する");

        assert!(matches!(error, HttpWaitError::RequestBuild(_)));
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn reserved_headers_are_not_sent_case_insensitively() {
        let strategy = HttpWaitStrategy::new("/health")
            .with_header("host", "user.example")
            .with_header("CONNECTION", "keep-alive")
            .with_header("content-length", "999")
            .with_header("authorization", "Bearer user-token")
            .with_header("X-Request-Id", "request-id")
            .with_body(b"body".to_vec())
            .with_bearer_auth("strategy-token");

        let request = String::from_utf8(
            strategy
                .build_request_bytes("localhost", 8080)
                .expect("リクエストを構築できる"),
        )
        .expect("HTTP リクエストは ASCII");

        assert_eq!(request.matches("\r\nHost:").count(), 1);
        assert_eq!(request.matches("\r\nConnection:").count(), 1);
        assert_eq!(request.matches("\r\nContent-Length:").count(), 1);
        assert_eq!(request.matches("\r\nAuthorization:").count(), 1);
        assert!(request.contains("Host: localhost:8080\r\n"));
        assert!(request.contains("Connection: close\r\n"));
        assert!(request.contains("Content-Length: 4\r\n"));
        assert!(request.contains("Authorization: Bearer strategy-token\r\n"));
        assert!(request.contains("X-Request-Id: request-id\r\n"));
        assert!(!request.contains("user.example"));
        assert!(!request.contains("keep-alive"));
        assert!(!request.contains("999"));
        assert!(!request.contains("user-token"));
    }

    #[test]
    fn authorization_header_is_preserved_without_auth() {
        let strategy =
            HttpWaitStrategy::new("/health").with_header("Authorization", "Bearer user-token");

        let request = String::from_utf8(
            strategy
                .build_request_bytes("localhost", 8080)
                .expect("リクエストを構築できる"),
        )
        .expect("HTTP リクエストは ASCII");

        assert!(request.contains("Authorization: Bearer user-token\r\n"));
    }

    #[test]
    fn ipv6_host_header_is_bracketed() {
        let strategy = HttpWaitStrategy::new("/health");

        let request = String::from_utf8(
            strategy
                .build_request_bytes("2001:db8::1", 8080)
                .expect("リクエストを構築できる"),
        )
        .expect("HTTP リクエストは ASCII");

        assert!(request.contains("Host: [2001:db8::1]:8080\r\n"));
    }

    #[test]
    fn debug_redacts_auth_authorization_and_body() {
        let strategy = HttpWaitStrategy::new("/health")
            .with_header("Authorization", "Bearer header-secret")
            .with_body(b"body-secret".to_vec())
            .with_basic_auth("user", "password-secret");

        let debug = format!("{strategy:?}");

        assert!(debug.contains("user"));
        assert!(debug.contains("***"));
        assert!(debug.contains("body_len: Some(11)"));
        assert!(!debug.contains("header-secret"));
        assert!(!debug.contains("password-secret"));
        assert!(!debug.contains("body-secret"));
    }

    #[tokio::test]
    async fn zero_request_timeout_returns_immediately() {
        let strategy = HttpWaitStrategy::new("/health").with_request_timeout(Duration::ZERO);
        let request = strategy
            .build_request_bytes("127.0.0.1", 1)
            .expect("リクエストを構築できる");

        let error = strategy
            .send_request("127.0.0.1", 1, &request)
            .await
            .expect_err("ZERO は即時にタイムアウトする");

        assert!(
            error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::TimedOut)
        );
    }

    #[tokio::test]
    async fn request_timeout_limits_unresponsive_server() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("TCP リスナーを確保できる");
        let port = listener
            .local_addr()
            .expect("TCP リスナーのアドレスを取得できる")
            .port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("TCP 接続を受け付ける");
            let mut request = [0_u8; 1024];
            let _ = stream
                .read(&mut request)
                .await
                .expect("リクエストを受信できる");
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let strategy =
            HttpWaitStrategy::new("/health").with_request_timeout(Duration::from_millis(20));
        let request = strategy
            .build_request_bytes("127.0.0.1", port)
            .expect("リクエストを構築できる");

        let error = strategy
            .send_request("127.0.0.1", port, &request)
            .await
            .expect_err("無応答サーバーはタイムアウトする");

        assert!(
            error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::TimedOut)
        );
        server.abort();
    }

    #[tokio::test]
    async fn response_body_is_limited_to_one_mebibyte() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("TCP リスナーを確保できる");
        let port = listener
            .local_addr()
            .expect("TCP リスナーのアドレスを取得できる")
            .port();
        let body = vec![b'x'; MAX_HTTP_RESPONSE_BODY_BYTES + 1];
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("TCP 接続を受け付ける");
            let mut request = [0_u8; 1024];
            let _ = stream
                .read(&mut request)
                .await
                .expect("リクエストを受信できる");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("レスポンスヘッダを送信できる");
            stream
                .write_all(&body)
                .await
                .expect("レスポンスボディを送信できる");
        });
        let strategy = HttpWaitStrategy::new("/health");
        let request = strategy
            .build_request_bytes("127.0.0.1", port)
            .expect("リクエストを構築できる");

        let response = strategy
            .send_request("127.0.0.1", port, &request)
            .await
            .expect("レスポンスを受信できる");

        assert_eq!(response.body().len(), MAX_HTTP_RESPONSE_BODY_BYTES);
        assert!(response.body().iter().all(|byte| *byte == b'x'));
        server.await.expect("TCP サーバーが完了する");
    }
}
