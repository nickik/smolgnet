use smolgnet::p4_gts::parse_lower_for_test;
use smolgnet::wire::gts::GtsContext;
use smolgnet::*;

fn ctx(c: SizeClass) -> GtsContext {
    GtsContext {
        gdp_version: 0,
        size_class: c,
        source: GdpAddress(0x1111_2222_3333_4444),
        destination: GdpAddress(0xaaaa_bbbb_cccc_dddd),
    }
}

fn compare(packet: GtsPacket, profile: Option<StreamProfile>) {
    let class = packet.choose_size_class(profile).unwrap();
    let context = ctx(class);
    let bytes = packet.encode(context, profile).unwrap();

    let fixed = parse_lower_for_test(&bytes).unwrap();
    assert_eq!(fixed.packet_type(), packet.packet_type());

    let p4 = GtsPacket::decode_p4_reference(&bytes, context, profile).unwrap();
    let handwritten = GtsPacket::decode_handwritten_reference(&bytes, context, profile).unwrap();
    let selected = GtsPacket::decode(&bytes, context, profile).unwrap();
    assert_eq!(p4, packet);
    assert_eq!(handwritten, packet);
    assert_eq!(selected, packet);
}

#[test]
fn every_control_form_matches_handwritten_lower_backend() {
    let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let css = ServiceSelector::registered(1).unwrap();

    let packets = vec![
        GtsPacket::Connect {
            initiator_receive_tunnel: 0x1122_3344,
            initiator_reset_id: 0x5566_7788,
            profile,
            initial_receive_credit: 8,
            css,
        },
        GtsPacket::ConnectAck {
            initiator_receive_tunnel: 0x1122_3344,
            responder_receive_tunnel: 0x99aa_bbcc,
            responder_reset_id: 0xddee_ff00,
            status: 3,
            initial_receive_credit: 7,
        },
        GtsPacket::StreamOpen {
            tunnel_id: 0x1234_5678,
            stream_id: 9,
            profile,
            initial_receive_credit: 6,
        },
        GtsPacket::StreamAck {
            tunnel_id: 0x1234_5678,
            stream_id: 9,
            status: 2,
            initial_receive_credit: 5,
        },
        GtsPacket::Ack {
            tunnel_id: 0x1234_5678,
            stream_id: 9,
            ack_base: 0x1020_3040,
            receive_bitmap: 0x55aa_aa55,
            receive_credit: 4,
        },
        GtsPacket::StreamClose {
            tunnel_id: 0x1234_5678,
            stream_id: 9,
            final_sequence: 0x0102_0304,
            ack: false,
        },
        GtsPacket::StreamClose {
            tunnel_id: 0x1234_5678,
            stream_id: 9,
            final_sequence: 0x0102_0304,
            ack: true,
        },
        GtsPacket::TunnelClose {
            tunnel_id: 0x1234_5678,
            ack: false,
        },
        GtsPacket::TunnelClose {
            tunnel_id: 0x1234_5678,
            ack: true,
        },
        GtsPacket::Reset {
            tunnel_id: 0x1234_5678,
            reset_id: 0x90ab_cdef,
            reason: 7,
        },
        GtsPacket::StreamReset {
            tunnel_id: 0x1234_5678,
            stream_id: 9,
            reason: 8,
            ack: false,
        },
        GtsPacket::StreamReset {
            tunnel_id: 0x1234_5678,
            stream_id: 9,
            reason: 8,
            ack: true,
        },
    ];

    for packet in packets {
        compare(packet, None);
    }
}

#[test]
fn reliable_data_profiles_share_one_semantic_layer() {
    let variable = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    compare(
        GtsPacket::Data {
            tunnel_id: 0x1234_5678,
            stream_id: 2,
            sequence: 17,
            data: b"P4 variable GTS".to_vec(),
            end: false,
        },
        Some(variable),
    );
    compare(
        GtsPacket::Data {
            tunnel_id: 0x1234_5678,
            stream_id: 2,
            sequence: 18,
            data: b"final".to_vec(),
            end: true,
        },
        Some(variable),
    );

    let fixed = StreamProfile::reliable_fixed(SizeClass::Ctrl32, Direction::Bidirectional);
    compare(
        GtsPacket::Data {
            tunnel_id: 0x1234_5678,
            stream_id: 2,
            sequence: 19,
            data: vec![0x5a; 18],
            end: false,
        },
        Some(fixed),
    );
    compare(
        GtsPacket::Data {
            tunnel_id: 0x1234_5678,
            stream_id: 2,
            sequence: 20,
            data: b"end".to_vec(),
            end: true,
        },
        Some(fixed),
    );
}

