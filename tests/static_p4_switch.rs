use smolgnet::error::Error;
use smolgnet::wire::gdp::{GdpAddress, GdpHeader, GdpPacket, GdpType, SizeClass};
use smolgnet::{StaticP4Switch, SwitchDisposition, SWITCH_PORT_COUNT};

fn packet(destination: u64, source: u64, hop: u8) -> GdpPacket {
    let header = GdpHeader::global(
        GdpType::Gts,
        SizeClass::B64,
        hop,
        GdpAddress(destination),
        GdpAddress(source),
    )
    .unwrap();
    GdpPacket::new(header, vec![0x5a; 64]).unwrap()
}

#[test]
fn switch_has_exactly_eight_ports() {
    let switch = StaticP4Switch::new();
    assert_eq!(switch.physical_port_count(), 8);
    assert_eq!(SWITCH_PORT_COUNT, 8);
}

#[test]
fn node_table_registers_moves_and_removes_nodes() {
    let mut switch = StaticP4Switch::new();
    let node = GdpAddress(0x1200_0000_0000_0042);
    assert_eq!(switch.register_node(node, 0).unwrap(), None);
    assert_eq!(switch.node_port(node), Some(0));
    assert_eq!(switch.register_node(node, 7).unwrap(), Some(0));
    assert_eq!(switch.node_port(node), Some(7));
    assert_eq!(switch.remove_node(node), Some(7));
    assert_eq!(switch.node_port(node), None);
}

#[test]
fn invalid_ports_and_link_local_nodes_are_rejected() {
    let mut switch = StaticP4Switch::new();
    assert_eq!(
        switch
            .register_node(GdpAddress(0x1200_0000_0000_0001), 8)
            .unwrap_err(),
        Error::InvalidField
    );
    assert_eq!(
        switch
            .register_node(GdpAddress(0xfe80_0000_0000_0001), 0)
            .unwrap_err(),
        Error::InvalidField
    );
    assert_eq!(
        switch
            .process(
                8,
                packet(0x1200_0000_0000_0001, 0x9900_0000_0000_0001, 8)
            )
            .unwrap_err(),
        Error::InvalidField
    );
}

#[test]
fn forwards_zero_to_seven_without_modifying_gdp() {
    let mut switch = StaticP4Switch::new();
    let destination = GdpAddress(0x1200_0000_0000_0007);
    switch.register_node(destination, 7).unwrap();
    let original = packet(destination.0, 0x9900_0000_0000_0001, 9);

    match switch.process(0, original.clone()).unwrap() {
        SwitchDisposition::Forward {
            egress_port,
            packet,
        } => {
            assert_eq!(egress_port, 7);
            assert_eq!(packet, original);
            assert_eq!(packet.header.hop_limit, 9);
        }
        other => panic!("expected forwarding, got {other:?}"),
    }
}

#[test]
fn forwards_seven_to_zero_without_modifying_gdp() {
    let mut switch = StaticP4Switch::new();
    let destination = GdpAddress(0x2200_0000_0000_0001);
    switch.register_node(destination, 0).unwrap();
    let original = packet(destination.0, 0x9900_0000_0000_0007, 3);

    assert_eq!(
        switch.process(7, original.clone()).unwrap(),
        SwitchDisposition::Forward {
            egress_port: 0,
            packet: original,
        }
    );
}

#[test]
fn unknown_destination_and_hairpin_are_dropped() {
    let mut switch = StaticP4Switch::new();
    let destination = GdpAddress(0x3300_0000_0000_0001);
    let original = packet(destination.0, 0x9900_0000_0000_0001, 8);
    assert_eq!(
        switch.process(0, original.clone()).unwrap(),
        SwitchDisposition::Drop
    );

    switch.register_node(destination, 0).unwrap();
    assert_eq!(
        switch.process(0, original).unwrap(),
        SwitchDisposition::Drop
    );
}

#[test]
fn multiple_nodes_can_share_one_egress_port() {
    let mut switch = StaticP4Switch::new();
    let a = GdpAddress(0x4400_0000_0000_0001);
    let b = GdpAddress(0x4400_0000_0000_0002);
    switch.register_node(a, 6).unwrap();
    switch.register_node(b, 6).unwrap();

    for destination in [a, b] {
        assert!(matches!(
            switch
                .process(
                    1,
                    packet(destination.0, 0x9900_0000_0000_0001, 8)
                )
                .unwrap(),
            SwitchDisposition::Forward { egress_port: 6, .. }
        ));
    }
}
