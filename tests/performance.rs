use std::time::{Duration, Instant as StdInstant};

use smolgnet::*;
use smoltcp::iface::{Config as SmolConfig, Interface as SmolInterface, SocketSet};
use smoltcp::phy::{Device, Loopback, Medium};
use smoltcp::socket::tcp;
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr};

const ONE_MIB: usize = 1024 * 1024;

fn random_bytes(len: usize, mut state: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push(state as u8);
    }
    out
}

#[derive(Debug)]
struct ResultRow {
    elapsed: Duration,
    bytes: usize,
    logical_units: usize,
}

impl ResultRow {
    fn mib_per_sec(&self) -> f64 {
        self.bytes as f64 / (1024.0 * 1024.0) / self.elapsed.as_secs_f64()
    }

    fn gbps(&self) -> f64 {
        self.bytes as f64 * 8.0 / self.elapsed.as_secs_f64() / 1_000_000_000.0
    }
}

fn run_smolgnet(payload: &[u8]) -> ResultRow {
    let mut cfg = EndpointConfig::new(400_000);
    cfg.gts_receive_slots = 255;
    cfg.credit_update_threshold = 64;

    let mut client = Endpoint::new(GdpAddress(0x1234_5678_0000_0001), cfg).unwrap();
    let mut server = Endpoint::new(GdpAddress(0x1234_5678_0000_0002), cfg).unwrap();
    let css = ServiceSelector::registered(1).unwrap();
    server.listen(css, ListenerConfig { receive_slots: 255 });
    let profile = StreamProfile::reliable_variable(SizeClass::Bulk1280, Direction::Bidirectional);
    let client_tunnel = client.connect(server.address(), css, profile).unwrap();
    let mut link = DirectLink::new();
    link.pump(&mut client, &mut server, 0, 2_000_000).unwrap();
    let server_tunnel = server.accept().unwrap();

    let mut sent = 0usize;
    let mut output = Vec::with_capacity(payload.len());
    let mut flits = 0usize;
    let mut now = 1u64;
    let start = StdInstant::now();

    while output.len() < payload.len() {
        while sent < payload.len() {
            let n = (payload.len() - sent).min(1200);
            match client.send(client_tunnel, 0, &payload[sent..sent + n], now) {
                Ok(()) => sent += n,
                Err(Error::WouldBlock) => break,
                Err(e) => panic!("smolgnet send failed: {e:?}"),
            }
        }

        flits += link
            .pump(&mut client, &mut server, now, 8_000_000)
            .unwrap();
        while let Some(msg) = server.recv(server_tunnel, 0).unwrap() {
            output.extend_from_slice(&msg);
        }
        flits += link
            .pump(&mut client, &mut server, now, 8_000_000)
            .unwrap();
        now += 1;
    }

    let elapsed = start.elapsed();
    assert_eq!(output, payload);
    ResultRow {
        elapsed,
        bytes: payload.len(),
        logical_units: flits,
    }
}

fn run_smoltcp(payload: &[u8]) -> ResultRow {
    let mut device = Loopback::new(Medium::Ethernet);
    let config = SmolConfig::new(EthernetAddress([0x02, 0, 0, 0, 0, 1]).into());
    let mut iface = SmolInterface::new(config, &mut device, SmolInstant::now());
    iface.update_ip_addrs(|addrs| {
        addrs
            .push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8))
            .unwrap();
    });

    let server = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; 65_536]),
        tcp::SocketBuffer::new(vec![0; 65_536]),
    );
    let client = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; 65_536]),
        tcp::SocketBuffer::new(vec![0; 65_536]),
    );
    let mut sockets = SocketSet::new(vec![]);
    let server_handle = sockets.add(server);
    let client_handle = sockets.add(client);

    sockets
        .get_mut::<tcp::Socket>(server_handle)
        .listen(1234)
        .unwrap();
    {
        let cx = iface.context();
        sockets
            .get_mut::<tcp::Socket>(client_handle)
            .connect(cx, (IpAddress::v4(127, 0, 0, 1), 1234), 65_000)
            .unwrap();
    }

    // Complete the handshake before measuring payload transfer.
    for _ in 0..10_000 {
        iface.poll(SmolInstant::now(), &mut device, &mut sockets);
        if sockets.get::<tcp::Socket>(client_handle).can_send()
            && sockets.get::<tcp::Socket>(server_handle).is_active()
        {
            break;
        }
    }
    assert!(sockets.get::<tcp::Socket>(client_handle).can_send());

    let mut sent = 0usize;
    let mut output = Vec::with_capacity(payload.len());
    let mut recv_buf = vec![0u8; 65_536];
    let mut polls = 0usize;
    let start = StdInstant::now();

    while output.len() < payload.len() {
        polls += 1;
        iface.poll(SmolInstant::now(), &mut device, &mut sockets);

        {
            let socket = sockets.get_mut::<tcp::Socket>(client_handle);
            while socket.can_send() && sent < payload.len() {
                let n = socket.send_slice(&payload[sent..]).unwrap();
                if n == 0 {
                    break;
                }
                sent += n;
            }
        }

        iface.poll(SmolInstant::now(), &mut device, &mut sockets);

        {
            let socket = sockets.get_mut::<tcp::Socket>(server_handle);
            while socket.can_recv() {
                let n = socket.recv_slice(&mut recv_buf).unwrap();
                if n == 0 {
                    break;
                }
                output.extend_from_slice(&recv_buf[..n]);
            }
        }

        assert!(polls < 1_000_000, "smoltcp loopback benchmark made no progress");
    }

    let elapsed = start.elapsed();
    assert_eq!(output, payload);
    ResultRow {
        elapsed,
        bytes: payload.len(),
        logical_units: polls,
    }
}

#[test]
#[ignore = "release-mode microbenchmark; run with cargo test --release --test performance -- --ignored --nocapture"]
fn compare_one_mib_smolgnet_and_smoltcp_loopback() {
    let payload = random_bytes(ONE_MIB, 0x5eed_cafe_1234_5678);

    // Warm up both implementations once so allocator/code-page startup has
    // less influence on the measured pass.
    let warm = &payload[..64 * 1024];
    let _ = run_smolgnet(warm);
    let _ = run_smoltcp(warm);

    let gnet = run_smolgnet(&payload);
    let tcp = run_smoltcp(&payload);

    println!("1 MiB in-memory established-stream transfer");
    println!(
        "smolgnet GTS/GDP/DLP: {:8.2} MiB/s  {:6.3} Gbps  {:?}  {} flits",
        gnet.mib_per_sec(),
        gnet.gbps(),
        gnet.elapsed,
        gnet.logical_units
    );
    println!(
        "smoltcp TCP loopback: {:8.2} MiB/s  {:6.3} Gbps  {:?}  {} poll rounds",
        tcp.mib_per_sec(),
        tcp.gbps(),
        tcp.elapsed,
        tcp.logical_units
    );
    println!(
        "smolgnet/smoltcp throughput ratio: {:.3}",
        gnet.mib_per_sec() / tcp.mib_per_sec()
    );

    // Performance varies by runner, so correctness is the hard assertion.
    // The printed ratio is tracked as a benchmark observation, not a CI gate.
    assert_eq!(gnet.bytes, ONE_MIB);
    assert_eq!(tcp.bytes, ONE_MIB);
}
