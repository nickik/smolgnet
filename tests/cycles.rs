use std::collections::{BTreeMap, BTreeSet};

use smolgnet::*;

const ROUTERS: usize = 4;
const NODE_PREFIX_BASE: u64 = 0x7000_0000_0000_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Resource {
    link: usize,
    vcid: u8,
}

#[derive(Debug, Clone)]
struct Flow {
    id: usize,
    held: Vec<Resource>,
    wants: Resource,
}

fn resource(link: usize, vcid: u8) -> Resource {
    Resource { link, vcid }
}

fn node_network(index: usize) -> GdpAddress {
    GdpAddress(NODE_PREFIX_BASE | ((index as u64 + 1) << 16))
}

fn node_address(index: usize) -> GdpAddress {
    GdpAddress(node_network(index).0 | 1)
}

fn node_prefix(index: usize) -> GdpPrefix {
    GdpPrefix::new(node_network(index), 48).unwrap()
}

fn router(index: usize, policy: RouterVc0Policy) -> CycleAwareRouter {
    let p0 = RouterPortConfig::new(0, GdpAddress(0xfe80_0000_0000_0100 + index as u64 * 2))
        .with_connected_network(node_prefix(index), node_address(index));
    let p1 = RouterPortConfig::new(1, GdpAddress(0xfe80_0000_0000_0101 + index as u64 * 2));
    let routes = vec![StaticRouteConfig::direct(GdpPrefix::default_route(), 1)];
    CycleAwareRouter::new(RouterStartupConfig::new(vec![p0, p1], routes), policy).unwrap()
}

fn packet(flow: usize) -> GdpPacket {
    let source = GdpAddress(0x7100_0000_0000_0000 | flow as u64);
    let destination = node_address((flow + 2) % ROUTERS);
    let mut payload = vec![0x40 + flow as u8; 32];
    payload[0] = flow as u8;
    GdpPacket::new(
        GdpHeader::global(GdpType::Gts, SizeClass::Ctrl32, 8, source, destination),
        payload,
    )
    .unwrap()
}

/// Verify that each synthetic flow really requires the two consecutive ring
/// links represented by the basic resource model below.
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

fn make_cycle(vcid: u8) -> Vec<Flow> {
    (0..ROUTERS)
        .map(|flow| Flow {
            id: flow,
            held: vec![resource(flow, vcid)],
            wants: resource((flow + 1) % ROUTERS, vcid),
        })
        .collect()
}

fn held_resources(flows: &[Flow]) -> BTreeMap<Resource, usize> {
    let mut held = BTreeMap::new();
    for flow in flows {
        for held_resource in &flow.held {
            assert!(held.insert(*held_resource, flow.id).is_none());
        }
    }
    held
}

/// Build the actual wait-for graph for the current wormhole state and return
/// every closed flow cycle. A flow has one outgoing wait edge only when its
/// requested resource is currently held by another blocked flow.
///
/// This is deliberately stronger than merely finding a possible cycle in the
/// static route/channel-dependency graph: it proves that the resources needed
/// to close the cycle are simultaneously occupied in this simulated state.
fn detect_wait_cycles(flows: &[Flow]) -> Vec<Vec<usize>> {
    let held = held_resources(flows);
    let waits_for: BTreeMap<usize, usize> = flows
        .iter()
        .filter_map(|flow| {
            held.get(&flow.wants)
                .copied()
                .filter(|holder| *holder != flow.id)
                .map(|holder| (flow.id, holder))
        })
        .collect();

    let mut cycles = Vec::new();
    let mut globally_seen = BTreeSet::new();

    for start in waits_for.keys().copied() {
        if globally_seen.contains(&start) {
            continue;
        }

        let mut path = Vec::new();
        let mut index = BTreeMap::new();
        let mut current = start;

        loop {
            if let Some(cycle_start) = index.get(&current).copied() {
                cycles.push(path[cycle_start..].to_vec());
                globally_seen.extend(path);
                break;
            }
            if globally_seen.contains(&current) {
                globally_seen.extend(path);
                break;
            }

            index.insert(current, path.len());
            path.push(current);

            let Some(next) = waits_for.get(&current).copied() else {
                globally_seen.extend(path);
                break;
            };
            current = next;
        }
    }

    cycles.sort();
    cycles
}

fn assert_closed_wait_cycle(flows: &[Flow], expected: &[usize]) {
    let cycles = detect_wait_cycles(flows);
    assert_eq!(
        cycles.len(),
        1,
        "expected exactly one wait cycle: {cycles:?}"
    );
    assert_eq!(cycles[0].len(), expected.len());
    assert_eq!(
        cycles[0].iter().copied().collect::<BTreeSet<_>>(),
        expected.iter().copied().collect::<BTreeSet<_>>()
    );
}

/// Escape paths use VC0 exclusively and a monotonically increasing escape
/// rank. Strict rank increase is the test's compact representation of an
/// acyclic deterministic escape routing function (for example a topology-
/// derived up*/down* or spanning-tree discipline).
fn assert_acyclic_escape_paths(paths: &[Vec<(usize, usize)>]) {
    for path in paths {
        let mut previous_rank = None;
        for (link, rank) in path {
            let escape = resource(*link, 0);
            assert_eq!(escape.vcid, 0);
            if let Some(previous) = previous_rank {
                assert!(
                    previous < *rank,
                    "escape rank must increase: {previous} !< {rank} on link {link}"
                );
            }
            previous_rank = Some(*rank);
        }
    }
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

    let flows = make_cycle(0);
    assert_closed_wait_cycle(&flows, &[0, 1, 2, 3]);
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

    let flows = make_cycle(1);
    assert_closed_wait_cycle(&flows, &[0, 1, 2, 3]);
    let held = held_resources(&flows);

    for flow in &flows {
        let escape = resource(flow.wants.link, 0);
        assert!(!held.contains_key(&escape));
    }

    // In this minimal case escape is the final hop, so there is no VC0->VC0
    // dependency at all.
    assert_acyclic_escape_paths(&[vec![(1, 10)], vec![(2, 20)], vec![(3, 30)], vec![(0, 40)]]);
}

