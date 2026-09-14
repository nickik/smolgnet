use smolgnet::*;

fn config(buffer: u32) -> EndpointConfig {
    EndpointConfig::new(buffer)
}

fn pair() -> (Endpoint, Endpoint, DirectLink) {
    (
        Endpoint::new(GdpAddress(0x0102_0304_0000_0001), config(512)).unwrap(),
        Endpoint::new(GdpAddress(0x0102_0304_0000_0002), config(512)).unwrap(),
        DirectLink::new(),
    )
}

#[test]
fn direct_link_bootstraps_data_credit_with_real_gctl_messages() {
    let (mut a, mut b, mut link) = pair();
    link.attach(&mut a, &mut b).unwrap();

    assert_eq!(a.dlp().data_tx_credit(), 0);
    assert_eq!(b.dlp().data_tx_credit(), 0);
    assert_eq!(a.dlp().data_credit_outstanding(), 512);
    assert_eq!(b.dlp().data_credit_outstanding(), 512);

    link.pump(&mut a, &mut b, 0, 10_000).unwrap();
    assert_eq!(a.dlp().data_tx_credit(), 512);
    assert_eq!(b.dlp().data_tx_credit(), 512);
}

#[test]
fn gctl_echo_roundtrip_and_link_credit_is_replenished() {
    let (mut a, mut b, mut link) = pair();
    link.pump(&mut a, &mut b, 0, 10_000).unwrap();
    let initial = a.dlp().data_tx_credit();

    a.send_echo(b.address(), 0x1234, b"ping", SizeClass::Ctrl32)
        .unwrap();
    link.pump(&mut a, &mut b, 1, 10_000).unwrap();
    let body = a.take_echo_reply(0x1234).unwrap();
    assert_eq!(&body[..4], b"ping");
    // ECHO uses the reserved control VC, so ordinary data credit is untouched.
    assert_eq!(a.dlp().data_tx_credit(), initial);
}

#[test]
fn connect_and_reliable_variable_message_roundtrip() {
    let (mut client, mut server, mut link) = pair();
    let file = ServiceSelector::registered(1).unwrap();
    server.listen(file, ListenerConfig::default());
    let profile =
        StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let ch = client.connect(server.address(), file, profile).unwrap();

    link.pump(&mut client, &mut server, 0, 100_000).unwrap();
    let sh = server.accept().unwrap();
    assert_eq!(client.tunnel_state(ch).unwrap(), TunnelState::Established);

    client.send(ch, 0, b"hello", 10).unwrap();
    link.pump(&mut client, &mut server, 10, 100_000).unwrap();
    assert_eq!(server.recv(sh, 0).unwrap(), Some(b"hello".to_vec()));
    link.pump(&mut client, &mut server, 11, 100_000).unwrap();

    server.send(sh, 0, b"world", 20).unwrap();
    link.pump(&mut client, &mut server, 20, 100_000).unwrap();
    assert_eq!(client.recv(ch, 0).unwrap(), Some(b"world".to_vec()));
}

#[test]
fn reliable_receive_credit_reopens_when_application_consumes_message() {
    let mut client_cfg = config(512);
    client_cfg.gts_receive_slots = 1;
    let mut server_cfg = config(512);
    server_cfg.gts_receive_slots = 1;
    let mut client = Endpoint::new(GdpAddress(0x2000_0000_0000_0001), client_cfg).unwrap();
    let mut server = Endpoint::new(GdpAddress(0x2000_0000_0000_0002), server_cfg).unwrap();
    let mut link = DirectLink::new();

    let css = ServiceSelector::registered(1).unwrap();
    server.listen(css, ListenerConfig { receive_slots: 1 });
    let profile =
        StreamProfile::reliable_variable(SizeClass::Ctrl64, Direction::Bidirectional);
    let ch = client.connect(server.address(), css, profile).unwrap();
    link.pump(&mut client, &mut server, 0, 100_000).unwrap();
    let sh = server.accept().unwrap();

    client.send(ch, 0, b"one", 1).unwrap();
    link.pump(&mut client, &mut server, 1, 100_000).unwrap();
    assert_eq!(client.stream_peer_credit(ch, 0).unwrap(), Some(0));
    assert_eq!(client.send(ch, 0, b"two", 2).unwrap_err(), Error::WouldBlock);

    assert_eq!(server.recv(sh, 0).unwrap(), Some(b"one".to_vec()));
    link.pump(&mut client, &mut server, 3, 100_000).unwrap();
    assert_eq!(client.stream_peer_credit(ch, 0).unwrap(), Some(1));
    client.send(ch, 0, b"two", 4).unwrap();
}

