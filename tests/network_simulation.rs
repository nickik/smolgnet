use smolgnet::routing::GdpPrefix;
use smolgnet::wire::gctl::{
    AddressAck, AddressClaim, AddressOffer, Advertise, DiscoveryScope, GctlMessage, NodeAnnounce,
    ServiceType,
};
use smolgnet::wire::gdp::{GdpAddress, GdpHeader, GdpPacket, GdpType, SizeClass};
use smolgnet::{
    RouterDisposition, RouterPortConfig, RouterStartupConfig, StaticP4Router, StaticP4Switch,
    SwitchDisposition,
};

const LEFT_PREFIX: u64 = 0x1001_0000_0000_0000;
const RIGHT_PREFIX: u64 = 0x2002_0000_0000_0000;
const LEFT_ROUTER: GdpAddress = GdpAddress(LEFT_PREFIX | 0x0001);
const RIGHT_ROUTER: GdpAddress = GdpAddress(RIGHT_PREFIX | 0x0001);
const SWITCH_ROUTER_PORT: u16 = 7;

#[derive(Clone, Copy, Debug)]
struct SimEndpoint {
    address: GdpAddress,
    port: u16,
    nonce: u64,
    marker: u8,
}

fn left_endpoint(index: u16) -> SimEndpoint {
    SimEndpoint {
        address: GdpAddress(LEFT_PREFIX | 0x0100 | index as u64),
        port: index,
        nonce: 0x1000_0000_0000_0000 | index as u64,
        marker: 0x10 + index as u8,
    }
}

fn right_endpoint(index: u16) -> SimEndpoint {
    SimEndpoint {
        address: GdpAddress(RIGHT_PREFIX | 0x0100 | index as u64),
        port: index,
        nonce: 0x2000_0000_0000_0000 | index as u64,
        marker: 0x20 + index as u8,
    }
}

fn router() -> StaticP4Router {
    let left = RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0010)).with_connected_network(
        GdpPrefix::new(GdpAddress(LEFT_PREFIX), 48).unwrap(),
        LEFT_ROUTER,
    );
    let right = RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0020)).with_connected_network(
        GdpPrefix::new(GdpAddress(RIGHT_PREFIX), 48).unwrap(),
        RIGHT_ROUTER,
    );
    StaticP4Router::new(RouterStartupConfig::new(vec![left, right], vec![])).unwrap()
}

fn gctl_roundtrip(message: GctlMessage) -> GctlMessage {
    let size = message.recommended_size_class().unwrap().bytes();
    GctlMessage::decode(&message.encode_exact(size).unwrap()).unwrap()
}

fn announce(switch: &mut StaticP4Switch, endpoint: SimEndpoint) {
    let announced = NodeAnnounce {
        node: endpoint.address,
        nonce: endpoint.nonce,
        capabilities: 0,
    };
    let decoded = gctl_roundtrip(GctlMessage::node_announce(endpoint.port as u32 + 1, announced))
        .parse_node_announce()
        .unwrap();
    assert_eq!(decoded, announced);
    switch.register_node(decoded.node, endpoint.port).unwrap();
}

fn configure_address(
    router_address: GdpAddress,
    prefix: u64,
    endpoint: SimEndpoint,
    transaction: u32,
) {
    let offer = AddressOffer {
        router: router_address,
        prefix,
        prefix_len: 48,
        candidate: endpoint.address,
        lifetime: 3600,
    };
    let offered = gctl_roundtrip(GctlMessage::address_offer(transaction, offer).unwrap())
        .parse_address_offer()
        .unwrap();
    assert_eq!(offered, offer);

    let claim = AddressClaim {
        candidate: endpoint.address,
        nonce: endpoint.nonce,
    };
    let claimed = gctl_roundtrip(GctlMessage::address_claim(transaction + 1, claim))
        .parse_address_claim()
        .unwrap();
    assert_eq!(claimed, claim);

    let ack = AddressAck {
        candidate: endpoint.address,
        lifetime: 3600,
    };
    let acknowledged = gctl_roundtrip(GctlMessage::address_ack(transaction + 2, ack))
        .parse_address_ack()
        .unwrap();
    assert_eq!(acknowledged, ack);
}

