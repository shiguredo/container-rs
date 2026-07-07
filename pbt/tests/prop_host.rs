//! `Host` の Property-Based Testing。

use proptest::prelude::*;
use shiguredo_container::core::Host;

proptest! {
    /// IPv4 アドレスは Display → parse で往復すること。
    #[test]
    fn ipv4_display_parse_roundtrip(a in any::<u8>(), b in any::<u8>(), c in any::<u8>(), d in any::<u8>()) {
        let s = format!("{a}.{b}.{c}.{d}");
        let host = Host::parse(&s);
        prop_assert!(matches!(host, Host::Addr(_)), "IP として解釈されること: {s}");
        prop_assert_eq!(host.to_string(), s);
    }

    /// 非 IP 文字列は Domain になること。
    #[test]
    fn non_ip_becomes_domain(s in "[a-z][a-z0-9.-]{0,32}") {
        prop_assume!(s.parse::<std::net::IpAddr>().is_err());
        let host = Host::parse(&s);
        prop_assert_eq!(host.to_string(), s.clone());
        prop_assert_eq!(host, Host::Domain(s));
    }
}
