use smolgnet::{
    Error, GdpAddress, GdpPrefix, LinkId, NeighborState, RouteAdvertise, RouteMetric,
    RouteOrigin, RouterAdjacency, RouterHello, RouterId, RoutingGctlMessage, Instant,
};

fn rid(value: u64) -> RouterId {
    RouterId::new(value).unwrap()
}

fn lid(value: u64) -> LinkId {
    LinkId::new(value).unwrap()
}

fn adjacency(router: u64, link: u64, metric: u32, hold_ms: u32) -> RouterAdjacency {
    RouterAdjacency::new(rid(router), lid(link), RouteMetric(metric), hold_ms).unwrap()
}

#[test]
fn two_routers_discover_each_other_with_hello_and_ack() {
    let mut a = adjacency(0xa, 0xa1, 10, 1_000);
    let mut b = adjacency(0xb, 0xb1, 20, 2_000);

    assert_eq!(a.state(), NeighborState::Down);
    assert_eq!(b.state(), NeighborState::Down);

    let hello = a.hello(0x1234_5678);
    let ack = b
        .receive(Instant::from_millis(10), hello)
        .unwrap()
        .expect("HELLO must produce ACK");

    assert_eq!(ack.transaction_id, 0x1234_5678);
    assert_eq!(b.state(), NeighborState::Up);
    let learned_a = b.neighbor().unwrap();
    assert_eq!(learned_a.router_id, rid(0xa));
    assert_eq!(learned_a.remote_link_id, lid(0xa1));
    assert_eq!(learned_a.metric, RouteMetric(10));
    assert_eq!(learned_a.hold_time_ms, 1_000);
    assert_eq!(learned_a.last_seen, Instant::from_millis(10));

    assert_eq!(a.receive(Instant::from_millis(20), ack).unwrap(), None);
    assert_eq!(a.state(), NeighborState::Up);
    let learned_b = a.neighbor().unwrap();
    assert_eq!(learned_b.router_id, rid(0xb));
    assert_eq!(learned_b.remote_link_id, lid(0xb1));
    assert_eq!(learned_b.metric, RouteMetric(20));
    assert_eq!(learned_b.hold_time_ms, 2_000);
    assert_eq!(learned_b.last_seen, Instant::from_millis(20));
}

#[test]
fn repeated_hello_refreshes_liveness_and_peer_parameters() {
    let mut local = adjacency(1, 11, 100, 1_000);

    let first = RoutingGctlMessage::router_hello(
        1,
        RouterHello {
            router_id: rid(2),
            link_id: lid(21),
            hold_time_ms: 100,
            metric: RouteMetric(50),
        },
    );
    local.receive(Instant::from_millis(10), first).unwrap();

    let refresh = RoutingGctlMessage::router_hello(
        2,
        RouterHello {
            router_id: rid(2),
            link_id: lid(22),
            hold_time_ms: 200,
            metric: RouteMetric(40),
        },
    );
    local.receive(Instant::from_millis(80), refresh).unwrap();

    let peer = local.neighbor().unwrap();
    assert_eq!(peer.remote_link_id, lid(22));
    assert_eq!(peer.hold_time_ms, 200);
    assert_eq!(peer.metric, RouteMetric(40));
    assert_eq!(peer.last_seen, Instant::from_millis(80));
    assert!(!local.expire(Instant::from_millis(279)));
    assert!(local.expire(Instant::from_millis(280)));
    assert_eq!(local.state(), NeighborState::Down);
}

#[test]
fn expired_neighbor_can_return_to_up() {
    let mut local = adjacency(1, 11, 100, 1_000);
    let peer = RouterHello {
        router_id: rid(2),
        link_id: lid(21),
        hold_time_ms: 50,
        metric: RouteMetric(100),
    };

    local
        .receive(
            Instant::ZERO,
            RoutingGctlMessage::router_hello(1, peer),
        )
        .unwrap();
    assert!(local.expire(Instant::from_millis(50)));
    assert_eq!(local.state(), NeighborState::Down);

    local
        .receive(
            Instant::from_millis(60),
            RoutingGctlMessage::router_hello(2, peer),
        )
        .unwrap();
    assert_eq!(local.state(), NeighborState::Up);
    assert_eq!(local.neighbor().unwrap().last_seen, Instant::from_millis(60));
}

#[test]
fn rejects_zero_hold_time_and_self_identity() {
    assert_eq!(
        RouterAdjacency::new(rid(1), lid(11), RouteMetric(100), 0),
        Err(Error::InvalidField)
    );

    let mut local = adjacency(1, 11, 100, 1_000);
    let zero_hold = RoutingGctlMessage::router_hello(
        1,
        RouterHello {
            router_id: rid(2),
            link_id: lid(21),
            hold_time_ms: 0,
            metric: RouteMetric(100),
        },
    );
    assert_eq!(local.receive(Instant::ZERO, zero_hold), Err(Error::InvalidField));

    let self_hello = RoutingGctlMessage::router_hello(
        2,
        RouterHello {
            router_id: rid(1),
            link_id: lid(12),
            hold_time_ms: 100,
            metric: RouteMetric(100),
        },
    );
    assert_eq!(local.receive(Instant::ZERO, self_hello), Err(Error::InvalidField));
    assert_eq!(local.state(), NeighborState::Down);
}

#[test]
fn stage2_does_not_process_route_advertisements() {
    let mut local = adjacency(1, 11, 100, 1_000);
    let route = RouteAdvertise {
        advertiser: rid(2),
        prefix: GdpPrefix::new(GdpAddress(0x1234_0000_0000_0000), 16).unwrap(),
        origin: RouteOrigin::Connected,
        metric: RouteMetric(100),
    };

    assert_eq!(
        local.receive(
            Instant::ZERO,
            RoutingGctlMessage::route_advertise(7, route),
        ),
        Err(Error::Unsupported)
    );
}
