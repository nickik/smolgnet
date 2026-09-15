use ::core::ops::{Deref, DerefMut};

use crate::error::{Error, Result};
use crate::routing::RouteTable;
use crate::time::Instant;
use crate::wire::css::ServiceSelector;
use crate::wire::gctl::GctlMessage;
use crate::wire::gdp::{GdpAddress, GdpPacket, GdpWireConfig, SizeClass};
use crate::wire::gts::StreamProfile;

mod core {
    include!("endpoint/core.rs");
    include!("endpoint/link_api.rs");
}

pub use core::{
    make_link_local, AddressAuthorityConfig, AddressState, EndpointConfig,
    ListenerConfig, TunnelHandle, BOOTSTRAP_ADDRESS, LINK_LOCAL_PREFIX,
};
#[doc(hidden)]
pub use core::Endpoint as EndpointCore;

/// Small endpoint-local routing table, deliberately comparable in scope to
/// smoltcp's interface routing table rather than a router/control-plane RIB.
pub const ENDPOINT_ROUTE_CAPACITY: usize = 8;

/// Alloc-backed GNet endpoint with optional static/default route lookup.
///
/// Routing remains intentionally small:
/// - an empty table preserves legacy point-to-point direct delivery;
/// - once any route is configured, the route table is authoritative;
/// - GDP destination addresses are never rewritten by route selection.
#[derive(Debug, Clone)]
pub struct Endpoint {
    inner: EndpointCore,
    routes: RouteTable<ENDPOINT_ROUTE_CAPACITY>,
}

impl Endpoint {
    pub fn new(address: GdpAddress, config: EndpointConfig) -> Result<Self> {
        Ok(Self {
            inner: EndpointCore::new(address, config)?,
            routes: RouteTable::new(),
        })
    }

