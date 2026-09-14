use p4_gdp::{
    parse_gdp, validate_gdp, GdpPipeline, P4GdpError, ParsedAddresses, RouteDisposition,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use smolgnet::wire::gdp::{
    GdpAddress, GdpAddresses, GdpHeader, GdpPacket, GdpType, GdpWireConfig, SizeClass,
};

const PREFIX: u64 = 0x1234_5678_9abc_0000;
const SOURCE: u64 = 0x0102_0304_0506_0708;
const DESTINATION: u64 = 0x1112_1314_1516_1718;

fn global_packet_with(
    packet_type: GdpType,
    size_class: SizeClass,
    hop_limit: u8,
    source: u64,
    destination: u64,
) -> Vec<u8> {
    let header = GdpHeader::global(
        packet_type,
        size_class,
        hop_limit,
        GdpAddress(source),
        GdpAddress(destination),
    );
    let payload = vec![0xa5; size_class.bytes()];
    GdpPacket::new(header, payload)
        .unwrap()
        .encode(GdpWireConfig::default())
        .unwrap()
}

fn global_packet(packet_type: GdpType, size_class: SizeClass, hop_limit: u8) -> Vec<u8> {
    global_packet_with(packet_type, size_class, hop_limit, SOURCE, DESTINATION)
}

fn local_packet_with(
    packet_type: GdpType,
    size_class: SizeClass,
    hop_limit: u8,
    prefix: u64,
    source: u16,
    destination: u16,
) -> Vec<u8> {
    let header = GdpHeader::local(
        packet_type,
        size_class,
        hop_limit,
        prefix,
        source,
        destination,
    )
    .unwrap();
    let payload = vec![0x5a; size_class.bytes()];
    GdpPacket::new(header, payload)
        .unwrap()
        .encode(GdpWireConfig::default())
        .unwrap()
}

fn local_packet(packet_type: GdpType, size_class: SizeClass, hop_limit: u8) -> Vec<u8> {
    local_packet_with(packet_type, size_class, hop_limit, PREFIX, 0x1122, 0x3344)
}

fn compare_parsed_fields(bytes: &[u8], local_prefix: u64) {
    let smol = GdpPacket::decode(bytes, GdpWireConfig::default(), local_prefix).unwrap();
    let p4 = validate_gdp(bytes).unwrap();

    assert_eq!(p4.version, smol.header.version);
    assert_eq!(p4.packet_type, smol.header.packet_type.to_wire());
    assert_eq!(p4.size_class, smol.header.size_class as u8);
    assert_eq!(p4.hop_limit, smol.header.hop_limit);
    assert!(p4.crc_is_valid());

    match (&smol.header.addresses, p4.addresses) {
        (
            GdpAddresses::Global {
                destination,
                source,
            },
            ParsedAddresses::Global {
                destination: p4_destination,
                source: p4_source,
            },
        ) => {
            assert!(!p4.local_form);
            assert_eq!(p4_destination, destination.0);
            assert_eq!(p4_source, source.0);
        }
        (
            GdpAddresses::Local {
                destination,
                source,
                ..
            },
            ParsedAddresses::Local {
                destination: p4_destination,
                source: p4_source,
            },
        ) => {
            assert!(p4.local_form);
            assert_eq!(p4_destination, *destination);
            assert_eq!(p4_source, *source);
        }
        (a, b) => panic!("address-form mismatch: smolgnet={a:?}, p4={b:?}"),
    }
}

fn local_delivery(bytes: &[u8], destination: u64) -> Vec<u8> {
    let mut pipeline = GdpPipeline::new(4);
    pipeline.add_global_route(destination, 3, RouteDisposition::Local);
    let output = pipeline.process(1, bytes).unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].port, 3);
    output[0].bytes.clone()
}

#[test]
fn generated_pipeline_exposes_expected_tables() {
    let pipeline = GdpPipeline::new(4);
    let ids = pipeline.table_ids();
    assert!(ids.contains(&"ingress.global_routes"));
    assert!(ids.contains(&"ingress.local_routes"));
}