#[test]
fn all_datagram_profile_shapes_match() {
    let profiles = [
        StreamProfile::unreliable_fixed(
            SizeClass::Ctrl32,
            Direction::Bidirectional,
            false,
            false,
        ),
        StreamProfile::unreliable_fixed(
            SizeClass::Ctrl32,
            Direction::Bidirectional,
            true,
            false,
        ),
        StreamProfile::unreliable_variable(
            SizeClass::Ctrl64,
            Direction::Bidirectional,
            false,
            false,
        ),
        StreamProfile::unreliable_variable(
            SizeClass::Ctrl64,
            Direction::Bidirectional,
            true,
            false,
        ),
    ];

    for (i, profile) in profiles.into_iter().enumerate() {
        let metadata = 6 + if profile.sequenced { 4 } else { 0 } + if profile.variable { 2 } else { 0 };
        let payload_len = if profile.variable {
            7
        } else {
            profile.size_class.bytes() - metadata - 4
        };
        compare(
            GtsPacket::Datagram {
                tunnel_id: 0x1234_5678,
                stream_id: 3,
                sequence: profile.sequenced.then_some(0x0102_0304 + i as u32),
                data: vec![0x30 + i as u8; payload_len],
            },
            Some(profile),
        );
    }
}

#[test]
fn unchecked_payload_crc_semantics_are_shared() {
    let profile = StreamProfile::unreliable_variable(
        SizeClass::Ctrl64,
        Direction::Bidirectional,
        true,
        true,
    );
    let packet = GtsPacket::Datagram {
        tunnel_id: 7,
        stream_id: 1,
        sequence: Some(9),
        data: b"abcdef".to_vec(),
    };
    let class = packet.choose_size_class(Some(profile)).unwrap();
    let context = ctx(class);
    let mut bytes = packet.encode(context, Some(profile)).unwrap();

    bytes[12] ^= 0x40;
    assert!(GtsPacket::decode_p4_reference(&bytes, context, Some(profile)).is_ok());
    assert!(GtsPacket::decode_handwritten_reference(&bytes, context, Some(profile)).is_ok());

    bytes[2] ^= 0x01;
    assert_eq!(
        GtsPacket::decode_p4_reference(&bytes, context, Some(profile)).unwrap_err(),
        Error::InvalidCrc
    );
    assert_eq!(
        GtsPacket::decode_handwritten_reference(&bytes, context, Some(profile)).unwrap_err(),
        Error::InvalidCrc
    );
}

#[test]
fn malformed_and_crc_errors_match_reference() {
    let packet = GtsPacket::Ack {
        tunnel_id: 0x1234_5678,
        stream_id: 9,
        ack_base: 0x1020_3040,
        receive_bitmap: 0x55aa_aa55,
        receive_credit: 4,
    };
    let class = packet.choose_size_class(None).unwrap();
    let context = ctx(class);
    let bytes = packet.encode(context, None).unwrap();

    for cut in 0..bytes.len() {
        let short = &bytes[..cut];
        let p4 = GtsPacket::decode_p4_reference(short, context, None);
        let hand = GtsPacket::decode_handwritten_reference(short, context, None);
        assert_eq!(p4.as_ref().err(), hand.as_ref().err(), "cut={cut}");
    }

    let mut high_nibble = bytes.clone();
    high_nibble[0] |= 0x80;
    assert_eq!(
        GtsPacket::decode_p4_reference(&high_nibble, context, None).unwrap_err(),
        Error::Unsupported
    );
    assert_eq!(
        GtsPacket::decode_handwritten_reference(&high_nibble, context, None).unwrap_err(),
        Error::Unsupported
    );

    let mut bad_crc = bytes.clone();
    bad_crc[5] ^= 0x01;
    assert_eq!(
        GtsPacket::decode_p4_reference(&bad_crc, context, None).unwrap_err(),
        Error::InvalidCrc
    );
    assert_eq!(
        GtsPacket::decode_handwritten_reference(&bad_crc, context, None).unwrap_err(),
        Error::InvalidCrc
    );
}

struct Lcg(u64);

impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }

    fn byte(&mut self) -> u8 {
        self.next_u32() as u8
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.byte()).collect()
    }
}

fn assert_backends_equal(bytes: &[u8], context: GtsContext, profile: Option<StreamProfile>) {
    let p4 = GtsPacket::decode_p4_reference(bytes, context, profile);
    let handwritten = GtsPacket::decode_handwritten_reference(bytes, context, profile);
    assert_eq!(p4, handwritten);
}

