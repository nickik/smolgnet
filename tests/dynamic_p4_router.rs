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
fn failed_neighbor_reprograms_p4_to_alternate_and_removes_stale_route() {
    let local = RouterId::new(0x1000).unwrap();
    let preferred = RouterId::new(0x2000).unwrap();
    let alternate = RouterId::new(0x3000).unwrap();
    let destination_prefix = prefix(0x5505_0000_0000_0000);
    let destination = GdpAddress(0x5505_0000_0000_0123);
    let source = GdpAddress(0x6606_0000_0000_0456);

    let port0 = RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0100));
    let port1 = RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0101));
    let mut router = DynamicP4Router::new(local, vec![port0, port1], vec![]).unwrap();

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

    let changed = router.neighbor_down(preferred).unwrap();
    assert_eq!(changed, vec![destination_prefix]);
    assert_eq!(router.learned_routes()[0].learned_from, Some(alternate));
    match router.process(0, packet(source, destination)).unwrap() {
        RouterDisposition::Forward { egress_port, .. } => assert_eq!(egress_port, 1),
        other => panic!("alternate learned route was not installed after failure: {other:?}"),
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
