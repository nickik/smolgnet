use smolgnet::*;

const LEFT_PREFIX: u64 = 0x3101_0000_0000_0000;
const RIGHT_PREFIX: u64 = 0x4202_0000_0000_0000;

fn prefix(value: u64) -> GdpPrefix {
    GdpPrefix::new(GdpAddress(value), 48).unwrap()
}

fn packet(source: GdpAddress, destination: GdpAddress) -> GdpPacket {
    GdpPacket::new(
        GdpHeader::global(GdpType::Gts, SizeClass::Ctrl32, 16, source, destination),
        vec![0x5a; SizeClass::Ctrl32.bytes()],
    )
    .unwrap()
}

#[test]
fn learned_cross_router_routes_are_installed_without_static_configuration() {
    let left_id = RouterId::new(0x1111).unwrap();
    let right_id = RouterId::new(0x2222).unwrap();

    let left_lan = RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0010))
        .with_connected_network(prefix(LEFT_PREFIX), GdpAddress(LEFT_PREFIX | 1));
    let left_link = RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0011));
    let right_link = RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0020));
    let right_lan = RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0021))
        .with_connected_network(prefix(RIGHT_PREFIX), GdpAddress(RIGHT_PREFIX | 1));

    let mut left = DynamicP4Router::new(left_id, vec![left_lan, left_link], vec![]).unwrap();
    let mut right = DynamicP4Router::new(right_id, vec![right_link, right_lan], vec![]).unwrap();

    assert!(left.learned_routes().is_empty());
    assert!(right.learned_routes().is_empty());

    for advertisement in left.advertisements_for(right_id, RouteMetric::DEFAULT_LINK) {
        right
            .receive_advertisement(left_id, 0, advertisement)
            .unwrap();
    }
    for advertisement in right.advertisements_for(left_id, RouteMetric::DEFAULT_LINK) {
        left.receive_advertisement(right_id, 1, advertisement)
            .unwrap();
    }

    assert_eq!(left.learned_routes().len(), 1);
    assert_eq!(right.learned_routes().len(), 1);
    assert_eq!(left.learned_routes()[0].prefix, prefix(RIGHT_PREFIX));
    assert_eq!(right.learned_routes()[0].prefix, prefix(LEFT_PREFIX));

    let source = GdpAddress(LEFT_PREFIX | 0x0100);
    let destination = GdpAddress(RIGHT_PREFIX | 0x0100);
    match left.process(0, packet(source, destination)).unwrap() {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, 1);
            assert_eq!(packet.header.hop_limit, 15);
            assert_eq!(packet.header.destination(), destination);
        }
        other => panic!("learned route was not installed into P4: {other:?}"),
    }
}

#[test]
fn hold_timeout_reprograms_p4_to_alternate_and_removes_stale_route() {
    let local = RouterId::new(0x1000).unwrap();
    let preferred = RouterId::new(0x2000).unwrap();
    let alternate = RouterId::new(0x3000).unwrap();
    let destination_prefix = prefix(0x5505_0000_0000_0000);
    let destination = GdpAddress(0x5505_0000_0000_0123);
    let source = GdpAddress(0x6606_0000_0000_0456);

    let port0 = RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0100));
    let port1 = RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0101));
    let mut router = DynamicP4Router::new(local, vec![port0, port1], vec![]).unwrap();
    let mut preferred_adjacency =
        RouterAdjacency::new(local, LinkId::new(0x100).unwrap(), RouteMetric(100), 1_000).unwrap();
    preferred_adjacency
        .receive(
            Instant::ZERO,
            RoutingGctlMessage::router_hello(
                1,
                RouterHello {
                    router_id: preferred,
                    link_id: LinkId::new(0x200).unwrap(),
                    hold_time_ms: 1_000,
                    metric: RouteMetric(100),
                },
            ),
        )
        .unwrap();
    assert_eq!(preferred_adjacency.state(), NeighborState::Up);

    router
        .receive_advertisement(
            preferred,
            0,
            RouteAdvertise {
                advertiser: preferred,
                prefix: destination_prefix,
                origin: RouteOrigin::Learned,
                metric: RouteMetric(100),
            },
        )
        .unwrap();
    router
        .receive_advertisement(
            alternate,
            1,
            RouteAdvertise {
                advertiser: alternate,
                prefix: destination_prefix,
                origin: RouteOrigin::Learned,
                metric: RouteMetric(200),
            },
        )
        .unwrap();

    assert_eq!(router.learned_routes()[0].learned_from, Some(preferred));
    match router.process(1, packet(source, destination)).unwrap() {
        RouterDisposition::Forward { egress_port, .. } => assert_eq!(egress_port, 0),
        other => panic!("preferred learned route was not active: {other:?}"),
    }

    let changed = router
        .expire_adjacency(&mut preferred_adjacency, Instant::from_millis(1_000))
        .unwrap();
    assert_eq!(changed, vec![destination_prefix]);
    assert_eq!(preferred_adjacency.state(), NeighborState::Down);
    assert_eq!(router.learned_routes()[0].learned_from, Some(alternate));
    match router.process(0, packet(source, destination)).unwrap() {
        RouterDisposition::Forward { egress_port, .. } => assert_eq!(egress_port, 1),
        other => panic!("alternate learned route was not installed after timeout: {other:?}"),
    }

    let changed = router.neighbor_down(alternate).unwrap();
    assert_eq!(changed, vec![destination_prefix]);
    assert!(router.learned_routes().is_empty());
    assert_eq!(
        router.process(0, packet(source, destination)).unwrap(),
        RouterDisposition::Drop
    );

    router
        .receive_advertisement(
            preferred,
            0,
            RouteAdvertise {
                advertiser: preferred,
                prefix: destination_prefix,
                origin: RouteOrigin::Learned,
                metric: RouteMetric(100),
            },
        )
        .unwrap();
    match router.process(1, packet(source, destination)).unwrap() {
        RouterDisposition::Forward { egress_port, .. } => assert_eq!(egress_port, 0),
        other => panic!("restored preferred route was not reinstalled: {other:?}"),
    }
}

