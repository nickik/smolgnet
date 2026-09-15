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

const INTERNAL_PREFIX: u64 = 0xa1b2_c300_0000_0000;
const EXTERNAL_PREFIX: u64 = 0xfedc_ba00_0000_0000;
const INTERNAL_ROUTER: GdpAddress = GdpAddress(INTERNAL_PREFIX | 0x0001);
const EXTERNAL_ROUTER: GdpAddress = GdpAddress(EXTERNAL_PREFIX | 0x0001);
const ROUTER_SWITCH_PORT: u16 = 7;

#[derive(Clone, Copy, Debug)]
struct Endpoint {
    address: GdpAddress,
    port: u16,
    nonce: u64,
    marker: u8,
}

fn internal_endpoint(index: u16) -> Endpoint {
    Endpoint {
        address: GdpAddress(INTERNAL_PREFIX | 0x0100 | index as u64),
        port: index,
        nonce: 0x1111_0000_0000_0000 | index as u64,
        marker: 0x40 + index as u8,
    }
}

fn external_endpoint() -> Endpoint {
    Endpoint {
        address: GdpAddress(EXTERNAL_PREFIX | 0x0201),
        port: 1,
        nonce: 0xeeee_0000_0000_0001,
        marker: 0xe1,
    }
}

fn router() -> StaticP4Router {
    let internal = RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0010))
        .with_connected_network(
            GdpPrefix::new(GdpAddress(INTERNAL_PREFIX), 48).unwrap(),
            INTERNAL_ROUTER,
        );
    let external = RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0020))
        .with_connected_network(
            GdpPrefix::new(GdpAddress(EXTERNAL_PREFIX), 48).unwrap(),
            EXTERNAL_ROUTER,
        );
    StaticP4Router::new(RouterStartupConfig::new(vec![internal, external], vec![])).unwrap()
}

fn gctl_roundtrip(message: GctlMessage) -> GctlMessage {
    let size = message.recommended_size_class().unwrap().bytes();
    GctlMessage::decode(&message.encode_exact(size).unwrap()).unwrap()
}

fn announce_on_switch(switch: &mut StaticP4Switch, endpoint: Endpoint, transaction: u32) {
    let announce = NodeAnnounce {
        node: endpoint.address,
        nonce: endpoint.nonce,
        capabilities: 0,
    };
    let decoded = gctl_roundtrip(GctlMessage::node_announce(transaction, announce))
        .parse_node_announce()
        .unwrap();
    assert_eq!(decoded, announce);
    switch.register_node(decoded.node, endpoint.port).unwrap();
}

fn announce_direct(endpoint: Endpoint, transaction: u32) {
    let announce = NodeAnnounce {
        node: endpoint.address,
        nonce: endpoint.nonce,
        capabilities: 0,
    };
    assert_eq!(
        gctl_roundtrip(GctlMessage::node_announce(transaction, announce))
            .parse_node_announce()
            .unwrap(),
        announce
    );
}

fn configure_address(
    router_address: GdpAddress,
    prefix: u64,
    endpoint: Endpoint,
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

fn discover_router(switch: &mut StaticP4Switch) {
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
        provider: INTERNAL_ROUTER,
        lifetime: 600,
        capabilities: 0,
    };
    assert_eq!(
        gctl_roundtrip(GctlMessage::advertise(0x101, advertise))
            .parse_advertise()
            .unwrap(),
        advertise
    );
    switch.set_default_router_port(ROUTER_SWITCH_PORT).unwrap();

    assert_eq!(
        gctl_roundtrip(GctlMessage::reannounce_solicit(
            0x102,
            DiscoveryScope::Link,
        ))
        .parse_reannounce_solicit()
        .unwrap()
        .scope,
        DiscoveryScope::Link
    );
}

fn packet(source: Endpoint, destination: Endpoint) -> GdpPacket {
    let mut payload = vec![source.marker; 32];
    payload[0..8].copy_from_slice(&source.address.0.to_be_bytes());
    payload[8..16].copy_from_slice(&destination.address.0.to_be_bytes());
    let header = GdpHeader::global(
        GdpType::Gts,
        SizeClass::Ctrl32,
        16,
        source.address,
        destination.address,
    );
    GdpPacket::new(header, payload).unwrap()
}

