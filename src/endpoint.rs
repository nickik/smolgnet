use core::ops::{Deref, DerefMut};

use crate::error::Result;
use crate::routing::RouteTable;
use crate::time::Instant;
use crate::wire::gdp::GdpAddress;

mod core {
    include!("endpoint/core.rs");
    include!("endpoint/link_api.rs");
}

pub use core::{
    make_link_local, AddressAuthorityConfig, AddressState, EndpointConfig,
    ListenerConfig, TunnelHandle, BOOTSTRAP_ADDRESS, LINK_LOCAL_PREFIX,
};

/// Small endpoint-local routing table, deliberately comparable in scope to
/// smoltcp's interface routing table rather than a router/control-plane RIB.
pub const ENDPOINT_ROUTE_CAPACITY: usize = 8;

/// Alloc-backed GNet endpoint with optional static/default route lookup.
///
/// Existing endpoint behavior is unchanged when the route table is empty.
/// Routes are currently exposed for next-hop selection only; packet TX is not
/// silently redirected until on-link/direct-destination semantics are defined.
#[derive(Debug, Clone)]
pub struct Endpoint {
    inner: core::Endpoint,
    routes: RouteTable<ENDPOINT_ROUTE_CAPACITY>,
}

impl Endpoint {
    pub fn new(address: GdpAddress, config: EndpointConfig) -> Result<Self> {
        Ok(Self {
            inner: core::Endpoint::new(address, config)?,
            routes: RouteTable::new(),
        })
    }

    pub fn unconfigured(link_local_suffix: u64, config: EndpointConfig) -> Result<Self> {
        Ok(Self {
            inner: core::Endpoint::unconfigured(link_local_suffix, config)?,
            routes: RouteTable::new(),
        })
    }

    pub const fn routes(&self) -> &RouteTable<ENDPOINT_ROUTE_CAPACITY> {
        &self.routes
    }

    pub fn routes_mut(&mut self) -> &mut RouteTable<ENDPOINT_ROUTE_CAPACITY> {
        &mut self.routes
    }

    /// Look up a configured next-hop router for `destination`.
    ///
    /// `None` means no configured route matched. It does not mean the
    /// destination is unreachable: current point-to-point behavior may still
    /// send directly to the destination.
    pub fn route(&self, destination: GdpAddress, now: Instant) -> Option<GdpAddress> {
        self.routes.lookup(destination, now)
    }
}

impl Deref for Endpoint {
    type Target = core::Endpoint;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Endpoint {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::{GdpPrefix, Route};

    #[test]
    fn endpoint_specific_route_overrides_default() {
        let mut endpoint = Endpoint::new(
            GdpAddress(0x0102_0304_0000_0001),
            EndpointConfig::new(256),
        )
        .unwrap();

        let default_router = GdpAddress(0x0102_0304_0000_00fe);
        let specific_router = GdpAddress(0x0102_0304_0000_00fd);

        endpoint
            .routes_mut()
            .add_default_route(default_router)
            .unwrap();
        endpoint
            .routes_mut()
            .add(Route::new(
                GdpPrefix::new(GdpAddress(0x1234_0000_0000_0000), 16).unwrap(),
                specific_router,
            ))
            .unwrap();

        assert_eq!(
            endpoint.route(GdpAddress(0x1234_abcd_0000_0001), Instant::ZERO),
            Some(specific_router)
        );
        assert_eq!(
            endpoint.route(GdpAddress(0x9999_0000_0000_0001), Instant::ZERO),
            Some(default_router)
        );
    }

    #[test]
    fn endpoint_routes_are_optional_when_table_is_empty() {
        let endpoint = Endpoint::new(
            GdpAddress(0x0102_0304_0000_0001),
            EndpointConfig::new(256),
        )
        .unwrap();

        assert!(endpoint.routes().is_empty());
        assert_eq!(
            endpoint.route(GdpAddress(0x9999_0000_0000_0001), Instant::ZERO),
            None
        );
    }
}
