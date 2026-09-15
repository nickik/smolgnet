use smolgnet::*;

fn prefix(value: u64) -> GdpPrefix {
    GdpPrefix::new(GdpAddress(value), 48).unwrap()
}

#[test]
fn connected_route_is_forwarded_across_multiple_routers_with_metric_growth() {
    let a = RouterId::new(1).unwrap();
    let b = RouterId::new(2).unwrap();
    let c = RouterId::new(3).unwrap();
    let network = prefix(0x3101_0000_0000_0000);

    let mut a_rib = RouterRib::new(a);
    let mut b_rib = RouterRib::new(b);
    let mut c_rib = RouterRib::new(c);
    a_rib.add_connected(network, 0);

    let a_to_b = a_rib.advertisements_for(b, RouteMetric(100));
    assert_eq!(a_to_b.len(), 1);
    assert_eq!(a_to_b[0].origin, RouteOrigin::Connected);
    assert_eq!(a_to_b[0].metric, RouteMetric(100));
    b_rib.receive_advertisement(a, 1, a_to_b[0]).unwrap();

    let b_to_c = b_rib.advertisements_for(c, RouteMetric(50));
    assert_eq!(b_to_c.len(), 1);
    assert_eq!(b_to_c[0].origin, RouteOrigin::Learned);
    assert_eq!(b_to_c[0].metric, RouteMetric(150));
    c_rib.receive_advertisement(b, 0, b_to_c[0]).unwrap();

    let learned = c_rib.learned_fib();
    assert_eq!(learned.len(), 1);
    assert_eq!(learned[0].prefix, network);
    assert_eq!(learned[0].metric, RouteMetric(150));
    assert_eq!(learned[0].learned_from, Some(b));
}

#[test]
fn split_horizon_does_not_advertise_route_back_to_source_neighbor() {
    let a = RouterId::new(1).unwrap();
    let b = RouterId::new(2).unwrap();
    let network = prefix(0x3101_0000_0000_0000);

    let mut b_rib = RouterRib::new(b);
    b_rib
        .receive_advertisement(
            a,
            0,
            RouteAdvertise {
                advertiser: a,
                prefix: network,
                origin: RouteOrigin::Connected,
                metric: RouteMetric(100),
            },
        )
        .unwrap();

    assert!(b_rib.advertisements_for(a, RouteMetric(100)).is_empty());
}

#[test]
fn lower_metric_then_lower_router_id_breaks_learned_route_ties() {
    let local = RouterId::new(100).unwrap();
    let n9 = RouterId::new(9).unwrap();
    let n10 = RouterId::new(10).unwrap();
    let network = prefix(0x4202_0000_0000_0000);
    let mut rib = RouterRib::new(local);

    for (neighbor, port, metric) in [(n10, 1, 200), (n9, 0, 200)] {
        rib.receive_advertisement(
            neighbor,
            port,
            RouteAdvertise {
                advertiser: neighbor,
                prefix: network,
                origin: RouteOrigin::Learned,
                metric: RouteMetric(metric),
            },
        )
        .unwrap();
    }

    let selected = rib.learned_fib();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].learned_from, Some(n9));
    assert_eq!(selected[0].egress_port, 0);

    rib.receive_advertisement(
        n10,
        1,
        RouteAdvertise {
            advertiser: n10,
            prefix: network,
            origin: RouteOrigin::Learned,
            metric: RouteMetric(150),
        },
    )
    .unwrap();
    let selected = rib.learned_fib();
    assert_eq!(selected[0].learned_from, Some(n10));
    assert_eq!(selected[0].egress_port, 1);
}

#[test]
fn static_route_preference_beats_learned_route_for_same_prefix() {
    let local = RouterId::new(100).unwrap();
    let neighbor = RouterId::new(1).unwrap();
    let network = prefix(0x4202_0000_0000_0000);
    let mut rib = RouterRib::new(local);

    rib.receive_advertisement(
        neighbor,
        1,
        RouteAdvertise {
            advertiser: neighbor,
            prefix: network,
            origin: RouteOrigin::Connected,
            metric: RouteMetric(1),
        },
    )
    .unwrap();
    rib.add_static(network, 0, RouteMetric(50_000));

    let selected = rib.selected_routes();
    let route = selected.iter().find(|route| route.prefix == network).unwrap();
    assert_eq!(route.origin, RouteOrigin::Static);
    assert_eq!(route.egress_port, 0);
    assert!(rib.learned_fib().is_empty());
}

#[test]
fn advertisement_must_identify_the_actual_sending_neighbor() {
    let local = RouterId::new(100).unwrap();
    let sender = RouterId::new(1).unwrap();
    let claimed = RouterId::new(2).unwrap();
    let mut rib = RouterRib::new(local);
    let result = rib.receive_advertisement(
        sender,
        0,
        RouteAdvertise {
            advertiser: claimed,
            prefix: prefix(0x3101_0000_0000_0000),
            origin: RouteOrigin::Connected,
            metric: RouteMetric(100),
        },
    );
    assert_eq!(result, Err(Error::InvalidField));
}

