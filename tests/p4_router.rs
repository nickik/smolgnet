use smolgnet::*;

fn packet(packet_type: GdpType, destination: u64, hop: u8, class: SizeClass) -> GdpPacket {
    GdpPacket::new(
        GdpHeader::global(
            packet_type,
            class,
            hop,
            GdpAddress(0x0102_0304_0506_0708),
            GdpAddress(destination),
        ),
        vec![0x5a; class.bytes()],
    )
    .unwrap()
}

fn router() -> P4Router {
    P4Router::new(4, 32).unwrap()
}

#[test]
fn global_lpm_prefers_64_then_48_then_32_then_default() {
    let mut r = router();
    r.install_adjacency(1, 0, GdpAddress(0x100)).unwrap();
    r.install_adjacency(2, 1, GdpAddress(0x200)).unwrap();
    r.install_adjacency(3, 2, GdpAddress(0x300)).unwrap();
    r.install_adjacency(4, 3, GdpAddress(0x400)).unwrap();

    r.install_route(0, 0, 100, RouteTarget::Adjacency(1)).unwrap();
    r.install_route(
        0x1122_3344_0000_0000,
        32,
        100,
        RouteTarget::Adjacency(2),
    )
    .unwrap();
    r.install_route(
        0x1122_3344_5566_0000,
        48,
        100,
        RouteTarget::Adjacency(3),
    )
    .unwrap();
    r.install_route(
        0x1122_3344_5566_7788,
        64,
        100,
        RouteTarget::Adjacency(4),
    )
    .unwrap();

    r.ingest(
        0,
        packet(GdpType::Gts, 0x1122_3344_5566_7788, 8, SizeClass::Ctrl32),
    )
    .unwrap();
    assert!(r.poll_egress(3).unwrap().is_some());

    r.ingest(
        0,
        packet(GdpType::Gts, 0x1122_3344_5566_9999, 8, SizeClass::Ctrl32),
    )
    .unwrap();
    assert!(r.poll_egress(2).unwrap().is_some());

    r.ingest(
        0,
        packet(GdpType::Gts, 0x1122_3344_abcd_0001, 8, SizeClass::Ctrl32),
    )
    .unwrap();
    assert!(r.poll_egress(1).unwrap().is_some());

    r.ingest(
        0,
        packet(GdpType::Gts, 0xdead_beef_0000_0001, 8, SizeClass::Ctrl32),
    )
    .unwrap();
    assert!(r.poll_egress(0).unwrap().is_some());
}

#[test]
fn transit_hop_limit_is_decremented_and_expiry_drops() {
    let mut r = router();
    r.install_adjacency(1, 1, GdpAddress(0x200)).unwrap();
    r.install_route(0, 0, 100, RouteTarget::Adjacency(1)).unwrap();

    r.ingest(
        0,
        packet(GdpType::Gts, 0xaaaa_bbbb_cccc_dddd, 5, SizeClass::Ctrl32),
    )
    .unwrap();
    let out = r.poll_egress(1).unwrap().unwrap();
    assert_eq!(out.header.hop_limit, 4);

    r.ingest(
        0,
        packet(GdpType::Gts, 0xaaaa_bbbb_cccc_dddd, 1, SizeClass::Ctrl32),
    )
    .unwrap();
    assert!(r.poll_egress(1).unwrap().is_none());
    assert!(r.counters().drops >= 1);
}

#[test]
fn link_down_rebuilds_routes_and_uses_backup_candidate() {
    let mut r = router();
    r.install_adjacency(10, 1, GdpAddress(0x1111)).unwrap();
    r.install_adjacency(20, 2, GdpAddress(0x2222)).unwrap();

    let prefix = 0xaabb_ccdd_eeff_0000;
    r.install_route(prefix, 48, 10, RouteTarget::Adjacency(10)).unwrap();
    r.install_route(prefix, 48, 20, RouteTarget::Adjacency(20)).unwrap();

    r.ingest(0, packet(GdpType::Gts, prefix | 7, 8, SizeClass::Ctrl32))
        .unwrap();
    assert!(r.poll_egress(1).unwrap().is_some());

    r.set_port_up(1, false).unwrap();
    r.ingest(0, packet(GdpType::Gts, prefix | 8, 8, SizeClass::Ctrl32))
        .unwrap();
    assert!(r.poll_egress(2).unwrap().is_some());

    r.set_port_up(1, true).unwrap();
    r.ingest(0, packet(GdpType::Gts, prefix | 9, 8, SizeClass::Ctrl32))
        .unwrap();
    assert!(r.poll_egress(1).unwrap().is_some());
}

