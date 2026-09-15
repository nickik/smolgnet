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