#[test]
fn route_withdraw_has_exact_wire_image_and_rejects_reserved_bytes() {
    let message = RoutingGctlMessage::route_withdraw(
        0x1122_3344,
        RouteWithdraw {
            advertiser: RouterId::new(0x0102_0304_0506_0708).unwrap(),
            prefix: prefix(0x4202_0000_0000_0000),
        },
    );
    let encoded = message.encode();
    assert_eq!(encoded.len(), 32);
    assert_eq!(encoded[0..8], [1, 0x43, 0, 0, 0x11, 0x22, 0x33, 0x44]);
    assert_eq!(encoded[8..16], 0x0102_0304_0506_0708u64.to_be_bytes());
    assert_eq!(encoded[16..24], 0x4202_0000_0000_0000u64.to_be_bytes());
    assert_eq!(encoded[24], 48);
    assert_eq!(&encoded[25..32], &[0; 7]);
    assert_eq!(RoutingGctlMessage::decode(&encoded).unwrap(), message);

    let mut malformed = encoded;
    malformed[31] = 1;
    assert_eq!(RoutingGctlMessage::decode(&malformed), Err(Error::InvalidField));
}

#[test]
fn withdrawal_is_neighbor_scoped_and_idempotent() {
    let local = RouterId::new(100).unwrap();
    let b = RouterId::new(20).unwrap();
    let c = RouterId::new(30).unwrap();
    let network = prefix(0x4202_0000_0000_0000);
    let mut rib = RouterRib::new(local);

    for (neighbor, port, metric) in [(b, 0, 100), (c, 1, 200)] {
        rib.receive_advertisement(
            neighbor,
            port,
            RouteAdvertise {
                advertiser: neighbor,
                prefix: network,
                origin: RouteOrigin::Learned,
                metric: RouteMetric(metric),
            },
        )
        .unwrap();
    }

    let withdrawal = RouteWithdraw {
        advertiser: b,
        prefix: network,
    };
    assert!(rib.receive_withdrawal(b, withdrawal).unwrap());
    assert_eq!(rib.learned_fib()[0].learned_from, Some(c));
    assert!(!rib.receive_withdrawal(b, withdrawal).unwrap());
    assert_eq!(rib.routes().iter().filter(|route| route.prefix == network).count(), 1);

    assert_eq!(
        rib.receive_withdrawal(
            b,
            RouteWithdraw {
                advertiser: c,
                prefix: network,
            },
        ),
        Err(Error::InvalidField)
    );
}

#[test]
fn preferred_path_failure_propagates_withdrawal_and_uses_retained_alternate() {
    let a = RouterId::new(10).unwrap();
    let b = RouterId::new(20).unwrap();
    let c = RouterId::new(30).unwrap();
    let d = RouterId::new(40).unwrap();
    let network = prefix(0x5505_0000_0000_0000);

    let mut a_rib = RouterRib::new(a);
    let mut b_rib = RouterRib::new(b);
    let mut c_rib = RouterRib::new(c);
    let mut d_rib = RouterRib::new(d);
    d_rib.add_connected(network, 0);

    let d_to_b = d_rib.advertisements_for(b, RouteMetric(100));
    let d_to_c = d_rib.advertisements_for(c, RouteMetric(200));
    b_rib.receive_advertisement(d, 1, d_to_b[0]).unwrap();
    c_rib.receive_advertisement(d, 1, d_to_c[0]).unwrap();

    let b_to_a = b_rib.advertisements_for(a, RouteMetric(50));
    let c_to_a = c_rib.advertisements_for(a, RouteMetric(50));
    a_rib.receive_advertisement(b, 0, b_to_a[0]).unwrap();
    a_rib.receive_advertisement(c, 1, c_to_a[0]).unwrap();
    assert_eq!(a_rib.learned_fib()[0].learned_from, Some(b));
    assert_eq!(a_rib.learned_fib()[0].metric, RouteMetric(150));

    let changed = b_rib.remove_learned_from(d);
    assert_eq!(changed, vec![network]);
    let updates = b_rib.updates_for(a, RouteMetric(50), &changed);
    assert_eq!(updates.len(), 1);
    let withdrawal = match updates[0] {
        RouteUpdate::Withdraw(withdrawal) => withdrawal,
        other => panic!("failed path should be withdrawn, got {other:?}"),
    };
    assert!(a_rib.receive_withdrawal(b, withdrawal).unwrap());
    assert_eq!(a_rib.learned_fib()[0].learned_from, Some(c));
    assert_eq!(a_rib.learned_fib()[0].metric, RouteMetric(250));

    b_rib.receive_advertisement(d, 1, d_to_b[0]).unwrap();
    let restored = b_rib.advertisements_for(a, RouteMetric(50));
    a_rib.receive_advertisement(b, 0, restored[0]).unwrap();
    assert_eq!(a_rib.learned_fib()[0].learned_from, Some(b));
    assert_eq!(a_rib.learned_fib()[0].metric, RouteMetric(150));
}
