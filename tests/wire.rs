use smolgnet::*;
use smolgnet::wire::crc::{crc32_gnet, crc8_gnet};
use smolgnet::wire::gdp::GdpAddresses;
use smolgnet::wire::gts::{Direction, GtsContext};

#[test]
fn crc_vectors() {
    assert_eq!(crc8_gnet(b"123456789"), 0xF4);
    assert_eq!(crc32_gnet(b"123456789"), 0xFC89_1918);
}

#[test]
fn size_classes_are_frozen() {
    let expected = [
        0, 3, 32, 64, 128, 192, 256, 384, 512, 768, 1024, 1280, 1500, 2048, 4096,
        8192,
    ];
    for (i, c) in SizeClass::ALL.iter().enumerate() {
        assert_eq!(c.bytes(), expected[i]);
    }
}

#[test]
fn gdp_global_roundtrip_and_crc_reject() {
    let cfg = GdpWireConfig::default();
    let h = GdpHeader::global(
        GdpType::Gctl,
        SizeClass::Ctrl32,
        42,
        GdpAddress(0x1122_3344_5566_7788),
        GdpAddress(0x8877_6655_4433_2211),
    );
    let p = GdpPacket::new(h, vec![0x55; 32]).unwrap();
    let bytes = p.encode(cfg).unwrap();
    assert_eq!(GdpPacket::decode(&bytes, cfg, 0).unwrap(), p);
    let mut bad = bytes.clone();
    bad[5] ^= 1;
    assert_eq!(GdpPacket::decode(&bad, cfg, 0).unwrap_err(), Error::InvalidCrc);
}

#[test]
fn gdp_local_roundtrip_expands_identity() {
    let cfg = GdpWireConfig::default();
    let prefix = 0x1234_5678_9abc_0000;
    let h = GdpHeader::local(
        GdpType::Gctl,
        SizeClass::Ctrl32,
        7,
        prefix,
        0x1234,
        0xabcd,
    )
    .unwrap();
    let p = GdpPacket::new(h, vec![0; 32]).unwrap();
    let bytes = p.encode(cfg).unwrap();
    let q = GdpPacket::decode(&bytes, cfg, prefix).unwrap();
    assert_eq!(q.header.source(), GdpAddress(prefix | 0x1234));
    assert_eq!(q.header.destination(), GdpAddress(prefix | 0xabcd));
    assert!(matches!(q.header.addresses, GdpAddresses::Local { .. }));
}

#[test]
fn css_uses_shortest_canonical_form() {
    let file = ServiceSelector::registered(1).unwrap();
    assert_eq!(file.encode().unwrap(), vec![0, 1]);
    let capi = ServiceSelector::short(*b"CAPI").unwrap();
    assert_eq!(capi.encode().unwrap(), b"\x40CAPI".to_vec());
    assert_eq!(ServiceSelector::short(*b"FILE").unwrap_err(), Error::NonCanonical);
    let mut full = [0u8; 16];
    full[0] = 1;
    full[15] = 9;
    let x = ServiceSelector::full(full).unwrap();
    assert_eq!(x.encode().unwrap().len(), 17);
}

#[test]
fn gctl_credit_profile_roundtrip() {
    let req = GctlMessage::credit_request(7, 123);
    let req = GctlMessage::decode(&req.encode_exact(32).unwrap()).unwrap();
    assert_eq!(req.parse_credit_request().unwrap().requested_flits, 123);

    let grant = GctlMessage::credit(8, 99);
    let grant = GctlMessage::decode(&grant.encode_exact(32).unwrap()).unwrap();
    assert_eq!(grant.parse_credit().unwrap().granted_flits, 99);
}

