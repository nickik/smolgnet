use std::collections::BTreeMap;

use smolgnet::*;

const ROUTERS: usize = 4;
const NODE_PREFIX: u64 = 0x7000_0000_0000_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Resource {
    link: usize,
    vcid: u8,
}

#[derive(Debug, Clone, Copy)]
struct Flow {
    id: usize,
    held: Resource,
    wants: Resource,
}

fn node_address(index: usize) -> GdpAddress {
    GdpAddress(NODE_PREFIX | ((index as u64 + 1) << 16) | 1)
}

fn node_prefix(index: usize) -> GdpPrefix {
    GdpPrefix::new(node_address(index), 64).unwrap()
}

fn router(index: usize, policy: RouterVc0Policy) -> CycleAwareRouter {
    let p0 = RouterPortConfig::new(
        0,
        GdpAddress(0xfe80_0000_0000_0100 + index as u64 * 2),
    )
    .with_connected_network(node_prefix(index), node_address(index));
    let p1 = RouterPortConfig::new(
        1,
        GdpAddress(0xfe80_0000_0000_0101 + index as u64 * 2),
    );
    let routes = vec![StaticRouteConfig::direct(GdpPrefix::default_route(), 1)];
    CycleAwareRouter::new(RouterStartupConfig::new(vec![p0, p1], routes), policy).unwrap()
}

fn packet(flow: usize) -> GdpPacket {
    let source = GdpAddress(0x7100_0000_0000_0000 | flow as u64);
    let destination = node_address((flow + 2) % ROUTERS);
    let mut payload = vec![0x40 + flow as u8; 32];
    payload[0] = flow as u8;
    GdpPacket::new(
        GdpHeader::global(
            GdpType::Gts,
            SizeClass::Ctrl32,
            8,
            source,
            destination,
        ),
        payload,
    )
    .unwrap()
}

/// Verify that each synthetic flow really requires the two consecutive ring
/// links represented by the resource model below.
fn verify_two_hop_routes(routers: &mut [CycleAwareRouter; ROUTERS]) {
    for flow in 0..ROUTERS {
        let next = (flow + 1) % ROUTERS;
        let destination = (flow + 2) % ROUTERS;
        let packet = match routers[flow].process(0, packet(flow)).unwrap() {
            RouterDisposition::Forward {
                egress_port,
                packet,
            } => {
                assert_eq!(egress_port, 1);
                assert_eq!(packet.header.hop_limit, 7);
                packet
            }
            other => panic!("flow {flow} did not take first clockwise link: {other:?}"),
        };

        let packet = match routers[next].process(0, packet).unwrap() {
            RouterDisposition::Forward {
                egress_port,
                packet,
            } => {
                assert_eq!(egress_port, 1);
                assert_eq!(packet.header.hop_limit, 6);
                packet
            }
            other => panic!("flow {flow} did not take second clockwise link: {other:?}"),
        };

        match routers[destination].process(0, packet).unwrap() {
            RouterDisposition::Punt { packet, .. } => {
                assert_eq!(packet.header.destination(), node_address(destination));
                assert_eq!(packet.payload[0] as usize, flow);
                assert_eq!(packet.header.hop_limit, 6);
            }
            other => panic!("flow {flow} did not terminate at destination router: {other:?}"),
        }
    }
}

fn make_cycle(vcid: u8) -> [Flow; ROUTERS] {
    std::array::from_fn(|flow| Flow {
        id: flow,
        held: Resource { link: flow, vcid },
        wants: Resource {
            link: (flow + 1) % ROUTERS,
            vcid,
        },
    })
}

fn held_resources(flows: &[Flow; ROUTERS]) -> BTreeMap<Resource, usize> {
    flows.iter().map(|flow| (flow.held, flow.id)).collect()
}

fn is_closed_wait_cycle(flows: &[Flow; ROUTERS]) -> bool {
    let held = held_resources(flows);
    flows.iter().all(|flow| {
        held.get(&flow.wants)
            .is_some_and(|holder| *holder != flow.id)
    })
}

#[test]
fn vc0_used_as_normal_data_allows_four_router_cycle_to_deadlock() {
    let mut routers: [CycleAwareRouter; ROUTERS] =
        std::array::from_fn(|i| router(i, RouterVc0Policy::NormalData));
    verify_two_hop_routes(&mut routers);

    for router in &routers {
        assert_eq!(router.vc0_policy(), RouterVc0Policy::NormalData);
        assert_eq!(router.normal_data_vcids(), &[0, 1, 2, 3]);
        assert_eq!(router.escape_vcid(), None);
    }

    // Four worms have each acquired their first clockwise link on VC0. Each
    // now waits for the next VC0 resource, which is held by the next worm.
    // No head flit can advance and none can release its tail: a closed wait
    // cycle exists across all four links.
    let flows = make_cycle(0);
    assert!(is_closed_wait_cycle(&flows));

    let held = held_resources(&flows);
    for flow in flows {
        assert!(held.contains_key(&flow.wants));
    }
}

#[test]
fn vc0_reserved_for_escape_breaks_the_same_cycle() {
    let mut routers: [CycleAwareRouter; ROUTERS] =
        std::array::from_fn(|i| router(i, RouterVc0Policy::EscapeOnly));
    verify_two_hop_routes(&mut routers);

    for router in &routers {
        assert_eq!(router.vc0_policy(), RouterVc0Policy::EscapeOnly);
        assert_eq!(router.normal_data_vcids(), &[1, 2, 3]);
        assert_eq!(router.escape_vcid(), Some(0));
    }

    // The same four worms can form a dependency cycle on an ordinary VC.
    let flows = make_cycle(1);
    assert!(is_closed_wait_cycle(&flows));
    let held = held_resources(&flows);

    // But VC0 was never consumed by ordinary traffic. Every blocked worm has
    // a distinct next-link VC0 escape resource available. In this deliberately
    // minimal two-hop test, entering that escape VC is the final hop, so there
    // is no VC0 -> VC0 dependency and the escape subnetwork is acyclic.
    let mut escape_owners = BTreeMap::new();
    for flow in flows {
        let escape = Resource {
            link: flow.wants.link,
            vcid: routers[flow.id].escape_vcid().unwrap(),
        };
        assert!(!held.contains_key(&escape));
        assert!(escape_owners.insert(escape, flow.id).is_none());
    }

    assert_eq!(escape_owners.len(), ROUTERS);
}
