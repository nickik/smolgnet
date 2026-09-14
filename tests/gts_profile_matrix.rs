use smolgnet::wire::gts::GtsContext;
use smolgnet::*;

fn cfg() -> EndpointConfig {
    EndpointConfig::new(512)
}

fn pair() -> (Endpoint, Endpoint, DirectLink) {
    (
        Endpoint::new(GdpAddress(0x1111_0000_0000_0001), cfg()).unwrap(),
        Endpoint::new(GdpAddress(0x1111_0000_0000_0002), cfg()).unwrap(),
        DirectLink::new(),
    )
}

#[derive(Clone, Copy)]
struct MatrixStream {
    id: u8,
    profile: StreamProfile,
    opener_is_a: bool,
}

struct MatrixRng(u64);

impl MatrixRng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn fill(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next_u64() as u8).collect()
    }
}

fn all_stream_profile_candidates() -> Vec<StreamProfile> {
    let mut out = Vec::new();
    for unreliable in [false, true] {
        for variable in [false, true] {
            for sequenced in [false, true] {
                for unchecked_payload in [false, true] {
                    for direction in [
                        Direction::OpenerToPeer,
                        Direction::PeerToOpener,
                        Direction::Bidirectional,
                    ] {
                        out.push(StreamProfile {
                            unreliable,
                            variable,
                            sequenced,
                            unchecked_payload,
                            direction,
                            size_class: if variable {
                                SizeClass::Msg128
                            } else {
                                SizeClass::Ctrl64
                            },
                        });
                    }
                }
            }
        }
    }
    out
}

fn payload_len_for(profile: StreamProfile, rng: &mut MatrixRng) -> usize {
    if !profile.variable {
        if profile.unreliable {
            profile.size_class.bytes() - if profile.sequenced { 14 } else { 10 }
        } else {
            profile.size_class.bytes() - 14
        }
    } else {
        let overhead = if profile.unreliable {
            if profile.sequenced { 16 } else { 12 }
        } else {
            16
        };
        let max = profile.size_class.bytes() - overhead;
        1 + (rng.next_u64() as usize % max)
    }
}

fn assert_payload_crc_contract(profile: StreamProfile) {
    let data = vec![0x5a; if profile.variable {
        19
    } else if profile.unreliable {
        profile.size_class.bytes() - if profile.sequenced { 14 } else { 10 }
    } else {
        profile.size_class.bytes() - 14
    }];

    let packet = if profile.unreliable {
        GtsPacket::Datagram {
            tunnel_id: 0x1020_3040,
            stream_id: 7,
            sequence: profile.sequenced.then_some(0x5566_7788),
            data,
        }
    } else {
        GtsPacket::Data {
            tunnel_id: 0x1020_3040,
            stream_id: 7,
            sequence: 0x5566_7788,
            data,
            end: false,
        }
    };

    let class = packet.choose_size_class(Some(profile)).unwrap();
    let ctx = GtsContext {
        gdp_version: 0,
        size_class: class,
        source: GdpAddress(0x1111_0000_0000_0001),
        destination: GdpAddress(0x1111_0000_0000_0002),
    };
    let mut encoded = packet.encode(ctx, Some(profile)).unwrap();
    let payload_start = if profile.unreliable {
        1 + 4 + 1 + if profile.sequenced { 4 } else { 0 } + if profile.variable { 2 } else { 0 }
    } else {
        1 + 4 + 1 + 4 + if profile.variable { 2 } else { 0 }
    };
    encoded[payload_start] ^= 0x80;

    let decoded = GtsPacket::decode(&encoded, ctx, Some(profile));
    if profile.unreliable && profile.unchecked_payload {
        assert!(
            decoded.is_ok(),
            "unchecked datagram payload must not be covered by CRC: {profile:?}"
        );
    } else {
        assert_eq!(
            decoded.unwrap_err(),
            Error::InvalidCrc,
            "checked payload mutation must fail CRC: {profile:?}"
        );
    }
}

