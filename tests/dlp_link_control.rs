use smolgnet::*;

fn cfg(buffer: u32, vc_mode: VcMode, threshold: u32) -> EndpointConfig {
    let mut cfg = EndpointConfig::new(buffer);
    cfg.vc_mode = vc_mode;
    cfg.credit_update_threshold = threshold;
    cfg
}

fn decode_gctl(frame: &GnetFrame) -> GctlMessage {
    assert_eq!(frame.traffic, LinkTraffic::Control);
    let packet = GdpPacket::decode(&frame.bytes, GdpWireConfig::default(), 0).unwrap();
    assert_eq!(packet.header.packet_type, GdpType::Gctl);
    GctlMessage::decode(&packet.payload).unwrap()
}

fn enter_negotiate(endpoint: &mut DlpManagedEndpoint, peer_generation: u8) {
    endpoint.carrier_up().unwrap();
    let local_generation = endpoint.local_generation();
    endpoint.receive_control(&DlpControlFrame::Hello { kind: DlpHelloKind::Initial, sender_generation: peer_generation, peer_generation: 0 }.encode()).unwrap();
    endpoint.receive_control(&DlpControlFrame::Hello { kind: DlpHelloKind::Acknowledgement, sender_generation: peer_generation, peer_generation: local_generation }.encode()).unwrap();
    assert_eq!(endpoint.control_state(), DlpControlState::Negotiate);
}

#[test]
fn glcp_control_flits_match_32_bit_wire_layout_and_reject_malformed_input() {
    let hello = DlpControlFrame::Hello { kind: DlpHelloKind::Initial, sender_generation: 7, peer_generation: 0 };
    assert_eq!(hello.encode(), [0x11, 0x07, 0x00, 0x00]);
    assert_eq!(DlpControlFrame::decode(&hello.encode()).unwrap(), hello);
    let ack = DlpControlFrame::Hello { kind: DlpHelloKind::Acknowledgement, sender_generation: 9, peer_generation: 7 };
    assert_eq!(ack.encode(), 0x1149_1c00u32.to_be_bytes());
    assert_eq!(DlpControlFrame::decode(&ack.encode()).unwrap(), ack);
    let capabilities = DlpControlFrame::Capabilities { kind: DlpCapabilityKind::Offer, sender_generation: 7, profiles: 0b0011, rates: 0b111 };
    assert_eq!(capabilities.encode(), 0x211c_3e00u32.to_be_bytes());
    assert_eq!(DlpControlFrame::decode(&capabilities.encode()).unwrap(), capabilities);
    let reset = DlpControlFrame::Reset { sender_generation: 7, reason: 2 };
    assert_eq!(reset.encode(), 0x311c_8000u32.to_be_bytes());
    assert_eq!(DlpControlFrame::decode(&reset.encode()).unwrap(), reset);
    let params = DlpControlFrame::LinkParameters { kind: DlpCapabilityKind::Offer, sender_generation: 7, rx_window_units: 64, burst_units: 16 };
    assert_eq!(params.encode(), 0x811c_4010u32.to_be_bytes());
    assert_eq!(DlpControlFrame::decode(&params.encode()).unwrap(), params);
    let mut future = hello.encode(); future[0] = 0x12;
    assert_eq!(DlpControlFrame::decode(&future).unwrap_err(), Error::Unsupported);
    let mut noncanonical = hello.encode(); noncanonical[3] = 1;
    assert_eq!(DlpControlFrame::decode(&noncanonical).unwrap_err(), Error::NonCanonical);
    let invalid_ack = DlpControlFrame::Hello { kind: DlpHelloKind::Acknowledgement, sender_generation: 7, peer_generation: 0 }.encode();
    assert_eq!(DlpControlFrame::decode(&invalid_ack).unwrap_err(), Error::InvalidField);
    let zero_param = 0x811c_0010u32.to_be_bytes();
    assert_eq!(DlpControlFrame::decode(&zero_param).unwrap_err(), Error::InvalidField);
}

