use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::vec::Vec;

use p4rs::{packet_in, Pipeline};

use crate::error::{Error, Result};
use crate::wire::gdp::{GdpAddress, GdpAddresses, GdpPacket, GdpType, GdpWireConfig, SizeClass};

p4_macro::use_p4!(
    p4 = "p4-router/p4/router.p4",
    pipeline_name = "gnet_router",
);

pub type RouterPortId = u16;
pub type AdjacencyId = u16;
pub type RouteId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueClass {
    Control,
    Interactive,
    Bulk,
}

impl QueueClass {
    const fn index(self) -> usize {
        match self {
            Self::Control => 0,
            Self::Interactive => 1,
            Self::Bulk => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    Allow,
    Drop,
    Punt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteTarget {
    Adjacency(AdjacencyId),
    Cpu,
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterAdjacency {
    pub id: AdjacencyId,
    pub port: RouterPortId,
    pub next_hop: GdpAddress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterRoute {
    pub id: RouteId,
    pub prefix: u64,
    pub prefix_len: u8,
    pub preference: u16,
    pub target: RouteTarget,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouterCounters {
    pub rx_packets: u64,
    pub forwarded_packets: u64,
    pub cpu_punts: u64,
    pub drops: u64,
    pub queue_drops: u64,
    pub route_rebuilds: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PortCounters {
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub drops: u64,
}

#[derive(Debug)]
struct RouterPort {
    up: bool,
    queues: [VecDeque<GdpPacket>; 3],
    counters: PortCounters,
}

impl RouterPort {
    fn new() -> Self {
        Self {
            up: true,
            queues: [VecDeque::new(), VecDeque::new(), VecDeque::new()],
            counters: PortCounters::default(),
        }
    }

    fn queued(&self) -> usize {
        self.queues.iter().map(VecDeque::len).sum()
    }

    fn clear_queues(&mut self) -> usize {
        let dropped = self.queued();
        for queue in &mut self.queues {
            queue.clear();
        }
        self.counters.drops = self.counters.drops.saturating_add(dropped as u64);
        dropped
    }

    fn push(&mut self, class: QueueClass, packet: GdpPacket, limit: usize) -> Result<()> {
        if self.queued() >= limit {
            self.counters.drops = self.counters.drops.saturating_add(1);
            return Err(Error::BufferFull);
        }
        self.queues[class.index()].push_back(packet);
        Ok(())
    }

    fn pop(&mut self) -> Option<GdpPacket> {
        for queue in &mut self.queues {
            if let Some(packet) = queue.pop_front() {
                self.counters.tx_packets = self.counters.tx_packets.saturating_add(1);
                return Some(packet);
            }
        }
        None
    }
}

/// Multi-port GDP router with an x4c/P4 forwarding plane and a small Rust
/// control/queue shell. DLP is deliberately absent from this type: callers
/// exchange only complete `GdpPacket` values at each router port.
pub struct P4Router {
    pipeline: main_pipeline,
    physical_ports: u16,
    cpu_port: u16,
    queue_limit: usize,
    cpu_queue_limit: usize,
    ports: Vec<RouterPort>,
    cpu_queue: VecDeque<GdpPacket>,
    adjacencies: BTreeMap<AdjacencyId, RouterAdjacency>,
    routes: BTreeMap<RouteId, RouterRoute>,
    policies: BTreeMap<(RouterPortId, u8), PolicyAction>,
    next_route_id: RouteId,
    counters: RouterCounters,
}

impl P4Router {
    pub fn new(physical_ports: u16, queue_limit: usize) -> Result<Self> {
        if physical_ports == 0 || queue_limit == 0 || physical_ports == u16::MAX {
            return Err(Error::InvalidField);
        }
        let cpu_port = physical_ports;
        Ok(Self {
            pipeline: main_pipeline::new(physical_ports + 1),
            physical_ports,
            cpu_port,
            queue_limit,
            cpu_queue_limit: queue_limit,
            ports: (0..physical_ports).map(|_| RouterPort::new()).collect(),
            cpu_queue: VecDeque::new(),
            adjacencies: BTreeMap::new(),
            routes: BTreeMap::new(),
            policies: BTreeMap::new(),
            next_route_id: 1,
            counters: RouterCounters::default(),
        })
    }

    pub const fn physical_port_count(&self) -> u16 {
        self.physical_ports
    }

    pub const fn cpu_port(&self) -> u16 {
        self.cpu_port
    }

    pub const fn counters(&self) -> RouterCounters {
        self.counters
    }

    pub fn port_counters(&self, port: RouterPortId) -> Result<PortCounters> {
        Ok(self.port(port)?.counters)
    }

    pub fn adjacency(&self, id: AdjacencyId) -> Option<RouterAdjacency> {
        self.adjacencies.get(&id).copied()
    }

    pub fn route(&self, id: RouteId) -> Option<RouterRoute> {
        self.routes.get(&id).copied()
    }

    pub fn set_cpu_queue_limit(&mut self, limit: usize) -> Result<()> {
        if limit == 0 {
            return Err(Error::InvalidField);
        }
        self.cpu_queue_limit = limit;
        Ok(())
    }

    pub fn install_adjacency(
        &mut self,
        id: AdjacencyId,
        port: RouterPortId,
        next_hop: GdpAddress,
    ) -> Result<()> {
        self.port(port)?;
        self.adjacencies.insert(id, RouterAdjacency { id, port, next_hop });
        self.rebuild_pipeline()
    }

    pub fn remove_adjacency(&mut self, id: AdjacencyId) -> Result<()> {
        self.adjacencies.remove(&id);
        self.rebuild_pipeline()
    }

    pub fn install_route(
        &mut self,
        prefix: u64,
        prefix_len: u8,
        preference: u16,
        target: RouteTarget,
    ) -> Result<RouteId> {
        if prefix_len > 64 {
            return Err(Error::InvalidField);
        }
        if let RouteTarget::Adjacency(id) = target {
            if !self.adjacencies.contains_key(&id) {
                return Err(Error::InvalidField);
            }
        }
        let id = self.next_route_id;
        self.next_route_id = self.next_route_id.wrapping_add(1).max(1);
        self.routes.insert(
            id,
            RouterRoute {
                id,
                prefix: normalize_prefix(prefix, prefix_len),
                prefix_len,
                preference,
                target,
            },
        );
        self.rebuild_pipeline()?;
        Ok(id)
    }

    pub fn remove_route(&mut self, id: RouteId) -> Result<()> {
        if self.routes.remove(&id).is_none() {
            return Err(Error::InvalidField);
        }
        self.rebuild_pipeline()
    }

    pub fn set_policy(
        &mut self,
        ingress_port: RouterPortId,
        packet_type: GdpType,
        action: PolicyAction,
    ) -> Result<()> {
        self.port(ingress_port)?;
        self.policies
            .insert((ingress_port, packet_type.to_wire()), action);
        self.rebuild_pipeline()
    }

    pub fn clear_policy(&mut self, ingress_port: RouterPortId, packet_type: GdpType) -> Result<()> {
        self.policies.remove(&(ingress_port, packet_type.to_wire()));
        self.rebuild_pipeline()
    }

    /// Link-down is a control-plane event. Pending egress packets for the dead
    /// link are discarded and P4 tables are rebuilt so the next-best candidate
    /// for each affected prefix becomes active immediately.
    pub fn set_port_up(&mut self, port: RouterPortId, up: bool) -> Result<()> {
        let dropped = {
            let p = self.port_mut(port)?;
            p.up = up;
            if up { 0 } else { p.clear_queues() }
        };
        if dropped != 0 {
            self.counters.queue_drops = self.counters.queue_drops.saturating_add(dropped as u64);
            self.counters.drops = self.counters.drops.saturating_add(dropped as u64);
        }
        self.rebuild_pipeline()
    }

    pub fn port_is_up(&self, port: RouterPortId) -> Result<bool> {
        Ok(self.port(port)?.up)
    }

    /// Ingest one complete packet from the frozen DLP/GDP boundary.
    pub fn ingest(&mut self, ingress_port: RouterPortId, packet: GdpPacket) -> Result<()> {
        if !self.port(ingress_port)?.up {
            let p = self.port_mut(ingress_port)?;
            p.counters.drops = p.counters.drops.saturating_add(1);
            self.counters.drops = self.counters.drops.saturating_add(1);
            return Err(Error::LinkDown);
        }

        self.counters.rx_packets = self.counters.rx_packets.saturating_add(1);
        {
            let p = self.port_mut(ingress_port)?;
            p.counters.rx_packets = p.counters.rx_packets.saturating_add(1);
        }

        let local_prefix = match packet.header.addresses {
            GdpAddresses::Global { .. } => 0,
            GdpAddresses::Local { prefix, .. } => prefix,
        };
        let cfg = GdpWireConfig::default();
        let bytes = packet.encode(cfg)?;
        let mut input = packet_in::new(&bytes);
        let outputs = self.pipeline.process_packet(ingress_port, &mut input);

        if outputs.is_empty() {
            self.counters.drops = self.counters.drops.saturating_add(1);
            let p = self.port_mut(ingress_port)?;
            p.counters.drops = p.counters.drops.saturating_add(1);
            return Ok(());
        }

        for (out, port) in outputs {
            let mut bytes = out.header_data;
            bytes.extend_from_slice(out.payload_data);
            let routed = GdpPacket::decode(&bytes, cfg, local_prefix)?;
            if port == self.cpu_port {
                if self.cpu_queue.len() >= self.cpu_queue_limit {
                    self.counters.queue_drops = self.counters.queue_drops.saturating_add(1);
                    self.counters.drops = self.counters.drops.saturating_add(1);
                    continue;
                }
                self.cpu_queue.push_back(routed);
                self.counters.cpu_punts = self.counters.cpu_punts.saturating_add(1);
                continue;
            }

            let egress = port as RouterPortId;
            if egress >= self.physical_ports || !self.port(egress)?.up {
                self.counters.drops = self.counters.drops.saturating_add(1);
                continue;
            }
            let class = classify_queue(&routed);
            match self.port_mut(egress)?.push(class, routed, self.queue_limit) {
                Ok(()) => {
                    self.counters.forwarded_packets =
                        self.counters.forwarded_packets.saturating_add(1);
                }
                Err(Error::BufferFull) => {
                    self.counters.queue_drops = self.counters.queue_drops.saturating_add(1);
                    self.counters.drops = self.counters.drops.saturating_add(1);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn poll_egress(&mut self, port: RouterPortId) -> Result<Option<GdpPacket>> {
        if !self.port(port)?.up {
            return Err(Error::LinkDown);
        }
        Ok(self.port_mut(port)?.pop())
    }

    pub fn queued_on_port(&self, port: RouterPortId) -> Result<usize> {
        Ok(self.port(port)?.queued())
    }

    pub fn poll_cpu(&mut self) -> Option<GdpPacket> {
        self.cpu_queue.pop_front()
    }

    pub fn cpu_queue_len(&self) -> usize {
        self.cpu_queue.len()
    }

    fn port(&self, port: RouterPortId) -> Result<&RouterPort> {
        self.ports.get(port as usize).ok_or(Error::InvalidField)
    }

    fn port_mut(&mut self, port: RouterPortId) -> Result<&mut RouterPort> {
        self.ports.get_mut(port as usize).ok_or(Error::InvalidField)
    }

    fn route_target_active(&self, target: RouteTarget) -> bool {
        match target {
            RouteTarget::Adjacency(id) => self
                .adjacencies
                .get(&id)
                .and_then(|a| self.ports.get(a.port as usize))
                .map(|p| p.up)
                .unwrap_or(false),
            RouteTarget::Cpu | RouteTarget::Drop => true,
        }
    }

    fn target_programming(&self, target: RouteTarget) -> Option<(&'static str, Vec<u8>)> {
        match target {
            RouteTarget::Adjacency(id) => {
                let adjacency = self.adjacencies.get(&id)?;
                if !self.ports.get(adjacency.port as usize)?.up {
                    return None;
                }
                Some(("forward", adjacency.port.to_le_bytes().to_vec()))
            }
            RouteTarget::Cpu => Some(("punt", self.cpu_port.to_le_bytes().to_vec())),
            RouteTarget::Drop => Some(("drop", Vec::new())),
        }
    }

    fn rebuild_pipeline(&mut self) -> Result<()> {
        let mut pipeline = main_pipeline::new(self.physical_ports + 1);

        for (&(port, packet_type), &action) in &self.policies {
            let mut key = port.to_le_bytes().to_vec();
            key.push(packet_type);
            let (name, params) = match action {
                PolicyAction::Allow => ("allow", Vec::new()),
                PolicyAction::Drop => ("drop", Vec::new()),
                PolicyAction::Punt => ("punt", self.cpu_port.to_le_bytes().to_vec()),
            };
            pipeline.add_table_entry("ingress.policy", name, &key, &params, 0);
        }

        let mut candidates: Vec<RouterRoute> = self
            .routes
            .values()
            .copied()
            .filter(|r| self.route_target_active(r.target))
            .collect();
        candidates.sort_by_key(|r| (r.prefix, r.prefix_len, r.preference, r.id));

        let mut programmed = BTreeSet::new();
        for route in candidates {
            if !programmed.insert((route.prefix, route.prefix_len)) {
                continue;
            }
            let Some((action, params)) = self.target_programming(route.target) else {
                continue;
            };
            if route.prefix_len > 32 {
                let hi = (route.prefix >> 32) as u32;
                let lo = route.prefix as u32;
                let mut key = hi.to_le_bytes().to_vec();
                key.extend_from_slice(&lo.to_be_bytes());
                key.push(route.prefix_len - 32);
                pipeline.add_table_entry(
                    "ingress.global_long_routes",
                    action,
                    &key,
                    &params,
                    0,
                );
            } else {
                let hi = (route.prefix >> 32) as u32;
                let mut key = hi.to_be_bytes().to_vec();
                key.push(route.prefix_len);
                pipeline.add_table_entry(
                    "ingress.global_short_routes",
                    action,
                    &key,
                    &params,
                    0,
                );
            }
        }

        self.pipeline = pipeline;
        self.counters.route_rebuilds = self.counters.route_rebuilds.saturating_add(1);
        Ok(())
    }
}

fn normalize_prefix(prefix: u64, len: u8) -> u64 {
    match len {
        0 => 0,
        64 => prefix,
        n => prefix & (u64::MAX << (64 - n)),
    }
}

fn classify_queue(packet: &GdpPacket) -> QueueClass {
    if packet.header.packet_type == GdpType::Gctl {
        QueueClass::Control
    } else if matches!(
        packet.header.size_class,
        SizeClass::Empty | SizeClass::Tiny3 | SizeClass::Ctrl32 | SizeClass::Ctrl64
    ) {
        QueueClass::Interactive
    } else {
        QueueClass::Bulk
    }
}
