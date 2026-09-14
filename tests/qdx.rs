use smolgnet::*;

fn packet() -> GdpPacket {
    let h = GdpHeader::global(
        GdpType::Gts,
        SizeClass::Ctrl32,
        64,
        GdpAddress(1),
        GdpAddress(2),
    );
    GdpPacket::new(h, vec![0x5a; 32]).unwrap()
}

#[test]
fn qdx_frame_path_preserves_flit_credit_accounting() {
    let mut a = DlpEndpoint::new(DlpConfig::new(128, VcMode::Two).unwrap()).unwrap();
    let mut b = DlpEndpoint::new(DlpConfig::new(128, VcMode::Two).unwrap()).unwrap();
    let p = packet();
    let n = a.queue_data_packet(&p).unwrap();

    b.note_data_credit_granted(n as u32).unwrap();
    a.grant_data_tx_credit(n as u32);

    let frame = a.poll_tx_frame().unwrap().unwrap();
    assert_eq!(frame.flit_len(), n);
    assert_eq!(frame.traffic, LinkTraffic::Data);
    let got = b.receive_frame(frame).unwrap();

    assert_eq!(got, p);
    assert_eq!(a.data_tx_credit(), 0);
    assert_eq!(b.data_credit_outstanding(), 0);
    assert_eq!(b.rx_buffer_in_use(), 0);
}

#[test]
fn qdx_frame_crc_failure_keeps_known_frame_boundary() {
    let cfg = GdpWireConfig::default();
    let mut b = DlpEndpoint::new(DlpConfig::new(256, VcMode::Two).unwrap()).unwrap();
    let p = packet();
    let good = p.encode(cfg).unwrap();
    let flits = (good.len() + 3) / 4;
    b.note_data_credit_granted((flits * 2) as u32).unwrap();

    let mut bad = good.clone();
    // Corrupt the global GDP header while leaving the explicit QDX frame
    // boundary intact. GDP CRC must reject it, but the next frame remains
    // independently decodable.
    bad[4] ^= 0x01;
    let bad_frame = GnetFrame {
        vcid: Vcid::VC1,
        traffic: LinkTraffic::Data,
        bytes: bad,
    };
    assert_eq!(b.receive_frame(bad_frame).unwrap_err(), Error::InvalidCrc);

    let good_frame = GnetFrame {
        vcid: Vcid::VC1,
        traffic: LinkTraffic::Data,
        bytes: good,
    };
    assert_eq!(b.receive_frame(good_frame).unwrap(), p);
}

#[test]
fn virtual_nic_pair_moves_native_gnet_frames_only() {
    let (mut a, mut b) = VirtualNic::pair(2).unwrap();
    let frame = GnetFrame {
        vcid: Vcid::VC2,
        traffic: LinkTraffic::Data,
        bytes: vec![0x47, 0x4e, 0x45, 0x54, 1, 2, 3, 4],
    };

    a.transmit_frame(frame.clone()).unwrap();
    assert_eq!(a.pending_rx_frames(), 0);
    assert_eq!(b.pending_rx_frames(), 1);
    assert_eq!(b.receive_frame().unwrap(), Some(frame));
    assert_eq!(b.receive_frame().unwrap(), None);

    b.set_link_up(false);
    assert_eq!(a.receive_frame().unwrap_err(), Error::LinkDown);
}

#[test]
fn virtual_nic_queue_is_bounded() {
    let (mut a, _b) = VirtualNic::pair(1).unwrap();
    let frame = GnetFrame {
        vcid: Vcid::VC1,
        traffic: LinkTraffic::Data,
        bytes: vec![1, 2, 3, 4],
    };
    a.transmit_frame(frame.clone()).unwrap();
    assert_eq!(a.transmit_frame(frame).unwrap_err(), Error::BufferFull);
}

#[test]
fn virtual_nic_link_runs_full_gts_endpoint_stack() {
    let cfg = EndpointConfig::new(4096);
    let mut client = Endpoint::new(GdpAddress(0x1234_0000_0000_0011), cfg).unwrap();
    let mut server = Endpoint::new(GdpAddress(0x1234_0000_0000_0022), cfg).unwrap();
    let css = ServiceSelector::registered(1).unwrap();
    server.listen(css, ListenerConfig::default());

    let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let client_tunnel = client.connect(server.address(), css, profile).unwrap();
    let mut link = VirtualNicLink::new(16).unwrap();

    link.pump(&mut client, &mut server, 0, 100_000).unwrap();
    let server_tunnel = server.accept().unwrap();

    client
        .send(client_tunnel, 0, b"native-virtual-nic", 1)
        .unwrap();
    link.pump(&mut client, &mut server, 1, 100_000).unwrap();
    assert_eq!(
        server.recv(server_tunnel, 0).unwrap(),
        Some(b"native-virtual-nic".to_vec())
    );
    link.pump(&mut client, &mut server, 2, 100_000).unwrap();
}

#[test]
fn qdx_direct_link_runs_full_gts_endpoint_stack() {
    let cfg = EndpointConfig::new(4096);
    let mut client = Endpoint::new(GdpAddress(0x1234_0000_0000_0001), cfg).unwrap();
    let mut server = Endpoint::new(GdpAddress(0x1234_0000_0000_0002), cfg).unwrap();
    let css = ServiceSelector::registered(1).unwrap();
    server.listen(css, ListenerConfig::default());

    let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let client_tunnel = client.connect(server.address(), css, profile).unwrap();
    let mut link = QdxDirectLink::new();
    link.pump(&mut client, &mut server, 0, 100_000).unwrap();
    let server_tunnel = server.accept().unwrap();

    client.send(client_tunnel, 0, b"qdx-frame-path", 1).unwrap();
    link.pump(&mut client, &mut server, 1, 100_000).unwrap();
    assert_eq!(server.recv(server_tunnel, 0).unwrap(), Some(b"qdx-frame-path".to_vec()));
    link.pump(&mut client, &mut server, 2, 100_000).unwrap();
}
