use crate::time::Instant;
use crate::wire::gdp::GdpAddress;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrefixLengthError;

impl core::fmt::Display for PrefixLengthError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "GDP prefix length must be in 0..=64")
    }
}

impl core::error::Error for PrefixLengthError {}

/// Canonical GDP destination prefix.
///
/// Host bits are cleared when the prefix is constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GdpPrefix {
    network: GdpAddress,
    prefix_len: u8,
}

impl GdpPrefix {
    pub fn new(address: GdpAddress, prefix_len: u8) -> Result<Self, PrefixLengthError> {
        if prefix_len > 64 {
            return Err(PrefixLengthError);
        }

        Ok(Self {
            network: GdpAddress(address.0 & prefix_mask(prefix_len)),
            prefix_len,
        })
    }

    pub const fn network(self) -> GdpAddress {
        self.network
    }

    pub const fn prefix_len(self) -> u8 {
        self.prefix_len
    }

    pub const fn contains(self, address: GdpAddress) -> bool {
        let mask = prefix_mask(self.prefix_len);
        (address.0 & mask) == self.network.0
    }

    pub const fn default_route() -> Self {
        Self {
            network: GdpAddress(0),
            prefix_len: 0,
        }
    }
}

const fn prefix_mask(prefix_len: u8) -> u64 {
    if prefix_len == 0 {
        0
    } else {
        u64::MAX << (64 - prefix_len)
    }
}

/// A GDP prefix routed through a router.
///
/// This intentionally mirrors smoltcp's small route-table model. Interface
/// selection, forwarding state, dynamic routing, and router policy are outside
/// this first smolgnet routing milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub prefix: GdpPrefix,
    pub via_router: GdpAddress,
    /// `None` means preferred forever. Kept for parity with smoltcp's route
    /// model; the initial lookup algorithm does not use this field.
    pub preferred_until: Option<Instant>,
    /// `None` means the route never expires.
    pub expires_at: Option<Instant>,
}

impl Route {
    pub fn new(prefix: GdpPrefix, via_router: GdpAddress) -> Self {
        Self {
            prefix,
            via_router,
            preferred_until: None,
            expires_at: None,
        }
    }

    pub fn new_default(via_router: GdpAddress) -> Self {
        Self::new(GdpPrefix::default_route(), via_router)
    }

    pub fn is_default(self) -> bool {
        self.prefix.prefix_len() == 0
    }

    pub fn is_expired(self, now: Instant) -> bool {
        matches!(self.expires_at, Some(expires_at) if now > expires_at)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteTableFull;

impl core::fmt::Display for RouteTableFull {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "route table full")
    }
}

impl core::error::Error for RouteTableFull {}

/// Fixed-capacity GDP routing table.
///
/// Lookup ignores expired routes and selects the matching route with the
/// longest prefix. Equal-length ties are resolved by insertion order, matching
/// the deliberately small and predictable nature of this initial table.
#[derive(Debug, Clone, Copy)]
pub struct RouteTable<const N: usize> {
    routes: [Option<Route>; N],
    len: usize,
}

