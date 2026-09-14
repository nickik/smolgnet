use p4_gdp::{GdpPipeline, RouteDisposition};
use smolgnet::{
    Direction, Endpoint, EndpointConfig, Error, GdpAddress, GnetFrame, LinkTraffic,
    ListenerConfig, ServiceSelector, SizeClass, StreamProfile,
};

#[derive(Debug, Clone, Copy, Default)]
struct LinkSetup {
    attached: bool,
}

impl LinkSetup {
    fn attach(&mut self, a: &mut Endpoint, b: &mut Endpoint) {
        if self.attached {
            return;
        }
        let a_control = a.dlp().control_window_flits();
        let b_control = b.dlp().control_window_flits();
        let a_peer = a.link_local_address();
        let b_peer = b.link_local_address();
        a.link_attached(b_peer, b_control).unwrap();
        b.link_attached(a_peer, a_control).unwrap();
        self.attached = true;
    }
}

fn forward_one(
    pipeline: &mut GdpPipeline,
    ingress_port: u16,
    expected_egress: u16,
    mut frame: GnetFrame,
) -> GnetFrame {
    let output = pipeline.process(ingress_port, &frame.bytes).unwrap();
    assert_eq!(output.len(), 1, "P4 dataplane unexpectedly dropped frame");
    assert_eq!(output[0].port, expected_egress);
    frame.bytes = output[0].bytes.clone();
    frame
}

fn pump_p4(
    setup_link: &mut LinkSetup,
    pipeline: &mut GdpPipeline,
    a: &mut Endpoint,
    b: &mut Endpoint,
    now: u64,
    max_flits: usize,
) -> usize {
    setup_link.attach(a, b);
    let mut moved = 0usize;

    loop {
        let mut progress = false;

        match a.poll_tx_frame() {
            Ok(Some(frame)) => {
                let flits = frame.flit_len();
                let control = frame.traffic == LinkTraffic::Control;
                let frame = forward_one(pipeline, 0, 1, frame);
                b.receive_frame(frame, now).unwrap();
                if control {
                    a.dlp_mut().grant_control_tx_credit(flits as u32);
                }
                moved += flits;
                progress = true;
            }
            Ok(None) | Err(Error::NoCredit) => {}
            Err(e) => panic!("endpoint A transmit failed: {e:?}"),
        }

        match b.poll_tx_frame() {
            Ok(Some(frame)) => {
                let flits = frame.flit_len();
                let control = frame.traffic == LinkTraffic::Control;
                let frame = forward_one(pipeline, 1, 0, frame);
                a.receive_frame(frame, now).unwrap();
                if control {
                    b.dlp_mut().grant_control_tx_credit(flits as u32);
                }
                moved += flits;
                progress = true;
            }
            Ok(None) | Err(Error::NoCredit) => {}
            Err(e) => panic!("endpoint B transmit failed: {e:?}"),
        }

        assert!(moved < max_flits, "P4/smolgnet pump exceeded safety limit");
        if !progress {
            break;
        }
    }

    moved
}

fn add_endpoint_global_routes(pipeline: &mut GdpPipeline, a: &Endpoint, b: &Endpoint) {
    pipeline.add_global_route(a.address().0, 0, RouteDisposition::Forward);
    pipeline.add_global_route(b.address().0, 1, RouteDisposition::Forward);
    pipeline.add_global_route(a.link_local_address().0, 0, RouteDisposition::Forward);
    pipeline.add_global_route(b.link_local_address().0, 1, RouteDisposition::Forward);
}

#[test]
fn smolgnet_gctl_and_gts_run_through_p4_forwarding() {
    let cfg = EndpointConfig::new(512);
    let mut a = Endpoint::new(GdpAddress(0x0102_0304_0000_0001), cfg).unwrap();
    let mut b = Endpoint::new(GdpAddress(0x0102_0304_0000_0002), cfg).unwrap();

    let mut pipeline = GdpPipeline::new(2);
    add_endpoint_global_routes(&mut pipeline, &a, &b);
    let mut setup_link = LinkSetup::default();

    // Link credit bootstrapping itself traverses P4.
    pump_p4(&mut setup_link, &mut pipeline, &mut a, &mut b, 0, 100_000);
    assert!(a.dlp().data_tx_credit() > 0);
    assert!(b.dlp().data_tx_credit() > 0);

    // GCTL request/reply traverses the P4 forwarding path in both directions.
    a.send_echo(b.address(), 0x1234, b"p4-gctl", SizeClass::Ctrl32)
        .unwrap();
    pump_p4(&mut setup_link, &mut pipeline, &mut a, &mut b, 1, 100_000);
    let reply = a.take_echo_reply(0x1234).unwrap();
    assert_eq!(&reply[..7], b"p4-gctl");

    // A real smolgnet GTS connection and reliable message uses the same P4
    // dataplane without P4 knowing anything about GTS connection state.
    let service = ServiceSelector::registered(1).unwrap();
    b.listen(service, ListenerConfig::default());
    let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let ah = a.connect(b.address(), service, profile).unwrap();
    pump_p4(&mut setup_link, &mut pipeline, &mut a, &mut b, 2, 100_000);
    let bh = b.accept().unwrap();

    a.send(ah, 0, b"smolgnet over p4", 3).unwrap();
    pump_p4(&mut setup_link, &mut pipeline, &mut a, &mut b, 3, 100_000);
    assert_eq!(b.recv(bh, 0).unwrap(), Some(b"smolgnet over p4".to_vec()));
}

#[test]
fn smolgnet_local_gdp_runs_through_p4_local_route_table() {
    let prefix = 0x1234_5678_9abc_0000;
    let mut cfg = EndpointConfig::new(512);
    cfg.local_context_prefix = Some(prefix);
    cfg.prefer_local_gdp = true;

    let mut a = Endpoint::new(GdpAddress(prefix | 1), cfg).unwrap();
    let mut b = Endpoint::new(GdpAddress(prefix | 2), cfg).unwrap();

    let mut pipeline = GdpPipeline::new(2);
    // Link-local GCTL bootstrapping remains global-form.
    pipeline.add_global_route(a.link_local_address().0, 0, RouteDisposition::Forward);
    pipeline.add_global_route(b.link_local_address().0, 1, RouteDisposition::Forward);
    // Traffic inside the configured GDP context uses local-form addresses.
    pipeline.add_local_route(1, 0, RouteDisposition::Forward);
    pipeline.add_local_route(2, 1, RouteDisposition::Forward);

    let mut setup_link = LinkSetup::default();
    pump_p4(&mut setup_link, &mut pipeline, &mut a, &mut b, 0, 100_000);

    a.send_echo(b.address(), 0x55, b"local-p4", SizeClass::Ctrl32)
        .unwrap();
    pump_p4(&mut setup_link, &mut pipeline, &mut a, &mut b, 1, 100_000);

    assert!(a.take_echo_reply(0x55).is_some());
    assert_eq!(b.last_rx_address_form(), Some(smolgnet::AddressForm::Local));
}
