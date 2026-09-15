use smolgnet::wire::gctl::{Advertise, DiscoveryScope, GctlMessage, GctlType, NodeAnnounce, ServiceType};
use smolgnet::wire::gdp::GdpAddress;

fn wire_roundtrip(msg: GctlMessage) -> GctlMessage {
    let size = msg.recommended_size_class().unwrap().bytes();
    GctlMessage::decode(&msg.encode_exact(size).unwrap()).unwrap()
}

#[test]
fn node_announce_roundtrips_and_fits_control_packet() {
    let announce = NodeAnnounce {
        node: GdpAddress(0x1234_5678_9abc_0042),
        nonce: 0xfeed_face_dead_beef,
        capabilities: 0x0102_0408,
    };
    let msg = GctlMessage::node_announce(0x1122_3344, announce);
    assert_eq!(msg.message_type, GctlType::NodeAnnounce);
    let decoded = wire_roundtrip(msg);
    assert_eq!(decoded.transaction_id, 0x1122_3344);
    assert_eq!(decoded.parse_node_announce().unwrap(), announce);
}

#[test]
fn duplicate_address_detection_distinguishes_replay_from_collision() {
    let original = NodeAnnounce {
        node: GdpAddress(0x1234_5678_9abc_0042),
        nonce: 1,
        capabilities: 0,
    };
    let replay = original;
    let collision = NodeAnnounce { nonce: 2, ..original };
    let other_node = NodeAnnounce {
        node: GdpAddress(0x1234_5678_9abc_0043),
        nonce: 2,
        capabilities: 0,
    };
    assert!(!original.conflicts_with(replay));
    assert!(original.conflicts_with(collision));
    assert!(!original.conflicts_with(other_node));
}

#[test]
fn late_router_can_trigger_reannouncement_without_switch_identity() {
    let node = NodeAnnounce {
        node: GdpAddress(0x1234_5678_9abc_0042),
        nonce: 0x4455_6677_8899_aabb,
        capabilities: 7,
    };

    // Initial attachment: GS3/router-facing control observes NODE_ANNOUNCE.
    assert_eq!(
        wire_roundtrip(GctlMessage::node_announce(1, node))
            .parse_node_announce()
            .unwrap(),
        node
    );

    // A router appears later using the existing service discovery mechanism.
    let solicit = wire_roundtrip(GctlMessage::solicit(
        2,
        ServiceType::ROUTER,
        DiscoveryScope::Link,
    ));
    assert_eq!(solicit.parse_solicit().unwrap().service_type, ServiceType::ROUTER);

    let router = Advertise {
        service_type: ServiceType::ROUTER,
        preference: 1,
        provider: GdpAddress(0x1234_5678_9abc_0001),
        lifetime: 600,
        capabilities: 0,
    };
    assert_eq!(
        wire_roundtrip(GctlMessage::advertise(2, router))
            .parse_advertise()
            .unwrap(),
        router
    );

    // GS3 needs no GDP address: it requests link-local replay of announcements.
    let replay_request = wire_roundtrip(GctlMessage::reannounce_solicit(
        3,
        DiscoveryScope::Link,
    ));
    assert_eq!(replay_request.message_type, GctlType::ReannounceSolicit);
    assert_eq!(
        replay_request.parse_reannounce_solicit().unwrap().scope,
        DiscoveryScope::Link
    );

    // The node replays the same identity/nonce, allowing the new router to learn it.
    assert_eq!(
        wire_roundtrip(GctlMessage::node_announce(4, node))
            .parse_node_announce()
            .unwrap(),
        node
    );
}

#[test]
fn reserved_reannounce_bytes_are_rejected() {
    let mut bytes = GctlMessage::reannounce_solicit(9, DiscoveryScope::Link)
        .encode_exact(32)
        .unwrap();
    bytes[9] = 1;
    let decoded = GctlMessage::decode(&bytes).unwrap();
    assert!(decoded.parse_reannounce_solicit().is_err());
}