#[test]
fn reliable_fixed_stream_uses_exact_normal_message_and_short_data_end() {
    let (mut client, mut server, mut link) = pair();
    let css = ServiceSelector::registered(1).unwrap();
    server.listen(css, ListenerConfig::default());
    let profile = StreamProfile::reliable_fixed(SizeClass::Ctrl32, Direction::Bidirectional);
    let ch = client.connect(server.address(), css, profile).unwrap();
    link.pump(&mut client, &mut server, 0, 100_000).unwrap();
    let sh = server.accept().unwrap();

    client.send(ch, 0, &[0x42; 18], 1).unwrap();
    assert_eq!(
        client.send(ch, 0, &[0x42; 17], 2).unwrap_err(),
        Error::InvalidLength
    );
    link.pump(&mut client, &mut server, 2, 100_000).unwrap();
    assert_eq!(server.recv(sh, 0).unwrap(), Some(vec![0x42; 18]));
    link.pump(&mut client, &mut server, 3, 100_000).unwrap();

    client.send_end(ch, 0, b"final", 4).unwrap();
    link.pump(&mut client, &mut server, 4, 100_000).unwrap();
    assert_eq!(server.recv(sh, 0).unwrap(), Some(b"final".to_vec()));
    assert_eq!(
        client.send(ch, 0, &[0; 18], 5).unwrap_err(),
        Error::InvalidState
    );
}

#[test]
fn secondary_stream_reset_discards_stream_but_tunnel_survives() {
    let (mut a, mut b, mut link) = pair();
    let file = ServiceSelector::registered(1).unwrap();
    b.listen(file, ListenerConfig::default());
    let base = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let ah = a.connect(b.address(), file, base).unwrap();
    link.pump(&mut a, &mut b, 0, 100_000).unwrap();
    let bh = b.accept().unwrap();

    let p = StreamProfile::unreliable_variable(
        SizeClass::Ctrl64,
        Direction::Bidirectional,
        true,
        false,
    );
    let sid = a.open_stream(ah, p).unwrap();
    link.pump(&mut a, &mut b, 1, 100_000).unwrap();
    a.send(ah, sid, b"voice", 2).unwrap();
    link.pump(&mut a, &mut b, 2, 100_000).unwrap();
    assert_eq!(b.recv(bh, sid).unwrap(), Some(b"voice".to_vec()));

    a.reset_stream(ah, sid, 2).unwrap();
    link.pump(&mut a, &mut b, 3, 100_000).unwrap();
    assert_eq!(a.stream_state(ah, sid).unwrap(), StreamState::Reset);
    assert_eq!(b.stream_state(bh, sid).unwrap(), StreamState::Reset);
    assert_eq!(a.send(ah, sid, b"dead", 4).unwrap_err(), Error::InvalidState);

    a.send(ah, 0, b"tunnel lives", 5).unwrap();
    link.pump(&mut a, &mut b, 5, 100_000).unwrap();
    assert_eq!(b.recv(bh, 0).unwrap(), Some(b"tunnel lives".to_vec()));
}

#[test]
fn tunnel_reset_resets_all_streams() {
    let (mut a, mut b, mut link) = pair();
    let css = ServiceSelector::registered(1).unwrap();
    b.listen(css, ListenerConfig::default());
    let p = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let ah = a.connect(b.address(), css, p).unwrap();
    link.pump(&mut a, &mut b, 0, 100_000).unwrap();
    let bh = b.accept().unwrap();
    let sid = a.open_stream(ah, p).unwrap();
    link.pump(&mut a, &mut b, 1, 100_000).unwrap();

    a.reset_tunnel(ah, 2).unwrap();
    link.pump(&mut a, &mut b, 2, 100_000).unwrap();
    assert_eq!(a.tunnel_state(ah).unwrap(), TunnelState::Reset);
    assert_eq!(b.tunnel_state(bh).unwrap(), TunnelState::Reset);
    assert_eq!(a.stream_state(ah, 0).unwrap(), StreamState::Reset);
    assert_eq!(a.stream_state(ah, sid).unwrap(), StreamState::Reset);
    assert_eq!(b.stream_state(bh, 0).unwrap(), StreamState::Reset);
    assert_eq!(b.stream_state(bh, sid).unwrap(), StreamState::Reset);
}

