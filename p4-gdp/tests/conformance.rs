use p4_gdp::main_pipeline;
use p4rs::{packet_in, Pipeline};
use smolgnet::wire::gdp::{
    GdpAddress, GdpHeader, GdpPacket, GdpType, GdpWireConfig, SizeClass,
};

fn through_p4(bytes: &[u8]) -> Vec<u8> {
    let mut pipeline = main_pipeline::new(4);
    let mut input = packet_in::new(bytes);
    let mut output = pipeline.process_packet(7, &mut input);
    assert_eq!(output.len(), 1, "transparent GDP pipeline must emit one packet");
    let (packet, port) = output.remove(0);
    assert_eq!(port, 7, "phase-0 pipeline must preserve the ingress port");

    let mut bytes = packet.header_data;
    bytes.extend_from_slice(packet.payload_data);
    bytes
}

fn global_packet(packet_type: GdpType, size_class: SizeClass, hop_limit: u8) -> Vec<u8> {
    let header = GdpHeader::global(
        packet_type,
        size_class,
        hop_limit,
        GdpAddress(0x0102_0304_0506_0708),
        GdpAddress(0x1112_1314_1516_1718),
    );
    let payload = vec![0xa5; size_class.bytes()];
    GdpPacket::new(header, payload)
        .unwrap()
        .encode(GdpWireConfig::default())
        .unwrap()
}

fn local_packet(packet_type: GdpType, size_class: SizeClass, hop_limit: u8) -> Vec<u8> {
    let header = GdpHeader::local(
        packet_type,
        size_class,
        hop_limit,
        0x1234_5678_9abc_0000,
        0x1122,
        0x3344,
    )
    .unwrap();
    let payload = vec![0x5a; size_class.bytes()];
    GdpPacket::new(header, payload)
        .unwrap()
        .encode(GdpWireConfig::default())
        .unwrap()
}

#[test]
fn global_header_round_trips_through_x4c_pipeline() {
    let expected = global_packet(GdpType::Gts, SizeClass::Ctrl32, 64);
    assert_eq!(through_p4(&expected), expected);
}

#[test]
fn local_header_round_trips_through_x4c_pipeline() {
    let expected = local_packet(GdpType::Gctl, SizeClass::Ctrl32, 15);
    assert_eq!(through_p4(&expected), expected);
}

#[test]
fn all_size_classes_round_trip() {
    for size_class in SizeClass::ALL {
        let expected = global_packet(GdpType::Gts, size_class, 31);
        assert_eq!(through_p4(&expected), expected, "size class {size_class:?}");
    }
}

#[test]
fn all_packet_type_values_round_trip() {
    for value in 0u8..=15 {
        let expected = global_packet(GdpType::from_wire(value), SizeClass::Ctrl32, 31);
        assert_eq!(through_p4(&expected), expected, "GDP type {value:#x}");
    }
}

#[test]
fn global_hop_limit_boundaries_round_trip() {
    for hop_limit in [0, 1, 254, 255] {
        let expected = global_packet(GdpType::Gts, SizeClass::Ctrl32, hop_limit);
        assert_eq!(through_p4(&expected), expected, "hop limit {hop_limit}");
    }
}

#[test]
fn local_hop_limit_boundaries_round_trip() {
    for hop_limit in [0, 1, 14, 15] {
        let expected = local_packet(GdpType::Gctl, SizeClass::Ctrl32, hop_limit);
        assert_eq!(through_p4(&expected), expected, "hop limit {hop_limit}");
    }
}
