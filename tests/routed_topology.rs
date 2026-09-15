use std::collections::BTreeMap;

use smolgnet::{
    Direction, Endpoint, EndpointConfig, Error, GdpAddress, GdpHeader, GdpPacket, GdpPrefix,
    GdpType, GdpWireConfig, GnetFrame, Instant, LinkTraffic, ListenerConfig, Result, Route,
    RouteTable, ServiceSelector, SizeClass, StreamProfile, TunnelState, Vcid,
};

const A: GdpAddress = GdpAddress(0x1000_0000_0000_0001);
const B: GdpAddress = GdpAddress(0x2000_0000_0000_0001);
const R1: GdpAddress = GdpAddress(0xf000_0000_0000_0001);
const R2: GdpAddress = GdpAddress(0xf000_0000_0000_0002);
const R3: GdpAddress = GdpAddress(0xf000_0000_0000_0003);
const R4: GdpAddress = GdpAddress(0xf000_0000_0000_0004);

fn a_prefix() -> GdpPrefix {
    GdpPrefix::new(GdpAddress(0x1000_0000_0000_0000), 16).unwrap()
}

fn b_prefix() -> GdpPrefix {
    GdpPrefix::new(GdpAddress(0x2000_0000_0000_0000), 16).unwrap()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropReason {
    NoRoute,
    HopLimit,
    LinkDown,
    UnknownNextHop,
}

#[derive(Debug, Clone)]
struct TestRouter {
    address: GdpAddress,
    routes: RouteTable<8>,
    links: BTreeMap<GdpAddress, bool>,
}

impl TestRouter {
    fn new(address: GdpAddress) -> Self {
        Self {
            address,
            routes: RouteTable::new(),
            links: BTreeMap::new(),
        }
    }

    fn connect(&mut self, neighbor: GdpAddress) {
        self.links.insert(neighbor, true);
    }

    fn set_link(&mut self, neighbor: GdpAddress, up: bool) {
        self.links.insert(neighbor, up);
    }

    fn add_route(&mut self, prefix: GdpPrefix, next_hop: GdpAddress) {
        self.routes.add(Route::new(prefix, next_hop)).unwrap();
    }

    fn add_default(&mut self, next_hop: GdpAddress) {
        self.routes.add_default_route(next_hop).unwrap();
    }

    fn forward(
        &self,
        mut frame: GnetFrame,
        now: Instant,
    ) -> core::result::Result<(GdpAddress, GnetFrame), DropReason> {
        let mut packet = GdpPacket::decode(&frame.bytes, GdpWireConfig::default(), 0)
            .map_err(|_| DropReason::UnknownNextHop)?;

        if packet.header.hop_limit <= 1 {
            return Err(DropReason::HopLimit);
        }

        let destination = packet.header.destination();
        let next_hop = self
            .routes
            .lookup(destination, now)
            .ok_or(DropReason::NoRoute)?;

        if !self.links.get(&next_hop).copied().unwrap_or(false) {
            return Err(DropReason::LinkDown);
        }

        packet.header.hop_limit -= 1;
        frame.bytes = packet
            .encode(GdpWireConfig::default())
            .map_err(|_| DropReason::UnknownNextHop)?;
        Ok((next_hop, frame))
    }
}

#[derive(Debug, Clone)]
struct TestNetwork {
    routers: BTreeMap<GdpAddress, TestRouter>,
}

impl TestNetwork {
    fn new(routers: impl IntoIterator<Item = TestRouter>) -> Self {
        Self {
            routers: routers.into_iter().map(|r| (r.address, r)).collect(),
        }
    }

    fn router_mut(&mut self, address: GdpAddress) -> &mut TestRouter {
        self.routers.get_mut(&address).unwrap()
    }

    fn route_frame(
        &self,
        first_router: GdpAddress,
        frame: GnetFrame,
        now: Instant,
    ) -> core::result::Result<(GdpAddress, GnetFrame, Vec<GdpAddress>), DropReason> {
        let mut current = first_router;
        let mut frame = frame;
        let mut path = Vec::new();

        for _ in 0..16 {
            path.push(current);
            let router = self.routers.get(&current).ok_or(DropReason::UnknownNextHop)?;
            let (next, forwarded) = router.forward(frame, now)?;
            frame = forwarded;

            if next == A || next == B {
                return Ok((next, frame, path));
            }
            if !self.routers.contains_key(&next) {
                return Err(DropReason::UnknownNextHop);
            }
            current = next;
        }

        Err(DropReason::UnknownNextHop)
    }
}

fn endpoint(address: GdpAddress) -> Endpoint {
    let mut endpoint = Endpoint::new(address, EndpointConfig::new(8192)).unwrap();
    // The routed harness operates at the real whole-frame DLP boundary but
    // does not model GC3/GS3 credit exchange. Seed enough deterministic credit
    // for the integration tests and account for receive credit explicitly.
    endpoint.dlp_mut().grant_data_tx_credit(8192);
    endpoint.dlp_mut().grant_control_tx_credit(8192);
    endpoint.dlp_mut().note_data_credit_granted(8192).unwrap();
    endpoint
}

fn linear_network() -> TestNetwork {
    let mut r1 = TestRouter::new(R1);
    r1.connect(A);
    r1.connect(R2);
    r1.add_route(a_prefix(), A);
    r1.add_route(b_prefix(), R2);

    let mut r2 = TestRouter::new(R2);
    r2.connect(R1);
    r2.connect(B);
    r2.add_route(a_prefix(), R1);
    r2.add_route(b_prefix(), B);

    TestNetwork::new([r1, r2])
}

fn pump_endpoints(
    a: &mut Endpoint,
    b: &mut Endpoint,
    network: &TestNetwork,
    now: u64,
) -> Result<usize> {
    let mut moved = 0usize;
    for _ in 0..256 {
        let mut progress = false;

        match a.poll_tx_frame() {
            Ok(Some(frame)) => {
                let (target, frame, _) = network
                    .route_frame(R1, frame, Instant::from_millis(now))
                    .map_err(|_| Error::NoRoute)?;
                assert_eq!(target, B);
                b.receive_frame(frame, now)?;
                moved += 1;
                progress = true;
            }
            Ok(None) | Err(Error::NoCredit) => {}
            Err(e) => return Err(e),
        }

        match b.poll_tx_frame() {
            Ok(Some(frame)) => {
                let (target, frame, _) = network
                    .route_frame(R2, frame, Instant::from_millis(now))
                    .map_err(|_| Error::NoRoute)?;
                assert_eq!(target, A);
                a.receive_frame(frame, now)?;
                moved += 1;
                progress = true;
            }
            Ok(None) | Err(Error::NoCredit) => {}
            Err(e) => return Err(e),
        }

        if !progress {
            return Ok(moved);
        }
    }

    Err(Error::BufferFull)
}

fn dummy_frame(source: GdpAddress, destination: GdpAddress, hop_limit: u8) -> GnetFrame {
    let packet = GdpPacket::new(
        GdpHeader::global(
            GdpType::Reserved(0x0f),
            SizeClass::Tiny3,
            hop_limit,
            source,
            destination,
        ),
        vec![1, 2, 3],
    )
    .unwrap();

    GnetFrame {
        vcid: Vcid::VC1,
        traffic: LinkTraffic::Data,
        bytes: packet.encode(GdpWireConfig::default()).unwrap(),
    }
}

#[test]
fn gts_roundtrip_crosses_two_real_gdp_forwarders() {
    let network = linear_network();
    let mut a = endpoint(A);
    let mut b = endpoint(B);

    a.routes_mut().add_default_route(R1).unwrap();
    b.routes_mut().add_default_route(R2).unwrap();

    let service = ServiceSelector::registered(1).unwrap();
    b.listen(service, ListenerConfig::default());
    let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);

    let ah = a
        .connect_at(B, service, profile, Instant::from_millis(0))
        .unwrap();
    pump_endpoints(&mut a, &mut b, &network, 0).unwrap();
    let bh = b.accept().unwrap();
    assert_eq!(a.tunnel_state(ah).unwrap(), TunnelState::Established);

    a.send(ah, 0, b"through two routers", 10).unwrap();
    pump_endpoints(&mut a, &mut b, &network, 10).unwrap();
    assert_eq!(
        b.recv(bh, 0).unwrap(),
        Some(b"through two routers".to_vec())
    );
    pump_endpoints(&mut a, &mut b, &network, 11).unwrap();

    b.send(bh, 0, b"reply", 20).unwrap();
    pump_endpoints(&mut a, &mut b, &network, 20).unwrap();
    assert_eq!(a.recv(ah, 0).unwrap(), Some(b"reply".to_vec()));
}

