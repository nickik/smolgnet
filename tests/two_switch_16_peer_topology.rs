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

const LEFT_PREFIX: u64 = 0x1a2b_3c00_0000_0000;
const RIGHT_PREFIX: u64 = 0x9abc_de00_0000_0000;
const LEFT_ROUTER: GdpAddress = GdpAddress(LEFT_PREFIX | 0x0001);
const RIGHT_ROUTER: GdpAddress = GdpAddress(RIGHT_PREFIX | 0x0001);
const ROUTER_SWITCH_PORT: u16 = 8;
const SIM_SWITCH_PORTS: u16 = 9;
const PEERS_PER_SIDE: usize = 8;

#[derive(Clone, Copy, Debug)]
struct Peer {
    address: GdpAddress,
    port: u16,
    nonce: u64,
    marker: u8,
}

fn peer(prefix: u64, side_marker: u8, index: usize) -> Peer {
    Peer {
        address: GdpAddress(prefix | 0x0100 | index as u64),
        port: index as u16,
        nonce: ((side_marker as u64) << 56) | 0x0000_1234_0000_0000 | index as u64,
        marker: side_marker.wrapping_add(index as u8),
    }
}

fn router() -> StaticP4Router {
    let left = RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0010))
        .with_connected_network(
            GdpPrefix::new(GdpAddress(LEFT_PREFIX), 48).unwrap(),
            LEFT_ROUTER,
        );
    let right = RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0020))
        .with_connected_network(
            GdpPrefix::new(GdpAddress(RIGHT_PREFIX), 48).unwrap(),
            RIGHT_ROUTER,
        );
    StaticP4Router::new(RouterStartupConfig::new(vec![left, right], vec![])).unwrap()
}

fn gctl_roundtrip(message: GctlMessage) -> GctlMessage {
    let size = message.recommended_size_class().unwrap().bytes();
    GctlMessage::decode(&message.encode_exact(size).unwrap()).unwrap()
}

fn announce(switch: &mut StaticP4Switch, endpoint: Peer, transaction: u32) {
    let announce = NodeAnnounce {
        node: endpoint.address,
        nonce: endpoint.nonce,
        capabilities: 0,
    };
    let decoded = gctl_roundtrip(GctlMessage::node_announce(transaction, announce))
        .parse_node_announce()
        .unwrap();
    assert_eq!(decoded, announce);
    switch.register_node(endpoint.address, endpoint.port).unwrap();
}

fn configure_address(
    router_address: GdpAddress,
    prefix: u64,
    endpoint: Peer,
    transaction: u32,
) {
    let offer = AddressOffer {
        router: router_address,
        prefix,
        prefix_len: 48,
        candidate: endpoint.address,
        lifetime: 3600,
    };
    assert_eq!(
        gctl_roundtrip(GctlMessage::address_offer(transaction, offer).unwrap())
            .parse_address_offer()
            .unwrap(),
        offer
    );

    let claim = AddressClaim {
        candidate: endpoint.address,
        nonce: endpoint.nonce,
    };
    assert_eq!(
        gctl_roundtrip(GctlMessage::address_claim(transaction + 1, claim))
            .parse_address_claim()
            .unwrap(),
        claim
    );

    let ack = AddressAck {
        candidate: endpoint.address,
        lifetime: 3600,
    };
    assert_eq!(
        gctl_roundtrip(GctlMessage::address_ack(transaction + 2, ack))
            .parse_address_ack()
            .unwrap(),
        ack
    );
}

