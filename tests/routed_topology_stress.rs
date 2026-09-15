use smolgnet::{
    AddressForm, Direction, Endpoint, EndpointConfig, Error, GdpAddress, GdpHeader, GdpPacket,
    GdpType, GdpWireConfig, GnetFrame, Instant, LinkTraffic, ListenerConfig, Result,
    ServiceSelector, SizeClass, StreamProfile, Vcid,
};

const A: GdpAddress = GdpAddress(0x1234_5678_9abc_0001);
const B: GdpAddress = GdpAddress(0x1234_5678_9abc_0002);
const R1: GdpAddress = GdpAddress(0xf000_0000_0000_0001);
const R2: GdpAddress = GdpAddress(0xf000_0000_0000_0002);
const LOCAL_PREFIX: u64 = 0x1234_5678_9abc_0000;

fn endpoint(address: GdpAddress) -> Endpoint {
    let mut endpoint = Endpoint::new(address, EndpointConfig::new(8192)).unwrap();
    endpoint.dlp_mut().grant_data_tx_credit(8192);
    endpoint.dlp_mut().grant_control_tx_credit(8192);
    endpoint.dlp_mut().note_data_credit_granted(8192).unwrap();
    endpoint
}

fn forward_two_hops(mut frame: GnetFrame) -> Result<GnetFrame> {
    let mut packet = GdpPacket::decode(&frame.bytes, GdpWireConfig::default(), 0)?;
    if packet.header.hop_limit <= 2 {
        return Err(Error::InvalidState);
    }
    packet.header.hop_limit -= 2;
    frame.bytes = packet.encode(GdpWireConfig::default())?;
    Ok(frame)
}

fn pump_with_optional_drop(
    a: &mut Endpoint,
    b: &mut Endpoint,
    now: u64,
    drop_first_a_to_b: &mut bool,
) -> Result<usize> {
    let mut moved = 0usize;
    for _ in 0..256 {
        let mut progress = false;

        match a.poll_tx_frame() {
            Ok(Some(frame)) => {
                if *drop_first_a_to_b {
                    *drop_first_a_to_b = false;
                    moved += 1;
                    progress = true;
                } else {
                    b.receive_frame(forward_two_hops(frame)?, now)?;
                    moved += 1;
                    progress = true;
                }
            }
            Ok(None) | Err(Error::NoCredit) => {}
            Err(e) => return Err(e),
        }

        match b.poll_tx_frame() {
            Ok(Some(frame)) => {
                a.receive_frame(forward_two_hops(frame)?, now)?;
                moved += 1;
                progress = true;
            }
            Ok(None) | Err(Error::NoCredit) => {}
            Err(e) => return Err(e),
        }

        if !progress {
            return Ok(moved);
        }
    }

    Err(Error::BufferFull)
}

#[test]
fn reliable_gts_retransmits_after_one_routed_frame_is_lost() {
    let mut a = endpoint(A);
    let mut b = endpoint(B);
    a.routes_mut().add_default_route(R1).unwrap();
    b.routes_mut().add_default_route(R2).unwrap();

    let service = ServiceSelector::registered(1).unwrap();
    b.listen(service, ListenerConfig::default());
    let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let ah = a
        .connect_at(B, service, profile, Instant::from_millis(0))
        .unwrap();

    let mut no_drop = false;
    pump_with_optional_drop(&mut a, &mut b, 0, &mut no_drop).unwrap();
    let bh = b.accept().unwrap();

    a.send(ah, 0, b"survives loss", 10).unwrap();
    let mut drop_one = true;
    pump_with_optional_drop(&mut a, &mut b, 10, &mut drop_one).unwrap();
    assert_eq!(b.recv(bh, 0).unwrap(), None);

    // DEFAULT_RTO_MS is 500 ms. Advancing beyond the RTO must queue the
    // reliable packet again; the routed path itself is unaware of GTS state.
    a.tick(600).unwrap();
    pump_with_optional_drop(&mut a, &mut b, 600, &mut no_drop).unwrap();
    assert_eq!(b.recv(bh, 0).unwrap(), Some(b"survives loss".to_vec()));
}

#[test]
fn router_can_translate_local_ingress_to_global_egress_without_identity_change() {
    let local_header = GdpHeader::local(
        GdpType::Reserved(0x0f),
        SizeClass::Tiny3,
        15,
        LOCAL_PREFIX,
        A.0 as u16,
        B.0 as u16,
    )
    .unwrap();
    let packet = GdpPacket::new(local_header, vec![1, 2, 3]).unwrap();
    let local_bytes = packet.encode(GdpWireConfig::default()).unwrap();

    let decoded = GdpPacket::decode(&local_bytes, GdpWireConfig::default(), LOCAL_PREFIX).unwrap();
    assert_eq!(decoded.header.addresses.form(), AddressForm::Local);
    assert_eq!(decoded.header.source(), A);
    assert_eq!(decoded.header.destination(), B);

    // A test router works with canonical addresses and may choose a different
    // wire form on the next point-to-point link. Rebuild only the GDP envelope;
    // endpoint identity and payload remain unchanged.
    let global_header = GdpHeader::global(
        decoded.header.packet_type,
        decoded.header.size_class,
        decoded.header.hop_limit - 1,
        decoded.header.source(),
        decoded.header.destination(),
    );
    let global_packet = GdpPacket::new(global_header, decoded.payload).unwrap();
    let global_bytes = global_packet.encode(GdpWireConfig::default()).unwrap();
    let global = GdpPacket::decode(&global_bytes, GdpWireConfig::default(), 0).unwrap();

    assert_eq!(global.header.addresses.form(), AddressForm::Global);
    assert_eq!(global.header.source(), A);
    assert_eq!(global.header.destination(), B);
    assert_eq!(global.header.hop_limit, 14);
    assert_eq!(global.payload, vec![1, 2, 3]);
}

#[test]
fn routed_frame_shape_stays_native_gnet_not_ethernet_or_ip() {
    let packet = GdpPacket::new(
        GdpHeader::global(GdpType::Reserved(0x0f), SizeClass::Tiny3, 8, A, B),
        vec![1, 2, 3],
    )
    .unwrap();
    let frame = GnetFrame {
        vcid: Vcid::VC1,
        traffic: LinkTraffic::Data,
        bytes: packet.encode(GdpWireConfig::default()).unwrap(),
    };

    let forwarded = forward_two_hops(frame.clone()).unwrap();
    assert_eq!(forwarded.vcid, frame.vcid);
    assert_eq!(forwarded.traffic, LinkTraffic::Data);
    let packet = GdpPacket::decode(&forwarded.bytes, GdpWireConfig::default(), 0).unwrap();
    assert_eq!(packet.header.source(), A);
    assert_eq!(packet.header.destination(), B);
    assert_eq!(packet.header.hop_limit, 6);
}
