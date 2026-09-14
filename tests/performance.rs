use std::time::Instant;

use smolgnet::*;
use smolgnet::wire::gts::Direction;

const ONE_MIB: usize = 1024 * 1024;
const MESSAGE_BYTES: usize = 8_000;

#[derive(Clone, Copy)]
struct XorShift64(u64);

impl XorShift64 {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn bytes(mut self, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        for chunk in out.chunks_mut(8) {
            let bytes = self.next_u64().to_be_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        out
    }
}

fn mib_per_second(bytes: usize, seconds: f64) -> f64 {
    (bytes as f64 / (1024.0 * 1024.0)) / seconds.max(f64::MIN_POSITIVE)
}

#[test]
fn transfer_one_mib_random_buffer_end_to_end() {
    let input = XorShift64(0x243f_6a88_85a3_08d3).bytes(ONE_MIB);

    let mut client_cfg = EndpointConfig::new(65_536);
    client_cfg.gts_receive_slots = 255;
    let mut server_cfg = EndpointConfig::new(65_536);
    server_cfg.gts_receive_slots = 255;

    let mut client = Endpoint::new(GdpAddress(0x1000_0000_0000_0001), client_cfg).unwrap();
    let mut server = Endpoint::new(GdpAddress(0x1000_0000_0000_0002), server_cfg).unwrap();
    let mut link = DirectLink::new();

    let css = ServiceSelector::registered(1).unwrap();
    server.listen(css, ListenerConfig { receive_slots: 255 });
    let profile = StreamProfile::reliable_variable(SizeClass::Jumbo8K, Direction::Bidirectional);
    let client_tunnel = client.connect(server.address(), css, profile).unwrap();
    link.pump(&mut client, &mut server, 0, 200_000).unwrap();
    let server_tunnel = server.accept().expect("server accepted tunnel");
    assert_eq!(client.tunnel_state(client_tunnel).unwrap(), TunnelState::Established);

    let start = Instant::now();

    for chunk in input.chunks(MESSAGE_BYTES) {
        client.send(client_tunnel, 0, chunk, 1).unwrap();
    }

    let flits = link
        .pump(&mut client, &mut server, 1, 2_000_000)
        .unwrap();

    let mut output = Vec::with_capacity(ONE_MIB);
    while output.len() < ONE_MIB {
        let message = server
            .recv(server_tunnel, 0)
            .unwrap()
            .expect("all queued GTS messages were delivered");
        output.extend_from_slice(&message);
    }

    let elapsed = start.elapsed();
    assert_eq!(output, input);
    assert_eq!(output.len(), ONE_MIB);

    let seconds = elapsed.as_secs_f64();
    eprintln!(
        "smolgnet 1 MiB direct-link: {:.2} MiB/s, {:.3} ms, {} physical flits",
        mib_per_second(ONE_MIB, seconds),
        seconds * 1000.0,
        flits
    );
}