#[test]
fn longest_prefix_beats_default_and_records_expected_path() {
    let mut r1 = TestRouter::new(R1);
    r1.connect(R2);
    r1.connect(R3);
    r1.add_default(R3);
    r1.add_route(b_prefix(), R2);

    let mut r2 = TestRouter::new(R2);
    r2.connect(B);
    r2.add_route(b_prefix(), B);

    let mut r3 = TestRouter::new(R3);
    r3.connect(B);
    r3.add_route(b_prefix(), B);

    let network = TestNetwork::new([r1, r2, r3]);
    let (_, _, path) = network
        .route_frame(R1, dummy_frame(A, B, 8), Instant::ZERO)
        .unwrap();
    assert_eq!(path, vec![R1, R2]);
}

#[test]
fn default_route_is_used_when_no_specific_route_matches() {
    let mut r1 = TestRouter::new(R1);
    r1.connect(R3);
    r1.add_default(R3);

    let mut r3 = TestRouter::new(R3);
    r3.connect(B);
    r3.add_default(B);

    let network = TestNetwork::new([r1, r3]);
    let (_, _, path) = network
        .route_frame(R1, dummy_frame(A, B, 8), Instant::ZERO)
        .unwrap();
    assert_eq!(path, vec![R1, R3]);
}

#[test]
fn hop_limit_exhaustion_drops_at_first_router() {
    let network = linear_network();
    assert_eq!(
        network.route_frame(R1, dummy_frame(A, B, 1), Instant::ZERO),
        Err(DropReason::HopLimit)
    );
}

