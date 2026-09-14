use smolgnet::*;

fn dlp(buffer: u32, mode: VcMode) -> DlpEndpoint {
    DlpEndpoint::new(DlpConfig::new(buffer, mode).unwrap()).unwrap()
}

fn packet() -> GdpPacket {
    let h = GdpHeader::global(
        GdpType::Gts,
        SizeClass::Ctrl32,
        64,
        GdpAddress(1),
        GdpAddress(2),
    );
    GdpPacket::new(h, vec![0xaa; 32]).unwrap()
}

#[test]
fn data_requires_real_receiver_credit() {
    let mut a = dlp(64, VcMode::Two);
    let mut b = dlp(64, VcMode::Two);
    let p = packet();
    let n = a.queue_data_packet(&p).unwrap();

    assert_eq!(a.poll_tx().unwrap_err(), Error::NoCredit);
    assert_eq!(b.grantable_data_credit(), 64);

    b.note_data_credit_granted(n as u32).unwrap();
    a.grant_data_tx_credit(n as u32);

    let mut got = None;
    for _ in 0..n {
        let f = a.poll_tx().unwrap().unwrap();
        got = b.receive(f).unwrap().or(got);
    }
    assert_eq!(got.unwrap(), p);
    assert_eq!(a.data_tx_credit(), 0);
    assert_eq!(b.rx_buffer_in_use(), 0);
    assert_eq!(b.data_credit_outstanding(), 0);
    assert_eq!(b.grantable_data_credit(), 64);
}

#[test]
fn burst_path_moves_same_packet_and_credit_as_single_flits() {
    let mut a = dlp(128, VcMode::Four);
    let mut b = dlp(128, VcMode::Four);
    let p = packet();
    let n = a.queue_data_packet(&p).unwrap();

    b.note_data_credit_granted(n as u32).unwrap();
    a.grant_data_tx_credit(n as u32);

    let mut flits = [Flit {
        vcid: Vcid::CONTROL,
        data: 0,
    }; 64];
    let moved = a.poll_tx_burst(&mut flits).unwrap();
    assert_eq!(moved, n);
    assert!(flits[..moved].iter().all(|f| !f.vcid.is_control()));

    let mut packets = Vec::new();
    assert_eq!(b.receive_burst(&flits[..moved], &mut packets).unwrap(), 1);
    assert_eq!(packets, vec![p]);
    assert_eq!(a.data_tx_credit(), 0);
    assert_eq!(b.data_credit_outstanding(), 0);
    assert_eq!(b.rx_buffer_in_use(), 0);
}

#[test]
fn burst_tx_keeps_control_priority_and_can_fill_with_data() {
    let mut a = dlp(256, VcMode::Two);
    let data = packet();
    let control_header = GdpHeader::global(
        GdpType::Gctl,
        SizeClass::Ctrl32,
        64,
        GdpAddress(1),
        GdpAddress(2),
    );
    let control = GdpPacket::new(control_header, vec![0; 32]).unwrap();
    let cn = a.queue_control_packet(&control).unwrap();
    let dn = a.queue_data_packet(&data).unwrap();
    a.grant_control_tx_credit(cn as u32);
    a.grant_data_tx_credit(dn as u32);

    let mut flits = [Flit {
        vcid: Vcid::CONTROL,
        data: 0,
    }; 64];
    let moved = a.poll_tx_burst(&mut flits).unwrap();
    assert_eq!(moved, cn + dn);
    assert!(flits[..cn].iter().all(|f| f.vcid.is_control()));
    assert!(flits[cn..moved].iter().all(|f| !f.vcid.is_control()));
}

#[test]
fn receiver_rejects_flit_not_backed_by_advertised_credit() {
    let mut b = dlp(4, VcMode::Two);
    let f = Flit {
        vcid: Vcid::VC1,
        data: 0,
    };
    assert_eq!(b.receive(f).unwrap_err(), Error::CreditViolation);
}

#[test]
fn data_credit_cannot_exceed_free_receive_buffer() {
    let mut b = dlp(8, VcMode::Two);
    b.note_data_credit_granted(8).unwrap();
    assert_eq!(b.grantable_data_credit(), 0);
    assert_eq!(b.note_data_credit_granted(1).unwrap_err(), Error::BufferFull);
}

#[test]
fn vc2_and_vc4_are_hidden_data_scheduling_profiles() {
    for mode in [VcMode::Two, VcMode::Four] {
        let mut a = dlp(256, mode);
        let p = packet();
        let mut used = Vec::new();

        for _ in 0..6 {
            let n = a.queue_data_packet(&p).unwrap();
            a.grant_data_tx_credit(n as u32);
            let first = a.poll_tx().unwrap().unwrap();
            used.push(first.vcid.get());
            // drain the rest of this packet
            for _ in 1..n {
                a.poll_tx().unwrap().unwrap();
            }
        }

        assert!(used.iter().all(|&v| v > 0 && v < mode.count()));
        if mode == VcMode::Two {
            assert!(used.iter().all(|&v| v == 1));
        } else {
            assert!(used.contains(&1));
            assert!(used.contains(&2));
            assert!(used.contains(&3));
        }
    }
}

#[test]
fn control_lane_does_not_consume_data_credit() {
    let mut a = dlp(8, VcMode::Two);
    let h = GdpHeader::global(
        GdpType::Gctl,
        SizeClass::Ctrl32,
        64,
        GdpAddress(1),
        GdpAddress(2),
    );
    let p = GdpPacket::new(h, vec![0; 32]).unwrap();
    let n = a.queue_control_packet(&p).unwrap();
    a.grant_control_tx_credit(n as u32);
    for _ in 0..n {
        let f = a.poll_tx().unwrap().unwrap();
        assert_eq!(f.vcid, Vcid::CONTROL);
    }
    assert_eq!(a.data_tx_credit(), 0);
}

#[test]
fn invalid_header_desynchronizes_only_the_affected_vc() {
    let cfg = GdpWireConfig::default();
    let mut dcfg = DlpConfig::new(128, VcMode::Four).unwrap();
    dcfg.gdp = cfg;
    let mut b = DlpEndpoint::new(dcfg).unwrap();

    let h = GdpHeader::global(
        GdpType::Gts,
        SizeClass::Ctrl32,
        64,
        GdpAddress(1),
        GdpAddress(2),
    );
    let p = GdpPacket::new(h, vec![0; 32]).unwrap();
    let bytes = p.encode(cfg).unwrap();
    let mut words = bytes
        .chunks(4)
        .map(|c| {
            let mut x = [0u8; 4];
            x[..c.len()].copy_from_slice(c);
            u32::from_be_bytes(x)
        })
        .collect::<Vec<_>>();
    words[1] ^= 1;

    let vc = Vcid::VC2;
    b.note_data_credit_granted(words.len() as u32 + 1).unwrap();
    let mut failed = false;
    for w in words {
        match b.receive(Flit { vcid: vc, data: w }) {
            Err(Error::InvalidCrc) => {
                failed = true;
                break;
            }
            Err(e) => panic!("unexpected {e:?}"),
            _ => {}
        }
    }
    assert!(failed);
    assert_eq!(
        b.receive(Flit { vcid: vc, data: 0 }).unwrap_err(),
        Error::Desynchronized
    );
    b.reset_vc(vc).unwrap();
}
