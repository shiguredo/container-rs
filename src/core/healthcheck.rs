//! `Healthcheck` 設定型。本家 testcontainers-rs 0.27 と同一シグネチャ。
//!
//! Linux (Docker Engine API) では create JSON の `Config.Healthcheck` に配線する。
//! macOS では型・設定口は公開するが、`with_health_check` 指定時は start で明示エラーになる。

use std::time::Duration;

/// コンテナのヘルスチェック設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Healthcheck {
    test: Vec<String>,
    interval: Option<Duration>,
    timeout: Option<Duration>,
    retries: Option<u64>,
    start_period: Option<Duration>,
    start_interval: Option<Duration>,
}

impl Healthcheck {
    /// ヘルスチェックを無効化する (`Test=["NONE"]`)。
    pub fn none() -> Self {
        Self {
            test: vec!["NONE".into()],
            interval: None,
            timeout: None,
            retries: None,
            start_period: None,
            start_interval: None,
        }
    }

    /// 空の test (`Test=[]`)。イメージの HEALTHCHECK を継承する。
    pub fn empty() -> Self {
        Self {
            test: Vec::new(),
            interval: None,
            timeout: None,
            retries: None,
            start_period: None,
            start_interval: None,
        }
    }

    /// シェル経由のプローブコマンド (`Test=["CMD-SHELL", cmd]`)。
    pub fn cmd_shell(cmd: impl Into<String>) -> Self {
        Self {
            test: vec!["CMD-SHELL".into(), cmd.into()],
            interval: None,
            timeout: None,
            retries: None,
            start_period: None,
            start_interval: None,
        }
    }

    /// 直接実行のプローブコマンド (`Test=["CMD", ...cmd]`)。
    pub fn cmd<I, S>(cmd: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut test = vec!["CMD".into()];
        test.extend(cmd.into_iter().map(Into::into));
        Self {
            test,
            interval: None,
            timeout: None,
            retries: None,
            start_period: None,
            start_interval: None,
        }
    }

    /// プローブ実行間隔を設定する。
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = Some(interval);
        self
    }

    /// 1 回のプローブのタイムアウトを設定する。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// 連続失敗を unhealthy と判定するまでのリトライ回数を設定する。
    pub fn with_retries(mut self, retries: u64) -> Self {
        self.retries = Some(retries);
        self
    }

    /// 起動直後に失敗を無視する期間を設定する。
    pub fn with_start_period(mut self, start_period: Duration) -> Self {
        self.start_period = Some(start_period);
        self
    }

    /// 起動期間中のプローブ間隔を設定する。
    ///
    /// Docker Engine 25.0 (API v1.44) 未満では未知キーとして無視される。
    pub fn with_start_interval(mut self, start_interval: Duration) -> Self {
        self.start_interval = Some(start_interval);
        self
    }

    /// プローブコマンドの配列を返す。
    pub fn test(&self) -> &[String] {
        &self.test
    }

    /// プローブ実行間隔を返す。
    pub fn interval(&self) -> Option<Duration> {
        self.interval
    }

    /// 1 回のプローブのタイムアウトを返す。
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// リトライ回数を返す。
    pub fn retries(&self) -> Option<u64> {
        self.retries
    }

    /// 起動直後に失敗を無視する期間を返す。
    pub fn start_period(&self) -> Option<Duration> {
        self.start_period
    }

    /// 起動期間中のプローブ間隔を返す。
    pub fn start_interval(&self) -> Option<Duration> {
        self.start_interval
    }

    /// Docker create JSON の `Healthcheck` オブジェクト文字列を生成する。
    ///
    /// `test` が空かつ他フィールドがすべて `None` のときは image inherit と等価のため
    /// `None` を返す (呼び出し元はキーごと省略する)。
    #[cfg(target_os = "linux")]
    pub(crate) fn to_docker_json(&self) -> Option<String> {
        use crate::core::client::docker_client::json_array;

        let all_none = self.interval.is_none()
            && self.timeout.is_none()
            && self.retries.is_none()
            && self.start_period.is_none()
            && self.start_interval.is_none();
        if self.test.is_empty() && all_none {
            return None;
        }

        let mut json = String::from("{\"Test\":");
        json.push_str(&json_array(&self.test));
        if let Some(interval) = self.interval {
            json.push_str(",\"Interval\":");
            json.push_str(&duration_to_nanos_i64(interval).to_string());
        }
        if let Some(timeout) = self.timeout {
            json.push_str(",\"Timeout\":");
            json.push_str(&duration_to_nanos_i64(timeout).to_string());
        }
        if let Some(retries) = self.retries {
            json.push_str(",\"Retries\":");
            json.push_str(&retries.to_string());
        }
        if let Some(start_period) = self.start_period {
            json.push_str(",\"StartPeriod\":");
            json.push_str(&duration_to_nanos_i64(start_period).to_string());
        }
        if let Some(start_interval) = self.start_interval {
            json.push_str(",\"StartInterval\":");
            json.push_str(&duration_to_nanos_i64(start_interval).to_string());
        }
        json.push('}');
        Some(json)
    }
}

