use smolgnet::*;

fn prefix(address: u64, len: u8) -> GdpPrefix {
    GdpPrefix::new(GdpAddress(address), len).unwrap()
}

fn packet(source: u64, destination: u64, hop: u8) -> GdpPacket {
    GdpPacket::new(
        GdpHeader::global(
            GdpType::Gts,
            SizeClass::Ctrl32,
            hop,
            GdpAddress(source),
            GdpAddress(destination),
        ),
        vec![0x5a; 32],
    )
    .unwrap()
}

fn port(id: u16) -> RouterPortConfig {
    RouterPortConfig::new(id, GdpAddress(0xfe80_0000_0000_0001 + id as u64))
}

fn router() -> StaticP4Router {
    let p0 = port(0).with_connected_network(
        prefix(0x1200_0000_0000_0000, 16),
        GdpAddress(0x1200_0000_0000_0001),
    );
    let p1 = port(1);
    let routes = vec![
        StaticRouteConfig::direct(prefix(0x1200_5678_9abc_0000, 48), 1),
        StaticRouteConfig::via(
            GdpPrefix::default_route(),
            1,
            GdpAddress(0xfe80_0000_0000_00ff),
        ),
    ];
    StaticP4Router::new(RouterStartupConfig::new(vec![p0, p1], routes)).unwrap()
}

fn expect_forward(
    result: RouterDisposition,
    expected_port: u16,
    expected_destination: u64,
    expected_hop: u8,
) {
    match result {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, expected_port);
            assert_eq!(
                packet.header.destination(),
                GdpAddress(expected_destination)
            );
            assert_eq!(packet.header.hop_limit, expected_hop);
        }
        other => panic!("expected forward to port {expected_port}, got {other:?}"),
    }
}

#[test]
fn router_requires_exactly_two_physical_ports() {
    assert_eq!(ROUTER_PORT_COUNT, 2);

    let one = RouterStartupConfig::new(vec![port(0)], vec![]);
    assert_eq!(one.validate(), Err(Error::InvalidField));

    let three = RouterStartupConfig::new(vec![port(0), port(1), port(2)], vec![]);
    assert_eq!(three.validate(), Err(Error::InvalidField));

    let wrong_ids = RouterStartupConfig::new(vec![port(1), port(0)], vec![]);
    assert_eq!(wrong_ids.validate(), Err(Error::InvalidField));

    let two = RouterStartupConfig::new(vec![port(0), port(1)], vec![]);
    assert_eq!(two.validate(), Ok(()));
    assert_eq!(StaticP4Router::new(two).unwrap().physical_port_count(), 2);
}

#[test]
fn two_port_router_forwards_bidirectionally_and_preserves_destination() {
    let mut router = router();

    let to_wan = 0x9900_0000_0000_0001;
    expect_forward(
        router
            .process(0, packet(0x1200_0000_0000_0042, to_wan, 9))
            .unwrap(),
        1,
        to_wan,
        8,
    );

    let to_lan = 0x1200_1111_2222_3333;
    expect_forward(
        router
            .process(1, packet(0x9900_0000_0000_0001, to_lan, 8))
            .unwrap(),
        0,
        to_lan,
        7,
    );
}

#[test]
fn lpm_specific_route_overrides_connected_and_default_routes() {
    let mut router = router();
    let destination = 0x1200_5678_9abc_1234;
    expect_forward(
        router
            .process(0, packet(0x1200_0000_0000_0042, destination, 6))
            .unwrap(),
        1,
        destination,
        5,
    );
}

#[test]
fn split_64_bit_lpm_handles_1_32_33_63_and_64_boundaries() {
    let routes = vec![
        StaticRouteConfig::direct(prefix(0x8000_0000_0000_0000, 1), 0),
        StaticRouteConfig::direct(prefix(0x4000_0000_0000_0000, 32), 0),
        StaticRouteConfig::direct(prefix(0x5000_0000_0000_0000, 33), 1),
        StaticRouteConfig::direct(prefix(0x6000_0000_0000_0000, 63), 0),
        StaticRouteConfig::direct(prefix(0x7000_0000_0000_0001, 64), 1),
        StaticRouteConfig::direct(GdpPrefix::default_route(), 1),
    ];
    let mut router =
        StaticP4Router::new(RouterStartupConfig::new(vec![port(0), port(1)], routes)).unwrap();

    expect_forward(
        router
            .process(1, packet(0x1000_0000_0000_0001, 0x9000_0000_1234_5678, 10))
            .unwrap(),
        0,
        0x9000_0000_1234_5678,
        9,
    );
    expect_forward(
        router
            .process(1, packet(0x8000_0000_0000_0001, 0x4000_0000_dead_beef, 10))
            .unwrap(),
        0,
        0x4000_0000_dead_beef,
        9,
    );
    expect_forward(
        router
            .process(0, packet(0x8000_0000_0000_0001, 0x5000_0000_1234_5678, 10))
            .unwrap(),
        1,
        0x5000_0000_1234_5678,
        9,
    );
    expect_forward(
        router
            .process(1, packet(0x8000_0000_0000_0001, 0x6000_0000_0000_0001, 10))
            .unwrap(),
        0,
        0x6000_0000_0000_0001,
        9,
    );
    expect_forward(
        router
            .process(0, packet(0x8000_0000_0000_0001, 0x7000_0000_0000_0001, 10))
            .unwrap(),
        1,
        0x7000_0000_0000_0001,
        9,
    );
}