fn assert_packet(packet: &GdpPacket, source: Endpoint, destination: Endpoint, hop: u8) {
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

#[test]
fn direct_upstream_client_and_switched_internal_clients_exchange_full_64bit_addresses() {
    for seed in [1, 7, 0x55aa, 0x0123_4567_89ab_cdef] {
        let internals = [
            internal_endpoint(0),
            internal_endpoint(1),
            internal_endpoint(2),
        ];
        let external = external_endpoint();

        for endpoint in internals.into_iter().chain([external]) {
            assert!(endpoint.address.0 > u32::MAX as u64);
            assert_ne!(endpoint.address.0 >> 32, 0);
        }
        assert_ne!(internals[0].address.0 >> 32, external.address.0 >> 32);

        let mut switch = StaticP4Switch::new();
        let mut router = router();
        let mut router_present = false;
        let mut internal_present = [false; 3];
        let mut internal_configured = [false; 3];
        let mut external_present = false;
        let mut external_configured = false;

        // 0 = router, 1..=3 = switched internal clients, 4 = client directly upstream of the switch.
        for actor in shuffled((0..=4).collect(), seed) {
            match actor {
                0 => {
                    router_present = true;
                    discover_router(&mut switch);
                    for index in 0..internals.len() {
                        if internal_present[index] {
                            configure_address(
                                INTERNAL_ROUTER,
                                INTERNAL_PREFIX,
                                internals[index],
                                0x200 + index as u32 * 8,
                            );
                            internal_configured[index] = true;
                        }
                    }
                    if external_present {
                        configure_address(EXTERNAL_ROUTER, EXTERNAL_PREFIX, external, 0x300);
                        external_configured = true;
                    }
                }
                1..=3 => {
                    let index = actor - 1;
                    let endpoint = internals[index];
                    announce_on_switch(&mut switch, endpoint, 0x400 + actor as u32);
                    internal_present[index] = true;
                    if router_present {
                        configure_address(
                            INTERNAL_ROUTER,
                            INTERNAL_PREFIX,
                            endpoint,
                            0x500 + actor as u32 * 8,
                        );
                        internal_configured[index] = true;
                    }
                }
                4 => {
                    announce_direct(external, 0x600);
                    external_present = true;
                    if router_present {
                        configure_address(EXTERNAL_ROUTER, EXTERNAL_PREFIX, external, 0x610);
                        external_configured = true;
                    }
                }
                _ => unreachable!(),
            }
        }

        assert!(router_present);
        assert_eq!(internal_present, [true; 3]);
        assert_eq!(internal_configured, [true; 3]);
        assert!(external_present && external_configured);
        assert_eq!(switch.default_router_port(), Some(ROUTER_SWITCH_PORT));

        for internal in internals {
            let outbound = match switch.process(internal.port, packet(internal, external)).unwrap() {
                SwitchDisposition::Forward {
                    egress_port,
                    packet,
                } => {
                    assert_eq!(egress_port, ROUTER_SWITCH_PORT);
                    assert_packet(&packet, internal, external, 16);
                    packet
                }
                other => panic!("internal client did not use discovered router uplink: {other:?}"),
            };

            match router.process(0, outbound).unwrap() {
                RouterDisposition::Forward {
                    egress_port,
                    packet,
                } => {
                    assert_eq!(egress_port, 1);
                    assert_packet(&packet, internal, external, 15);
                }
                other => panic!("router failed internal-to-external forwarding: {other:?}"),
            }

            let inbound = match router.process(1, packet(external, internal)).unwrap() {
                RouterDisposition::Forward {
                    egress_port,
                    packet,
                } => {
                    assert_eq!(egress_port, 0);
                    assert_packet(&packet, external, internal, 15);
                    packet
                }
                other => panic!("router failed external-to-internal forwarding: {other:?}"),
            };

            match switch.process(ROUTER_SWITCH_PORT, inbound).unwrap() {
                SwitchDisposition::Forward {
                    egress_port,
                    packet,
                } => {
                    assert_eq!(egress_port, internal.port);
                    assert_packet(&packet, external, internal, 15);
                }
                other => panic!("switch failed routed delivery to internal client: {other:?}"),
            }
        }
    }
}
