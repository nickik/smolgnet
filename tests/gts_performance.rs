use std::hint::black_box;
use std::time::{Duration, Instant};

use smolgnet::p4_gts::{encode_handwritten_reference, encode_p4_reference};
use smolgnet::wire::gts::GtsContext;
use smolgnet::*;

#[derive(Clone)]
struct Case {
    name: &'static str,
    packet: GtsPacket,
    bytes: Vec<u8>,
    ctx: GtsContext,
    profile: Option<StreamProfile>,
}

fn context(class: SizeClass) -> GtsContext {
    GtsContext {
        gdp_version: 0,
        size_class: class,
        source: GdpAddress(0x1111_2222_3333_4444),
        destination: GdpAddress(0xaaaa_bbbb_cccc_dddd),
    }
}

fn make_case(name: &'static str, packet: GtsPacket, profile: Option<StreamProfile>) -> Case {
    let class = packet.choose_size_class(profile).unwrap();
    let ctx = context(class);
    // Build fixtures through the handwritten reference so RX benchmarking does
    // not accidentally privilege whichever production backend the feature set
    // selected for this test binary.
    let bytes = encode_handwritten_reference(&packet, ctx, profile).unwrap();
    Case {
        name,
        packet,
        bytes,
        ctx,
        profile,
    }
}

fn cases() -> Vec<Case> {
    let reliable_variable =
        StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let reliable_fixed =
        StreamProfile::reliable_fixed(SizeClass::Ctrl32, Direction::Bidirectional);
    let datagram = StreamProfile::unreliable_variable(
        SizeClass::Ctrl64,
        Direction::Bidirectional,
        true,
        false,
    );

    vec![
        make_case(
            "ack-ctrl32",
            GtsPacket::Ack {
                tunnel_id: 0x1234_5678,
                stream_id: 3,
                ack_base: 100,
                receive_bitmap: 0x55aa_aa55,
                receive_credit: 16,
            },
            None,
        ),
        make_case(
            "data-fixed-32",
            GtsPacket::Data {
                tunnel_id: 0x1234_5678,
                stream_id: 3,
                sequence: 101,
                data: vec![0x5a; 18],
                end: false,
            },
            Some(reliable_fixed),
        ),
        make_case(
            "data-variable-128",
            GtsPacket::Data {
                tunnel_id: 0x1234_5678,
                stream_id: 3,
                sequence: 102,
                data: vec![0x5a; 72],
                end: false,
            },
            Some(reliable_variable),
        ),
        make_case(
            "datagram-seq-var-64",
            GtsPacket::Datagram {
                tunnel_id: 0x1234_5678,
                stream_id: 4,
                sequence: Some(103),
                data: vec![0x33; 24],
            },
            Some(datagram),
        ),
    ]
}

fn bench<F>(iterations: usize, mut f: F) -> Duration
where
    F: FnMut(),
{
    for _ in 0..5_000 {
        f();
    }
    let start = Instant::now();
    for _ in 0..iterations {
        f();
    }
    start.elapsed()
}

fn ns_per_packet(d: Duration, iterations: usize) -> f64 {
    d.as_secs_f64() * 1e9 / iterations as f64
}

fn mpps(d: Duration, iterations: usize) -> f64 {
    iterations as f64 / d.as_secs_f64() / 1e6
}

#[test]
#[ignore = "release/LTO GTS lower-backend receive microbenchmark"]
fn compare_handwritten_and_p4_gts_decode() {
    let iterations = 250_000usize;
    println!("GTS RX benchmark: {iterations} iterations per case");
    println!("case,handwritten_ns,p4_ns,p4_over_handwritten,mpps_handwritten,mpps_p4");

    for case in cases() {
        let hand = bench(iterations, || {
            let packet = GtsPacket::decode_handwritten_reference(
                black_box(case.bytes.as_slice()),
                case.ctx,
                case.profile,
            )
            .unwrap();
            black_box(packet);
        });
        let p4 = bench(iterations, || {
            let packet = GtsPacket::decode_p4_reference(
                black_box(case.bytes.as_slice()),
                case.ctx,
                case.profile,
            )
            .unwrap();
            black_box(packet);
        });

        let hand_ns = ns_per_packet(hand, iterations);
        let p4_ns = ns_per_packet(p4, iterations);
        println!(
            "{},{:.2},{:.2},{:.3},{:.3},{:.3}",
            case.name,
            hand_ns,
            p4_ns,
            p4_ns / hand_ns,
            mpps(hand, iterations),
            mpps(p4, iterations)
        );
    }
}

#[test]
#[ignore = "release/LTO GTS lower-backend transmit microbenchmark"]
fn compare_handwritten_and_p4_gts_encode() {
    // x4c header construction/deparsing allocates more than the handwritten
    // lower serializer, so use fewer iterations than RX while still keeping
    // timing noise well below the backend differences.
    let iterations = 100_000usize;
    println!("GTS TX benchmark: {iterations} iterations per case");
    println!("case,handwritten_ns,p4_ns,p4_over_handwritten,mpps_handwritten,mpps_p4");

    for case in cases() {
        let hand = bench(iterations, || {
            let bytes = encode_handwritten_reference(
                black_box(&case.packet),
                case.ctx,
                case.profile,
            )
            .unwrap();
            black_box(bytes);
        });
        let p4 = bench(iterations, || {
            let bytes = encode_p4_reference(
                black_box(&case.packet),
                case.ctx,
                case.profile,
            )
            .unwrap();
            black_box(bytes);
        });

        let hand_ns = ns_per_packet(hand, iterations);
        let p4_ns = ns_per_packet(p4, iterations);
        println!(
            "{},{:.2},{:.2},{:.3},{:.3},{:.3}",
            case.name,
            hand_ns,
            p4_ns,
            p4_ns / hand_ns,
            mpps(hand, iterations),
            mpps(p4, iterations)
        );
    }
}
