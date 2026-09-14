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
/// Host bits are always cleared when the prefix is constructed.
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
        let mask = prefix_mask(prefix_len);
        Ok(Self {
            network: GdpAddress(address.0 & mask),
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

/// Stable index identifying a native GNet router interface/link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InterfaceHandle(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOrigin {
    Connected,
    Static,
    Dynamic(u16),
}

/// One RIB/FIB entry.
///
/// `next_hop == None` means the destination is directly reachable on the
/// selected interface. Otherwise the frame is sent toward the named GDP router.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub prefix: GdpPrefix,
    pub next_hop: Option<GdpAddress>,
    pub interface: InterfaceHandle,
    pub origin: RouteOrigin,
    /// Lower values are preferred among routes with the same prefix length.
    pub preference: u16,
    /// Lower values are preferred after administrative preference.
    pub metric: u32,
    /// `None` means preferred forever. A deprecated route remains usable until
    /// expiry, but loses to an otherwise-equivalent preferred route.
    pub preferred_until: Option<Instant>,
    /// `None` means the route never expires.
    pub expires_at: Option<Instant>,
}

impl Route {
    pub fn connected(prefix: GdpPrefix, interface: InterfaceHandle) -> Self {
        Self {
            prefix,
            next_hop: None,
            interface,
            origin: RouteOrigin::Connected,
            preference: 0,
            metric: 0,
            preferred_until: None,
            expires_at: None,
        }
    }

    pub fn static_via(
        prefix: GdpPrefix,
        next_hop: GdpAddress,
        interface: InterfaceHandle,
    ) -> Self {
        Self {
            prefix,
            next_hop: Some(next_hop),
            interface,
            origin: RouteOrigin::Static,
            preference: 100,
            metric: 0,
            preferred_until: None,
            expires_at: None,
        }
    }

    pub fn default_via(next_hop: GdpAddress, interface: InterfaceHandle) -> Self {
        Self::static_via(GdpPrefix::default_route(), next_hop, interface)
    }

    pub fn is_expired(self, now: Instant) -> bool {
        matches!(self.expires_at, Some(expires_at) if now > expires_at)
    }

    pub fn is_preferred(self, now: Instant) -> bool {
        !matches!(self.preferred_until, Some(until) if now > until)
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

/// Fixed-capacity routing table suitable for `no_std` routers and endpoints.
///
/// Lookup order is:
/// 1. longest destination prefix;
/// 2. non-deprecated route;
/// 3. lower administrative preference;
/// 4. lower metric;
/// 5. lower interface handle, for deterministic final tie-breaking.
#[derive(Debug, Clone)]
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

    pub fn remove(&mut self, prefix: GdpPrefix, interface: InterfaceHandle) -> Option<Route> {
        for slot in &mut self.routes {
            if let Some(route) = *slot
                && route.prefix == prefix
                && route.interface == interface
            {
                *slot = None;
                self.len -= 1;
                return Some(route);
            }
        }
        None
    }

    pub fn set_default(
        &mut self,
        next_hop: GdpAddress,
        interface: InterfaceHandle,
    ) -> Result<Option<Route>, RouteTableFull> {
        let old = self.remove(GdpPrefix::default_route(), interface);
        match self.add(Route::default_via(next_hop, interface)) {
            Ok(()) => Ok(old),
            Err(err) => {
                if let Some(old) = old {
                    let _ = self.add(old);
                }
                Err(err)
            }
        }
    }

    pub fn default_for_interface(&self, interface: InterfaceHandle) -> Option<Route> {
        self.routes.iter().flatten().copied().find(|route| {
            route.prefix.prefix_len() == 0 && route.interface == interface
        })
    }

    pub fn lookup(&self, destination: GdpAddress, now: Instant) -> Option<Route> {
        let mut best: Option<Route> = None;
        for route in self.routes.iter().flatten().copied() {
            if route.is_expired(now) || !route.prefix.contains(destination) {
                continue;
            }
            if best.map_or(true, |current| route_better(route, current, now)) {
                best = Some(route);
            }
        }
        best
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

fn route_better(candidate: Route, current: Route, now: Instant) -> bool {
    candidate.prefix.prefix_len() > current.prefix.prefix_len()
        || (candidate.prefix.prefix_len() == current.prefix.prefix_len()
            && (candidate.is_preferred(now), core::cmp::Reverse(candidate.preference), core::cmp::Reverse(candidate.metric), core::cmp::Reverse(candidate.interface))
                > (current.is_preferred(now), core::cmp::Reverse(current.preference), core::cmp::Reverse(current.metric), core::cmp::Reverse(current.interface)))
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
        let mut table: RouteTable<4> = RouteTable::new();
        table.add(Route::default_via(addr(1), InterfaceHandle(0))).unwrap();
        table.add(Route::static_via(
            GdpPrefix::new(addr(0x1234_0000_0000_0000), 16).unwrap(),
            addr(2),
            InterfaceHandle(1),
        )).unwrap();
        table.add(Route::static_via(
            GdpPrefix::new(addr(0x1234_5600_0000_0000), 24).unwrap(),
            addr(3),
            InterfaceHandle(2),
        )).unwrap();

        let route = table.lookup(addr(0x1234_5678_0000_0001), Instant::ZERO).unwrap();
        assert_eq!(route.next_hop, Some(addr(3)));
        assert_eq!(route.interface, InterfaceHandle(2));

        let fallback = table.lookup(addr(0x9999_0000_0000_0001), Instant::ZERO).unwrap();
        assert_eq!(fallback.next_hop, Some(addr(1)));
    }

    #[test]
    fn expiry_and_preference_are_honored() {
        let prefix = GdpPrefix::new(addr(0xaaaa_0000_0000_0000), 16).unwrap();
        let mut table: RouteTable<4> = RouteTable::new();

        let mut deprecated = Route::static_via(prefix, addr(10), InterfaceHandle(1));
        deprecated.preferred_until = Some(Instant::from_millis(10));
        deprecated.expires_at = Some(Instant::from_millis(100));
        deprecated.preference = 10;

        let mut preferred = Route::static_via(prefix, addr(20), InterfaceHandle(2));
        preferred.preference = 100;

        table.add(deprecated).unwrap();
        table.add(preferred).unwrap();

        assert_eq!(table.lookup(addr(0xaaaa_1234), Instant::from_millis(5)).unwrap().next_hop, Some(addr(10)));
        assert_eq!(table.lookup(addr(0xaaaa_1234), Instant::from_millis(20)).unwrap().next_hop, Some(addr(20)));

        preferred.expires_at = Some(Instant::from_millis(30));
    }

    #[test]
    fn table_capacity_and_pruning_are_bounded() {
        let mut table: RouteTable<1> = RouteTable::new();
        let mut route = Route::default_via(addr(1), InterfaceHandle(0));
        route.expires_at = Some(Instant::from_millis(10));
        table.add(route).unwrap();
        assert_eq!(table.add(Route::default_via(addr(2), InterfaceHandle(1))), Err(RouteTableFull));
        assert_eq!(table.prune_expired(Instant::from_millis(11)), 1);
        assert!(table.is_empty());
    }
}