fn discover_router(
    switch: &mut StaticP4Switch,
    router_port: u16,
    router_address: GdpAddress,
    attached: &[SimEndpoint],
) {
    let solicit = gctl_roundtrip(GctlMessage::solicit(
        0x100,
        ServiceType::ROUTER,
        DiscoveryScope::Link,
    ));
    assert_eq!(
        solicit.parse_solicit().unwrap().service_type,
        ServiceType::ROUTER
    );

    let advertise = Advertise {
        service_type: ServiceType::ROUTER,
        preference: 1,
        provider: router_address,
        lifetime: 600,
        capabilities: 0,
    };
    let learned = gctl_roundtrip(GctlMessage::advertise(0x101, advertise))
        .parse_advertise()
        .unwrap();
    assert_eq!(learned, advertise);
    switch.set_default_router_port(router_port).unwrap();

    let replay = gctl_roundtrip(GctlMessage::reannounce_solicit(
        0x102,
        DiscoveryScope::Link,
    ));
    assert_eq!(
        replay.parse_reannounce_solicit().unwrap().scope,
        DiscoveryScope::Link
    );

    for &endpoint in attached {
        announce(switch, endpoint);
    }
}

fn data_packet(source: SimEndpoint, destination: SimEndpoint, hop: u8) -> GdpPacket {
    let mut payload = vec![source.marker; 32];
    payload[0..8].copy_from_slice(&source.address.0.to_be_bytes());
    payload[8..16].copy_from_slice(&destination.address.0.to_be_bytes());
    payload[16] = source.marker;
    let header = GdpHeader::global(
        GdpType::Gts,
        SizeClass::Ctrl32,
        hop,
        source.address,
        destination.address,
    );
    GdpPacket::new(header, payload).unwrap()
}

fn assert_payload(packet: &GdpPacket, source: SimEndpoint, destination: SimEndpoint) {
    assert_eq!(packet.header.source(), source.address);
    assert_eq!(packet.header.destination(), destination.address);
    assert_eq!(
        u64::from_be_bytes(packet.payload[0..8].try_into().unwrap()),
        source.address.0
    );
    assert_eq!(
        u64::from_be_bytes(packet.payload[8..16].try_into().unwrap()),
        destination.address.0
    );
    assert_eq!(packet.payload[16], source.marker);
}

fn send_on_switch(
    switch: &mut StaticP4Switch,
    source: SimEndpoint,
    destination: SimEndpoint,
) {
    let packet = data_packet(source, destination, 12);
    match switch.process(source.port, packet).unwrap() {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, destination.port);
            assert_eq!(packet.header.hop_limit, 12);
            assert_payload(&packet, source, destination);
        }
        other => panic!("same-segment delivery failed: {other:?}"),
    }
}

fn send_left_to_direct_right(
    switch: &mut StaticP4Switch,
    router: &mut StaticP4Router,
    source: SimEndpoint,
    destination: SimEndpoint,
) {
    let packet = data_packet(source, destination, 12);
    let packet = match switch.process(source.port, packet).unwrap() {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, SWITCH_ROUTER_PORT);
            packet
        }
        other => panic!("switch did not choose discovered router: {other:?}"),
    };
    match router.process(0, packet).unwrap() {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, 1);
            assert_eq!(packet.header.hop_limit, 11);
            assert_payload(&packet, source, destination);
        }
        other => panic!("router failed left-to-right delivery: {other:?}"),
    }
}

fn send_direct_right_to_left(
    switch: &mut StaticP4Switch,
    router: &mut StaticP4Router,
    source: SimEndpoint,
    destination: SimEndpoint,
) {
    let packet = data_packet(source, destination, 12);
    let packet = match router.process(1, packet).unwrap() {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, 0);
            assert_eq!(packet.header.hop_limit, 11);
            packet
        }
        other => panic!("router failed right-to-left delivery: {other:?}"),
    };
    match switch.process(SWITCH_ROUTER_PORT, packet).unwrap() {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, destination.port);
            assert_eq!(packet.header.hop_limit, 11);
            assert_payload(&packet, source, destination);
        }
        other => panic!("switch failed routed local delivery: {other:?}"),
    }
}

