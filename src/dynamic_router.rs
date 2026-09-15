#![cfg(feature = "p4-router")]

use alloc::vec::Vec;

use crate::dynamic_routing::{RouteMetric, RouteOrigin, RouterId};
use crate::error::{Error, Result};
use crate::router_adjacency::RouterAdjacency;
use crate::router_rib::{RouteUpdate, RouterRib, SelectedRoute};
use crate::static_router::{
    RouterDisposition, RouterPortConfig, RouterPortId, RouterStartupConfig, StaticP4Router,
    StaticRouteConfig,
};
use crate::time::Instant;
use crate::wire::gctl_routing::{RouteAdvertise, RouteWithdraw};
use crate::wire::gdp::GdpPacket;

pub struct DynamicP4Router {
    router_id: RouterId,
    ports: Vec<RouterPortConfig>,
    configured_static: Vec<StaticRouteConfig>,
    rib: RouterRib,
    forwarding: StaticP4Router,
}

impl DynamicP4Router {
    pub fn new(
        router_id: RouterId,
        ports: Vec<RouterPortConfig>,
        configured_static: Vec<StaticRouteConfig>,
    ) -> Result<Self> {
        let mut rib = RouterRib::new(router_id);
        for port in &ports {
            if let Some(prefix) = port.connected_prefix {
                rib.add_connected(prefix, port.id);
            }
        }
        for route in &configured_static {
            rib.add_static(route.prefix, route.egress_port, RouteMetric(0));
        }

        let forwarding = build_forwarding(&ports, &configured_static, &rib.learned_fib())?;
        Ok(Self {
            router_id,
            ports,
            configured_static,
            rib,
            forwarding,
        })
    }

    pub const fn router_id(&self) -> RouterId {
        self.router_id
    }

    pub fn rib(&self) -> &RouterRib {
        &self.rib
    }

    pub fn selected_fib(&self) -> Vec<SelectedRoute> {
        self.rib.selected_routes()
    }

    pub fn advertisements_for(
        &self,
        neighbor: RouterId,
        outgoing_metric: RouteMetric,
    ) -> Vec<RouteAdvertise> {
        self.rib.advertisements_for(neighbor, outgoing_metric)
    }

    pub fn updates_for(
        &self,
        neighbor: RouterId,
        outgoing_metric: RouteMetric,
        changed_prefixes: &[crate::routing::GdpPrefix],
    ) -> Vec<RouteUpdate> {
        self.rib
            .updates_for(neighbor, outgoing_metric, changed_prefixes)
    }

    pub fn receive_advertisement(
        &mut self,
        from: RouterId,
        egress_port: RouterPortId,
        advertisement: RouteAdvertise,
    ) -> Result<()> {
        self.rib
            .receive_advertisement(from, egress_port, advertisement)?;
        self.rebuild_forwarding()
    }

    pub fn receive_withdrawal(
        &mut self,
        from: RouterId,
        withdrawal: RouteWithdraw,
    ) -> Result<bool> {
        let removed = self.rib.receive_withdrawal(from, withdrawal)?;
        if removed {
            self.rebuild_forwarding()?;
        }
        Ok(removed)
    }

    pub fn neighbor_down(&mut self, neighbor: RouterId) -> Result<Vec<crate::routing::GdpPrefix>> {
        let changed = self.rib.remove_learned_from(neighbor);
        if !changed.is_empty() {
            self.rebuild_forwarding()?;
        }
        Ok(changed)
    }

    pub fn expire_adjacency(
        &mut self,
        adjacency: &mut RouterAdjacency,
        now: Instant,
    ) -> Result<Vec<crate::routing::GdpPrefix>> {
        if adjacency.local_router_id() != self.router_id {
            return Err(Error::InvalidField);
        }
        let neighbor = adjacency.neighbor().map(|neighbor| neighbor.router_id);
        if !adjacency.expire(now) {
            return Ok(Vec::new());
        }
        match neighbor {
            Some(neighbor) => self.neighbor_down(neighbor),
            None => Ok(Vec::new()),
        }
    }

    pub fn process(
        &mut self,
        ingress_port: RouterPortId,
        packet: GdpPacket,
    ) -> Result<RouterDisposition> {
        self.forwarding.process(ingress_port, packet)
    }

    pub fn learned_routes(&self) -> Vec<SelectedRoute> {
        self.rib
            .selected_routes()
            .into_iter()
            .filter(|route| route.origin == RouteOrigin::Learned)
            .collect()
    }

    fn rebuild_forwarding(&mut self) -> Result<()> {
        let learned = self.rib.learned_fib();
        self.forwarding = build_forwarding(&self.ports, &self.configured_static, &learned)?;
        Ok(())
    }
}

fn build_forwarding(
    ports: &[RouterPortConfig],
    configured_static: &[StaticRouteConfig],
    learned: &[SelectedRoute],
) -> Result<StaticP4Router> {
    let mut routes = configured_static.to_vec();
    for route in learned {
        if route.origin != RouteOrigin::Learned {
            return Err(Error::InvalidField);
        }
        if configured_static
            .iter()
            .any(|configured| configured.prefix == route.prefix)
        {
            continue;
        }
        routes.push(StaticRouteConfig::direct(route.prefix, route.egress_port));
    }
    StaticP4Router::new(RouterStartupConfig::new(ports.to_vec(), routes))
}