#[test]
fn graceful_tunnel_close_requires_retired_streams() {
    let (mut a, mut b, mut link) = pair();
    let css = ServiceSelector::registered(1).unwrap();
    b.listen(css, ListenerConfig::default());
    let p = StreamProfile::reliable_variable(SizeClass::Ctrl64, Direction::Bidirectional);
    let ah = a.connect(b.address(), css, p).unwrap();
    link.pump(&mut a, &mut b, 0, 100_000).unwrap();
    let bh = b.accept().unwrap();

    assert_eq!(a.close_tunnel(ah).unwrap_err(), Error::InvalidState);
    a.close_stream(ah, 0).unwrap();
    link.pump(&mut a, &mut b, 1, 100_000).unwrap();
    assert_eq!(a.stream_state(ah, 0).unwrap(), StreamState::Closed);
    assert_eq!(b.stream_state(bh, 0).unwrap(), StreamState::Closed);

    a.close_tunnel(ah).unwrap();
    link.pump(&mut a, &mut b, 2, 100_000).unwrap();
    assert_eq!(a.tunnel_state(ah).unwrap(), TunnelState::Closed);
    assert_eq!(b.tunnel_state(bh).unwrap(), TunnelState::Closed);
}

#[test]
fn endpoint_uses_global_gdp_by_default() {
    let (mut a, mut b, mut link) = pair();
    link.pump(&mut a, &mut b, 0, 10_000).unwrap();
    a.send_echo(b.address(), 1, b"global", SizeClass::Ctrl32)
        .unwrap();
    link.pump(&mut a, &mut b, 1, 10_000).unwrap();
    assert_eq!(b.last_rx_address_form(), Some(AddressForm::Global));
}

#[test]
fn endpoint_uses_local_gdp_inside_configured_context() {
    let prefix = 0x1234_5678_9abc_0000;
    let mut cfg_a = config(256);
    cfg_a.local_context_prefix = Some(prefix);
    cfg_a.prefer_local_gdp = true;
    let cfg_b = cfg_a;

    let mut a = Endpoint::new(GdpAddress(prefix | 1), cfg_a).unwrap();
    let mut b = Endpoint::new(GdpAddress(prefix | 2), cfg_b).unwrap();
    let mut link = DirectLink::new();
    link.pump(&mut a, &mut b, 0, 10_000).unwrap();

    a.send_echo(b.address(), 2, b"local", SizeClass::Ctrl32)
        .unwrap();
    link.pump(&mut a, &mut b, 1, 10_000).unwrap();
    assert_eq!(b.last_rx_address_form(), Some(AddressForm::Local));
    assert!(a.take_echo_reply(2).is_some());
}

#[test]
fn router_discovery_offer_claim_ack_assigns_address() {
    let mut client = Endpoint::unconfigured(0x1234, config(512)).unwrap();
    let prefix = 0x3333_4444_5555_0000;
    let mut router = Endpoint::new(GdpAddress(prefix | 0xfffe), config(512)).unwrap();
    router.enable_address_authority(
        AddressAuthorityConfig::new(prefix, 48, 3600).unwrap(),
    );
    let mut link = DirectLink::new();

    client.solicit_router(0x99).unwrap();
    link.pump(&mut client, &mut router, 0, 100_000).unwrap();

    let assigned = client.address();
    assert_ne!(assigned, client.link_local_address());
    assert_eq!(assigned.0 & !0xffff, prefix);
    assert_eq!(client.router_address(), Some(router.address()));
    assert!(matches!(
        client.address_state(),
        AddressState::Assigned {
            address,
            router: _,
            lifetime: 3600
        } if address == assigned
    ));
}

#[test]
fn unconfigured_endpoint_can_remain_link_local_without_router() {
    let endpoint = Endpoint::unconfigured(0x55, config(64)).unwrap();
    assert_eq!(endpoint.address(), endpoint.link_local_address());
    assert_eq!(endpoint.address_state(), AddressState::LinkLocalOnly);
    assert_eq!(endpoint.address().0 >> 48, 0xfe80);
}
