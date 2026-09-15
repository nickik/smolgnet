#![cfg(feature = "p4-switch")]

use alloc::collections::BTreeMap;

use crate::error::Error;
use crate::wire::gdp::{GdpAddress, GdpAddresses, GdpPacket};

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

/// Fixed eight-port GNet switch management model.
///
/// The first milestone deliberately keeps node registration explicit. The P4
/// fast path added next will consume the same exact node-to-port table; GS3
/// NODE_ANNOUNCE and DLP/GC3 lifecycle remain outside this object for now.
pub struct StaticP4Switch {
    nodes: BTreeMap<GdpAddress, SwitchPortId>,
}

impl StaticP4Switch {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
        }
    }

    pub const fn physical_port_count(&self) -> u16 {
        SWITCH_PORT_COUNT
    }

    pub fn node_port(&self, address: GdpAddress) -> Option<SwitchPortId> {
        self.nodes.get(&address).copied()
    }

    pub fn register_node(
        &mut self,
        address: GdpAddress,
        port: SwitchPortId,
    ) -> crate::error::Result<Option<SwitchPortId>> {
        validate_port(port)?;
        validate_global_node(address)?;
        Ok(self.nodes.insert(address, port))
    }

    pub fn remove_node(&mut self, address: GdpAddress) -> Option<SwitchPortId> {
        self.nodes.remove(&address)
    }

    pub fn process(
        &self,
        ingress_port: SwitchPortId,
        packet: GdpPacket,
    ) -> crate::error::Result<SwitchDisposition> {
        validate_port(ingress_port)?;

        let destination = match packet.header.addresses {
            GdpAddresses::Global { destination, .. } => destination,
            GdpAddresses::Local { .. } => return Ok(SwitchDisposition::Drop),
        };

        let Some(egress_port) = self.node_port(destination) else {
            return Ok(SwitchDisposition::Drop);
        };
        if egress_port == ingress_port {
            return Ok(SwitchDisposition::Drop);
        }

        Ok(SwitchDisposition::Forward {
            egress_port,
            packet,
        })
    }
}

impl Default for StaticP4Switch {
    fn default() -> Self {
        Self::new()
    }
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
