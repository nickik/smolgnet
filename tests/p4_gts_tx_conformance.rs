use smolgnet::p4_gts::{encode_handwritten_reference, encode_p4_reference};
use smolgnet::wire::gts::GtsContext;
use smolgnet::*;

fn ctx(size_class: SizeClass) -> GtsContext {
    GtsContext {
        gdp_version: 0,
        size_class,
        source: GdpAddress(0x1111_2222_3333_4444),
        destination: GdpAddress(0x5555_6666_7777_8888),
    }
}

fn assert_same(packet: GtsPacket, profile: Option<StreamProfile>) {
    let class = packet.choose_size_class(profile).unwrap();
    let context = ctx(class);
    let handwritten = encode_handwritten_reference(&packet, context, profile).unwrap();
    let p4 = encode_p4_reference(&packet, context, profile).unwrap();
    assert_eq!(p4, handwritten, "TX mismatch for {packet:?} / {profile:?}");
    assert_eq!(
        GtsPacket::decode(&p4, context, profile).unwrap(),
        packet,
        "P4 TX bytes must round-trip through the selected decoder"
    );
}

#[test]
fn every_gts_packet_shape_has_byte_exact_p4_and_handwritten_tx() {
    let reliable_variable =
        StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let reliable_fixed =
        StreamProfile::reliable_fixed(SizeClass::Ctrl64, Direction::Bidirectional);
    let unreliable_variable = StreamProfile::unreliable_variable(
        SizeClass::Msg128,
        Direction::Bidirectional,
        true,
        false,
    );

    assert_same(
        GtsPacket::Connect {
            initiator_receive_tunnel: 0x1020_3040,
            initiator_reset_id: 0x5060_7080,
            profile: reliable_variable,
            initial_receive_credit: 7,
            css: ServiceSelector::registered(1).unwrap(),
        },
        None,
    );
    assert_same(
        GtsPacket::ConnectAck {
            initiator_receive_tunnel: 0x1020_3040,
            responder_receive_tunnel: 0x1122_3344,
            responder_reset_id: 0x5566_7788,
            status: 0,
            initial_receive_credit: 9,
        },
        None,
    );
    assert_same(
        GtsPacket::StreamOpen {
            tunnel_id: 0x1122_3344,
            stream_id: 2,
            profile: unreliable_variable,
            initial_receive_credit: 0,
        },
        None,
    );
    assert_same(
        GtsPacket::StreamAck {
            tunnel_id: 0x1122_3344,
            stream_id: 2,
            status: 0,
            initial_receive_credit: 0,
        },
        None,
    );
    assert_same(
        GtsPacket::Data {
            tunnel_id: 0x1122_3344,
            stream_id: 4,
            sequence: 0x0102_0304,
            data: b"variable reliable data".to_vec(),
            end: false,
        },
        Some(reliable_variable),
    );
    assert_same(
        GtsPacket::Data {
            tunnel_id: 0x1122_3344,
            stream_id: 6,
            sequence: 0x1112_1314,
            data: vec![0x5a; SizeClass::Ctrl64.bytes() - 14],
            end: false,
        },
        Some(reliable_fixed),
    );
    assert_same(
        GtsPacket::Data {
            tunnel_id: 0x1122_3344,
            stream_id: 6,
            sequence: 0x2122_2324,
            data: b"short final unit".to_vec(),
            end: true,
        },
        Some(reliable_fixed),
    );
    assert_same(
        GtsPacket::Ack {
            tunnel_id: 0x1122_3344,
            stream_id: 4,
            ack_base: 0x0102_0304,
            receive_bitmap: 0xa5a5_5a5a,
            receive_credit: 11,
        },
        Some(reliable_variable),
    );

    for sequenced in [false, true] {
        for variable in [false, true] {
            for unchecked_payload in [false, true] {
                let profile = StreamProfile {
                    unreliable: true,
                    variable,
                    sequenced,
                    unchecked_payload,
                    direction: Direction::Bidirectional,
                    size_class: if variable {
                        SizeClass::Msg128
                    } else {
                        SizeClass::Ctrl64
                    },
                };
                let data_len = if variable {
                    23
                } else {
                    profile.size_class.bytes() - if sequenced { 14 } else { 10 }
                };
                assert_same(
                    GtsPacket::Datagram {
                        tunnel_id: 0xaabb_ccdd,
                        stream_id: 9,
                        sequence: sequenced.then_some(0x9988_7766),
                        data: (0..data_len).map(|i| (i as u8).wrapping_mul(17)).collect(),
                    },
                    Some(profile),
                );
            }
        }
    }

    for ack in [false, true] {
        assert_same(
            GtsPacket::StreamClose {
                tunnel_id: 0x1122_3344,
                stream_id: 4,
                final_sequence: 0x5566_7788,
                ack,
            },
            None,
        );
        assert_same(
            GtsPacket::TunnelClose {
                tunnel_id: 0x1122_3344,
                ack,
            },
            None,
        );
        assert_same(
            GtsPacket::StreamReset {
                tunnel_id: 0x1122_3344,
                stream_id: 4,
                reason: 3,
                ack,
            },
            None,
        );
    }
    assert_same(
        GtsPacket::Reset {
            tunnel_id: 0x1122_3344,
            reset_id: 0x99aa_bbcc,
            reason: 7,
        },
        None,
    );
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

#[test]
fn randomized_data_and_datagram_tx_backends_are_byte_exact() {
    let mut rng = Rng(0x4754_532d_5458_2d31);

    for i in 0..512u32 {
        let unreliable = rng.next() & 1 != 0;
        let variable = rng.next() & 1 != 0;
        let sequenced = unreliable && rng.next() & 1 != 0;
        let unchecked_payload = unreliable && rng.next() & 1 != 0;
        let profile = StreamProfile {
            unreliable,
            variable,
            sequenced,
            unchecked_payload,
            direction: Direction::Bidirectional,
            size_class: if variable {
                SizeClass::Msg128
            } else {
                SizeClass::Ctrl64
            },
        };
        profile.validate().unwrap();

        let overhead = if unreliable {
            if sequenced { 14 } else { 10 }
        } else {
            14
        };
        let data_len = if variable {
            1 + (rng.next() as usize % 47)
        } else {
            profile.size_class.bytes() - overhead
        };
        let data: Vec<u8> = (0..data_len)
            .map(|_| rng.next() as u8)
            .collect();

        let packet = if unreliable {
            GtsPacket::Datagram {
                tunnel_id: rng.next() as u32,
                stream_id: (rng.next() as u8) | 1,
                sequence: sequenced.then(|| rng.next() as u32),
                data,
            }
        } else {
            GtsPacket::Data {
                tunnel_id: rng.next() as u32,
                stream_id: (rng.next() as u8) & 0xfe,
                sequence: i ^ rng.next() as u32,
                data,
                end: false,
            }
        };

        assert_same(packet, Some(profile));
    }
}
