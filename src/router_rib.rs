use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::dynamic_routing::{RouteMetric, RouteOrigin, RouterId};
use crate::error::{Error, Result};
use crate::routing::GdpPrefix;
use crate::wire::gctl_routing::{RouteAdvertise, RouteWithdraw};

pub type RoutingPortId = u16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RibRoute {
    pub prefix: GdpPrefix,
    pub egress_port: RoutingPortId,
    pub origin: RouteOrigin,
    pub metric: RouteMetric,
    pub learned_from: Option<RouterId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedRoute {
    pub prefix: GdpPrefix,
    pub egress_port: RoutingPortId,
    pub origin: RouteOrigin,
    pub metric: RouteMetric,
    pub learned_from: Option<RouterId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteUpdate {
    Advertise(RouteAdvertise),
    Withdraw(RouteWithdraw),
}

#[derive(Debug, Clone)]
pub struct RouterRib {
    local_router_id: RouterId,
    routes: Vec<RibRoute>,
}

impl RouterRib {
    pub fn new(local_router_id: RouterId) -> Self {
        Self {
            local_router_id,
            routes: Vec::new(),
        }
    }

    pub const fn local_router_id(&self) -> RouterId {
        self.local_router_id
    }

    pub fn routes(&self) -> &[RibRoute] {
        &self.routes
    }

    pub fn add_connected(&mut self, prefix: GdpPrefix, egress_port: RoutingPortId) {
        self.upsert_local(RibRoute {
            prefix,
            egress_port,
            origin: RouteOrigin::Connected,
            metric: RouteMetric(0),
            learned_from: None,
        });
    }

    pub fn add_static(
        &mut self,
        prefix: GdpPrefix,
        egress_port: RoutingPortId,
        metric: RouteMetric,
    ) {
        self.upsert_local(RibRoute {
            prefix,
            egress_port,
            origin: RouteOrigin::Static,
            metric,
            learned_from: None,
        });
    }

    pub fn receive_advertisement(
        &mut self,
        from: RouterId,
        egress_port: RoutingPortId,
        advertisement: RouteAdvertise,
    ) -> Result<()> {
        if from == self.local_router_id
            || advertisement.advertiser != from
            || matches!(
                advertisement.origin,
                RouteOrigin::Static | RouteOrigin::Escape
            )
        {
            return Err(Error::InvalidField);
        }

        let route = RibRoute {
            prefix: advertisement.prefix,
            egress_port,
            origin: RouteOrigin::Learned,
            metric: advertisement.metric,
            learned_from: Some(from),
        };

        if let Some(existing) = self.routes.iter_mut().find(|existing| {
            existing.prefix == route.prefix
                && existing.origin == RouteOrigin::Learned
                && existing.learned_from == Some(from)
        }) {
            *existing = route;
        } else {
            self.routes.push(route);
        }
        Ok(())
    }

    pub fn receive_withdrawal(
        &mut self,
        from: RouterId,
        withdrawal: RouteWithdraw,
    ) -> Result<bool> {
        if from == self.local_router_id || withdrawal.advertiser != from {
            return Err(Error::InvalidField);
        }

        let before = self.routes.len();
        self.routes.retain(|route| {
            !(route.prefix == withdrawal.prefix
                && route.origin == RouteOrigin::Learned
                && route.learned_from == Some(from))
        });
        Ok(self.routes.len() != before)
    }

    pub fn remove_learned_from(&mut self, neighbor: RouterId) -> Vec<GdpPrefix> {
        let mut changed = BTreeSet::new();
        self.routes.retain(|route| {
            let remove =
                route.origin == RouteOrigin::Learned && route.learned_from == Some(neighbor);
            if remove {
                changed.insert(route.prefix);
            }
            !remove
        });
        changed.into_iter().collect()
    }

    pub fn advertisements_for(
        &self,
        neighbor: RouterId,
        outgoing_metric: RouteMetric,
    ) -> Vec<RouteAdvertise> {
        self.selected_routes()
            .into_iter()
            .filter(|route| route_is_advertisable_to(*route, neighbor))
            .map(|route| self.advertisement(route, outgoing_metric))
            .collect()
    }

    pub fn updates_for(
        &self,
        neighbor: RouterId,
        outgoing_metric: RouteMetric,
        changed_prefixes: &[GdpPrefix],
    ) -> Vec<RouteUpdate> {
        let unique: BTreeSet<GdpPrefix> = changed_prefixes.iter().copied().collect();
        unique
            .into_iter()
            .map(|prefix| match self.selected_for_prefix(prefix) {
                Some(route) if route_is_advertisable_to(route, neighbor) => {
                    RouteUpdate::Advertise(self.advertisement(route, outgoing_metric))
                }
                _ => RouteUpdate::Withdraw(RouteWithdraw {
                    advertiser: self.local_router_id,
                    prefix,
                }),
            })
            .collect()
    }

    pub fn selected_routes(&self) -> Vec<SelectedRoute> {
        let mut winners: BTreeMap<GdpPrefix, RibRoute> = BTreeMap::new();
        for route in self.routes.iter().copied() {
            match winners.get(&route.prefix).copied() {
                None => {
                    winners.insert(route.prefix, route);
                }
                Some(current) if route_better(route, current) => {
                    winners.insert(route.prefix, route);
                }
                Some(_) => {}
            }
        }

        winners.into_values().map(SelectedRoute::from).collect()
    }

    pub fn learned_fib(&self) -> Vec<SelectedRoute> {
        self.selected_routes()
            .into_iter()
            .filter(|route| route.origin == RouteOrigin::Learned)
            .collect()
    }

    fn selected_for_prefix(&self, prefix: GdpPrefix) -> Option<SelectedRoute> {
        self.routes
            .iter()
            .copied()
            .filter(|route| route.prefix == prefix)
            .reduce(|current, candidate| {
                if route_better(candidate, current) {
                    candidate
                } else {
                    current
                }
            })
            .map(SelectedRoute::from)
    }

    fn advertisement(&self, route: SelectedRoute, outgoing_metric: RouteMetric) -> RouteAdvertise {
        RouteAdvertise {
            advertiser: self.local_router_id,
            prefix: route.prefix,
            origin: route.origin,
            metric: route.metric.saturating_add(outgoing_metric),
        }
    }

    fn upsert_local(&mut self, route: RibRoute) {
        if let Some(existing) = self.routes.iter_mut().find(|existing| {
            existing.prefix == route.prefix
                && existing.origin == route.origin
                && existing.learned_from.is_none()
        }) {
            *existing = route;
        } else {
            self.routes.push(route);
        }
    }
}

impl From<RibRoute> for SelectedRoute {
    fn from(route: RibRoute) -> Self {
        Self {
            prefix: route.prefix,
            egress_port: route.egress_port,
            origin: route.origin,
            metric: route.metric,
            learned_from: route.learned_from,
        }
    }
}

fn route_is_advertisable_to(route: SelectedRoute, neighbor: RouterId) -> bool {
    match route.origin {
        RouteOrigin::Connected => true,
        RouteOrigin::Learned => route.learned_from != Some(neighbor),
        RouteOrigin::Static | RouteOrigin::Escape => false,
    }
}

fn route_better(candidate: RibRoute, current: RibRoute) -> bool {
    let candidate_preference = candidate.origin.administrative_preference();
    let current_preference = current.origin.administrative_preference();
    if candidate_preference != current_preference {
        return candidate_preference < current_preference;
    }
    if candidate.metric != current.metric {
        return candidate.metric < current.metric;
    }

    match (candidate.learned_from, current.learned_from) {
        (Some(candidate), Some(current)) => candidate < current,
        (None, Some(_)) => true,
        _ => false,
    }
}