#[test]
fn link_negotiates_highest_common_vc_and_smallest_burst_then_gctl_credit() {
    let mut a = DlpManagedEndpoint::new(GdpAddress(0x1000_0000_0000_0001), cfg(256, VcMode::Four, 1), 64).unwrap();
    let mut b = DlpManagedEndpoint::new(GdpAddress(0x1000_0000_0000_0002), cfg(128, VcMode::Two, 1), 32).unwrap();
    let mut cable = DlpDirectCable::new(); cable.attach(&mut a, &mut b).unwrap();
    assert_eq!(a.control_state(), DlpControlState::Up); assert_eq!(b.control_state(), DlpControlState::Up);
    assert_eq!(a.negotiated_profile().unwrap().vc_mode, VcMode::Two); assert_eq!(b.negotiated_profile().unwrap().vc_mode, VcMode::Two);
    assert_eq!(a.negotiated_profile().unwrap().burst_flits, 32); assert_eq!(b.negotiated_profile().unwrap().burst_flits, 32);
    assert_eq!(a.dlp().vc_mode(), VcMode::Two); assert_eq!(b.dlp().vc_mode(), VcMode::Two);
    assert_eq!(a.dlp().data_tx_credit(), 128); assert_eq!(b.dlp().data_tx_credit(), 256);
    assert_eq!(a.dlp().data_credit_outstanding(), 256); assert_eq!(b.dlp().data_credit_outstanding(), 128);
    assert!(!a.control_pending()); assert!(!b.control_pending());
}

#[test]
fn steady_state_credit_return_is_gctl_on_data_path_not_control_pair() {
    let mut client = DlpManagedEndpoint::new(GdpAddress(0x1100_0000_0000_0001), cfg(256, VcMode::Two, 1), 64).unwrap();
    let mut server = DlpManagedEndpoint::new(GdpAddress(0x1100_0000_0000_0002), cfg(256, VcMode::Two, 1), 64).unwrap();
    let mut cable = DlpDirectCable::new(); cable.attach(&mut client, &mut server).unwrap();
    let service = ServiceSelector::registered(1).unwrap(); server.listen(service, ListenerConfig::default());
    let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional); client.connect(server.address(), service, profile).unwrap();
    let data = client.poll_tx_frame().unwrap().expect("GTS CONNECT frame"); assert_eq!(data.traffic, LinkTraffic::Data); server.receive_frame(data, 0).unwrap();
    assert!(!server.control_pending()); assert!(server.dlp().queued_control_flits() > 0);
    let credit_frame = server.poll_tx_frame().unwrap().unwrap(); let credit = decode_gctl(&credit_frame);
    assert_eq!(credit.message_type, GctlType::Credit); assert!(credit.parse_credit().unwrap().granted_flits > 0);
}

#[test]
fn two_unconfigured_hosts_negotiate_then_establish_gts_and_exchange_data() {
    let mut client = DlpManagedEndpoint::unconfigured(1, cfg(512, VcMode::Four, 16), 128).unwrap();
    let mut server = DlpManagedEndpoint::unconfigured(2, cfg(256, VcMode::Four, 16), 64).unwrap();
    let mut cable = DlpDirectCable::new(); cable.attach(&mut client, &mut server).unwrap();
    assert_eq!(client.negotiated_profile().unwrap().vc_mode, VcMode::Four); assert_eq!(client.negotiated_profile().unwrap().burst_flits, 64);
    let service = ServiceSelector::registered(2).unwrap(); server.listen(service, ListenerConfig::default());
    let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional); let client_tunnel = client.connect(server.address(), service, profile).unwrap();
    cable.pump(&mut client, &mut server, 0, 100_000).unwrap(); let server_tunnel = server.accept().expect("server accepted GTS tunnel");
    assert_eq!(client.tunnel_state(client_tunnel).unwrap(), TunnelState::Established);
    client.send(client_tunnel, 0, b"native dlp", 1).unwrap(); cable.pump(&mut client, &mut server, 1, 100_000).unwrap();
    assert_eq!(server.recv(server_tunnel, 0).unwrap(), Some(b"native dlp".to_vec()));
    server.send(server_tunnel, 0, b"gts reply", 2).unwrap(); cable.pump(&mut client, &mut server, 2, 100_000).unwrap();
    assert_eq!(client.recv(client_tunnel, 0).unwrap(), Some(b"gts reply".to_vec()));
}

