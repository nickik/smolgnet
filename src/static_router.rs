#![cfg(feature = "p4-router")]

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use p4rs::{packet_in, Pipeline};

use crate::error::{Error, Result};
use crate::routing::GdpPrefix;
use crate::wire::gdp::{GdpAddress, GdpAddresses, GdpPacket, GdpWireConfig};

p4_macro::use_p4!(
    p4 = "p4-static-router/p4/router.p4",
    pipeline_name = "gnet_static_router",
);

pub type RouterPortId = u16;

/// Reserved link-scoped destination used by an unconfigured host for its first
/// `SOLICIT(Router)` exchange. It is always punted to Rust management and is
/// never transit-routed.
pub const ROUTER_BOOTSTRAP_ADDRESS: GdpAddress = GdpAddress(0xfe80_0000_0000_0000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterAttachment {
    Direct,
    Coupler,
    Switch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchRegistrationState {
    NotRequired,
    Unregistered,
    Registering,
    Registered,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    Observed,
    Offered,
    Configured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterNodeRecord {
    pub link_local: GdpAddress,
    pub routed_address: Option<GdpAddress>,
    pub ingress_port: RouterPortId,
    pub state: NodeState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterPortConfig {
    pub id: RouterPortId,
    pub attachment: RouterAttachment,
    pub link_local: GdpAddress,
    pub connected_prefix: Option<GdpPrefix>,
    pub routed_address: Option<GdpAddress>,
}

impl RouterPortConfig {
    pub fn new(id: RouterPortId, attachment: RouterAttachment, link_local: GdpAddress) -> Self {
        Self {
            id,
            attachment,
            link_local,
            connected_prefix: None,
            routed_address: None,
        }
    }

    pub fn with_connected_network(
        mut self,
        prefix: GdpPrefix,
        router_address: GdpAddress,
    ) -> Self {
        self.connected_prefix = Some(prefix);
        self.routed_address = Some(router_address);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticRouteConfig {
    pub prefix: GdpPrefix,
    pub egress_port: RouterPortId,
    /// Management/adjacency metadata only. GDP destination is never rewritten.
    pub next_hop: Option<GdpAddress>,
}

impl StaticRouteConfig {
    pub const fn direct(prefix: GdpPrefix, egress_port: RouterPortId) -> Self {
        Self {
            prefix,
            egress_port,
            next_hop: None,
        }
    }

    pub const fn via(
        prefix: GdpPrefix,
        egress_port: RouterPortId,
        next_hop: GdpAddress,
    ) -> Self {
        Self {
            prefix,
            egress_port,
            next_hop: Some(next_hop),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterStartupConfig {
    pub ports: Vec<RouterPortConfig>,
    pub routes: Vec<StaticRouteConfig>,
}

impl RouterStartupConfig {
    pub fn new(ports: Vec<RouterPortConfig>, routes: Vec<StaticRouteConfig>) -> Self {
        Self { ports, routes }
    }

    pub fn validate(&self) -> Result<()> {
        if self.ports.is_empty() || self.ports.len() >= u16::MAX as usize {
            return Err(Error::InvalidField);
        }

        let mut seen_ports = BTreeSet::new();
        let mut owned_addresses = BTreeSet::new();
        let mut forwarding_prefixes = BTreeSet::new();

        for (index, port) in self.ports.iter().enumerate() {
            if port.id as usize != index || !seen_ports.insert(port.id) {
                return Err(Error::InvalidField);
            }
            if !is_link_local(port.link_local) || port.link_local == ROUTER_BOOTSTRAP_ADDRESS {
                return Err(Error::InvalidField);
            }
            if !owned_addresses.insert(port.link_local) {
                return Err(Error::InvalidField);
            }

            match (port.connected_prefix, port.routed_address) {
                (None, None) => {}
                (Some(prefix), Some(address)) => {
                    if !is_address_authority_prefix(prefix)
                        || is_link_local(prefix.network())
                        || !prefix.contains(address)
                        || is_link_local(address)
                        || !owned_addresses.insert(address)
                        || !forwarding_prefixes.insert(prefix)
                    {
                        return Err(Error::InvalidField);
                    }
                }
                _ => return Err(Error::InvalidField),
            }
        }

        for route in &self.routes {
            if route.egress_port as usize >= self.ports.len() {
                return Err(Error::InvalidField);
            }
            if !forwarding_prefixes.insert(route.prefix) {
                return Err(Error::InvalidField);
            }
            if route.prefix.prefix_len() == 64 {
                let destination = route.prefix.network();
                if destination == ROUTER_BOOTSTRAP_ADDRESS || owned_addresses.contains(&destination) {
                    return Err(Error::InvalidField);
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterPortState {
    pub config: RouterPortConfig,
    pub up: bool,
    pub switch_registration: SwitchRegistrationState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterDisposition {
    Forward {
        egress_port: RouterPortId,
        packet: GdpPacket,
    },
    Punt {
        ingress_port: RouterPortId,
        packet: GdpPacket,
    },
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FibOrigin {
    Bootstrap,
    RouterLocal,
    Connected,
    Static,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FibEntry {
    pub prefix: GdpPrefix,
    pub egress_port: Option<RouterPortId>,
    pub origin: FibOrigin,
}

/// Static GNet router reference implementation.
///
/// Rust owns startup configuration, node/port management state and P4 table
/// programming. The P4 pipeline owns normal per-packet LPM and Hop-Limit work.
pub struct StaticP4Router {
    pipeline: main_pipeline,
    physical_ports: u16,
    cpu_port: u16,
    startup: RouterStartupConfig,
    ports: Vec<RouterPortState>,
    nodes: BTreeMap<GdpAddress, RouterNodeRecord>,
    fib: Vec<FibEntry>,
    fib_generation: u64,
}

impl StaticP4Router {
    pub fn new(startup: RouterStartupConfig) -> Result<Self> {
        startup.validate()?;
        let physical_ports = startup.ports.len() as u16;
        let cpu_port = physical_ports;
        let ports = startup
            .ports
            .iter()
            .copied()
            .map(|config| RouterPortState {
                switch_registration: match config.attachment {
                    RouterAttachment::Switch => SwitchRegistrationState::Unregistered,
                    RouterAttachment::Direct | RouterAttachment::Coupler => {
                        SwitchRegistrationState::NotRequired
                    }
                },
                config,
                up: true,
            })
            .collect();

        let mut router = Self {
            pipeline: main_pipeline::new(physical_ports + 1),
            physical_ports,
            cpu_port,
            startup,
            ports,
            nodes: BTreeMap::new(),
            fib: Vec::new(),
            fib_generation: 0,
        };
        router.compile_startup_fib()?;
        Ok(router)
    }

    pub const fn physical_port_count(&self) -> u16 {
        self.physical_ports
    }

    pub const fn cpu_port(&self) -> u16 {
        self.cpu_port
    }

    pub const fn fib_generation(&self) -> u64 {
        self.fib_generation
    }

    pub fn startup_config(&self) -> &RouterStartupConfig {
        &self.startup
    }

    pub fn fib(&self) -> &[FibEntry] {
        &self.fib
    }

    pub fn port(&self, port: RouterPortId) -> Result<RouterPortState> {
        self.ports
            .get(port as usize)
            .copied()
            .ok_or(Error::InvalidField)
    }

    pub fn node(&self, link_local: GdpAddress) -> Option<RouterNodeRecord> {
        self.nodes.get(&link_local).copied()
    }

    pub fn node_by_routed_address(&self, address: GdpAddress) -> Option<RouterNodeRecord> {
        self.nodes
            .values()
            .copied()
            .find(|node| node.routed_address == Some(address))
    }

    pub fn nodes(&self) -> impl Iterator<Item = RouterNodeRecord> + '_ {
        self.nodes.values().copied()
    }

    pub fn record_node(
        &mut self,
        ingress_port: RouterPortId,
        link_local: GdpAddress,
    ) -> Result<RouterNodeRecord> {
        self.port(ingress_port)?;
        if !is_link_local(link_local) || link_local == ROUTER_BOOTSTRAP_ADDRESS {
            return Err(Error::InvalidField);
        }
        if self
            .startup
            .ports
            .iter()
            .any(|port| port.link_local == link_local)
        {
            return Err(Error::InvalidField);
        }

        let record = RouterNodeRecord {
            link_local,
            routed_address: None,
            ingress_port,
            state: NodeState::Observed,
        };
        match self.nodes.get(&link_local) {
            Some(existing) if existing.ingress_port != ingress_port => Err(Error::InvalidState),
            Some(existing) => Ok(*existing),
            None => {
                self.nodes.insert(link_local, record);
                Ok(record)
            }
        }
    }

    pub fn configure_node_address(
        &mut self,
        link_local: GdpAddress,
        routed_address: GdpAddress,
    ) -> Result<RouterNodeRecord> {
        let current = self.nodes.get(&link_local).copied().ok_or(Error::InvalidState)?;
        let port = self.port(current.ingress_port)?;
        let prefix = port.config.connected_prefix.ok_or(Error::InvalidState)?;
        if !prefix.contains(routed_address)
            || is_link_local(routed_address)
            || port.config.routed_address == Some(routed_address)
            || self.node_by_routed_address(routed_address).is_some()
        {
            return Err(Error::InvalidField);
        }

        let updated = RouterNodeRecord {
            routed_address: Some(routed_address),
            state: NodeState::Configured,
            ..current
        };
        self.nodes.insert(link_local, updated);
        Ok(updated)
    }

    pub fn remove_node(&mut self, link_local: GdpAddress) -> Option<RouterNodeRecord> {
        self.nodes.remove(&link_local)
    }

    /// Process one complete GDP packet through the P4 forwarding pipeline.
    /// Ordinary forwarding decisions are made by P4; Rust only interprets the
    /// P4-selected output as physical egress, CPU punt, or drop.
    pub fn process(
        &mut self,
        ingress_port: RouterPortId,
        packet: GdpPacket,
    ) -> Result<RouterDisposition> {
        if ingress_port >= self.physical_ports || !self.port(ingress_port)?.up {
            return Err(Error::LinkDown);
        }

        let local_prefix = match &packet.header.addresses {
            GdpAddresses::Global { .. } => 0,
            GdpAddresses::Local { prefix, .. } => *prefix,
        };
        let cfg = GdpWireConfig::default();
        let bytes = packet.encode(cfg)?;
        let mut input = packet_in::new(&bytes);
        let outputs = self.pipeline.process_packet(ingress_port, &mut input);

        if outputs.is_empty() {
            return Ok(RouterDisposition::Drop);
        }
        if outputs.len() != 1 {
            return Err(Error::InvalidState);
        }

        let (out, port) = outputs.into_iter().next().unwrap();
        let mut bytes = out.header_data;
        bytes.extend_from_slice(out.payload_data);
        let packet = GdpPacket::decode(&bytes, cfg, local_prefix)?;

        if port == self.cpu_port {
            return Ok(RouterDisposition::Punt {
                ingress_port,
                packet,
            });
        }

        let egress_port = port as RouterPortId;
        if egress_port >= self.physical_ports || !self.port(egress_port)?.up {
            return Ok(RouterDisposition::Drop);
        }
        Ok(RouterDisposition::Forward {
            egress_port,
            packet,
        })
    }

    fn compile_startup_fib(&mut self) -> Result<()> {
        let mut pipeline = main_pipeline::new(self.physical_ports + 1);
        let mut fib = Vec::new();
        let mut programmed_global = BTreeSet::new();

        let bootstrap = GdpPrefix::new(ROUTER_BOOTSTRAP_ADDRESS, 64)
            .map_err(|_| Error::InvalidField)?;
        program_global(
            &mut pipeline,
            bootstrap,
            "punt",
            &self.cpu_port.to_le_bytes(),
        );
        programmed_global.insert(bootstrap);
        fib.push(FibEntry {
            prefix: bootstrap,
            egress_port: None,
            origin: FibOrigin::Bootstrap,
        });

        for port in &self.startup.ports {
            for address in [Some(port.link_local), port.routed_address]
                .into_iter()
                .flatten()
            {
                let prefix = GdpPrefix::new(address, 64).map_err(|_| Error::InvalidField)?;
                if programmed_global.insert(prefix) {
                    program_global(
                        &mut pipeline,
                        prefix,
                        "punt",
                        &self.cpu_port.to_le_bytes(),
                    );
                    fib.push(FibEntry {
                        prefix,
                        egress_port: None,
                        origin: FibOrigin::RouterLocal,
                    });
                }
            }

            program_local_punt(&mut pipeline, port.id, port.link_local.0 as u16, self.cpu_port);
            if let Some(address) = port.routed_address {
                program_local_punt(&mut pipeline, port.id, address.0 as u16, self.cpu_port);
            }
        }

        for port in &self.startup.ports {
            if let Some(prefix) = port.connected_prefix {
                if !programmed_global.insert(prefix) {
                    return Err(Error::InvalidField);
                }
                program_global(
                    &mut pipeline,
                    prefix,
                    "forward",
                    &port.id.to_le_bytes(),
                );
                fib.push(FibEntry {
                    prefix,
                    egress_port: Some(port.id),
                    origin: FibOrigin::Connected,
                });
            }
        }

        for route in &self.startup.routes {
            if !programmed_global.insert(route.prefix) {
                return Err(Error::InvalidField);
            }
            program_global(
                &mut pipeline,
                route.prefix,
                "forward",
                &route.egress_port.to_le_bytes(),
            );
            fib.push(FibEntry {
                prefix: route.prefix,
                egress_port: Some(route.egress_port),
                origin: FibOrigin::Static,
            });
        }

        self.pipeline = pipeline;
        self.fib = fib;
        self.fib_generation = self.fib_generation.wrapping_add(1).max(1);
        Ok(())
    }
}

fn is_link_local(address: GdpAddress) -> bool {
    (address.0 >> 48) as u16 == 0xfe80
}

fn is_address_authority_prefix(prefix: GdpPrefix) -> bool {
    matches!(prefix.prefix_len(), 16 | 32 | 48 | 56)
}

fn program_local_punt(
    pipeline: &mut main_pipeline,
    ingress_port: RouterPortId,
    destination: u16,
    cpu_port: u16,
) {
    let mut key = ingress_port.to_le_bytes().to_vec();
    key.extend_from_slice(&destination.to_le_bytes());
    pipeline.add_table_entry(
        "ingress.local_routes",
        "punt",
        &key,
        &cpu_port.to_le_bytes(),
        0,
    );
}

fn program_global(
    pipeline: &mut main_pipeline,
    prefix: GdpPrefix,
    action: &str,
    params: &[u8],
) {
    let value = prefix.network().0;
    let len = prefix.prefix_len();
    if len > 32 {
        let hi = (value >> 32) as u32;
        let lo = value as u32;
        let mut key = hi.to_le_bytes().to_vec();
        key.extend_from_slice(&lo.to_be_bytes());
        key.push(len - 32);
        pipeline.add_table_entry(
            "ingress.global_long_routes",
            action,
            &key,
            params,
            0,
        );
    } else {
        let hi = (value >> 32) as u32;
        let mut key = hi.to_be_bytes().to_vec();
        key.push(len);
        pipeline.add_table_entry(
            "ingress.global_short_routes",
            action,
            &key,
            params,
            0,
        );
    }
}
