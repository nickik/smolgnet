use std::hint::black_box;
use std::time::{Duration, Instant};

use smolgnet::p4_gdp::parse_and_validate;
use smolgnet::{GdpAddress, GdpHeader, GdpPacket, GdpType, GdpWireConfig, SizeClass};

#[derive(Clone)]
struct Case {
    name: &'static str,
    bytes: Vec<u8>,
    local_prefix: u64,
}

fn packet_cases() -> Vec<Case> {
    let cfg = GdpWireConfig::default();
    let prefix = 0x1234_5678_9abc_0000;

    let global_tiny = GdpPacket::new(
        GdpHeader::global(
            GdpType::Gts,
            SizeClass::Tiny3,
            63,
            GdpAddress(0x0102_0304_0506_0708),
            GdpAddress(0x1112_1314_1516_1718),
        ),
        vec![0x5a; SizeClass::Tiny3.bytes()],
    )
    .unwrap()
    .encode(cfg)
    .unwrap();

    let local_ctrl = GdpPacket::new(
        GdpHeader::local(GdpType::Gctl, SizeClass::Ctrl32, 7, prefix, 0x1234, 0xabcd).unwrap(),
        vec![0xa5; SizeClass::Ctrl32.bytes()],
    )
    .unwrap()
    .encode(cfg)
    .unwrap();

    let global_bulk = GdpPacket::new(
        GdpHeader::global(
            GdpType::Gts,
            SizeClass::Bulk1280,
            63,
            GdpAddress(0x2122_2324_2526_2728),
            GdpAddress(0x3132_3334_3536_3738),
        ),
        vec![0x3c; SizeClass::Bulk1280.bytes()],
    )
    .unwrap()
    .encode(cfg)
    .unwrap();

    vec![
        Case { name: "global-tiny3", bytes: global_tiny, local_prefix: prefix },
        Case { name: "local-ctrl32", bytes: local_ctrl, local_prefix: prefix },
        Case { name: "global-bulk1280", bytes: global_bulk, local_prefix: prefix },
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
#[ignore = "release/LTO GDP parser microbenchmark"]
fn compare_handwritten_and_x4c_gdp_decode() {
    let cfg = GdpWireConfig::default();
    let iterations = 250_000usize;

    println!("GDP decode benchmark: {iterations} iterations per case");
    println!("mode,case,handwritten_ns,p4_ns,p4_over_handwritten,mpps_handwritten,mpps_p4");

    for case in packet_cases() {
        let handwritten_header = bench(iterations, || {
            let h = GdpHeader::decode_handwritten_reference(
                black_box(case.bytes.as_slice()),
                cfg,
                case.local_prefix,
            )
            .unwrap();
            black_box(h);
        });
        let p4_header = bench(iterations, || {
            let h = parse_and_validate(black_box(case.bytes.as_slice())).unwrap();
            black_box(h);
        });

        let hw_ns = ns_per_packet(handwritten_header, iterations);
        let p4_ns = ns_per_packet(p4_header, iterations);
        println!(
            "header,{},{:.2},{:.2},{:.3},{:.3},{:.3}",
            case.name,
            hw_ns,
            p4_ns,
            p4_ns / hw_ns,
            mpps(handwritten_header, iterations),
            mpps(p4_header, iterations)
        );
    }
}
