use smolgnet::*;

fn random_bytes(len: usize, mut state: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push(state as u8);
    }
    out
}

#[derive(Default)]
struct PacketAssembly {
    flits: Vec<Flit>,
    expected: usize,
}

impl PacketAssembly {
    fn push(&mut self, flit: Flit) -> bool {
        if self.flits.is_empty() {
            let word = flit.data;
            let size = SizeClass::from_wire(((word >> 22) & 0x0f) as u8).unwrap();
            let form = GdpWireConfig::default().form_from_bit(((word >> 21) & 1) != 0);
            let header = match form {
                AddressForm::Global => 20,
                AddressForm::Local => 8,
            };
            self.expected = (header + size.bytes() + 3) / 4;
        }
        self.flits.push(flit);
        self.flits.len() == self.expected
    }

    fn take(&mut self) -> Vec<Flit> {
        self.expected = 0;
        core::mem::take(&mut self.flits)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    Pass,
    Drop,
    Corrupt,
}

struct FaultyLink {
    state: u64,
    drop_per_thousand: u32,
    corrupt_per_thousand: u32,
    a_to_b: PacketAssembly,
    b_to_a: PacketAssembly,
    dropped: usize,
    corrupted: usize,
    delivered: usize,
}

impl FaultyLink {
    fn new(seed: u64, drop_per_thousand: u32, corrupt_per_thousand: u32) -> Self {
        assert!(drop_per_thousand + corrupt_per_thousand < 1000);
        Self {
            state: seed,
            drop_per_thousand,
            corrupt_per_thousand,
            a_to_b: PacketAssembly::default(),
            b_to_a: PacketAssembly::default(),
            dropped: 0,
            corrupted: 0,
            delivered: 0,
        }
    }

    fn next_u32(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state as u32
    }

    fn choose_fault(&mut self, packet: &GdpPacket) -> Fault {
        if packet.header.packet_type != GdpType::Gts || packet.payload.is_empty() {
            return Fault::Pass;
        }
        let ty = packet.payload[0] & 0x0f;
        if !matches!(ty, 0x5 | 0x7 | 0xD) {
            return Fault::Pass;
        }
        let roll = self.next_u32() % 1000;
        if roll < self.drop_per_thousand {
            Fault::Drop
        } else if roll < self.drop_per_thousand + self.corrupt_per_thousand {
            Fault::Corrupt
        } else {
            Fault::Pass
        }
    }

    fn decode_packet(flits: &[Flit]) -> GdpPacket {
        let mut bytes = Vec::with_capacity(flits.len() * 4);
        for f in flits {
            bytes.extend_from_slice(&f.data.to_be_bytes());
        }
        let word = flits[0].data;
        let size = SizeClass::from_wire(((word >> 22) & 0x0f) as u8).unwrap();
        let form = GdpWireConfig::default().form_from_bit(((word >> 21) & 1) != 0);
        let header = match form {
            AddressForm::Global => 20,
            AddressForm::Local => 8,
        };
        bytes.truncate(header + size.bytes());
        GdpPacket::decode(&bytes, GdpWireConfig::default(), 0).unwrap()
    }

    fn corrupt_payload(flits: &mut [Flit]) {
        let word = flits[0].data;
        let form = GdpWireConfig::default().form_from_bit(((word >> 21) & 1) != 0);
        let header_bytes = match form {
            AddressForm::Global => 20,
            AddressForm::Local => 8,
        };
        // DATA and the sequenced/variable DATAGRAM profiles used below have
        // at least 12 bytes of GTS metadata. Flip a byte after that metadata,
        // leaving the GDP header and GTS CRC unchanged so the receiver must
        // reject the completed packet at the GTS integrity check.
        let byte_index = header_bytes + 12;
        let flit_index = byte_index / 4;
        let byte_in_word = byte_index % 4;
        let shift = (3 - byte_in_word) * 8;
        flits[flit_index].data ^= 0x01u32 << shift;
    }

