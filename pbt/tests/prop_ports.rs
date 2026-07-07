//! `ContainerPort` の Display ⇔ FromStr ラウンドトリップ PBT。

use proptest::prelude::*;
use shiguredo_container::core::ContainerPort;

fn any_container_port() -> impl Strategy<Value = ContainerPort> {
    (any::<u16>(), prop_oneof![Just(0u8), Just(1), Just(2)]).prop_map(|(port, kind)| match kind {
        0 => ContainerPort::Tcp(port),
        1 => ContainerPort::Udp(port),
        _ => ContainerPort::Sctp(port),
    })
}

proptest! {
    /// Display した文字列を FromStr すると元のポートに戻ること。
    #[test]
    fn container_port_display_from_str_roundtrip(port in any_container_port()) {
        let text = port.to_string();
        let parsed: ContainerPort = text
            .parse()
            .expect("Display した文字列は FromStr できること");
        prop_assert_eq!(parsed, port);
    }
}