#[test]
fn discovery_and_address_configuration_roundtrip() {
    let solicit = GctlMessage::solicit(1, ServiceType::ROUTER, DiscoveryScope::Link);
    let solicit = GctlMessage::decode(&solicit.encode_exact(32).unwrap()).unwrap();
    assert_eq!(solicit.parse_solicit().unwrap().service_type, ServiceType::ROUTER);

    let advertise = Advertise {
        service_type: ServiceType::ROUTER,
        preference: 9,
        provider: GdpAddress(0x1000_0000_0000_0001),
        lifetime: 600,
        capabilities: 0x55aa,
    };
    let msg = GctlMessage::advertise(1, advertise);
    let decoded = GctlMessage::decode(&msg.encode_exact(32).unwrap()).unwrap();
    assert_eq!(decoded.parse_advertise().unwrap(), advertise);

    let offer = AddressOffer {
        router: advertise.provider,
        prefix: 0x1234_5678_9abc_0000,
        prefix_len: 48,
        candidate: GdpAddress(0x1234_5678_9abc_0042),
        lifetime: 3600,
    };
    let msg = GctlMessage::address_offer(1, offer).unwrap();
    assert_eq!(msg.recommended_size_class().unwrap(), SizeClass::Ctrl64);
    let decoded = GctlMessage::decode(&msg.encode_exact(64).unwrap()).unwrap();
    assert_eq!(decoded.parse_address_offer().unwrap(), offer);

    let claim = AddressClaim {
        candidate: offer.candidate,
        nonce: 0x1234_5678_9abc_def0,
    };
    let decoded = GctlMessage::decode(
        &GctlMessage::address_claim(1, claim)
            .encode_exact(32)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(decoded.parse_address_claim().unwrap(), claim);

    let ack = AddressAck {
        candidate: offer.candidate,
        lifetime: 3600,
    };
    let decoded = GctlMessage::decode(
        &GctlMessage::address_ack(1, ack).encode_exact(32).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded.parse_address_ack().unwrap(), ack);

    let nak = AddressNak {
        candidate: offer.candidate,
        reason: 3,
        retry_delay: 10,
    };
    let decoded = GctlMessage::decode(
        &GctlMessage::address_nak(1, nak).encode_exact(32).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded.parse_address_nak().unwrap(), nak);
}

#[test]
fn address_offer_rejects_noncanonical_prefix() {
    let bad = AddressOffer {
        router: GdpAddress(1),
        prefix: 0x1234_5678_9abc_0001,
        prefix_len: 48,
        candidate: GdpAddress(0x1234_5678_9abc_0002),
        lifetime: 100,
    };
    assert_eq!(GctlMessage::address_offer(1, bad).unwrap_err(), Error::InvalidField);
}

fn ctx(c: SizeClass) -> GtsContext {
    GtsContext {
        gdp_version: 0,
        size_class: c,
        source: GdpAddress(1),
        destination: GdpAddress(2),
    }
}

fn roundtrip(p: GtsPacket, profile: Option<StreamProfile>) {
    let c = p.choose_size_class(profile).unwrap();
    let b = p.encode(ctx(c), profile).unwrap();
    let q = GtsPacket::decode(&b, ctx(c), profile).unwrap();
    assert_eq!(q, p);
}

#[test]
fn all_control_gts_packets_roundtrip() {
    let prof = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let css = ServiceSelector::registered(1).unwrap();
    roundtrip(
        GtsPacket::Connect {
            initiator_receive_tunnel: 1,
            initiator_reset_id: 2,
            profile: prof,
            initial_receive_credit: 8,
            css,
        },
        None,
    );
    roundtrip(
        GtsPacket::ConnectAck {
            initiator_receive_tunnel: 1,
            responder_receive_tunnel: 3,
            responder_reset_id: 4,
            status: 0,
            initial_receive_credit: 8,
        },
        None,
    );
    roundtrip(
        GtsPacket::StreamOpen {
            tunnel_id: 3,
            stream_id: 2,
            profile: prof,
            initial_receive_credit: 8,
        },
        None,
    );
    roundtrip(
        GtsPacket::StreamAck {
            tunnel_id: 1,
            stream_id: 2,
            status: 0,
            initial_receive_credit: 8,
        },
        None,
    );
    roundtrip(
        GtsPacket::Ack {
            tunnel_id: 1,
            stream_id: 0,
            ack_base: 9,
            receive_bitmap: 0x55aa,
            receive_credit: 7,
        },
        None,
    );
    roundtrip(
        GtsPacket::StreamClose {
            tunnel_id: 1,
            stream_id: 2,
            final_sequence: 9,
            ack: false,
        },
        None,
    );
    roundtrip(
        GtsPacket::StreamClose {
            tunnel_id: 1,
            stream_id: 2,
            final_sequence: 9,
            ack: true,
        },
        None,
    );
    roundtrip(GtsPacket::TunnelClose { tunnel_id: 1, ack: false }, None);
    roundtrip(GtsPacket::TunnelClose { tunnel_id: 1, ack: true }, None);
    roundtrip(
        GtsPacket::Reset {
            tunnel_id: 1,
            reset_id: 99,
            reason: 2,
        },
        None,
    );
    roundtrip(
        GtsPacket::StreamReset {
            tunnel_id: 1,
            stream_id: 2,
            reason: 2,
            ack: false,
        },
        None,
    );
    roundtrip(
        GtsPacket::StreamReset {
            tunnel_id: 1,
            stream_id: 2,
            reason: 2,
            ack: true,
        },
        None,
    );
}

#[test]
fn data_forms_roundtrip_including_short_fixed_data_end() {
    let rv = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    roundtrip(
        GtsPacket::Data {
            tunnel_id: 2,
            stream_id: 0,
            sequence: 5,
            data: b"hello".to_vec(),
            end: false,
        },
        Some(rv),
    );
    roundtrip(
        GtsPacket::Data {
            tunnel_id: 2,
            stream_id: 0,
            sequence: 6,
            data: b"bye".to_vec(),
            end: true,
        },
        Some(rv),
    );

    let rf = StreamProfile::reliable_fixed(SizeClass::Ctrl32, Direction::Bidirectional);
    roundtrip(
        GtsPacket::Data {
            tunnel_id: 2,
            stream_id: 0,
            sequence: 5,
            data: vec![7; 18],
            end: false,
        },
        Some(rf),
    );
    roundtrip(
        GtsPacket::Data {
            tunnel_id: 2,
            stream_id: 0,
            sequence: 6,
            data: b"end".to_vec(),
            end: true,
        },
        Some(rf),
    );
    assert_eq!(
        GtsPacket::Data {
            tunnel_id: 2,
            stream_id: 0,
            sequence: 7,
            data: vec![0; 17],
            end: false,
        }
        .choose_size_class(Some(rf))
        .unwrap_err(),
        Error::InvalidLength
    );

    let uv = StreamProfile::unreliable_variable(
        SizeClass::Ctrl64,
        Direction::Bidirectional,
        true,
        false,
    );
    roundtrip(
        GtsPacket::Datagram {
            tunnel_id: 2,
            stream_id: 1,
            sequence: Some(3),
            data: b"voice".to_vec(),
        },
        Some(uv),
    );
    let uf = StreamProfile::unreliable_fixed(
        SizeClass::Ctrl32,
        Direction::Bidirectional,
        false,
        false,
    );
    roundtrip(
        GtsPacket::Datagram {
            tunnel_id: 2,
            stream_id: 1,
            sequence: None,
            data: vec![1; 22],
        },
        Some(uf),
    );
}

#[test]
fn unchecked_payload_protects_metadata_not_data() {
    let p = StreamProfile::unreliable_variable(
        SizeClass::Ctrl64,
        Direction::Bidirectional,
        true,
        true,
    );
    let pkt = GtsPacket::Datagram {
        tunnel_id: 7,
        stream_id: 1,
        sequence: Some(9),
        data: b"abcdef".to_vec(),
    };
    let c = pkt.choose_size_class(Some(p)).unwrap();
    let mut b = pkt.encode(ctx(c), Some(p)).unwrap();
    b[12] ^= 0x40;
    assert!(GtsPacket::decode(&b, ctx(c), Some(p)).is_ok());
    b[2] ^= 1;
    assert_eq!(GtsPacket::decode(&b, ctx(c), Some(p)).unwrap_err(), Error::InvalidCrc);
}
