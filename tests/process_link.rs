#![cfg(unix)]

#[path = "support/direct.rs"]
mod direct_support;
#[path = "support/seqpacket.rs"]
mod seqpacket_support;

use std::env;
use std::os::fd::RawFd;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration as StdDuration;

use direct_support::DirectLink;
use seqpacket_support::SeqPacketNic;
use smolgnet::*;

const CLIENT_ADDRESS: GdpAddress = GdpAddress(0x1234_0000_0000_0011);
const SERVER_ADDRESS: GdpAddress = GdpAddress(0x1234_0000_0000_0022);
const CHILD_ENV: &str = "SMOLGNET_SEQPACKET_CHILD";
const FD_ENV: &str = "SMOLGNET_SEQPACKET_FD";

fn endpoint_config() -> EndpointConfig {
    EndpointConfig::new(4096)
}

fn service_css() -> ServiceSelector {
    ServiceSelector::registered(1).unwrap()
}

fn profile() -> StreamProfile {
    StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional)
}

fn attached_client() -> Endpoint {
    let mut client = Endpoint::new(CLIENT_ADDRESS, endpoint_config()).unwrap();
    let mut dummy_server = Endpoint::new(SERVER_ADDRESS, endpoint_config()).unwrap();
    DirectLink::new()
        .attach(&mut client, &mut dummy_server)
        .unwrap();
    client
}

fn attached_server() -> Endpoint {
    let mut dummy_client = Endpoint::new(CLIENT_ADDRESS, endpoint_config()).unwrap();
    let mut server = Endpoint::new(SERVER_ADDRESS, endpoint_config()).unwrap();
    server.listen(service_css(), ListenerConfig::default());
    DirectLink::new()
        .attach(&mut dummy_client, &mut server)
        .unwrap();
    server
}

fn child_main(fd: RawFd) {
    let mut nic = unsafe { SeqPacketNic::from_inherited_fd(fd) };
    let mut server = attached_server();
    let mut accepted = None;
    let mut reply_queued_at = None;

    for now in 0..10_000u64 {
        nic.drive_endpoint(&mut server, now).unwrap();
        server.tick(now).unwrap();

        if accepted.is_none() {
            accepted = server.accept();
        }

        if let Some(tunnel) = accepted {
            if reply_queued_at.is_none() {
                if let Some(message) = server.recv(tunnel, 0).unwrap() {
                    assert_eq!(message, b"hello-from-parent");
                    server
                        .send(tunnel, 0, b"hello-from-child", now)
                        .unwrap();
                    reply_queued_at = Some(now);
                }
            }
        }

        nic.drive_endpoint(&mut server, now).unwrap();

        if let Some(sent_at) = reply_queued_at {
            if server.dlp().queued_data_flits() == 0 && now.saturating_sub(sent_at) >= 100 {
                return;
            }
        }

        thread::sleep(StdDuration::from_millis(1));
    }

    panic!("child GNet endpoint did not complete the exchange");
}

#[test]
fn seqpacket_child_process() {
    if env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let fd: RawFd = env::var(FD_ENV)
        .expect("missing inherited seqpacket fd")
        .parse()
        .expect("invalid inherited seqpacket fd");
    child_main(fd);
}

#[test]
fn two_processes_exchange_reliable_gts_over_native_seqpacket_nics() {
    let (mut parent_nic, child_nic) = SeqPacketNic::pair().unwrap();
    child_nic.set_inheritable(true).unwrap();
    let child_fd = child_nic.raw_fd();

    let exe = env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .arg("--exact")
        .arg("seqpacket_child_process")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(CHILD_ENV, "1")
        .env(FD_ENV, child_fd.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn GNet peer process");

    drop(child_nic);

    let mut client = attached_client();
    let tunnel = client
        .connect(SERVER_ADDRESS, service_css(), profile())
        .unwrap();
    let mut request_sent = false;
    let mut reply = None;

    for now in 0..10_000u64 {
        parent_nic.drive_endpoint(&mut client, now).unwrap();
        client.tick(now).unwrap();

        if !request_sent
            && matches!(client.tunnel_state(tunnel), Ok(TunnelState::Established))
        {
            client
                .send(tunnel, 0, b"hello-from-parent", now)
                .unwrap();
            request_sent = true;
        }

        parent_nic.drive_endpoint(&mut client, now).unwrap();

        if request_sent {
            if let Some(message) = client.recv(tunnel, 0).unwrap() {
                reply = Some(message);
                break;
            }
        }

        thread::sleep(StdDuration::from_millis(1));
    }

    assert_eq!(reply.as_deref(), Some(&b"hello-from-child"[..]));
    let status = child.wait().expect("wait for GNet peer process");
    assert!(status.success(), "GNet peer process failed: {status}");
}