#[test]
fn lost_gctl_credit_request_is_retried_and_same_gts_stream_continues() {
    let mut client = DlpManagedEndpoint::new(GdpAddress(0x3000_0000_0000_0001), cfg(64, VcMode::Two, 64), 32).unwrap();
    let mut server = DlpManagedEndpoint::new(GdpAddress(0x3000_0000_0000_0002), cfg(64, VcMode::Two, 64), 32).unwrap();
    let mut cable = DlpDirectCable::new(); cable.attach(&mut client, &mut server).unwrap();
    let service = ServiceSelector::registered(1).unwrap(); server.listen(service, ListenerConfig::default());
    let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional); let ch = client.connect(server.address(), service, profile).unwrap();
    cable.pump(&mut client, &mut server, 0, 100_000).unwrap(); let sh = server.accept().unwrap();

    // Drain until the remaining credit is smaller than one data frame. Credit
    // need not reach exactly zero: after GTS setup this link has 51 flits and
    // these packets consume 13 flits each, leaving an unusable 12-flit tail.
    let mut frame_flits = None;
    for drain_attempt in 0..8 {
        if frame_flits.is_some_and(|needed| client.dlp().data_tx_credit() < needed) { break; }
        let before = client.dlp().data_tx_credit();
        client.send(ch, 0, b"drain credit", 1).unwrap();
        let mut sent_data = false;
        for poll_attempt in 0..256 {
            match client.poll_tx_frame() {
                Ok(Some(frame)) if frame.traffic == LinkTraffic::Data => {
                    server.receive_frame(frame, 1).unwrap();
                    sent_data = true;
                    break;
                }
                Ok(Some(frame)) => { server.receive_frame(frame, 1).unwrap(); }
                Ok(None) | Err(Error::NoCredit) => {}
                Err(e) => panic!("unexpected drain error at drain {drain_attempt}, poll {poll_attempt}: {e:?}"),
            }
        }
        let after = client.dlp().data_tx_credit();
        eprintln!("DLP credit drain {drain_attempt}: before={before}, after={after}, queued={}", client.dlp().queued_data_flits());
        assert!(sent_data, "credit drain stalled at attempt {drain_attempt}: credit={after}, queued={}", client.dlp().queued_data_flits());
        assert!(after < before, "credit drain made no progress at attempt {drain_attempt}: before={before}, after={after}");
        let consumed = before - after;
        frame_flits.get_or_insert(consumed);
    }
    let frame_flits = frame_flits.expect("no data frame was emitted while draining credit");
    assert!(client.dlp().data_tx_credit() < frame_flits, "credit was not exhausted below one frame: credit={}, frame_flits={frame_flits}", client.dlp().data_tx_credit());
    assert_eq!(client.dlp().queued_data_flits(), 0, "drain left an unexpected queued data frame");

    client.send(ch, 0, b"first packet", 1).unwrap(); client.send(ch, 0, b"second packet", 1).unwrap();
    let mut dropped_request = None;
    for _ in 0..256 { match client.poll_tx_frame() {
        Ok(Some(frame)) if frame.traffic == LinkTraffic::Control => { let msg = decode_gctl(&frame); if msg.message_type == GctlType::CreditRequest { dropped_request = Some(msg); break; } server.receive_frame(frame, 1).unwrap(); }
        Ok(Some(frame)) => { server.receive_frame(frame, 1).unwrap(); }
        Ok(None) | Err(Error::NoCredit) => {}
        Err(e) => panic!("unexpected transmit error: {e:?}"),
    }}
    let dropped_request = dropped_request.expect("first CREDIT_REQUEST was not emitted"); assert!(dropped_request.parse_credit_request().unwrap().requested_flits > 0); assert!(!client.control_pending());
    let mut retried_request = None;
    for _ in 0..256 { match client.poll_tx_frame() {
        Ok(Some(frame)) if frame.traffic == LinkTraffic::Control => { let msg = decode_gctl(&frame); if msg.message_type == GctlType::CreditRequest { server.receive_frame(frame, 2).unwrap(); retried_request = Some(msg); break; } server.receive_frame(frame, 2).unwrap(); }
        Ok(Some(frame)) => { server.receive_frame(frame, 2).unwrap(); }
        Ok(None) | Err(Error::NoCredit) => {}
        Err(e) => panic!("unexpected retry error: {e:?}"),
    }}
    let retried_request = retried_request.expect("lost CREDIT_REQUEST was not retried"); assert!(retried_request.parse_credit_request().unwrap().requested_flits > 0);
    let grant = server.poll_tx_frame().unwrap().expect("GCTL CREDIT reply"); assert_eq!(decode_gctl(&grant).message_type, GctlType::Credit); client.receive_frame(grant, 2).unwrap();
    cable.pump(&mut client, &mut server, 2, 100_000).unwrap();
    while server.recv(sh, 0).unwrap().is_some() {}
    assert_eq!(client.control_state(), DlpControlState::Up); assert_eq!(server.control_state(), DlpControlState::Up);
}

