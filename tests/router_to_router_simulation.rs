use std::collections::VecDeque;

use smolgnet::*;

const LEFT_PREFIX: u64 = 0x3101_0000_0000_0000;
const RIGHT_PREFIX: u64 = 0x4202_0000_0000_0000;
const LEFT_ROUTER_ADDRESS: GdpAddress = GdpAddress(LEFT_PREFIX | 0x0001);
const RIGHT_ROUTER_ADDRESS: GdpAddress = GdpAddress(RIGHT_PREFIX | 0x0001);
const SWITCH_ROUTER_PORT: u16 = 8;
const SWITCH_PORTS: u16 = 9;
const PEERS: usize = 8;
const LINK_CREDIT_FLITS: u32 = 32_768;

#[derive(Clone, Copy, Debug)]
struct Peer {
    address: GdpAddress,
    port: u16,
    marker: u8,
}

#[derive(Clone, Debug)]
struct RoutedFrame {
    flow: usize,
    packet: GdpPacket,
}

fn prefix(address: u64) -> GdpPrefix {
    GdpPrefix::new(GdpAddress(address), 48).unwrap()
}

fn peer(prefix: u64, marker_base: u8, index: usize) -> Peer {
    Peer {
        address: GdpAddress(prefix | 0x0100 | index as u64),
        port: index as u16,
        marker: marker_base.wrapping_add(index as u8),
    }
}

fn left_router() -> StaticP4Router {
    let lan = RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0010))
        .with_connected_network(prefix(LEFT_PREFIX), LEFT_ROUTER_ADDRESS);
    let inter_router = RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0011));
    StaticP4Router::new(RouterStartupConfig::new(
        vec![lan, inter_router],
        vec![StaticRouteConfig::direct(prefix(RIGHT_PREFIX), 1)],
    ))
    .unwrap()
}

fn right_router() -> StaticP4Router {
    let inter_router = RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0020));
    let lan = RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0021))
        .with_connected_network(prefix(RIGHT_PREFIX), RIGHT_ROUTER_ADDRESS);
    StaticP4Router::new(RouterStartupConfig::new(
        vec![inter_router, lan],
        vec![StaticRouteConfig::direct(prefix(LEFT_PREFIX), 0)],
    ))
    .unwrap()
}

fn switch(peers: &[Peer; PEERS]) -> StaticP4Switch {
    let mut switch = StaticP4Switch::with_port_count(SWITCH_PORTS).unwrap();
    for endpoint in peers {
        switch
            .register_node(endpoint.address, endpoint.port)
            .unwrap();
    }
    switch.set_default_router_port(SWITCH_ROUTER_PORT).unwrap();
    switch
}

fn dlp_link() -> DlpLink {
    let mut link = DlpLink::new(DlpConfig::new(LINK_CREDIT_FLITS, VcMode::Four).unwrap()).unwrap();
    link.begin_recovery().unwrap();
    link.complete_recovery().unwrap();
    link
}

fn size_for_flow(flow: usize) -> SizeClass {
    [
        SizeClass::Ctrl32,
        SizeClass::Ctrl64,
        SizeClass::Msg128,
        SizeClass::Msg256,
        SizeClass::Medium512,
        SizeClass::Bulk1K,
        SizeClass::Legacy1500,
        SizeClass::Xmtu2K,
    ][flow]
}

fn packet(source: Peer, destination: Peer, flow: usize, direction: u8) -> GdpPacket {
    let size = size_for_flow(flow);
    let mut payload = vec![source.marker; size.bytes()];
    payload[0] = flow as u8;
    payload[1] = direction;
    payload[2] = size as u8;
    if payload.len() >= 19 {
        payload[3..11].copy_from_slice(&source.address.0.to_be_bytes());
        payload[11..19].copy_from_slice(&destination.address.0.to_be_bytes());
    }
    GdpPacket::new(
        GdpHeader::global(GdpType::Gts, size, 16, source.address, destination.address),
        payload,
    )
    .unwrap()
}