#[test]
fn four_router_failure_reconverges_end_to_end_over_alternate_path() {
    let a_id = RouterId::new(10).unwrap();
    let b_id = RouterId::new(20).unwrap();
    let c_id = RouterId::new(30).unwrap();
    let d_id = RouterId::new(40).unwrap();
    let destination_prefix = prefix(0x7707_0000_0000_0000);
    let destination = GdpAddress(0x7707_0000_0000_0001);
    let source = GdpAddress(0x8808_0000_0000_0001);

    let mut a = DynamicP4Router::new(
        a_id,
        vec![
            RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0a00)),
            RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0a01)),
        ],
        vec![],
    )
    .unwrap();
    let mut b = DynamicP4Router::new(
        b_id,
        vec![
            RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0b00)),
            RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0b01)),
        ],
        vec![],
    )
    .unwrap();
    let mut c = DynamicP4Router::new(
        c_id,
        vec![
            RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0c00)),
            RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0c01)),
        ],
        vec![],
    )
    .unwrap();
    let mut d = DynamicP4Router::new(
        d_id,
        vec![
            RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0d00))
                .with_connected_network(destination_prefix, destination),
            RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0d01)),
        ],
        vec![],
    )
    .unwrap();

    let d_to_b = d.advertisements_for(b_id, RouteMetric(100));
    let d_to_c = d.advertisements_for(c_id, RouteMetric(200));
    b.receive_advertisement(d_id, 1, d_to_b[0]).unwrap();
    c.receive_advertisement(d_id, 1, d_to_c[0]).unwrap();
    let b_to_a = b.advertisements_for(a_id, RouteMetric(50));
    let c_to_a = c.advertisements_for(a_id, RouteMetric(50));
    a.receive_advertisement(b_id, 0, b_to_a[0]).unwrap();
    a.receive_advertisement(c_id, 1, c_to_a[0]).unwrap();
    assert_eq!(a.learned_routes()[0].learned_from, Some(b_id));

    let preferred_packet = match a.process(1, packet(source, destination)).unwrap() {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, 0);
            packet
        }
        other => panic!("A did not select preferred path through B: {other:?}"),
    };
    let preferred_packet = match b.process(0, preferred_packet).unwrap() {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, 1);
            packet
        }
        other => panic!("B did not forward to D: {other:?}"),
    };
    assert!(matches!(
        d.process(0, preferred_packet).unwrap(),
        RouterDisposition::Punt { .. }
    ));

    let changed = b.neighbor_down(d_id).unwrap();
    let update = b.updates_for(a_id, RouteMetric(50), &changed);
    let withdrawal = match update.as_slice() {
        [RouteUpdate::Withdraw(withdrawal)] => *withdrawal,
        other => panic!("B did not trigger the expected withdrawal: {other:?}"),
    };
    assert!(a.receive_withdrawal(b_id, withdrawal).unwrap());
    assert_eq!(a.learned_routes()[0].learned_from, Some(c_id));

    let alternate_packet = match a.process(0, packet(source, destination)).unwrap() {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, 1);
            packet
        }
        other => panic!("A did not reconverge to C: {other:?}"),
    };
    let alternate_packet = match c.process(0, alternate_packet).unwrap() {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, 1);
            packet
        }
        other => panic!("C did not forward alternate traffic to D: {other:?}"),
    };
    assert!(matches!(
        d.process(1, alternate_packet).unwrap(),
        RouterDisposition::Punt { .. }
    ));

    b.receive_advertisement(d_id, 1, d_to_b[0]).unwrap();
    let restored = b.advertisements_for(a_id, RouteMetric(50));
    a.receive_advertisement(b_id, 0, restored[0]).unwrap();
    assert_eq!(a.learned_routes()[0].learned_from, Some(b_id));
}