#[test]
fn explicit_global_fields_match_smolgnet() {
    compare_parsed_fields(
        &global_packet(GdpType::Gts, SizeClass::Ctrl32, 64),
        PREFIX,
    );
}

#[test]
fn explicit_local_fields_match_smolgnet() {
    compare_parsed_fields(
        &local_packet(GdpType::Gctl, SizeClass::Ctrl32, 15),
        PREFIX,
    );
}

#[test]
fn local_delivery_deparses_byte_for_byte() {
    let expected = global_packet(GdpType::Gts, SizeClass::Ctrl32, 64);
    assert_eq!(local_delivery(&expected, DESTINATION), expected);
}

#[test]
fn all_size_classes_match_smolgnet() {
    for size_class in SizeClass::ALL {
        let packet = global_packet(GdpType::Gts, size_class, 31);
        compare_parsed_fields(&packet, PREFIX);
    }
}

#[test]
fn all_packet_type_values_match_smolgnet() {
    for value in 0u8..=15 {
        let packet = global_packet(GdpType::from_wire(value), SizeClass::Ctrl32, 31);
        compare_parsed_fields(&packet, PREFIX);
    }
}

#[test]
fn hop_limit_boundaries_match_smolgnet() {
    for hop_limit in [0, 1, 254, 255] {
        compare_parsed_fields(
            &global_packet(GdpType::Gts, SizeClass::Ctrl32, hop_limit),
            PREFIX,
        );
    }
    for hop_limit in [0, 1, 14, 15] {
        compare_parsed_fields(
            &local_packet(GdpType::Gctl, SizeClass::Ctrl32, hop_limit),
            PREFIX,
        );
    }
}

#[test]
fn truncated_and_wrong_length_packets_are_rejected_by_both() {
    let global = global_packet(GdpType::Gts, SizeClass::Ctrl32, 32);
    for cut in 0..20 {
        assert!(GdpPacket::decode(&global[..cut], GdpWireConfig::default(), PREFIX).is_err());
        assert!(matches!(
            validate_gdp(&global[..cut]),
            Err(P4GdpError::Truncated)
        ));
    }
    assert!(GdpPacket::decode(
        &global[..global.len() - 1],
        GdpWireConfig::default(),
        PREFIX
    )
    .is_err());
    assert!(matches!(
        validate_gdp(&global[..global.len() - 1]),
        Err(P4GdpError::InvalidLength { .. })
    ));

    let local = local_packet(GdpType::Gctl, SizeClass::Ctrl32, 8);
    for cut in 0..8 {
        assert!(GdpPacket::decode(&local[..cut], GdpWireConfig::default(), PREFIX).is_err());
        assert!(matches!(
            validate_gdp(&local[..cut]),
            Err(P4GdpError::Truncated)
        ));
    }

    let mut too_long = global.clone();
    too_long.push(0);
    assert!(GdpPacket::decode(&too_long, GdpWireConfig::default(), PREFIX).is_err());
    assert!(matches!(
        validate_gdp(&too_long),
        Err(P4GdpError::InvalidLength { .. })
    ));
}

#[test]
fn crc8_validation_matches_smolgnet_and_hop_is_excluded() {
    let valid = global_packet(GdpType::Gts, SizeClass::Ctrl32, 64);
    let parsed = validate_gdp(&valid).unwrap();
    assert_eq!(parsed.recomputed_crc8(), parsed.crc8);

    let mut corrupted = valid.clone();
    corrupted[4] ^= 0x01;
    assert!(GdpPacket::decode(&corrupted, GdpWireConfig::default(), PREFIX).is_err());
    assert!(parse_gdp(&corrupted).is_ok());
    assert!(matches!(
        validate_gdp(&corrupted),
        Err(P4GdpError::InvalidCrc { .. })
    ));

    let mut changed_hop = valid;
    changed_hop[3] = 63;
    assert!(GdpPacket::decode(&changed_hop, GdpWireConfig::default(), PREFIX).is_ok());
    assert!(validate_gdp(&changed_hop).is_ok());
}

