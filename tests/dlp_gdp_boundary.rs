use smolgnet::*;

fn port(mode: VcMode) -> DlpGdpPort {
    let mut p = DlpGdpPort::new(DlpConfig::new(256, mode).unwrap()).unwrap();
    p.begin_recovery().unwrap();
    p.complete_recovery().unwrap();
    p
}

fn gts_packet(source: u64, destination: u64) -> GdpPacket {
    let h = GdpHeader::global(
        GdpType::Gts,
        SizeClass::Ctrl32,
        8,
        GdpAddress(source),
        GdpAddress(destination),
    );
    GdpPacket::new(h, vec![0x5a; 32]).unwrap()
}

fn gctl_packet(source: u64, destination: u64) -> GdpPacket {
    let h = GdpHeader::global(
        GdpType::Gctl,
        SizeClass::Ctrl32,
        8,
        GdpAddress(source),
        GdpAddress(destination),
    );
    GdpPacket::new(h, vec![0xa5; 32]).unwrap()
}

fn forward_one<I: GdpPacketPort, O: GdpPacketPort>(
    ingress: &mut I,
    egress: &mut O,
) -> Result<bool> {
    let Some(packet) = ingress.poll_gdp() else {
        return Ok(false);
    };
    egress.transmit_gdp(packet)?;
    Ok(true)
}

#[test]
fn router_facing_trait_moves_only_complete_gdp_packets() {
    let mut source = port(VcMode::Two);
    let mut ingress = port(VcMode::Two);
    let mut egress = port(VcMode::Four);
    let mut sink = port(VcMode::Two);
    let packet = gts_packet(1, 2);

    let flits = packet.encode(GdpWireConfig::default()).unwrap().len().div_ceil(4) as u32;

    DlpGdpPort::grant_peer_data_credit(&mut ingress, &mut source, flits).unwrap();
    source.transmit_gdp(packet.clone()).unwrap();
    let frame = source.physical_poll_tx_frame().unwrap().unwrap();
    assert_eq!(frame.traffic, LinkTraffic::Data);
    ingress.physical_receive_frame(frame).unwrap();

    // This helper has access only to the packet-level trait. It cannot select
    // VCIDs, grant credit, inspect flits, or alter DLP scheduling.
    assert!(forward_one(&mut ingress, &mut egress).unwrap());
    assert!(!forward_one(&mut ingress, &mut egress).unwrap());

    DlpGdpPort::grant_peer_data_credit(&mut sink, &mut egress, flits).unwrap();
    let forwarded = egress.physical_poll_tx_frame().unwrap().unwrap();
    assert_eq!(forwarded.traffic, LinkTraffic::Data);
    assert!(!forwarded.vcid.is_control());
    sink.physical_receive_frame(forwarded).unwrap();

    assert_eq!(sink.poll_gdp().unwrap(), packet);
}

#[test]
fn dlp_selects_control_or_data_lane_from_gdp_type() {
    let mut tx = port(VcMode::Four);
    let mut rx = port(VcMode::Four);

    let control = gctl_packet(1, 2);
    let data = gts_packet(1, 2);
    let data_flits = data.encode(GdpWireConfig::default()).unwrap().len().div_ceil(4) as u32;
    let control_flits = control.encode(GdpWireConfig::default()).unwrap().len().div_ceil(4) as u32;

    DlpGdpPort::grant_peer_control_credit(&mut tx, control_flits).unwrap();
    DlpGdpPort::grant_peer_data_credit(&mut rx, &mut tx, data_flits).unwrap();

    // The router submits GDP packets only; there is no VCID or traffic-class
    // argument at this boundary.
    tx.transmit_gdp(data).unwrap();
    tx.transmit_gdp(control).unwrap();

    let first = tx.physical_poll_tx_frame().unwrap().unwrap();
    assert_eq!(first.traffic, LinkTraffic::Control);
    assert_eq!(first.vcid, Vcid::CONTROL);

    let second = tx.physical_poll_tx_frame().unwrap().unwrap();
    assert_eq!(second.traffic, LinkTraffic::Data);
    assert!(!second.vcid.is_control());
}

#[test]
fn incomplete_physical_input_never_crosses_router_boundary() {
    let mut tx = port(VcMode::Two);
    let mut rx = port(VcMode::Two);
    let packet = gts_packet(11, 22);
    let bytes = packet.encode(GdpWireConfig::default()).unwrap();
    let flits = bytes.len().div_ceil(4) as u32;

    DlpGdpPort::grant_peer_data_credit(&mut rx, &mut tx, flits).unwrap();
    tx.transmit_gdp(packet.clone()).unwrap();

    let first = tx.physical_poll_tx_flit().unwrap().unwrap();
    assert!(!rx.physical_receive_flit(first).unwrap());
    assert!(rx.poll_gdp().is_none());

    let mut completed = false;
    while let Some(flit) = tx.physical_poll_tx_flit().unwrap() {
        completed |= rx.physical_receive_flit(flit).unwrap();
    }
    assert!(completed);
    assert_eq!(rx.poll_gdp().unwrap(), packet);
}

#[test]
fn reset_discards_packets_from_previous_dlp_generation() {
    let mut tx = port(VcMode::Two);
    let mut rx = port(VcMode::Two);
    let packet = gts_packet(3, 4);
    let flits = packet.encode(GdpWireConfig::default()).unwrap().len().div_ceil(4) as u32;

    DlpGdpPort::grant_peer_data_credit(&mut rx, &mut tx, flits).unwrap();
    tx.transmit_gdp(packet).unwrap();
    let frame = tx.physical_poll_tx_frame().unwrap().unwrap();
    rx.physical_receive_frame(frame).unwrap();
    assert_eq!(rx.generation(), 0);

    // A packet completed under an old physical generation must not be handed
    // to GDP after reset/recovery.
    rx.reset();
    assert_eq!(rx.generation(), 1);
    rx.begin_recovery().unwrap();
    rx.complete_recovery().unwrap();
    assert!(rx.poll_gdp().is_none());
}