#[test]
fn multi_hop_wraparound_cycle_is_detected_and_escape_route_breaks_it() {
    // Each worm already spans two physical links. Its head then requests a
    // third resource. Flow 2's request wraps from link 5 back to link 0, which
    // closes the dependency after the packets have occupied multiple hops:
    //
    // F0: holds L0,L1 -> wants L2
    // F1: holds L2,L3 -> wants L4
    // F2: holds L4,L5 -> wants L0
    let flows = vec![
        Flow {
            id: 0,
            held: vec![resource(0, 1), resource(1, 1)],
            wants: resource(2, 1),
        },
        Flow {
            id: 1,
            held: vec![resource(2, 1), resource(3, 1)],
            wants: resource(4, 1),
        },
        Flow {
            id: 2,
            held: vec![resource(4, 1), resource(5, 1)],
            wants: resource(0, 1),
        },
    ];

    assert_closed_wait_cycle(&flows, &[0, 1, 2]);

    let held = held_resources(&flows);
    for flow in &flows {
        assert!(!held.contains_key(&resource(flow.wants.link, 0)));
    }

    // Escape routing does NOT simply repeat the cyclic ring route on VC0.
    // These synthetic links model a deterministic escape tree/subnetwork.
    // Every path moves only toward larger escape ranks, so no VC0 path can
    // return to an earlier escape resource and close a cycle.
    let escape_paths = vec![
        vec![(100, 10), (101, 20), (102, 30)],
        vec![(103, 15), (101, 20), (102, 30)],
        vec![(104, 5), (100, 10), (101, 20)],
    ];
    assert_acyclic_escape_paths(&escape_paths);
}

#[test]
fn mixed_vc_multi_hop_cycle_still_needs_the_vc0_escape_class() {
    // A dependency cycle is not required to stay on a single normal VC class.
    // Legal VC transitions can create a cycle across VC1/VC2/VC3 as well.
    let flows = vec![
        Flow {
            id: 0,
            held: vec![resource(0, 1), resource(1, 2)],
            wants: resource(2, 3),
        },
        Flow {
            id: 1,
            held: vec![resource(2, 3), resource(3, 1)],
            wants: resource(4, 2),
        },
        Flow {
            id: 2,
            held: vec![resource(4, 2), resource(5, 3)],
            wants: resource(0, 1),
        },
    ];

    assert_closed_wait_cycle(&flows, &[0, 1, 2]);

    let held = held_resources(&flows);
    for flow in &flows {
        assert!(!held.contains_key(&resource(flow.wants.link, 0)));
    }

    assert_acyclic_escape_paths(&[
        vec![(200, 1), (201, 2), (202, 3), (203, 4)],
        vec![(204, 1), (205, 2), (203, 4)],
        vec![(206, 1), (201, 2), (202, 3)],
    ]);
}

#[test]
fn detector_finds_multiple_deadlocked_components_without_calling_blocked_tail_deadlocked() {
    // Two independent cycles plus one flow that is merely queued behind one of
    // them. The extra flow is blocked, but it is not itself part of a circular
    // wait. This is why "no progress for N cycles" is not a proof of deadlock.
    let flows = vec![
        Flow {
            id: 0,
            held: vec![resource(0, 1)],
            wants: resource(1, 1),
        },
        Flow {
            id: 1,
            held: vec![resource(1, 1)],
            wants: resource(2, 1),
        },
        Flow {
            id: 2,
            held: vec![resource(2, 1)],
            wants: resource(0, 1),
        },
        Flow {
            id: 3,
            held: vec![resource(3, 2)],
            wants: resource(4, 2),
        },
        Flow {
            id: 4,
            held: vec![resource(4, 2)],
            wants: resource(3, 2),
        },
        Flow {
            id: 5,
            held: vec![resource(5, 3)],
            wants: resource(1, 1),
        },
    ];

    let cycles = detect_wait_cycles(&flows);
    assert_eq!(cycles.len(), 2);
    let members: Vec<BTreeSet<usize>> = cycles
        .iter()
        .map(|cycle| cycle.iter().copied().collect())
        .collect();
    assert!(members.contains(&BTreeSet::from([0, 1, 2])));
    assert!(members.contains(&BTreeSet::from([3, 4])));
    assert!(members.iter().all(|cycle| !cycle.contains(&5)));

    let held = held_resources(&flows);
    for flow in &flows {
        assert!(!held.contains_key(&resource(flow.wants.link, 0)));
    }

    assert_acyclic_escape_paths(&[
        vec![(300, 1), (301, 2)],
        vec![(302, 1), (301, 2)],
        vec![(303, 1), (304, 2)],
        vec![(305, 1), (306, 2)],
        vec![(307, 1), (306, 2)],
        vec![(308, 1), (301, 2)],
    ]);
}