impl<const N: usize> Default for RouteTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RouteTable<N> {
    pub const fn new() -> Self {
        Self {
            routes: [None; N],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn add(&mut self, route: Route) -> Result<(), RouteTableFull> {
        for slot in &mut self.routes {
            if slot.is_none() {
                *slot = Some(route);
                self.len += 1;
                return Ok(());
            }
        }
        Err(RouteTableFull)
    }

    pub fn remove(&mut self, prefix: GdpPrefix) -> Option<Route> {
        for slot in &mut self.routes {
            if let Some(route) = *slot
                && route.prefix == prefix
            {
                *slot = None;
                self.len -= 1;
                return Some(route);
            }
        }
        None
    }

    pub fn add_default_route(
        &mut self,
        via_router: GdpAddress,
    ) -> Result<Option<Route>, RouteTableFull> {
        let old = self.remove_default_route();
        match self.add(Route::new_default(via_router)) {
            Ok(()) => Ok(old),
            Err(err) => {
                if let Some(old) = old {
                    let _ = self.add(old);
                }
                Err(err)
            }
        }
    }

    pub fn get_default_route(&self) -> Option<Route> {
        self.routes.iter().flatten().copied().find(|route| route.is_default())
    }

    pub fn remove_default_route(&mut self) -> Option<Route> {
        self.remove(GdpPrefix::default_route())
    }

    pub fn lookup(&self, destination: GdpAddress, now: Instant) -> Option<GdpAddress> {
        let mut best: Option<Route> = None;

        for route in self.routes.iter().flatten().copied() {
            if route.is_expired(now) || !route.prefix.contains(destination) {
                continue;
            }

            if best.map_or(true, |current| {
                route.prefix.prefix_len() > current.prefix.prefix_len()
            }) {
                best = Some(route);
            }
        }

        best.map(|route| route.via_router)
    }

    /// Earliest finite route expiry, used by endpoint scheduling.
    pub fn next_expiry(&self) -> Option<Instant> {
        self.routes
            .iter()
            .flatten()
            .filter_map(|route| route.expires_at)
            .min()
    }

    pub fn prune_expired(&mut self, now: Instant) -> usize {
        let mut removed = 0;
        for slot in &mut self.routes {
            if slot.map_or(false, |route| route.is_expired(now)) {
                *slot = None;
                self.len -= 1;
                removed += 1;
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(v: u64) -> GdpAddress {
        GdpAddress(v)
    }

    #[test]
    fn prefix_is_canonical_and_matches() {
        let prefix = GdpPrefix::new(addr(0x1234_5678_9abc_def0), 48).unwrap();
        assert_eq!(prefix.network(), addr(0x1234_5678_9abc_0000));
        assert!(prefix.contains(addr(0x1234_5678_9abc_0042)));
        assert!(!prefix.contains(addr(0x1234_5678_9abd_0001)));
        assert!(GdpPrefix::new(addr(0), 65).is_err());
    }

    #[test]
    fn longest_prefix_wins_over_default() {
        let mut routes: RouteTable<4> = RouteTable::new();
        routes.add_default_route(addr(1)).unwrap();
        routes
            .add(Route::new(
                GdpPrefix::new(addr(0x1234_0000_0000_0000), 16).unwrap(),
                addr(2),
            ))
            .unwrap();
        routes
            .add(Route::new(
                GdpPrefix::new(addr(0x1234_5600_0000_0000), 24).unwrap(),
                addr(3),
            ))
            .unwrap();

        assert_eq!(
            routes.lookup(addr(0x1234_5678_0000_0001), Instant::ZERO),
            Some(addr(3))
        );
        assert_eq!(
            routes.lookup(addr(0x9999_0000_0000_0001), Instant::ZERO),
            Some(addr(1))
        );
    }

    #[test]
    fn expired_routes_are_ignored() {
        let prefix = GdpPrefix::new(addr(0xaaaa_0000_0000_0000), 16).unwrap();
        let mut routes: RouteTable<2> = RouteTable::new();

        let mut route = Route::new(prefix, addr(10));
        route.expires_at = Some(Instant::from_millis(10));
        routes.add(route).unwrap();
        routes.add_default_route(addr(20)).unwrap();

        assert_eq!(
            routes.lookup(addr(0xaaaa_1234), Instant::from_millis(5)),
            Some(addr(10))
        );
        assert_eq!(
            routes.lookup(addr(0xaaaa_1234), Instant::from_millis(11)),
            Some(addr(20))
        );
    }

    #[test]
    fn earliest_expiry_is_reported() {
        let mut routes: RouteTable<3> = RouteTable::new();
        let mut later = Route::new_default(addr(1));
        later.expires_at = Some(Instant::from_millis(50));
        let mut sooner = Route::new(
            GdpPrefix::new(addr(0xaaaa_0000_0000_0000), 16).unwrap(),
            addr(2),
        );
        sooner.expires_at = Some(Instant::from_millis(20));
        routes.add(later).unwrap();
        routes.add(sooner).unwrap();
        routes
            .add(Route::new(
                GdpPrefix::new(addr(0xbbbb_0000_0000_0000), 16).unwrap(),
                addr(3),
            ))
            .unwrap();

        assert_eq!(routes.next_expiry(), Some(Instant::from_millis(20)));
    }

    #[test]
    fn default_route_can_be_replaced_and_removed() {
        let mut routes: RouteTable<2> = RouteTable::new();
        assert_eq!(routes.add_default_route(addr(1)).unwrap(), None);
        assert_eq!(routes.get_default_route().unwrap().via_router, addr(1));

        let old = routes.add_default_route(addr(2)).unwrap().unwrap();
        assert_eq!(old.via_router, addr(1));
        assert_eq!(routes.get_default_route().unwrap().via_router, addr(2));

        assert_eq!(routes.remove_default_route().unwrap().via_router, addr(2));
        assert!(routes.get_default_route().is_none());
    }

    #[test]
    fn table_capacity_and_pruning_are_bounded() {
        let mut routes: RouteTable<1> = RouteTable::new();
        let mut route = Route::new_default(addr(1));
        route.expires_at = Some(Instant::from_millis(10));
        routes.add(route).unwrap();

        assert_eq!(routes.add(Route::new_default(addr(2))), Err(RouteTableFull));
        assert_eq!(routes.prune_expired(Instant::from_millis(11)), 1);
        assert!(routes.is_empty());
    }
}
