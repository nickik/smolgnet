use smolgnet::*;
use smolgnet::wire::gts::Direction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FaultAction {
    Pass,
    Drop,
    Corrupt,
}

#[derive(Debug, Default, Clone, Copy)]
struct FaultStats {
    dropped_packets: u64,
    corrupted_packets: u64,
    detected_corruptions: u64,
    dropped_flits: u64,
    delivered_flits: u64,
}

#[derive(Debug, Clone, Copy)]
struct SegmentState {
    remaining: usize,
    action: FaultAction,
}

impl Default for SegmentState {
    fn default() -> Self {
        Self {
            remaining: 0,
            action: FaultAction::Pass,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn permille(&mut self) -> u16 {
        (self.next_u64() % 1000) as u16
    }

    fn fill(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(8) {
            let bytes = self.next_u64().to_be_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}

/// Deterministic packet-level fault harness.
///
/// DLP still sees complete packet boundaries. A dropped packet is removed after
/// the transmitting DLP has spent its link credit, and that credit is refunded
/// because the adjacent receive buffer was never consumed. Corruption flips a
/// bit in the final GDP payload flit, which for GTS damages its CRC-32 while
/// leaving the GDP header and DLP packet boundary intact.
///
/// This deliberately tests GTS over a lossy/corrupt data path without claiming
/// to solve the separate DLP resynchronization problem caused by losing header
/// flits themselves.
struct FaultLink {
    rng: XorShift64,
    drop_permille: u16,
    corrupt_permille: u16,
    ab: SegmentState,
    ba: SegmentState,
    stats: FaultStats,
}

impl FaultLink {
    fn new(seed: u64, drop_permille: u16, corrupt_permille: u16) -> Self {
        assert!(drop_permille + corrupt_permille <= 1000);
        Self {
            rng: XorShift64::new(seed),
            drop_permille,
            corrupt_permille,
            ab: SegmentState::default(),
            ba: SegmentState::default(),
            stats: FaultStats::default(),
        }
    }

    fn choose_action(
        rng: &mut XorShift64,
        drop_permille: u16,
        corrupt_permille: u16,
    ) -> FaultAction {
        let v = rng.permille();
        if v < drop_permille {
            FaultAction::Drop
        } else if v < drop_permille + corrupt_permille {
            FaultAction::Corrupt
        } else {
            FaultAction::Pass
        }
    }

    fn packet_flits(first: Flit) -> Result<usize> {
        let size = SizeClass::from_wire(((first.data >> 22) & 0x0f) as u8)?;
        let cfg = GdpWireConfig::default();
        let form = cfg.form_from_bit(((first.data >> 21) & 1) != 0);
        let header = match form {
            AddressForm::Global => 20,
            AddressForm::Local => 8,
        };
        Ok((header + size.bytes() + 3) / 4)
    }

    fn transfer_one(
        state: &mut SegmentState,
        rng: &mut XorShift64,
        drop_permille: u16,
        corrupt_permille: u16,
        stats: &mut FaultStats,
        source: &mut Endpoint,
        destination: &mut Endpoint,
        now: u64,
    ) -> Result<bool> {
        let flit = match source.poll_tx_flit() {
            Ok(Some(f)) => f,
            Ok(None) | Err(Error::NoCredit) => return Ok(false),
            Err(e) => return Err(e),
        };

        if flit.vcid.is_control() {
            destination.receive_flit(flit, now)?;
            source.dlp_mut().grant_control_tx_credit(1);
            stats.delivered_flits += 1;
            return Ok(true);
        }

        if state.remaining == 0 {
            state.remaining = Self::packet_flits(flit)?;
            state.action = Self::choose_action(rng, drop_permille, corrupt_permille);
            match state.action {
                FaultAction::Pass => {}
                FaultAction::Drop => stats.dropped_packets += 1,
                FaultAction::Corrupt => stats.corrupted_packets += 1,
            }
        }

        let is_last = state.remaining == 1;
        match state.action {
            FaultAction::Drop => {
                // The adjacent receive endpoint did not consume this flit, so
                // the same advertised link credit remains usable.
                source.dlp_mut().grant_data_tx_credit(1);
                stats.dropped_flits += 1;
            }
            FaultAction::Pass => {
                destination.receive_flit(flit, now)?;
                stats.delivered_flits += 1;
            }
            FaultAction::Corrupt => {
                let mut delivered = flit;
                if is_last {
                    delivered.data ^= 0x0000_0001;
                }
                match destination.receive_flit(delivered, now) {
                    Ok(_) => {}
                    Err(Error::InvalidCrc) if is_last => {
                        stats.detected_corruptions += 1;
                    }
                    Err(e) => return Err(e),
                }
                stats.delivered_flits += 1;
            }
        }

        state.remaining -= 1;
        if state.remaining == 0 {
            state.action = FaultAction::Pass;
        }
        Ok(true)
    }

    fn pump(
        &mut self,
        a: &mut Endpoint,
        b: &mut Endpoint,
        now: u64,
        max_flits: usize,
    ) -> Result<usize> {
        let mut moved = 0;
        loop {
            if moved >= max_flits {
                return Err(Error::BufferFull);
            }
            let mut progress = false;

            if Self::transfer_one(
                &mut self.ab,
                &mut self.rng,
                self.drop_permille,
                self.corrupt_permille,
                &mut self.stats,
                a,
                b,
                now,
            )? {
                moved += 1;
                progress = true;
            }

            if Self::transfer_one(
                &mut self.ba,
                &mut self.rng,
                self.drop_permille,
                self.corrupt_permille,
                &mut self.stats,
                b,
                a,
                now,
            )? {
                moved += 1;
                progress = true;
            }

            if !progress {
                break;
            }
        }
        Ok(moved)
    }
}

fn connected_pair() -> (Endpoint, Endpoint, TunnelHandle, TunnelHandle) {
    let mut cfg_a = EndpointConfig::new(8_192);
    cfg_a.gts_receive_slots = 32;
    let mut cfg_b = EndpointConfig::new(8_192);
    cfg_b.gts_receive_slots = 32;

    let mut a = Endpoint::new(GdpAddress(0x1111_2222_3333_0001), cfg_a).unwrap();
    let mut b = Endpoint::new(GdpAddress(0x1111_2222_3333_0002), cfg_b).unwrap();
    let css = ServiceSelector::registered(1).unwrap();
    b.listen(css, ListenerConfig { receive_slots: 32 });

    let profile = StreamProfile::reliable_variable(SizeClass::Msg256, Direction::Bidirectional);
    let ah = a.connect(b.address(), css, profile).unwrap();
    let mut clean = DirectLink::new();
    clean.pump(&mut a, &mut b, 0, 100_000).unwrap();
    let bh = b.accept().expect("server accepted tunnel");
    assert_eq!(a.tunnel_state(ah).unwrap(), TunnelState::Established);
    (a, b, ah, bh)
}

#[test]
fn reliable_gts_recovers_from_random_packet_loss_and_corruption() {
    let (mut a, mut b, ah, bh) = connected_pair();
    let mut link = FaultLink::new(0x4d59_5df4_d0f3_3173, 180, 180);
    let mut data_rng = XorShift64::new(0x9e37_79b9_7f4a_7c15);
    let mut now = 10u64;
    let mut retransmissions = 0usize;

    for message_no in 0..48u32 {
        let len = 32 + (data_rng.next_u64() as usize % 160);
        let mut expected = vec![0u8; len];
        data_rng.fill(&mut expected);
        expected[..4].copy_from_slice(&message_no.to_be_bytes());

        // A previous ACK may itself have been lost. Keep driving time until
        // this new message can enter the reliable window.
        loop {
            match a.send(ah, 0, &expected, now) {
                Ok(()) => break,
                Err(Error::WouldBlock) => {
                    now += 500;
                    retransmissions += a.tick(now).unwrap();
                    retransmissions += b.tick(now).unwrap();
                    link.pump(&mut a, &mut b, now, 200_000).unwrap();
                }
                Err(e) => panic!("unexpected send error: {e:?}"),
            }
        }

        let mut delivered = None;
        for _ in 0..64 {
            link.pump(&mut a, &mut b, now, 200_000).unwrap();
            if let Some(message) = b.recv(bh, 0).unwrap() {
                delivered = Some(message);
                // recv() creates a fresh ACK with the newly available GTS
                // receive credit; let that ACK traverse the same faulty path.
                link.pump(&mut a, &mut b, now, 200_000).unwrap();
                break;
            }

            now += 500;
            retransmissions += a.tick(now).unwrap();
            retransmissions += b.tick(now).unwrap();
        }

        assert_eq!(delivered.as_deref(), Some(expected.as_slice()));
        now += 1;
    }

    // Drain/re-ACK any final duplicate caused by a lost ACK and give the
    // sender enough time to prove it can settle outstanding retransmissions.
    for _ in 0..8 {
        link.pump(&mut a, &mut b, now, 200_000).unwrap();
        while b.recv(bh, 0).unwrap().is_some() {}
        now += 500;
        retransmissions += a.tick(now).unwrap();
        retransmissions += b.tick(now).unwrap();
    }

    assert!(link.stats.dropped_packets > 0, "loss injector never fired");
    assert!(link.stats.corrupted_packets > 0, "corruption injector never fired");
    assert!(
        link.stats.detected_corruptions > 0,
        "GTS CRC never rejected a corrupted packet"
    );
    assert!(retransmissions > 0, "reliable GTS never retransmitted");
}

#[test]
fn unreliable_gts_discards_corrupted_datagrams_without_retransmission() {
    let mut cfg_a = EndpointConfig::new(4_096);
    cfg_a.gts_receive_slots = 8;
    let mut cfg_b = EndpointConfig::new(4_096);
    cfg_b.gts_receive_slots = 8;
    let mut a = Endpoint::new(GdpAddress(0x2222_0000_0000_0001), cfg_a).unwrap();
    let mut b = Endpoint::new(GdpAddress(0x2222_0000_0000_0002), cfg_b).unwrap();
    let css = ServiceSelector::registered(1).unwrap();
    b.listen(css, ListenerConfig { receive_slots: 8 });

    let base = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let ah = a.connect(b.address(), css, base).unwrap();
    let mut clean = DirectLink::new();
    clean.pump(&mut a, &mut b, 0, 100_000).unwrap();
    let bh = b.accept().unwrap();

    let datagram = StreamProfile::unreliable_variable(
        SizeClass::Msg128,
        Direction::Bidirectional,
        true,
        false,
    );
    let stream = a.open_stream(ah, datagram).unwrap();
    clean.pump(&mut a, &mut b, 1, 100_000).unwrap();

    // 100% corruption on GTS data packets. The datagram must be rejected by
    // CRC and, because the stream is unreliable, must never be retransmitted.
    let mut fault = FaultLink::new(1, 0, 1000);
    a.send(ah, stream, b"one-shot", 2).unwrap();
    fault.pump(&mut a, &mut b, 2, 100_000).unwrap();
    assert_eq!(b.recv(bh, stream).unwrap(), None);
    assert!(fault.stats.detected_corruptions > 0);

    assert_eq!(a.tick(10_000).unwrap(), 0);
    assert_eq!(b.tick(10_000).unwrap(), 0);
}
