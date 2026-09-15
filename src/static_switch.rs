#![cfg(feature = "p4-switch")]

use alloc::collections::BTreeMap;

use p4rs::{packet_in, Pipeline};

use crate::error::Error;
use crate::wire::gdp::{GdpAddress, GdpAddresses, GdpPacket, GdpWireConfig};

p4_macro::use_p4!(
    p4 = "p4-static-switch/p4/switch.p4",
    pipeline_name = "gnet_static_switch",
);

pub type SwitchPortId = u16;
pub const SWITCH_PORT_COUNT: u16 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchDisposition {
    Forward {
        egress_port: SwitchPortId,
        packet: GdpPacket,
    },
    Drop,
}

/// Fixed eight-port GNet switch with a Rust management plane and the same
/// x4c/P4 GDP parser/SoftNPU model used elsewhere in smolgnet. Node management
/// programs an exact destination-address table. Once GCTL discovery identifies
/// a router-facing port, unknown global destinations from other ports use that
/// port as the default uplink; packets arriving from the router never bounce
/// back to it.
pub struct StaticP4Switch {
    pipeline: main_pipeline,
    nodes: BTreeMap<GdpAddress, SwitchPortId>,
    default_router_port: Option<SwitchPortId>,
}

impl StaticP4Switch {
    pub fn new() -> Self {
        Self {
            pipeline: main_pipeline::new(SWITCH_PORT_COUNT),
            nodes: BTreeMap::new(),
            default_router_port: None,
        }
    }

    pub const fn physical_port_count(&self) -> u16 {
        SWITCH_PORT_COUNT
    }

    pub fn node_port(&self, address: GdpAddress) -> Option<SwitchPortId> {
        self.nodes.get(&address).copied()
    }

    pub const fn default_router_port(&self) -> Option<SwitchPortId> {
        self.default_router_port
    }

    pub fn set_default_router_port(
        &mut self,
        port: SwitchPortId,
    ) -> crate::error::Result<Option<SwitchPortId>> {
        validate_port(port)?;
        let previous = self.default_router_port.replace(port);
        self.rebuild_pipeline();
        Ok(previous)
    }

    pub fn clear_default_router_port(&mut self) -> Option<SwitchPortId> {
        let previous = self.default_router_port.take();
        if previous.is_some() {
            self.rebuild_pipeline();
        }
        previous
    }

    pub fn register_node(
        &mut self,
        address: GdpAddress,
        port: SwitchPortId,
    ) -> crate::error::Result<Option<SwitchPortId>> {
        validate_port(port)?;
        validate_global_node(address)?;
        let previous = self.nodes.insert(address, port);
        self.rebuild_pipeline();
        Ok(previous)
    }

    pub fn remove_node(&mut self, address: GdpAddress) -> Option<SwitchPortId> {
        let previous = self.nodes.remove(&address);
        if previous.is_some() {
            self.rebuild_pipeline();
        }
        previous
    }

    pub fn process(
        &mut self,
        ingress_port: SwitchPortId,
        packet: GdpPacket,
    ) -> crate::error::Result<SwitchDisposition> {
        validate_port(ingress_port)?;

        let local_prefix = match &packet.header.addresses {
            GdpAddresses::Global { .. } => 0,
            GdpAddresses::Local { prefix, .. } => *prefix,
        };
        let cfg = GdpWireConfig::default();
        let bytes = packet.encode(cfg)?;
        let mut input = packet_in::new(&bytes);
        let outputs = self.pipeline.process_packet(ingress_port, &mut input);

        if outputs.is_empty() {
            return Ok(SwitchDisposition::Drop);
        }
        if outputs.len() != 1 {
            return Err(Error::InvalidState);
        }

        let (out, port) = outputs.into_iter().next().unwrap();
        let egress_port = port as SwitchPortId;
        if egress_port >= SWITCH_PORT_COUNT || egress_port == ingress_port {
            return Ok(SwitchDisposition::Drop);
        }

        let mut bytes = out.header_data;
        bytes.extend_from_slice(out.payload_data);
        let packet = GdpPacket::decode(&bytes, cfg, local_prefix)?;
        Ok(SwitchDisposition::Forward {
            egress_port,
            packet,
        })
    }

    fn rebuild_pipeline(&mut self) {
        let mut pipeline = main_pipeline::new(SWITCH_PORT_COUNT);
        for (&address, &port) in &self.nodes {
            program_global_node(&mut pipeline, address, port);
        }
        if let Some(router_port) = self.default_router_port {
            for ingress_port in 0..SWITCH_PORT_COUNT {
                if ingress_port != router_port {
                    program_default_router(&mut pipeline, ingress_port, router_port);
                }
            }
        }
        self.pipeline = pipeline;
    }
}

impl Default for StaticP4Switch {
    fn default() -> Self {
        Self::new()
    }
}

fn program_global_node(pipeline: &mut main_pipeline, address: GdpAddress, port: SwitchPortId) {
    let hi = (address.0 >> 32) as u32;
    let lo = address.0 as u32;
    let mut key = hi.to_le_bytes().to_vec();
    key.extend_from_slice(&lo.to_le_bytes());
    pipeline.add_table_entry(
        "ingress.global_nodes",
        "forward",
        &key,
        &port.to_le_bytes(),
        0,
    );
}

fn program_default_router(
    pipeline: &mut main_pipeline,
    ingress_port: SwitchPortId,
    router_port: SwitchPortId,
) {
    pipeline.add_table_entry(
        "ingress.default_router",
        "forward",
        &ingress_port.to_le_bytes(),
        &router_port.to_le_bytes(),
        0,
    );
}

fn validate_port(port: SwitchPortId) -> crate::error::Result<()> {
    if port >= SWITCH_PORT_COUNT {
        return Err(Error::InvalidField);
    }
    Ok(())
}

fn validate_global_node(address: GdpAddress) -> crate::error::Result<()> {
    // FE80::/16 is reserved for link-local/control addressing and is not a
    // globally switched node address in this milestone.
    if (address.0 >> 48) as u16 == 0xfe80 {
        return Err(Error::InvalidField);
    }
    Ok(())
}