    fn deliver_packet(
        &mut self,
        mut flits: Vec<Flit>,
        receiver: &mut Endpoint,
        now: u64,
    ) -> Result<()> {
        let packet = Self::decode_packet(&flits);
        match self.choose_fault(&packet) {
            Fault::Drop => {
                self.dropped += 1;
                return Ok(());
            }
            Fault::Corrupt => {
                self.corrupted += 1;
                Self::corrupt_payload(&mut flits);
            }
            Fault::Pass => {
                self.delivered += 1;
            }
        }

        for flit in flits {
            match receiver.receive_flit(flit, now) {
                Ok(_) => {}
                Err(Error::InvalidCrc | Error::InvalidField) => {
                    // Expected for an injected corruption. The complete DLP
                    // segment has already been consumed; GTS rejected it.
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn pump_one_direction(
        &mut self,
        sender: &mut Endpoint,
        receiver: &mut Endpoint,
        from_a: bool,
        now: u64,
    ) -> Result<bool> {
        let flit = match sender.poll_tx_flit() {
            Ok(Some(f)) => f,
            Ok(None) | Err(Error::NoCredit) => return Ok(false),
            Err(e) => return Err(e),
        };

        if flit.vcid.is_control() {
            receiver.receive_flit(flit, now)?;
            sender.dlp_mut().grant_control_tx_credit(1);
            return Ok(true);
        }

        let completed = if from_a {
            self.a_to_b.push(flit)
        } else {
            self.b_to_a.push(flit)
        };
        if completed {
            let flits = if from_a {
                self.a_to_b.take()
            } else {
                self.b_to_a.take()
            };
            self.deliver_packet(flits, receiver, now)?;
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
            if self.pump_one_direction(a, b, true, now)? {
                moved += 1;
                progress = true;
            }
            if self.pump_one_direction(b, a, false, now)? {
                moved += 1;
                progress = true;
            }
            if !progress {
                return Ok(moved);
            }
        }
    }
}

fn connected_pair(base_profile: StreamProfile) -> (Endpoint, Endpoint, TunnelHandle, TunnelHandle) {
    let mut cfg = EndpointConfig::new(250_000);
    cfg.gts_receive_slots = 32;
    let mut a = Endpoint::new(GdpAddress(0x1111_0000_0000_0001), cfg).unwrap();
    let mut b = Endpoint::new(GdpAddress(0x1111_0000_0000_0002), cfg).unwrap();
    let css = ServiceSelector::registered(1).unwrap();
    b.listen(css, ListenerConfig { receive_slots: 32 });
    let ah = a.connect(b.address(), css, base_profile).unwrap();
    let mut clean = DirectLink::new();
    clean.pump(&mut a, &mut b, 0, 200_000).unwrap();
    let bh = b.accept().unwrap();
    (a, b, ah, bh)
}

#[test]
fn reliable_gts_recovers_random_packet_loss_and_corruption() {
    let profile = StreamProfile::reliable_variable(SizeClass::Msg256, Direction::Bidirectional);
    let (mut a, mut b, ah, bh) = connected_pair(profile);
    let source = random_bytes(128 * 1024, 0x1234_5678_9abc_def0);
    let mut output = Vec::with_capacity(source.len());
    let mut sent = 0usize;
    let mut now = 0u64;
    let mut retransmissions = 0usize;
    let mut link = FaultyLink::new(0xfeed_beef_0123_4567, 90, 90);

    for _round in 0..400 {
        while sent < source.len() {
            let n = (source.len() - sent).min(200);
            match a.send(ah, 0, &source[sent..sent + n], now) {
                Ok(()) => sent += n,
                Err(Error::WouldBlock) => break,
                Err(e) => panic!("send failed: {e:?}"),
            }
        }

        link.pump(&mut a, &mut b, now, 2_000_000).unwrap();
        while let Some(msg) = b.recv(bh, 0).unwrap() {
            output.extend_from_slice(&msg);
        }
        link.pump(&mut a, &mut b, now, 2_000_000).unwrap();

        if output.len() == source.len() && sent == source.len() {
            break;
        }

        now += 500;
        retransmissions += a.tick(now).unwrap();
        link.pump(&mut a, &mut b, now, 2_000_000).unwrap();
    }

    assert!(link.dropped > 0, "test seed should inject packet loss");
    assert!(link.corrupted > 0, "test seed should inject corruption");
    assert!(retransmissions > 0, "loss/corruption must force GTS retransmission");
    assert_eq!(output, source, "reliable GTS must deliver exact original bytes once");
}

#[test]
fn unreliable_gts_discards_bad_datagrams_without_retransmission() {
    let base = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let (mut a, mut b, ah, bh) = connected_pair(base);
    let datagram = StreamProfile::unreliable_variable(
        SizeClass::Msg128,
        Direction::Bidirectional,
        true,
        false,
    );
    let sid = a.open_stream(ah, datagram).unwrap();
    let mut clean = DirectLink::new();
    // The endpoints are already attached at the DLP level. DirectLink::pump
    // simply carries the STREAM_OPEN/ACK exchange here.
    clean.pump(&mut a, &mut b, 1, 100_000).unwrap();

    let mut expected = Vec::new();
    for id in 0u32..100 {
        let mut msg = random_bytes(64, 0x900d_f00d ^ id as u64);
        msg[..4].copy_from_slice(&id.to_be_bytes());
        expected.push(msg.clone());
        a.send(ah, sid, &msg, 2).unwrap();
    }

    let mut link = FaultyLink::new(0x0ddc_0ffe_e15e_beef, 120, 120);
    link.pump(&mut a, &mut b, 2, 2_000_000).unwrap();

    let mut delivered = Vec::new();
    while let Some(msg) = b.recv(bh, sid).unwrap() {
        let id = u32::from_be_bytes(msg[..4].try_into().unwrap()) as usize;
        assert_eq!(msg, expected[id], "corrupt datagram must never reach the application");
        delivered.push(id);
    }

    assert!(link.dropped > 0);
    assert!(link.corrupted > 0);
    assert!(!delivered.is_empty());
    assert!(delivered.len() < expected.len(), "unreliable GTS must not retransmit losses");
    assert_eq!(a.tick(10_000).unwrap(), 0, "unreliable streams have no retransmission state");
}
