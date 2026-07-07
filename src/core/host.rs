//! ホスト名または IP アドレスを表す型。

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

/// ホスト名または IP アドレス。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// IP アドレス。
    Addr(IpAddr),
    /// ドメイン名。
    Domain(String),
}

impl Host {
    /// 文字列をパースする。
    /// IP アドレスとして解釈できなければドメイン名として扱う。
    pub fn parse(s: &str) -> Self {
        match IpAddr::from_str(s) {
            Ok(ip) => Host::Addr(ip),
            Err(_) => Host::Domain(s.to_string()),
        }
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Host::Addr(ip) => write!(f, "{ip}"),
            Host::Domain(s) => write!(f, "{s}"),
        }
    }
}
