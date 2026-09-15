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

fn router() -> StaticP4Router {
    let access = RouterPortConfig::new(
        0,
        RouterAttachment::Switch,
        GdpAddress(0xfe80_0000_0000_0001),
    )
    .with_connected_network(
        prefix(0x1200_0000_0000_0000, 16),
        GdpAddress(0x1200_0000_0000_0001),
    );
    let transit = RouterPortConfig::new(
        1,
        RouterAttachment::Direct,
        GdpAddress(0xfe80_0000_0000_0002),
    );
    let routes = vec![
        StaticRouteConfig::direct(prefix(0x1200_5678_9abc_0000, 48), 1),
        StaticRouteConfig::via(
            GdpPrefix::default_route(),
            1,
            GdpAddress(0xfe80_0000_0000_00ff),
        ),
    ];
    StaticP4Router::new(RouterStartupConfig::new(vec![access, transit], routes)).unwrap()
}

#[test]
fn startup_config_builds_management_and_fib_state() {
    let router = router();
    assert_eq!(router.physical_port_count(), 2);
    assert_eq!(router.cpu_port(), 2);
    assert_eq!(router.fib_generation(), 1);

    let p0 = router.port(0).unwrap();
    assert_eq!(p0.switch_registration, SwitchRegistrationState::Unregistered);
    assert!(p0.up);
    assert_eq!(
        router.port(1).unwrap().switch_registration,
        SwitchRegistrationState::NotRequired
    );

    assert!(router.fib().iter().any(|entry| {
        entry.origin == FibOrigin::Bootstrap
            && entry.prefix == prefix(ROUTER_BOOTSTRAP_ADDRESS.0, 64)
            && entry.egress_port.is_none()
    }));
    assert!(router.fib().iter().any(|entry| {
        entry.origin == FibOrigin::Connected
            && entry.prefix == prefix(0x1200_0000_0000_0000, 16)
            && entry.egress_port == Some(0)
    }));
    assert!(router.fib().iter().any(|entry| {
        entry.origin == FibOrigin::Static
            && entry.prefix == GdpPrefix::default_route()
            && entry.egress_port == Some(1)
    }));
}

#[test]
fn p4_fast_path_does_lpm_and_decrements_hop() {
    let mut router = router();

    // Ordinary connected-prefix traffic stays on the access-side port.
    let connected = router
        .process(1, packet(0x3300_0000_0000_0001, 0x1200_1111_2222_3333, 8))
        .unwrap();
    match connected {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, 0);
            assert_eq!(packet.header.destination(), GdpAddress(0x1200_1111_2222_3333));
            assert_eq!(packet.header.hop_limit, 7);
        }
        other => panic!("expected connected forward, got {other:?}"),
    }

    // The /48 static route overrides the covering connected /16.
    let specific = router
        .process(0, packet(0x1200_0000_0000_0042, 0x1200_5678_9abc_1234, 9))
        .unwrap();
    match specific {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, 1);
            assert_eq!(packet.header.destination(), GdpAddress(0x1200_5678_9abc_1234));
            assert_eq!(packet.header.hop_limit, 8);
        }
        other => panic!("expected /48 forward, got {other:?}"),
    }

    // Everything else falls through to the configured /0 route.
    let defaulted = router
        .process(0, packet(0x1200_0000_0000_0042, 0x9900_0000_0000_0001, 5))
        .unwrap();
    match defaulted {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, 1);
            assert_eq!(packet.header.hop_limit, 4);
        }
        other => panic!("expected default forward, got {other:?}"),
    }
}

#[test]
fn router_owned_and_bootstrap_addresses_are_punted_by_p4() {
    let mut router = router();

    for destination in [
        ROUTER_BOOTSTRAP_ADDRESS,
        GdpAddress(0xfe80_0000_0000_0001),
        GdpAddress(0x1200_0000_0000_0001),
    ] {
        let result = router
            .process(0, packet(0xfe80_0000_0000_0042, destination.0, 8))
            .unwrap();
        match result {
            RouterDisposition::Punt {
                ingress_port,
                packet,
            } => {
                assert_eq!(ingress_port, 0);
                assert_eq!(packet.header.destination(), destination);
                // Local CPU delivery is not transit forwarding.
                assert_eq!(packet.header.hop_limit, 8);
            }
            other => panic!("expected CPU punt for {destination:?}, got {other:?}"),
        }
    }
}

#[test]
fn local_form_router_destination_is_punted_not_transit_routed() {
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

    match router.process(0, packet).unwrap() {
        RouterDisposition::Punt {
            ingress_port,
            packet,
        } => {
            assert_eq!(ingress_port, 0);
            assert_eq!(packet.header.hop_limit, 7);
        }
        other => panic!("expected local-form CPU punt, got {other:?}"),
    }
}

#[test]
fn expired_hop_is_dropped_in_p4() {
    let mut router = router();
    assert_eq!(
        router
            .process(0, packet(0x1200_0000_0000_0042, 0x9900_0000_0000_0001, 1))
            .unwrap(),
        RouterDisposition::Drop
    );
}

#[test]
fn node_database_records_management_state_without_host_fib_entries() {
    let mut router = router();
    let before_fib = router.fib().to_vec();
    let link_local = GdpAddress(0xfe80_0000_0000_0042);
    let routed = GdpAddress(0x1200_0000_0000_0042);

    let observed = router.record_node(0, link_local).unwrap();
    assert_eq!(observed.state, NodeState::Observed);
    assert_eq!(observed.routed_address, None);

    let configured = router.configure_node_address(link_local, routed).unwrap();
    assert_eq!(configured.state, NodeState::Configured);
    assert_eq!(configured.routed_address, Some(routed));
    assert_eq!(router.node(link_local).unwrap(), configured);
    assert_eq!(router.node_by_routed_address(routed).unwrap(), configured);

    // A host lease is management state. The connected-prefix P4 entry already
    // sends every address in this LAN to the attached switch.
    assert_eq!(router.fib(), before_fib.as_slice());
    assert_eq!(router.fib_generation(), 1);
}

#[test]
fn startup_validation_rejects_ambiguous_or_impossible_config() {
    let p0 = RouterPortConfig::new(
        1,
        RouterAttachment::Direct,
        GdpAddress(0xfe80_0000_0000_0001),
    );
    assert_eq!(
        StaticP4Router::new(RouterStartupConfig::new(vec![p0], vec![]))
            .err()
            .unwrap(),
        Error::InvalidField
    );

    let bad_prefix = RouterPortConfig::new(
        0,
        RouterAttachment::Switch,
        GdpAddress(0xfe80_0000_0000_0001),
    )
    .with_connected_network(
        prefix(0x1200_0000_0000_0000, 24),
        GdpAddress(0x1200_0000_0000_0001),
    );
    assert_eq!(
        StaticP4Router::new(RouterStartupConfig::new(vec![bad_prefix], vec![]))
            .err()
            .unwrap(),
        Error::InvalidField
    );
}
