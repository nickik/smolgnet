use smolgnet::{
    Error, GdpAddress, GdpPrefix, LinkId, RouteAdvertise, RouteMetric, RouteOrigin, RouterHello,
    RouterId, RoutingGctlBody, RoutingGctlMessage, GCTL_ROUTE_ADVERTISE, GCTL_ROUTER_HELLO,
    GCTL_ROUTER_HELLO_ACK,
};

fn hello() -> RouterHello {
    RouterHello {
        router_id: RouterId::new(0x1122_3344_5566_7788).unwrap(),
        link_id: LinkId::new(0x99aa_bbcc_ddee_ff00).unwrap(),
        hold_time_ms: 30_000,
        metric: RouteMetric(100),
    }
}

#[test]
fn router_id_is_nonzero_random_64_bit_identity() {
    assert_eq!(RouterId::new(0), Err(Error::InvalidField));
    let first = RouterId::random().unwrap();
    let second = RouterId::random().unwrap();
    assert_ne!(first.0, 0);
    assert_ne!(second.0, 0);
    assert_ne!(first, second);
}

#[test]
fn route_origins_have_stable_wire_values_and_preferences() {
    assert_eq!(RouteOrigin::Connected as u8, 0);
    assert_eq!(RouteOrigin::Learned as u8, 1);
    assert_eq!(RouteOrigin::Static as u8, 2);
    assert_eq!(RouteOrigin::Escape as u8, 3);
    assert_eq!(RouteOrigin::Connected.administrative_preference(), 0);
    assert_eq!(RouteOrigin::Static.administrative_preference(), 10);
    assert_eq!(RouteOrigin::Learned.administrative_preference(), 100);
    assert_eq!(RouteOrigin::Escape.administrative_preference(), 255);
    assert_eq!(RouteOrigin::from_wire(4), Err(Error::InvalidField));
}

#[test]
fn router_hello_has_exact_spec_wire_image() {
    let encoded = RoutingGctlMessage::router_hello(0x0102_0304, hello()).encode();
    assert_eq!(encoded.len(), 32);
    assert_eq!(encoded[0], 1);
    assert_eq!(encoded[1], GCTL_ROUTER_HELLO);
    assert_eq!(&encoded[2..4], &[0, 0]);
    assert_eq!(&encoded[4..8], &0x0102_0304u32.to_be_bytes());
    assert_eq!(&encoded[8..16], &0x1122_3344_5566_7788u64.to_be_bytes());
    assert_eq!(&encoded[16..24], &0x99aa_bbcc_ddee_ff00u64.to_be_bytes());
    assert_eq!(&encoded[24..28], &30_000u32.to_be_bytes());
    assert_eq!(&encoded[28..32], &100u32.to_be_bytes());
    assert_eq!(RoutingGctlMessage::decode(&encoded).unwrap(), RoutingGctlMessage::router_hello(0x0102_0304, hello()));
}

#[test]
fn router_hello_ack_uses_same_body_with_distinct_type() {
    let encoded = RoutingGctlMessage::router_hello_ack(7, hello()).encode();
    assert_eq!(encoded[1], GCTL_ROUTER_HELLO_ACK);
    let decoded = RoutingGctlMessage::decode(&encoded).unwrap();
    assert!(matches!(decoded.body, RoutingGctlBody::RouterHelloAck(v) if v == hello()));
}

#[test]
fn route_advertisement_has_exact_canonical_wire_image() {
    let route = RouteAdvertise {
        advertiser: RouterId::new(0x0102_0304_0506_0708).unwrap(),
        prefix: GdpPrefix::new(GdpAddress(0x1234_5678_9abc_0000), 48).unwrap(),
        origin: RouteOrigin::Connected,
        metric: RouteMetric(7),
    };
    let encoded = RoutingGctlMessage::route_advertise(9, route).encode();
    assert_eq!(encoded.len(), 32);
    assert_eq!(encoded[1], GCTL_ROUTE_ADVERTISE);
    assert_eq!(&encoded[8..16], &0x0102_0304_0506_0708u64.to_be_bytes());
    assert_eq!(&encoded[16..24], &0x1234_5678_9abc_0000u64.to_be_bytes());
    assert_eq!(encoded[24], 48);
    assert_eq!(encoded[25], RouteOrigin::Connected as u8);
    assert_eq!(&encoded[26..28], &[0, 0]);
    assert_eq!(&encoded[28..32], &7u32.to_be_bytes());
    assert_eq!(RoutingGctlMessage::decode(&encoded).unwrap(), RoutingGctlMessage::route_advertise(9, route));
}

#[test]
fn malformed_routing_control_messages_are_rejected() {
    let mut hello_wire = RoutingGctlMessage::router_hello(1, hello()).encode();
    hello_wire[8..16].fill(0);
    assert_eq!(RoutingGctlMessage::decode(&hello_wire), Err(Error::InvalidField));

    let route = RouteAdvertise {
        advertiser: RouterId::new(1).unwrap(),
        prefix: GdpPrefix::new(GdpAddress(0x1234_0000_0000_0000), 16).unwrap(),
        origin: RouteOrigin::Learned,
        metric: RouteMetric(100),
    };
    let mut route_wire = RoutingGctlMessage::route_advertise(2, route).encode();
    route_wire[23] = 1;
    assert_eq!(RoutingGctlMessage::decode(&route_wire), Err(Error::NonCanonical));

    let mut reserved = RoutingGctlMessage::route_advertise(2, route).encode();
    reserved[26] = 1;
    assert_eq!(RoutingGctlMessage::decode(&reserved), Err(Error::InvalidField));

    assert_eq!(RoutingGctlMessage::decode(&reserved[..31]), Err(Error::InvalidLength));
}