#[test]
fn router_owned_and_bootstrap_addresses_override_covering_routes() {
    let mut router = router();
    for destination in [
        ROUTER_BOOTSTRAP_ADDRESS,
        GdpAddress(0xfe80_0000_0000_0001),
        GdpAddress(0x1200_0000_0000_0001),
    ] {
        match router
            .process(0, packet(0xfe80_0000_0000_0042, destination.0, 8))
            .unwrap()
        {
            RouterDisposition::Punt {
                ingress_port,
                packet,
            } => {
                assert_eq!(ingress_port, 0);
                assert_eq!(packet.header.destination(), destination);
                assert_eq!(packet.header.hop_limit, 8);
            }
            other => panic!("expected CPU punt for {destination:?}, got {other:?}"),
        }
    }
}

#[test]
fn local_form_is_never_transit_routed() {
    let mut router = router();
    let header = GdpHeader::local(
        GdpType::Gctl,
        SizeClass::Ctrl32,
        7,
        0xfe80_0000_0000_0000,
        0x0042,
        0x0001,
    )
    .unwrap();
    let packet = GdpPacket::new(header, vec![0; 32]).unwrap();
    assert!(matches!(
        router.process(0, packet).unwrap(),
        RouterDisposition::Punt {
            ingress_port: 0,
            ..
        }
    ));
}

#[test]
fn expired_hops_same_port_hairpin_and_invalid_ingress_are_rejected() {
    let mut router = router();
    for hop in [0, 1] {
        assert_eq!(
            router
                .process(0, packet(0x1200_0000_0000_0042, 0x9900_0000_0000_0001, hop))
                .unwrap(),
            RouterDisposition::Drop
        );
    }
    assert_eq!(
        router
            .process(1, packet(0x3300_0000_0000_0001, 0x9900_0000_0000_0001, 8))
            .unwrap(),
        RouterDisposition::Drop
    );
    assert_eq!(
        router
            .process(2, packet(0x3300_0000_0000_0001, 0x9900_0000_0000_0001, 8))
            .unwrap_err(),
        Error::InvalidField
    );
}

#[test]
fn static_route_validation_rejects_invalid_ports_duplicates_and_next_hops() {
    let bad_port = RouterStartupConfig::new(
        vec![port(0), port(1)],
        vec![StaticRouteConfig::direct(GdpPrefix::default_route(), 2)],
    );
    assert_eq!(bad_port.validate(), Err(Error::InvalidField));

    let duplicate = RouterStartupConfig::new(
        vec![port(0), port(1)],
        vec![
            StaticRouteConfig::direct(prefix(0x4400_0000_0000_0000, 16), 0),
            StaticRouteConfig::direct(prefix(0x4400_0000_0000_0000, 16), 1),
        ],
    );
    assert_eq!(duplicate.validate(), Err(Error::InvalidField));

    let connected_prefix = prefix(0x1200_0000_0000_0000, 16);
    let connected =
        port(0).with_connected_network(connected_prefix, GdpAddress(0x1200_0000_0000_0001));
    let duplicate_connected = RouterStartupConfig::new(
        vec![connected, port(1)],
        vec![StaticRouteConfig::direct(connected_prefix, 1)],
    );
    assert_eq!(duplicate_connected.validate(), Err(Error::InvalidField));

    let valid_next_hop = RouterStartupConfig::new(
        vec![port(0), port(1)],
        vec![StaticRouteConfig::via(
            GdpPrefix::default_route(),
            1,
            GdpAddress(0xfe80_0000_0000_00ff),
        )],
    );
    assert_eq!(valid_next_hop.validate(), Ok(()));

    for next_hop in [
        GdpAddress(0x9900_0000_0000_0001),
        ROUTER_BOOTSTRAP_ADDRESS,
        GdpAddress(0xfe80_0000_0000_0001),
    ] {
        let invalid = RouterStartupConfig::new(
            vec![port(0), port(1)],
            vec![StaticRouteConfig::via(
                GdpPrefix::default_route(),
                1,
                next_hop,
            )],
        );
        assert_eq!(invalid.validate(), Err(Error::InvalidField));
    }
}

#[test]
fn static_64_routes_cannot_capture_any_router_owned_or_bootstrap_address() {
    let connected_prefix = prefix(0x1200_0000_0000_0000, 16);
    let routed_address = GdpAddress(0x1200_0000_0000_0001);

    for destination in [
        ROUTER_BOOTSTRAP_ADDRESS,
        GdpAddress(0xfe80_0000_0000_0001),
        routed_address,
    ] {
        let p0 = port(0).with_connected_network(connected_prefix, routed_address);
        let config = RouterStartupConfig::new(
            vec![p0, port(1)],
            vec![StaticRouteConfig::direct(prefix(destination.0, 64), 1)],
        );
        assert_eq!(config.validate(), Err(Error::InvalidField));
    }
}