    pub fn unconfigured(link_local_suffix: u64, config: EndpointConfig) -> Result<Self> {
        Ok(Self {
            inner: EndpointCore::unconfigured(link_local_suffix, config)?,
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
    pub fn route(&self, destination: GdpAddress, now: Instant) -> Option<GdpAddress> {
        self.routes.lookup(destination, now)
    }

    /// Resolve the adjacent peer to which a packet should be sent.
    ///
    /// With no configured routes, preserve the historical point-to-point
    /// behavior and send directly to the GDP destination. Once routing is
    /// configured, the table is authoritative and an unmatched destination is
    /// unreachable. A directly reachable routed destination can be represented
    /// with a /64 route whose `via_router` equals the destination itself.
    pub fn next_hop(&self, destination: GdpAddress, now: Instant) -> Option<GdpAddress> {
        if self.routes.is_empty() {
            Some(destination)
        } else {
            self.route(destination, now)
        }
    }

    fn require_next_hop(&self, destination: GdpAddress, now: Instant) -> Result<GdpAddress> {
        self.next_hop(destination, now).ok_or(Error::NoRoute)
    }

    /// Route-aware GTS tunnel establishment.
    ///
    /// The selected next hop is the adjacent peer on the single DLP link. The
    /// GDP destination remains `remote`; routing never rewrites endpoint
    /// identity.
    pub fn connect_at(
        &mut self,
        remote: GdpAddress,
        css: ServiceSelector,
        profile: StreamProfile,
        now: Instant,
    ) -> Result<TunnelHandle> {
        let _next_hop = self.require_next_hop(remote, now)?;
        self.inner.connect(remote, css, profile)
    }

    /// Route-aware GCTL transmission with an explicit routing timestamp.
    pub fn send_gctl_at(
        &mut self,
        remote: GdpAddress,
        msg: GctlMessage,
        class: SizeClass,
        now: Instant,
    ) -> Result<()> {
        let _next_hop = self.require_next_hop(remote, now)?;
        self.inner.send_gctl(remote, msg, class)
    }

    /// Route-aware GCTL echo transmission with an explicit routing timestamp.
    pub fn send_echo_at(
        &mut self,
        remote: GdpAddress,
        transaction_id: u32,
        data: &[u8],
        class: SizeClass,
        now: Instant,
    ) -> Result<()> {
        let _next_hop = self.require_next_hop(remote, now)?;
        self.inner.send_echo(remote, transaction_id, data, class)
    }
}

impl Deref for Endpoint {
    type Target = EndpointCore;

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

    fn endpoint() -> Endpoint {
        Endpoint::new(
            GdpAddress(0x0102_0304_0000_0001),
            EndpointConfig::new(256),
        )
        .unwrap()
    }

    #[test]
    fn direct_send_is_preserved_without_routes() {
        let endpoint = endpoint();
        let destination = GdpAddress(0x9999_0000_0000_0001);
        assert!(endpoint.routes().is_empty());
        assert_eq!(endpoint.route(destination, Instant::ZERO), None);
        assert_eq!(endpoint.next_hop(destination, Instant::ZERO), Some(destination));
    }

    #[test]
    fn endpoint_default_router_is_selected() {
        let mut endpoint = endpoint();
        let router = GdpAddress(0x0102_0304_0000_00fe);
        endpoint.routes_mut().add_default_route(router).unwrap();

        assert_eq!(
            endpoint.next_hop(GdpAddress(0x9999_0000_0000_0001), Instant::ZERO),
            Some(router)
        );
    }

    #[test]
    fn endpoint_specific_route_overrides_default() {
        let mut endpoint = endpoint();
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
            endpoint.next_hop(GdpAddress(0x1234_abcd_0000_0001), Instant::ZERO),
            Some(specific_router)
        );
        assert_eq!(
            endpoint.next_hop(GdpAddress(0x9999_0000_0000_0001), Instant::ZERO),
            Some(default_router)
        );
    }

    #[test]
    fn expired_specific_route_falls_back_to_default() {
        let mut endpoint = endpoint();
        let default_router = GdpAddress(0x0102_0304_0000_00fe);
        let specific_router = GdpAddress(0x0102_0304_0000_00fd);
        let prefix = GdpPrefix::new(GdpAddress(0x1234_0000_0000_0000), 16).unwrap();
        let mut specific = Route::new(prefix, specific_router);
        specific.expires_at = Some(Instant::from_millis(10));

        endpoint.routes_mut().add_default_route(default_router).unwrap();
        endpoint.routes_mut().add(specific).unwrap();

        assert_eq!(
            endpoint.next_hop(GdpAddress(0x1234_abcd_0000_0001), Instant::from_millis(5)),
            Some(specific_router)
        );
        assert_eq!(
            endpoint.next_hop(GdpAddress(0x1234_abcd_0000_0001), Instant::from_millis(11)),
            Some(default_router)
        );
    }

    #[test]
    fn configured_table_without_match_reports_no_route() {
        let mut endpoint = endpoint();
        endpoint
            .routes_mut()
            .add(Route::new(
                GdpPrefix::new(GdpAddress(0x1234_0000_0000_0000), 16).unwrap(),
                GdpAddress(0x0102_0304_0000_00fd),
            ))
            .unwrap();

        let destination = GdpAddress(0x9999_0000_0000_0001);
        assert_eq!(endpoint.next_hop(destination, Instant::ZERO), None);

        let css = ServiceSelector::registered(1).unwrap();
        let profile = StreamProfile::reliable_variable(
            SizeClass::Ctrl64,
            crate::wire::gts::Direction::Bidirectional,
        );
        assert_eq!(
            endpoint.connect_at(destination, css, profile, Instant::ZERO),
            Err(Error::NoRoute)
        );
    }

    #[test]
    fn route_aware_send_keeps_original_gdp_destination() {
        let mut endpoint = endpoint();
        let destination = GdpAddress(0x1234_abcd_0000_0001);
        let router = GdpAddress(0x0102_0304_0000_00fe);
        endpoint.routes_mut().add_default_route(router).unwrap();
        assert_eq!(endpoint.next_hop(destination, Instant::ZERO), Some(router));

        endpoint
            .send_echo_at(
                destination,
                7,
                b"route",
                SizeClass::Ctrl32,
                Instant::ZERO,
            )
            .unwrap();

        endpoint.dlp_mut().grant_control_tx_credit(32);
        let frame = endpoint.dlp_mut().poll_tx_frame().unwrap().unwrap();
        let packet = GdpPacket::decode(&frame.bytes, GdpWireConfig::default(), 0).unwrap();
        assert_eq!(packet.header.addresses.effective_destination(), destination);
        assert_ne!(packet.header.addresses.effective_destination(), router);
    }
}
