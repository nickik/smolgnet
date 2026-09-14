#![cfg(target_os = "linux")]

use std::env;
use std::thread;
use std::time::{Duration, Instant};

use smolgnet::{
    GnetFrame, GnetFrameDevice, LinkTraffic, TapAddressing, TapDevice, Vcid,
};

fn receive_until(device: &mut TapDevice, timeout: Duration) -> GnetFrame {
    let deadline = Instant::now() + timeout;
    loop {
        match device.receive_frame() {
            Ok(Some(frame)) => return frame,
            Ok(None) => {}
            Err(err) => panic!("TAP receive failed: {err}"),
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for GNet TAP frame");
        }
        thread::sleep(Duration::from_millis(1));
    }
}

/// Real kernel TAP integration test.
///
/// The two TAP interfaces must already exist and be connected to the same
/// Linux bridge. `scripts/test-tap.sh` creates the topology and invokes this
/// test with the required environment variables.
#[test]
#[ignore = "requires two Linux TAP interfaces connected by a bridge; run scripts/test-tap.sh"]
fn real_linux_tap_roundtrip() {
    let tap_a = env::var("SMOLGNET_TAP_A")
        .expect("SMOLGNET_TAP_A must name the first prepared TAP interface");
    let tap_b = env::var("SMOLGNET_TAP_B")
        .expect("SMOLGNET_TAP_B must name the second prepared TAP interface");

    let addr_a = TapAddressing::new(
        [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
        [0x02, 0x00, 0x00, 0x00, 0x00, 0x02],
    );
    let addr_b = TapAddressing::new(addr_a.peer_mac, addr_a.local_mac);

    let mut a = TapDevice::open(&tap_a, addr_a).expect("open first TAP");
    let mut b = TapDevice::open(&tap_b, addr_b).expect("open second TAP");

    let data = GnetFrame {
        vcid: Vcid::VC2,
        traffic: LinkTraffic::Data,
        bytes: vec![0x47, 0x4e, 0x45, 0x54, 1, 2, 3, 4, 5, 6, 7],
    };
    a.transmit_frame(data.clone()).expect("A -> B transmit");
    assert_eq!(receive_until(&mut b, Duration::from_secs(2)), data);

    let control = GnetFrame {
        vcid: Vcid::CONTROL,
        traffic: LinkTraffic::Control,
        bytes: vec![0x43, 0x54, 0x4c, 0xaa, 0x55],
    };
    b.transmit_frame(control.clone()).expect("B -> A transmit");
    assert_eq!(receive_until(&mut a, Duration::from_secs(2)), control);
}