#[test]
fn deterministic_generated_packets_and_mutations_match() {
    let mut rng = Lcg(0x4754_532d_5034_2d31);

    for i in 0..512u32 {
        let tunnel = rng.next_u32();
        let stream = rng.byte();
        let sequence = rng.next_u32();

        let (packet, profile) = match i % 8 {
            0 => (
                GtsPacket::Ack {
                    tunnel_id: tunnel,
                    stream_id: stream,
                    ack_base: sequence,
                    receive_bitmap: rng.next_u32(),
                    receive_credit: rng.byte(),
                },
                None,
            ),
            1 => (
                GtsPacket::StreamClose {
                    tunnel_id: tunnel,
                    stream_id: stream,
                    final_sequence: sequence,
                    ack: i & 8 != 0,
                },
                None,
            ),
            2 => (
                GtsPacket::Reset {
                    tunnel_id: tunnel,
                    reset_id: rng.next_u32(),
                    reason: rng.byte(),
                },
                None,
            ),
            3 => (
                GtsPacket::StreamReset {
                    tunnel_id: tunnel,
                    stream_id: stream,
                    reason: rng.byte(),
                    ack: i & 8 != 0,
                },
                None,
            ),
            4 => {
                let p = StreamProfile::reliable_variable(
                    SizeClass::Msg256,
                    Direction::Bidirectional,
                );
                let len = (rng.next_u32() as usize) % 180;
                (
                    GtsPacket::Data {
                        tunnel_id: tunnel,
                        stream_id: stream,
                        sequence,
                        data: rng.bytes(len),
                        end: i & 16 != 0,
                    },
                    Some(p),
                )
            }
            5 => {
                let p = StreamProfile::reliable_fixed(
                    SizeClass::Ctrl32,
                    Direction::Bidirectional,
                );
                (
                    GtsPacket::Data {
                        tunnel_id: tunnel,
                        stream_id: stream,
                        sequence,
                        data: rng.bytes(18),
                        end: false,
                    },
                    Some(p),
                )
            }
            6 => {
                let sequenced = i & 16 != 0;
                let p = StreamProfile::unreliable_variable(
                    SizeClass::Msg128,
                    Direction::Bidirectional,
                    sequenced,
                    false,
                );
                let len = (rng.next_u32() as usize) % 80;
                (
                    GtsPacket::Datagram {
                        tunnel_id: tunnel,
                        stream_id: stream,
                        sequence: sequenced.then_some(sequence),
                        data: rng.bytes(len),
                    },
                    Some(p),
                )
            }
            _ => {
                let sequenced = i & 16 != 0;
                let p = StreamProfile::unreliable_fixed(
                    SizeClass::Ctrl64,
                    Direction::Bidirectional,
                    sequenced,
                    false,
                );
                let metadata = 6 + if sequenced { 4 } else { 0 };
                let len = p.size_class.bytes() - metadata - 4;
                (
                    GtsPacket::Datagram {
                        tunnel_id: tunnel,
                        stream_id: stream,
                        sequence: sequenced.then_some(sequence),
                        data: rng.bytes(len),
                    },
                    Some(p),
                )
            }
        };

        let class = packet.choose_size_class(profile).unwrap();
        let context = GtsContext {
            gdp_version: (i & 3) as u8,
            size_class: class,
            source: GdpAddress(((rng.next_u32() as u64) << 32) | rng.next_u32() as u64),
            destination: GdpAddress(((rng.next_u32() as u64) << 32) | rng.next_u32() as u64),
        };
        let bytes = packet.encode(context, profile).unwrap();

        assert_backends_equal(&bytes, context, profile);
        assert_eq!(
            GtsPacket::decode_p4_reference(&bytes, context, profile).unwrap(),
            packet
        );

        // Mutate representative bytes throughout the packet. Some mutations
        // remain valid (especially unchecked payload in other tests), others
        // fail CRC/semantics; either way both lower backends must agree.
        for mutation in 0..4usize {
            let mut changed = bytes.clone();
            let index = (rng.next_u32() as usize) % changed.len();
            changed[index] ^= 1u8 << (rng.byte() & 7);
            assert_backends_equal(&changed, context, profile);
            if mutation == 0 {
                // Also force a reserved high bit in the type octet.
                changed[0] |= 0x80;
                assert_backends_equal(&changed, context, profile);
            }
        }
    }
}