fn discover_router(
    switch: &mut StaticP4Switch,
    provider: GdpAddress,
    transaction: u32,
) {
    let solicit = gctl_roundtrip(GctlMessage::solicit(
        transaction,
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
        provider,
        lifetime: 600,
        capabilities: 0,
    };
    assert_eq!(
        gctl_roundtrip(GctlMessage::advertise(transaction + 1, advertise))
            .parse_advertise()
            .unwrap(),
        advertise
    );
    switch.set_default_router_port(ROUTER_SWITCH_PORT).unwrap();

    assert_eq!(
        gctl_roundtrip(GctlMessage::reannounce_solicit(
            transaction + 2,
            DiscoveryScope::Link,
        ))
        .parse_reannounce_solicit()
        .unwrap()
        .scope,
        DiscoveryScope::Link
    );
}

fn payload(source: Peer, destination: Peer, form: u8) -> Vec<u8> {
    let mut payload = vec![source.marker; 32];
    payload[0..8].copy_from_slice(&source.address.0.to_be_bytes());
    payload[8..16].copy_from_slice(&destination.address.0.to_be_bytes());
    payload[16] = source.marker;
    payload[17] = destination.marker;
    payload[18] = form;
    payload
}

fn global_packet(source: Peer, destination: Peer, hop: u8) -> GdpPacket {
    GdpPacket::new(
        GdpHeader::global(
            GdpType::Gts,
            SizeClass::Ctrl32,
            hop,
            source.address,
            destination.address,
        ),
        payload(source, destination, 0),
    )
    .unwrap()
}

fn local_packet(prefix: u64, source: Peer, destination: Peer, hop: u8) -> GdpPacket {
    GdpPacket::new(
        GdpHeader::local(
            GdpType::Gts,
            SizeClass::Ctrl32,
            hop,
            prefix,
            source.address.0 as u16,
            destination.address.0 as u16,
        )
        .unwrap(),
        payload(source, destination, 1),
    )
    .unwrap()
}

fn assert_packet(
    packet: &GdpPacket,
    source: Peer,
    destination: Peer,
    hop: u8,
    form: u8,
) {
    assert_eq!(packet.header.source(), source.address);
    assert_eq!(packet.header.destination(), destination.address);
    assert_eq!(packet.header.hop_limit, hop);
    assert_eq!(
        u64::from_be_bytes(packet.payload[0..8].try_into().unwrap()),
        source.address.0
    );
    assert_eq!(
        u64::from_be_bytes(packet.payload[8..16].try_into().unwrap()),
        destination.address.0
    );
    assert_eq!(packet.payload[16], source.marker);
    assert_eq!(packet.payload[17], destination.marker);
    assert_eq!(packet.payload[18], form);
}

fn send_same_segment(
    switch: &mut StaticP4Switch,
    source: Peer,
    destination: Peer,
    prefix: u64,
) {
    let global = global_packet(source, destination, 12);
    match switch.process(source.port, global).unwrap() {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, destination.port);
            assert_packet(&packet, source, destination, 12, 0);
        }
        other => panic!("same-segment global delivery failed: {other:?}"),
    }

    let local = local_packet(prefix, source, destination, 12);
    match switch.process(source.port, local).unwrap() {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, destination.port);
            assert_packet(&packet, source, destination, 12, 1);
        }
        other => panic!("same-segment local delivery failed: {other:?}"),
    }
}

fn send_cross_segment(
    source_switch: &mut StaticP4Switch,
    destination_switch: &mut StaticP4Switch,
    router: &mut StaticP4Router,
    source: Peer,
    destination: Peer,
    ingress_router: u16,
    egress_router: u16,
) {
    let packet = match source_switch
        .process(source.port, global_packet(source, destination, 12))
        .unwrap()
    {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, ROUTER_SWITCH_PORT);
            assert_packet(&packet, source, destination, 12, 0);
            packet
        }
        other => panic!("source switch did not select router: {other:?}"),
    };

    let packet = match router.process(ingress_router, packet).unwrap() {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, egress_router);
            assert_packet(&packet, source, destination, 11, 0);
            packet
        }
        other => panic!("router failed cross-segment forwarding: {other:?}"),
    };

    match destination_switch
        .process(ROUTER_SWITCH_PORT, packet)
        .unwrap()
    {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, destination.port);
            assert_packet(&packet, source, destination, 11, 0);
        }
        other => panic!("destination switch failed routed delivery: {other:?}"),
    }
}

fn shuffled(mut values: Vec<usize>, mut seed: u64) -> Vec<usize> {
    for i in (1..values.len()).rev() {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let j = seed as usize % (i + 1);
        values.swap(i, j);
    }
    values
}

fn bring_up_side(
    switch: &mut Option<StaticP4Switch>,
    peers: &[Peer; PEERS_PER_SIDE],
    present: &[bool; PEERS_PER_SIDE],
    configured: &mut [bool; PEERS_PER_SIDE],
    router_present: bool,
    provider: GdpAddress,
    prefix: u64,
    tx_base: u32,
) {
    let mut new_switch = StaticP4Switch::with_port_count(SIM_SWITCH_PORTS).unwrap();
    for (index, endpoint) in peers.iter().copied().enumerate() {
        if present[index] {
            announce(&mut new_switch, endpoint, tx_base + index as u32 * 8);
        }
    }
    if router_present {
        discover_router(&mut new_switch, provider, tx_base + 0x100);
        for (index, endpoint) in peers.iter().copied().enumerate() {
            if present[index] {
                configure_address(provider, prefix, endpoint, tx_base + 0x200 + index as u32 * 8);
                configured[index] = true;
            }
        }
    }
    *switch = Some(new_switch);
}

