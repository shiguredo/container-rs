//! `ContainerPort` の Display ⇔ FromStr ラウンドトリップ PBT。

use std::cell::Cell;

use shiguredo_container::core::ContainerPort;

#[test]
fn container_port_display_from_str_roundtrip() -> noprop::TestResult {
    let seed = noprop::seed_from_env_or_time("CONTAINER_RS_SEED")?;
    let mut runner = noprop::Runner::new(seed);
    // プロトコル種別ごとの到達回数を数える (1 種別だけになると他種別の検証を逃すため)。
    let tcp = Cell::new(0usize);
    let udp = Cell::new(0usize);
    let sctp = Cell::new(0usize);

    runner.run(256, |ctx| {
        // プロトコル (0: tcp, 1: udp, 2: sctp) を均等に選ぶ。
        // u16 全空間を覆うため、ポート番号はそのまま一様サンプリングする。
        let port = match noprop::sample_usize_in(ctx, 0..3) {
            0 => ContainerPort::Tcp(noprop::sample_u16(ctx)),
            1 => ContainerPort::Udp(noprop::sample_u16(ctx)),
            _ => ContainerPort::Sctp(noprop::sample_u16(ctx)),
        };

        let text = port.to_string();
        let parsed: ContainerPort = text
            .parse()
            .expect("Display した文字列は FromStr できること");
        assert_eq!(
            parsed, port,
            "ラウンドトリップで元のポートに戻ること: {text}"
        );

        // 到達ゲートは不変条件の評価地点で数える。
        match port {
            ContainerPort::Tcp(_) => tcp.set(tcp.get() + 1),
            ContainerPort::Udp(_) => udp.set(udp.get() + 1),
            ContainerPort::Sctp(_) => sctp.set(sctp.get() + 1),
        }
        Ok(())
    })?;

    assert!(
        tcp.get() > 0,
        "Tcp のケースが 1 件も実行されていないこと\n{runner}"
    );
    assert!(
        udp.get() > 0,
        "Udp のケースが 1 件も実行されていないこと\n{runner}"
    );
    assert!(
        sctp.get() > 0,
        "Sctp のケースが 1 件も実行されていないこと\n{runner}"
    );
    Ok(())
}
