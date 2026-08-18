//! `Host` の Property-Based Testing。

use std::cell::Cell;

use shiguredo_container::core::Host;

#[test]
fn ipv4_display_parse_roundtrip() -> noprop::TestResult {
    let seed = noprop::seed_from_env_or_time("CONTAINER_RS_SEED")?;
    let mut runner = noprop::Runner::new(seed);

    runner.run(256, |ctx| {
        // 各オクテットを独立に一様サンプリングする (全 IPv4 空間を覆う)。
        let a = noprop::sample_u8(ctx);
        let b = noprop::sample_u8(ctx);
        let c = noprop::sample_u8(ctx);
        let d = noprop::sample_u8(ctx);
        let s = format!("{a}.{b}.{c}.{d}");
        let host = Host::parse(&s);
        assert!(
            matches!(host, Host::Addr(_)),
            "IP として解釈されること: {s}"
        );
        assert_eq!(host.to_string(), s, "Display で元の文字列に戻ること: {s}");
        Ok(())
    })?;
    Ok(())
}

#[test]
fn non_ip_becomes_domain() -> noprop::TestResult {
    let seed = noprop::seed_from_env_or_time("CONTAINER_RS_SEED")?;
    let mut runner = noprop::Runner::new(seed);
    // 名前長の境界 (空と最大 32) の到達回数を数える。
    let short = Cell::new(0usize);
    let long = Cell::new(0usize);

    runner.run(256, |ctx| {
        // 英小文字で始まり、英小文字・数字・`.`・`-` を 0..=32 個続ける。
        // 先頭が英字でコロンを含まないため IP アドレスとして解釈され得ない。
        // したがって拒否なしで valid-by-construction に生成できる。
        let suffix_len =
            noprop::sample_with_boundaries(ctx, &[0usize, 32], noprop::Ratio::one_nth(4), |ctx| {
                noprop::sample_usize_in(ctx, 0..=32)
            });
        let mut s = String::with_capacity(suffix_len + 1);
        s.push(sample_lower_alpha(ctx));
        for _ in 0..suffix_len {
            s.push(sample_lower_alpha_num_dot_dash(ctx));
        }

        let host = Host::parse(&s);
        assert_eq!(host.to_string(), s, "Display で元の文字列に戻ること: {s}");
        assert_eq!(host, Host::Domain(s), "非 IP 文字列は Domain になること");

        // 到達ゲートは不変条件の評価地点で数える。
        if suffix_len == 0 {
            short.set(short.get() + 1);
        } else if suffix_len == 32 {
            long.set(long.get() + 1);
        }
        Ok(())
    })?;

    assert!(
        short.get() > 0,
        "空の名前長 (1 文字のみ) のケースが実行されていないこと\n{runner}"
    );
    assert!(
        long.get() > 0,
        "最大の名前長 (33 文字) のケースが実行されていないこと\n{runner}"
    );
    Ok(())
}

/// 英小文字 1 文字を一様にサンプリングする。
fn sample_lower_alpha(ctx: &mut noprop::TestCaseContext) -> char {
    (b'a' + noprop::sample_usize_in(ctx, 0..26) as u8) as char
}

/// 英小文字・数字・`.`・`-` の 38 文字から 1 文字を一様にサンプリングする。
fn sample_lower_alpha_num_dot_dash(ctx: &mut noprop::TestCaseContext) -> char {
    match noprop::sample_usize_in(ctx, 0..38) {
        i @ 0..26 => (b'a' + i as u8) as char,
        i @ 26..36 => (b'0' + (i - 26) as u8) as char,
        36 => '.',
        _ => '-',
    }
}