#[test]
fn two_switches_eight_peers_each_random_startup_all_pairs_local_and_global() {
    for seed in [1, 7, 0x55aa, 0xdead_beef, 0x0123_4567_89ab_cdef] {
        let left: [Peer; PEERS_PER_SIDE] = std::array::from_fn(|i| peer(LEFT_PREFIX, 0x20, i));
        let right: [Peer; PEERS_PER_SIDE] = std::array::from_fn(|i| peer(RIGHT_PREFIX, 0x80, i));

        let mut left_switch: Option<StaticP4Switch> = None;
        let mut right_switch: Option<StaticP4Switch> = None;
        let mut router = router();
        let mut router_present = false;
        let mut left_present = [false; PEERS_PER_SIDE];
        let mut right_present = [false; PEERS_PER_SIDE];
        let mut left_configured = [false; PEERS_PER_SIDE];
        let mut right_configured = [false; PEERS_PER_SIDE];

        // 0 left switch, 1 right switch, 2 router,
        // 3..10 left peers, 11..18 right peers.
        for actor in shuffled((0..19).collect(), seed) {
            match actor {
                0 => bring_up_side(
                    &mut left_switch,
                    &left,
                    &left_present,
                    &mut left_configured,
                    router_present,
                    LEFT_ROUTER,
                    LEFT_PREFIX,
                    0x1000,
                ),
                1 => bring_up_side(
                    &mut right_switch,
                    &right,
                    &right_present,
                    &mut right_configured,
                    router_present,
                    RIGHT_ROUTER,
                    RIGHT_PREFIX,
                    0x2000,
                ),
                2 => {
                    router_present = true;
                    if let Some(switch) = left_switch.as_mut() {
                        discover_router(switch, LEFT_ROUTER, 0x3000);
                        for index in 0..PEERS_PER_SIDE {
                            if left_present[index] {
                                configure_address(
                                    LEFT_ROUTER,
                                    LEFT_PREFIX,
                                    left[index],
                                    0x3100 + index as u32 * 8,
                                );
                                left_configured[index] = true;
                            }
                        }
                    }
                    if let Some(switch) = right_switch.as_mut() {
                        discover_router(switch, RIGHT_ROUTER, 0x3200);
                        for index in 0..PEERS_PER_SIDE {
                            if right_present[index] {
                                configure_address(
                                    RIGHT_ROUTER,
                                    RIGHT_PREFIX,
                                    right[index],
                                    0x3300 + index as u32 * 8,
                                );
                                right_configured[index] = true;
                            }
                        }
                    }
                }
                3..=10 => {
                    let index = actor - 3;
                    left_present[index] = true;
                    if let Some(switch) = left_switch.as_mut() {
                        announce(switch, left[index], 0x4000 + index as u32 * 8);
                        if router_present && switch.default_router_port().is_some() {
                            configure_address(
                                LEFT_ROUTER,
                                LEFT_PREFIX,
                                left[index],
                                0x4100 + index as u32 * 8,
                            );
                            left_configured[index] = true;
                        }
                    }
                }
                11..=18 => {
                    let index = actor - 11;
                    right_present[index] = true;
                    if let Some(switch) = right_switch.as_mut() {
                        announce(switch, right[index], 0x5000 + index as u32 * 8);
                        if router_present && switch.default_router_port().is_some() {
                            configure_address(
                                RIGHT_ROUTER,
                                RIGHT_PREFIX,
                                right[index],
                                0x5100 + index as u32 * 8,
                            );
                            right_configured[index] = true;
                        }
                    }
                }
                _ => unreachable!(),
            }
        }

        assert!(router_present);
        assert_eq!(left_present, [true; PEERS_PER_SIDE]);
        assert_eq!(right_present, [true; PEERS_PER_SIDE]);
        assert_eq!(left_configured, [true; PEERS_PER_SIDE]);
        assert_eq!(right_configured, [true; PEERS_PER_SIDE]);

        let left_switch = left_switch.as_mut().unwrap();
        let right_switch = right_switch.as_mut().unwrap();
        assert_eq!(left_switch.physical_port_count(), SIM_SWITCH_PORTS);
        assert_eq!(right_switch.physical_port_count(), SIM_SWITCH_PORTS);
        assert_eq!(left_switch.default_router_port(), Some(ROUTER_SWITCH_PORT));
        assert_eq!(right_switch.default_router_port(), Some(ROUTER_SWITCH_PORT));

        for endpoint in left {
            assert_eq!(left_switch.node_port(endpoint.address), Some(endpoint.port));
        }
        for endpoint in right {
            assert_eq!(right_switch.node_port(endpoint.address), Some(endpoint.port));
        }

        for source in left {
            for destination in left {
                if source.address != destination.address {
                    send_same_segment(left_switch, source, destination, LEFT_PREFIX);
                }
            }
        }
        for source in right {
            for destination in right {
                if source.address != destination.address {
                    send_same_segment(right_switch, source, destination, RIGHT_PREFIX);
                }
            }
        }

        for source in left {
            for destination in right {
                send_cross_segment(
                    left_switch,
                    right_switch,
                    &mut router,
                    source,
                    destination,
                    0,
                    1,
                );
                send_cross_segment(
                    right_switch,
                    left_switch,
                    &mut router,
                    destination,
                    source,
                    1,
                    0,
                );
            }
        }
    }
}
