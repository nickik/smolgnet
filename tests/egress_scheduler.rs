use smolgnet::*;

const CREDIT: u32 = 1_000_000;

fn link(mode: VcMode) -> DlpLink {
    let mut link = DlpLink::new(DlpConfig::new(CREDIT, mode).unwrap()).unwrap();
    link.begin_recovery().unwrap();
    link.complete_recovery().unwrap();
    link.endpoint();
    link
}

fn packet(flow: u64, sequence: u8) -> GdpPacket {
    let source = GdpAddress(0x1000 + flow);
    let destination = GdpAddress(0x2000 + flow);
    let mut payload = vec![0u8; SizeClass::Ctrl32.bytes()];
    payload[0] = flow as u8;
    payload[1] = sequence;
    GdpPacket::new(
        GdpHeader::global(
            GdpType::Gts,
            SizeClass::Ctrl32,
            16,
            source,
            destination,
        ),
        payload,
    )
    .unwrap()
}

fn flow(id: u64) -> EgressFlowId {
    EgressFlowId::new(GdpAddress(0x1000 + id), GdpAddress(0x2000 + id), id)
}

fn drain_round(link: &mut DlpLink, count: usize) -> Vec<(u8, u8, Vcid)> {
    let mut out = Vec::new();
    for _ in 0..count {
        let frame = link.poll_tx_frame().unwrap().unwrap();
        let vcid = frame.vcid;
        let packet = GdpPacket::decode(&frame.bytes, GdpWireConfig::default()).unwrap();
        out.push((packet.payload[0], packet.payload[1], vcid));
    }
    out
}

#[test]
fn vc4_sustained_busy_flows_rotate_one_packet_quantum() {
    let mut scheduler = RouterEgressScheduler::new();
    let mut tx = link(VcMode::Four);
    tx.endpoint();
    // Credit belongs to the sender for this isolated egress test.
    let mut rx = link(VcMode::Four);
    DlpLink::grant_peer_data_credit(&mut rx, &mut tx, CREDIT).unwrap();

    // Eight flows are permanently backlogged for several rounds. A FIFO of
    // packets per flow plus round-robin flow rotation must give every flow a
    // turn before any flow receives its second quantum.
    for id in 0..8u64 {
        for seq in 0..4u8 {
            scheduler.enqueue(flow(id), packet(id, seq));
        }
    }

    let mut history = Vec::new();
    while scheduler.queued_packets() != 0 {
        let admitted = scheduler.schedule_round(&mut tx).unwrap();
        assert!((1..=3).contains(&admitted));
        history.extend(drain_round(&mut tx, admitted));
    }

    assert_eq!(history.len(), 32);
    for (position, (flow_id, sequence, vcid)) in history.iter().enumerate() {
        assert_eq!(*flow_id as usize, position % 8);
        assert_eq!(*sequence as usize, position / 8);
        assert_eq!(*vcid, [Vcid::VC1, Vcid::VC2, Vcid::VC3][position % 3]);
    }
    assert_eq!(scheduler.flow_count(), 0);
}

#[test]
fn newly_waiting_flow_cannot_be_starved_by_busy_flows() {
    let mut scheduler = RouterEgressScheduler::new();
    let mut tx = link(VcMode::Four);
    let mut rx = link(VcMode::Four);
    DlpLink::grant_peer_data_credit(&mut rx, &mut tx, CREDIT).unwrap();

    for id in 0..3u64 {
        for seq in 0..20u8 {
            scheduler.enqueue(flow(id), packet(id, seq));
        }
    }

    let first = scheduler.schedule_round(&mut tx).unwrap();
    assert_eq!(first, 3);
    drain_round(&mut tx, first);

    scheduler.enqueue(flow(9), packet(9, 0));

    // The three old flows were appended to the ready tail after their first
    // quanta. The new flow joins behind them, so it must appear no later than
    // the second subsequent VC4 round; it cannot wait for the busy flows to
    // drain their twenty-packet backlogs.
    let second = scheduler.schedule_round(&mut tx).unwrap();
    let mut seen = drain_round(&mut tx, second);
    let third = scheduler.schedule_round(&mut tx).unwrap();
    seen.extend(drain_round(&mut tx, third));
    assert!(seen.iter().any(|(id, _, _)| *id == 9));
}

#[test]
fn vc2_admits_exactly_one_flow_per_round() {
    let mut scheduler = RouterEgressScheduler::new();
    let mut tx = link(VcMode::Two);
    let mut rx = link(VcMode::Two);
    DlpLink::grant_peer_data_credit(&mut rx, &mut tx, CREDIT).unwrap();

    for id in 0..4u64 {
        scheduler.enqueue(flow(id), packet(id, 0));
    }

    for id in 0..4u8 {
        assert_eq!(scheduler.schedule_round(&mut tx).unwrap(), 1);
        assert_eq!(drain_round(&mut tx, 1), vec![(id, 0, Vcid::VC1)]);
    }
}

#[test]
fn scheduler_refuses_to_stack_rounds_in_dlp() {
    let mut scheduler = RouterEgressScheduler::new();
    let mut tx = link(VcMode::Four);
    let mut rx = link(VcMode::Four);
    DlpLink::grant_peer_data_credit(&mut rx, &mut tx, CREDIT).unwrap();

    for id in 0..4u64 {
        scheduler.enqueue(flow(id), packet(id, 0));
    }
    assert_eq!(scheduler.schedule_round(&mut tx).unwrap(), 3);
    assert_eq!(scheduler.schedule_round(&mut tx), Err(Error::InvalidState));
}