fn route_to_link(
    switch: &mut StaticP4Switch,
    router: &mut StaticP4Router,
    source: Peer,
    packet: GdpPacket,
    router_ingress: u16,
    router_egress: u16,
) -> GdpPacket {
    let packet = match switch.process(source.port, packet).unwrap() {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, SWITCH_ROUTER_PORT);
            assert_eq!(packet.header.hop_limit, 16);
            packet
        }
        other => panic!("source switch did not forward to router: {other:?}"),
    };

    match router.process(router_ingress, packet).unwrap() {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, router_egress);
            assert_eq!(packet.header.hop_limit, 15);
            packet
        }
        other => panic!("first router did not forward onto inter-router link: {other:?}"),
    }
}

fn deliver_from_link(
    router: &mut StaticP4Router,
    switch: &mut StaticP4Switch,
    destination: Peer,
    packet: GdpPacket,
    router_ingress: u16,
    router_egress: u16,
) -> GdpPacket {
    let packet = match router.process(router_ingress, packet).unwrap() {
        RouterDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, router_egress);
            assert_eq!(packet.header.hop_limit, 14);
            packet
        }
        other => panic!("second router did not forward to destination LAN: {other:?}"),
    };

    match switch.process(SWITCH_ROUTER_PORT, packet).unwrap() {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, destination.port);
            assert_eq!(packet.header.hop_limit, 14);
            packet
        }
        other => panic!("destination switch did not deliver to peer: {other:?}"),
    }
}

fn queue_three_flows(pending: &mut VecDeque<RoutedFrame>, link: &mut DlpLink) -> Vec<usize> {
    let mut flows = Vec::new();
    for _ in 0..3 {
        let Some(frame) = pending.pop_front() else {
            break;
        };
        flows.push(frame.flow);
        link.queue_data_packet(&frame.packet).unwrap();
    }
    flows
}

fn transmit_batch(
    sender: &mut DlpLink,
    receiver: &mut DlpLink,
    expected_flows: &[usize],
    expected_vcs: &[Vcid],
) -> Vec<(usize, Vcid, GdpPacket)> {
    assert_eq!(expected_flows.len(), expected_vcs.len());
    let mut delivered = Vec::new();

    for (&flow, &expected_vc) in expected_flows.iter().zip(expected_vcs) {
        let frame = sender
            .poll_tx_frame()
            .unwrap()
            .expect("queued routed frame must be sendable");
        assert_eq!(frame.traffic, LinkTraffic::Data);
        assert_eq!(frame.vcid, expected_vc);
        let flit_len = frame.flit_len();
        let packet = receiver.receive_frame(frame).unwrap();
        assert_eq!(packet.payload[0] as usize, flow);
        assert_eq!(packet.header.size_class, size_for_flow(flow));
        assert_eq!(
            flit_len,
            (packet.header.header_len() + packet.header.size_class.bytes() + 3) / 4
        );
        delivered.push((flow, expected_vc, packet));
    }

    delivered
}