#[test]
fn one_tunnel_exercises_every_legal_stream_profile_and_rejects_every_illegal_one() {
    let candidates = all_stream_profile_candidates();
    let legal: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|p| p.validate().is_ok())
        .collect();
    let illegal: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|p| p.validate().is_err())
        .collect();

    // 2 reliable forms * 3 directions +
    // 2 unreliable forms * 2 sequencing modes * 2 CRC modes * 3 directions.
    assert_eq!(legal.len(), 30);
    assert_eq!(illegal.len(), 18);
    assert!(
        legal
            .iter()
            .filter(|p| !p.unreliable)
            .all(|p| !p.sequenced && !p.unchecked_payload)
    );
    assert!(
        illegal
            .iter()
            .all(|p| !p.unreliable && (p.sequenced || p.unchecked_payload))
    );

    // The profile constraint must have real wire consequences: reliable and
    // checked-unreliable payload corruption is detected, while deliberately
    // unchecked unreliable payload bytes are outside the CRC.
    for profile in legal.iter().copied() {
        assert_eq!(
            StreamProfile::from_wire(profile.to_wire().unwrap()).unwrap(),
            profile
        );
        assert_payload_crc_contract(profile);
    }
    for profile in illegal.iter().copied() {
        assert_eq!(profile.to_wire().unwrap_err(), Error::ProfileViolation);
    }

    let (mut a, mut b, mut link) = pair();
    let css = ServiceSelector::registered(0x21).unwrap();
    b.listen(css, ListenerConfig::default());

    let ah = a.connect(b.address(), css, legal[0]).unwrap();
    link.pump(&mut a, &mut b, 0, 100_000).unwrap();
    let bh = b.accept().unwrap();
    assert_eq!(a.tunnel_state(ah).unwrap(), TunnelState::Established);
    assert_eq!(b.tunnel_state(bh).unwrap(), TunnelState::Established);

    // Invalid profile bit combinations must be rejected at the high-level API,
    // before they can become streams in an established tunnel.
    for profile in illegal.iter().copied() {
        assert_eq!(
            a.open_stream(ah, profile).unwrap_err(),
            Error::ProfileViolation
        );
        assert_eq!(
            b.open_stream(bh, profile).unwrap_err(),
            Error::ProfileViolation
        );
    }

    let mut streams = vec![MatrixStream {
        id: 0,
        profile: legal[0],
        opener_is_a: true,
    }];

    // Put every remaining legal profile into the same tunnel. Alternate which
    // endpoint opens streams so both initiator-even and responder-odd stream-ID
    // spaces, and both meanings of opener/peer directions, are exercised.
    let mut now = 1u64;
    for (i, profile) in legal.iter().copied().enumerate().skip(1) {
        let opener_is_a = i % 2 == 1;
        let id = if opener_is_a {
            a.open_stream(ah, profile).unwrap()
        } else {
            b.open_stream(bh, profile).unwrap()
        };
        link.pump(&mut a, &mut b, now, 100_000).unwrap();
        now += 1;
        assert_eq!(a.stream_state(ah, id).unwrap(), StreamState::Open);
        assert_eq!(b.stream_state(bh, id).unwrap(), StreamState::Open);
        streams.push(MatrixStream {
            id,
            profile,
            opener_is_a,
        });
    }
    assert_eq!(streams.len(), 30);

    // Deterministic pseudo-random traffic over all profile shapes. Every stream
    // is first exercised at least once, then additional random stream/direction
    // choices mix packets from all profiles in the same live tunnel.
    let mut rng = MatrixRng::new(0x4754_532d_4d41_5452);
    let mut schedule: Vec<usize> = (0..streams.len()).collect();
    for _ in 0..180 {
        schedule.push(rng.next_u64() as usize % streams.len());
    }

    for index in schedule {
        let stream = streams[index];
        let opener_may_send = stream.profile.direction.opener_may_send();
        let peer_may_send = stream.profile.direction.peer_may_send();
        let sender_is_opener = match (opener_may_send, peer_may_send) {
            (true, true) => rng.next_u64() & 1 == 0,
            (true, false) => true,
            (false, true) => false,
            (false, false) => unreachable!(),
        };
        let sender_is_a = if sender_is_opener {
            stream.opener_is_a
        } else {
            !stream.opener_is_a
        };
        let payload_len = payload_len_for(stream.profile, &mut rng);
        let payload = rng.fill(payload_len);

        if sender_is_a {
            a.send(ah, stream.id, &payload, now).unwrap();
            link.pump(&mut a, &mut b, now, 100_000).unwrap();
            assert_eq!(b.recv(bh, stream.id).unwrap(), Some(payload));
        } else {
            b.send(bh, stream.id, &payload, now).unwrap();
            link.pump(&mut a, &mut b, now, 100_000).unwrap();
            assert_eq!(a.recv(ah, stream.id).unwrap(), Some(payload));
        }
        now += 1;

        // recv() replenishes reliable receive credit by sending an ACK. Pump it
        // immediately so the stress loop also exercises continuing flow-control
        // state rather than relying only on the initial credit window.
        if !stream.profile.unreliable {
            link.pump(&mut a, &mut b, now, 100_000).unwrap();
            now += 1;
        }
    }
}