fn send_between_switches(
    left: &mut StaticP4Switch,
    right: &mut StaticP4Switch,
    router: &mut StaticP4Router,
    source: SimEndpoint,
    destination: SimEndpoint,
    left_to_right: bool,
) {
    let (source_switch, destination_switch, ingress_router, egress_router) = if left_to_right {
        (left, right, 0, 1)
    } else {
        (right, left, 1, 0)
    };

    let packet = data_packet(source, destination, 12);
    let packet = match source_switch.process(source.port, packet).unwrap() {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, SWITCH_ROUTER_PORT);
            packet
        }
        other => panic!("source switch did not choose router: {other:?}"),
    };
    let packet = match router.process(ingress_router, packet).unwrap() {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, egress_router);
            assert_eq!(packet.header.hop_limit, 11);
            packet
        }
        other => panic!("router failed cross-segment delivery: {other:?}"),
    };
    match destination_switch
        .process(SWITCH_ROUTER_PORT, packet)
        .unwrap()
    {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, destination.port);
            assert_eq!(packet.header.hop_limit, 11);
            assert_payload(&packet, source, destination);
        }
        other => panic!("destination switch failed delivery: {other:?}"),
    }
}

fn shuffled(mut values: Vec<usize>, mut seed: u64) -> Vec<usize> {
    for i in (1..values.len()).rev() {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let j = (seed as usize) % (i + 1);
        values.swap(i, j);
    }
    values
}

#[test]
fn downstream_only_random_attachment_orders_discover_and_exchange_data() {
    for seed in [1, 7, 0x1234, 0xfeed_beef] {
        let endpoints: Vec<_> = (0..5).map(left_endpoint).collect();
        let mut switch = StaticP4Switch::new();
        let mut router_present = false;
        let mut attached = Vec::new();

        // 0 denotes router, 1..=5 denote endpoints.
        for actor in shuffled((0..=5).collect(), seed) {
            if actor == 0 {
                router_present = true;
                discover_router(&mut switch, SWITCH_ROUTER_PORT, LEFT_ROUTER, &attached);
                for (i, endpoint) in attached.iter().copied().enumerate() {
                    configure_address(LEFT_ROUTER, LEFT_PREFIX, endpoint, 0x200 + (i as u32 * 4));
                }
            } else {
                let endpoint = endpoints[actor - 1];
                announce(&mut switch, endpoint);
                attached.push(endpoint);
                if router_present {
                    configure_address(
                        LEFT_ROUTER,
                        LEFT_PREFIX,
                        endpoint,
                        0x300 + actor as u32 * 4,
                    );
                }
            }
        }

        assert!(router_present);
        assert_eq!(switch.default_router_port(), Some(SWITCH_ROUTER_PORT));
        for &source in &endpoints {
            for &destination in &endpoints {
                if source.address != destination.address {
                    send_on_switch(&mut switch, source, destination);
                }
            }
        }
    }
}

#[test]
fn downstream_switch_and_direct_upstream_endpoint_route_both_directions() {
    for seed in [2, 9, 0x55aa] {
        let downstream: Vec<_> = (0..4).map(left_endpoint).collect();
        let upstream = right_endpoint(0);
        let mut switch = StaticP4Switch::new();
        let mut router = router();
        let mut router_present = false;
        let mut downstream_attached = Vec::new();
        let mut upstream_attached = false;

        // 0 router, 1..=4 downstream nodes, 5 direct upstream node.
        for actor in shuffled((0..=5).collect(), seed) {
            match actor {
                0 => {
                    router_present = true;
                    discover_router(
                        &mut switch,
                        SWITCH_ROUTER_PORT,
                        LEFT_ROUTER,
                        &downstream_attached,
                    );
                    for (i, endpoint) in downstream_attached.iter().copied().enumerate() {
                        configure_address(
                            LEFT_ROUTER,
                            LEFT_PREFIX,
                            endpoint,
                            0x400 + i as u32 * 4,
                        );
                    }
                    if upstream_attached {
                        configure_address(RIGHT_ROUTER, RIGHT_PREFIX, upstream, 0x500);
                    }
                }
                5 => {
                    upstream_attached = true;
                    let announce_msg = NodeAnnounce {
                        node: upstream.address,
                        nonce: upstream.nonce,
                        capabilities: 0,
                    };
                    assert_eq!(
                        gctl_roundtrip(GctlMessage::node_announce(0x510, announce_msg))
                            .parse_node_announce()
                            .unwrap(),
                        announce_msg
                    );
                    if router_present {
                        configure_address(RIGHT_ROUTER, RIGHT_PREFIX, upstream, 0x520);
                    }
                }
                _ => {
                    let endpoint = downstream[actor - 1];
                    announce(&mut switch, endpoint);
                    downstream_attached.push(endpoint);
                    if router_present {
                        configure_address(
                            LEFT_ROUTER,
                            LEFT_PREFIX,
                            endpoint,
                            0x600 + actor as u32 * 4,
                        );
                    }
                }
            }
        }

        assert!(router_present && upstream_attached);
        for &endpoint in &downstream {
            send_left_to_direct_right(&mut switch, &mut router, endpoint, upstream);
            send_direct_right_to_left(&mut switch, &mut router, upstream, endpoint);
        }
    }
}