#[test]
fn cpu_punt_and_ingress_policy_work_without_consuming_hop() {
    let mut r = router();
    r.install_route(
        0x4444_5555_6666_7777,
        64,
        10,
        RouteTarget::Cpu,
    )
    .unwrap();

    r.ingest(
        0,
        packet(GdpType::Gts, 0x4444_5555_6666_7777, 3, SizeClass::Ctrl32),
    )
    .unwrap();
    let punted = r.poll_cpu().unwrap();
    assert_eq!(punted.header.hop_limit, 3);

    r.set_policy(0, GdpType::Gctl, PolicyAction::Punt).unwrap();
    r.ingest(
        0,
        packet(GdpType::Gctl, 0x9999_0000_0000_0001, 7, SizeClass::Ctrl32),
    )
    .unwrap();
    let punted = r.poll_cpu().unwrap();
    assert_eq!(punted.header.packet_type, GdpType::Gctl);
    assert_eq!(punted.header.hop_limit, 7);
}

#[test]
fn policy_drop_precedes_route_lookup() {
    let mut r = router();
    r.install_adjacency(1, 1, GdpAddress(2)).unwrap();
    r.install_route(0, 0, 100, RouteTarget::Adjacency(1)).unwrap();
    r.set_policy(0, GdpType::Gts, PolicyAction::Drop).unwrap();

    r.ingest(0, packet(GdpType::Gts, 0x1234, 8, SizeClass::Ctrl32))
        .unwrap();
    assert!(r.poll_egress(1).unwrap().is_none());
    assert_eq!(r.counters().forwarded_packets, 0);
    assert_eq!(r.counters().drops, 1);
}

#[test]
fn per_port_queues_prioritize_control_then_interactive_then_bulk() {
    let mut r = router();
    r.install_adjacency(1, 1, GdpAddress(2)).unwrap();
    r.install_route(0, 0, 100, RouteTarget::Adjacency(1)).unwrap();

    let bulk = packet(GdpType::Gts, 0x100, 8, SizeClass::Medium512);
    let interactive = packet(GdpType::Gts, 0x101, 8, SizeClass::Ctrl32);
    let control = packet(GdpType::Gctl, 0x102, 8, SizeClass::Ctrl32);

    r.ingest(0, bulk).unwrap();
    r.ingest(0, interactive).unwrap();
    r.ingest(0, control).unwrap();

    assert_eq!(
        r.poll_egress(1).unwrap().unwrap().header.packet_type,
        GdpType::Gctl
    );
    assert_eq!(
        r.poll_egress(1).unwrap().unwrap().header.size_class,
        SizeClass::Ctrl32
    );
    assert_eq!(
        r.poll_egress(1).unwrap().unwrap().header.size_class,
        SizeClass::Medium512
    );
}

#[test]
fn counters_track_forward_punt_drop_and_port_activity() {
    let mut r = router();
    r.install_adjacency(1, 1, GdpAddress(2)).unwrap();
    r.install_route(0, 0, 100, RouteTarget::Adjacency(1)).unwrap();
    r.set_policy(0, GdpType::Gctl, PolicyAction::Punt).unwrap();

    r.ingest(0, packet(GdpType::Gts, 0x1234, 8, SizeClass::Ctrl32))
        .unwrap();
    r.ingest(0, packet(GdpType::Gctl, 0x1234, 8, SizeClass::Ctrl32))
        .unwrap();
    r.poll_egress(1).unwrap();
    r.poll_cpu();

    let c = r.counters();
    assert_eq!(c.rx_packets, 2);
    assert_eq!(c.forwarded_packets, 1);
    assert_eq!(c.cpu_punts, 1);
    assert!(c.route_rebuilds >= 3);

    let ingress = r.port_counters(0).unwrap();
    let egress = r.port_counters(1).unwrap();
    assert_eq!(ingress.rx_packets, 2);
    assert_eq!(egress.tx_packets, 1);
}
