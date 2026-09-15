#![cfg(feature = "p4-router")]

use crate::{
    GdpPacket, Result, RouterDisposition, RouterPortId, RouterStartupConfig, StaticP4Router,
};

/// Controls whether VC0 is part of the ordinary data VC pool or held back as
/// an escape resource for breaking wormhole channel-dependency cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterVc0Policy {
    /// VC0 may be chosen for ordinary traffic just like VC1-VC3.
    ///
    /// This maximizes the normal VC pool but means VC0 cannot be relied upon as
    /// an always-available escape resource when a dependency cycle forms.
    NormalData,
    /// Ordinary traffic uses VC1-VC3. VC0 is reserved for escape routing.
    ///
    /// Reserving VC0 is only one half of deadlock avoidance: once a packet
    /// enters VC0, the escape routing function itself must be acyclic and the
    /// packet must not transition back to the adaptive/ordinary VC set.
    EscapeOnly,
}

impl Default for RouterVc0Policy {
    fn default() -> Self {
        Self::EscapeOnly
    }
}

/// Static two-port P4 router plus its wormhole VC allocation policy.
///
/// The P4 forwarding plane still decides the physical egress port. This wrapper
/// owns the link-resource policy used by wormhole-capable router/link hardware.
/// Keeping the policy next to the router makes it explicit and testable without
/// teaching GDP or GTS about numeric VCIDs.
pub struct CycleAwareRouter {
    inner: StaticP4Router,
    vc0_policy: RouterVc0Policy,
}

impl CycleAwareRouter {
    pub fn new(startup: RouterStartupConfig, vc0_policy: RouterVc0Policy) -> Result<Self> {
        Ok(Self {
            inner: StaticP4Router::new(startup)?,
            vc0_policy,
        })
    }

    pub const fn vc0_policy(&self) -> RouterVc0Policy {
        self.vc0_policy
    }

    /// VCIDs available to ordinary routed data.
    pub const fn normal_data_vcids(&self) -> &'static [u8] {
        match self.vc0_policy {
            RouterVc0Policy::NormalData => &[0, 1, 2, 3],
            RouterVc0Policy::EscapeOnly => &[1, 2, 3],
        }
    }

    /// Dedicated escape VC, if one is reserved by this router.
    pub const fn escape_vcid(&self) -> Option<u8> {
        match self.vc0_policy {
            RouterVc0Policy::NormalData => None,
            RouterVc0Policy::EscapeOnly => Some(0),
        }
    }

    pub const fn physical_port_count(&self) -> u16 {
        self.inner.physical_port_count()
    }

    pub fn startup_config(&self) -> &RouterStartupConfig {
        self.inner.startup_config()
    }

    pub fn process(
        &mut self,
        ingress_port: RouterPortId,
        packet: GdpPacket,
    ) -> Result<RouterDisposition> {
        self.inner.process(ingress_port, packet)
    }

    pub fn inner(&self) -> &StaticP4Router {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut StaticP4Router {
        &mut self.inner
    }
}