#[test]
fn two_switched_populations_random_order_all_pairs_exchange_unique_data() {
    for seed in [3, 11, 0xdead_beef, 0x0123_4567_89ab_cdef] {
        let left_nodes: Vec<_> = (0..3).map(left_endpoint).collect();
        let right_nodes: Vec<_> = (0..3).map(right_endpoint).collect();
        let mut left = StaticP4Switch::new();
        let mut right = StaticP4Switch::new();
        let mut router = router();
        let mut router_present = false;
        let mut left_attached = Vec::new();
        let mut right_attached = Vec::new();

        // 0 router, 1..=3 left nodes, 4..=6 right nodes.
        for actor in shuffled((0..=6).collect(), seed) {
            if actor == 0 {
                router_present = true;
                discover_router(&mut left, SWITCH_ROUTER_PORT, LEFT_ROUTER, &left_attached);
                discover_router(
                    &mut right,
                    SWITCH_ROUTER_PORT,
                    RIGHT_ROUTER,
                    &right_attached,
                );
                for (i, endpoint) in left_attached.iter().copied().enumerate() {
                    configure_address(
                        LEFT_ROUTER,
                        LEFT_PREFIX,
                        endpoint,
                        0x700 + i as u32 * 4,
                    );
                }
                for (i, endpoint) in right_attached.iter().copied().enumerate() {
                    configure_address(
                        RIGHT_ROUTER,
                        RIGHT_PREFIX,
                        endpoint,
                        0x800 + i as u32 * 4,
                    );
                }
            } else if actor <= 3 {
                let endpoint = left_nodes[actor - 1];
                announce(&mut left, endpoint);
                left_attached.push(endpoint);
                if router_present {
                    configure_address(
                        LEFT_ROUTER,
                        LEFT_PREFIX,
                        endpoint,
                        0x900 + actor as u32 * 4,
                    );
                }
            } else {
                let endpoint = right_nodes[actor - 4];
                announce(&mut right, endpoint);
                right_attached.push(endpoint);
                if router_present {
                    configure_address(
                        RIGHT_ROUTER,
                        RIGHT_PREFIX,
                        endpoint,
                        0xa00 + actor as u32 * 4,
                    );
                }
            }
        }

        assert_eq!(left.default_router_port(), Some(SWITCH_ROUTER_PORT));
        assert_eq!(right.default_router_port(), Some(SWITCH_ROUTER_PORT));

        for &source in &left_nodes {
            for &destination in &left_nodes {
                if source.address != destination.address {
                    send_on_switch(&mut left, source, destination);
                }
            }
        }
        for &source in &right_nodes {
            for &destination in &right_nodes {
                if source.address != destination.address {
                    send_on_switch(&mut right, source, destination);
                }
            }
        }
        for &left_node in &left_nodes {
            for &right_node in &right_nodes {
                send_between_switches(
                    &mut left,
                    &mut right,
                    &mut router,
                    left_node,
                    right_node,
                    true,
                );
                send_between_switches(
                    &mut left,
                    &mut right,
                    &mut router,
                    right_node,
                    left_node,
                    false,
                );
            }
        }
    }
}