#[test]
fn duplicate_and_stale_generation_control_flits_cannot_replay_state() {
    let mut endpoint = DlpManagedEndpoint::new(GdpAddress(0x5000_0000_0000_0001), cfg(128, VcMode::Four, 8), 64).unwrap(); enter_negotiate(&mut endpoint, 9);
    let duplicate_hello = DlpControlFrame::Hello { kind: DlpHelloKind::Initial, sender_generation: 9, peer_generation: 0 }; endpoint.receive_control(&duplicate_hello.encode()).unwrap();
    assert_eq!(endpoint.control_state(), DlpControlState::Negotiate); assert_eq!(endpoint.peer_generation(), Some(9));
    endpoint.receive_control(&DlpControlFrame::Capabilities { kind: DlpCapabilityKind::Offer, sender_generation: 8, profiles: 0b0011, rates: 0b111 }.encode()).unwrap();
    assert_eq!(endpoint.control_state(), DlpControlState::Negotiate); assert_eq!(endpoint.peer_generation(), Some(9));
    assert_eq!(endpoint.receive_control(&DlpControlFrame::Capabilities { kind: DlpCapabilityKind::Selection, sender_generation: 9, profiles: 0b0010, rates: 0b100 }.encode()).unwrap_err(), Error::InvalidState);
}

#[test]
fn incompatible_capabilities_and_versions_are_rejected() {
    let mut endpoint = DlpManagedEndpoint::new(GdpAddress(0x5100_0000_0000_0001), cfg(128, VcMode::Two, 8), 64).unwrap(); enter_negotiate(&mut endpoint, 7);
    assert_eq!(endpoint.receive_control(&DlpControlFrame::Capabilities { kind: DlpCapabilityKind::Offer, sender_generation: 7, profiles: 0b1000, rates: 0b001 }.encode()).unwrap_err(), Error::Unsupported);
    let mut version2 = DlpControlFrame::Hello { kind: DlpHelloKind::Initial, sender_generation: 7, peer_generation: 0 }.encode(); version2[0] = 0x12;
    assert_eq!(endpoint.receive_control(&version2).unwrap_err(), Error::Unsupported);
}

#[test]
fn credit_overflow_underflow_and_excess_gctl_grant_are_rejected() {
    let mut dlp = DlpEndpoint::new(DlpConfig::new(8, VcMode::Two).unwrap()).unwrap(); assert_eq!(dlp.note_data_credit_granted(9).unwrap_err(), Error::BufferFull);
    let frame = GnetFrame { vcid: Vcid::VC1, traffic: LinkTraffic::Data, bytes: vec![0,0,0,0] }; assert_eq!(dlp.receive_frame(frame).unwrap_err(), Error::CreditViolation);
    let mut a = DlpManagedEndpoint::new(GdpAddress(0x5200_0000_0000_0001), cfg(64, VcMode::Two, 1), 32).unwrap();
    let mut b = DlpManagedEndpoint::new(GdpAddress(0x5200_0000_0000_0002), cfg(64, VcMode::Two, 1), 32).unwrap();
    let mut cable = DlpDirectCable::new(); cable.attach(&mut a, &mut b).unwrap(); assert_eq!(a.dlp().data_tx_credit(), 64);
    b.send_gctl(a.link_local_address(), GctlMessage::credit(0xdead, 1), SizeClass::Ctrl32).unwrap(); let excess = b.poll_tx_frame().unwrap().unwrap();
    assert_eq!(decode_gctl(&excess).message_type, GctlType::Credit); assert_eq!(a.receive_frame(excess, 0).unwrap_err(), Error::CreditViolation);
}