#[test]
fn no_route_drops_deterministically() {
    let mut r1 = TestRouter::new(R1);
    r1.connect(A);
    r1.add_route(a_prefix(), A);
    let network = TestNetwork::new([r1]);

    assert_eq!(
        network.route_frame(R1, dummy_frame(A, B, 8), Instant::ZERO),
        Err(DropReason::NoRoute)
    );
}

#[test]
fn expired_specific_route_falls_back_to_default_path() {
    let mut r1 = TestRouter::new(R1);
    r1.connect(R2);
    r1.connect(R3);
    r1.add_default(R3);
    let mut specific = Route::new(b_prefix(), R2);
    specific.expires_at = Some(Instant::from_millis(50));
    r1.routes.add(specific).unwrap();

    let mut r2 = TestRouter::new(R2);
    r2.connect(B);
    r2.add_default(B);

    let mut r3 = TestRouter::new(R3);
    r3.connect(B);
    r3.add_default(B);

    let network = TestNetwork::new([r1, r2, r3]);

    let (_, _, before) = network
        .route_frame(R1, dummy_frame(A, B, 8), Instant::from_millis(49))
        .unwrap();
    assert_eq!(before, vec![R1, R2]);

    let (_, _, after) = network
        .route_frame(R1, dummy_frame(A, B, 8), Instant::from_millis(51))
        .unwrap();
    assert_eq!(after, vec![R1, R3]);
}

#[test]
fn diamond_topology_can_fail_over_to_alternate_static_path() {
    let mut r1 = TestRouter::new(R1);
    r1.connect(A);
    r1.connect(R2);
    r1.connect(R3);
    r1.add_route(a_prefix(), A);
    r1.add_route(b_prefix(), R2);

    let mut r2 = TestRouter::new(R2);
    r2.connect(R1);
    r2.connect(R4);
    r2.add_route(a_prefix(), R1);
    r2.add_route(b_prefix(), R4);

    let mut r3 = TestRouter::new(R3);
    r3.connect(R1);
    r3.connect(R4);
    r3.add_route(a_prefix(), R1);
    r3.add_route(b_prefix(), R4);

    let mut r4 = TestRouter::new(R4);
    r4.connect(R2);
    r4.connect(R3);
    r4.connect(B);
    r4.add_route(a_prefix(), R2);
    r4.add_route(b_prefix(), B);

    let mut network = TestNetwork::new([r1, r2, r3, r4]);

    let (_, _, primary) = network
        .route_frame(R1, dummy_frame(A, B, 8), Instant::ZERO)
        .unwrap();
    assert_eq!(primary, vec![R1, R2, R4]);

    network.router_mut(R1).set_link(R2, false);
    network.router_mut(R1).routes.remove(b_prefix());
    network.router_mut(R1).add_route(b_prefix(), R3);
    network.router_mut(R4).routes.remove(a_prefix());
    network.router_mut(R4).add_route(a_prefix(), R3);

    let (_, _, alternate) = network
        .route_frame(R1, dummy_frame(A, B, 8), Instant::ZERO)
        .unwrap();
    assert_eq!(alternate, vec![R1, R3, R4]);
}

#[test]
fn forwarded_frame_keeps_end_to_end_addresses_and_decrements_hops() {
    let network = linear_network();
    let original = dummy_frame(A, B, 9);
    let (_, frame, path) = network.route_frame(R1, original, Instant::ZERO).unwrap();
    assert_eq!(path, vec![R1, R2]);

    let packet = GdpPacket::decode(&frame.bytes, GdpWireConfig::default(), 0).unwrap();
    assert_eq!(packet.header.source(), A);
    assert_eq!(packet.header.destination(), B);
    assert_eq!(packet.header.hop_limit, 7);
}