#[test]
fn randomized_differential_parse_and_crc() {
    let mut rng = StdRng::seed_from_u64(0x474e_4554_5034_4744);

    for _ in 0..512 {
        let size_class = SizeClass::ALL[rng.random_range(0..SizeClass::ALL.len())];
        let packet_type = GdpType::from_wire(rng.random_range(0..16));
        let local = rng.random::<bool>();

        let bytes = if local {
            let source = rng.random::<u16>();
            let destination = rng.random::<u16>();
            let hop = rng.random_range(0..16);
            local_packet_with(
                packet_type,
                size_class,
                hop,
                PREFIX,
                source,
                destination,
            )
        } else {
            global_packet_with(
                packet_type,
                size_class,
                rng.random::<u8>(),
                rng.random::<u64>(),
                rng.random::<u64>(),
            )
        };

        compare_parsed_fields(&bytes, PREFIX);
    }
}

#[test]
fn global_forwarding_decrements_hop_and_preserves_crc_and_payload() {
    let bytes = global_packet(GdpType::Gts, SizeClass::Ctrl32, 5);
    let before = GdpPacket::decode(&bytes, GdpWireConfig::default(), PREFIX).unwrap();
    let before_crc = validate_gdp(&bytes).unwrap().crc8;

    let mut pipeline = GdpPipeline::new(4);
    pipeline.add_global_route(DESTINATION, 2, RouteDisposition::Forward);
    let output = pipeline.process(0, &bytes).unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].port, 2);

    let after = GdpPacket::decode(&output[0].bytes, GdpWireConfig::default(), PREFIX).unwrap();
    assert_eq!(after.header.hop_limit, 4);
    assert_eq!(after.header.source(), before.header.source());
    assert_eq!(after.header.destination(), before.header.destination());
    assert_eq!(after.payload, before.payload);
    assert_eq!(validate_gdp(&output[0].bytes).unwrap().crc8, before_crc);
}

#[test]
fn local_forwarding_decrements_hop() {
    let bytes = local_packet(GdpType::Gctl, SizeClass::Ctrl32, 5);
    let mut pipeline = GdpPipeline::new(4);
    pipeline.add_local_route(0x3344, 2, RouteDisposition::Forward);
    let output = pipeline.process(0, &bytes).unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].port, 2);
    let after = GdpPacket::decode(&output[0].bytes, GdpWireConfig::default(), PREFIX).unwrap();
    assert_eq!(after.header.hop_limit, 4);
}

#[test]
fn forwarding_drops_unknown_expired_and_explicit_drop_routes() {
    let bytes = global_packet(GdpType::Gts, SizeClass::Ctrl32, 5);
    let mut pipeline = GdpPipeline::new(4);
    assert!(pipeline.process(0, &bytes).unwrap().is_empty());

    pipeline.add_global_route(DESTINATION, 2, RouteDisposition::Drop);
    assert!(pipeline.process(0, &bytes).unwrap().is_empty());

    let expired = global_packet(GdpType::Gts, SizeClass::Ctrl32, 1);
    let mut pipeline = GdpPipeline::new(4);
    pipeline.add_global_route(DESTINATION, 2, RouteDisposition::Forward);
    assert!(pipeline.process(0, &expired).unwrap().is_empty());
}

#[test]
fn local_delivery_does_not_consume_hop() {
    let bytes = global_packet(GdpType::Gctl, SizeClass::Ctrl32, 1);
    let mut pipeline = GdpPipeline::new(4);
    pipeline.add_global_route(DESTINATION, 3, RouteDisposition::Local);
    let output = pipeline.process(0, &bytes).unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].port, 3);
    let decoded = GdpPacket::decode(&output[0].bytes, GdpWireConfig::default(), PREFIX).unwrap();
    assert_eq!(decoded.header.hop_limit, 1);
}
