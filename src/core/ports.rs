//! ポート関連の型。元の testcontainers 0.27 のサブセット（差分の正は `docs/TESTCONTAINERS.md` の該当節 15.2）。

use std::collections::BTreeMap;

/// コンテナが公開するポート。プロトコル付き。
///
/// `u16` からは `Into::into` で `Tcp` になる。`IntoContainerPort` トレイトで `123.udp()` のように明示できる。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ContainerPort {
    Tcp(u16),
    Udp(u16),
    Sctp(u16),
}

impl std::fmt::Display for ContainerPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContainerPort::Tcp(p) => write!(f, "{p}/tcp"),
            ContainerPort::Udp(p) => write!(f, "{p}/udp"),
            ContainerPort::Sctp(p) => write!(f, "{p}/sctp"),
        }
    }
}

impl std::str::FromStr for ContainerPort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (port, proto) = match s.rsplit_once('/') {
            Some((p, proto)) => (p, proto),
            None => (s, "tcp"),
        };
        // 本家と同様、先頭 `+` 付きや非数字は拒否する (u16::parse は "+" を受理するため)。
        if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("invalid port: {port}"));
        }
        let port: u16 = port
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        match proto {
            "tcp" => Ok(ContainerPort::Tcp(port)),
            "udp" => Ok(ContainerPort::Udp(port)),
            "sctp" => Ok(ContainerPort::Sctp(port)),
            other => Err(format!("unknown protocol: {other}")),
        }
    }
}

/// `u16` を `ContainerPort` に変換するヘルパートレイト。
pub trait IntoContainerPort {
    fn tcp(self) -> ContainerPort;
    fn udp(self) -> ContainerPort;
    fn sctp(self) -> ContainerPort;
}

impl IntoContainerPort for u16 {
    fn tcp(self) -> ContainerPort {
        ContainerPort::Tcp(self)
    }
    fn udp(self) -> ContainerPort {
        ContainerPort::Udp(self)
    }
    fn sctp(self) -> ContainerPort {
        ContainerPort::Sctp(self)
    }
}

impl From<u16> for ContainerPort {
    fn from(port: u16) -> Self {
        ContainerPort::Tcp(port)
    }
}

impl ContainerPort {
    /// ポート番号を返す（プロトコル問わず）。
    pub fn as_u16(self) -> u16 {
        match self {
            ContainerPort::Tcp(p) | ContainerPort::Udp(p) | ContainerPort::Sctp(p) => p,
        }
    }

    /// プロトコル文字列を返す。
    pub fn as_str(self) -> &'static str {
        match self {
            ContainerPort::Tcp(_) => "tcp",
            ContainerPort::Udp(_) => "udp",
            ContainerPort::Sctp(_) => "sctp",
        }
    }
}

/// 実行中コンテナの公開ポート一覧。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Ports {
    ipv4_mapping: BTreeMap<ContainerPort, u16>,
    ipv6_mapping: BTreeMap<ContainerPort, u16>,
}

impl Ports {
    /// IPv4 のホストポートを返す。
    pub fn map_to_host_port_ipv4(&self, container_port: impl Into<ContainerPort>) -> Option<u16> {
        self.ipv4_mapping.get(&container_port.into()).copied()
    }

    /// IPv6 のホストポートを返す。
    pub fn map_to_host_port_ipv6(&self, container_port: impl Into<ContainerPort>) -> Option<u16> {
        self.ipv6_mapping.get(&container_port.into()).copied()
    }

    /// マッピング済みのコンテナポートを 1 つ返す (IPv4 優先)。
    /// HTTP 待機戦略のポート未指定時のフォールバックに使う。
    /// BTreeMap のためキー最小 (Tcp < Udp < Sctp、同一プロトコルは番号昇順) が常に選ばれる。
    #[cfg(feature = "http_wait_plain")]
    pub(crate) fn first_container_port(&self) -> Option<ContainerPort> {
        self.ipv4_mapping
            .keys()
            .next()
            .or_else(|| self.ipv6_mapping.keys().next())
            .copied()
    }

    /// IPv4 のポートマッピングを追加する。
    pub(crate) fn add_mapping(&mut self, container_port: ContainerPort, host_port: u16) {
        self.ipv4_mapping.insert(container_port, host_port);
    }

    /// IPv6 のポートマッピングを追加する。ホストアドレスが IPv6 の公開に使う。
    // Linux では IPv6 公開の経路が無いため lib 本体からは未使用 (単体テストでは使う)
    #[cfg_attr(all(target_os = "linux", not(test)), expect(dead_code))]
    pub(crate) fn add_ipv6_mapping(&mut self, container_port: ContainerPort, host_port: u16) {
        self.ipv6_mapping.insert(container_port, host_port);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "http_wait_plain")]
    #[test]
    fn first_container_port_is_deterministic_min_key() {
        // 複数ポートがあっても常に BTreeMap 上の最小キー (IPv4 優先) を返すこと。
        let mut ports = Ports::default();
        ports.add_mapping(ContainerPort::Tcp(8080), 18080);
        ports.add_mapping(ContainerPort::Tcp(80), 18000);
        ports.add_mapping(ContainerPort::Udp(53), 1053);
        assert_eq!(
            ports.first_container_port(),
            Some(ContainerPort::Tcp(80)),
            "Tcp(80) が Udp / より大きい Tcp より先に選ばれること"
        );

        // IPv4 が空なら IPv6 の最小キーを返すこと。
        let mut v6_only = Ports::default();
        v6_only.add_ipv6_mapping(ContainerPort::Tcp(443), 8443);
        v6_only.add_ipv6_mapping(ContainerPort::Tcp(80), 8080);
        assert_eq!(
            v6_only.first_container_port(),
            Some(ContainerPort::Tcp(80)),
            "IPv6 のみでも最小キーが選ばれること"
        );
    }

    #[test]
    fn container_port_from_str_rejects_leading_plus() {
        // 先頭 `+` 付きは拒否すること。
        assert!(
            "+80".parse::<ContainerPort>().is_err(),
            "+80 は拒否されること"
        );
        assert!(
            "+80/tcp".parse::<ContainerPort>().is_err(),
            "+80/tcp は拒否されること"
        );
    }

    #[test]
    fn container_port_from_str_rejects_invalid_inputs() {
        // 空・非数字・未知プロトコル・範囲外は拒否すること。
        assert!("".parse::<ContainerPort>().is_err(), "空は拒否されること");
        assert!(
            "abc/tcp".parse::<ContainerPort>().is_err(),
            "非数字ポートは拒否されること"
        );
        assert!(
            "80/foo".parse::<ContainerPort>().is_err(),
            "未知プロトコルは拒否されること"
        );
        assert!(
            "99999/tcp".parse::<ContainerPort>().is_err(),
            "u16 超過は拒否されること"
        );
    }
}
