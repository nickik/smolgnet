#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

#[cfg(target_os = "linux")]
mod linux_tap {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::fd::AsRawFd;
    use std::thread;
    use std::time::{Duration, Instant};

    use smolgnet::*;
    use smolgnet::wire::gts::Direction;

    const ONE_MIB: usize = 1024 * 1024;
    const MESSAGE_BYTES: usize = 8_000;
    const MAX_FLITS_PER_FRAME: usize = 256;
    const ETH_HEADER: usize = 14;
    const PAYLOAD_HEADER: usize = 2;
    const FLIT_BYTES: usize = 5;
    const ETHERTYPE_GNET_TEST: u16 = 0x88b5;

    const TUNSETIFF: libc::c_ulong = 0x4004_54ca;
    const IFF_TAP: libc::c_short = 0x0002;
    const IFF_NO_PI: libc::c_short = 0x1000;

    #[repr(C)]
    struct IfReq {
        name: [libc::c_char; libc::IFNAMSIZ],
        data: [u8; 24],
    }

    fn open_tap(name: &str) -> io::Result<File> {
        if name.len() >= libc::IFNAMSIZ {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "TAP name too long"));
        }
        let file = OpenOptions::new().read(true).write(true).open("/dev/net/tun")?;
        let mut ifr = IfReq {
            name: [0; libc::IFNAMSIZ],
            data: [0; 24],
        };
        for (dst, src) in ifr.name.iter_mut().zip(name.as_bytes()) {
            *dst = *src as libc::c_char;
        }
        let flags = (IFF_TAP | IFF_NO_PI).to_ne_bytes();
        ifr.data[..2].copy_from_slice(&flags);

        let rc = unsafe { libc::ioctl(file.as_raw_fd(), TUNSETIFF, &ifr) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }

        let old = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        if old < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, old | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(file)
    }

    fn encode_frame(flits: &[Flit], source_mac: [u8; 6]) -> Vec<u8> {
        assert!(!flits.is_empty());
        assert!(flits.len() <= MAX_FLITS_PER_FRAME);
        let used = ETH_HEADER + PAYLOAD_HEADER + flits.len() * FLIT_BYTES;
        let mut out = vec![0u8; used.max(60)];
        out[0..6].fill(0xff);
        out[6..12].copy_from_slice(&source_mac);
        out[12..14].copy_from_slice(&ETHERTYPE_GNET_TEST.to_be_bytes());
        out[14..16].copy_from_slice(&(flits.len() as u16).to_be_bytes());
        let mut pos = 16;
        for flit in flits {
            out[pos] = flit.vcid.get();
            out[pos + 1..pos + 5].copy_from_slice(&flit.data.to_be_bytes());
            pos += 5;
        }
        out
    }

    fn decode_frame(frame: &[u8]) -> io::Result<Vec<Flit>> {
        if frame.len() < 16 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "short Ethernet frame"));
        }
        if u16::from_be_bytes([frame[12], frame[13]]) != ETHERTYPE_GNET_TEST {
            return Ok(Vec::new());
        }
        let count = u16::from_be_bytes([frame[14], frame[15]]) as usize;
        if count > MAX_FLITS_PER_FRAME || 16 + count * FLIT_BYTES > frame.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid GNet TAP batch"));
        }
        let mut flits = Vec::with_capacity(count);
        let mut pos = 16;
        for _ in 0..count {
            let vcid = Vcid::new(frame[pos])
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid VCID"))?;
            let data = u32::from_be_bytes(frame[pos + 1..pos + 5].try_into().unwrap());
            flits.push(Flit { vcid, data });
            pos += 5;
        }
        Ok(flits)
    }

    fn tx_batch(file: &mut File, endpoint: &mut Endpoint, source_mac: [u8; 6]) -> Result<usize> {
        let mut flits = Vec::with_capacity(MAX_FLITS_PER_FRAME);
        while flits.len() < MAX_FLITS_PER_FRAME {
            match endpoint.poll_tx_flit() {
                Ok(Some(f)) => flits.push(f),
                Ok(None) | Err(Error::NoCredit) => break,
                Err(e) => return Err(e),
            }
        }
        if flits.is_empty() {
            return Ok(0);
        }
        let frame = encode_frame(&flits, source_mac);
        match file.write(&frame) {
            Ok(n) if n == frame.len() => Ok(flits.len()),
            Ok(_) => Err(Error::BufferFull),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(Error::WouldBlock),
            Err(_) => Err(Error::LinkDown),
        }
    }

    fn rx_batches(
        file: &mut File,
        sender: &mut Endpoint,
        receiver: &mut Endpoint,
        now: u64,
    ) -> Result<usize> {
        let mut total = 0;
        let mut buf = [0u8; 2048];
        loop {
            let n = match file.read(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => return Err(Error::LinkDown),
            };
            if n == 0 {
                break;
            }
            let flits = decode_frame(&buf[..n]).map_err(|_| Error::InvalidField)?;
            for flit in flits {
                let control = flit.vcid.is_control();
                receiver.receive_flit(flit, now)?;
                if control {
                    sender.dlp_mut().grant_control_tx_credit(1);
                }
                total += 1;
            }
        }
        Ok(total)
    }

    fn drive_step(
        tap_a: &mut File,
        tap_b: &mut File,
        a: &mut Endpoint,
        b: &mut Endpoint,
        now: u64,
    ) -> Result<usize> {
        let mut progress = 0;
        match tx_batch(tap_a, a, [0x02, 0, 0, 0, 0, 1]) {
            Ok(n) => progress += n,
            Err(Error::WouldBlock) => {}
            Err(e) => return Err(e),
        }
        match tx_batch(tap_b, b, [0x02, 0, 0, 0, 0, 2]) {
            Ok(n) => progress += n,
            Err(Error::WouldBlock) => {}
            Err(e) => return Err(e),
        }
        // Frames written to tap A cross the Linux bridge and are read from B,
        // and vice versa.
        progress += rx_batches(tap_b, a, b, now)?;
        progress += rx_batches(tap_a, b, a, now)?;
        Ok(progress)
    }

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
                let x = self.next_u64().to_be_bytes();
                chunk.copy_from_slice(&x[..chunk.len()]);
            }
            out
        }
    }

    #[test]
    #[ignore = "requires two Linux TAP interfaces connected by a bridge; run tools/setup_tap_pair.sh"]
    fn transfer_one_mib_through_linux_tap_pair() {
        let tap_a_name = std::env::var("SMOLGNET_TAP_A").unwrap_or_else(|_| "gnettap0".into());
        let tap_b_name = std::env::var("SMOLGNET_TAP_B").unwrap_or_else(|_| "gnettap1".into());
        let mut tap_a = open_tap(&tap_a_name).expect("open first TAP");
        let mut tap_b = open_tap(&tap_b_name).expect("open second TAP");

        let mut cfg_a = EndpointConfig::new(65_536);
        cfg_a.gts_receive_slots = 255;
        let mut cfg_b = EndpointConfig::new(65_536);
        cfg_b.gts_receive_slots = 255;
        let mut a = Endpoint::new(GdpAddress(0x3000_0000_0000_0001), cfg_a).unwrap();
        let mut b = Endpoint::new(GdpAddress(0x3000_0000_0000_0002), cfg_b).unwrap();

        // Attach establishes only the direct-link control window and queues the
        // initial GCTL CREDIT messages. The actual messages/flits below cross
        // the kernel TAP/bridge path.
        let mut attach = DirectLink::new();
        attach.attach(&mut a, &mut b).unwrap();

        let css = ServiceSelector::registered(1).unwrap();
        b.listen(css, ListenerConfig { receive_slots: 255 });
        let profile = StreamProfile::reliable_variable(SizeClass::Jumbo8K, Direction::Bidirectional);
        let ah = a.connect(b.address(), css, profile).unwrap();

        let mut bh = None;
        for turn in 0..100_000u64 {
            let progress = drive_step(&mut tap_a, &mut tap_b, &mut a, &mut b, turn).unwrap();
            if bh.is_none() {
                bh = b.accept();
            }
            if bh.is_some() && a.tunnel_state(ah).unwrap() == TunnelState::Established {
                break;
            }
            if progress == 0 {
                thread::sleep(Duration::from_micros(50));
            }
        }
        let bh = bh.expect("GTS tunnel established over TAP pair");

        let input = XorShift64(0x1319_8a2e_0370_7344).bytes(ONE_MIB);
        for chunk in input.chunks(MESSAGE_BYTES) {
            a.send(ah, 0, chunk, 1).unwrap();
        }

        let start = Instant::now();
        let mut output = Vec::with_capacity(ONE_MIB);
        let mut idle = 0usize;
        let mut turn = 1u64;
        while output.len() < input.len() {
            let progress = drive_step(&mut tap_a, &mut tap_b, &mut a, &mut b, turn).unwrap();
            while let Some(message) = b.recv(bh, 0).unwrap() {
                output.extend_from_slice(&message);
            }
            if progress == 0 {
                idle += 1;
                if idle > 200_000 {
                    panic!("TAP benchmark stalled");
                }
                thread::sleep(Duration::from_micros(25));
            } else {
                idle = 0;
            }
            turn += 1;
        }
        let elapsed = start.elapsed();
        assert_eq!(output, input);
        eprintln!(
            "smolgnet Linux TAP 1 MiB: {:.2} MiB/s ({:.3} ms)",
            (ONE_MIB as f64 / 1_048_576.0) / elapsed.as_secs_f64(),
            elapsed.as_secs_f64() * 1000.0
        );
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
#[ignore = "Linux TAP benchmark"]
fn transfer_one_mib_through_linux_tap_pair() {
    eprintln!("Linux-only TAP benchmark");
}