#[test]
fn eight_counterpart_flows_reuse_vc1_to_vc3_one_packet_at_a_time() {
    let left: [Peer; PEERS] = std::array::from_fn(|i| peer(LEFT_PREFIX, 0x20, i));
    let right: [Peer; PEERS] = std::array::from_fn(|i| peer(RIGHT_PREFIX, 0x80, i));

    let mut left_switch = switch(&left);
    let mut right_switch = switch(&right);
    let mut left_router = left_router();
    let mut right_router = right_router();

    let mut left_dlp = dlp_link();
    let mut right_dlp = dlp_link();
    DlpLink::grant_peer_data_credit(&mut right_dlp, &mut left_dlp, LINK_CREDIT_FLITS).unwrap();
    DlpLink::grant_peer_data_credit(&mut left_dlp, &mut right_dlp, LINK_CREDIT_FLITS).unwrap();

    assert_eq!(left_dlp.endpoint().vc_mode(), VcMode::Four);
    assert_eq!(right_dlp.endpoint().vc_mode(), VcMode::Four);

    // All sixteen endpoints become ready at once: eight left->right counterpart
    // flows and eight right->left counterpart flows. The router-link scheduler
    // admits at most three flows per direction at once because VC4 exposes
    // exactly three data VCIDs.
    let mut left_to_right: VecDeque<RoutedFrame> = (0..PEERS)
        .map(|flow| RoutedFrame {
            flow,
            packet: route_to_link(
                &mut left_switch,
                &mut left_router,
                left[flow],
                packet(left[flow], right[flow], flow, 0),
                0,
                1,
            ),
        })
        .collect();
    let mut right_to_left: VecDeque<RoutedFrame> = (0..PEERS)
        .map(|flow| RoutedFrame {
            flow,
            packet: route_to_link(
                &mut right_switch,
                &mut right_router,
                right[flow],
                packet(right[flow], left[flow], flow, 1),
                1,
                0,
            ),
        })
        .collect();

    let mut l2r_vc_history = Vec::new();
    let mut r2l_vc_history = Vec::new();
    let mut l2r_delivered = [false; PEERS];
    let mut r2l_delivered = [false; PEERS];

    while !left_to_right.is_empty() || !right_to_left.is_empty() {
        let l2r_flows = queue_three_flows(&mut left_to_right, &mut left_dlp);
        let r2l_flows = queue_three_flows(&mut right_to_left, &mut right_dlp);

        let l2r_expected = [Vcid::VC1, Vcid::VC2, Vcid::VC3][..l2r_flows.len()].to_vec();
        let r2l_expected = [Vcid::VC1, Vcid::VC2, Vcid::VC3][..r2l_flows.len()].to_vec();

        // Full-duplex link: advance one complete GDP frame in each direction
        // alternately. A VCID is therefore owned for exactly one packet quantum
        // and is available to another flow in the next scheduler round.
        let l2r = transmit_batch(&mut left_dlp, &mut right_dlp, &l2r_flows, &l2r_expected);
        let r2l = transmit_batch(&mut right_dlp, &mut left_dlp, &r2l_flows, &r2l_expected);

        for (flow, vcid, packet) in l2r {
            l2r_vc_history.push((flow, vcid));
            let delivered = deliver_from_link(
                &mut right_router,
                &mut right_switch,
                right[flow],
                packet,
                0,
                1,
            );
            assert_eq!(delivered.payload[0] as usize, flow);
            assert_eq!(delivered.payload[1], 0);
            assert_eq!(delivered.header.source(), left[flow].address);
            assert_eq!(delivered.header.destination(), right[flow].address);
            assert!(!l2r_delivered[flow]);
            l2r_delivered[flow] = true;
        }

        for (flow, vcid, packet) in r2l {
            r2l_vc_history.push((flow, vcid));
            let delivered =
                deliver_from_link(&mut left_router, &mut left_switch, left[flow], packet, 1, 0);
            assert_eq!(delivered.payload[0] as usize, flow);
            assert_eq!(delivered.payload[1], 1);
            assert_eq!(delivered.header.source(), right[flow].address);
            assert_eq!(delivered.header.destination(), left[flow].address);
            assert!(!r2l_delivered[flow]);
            r2l_delivered[flow] = true;
        }
    }

    let expected = vec![
        (0, Vcid::VC1),
        (1, Vcid::VC2),
        (2, Vcid::VC3),
        (3, Vcid::VC1),
        (4, Vcid::VC2),
        (5, Vcid::VC3),
        (6, Vcid::VC1),
        (7, Vcid::VC2),
    ];
    assert_eq!(l2r_vc_history, expected);
    assert_eq!(r2l_vc_history, expected);
    assert_eq!(l2r_delivered, [true; PEERS]);
    assert_eq!(r2l_delivered, [true; PEERS]);
    assert_eq!(left_dlp.endpoint().queued_data_flits(), 0);
    assert_eq!(right_dlp.endpoint().queued_data_flits(), 0);
}
