#![cfg(feature = "p4-router")]

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use p4rs::{packet_in, Pipeline};

use crate::error::Error;
use crate::routing::GdpPrefix;
use crate::wire::gdp::{GdpAddress, GdpAddresses, GdpPacket, GdpWireConfig};

p4_macro::use_p4!(
    p4 = "p4-static-router/p4/router.p4",
    pipeline_name = "gnet_static_router",
);

pub type RouterPortId = u16;
pub const ROUTER_PORT_COUNT: u16 = 2;
pub const ROUTER_BOOTSTRAP_ADDRESS: GdpAddress = GdpAddress(0xfe80_0000_0000_0000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterPortConfig {
    pub id: RouterPortId,
    pub link_local: GdpAddress,
    pub connected_prefix: Option<GdpPrefix>,
    pub routed_address: Option<GdpAddress>,
}

impl RouterPortConfig {
    pub const fn new(id: RouterPortId, link_local: GdpAddress) -> Self {
        Self {
            id,
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
    /// Optional link-local adjacency. It selects the local next hop only; the
    /// GDP destination remains the final destination and is never rewritten.
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

    pub fn validate(&self) -> crate::error::Result<()> {
        if self.ports.len() != ROUTER_PORT_COUNT as usize {
            return Err(Error::InvalidField);
        }

        let mut owned_addresses = BTreeSet::new();
        let mut forwarding_prefixes = BTreeSet::new();

        for (index, port) in self.ports.iter().enumerate() {
            if port.id as usize != index {
                return Err(Error::InvalidField);
            }
            if !is_link_local(port.link_local)
                || port.link_local == ROUTER_BOOTSTRAP_ADDRESS
                || !owned_addresses.insert(port.link_local)
            {
                return Err(Error::InvalidField);
            }

            match (port.connected_prefix, port.routed_address) {
                (None, None) => {}
                (Some(prefix), Some(address)) => {
                    if is_link_local(prefix.network())
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
            if route.egress_port >= ROUTER_PORT_COUNT || !forwarding_prefixes.insert(route.prefix) {
                return Err(Error::InvalidField);
            }
            if let Some(next_hop) = route.next_hop {
                if !is_link_local(next_hop)
                    || next_hop == ROUTER_BOOTSTRAP_ADDRESS
                    || owned_addresses.contains(&next_hop)
                {
                    return Err(Error::InvalidField);
                }
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

/// Fixed two-port GNet router with a Rust startup/configuration plane and a P4
/// GDP forwarding plane. Both physical links are assumed available for this
/// milestone; DLP link lifecycle is deliberately outside this object.
pub struct StaticP4Router {
    pipeline: main_pipeline,
    cpu_port: u16,
    startup: RouterStartupConfig,
    fib: Vec<FibEntry>,
}

impl StaticP4Router {
    pub fn new(startup: RouterStartupConfig) -> crate::error::Result<Self> {
        startup.validate()?;
        let cpu_port = ROUTER_PORT_COUNT;
        let mut router = Self {
            pipeline: main_pipeline::new(ROUTER_PORT_COUNT + 1),
            cpu_port,
            startup,
            fib: Vec::new(),
        };
        router.compile_startup_fib()?;
        Ok(router)
    }

    pub const fn physical_port_count(&self) -> u16 {
        ROUTER_PORT_COUNT
    }

    pub const fn cpu_port(&self) -> u16 {
        self.cpu_port
    }

    pub fn startup_config(&self) -> &RouterStartupConfig {
        &self.startup
    }

    pub fn fib(&self) -> &[FibEntry] {
        &self.fib
    }

    /// Run one complete GDP packet through the P4 forwarding plane. For the
    /// first two-port router, forwarding back out the ingress port is rejected
    /// as a hairpin and dropped; every transit packet must cross the router.
    pub fn process(
        &mut self,
        ingress_port: RouterPortId,
        packet: GdpPacket,
    ) -> crate::error::Result<RouterDisposition> {
        if ingress_port >= ROUTER_PORT_COUNT {
            return Err(Error::InvalidField);
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
        if egress_port >= ROUTER_PORT_COUNT || egress_port == ingress_port {
            return Ok(RouterDisposition::Drop);
        }
        Ok(RouterDisposition::Forward {
            egress_port,
            packet,
        })
    }

    fn compile_startup_fib(&mut self) -> crate::error::Result<()> {
        let mut pipeline = main_pipeline::new(ROUTER_PORT_COUNT + 1);
        let mut fib = Vec::new();
        let mut programmed_global = BTreeSet::new();

        let bootstrap = GdpPrefix::new(ROUTER_BOOTSTRAP_ADDRESS, 64)
            .map_err(|_| Error::InvalidField)?;
        program_global(&mut pipeline, bootstrap, "punt", &self.cpu_port.to_le_bytes());
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
                    program_global(&mut pipeline, prefix, "punt", &self.cpu_port.to_le_bytes());
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
                program_global(&mut pipeline, prefix, "forward", &port.id.to_le_bytes());
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
        Ok(())
    }
}

fn is_link_local(address: GdpAddress) -> bool {
    (address.0 >> 48) as u16 == 0xfe80
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
