use smolgnet::*;

fn link(buffer: u32, mode: VcMode) -> DlpLink {
    let mut l = DlpLink::new(DlpConfig::new(buffer, mode).unwrap()).unwrap();
    l.begin_recovery().unwrap();
    l.complete_recovery().unwrap();
    l
}

fn data_packet(fill: u8) -> GdpPacket {
    let h = GdpHeader::global(
        GdpType::Gts,
        SizeClass::Ctrl32,
        64,
        GdpAddress(1),
        GdpAddress(2),
    );
    GdpPacket::new(h, vec![fill; 32]).unwrap()
}

fn control_packet() -> GdpPacket {
    let h = GdpHeader::global(
        GdpType::Gctl,
        SizeClass::Ctrl32,
        64,
        GdpAddress(1),
        GdpAddress(2),
    );
    GdpPacket::new(h, vec![0x55; 32]).unwrap()
}

#[test]
fn lifecycle_reset_invalidates_credit_and_queues() {
    let mut a = DlpLink::new(DlpConfig::new(64, VcMode::Two).unwrap()).unwrap();
    let p = data_packet(0xaa);

    assert_eq!(a.state(), DlpLinkState::Down);
    assert_eq!(a.queue_data_packet(&p).unwrap_err(), Error::LinkDown);
    a.begin_recovery().unwrap();
    assert_eq!(a.queue_data_packet(&p).unwrap_err(), Error::LinkDown);
    a.complete_recovery().unwrap();

    let n = a.queue_data_packet(&p).unwrap();
    assert_eq!(n, 13);
    let mut receiver = link(64, VcMode::Two);
    DlpLink::grant_peer_data_credit(&mut receiver, &mut a, n as u32).unwrap();
    assert_eq!(a.endpoint().data_tx_credit(), n as u32);
    assert_eq!(a.endpoint().queued_data_flits(), n);

    a.reset();
    assert_eq!(a.state(), DlpLinkState::Down);
    assert_eq!(a.generation(), 1);
    assert_eq!(a.endpoint().data_tx_credit(), 0);
    assert_eq!(a.endpoint().queued_data_flits(), 0);

    a.begin_recovery().unwrap();
    a.complete_recovery().unwrap();
    assert_eq!(a.poll_tx().unwrap(), None);
}

#[test]
fn exact_two_endpoint_flit_sequence_round_trips() {
    let mut a = link(128, VcMode::Two);
    let mut b = link(128, VcMode::Two);
    let p = data_packet(0xa5);
    let expected = p.encode(GdpWireConfig::default()).unwrap();
    let n = a.queue_data_packet(&p).unwrap();
    DlpLink::grant_peer_data_credit(&mut b, &mut a, n as u32).unwrap();

    let expected_words = expected.chunks(4).map(|chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        u32::from_be_bytes(word)
    });

    let mut got = None;
    for expected_word in expected_words {
        let flit = a.poll_tx().unwrap().unwrap();
        assert_eq!(flit.vcid, Vcid::VC1);
        assert_eq!(flit.data, expected_word);
        if let Some(packet) = b.receive(flit).unwrap() {
            got = Some(packet);
        }
    }
    assert_eq!(got, Some(p));
    assert_eq!(a.endpoint().data_tx_credit(), 0);
    assert_eq!(b.endpoint().data_credit_outstanding(), 0);
    assert_eq!(b.endpoint().rx_buffer_in_use(), 0);
}

#[test]
fn vc4_assignment_is_deterministic() {
    let mut a = link(256, VcMode::Four);
    let mut b = link(256, VcMode::Four);
    let p = data_packet(0x11);
    let mut first_vcs = Vec::new();

    for _ in 0..6 {
        let n = a.queue_data_packet(&p).unwrap();
        DlpLink::grant_peer_data_credit(&mut b, &mut a, n as u32).unwrap();
        let first = a.poll_tx().unwrap().unwrap();
        first_vcs.push(first.vcid);
        b.receive(first).unwrap();
        for _ in 1..n {
            b.receive(a.poll_tx().unwrap().unwrap()).unwrap();
        }
    }

    assert_eq!(first_vcs, vec![
        Vcid::VC1, Vcid::VC2, Vcid::VC3,
        Vcid::VC1, Vcid::VC2, Vcid::VC3,
    ]);
}

#[test]
fn control_progresses_before_data_with_independent_credit() {
    let mut a = link(256, VcMode::Two);
    let mut b = link(256, VcMode::Two);
    let dn = a.queue_data_packet(&data_packet(0x22)).unwrap();
    let cn = a.queue_control_packet(&control_packet()).unwrap();
    DlpLink::grant_peer_data_credit(&mut b, &mut a, dn as u32).unwrap();
    DlpLink::grant_peer_control_credit(&mut a, cn as u32).unwrap();

    for _ in 0..cn {
        assert_eq!(a.poll_tx().unwrap().unwrap().vcid, Vcid::CONTROL);
    }
    assert_eq!(a.endpoint().data_tx_credit(), dn as u32);
    assert_eq!(a.poll_tx().unwrap().unwrap().vcid, Vcid::VC1);
}

#[test]
fn burst_and_single_flit_paths_are_identical() {
    let p = data_packet(0x39);
    let mut single_a = link(128, VcMode::Two);
    let mut single_b = link(128, VcMode::Two);
    let n = single_a.queue_data_packet(&p).unwrap();
    DlpLink::grant_peer_data_credit(&mut single_b, &mut single_a, n as u32).unwrap();
    let mut single = Vec::new();
    for _ in 0..n {
        single.push(single_a.poll_tx().unwrap().unwrap());
    }

    let mut burst_a = link(128, VcMode::Two);
    let mut burst_b = link(128, VcMode::Two);
    let n2 = burst_a.queue_data_packet(&p).unwrap();
    DlpLink::grant_peer_data_credit(&mut burst_b, &mut burst_a, n2 as u32).unwrap();
    let mut burst = [Flit { vcid: Vcid::CONTROL, data: 0 }; 64];
    let moved = burst_a.poll_tx_burst(&mut burst).unwrap();

    assert_eq!(n2, n);
    assert_eq!(&burst[..moved], single.as_slice());
}

#[test]
fn credit_is_bounded_and_reestablished_after_reset() {
    let mut a = link(16, VcMode::Two);
    let mut b = link(16, VcMode::Two);
    assert_eq!(
        DlpLink::grant_peer_data_credit(&mut b, &mut a, 17).unwrap_err(),
        Error::BufferFull
    );
    DlpLink::grant_peer_data_credit(&mut b, &mut a, 16).unwrap();
    assert_eq!(a.endpoint().data_tx_credit(), 16);
    assert_eq!(b.endpoint().grantable_data_credit(), 0);

    a.reset();
    b.reset();
    a.begin_recovery().unwrap();
    b.begin_recovery().unwrap();
    a.complete_recovery().unwrap();
    b.complete_recovery().unwrap();

    assert_eq!(a.endpoint().data_tx_credit(), 0);
    assert_eq!(b.endpoint().data_credit_outstanding(), 0);
    assert_eq!(b.endpoint().grantable_data_credit(), 16);
}