#[test]
fn reset_during_partial_gdp_discards_stale_reassembly_and_allows_fresh_gts() {
    let mut a = DlpManagedEndpoint::new(GdpAddress(0x6000_0000_0000_0001), cfg(512, VcMode::Four, 1), 64).unwrap();
    let mut b = DlpManagedEndpoint::new(GdpAddress(0x6000_0000_0000_0002), cfg(512, VcMode::Four, 1), 64).unwrap();
    let mut cable = DlpDirectCable::new(); cable.attach(&mut a, &mut b).unwrap();
    let service = ServiceSelector::registered(1).unwrap(); b.listen(service, ListenerConfig::default()); let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let ah = a.connect(b.address(), service, profile).unwrap(); cable.pump(&mut a, &mut b, 0, 100_000).unwrap(); let _bh = b.accept().unwrap();
    a.send(ah, 0, b"this packet will be interrupted", 1).unwrap(); let mut partial = None;
    for _ in 0..128 { match a.poll_tx_flit() {
        Ok(Some(flit)) if !flit.vcid.is_control() => { partial = Some(flit); break; }
        Ok(Some(flit)) => { b.receive_flit(flit, 1).unwrap(); a.dlp_mut().grant_control_tx_credit(1); }
        Ok(None) | Err(Error::NoCredit) => {}
        Err(e) => panic!("unexpected transmit error: {e:?}"),
    }}
    let partial = partial.expect("no GTS data flit available"); assert!(!b.receive_flit(partial, 1).unwrap());
    let old_a_generation = a.local_generation(); let old_b_generation = b.local_generation(); a.request_reset(2).unwrap(); cable.pump(&mut a, &mut b, 2, 100_000).unwrap();
    assert_eq!(a.control_state(), DlpControlState::Up); assert_eq!(b.control_state(), DlpControlState::Up); assert_ne!(a.local_generation(), old_a_generation); assert_ne!(b.local_generation(), old_b_generation);
    assert_eq!(a.dlp().data_tx_credit(), 512); assert_eq!(b.dlp().data_tx_credit(), 512);
    let ah2 = a.connect(b.address(), service, profile).unwrap(); cable.pump(&mut a, &mut b, 3, 100_000).unwrap(); let bh2 = b.accept().unwrap(); assert_eq!(a.tunnel_state(ah2).unwrap(), TunnelState::Established);
    a.send(ah2, 0, b"fresh generation", 4).unwrap(); cable.pump(&mut a, &mut b, 4, 100_000).unwrap(); assert_eq!(b.recv(bh2, 0).unwrap(), Some(b"fresh generation".to_vec()));
}

#[test]
fn asymmetric_reset_reestablishes_fresh_credit_and_link_state() {
    let mut a = DlpManagedEndpoint::new(GdpAddress(0x7000_0000_0000_0001), cfg(128, VcMode::Four, 8), 64).unwrap();
    let mut b = DlpManagedEndpoint::new(GdpAddress(0x7000_0000_0000_0002), cfg(128, VcMode::Four, 8), 64).unwrap();
    let mut cable = DlpDirectCable::new(); cable.attach(&mut a, &mut b).unwrap();
    let a_generation = a.local_generation(); let b_generation = b.local_generation(); a.request_reset(7).unwrap(); cable.pump(&mut a, &mut b, 1, 100_000).unwrap();
    assert_eq!(a.control_state(), DlpControlState::Up); assert_eq!(b.control_state(), DlpControlState::Up); assert_ne!(a.local_generation(), a_generation); assert_ne!(b.local_generation(), b_generation);
    assert_eq!(a.dlp().data_tx_credit(), 128); assert_eq!(b.dlp().data_tx_credit(), 128); assert_eq!(a.dlp().data_credit_outstanding(), 128); assert_eq!(b.dlp().data_credit_outstanding(), 128);
}