/// `Duration` を Docker Engine の nanosecond int64 に飽和変換する。
#[cfg(target_os = "linux")]
fn duration_to_nanos_i64(d: Duration) -> i64 {
    let nanos = d.as_nanos();
    nanos.min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_expected_test_arrays() {
        // 各コンストラクタが期待の Test 配列を持つこと。
        assert_eq!(Healthcheck::none().test(), &["NONE".to_string()]);
        assert!(Healthcheck::empty().test().is_empty());
        assert_eq!(
            Healthcheck::cmd_shell("true").test(),
            &["CMD-SHELL".to_string(), "true".to_string()]
        );
        assert_eq!(
            Healthcheck::cmd(["echo", "ok"]).test(),
            &["CMD".to_string(), "echo".to_string(), "ok".to_string()]
        );
    }

    #[test]
    fn builders_are_readable_via_accessors() {
        // 各ビルダーで設定した値が対応 accessor から取り出せること。
        let hc = Healthcheck::empty()
            .with_interval(Duration::from_secs(1))
            .with_timeout(Duration::from_secs(2))
            .with_retries(3)
            .with_start_period(Duration::from_secs(4))
            .with_start_interval(Duration::from_secs(5));
        assert_eq!(hc.interval(), Some(Duration::from_secs(1)));
        assert_eq!(hc.timeout(), Some(Duration::from_secs(2)));
        assert_eq!(hc.retries(), Some(3));
        assert_eq!(hc.start_period(), Some(Duration::from_secs(4)));
        assert_eq!(hc.start_interval(), Some(Duration::from_secs(5)));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;

    #[test]
    fn to_docker_json_emits_test_variants() {
        // CMD-SHELL / CMD / NONE / [] の各形態が JSON 配列として正しく出ること。
        let shell = Healthcheck::cmd_shell("true").to_docker_json().unwrap();
        assert_eq!(shell, r#"{"Test":["CMD-SHELL","true"]}"#);

        let cmd = Healthcheck::cmd(["echo", "ok"]).to_docker_json().unwrap();
        assert_eq!(cmd, r#"{"Test":["CMD","echo","ok"]}"#);

        let none = Healthcheck::none().to_docker_json().unwrap();
        assert_eq!(none, r#"{"Test":["NONE"]}"#);

        // empty + interval は Test=[] を出しつつ Interval を付ける。
        let empty_with_interval = Healthcheck::empty()
            .with_interval(Duration::from_secs(5))
            .to_docker_json()
            .unwrap();
        assert_eq!(empty_with_interval, r#"{"Test":[],"Interval":5000000000}"#);
    }

    #[test]
    fn to_docker_json_duration_and_omission() {
        // 30 秒が nanosecond で出ること、ZERO は 0、None はキー省略。
        let hc = Healthcheck::cmd_shell("true")
            .with_interval(Duration::from_secs(30))
            .with_timeout(Duration::ZERO);
        let json = hc.to_docker_json().unwrap();
        assert!(
            json.contains(r#""Interval":30000000000"#),
            "30 秒が nanosecond で出ること: {json}"
        );
        assert!(
            json.contains(r#""Timeout":0"#),
            "Some(ZERO) は 0 を出すこと: {json}"
        );
        assert!(
            !json.contains("Retries"),
            "None の Retries はキー省略であること: {json}"
        );
        assert!(
            !json.contains("StartPeriod"),
            "None の StartPeriod はキー省略であること: {json}"
        );
    }

    #[test]
    fn to_docker_json_escapes_test_elements() {
        // Test 要素の特殊文字が正しく escape されること。
        let hc = Healthcheck::cmd_shell("a\"b\\c\nd");
        let json = hc.to_docker_json().unwrap();
        assert_eq!(
            json, r#"{"Test":["CMD-SHELL","a\"b\\c\nd"]}"#,
            "引用符・バックスラッシュ・改行が escape されること"
        );

        let multibyte = Healthcheck::cmd_shell("日本語").to_docker_json().unwrap();
        assert_eq!(
            multibyte, r#"{"Test":["CMD-SHELL","日本語"]}"#,
            "マルチバイト文字はそのまま出ること"
        );
    }

    #[test]
    fn to_docker_json_returns_none_for_empty_default() {
        // 全フィールド None / test 空は None (image inherit)。
        assert!(Healthcheck::empty().to_docker_json().is_none());
    }
}
