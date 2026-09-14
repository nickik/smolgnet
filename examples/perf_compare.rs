use std::time::{Duration, Instant};

use smolgnet::*;
use smolgnet::wire::gts::Direction;
use smoltcp::iface::{Config as SmolConfig, Interface as SmolInterface, SocketSet};
use smoltcp::phy::{Loopback, Medium};
use smoltcp::socket::tcp;
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr};

const DEFAULT_BYTES: usize = 1024 * 1024;
const GNET_MESSAGE_BYTES: usize = 8_000;

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

fn throughput_mib_s(bytes: usize, elapsed: Duration) -> f64 {
    (bytes as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64().max(f64::MIN_POSITIVE)
}

fn throughput_gbit_s(bytes: usize, elapsed: Duration) -> f64 {
    (bytes as f64 * 8.0 / 1_000_000_000.0) / elapsed.as_secs_f64().max(f64::MIN_POSITIVE)
}

fn run_smolgnet(data: &[u8]) -> Duration {
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

    let start = Instant::now();
    let client_tunnel = client.connect(server.address(), css, profile).unwrap();
    link.pump(&mut client, &mut server, 0, 250_000).unwrap();
    let server_tunnel = server.accept().expect("GNet server accepted tunnel");

    for chunk in data.chunks(GNET_MESSAGE_BYTES) {
        client.send(client_tunnel, 0, chunk, 1).unwrap();
    }
    link.pump(&mut client, &mut server, 1, data.len() * 2 + 500_000)
        .unwrap();

    let mut received = Vec::with_capacity(data.len());
    while received.len() < data.len() {
        let message = server
            .recv(server_tunnel, 0)
            .unwrap()
            .expect("GNet message available");
        received.extend_from_slice(&message);
    }
    assert_eq!(received, data);
    start.elapsed()
}

fn run_smoltcp(data: &[u8]) -> Duration {
    // This follows smoltcp's own examples/loopback_benchmark.rs structure:
    // one Interface, a Loopback Ethernet device, and two TCP sockets.
    let mut device = Loopback::new(Medium::Ethernet);
    let config = SmolConfig::new(EthernetAddress([0x02, 0, 0, 0, 0, 1]).into());
    let mut iface = SmolInterface::new(config, &mut device, SmolInstant::now());
    iface.update_ip_addrs(|addrs| {
        addrs
            .push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8))
            .unwrap();
    });

    let server_socket = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0u8; 65_536]),
        tcp::SocketBuffer::new(vec![0u8; 65_536]),
    );
    let client_socket = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0u8; 65_536]),
        tcp::SocketBuffer::new(vec![0u8; 65_536]),
    );

    let mut storage: [_; 2] = Default::default();
    let mut sockets = SocketSet::new(&mut storage[..]);
    let server_handle = sockets.add(server_socket);
    let client_handle = sockets.add(client_socket);

    let mut listened = false;
    let mut connected = false;
    let mut sent = 0usize;
    let mut received = 0usize;
    let mut iterations = 0usize;
    let start = Instant::now();

    while received < data.len() {
        iterations += 1;
        assert!(iterations < 20_000_000, "smoltcp loopback benchmark stalled");
        iface.poll(SmolInstant::now(), &mut device, &mut sockets);

        {
            let socket = sockets.get_mut::<tcp::Socket>(server_handle);
            if !socket.is_active() && !socket.is_listening() && !listened {
                socket.listen(1234).unwrap();
                listened = true;
            }
            while socket.can_recv() && received < data.len() {
                let consumed = socket
                    .recv(|buffer| {
                        let n = buffer.len().min(data.len() - received);
                        assert_eq!(&buffer[..n], &data[received..received + n]);
                        (n, n)
                    })
                    .unwrap();
                received += consumed;
            }
        }

        {
            let socket = sockets.get_mut::<tcp::Socket>(client_handle);
            if !socket.is_open() && !connected {
                let cx = iface.context();
                socket
                    .connect(cx, (IpAddress::v4(127, 0, 0, 1), 1234), 65_000)
                    .unwrap();
                connected = true;
            }
            while socket.can_send() && sent < data.len() {
                let written = socket
                    .send(|buffer| {
                        let n = buffer.len().min(data.len() - sent);
                        buffer[..n].copy_from_slice(&data[sent..sent + n]);
                        (n, n)
                    })
                    .unwrap();
                if written == 0 {
                    break;
                }
                sent += written;
            }
        }
    }

    assert_eq!(sent, data.len());
    start.elapsed()
}

fn main() {
    let bytes = std::env::var("SMOLGNET_BENCH_BYTES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_BYTES);
    let data = XorShift64(0x243f_6a88_85a3_08d3).bytes(bytes);

    // One warmup to reduce first-use effects in allocator/code pages.
    let _ = run_smolgnet(&data[..data.len().min(64 * 1024)]);
    let _ = run_smoltcp(&data[..data.len().min(64 * 1024)]);

    let gnet = run_smolgnet(&data);
    let tcp = run_smoltcp(&data);

    println!("payload: {} bytes ({:.3} MiB)", bytes, bytes as f64 / 1_048_576.0);
    println!(
        "smolgnet/GTS : {:9.2} MiB/s  {:7.3} Gbit/s  {:9.3} ms",
        throughput_mib_s(bytes, gnet),
        throughput_gbit_s(bytes, gnet),
        gnet.as_secs_f64() * 1000.0
    );
    println!(
        "smoltcp/TCP  : {:9.2} MiB/s  {:7.3} Gbit/s  {:9.3} ms",
        throughput_mib_s(bytes, tcp),
        throughput_gbit_s(bytes, tcp),
        tcp.as_secs_f64() * 1000.0
    );
    println!(
        "relative throughput (smolgnet / smoltcp): {:.3}x",
        throughput_mib_s(bytes, gnet) / throughput_mib_s(bytes, tcp)
    );
}
