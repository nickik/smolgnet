use crate::dynamic_routing::{LinkId, RouteMetric, RouterId};
use crate::error::{Error, Result};
use crate::time::{Duration, Instant};
use crate::wire::gctl_routing::{RouterHello, RoutingGctlBody, RoutingGctlMessage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeighborState {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterNeighbor {
    pub router_id: RouterId,
    pub remote_link_id: LinkId,
    pub metric: RouteMetric,
    pub hold_time_ms: u32,
    pub last_seen: Instant,
    pub state: NeighborState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterAdjacency {
    local_router_id: RouterId,
    local_link_id: LinkId,
    local_metric: RouteMetric,
    local_hold_time_ms: u32,
    neighbor: Option<RouterNeighbor>,
}

impl RouterAdjacency {
    pub fn new(
        local_router_id: RouterId,
        local_link_id: LinkId,
        local_metric: RouteMetric,
        local_hold_time_ms: u32,
    ) -> Result<Self> {
        if local_hold_time_ms == 0 {
            return Err(Error::InvalidField);
        }
        Ok(Self {
            local_router_id,
            local_link_id,
            local_metric,
            local_hold_time_ms,
            neighbor: None,
        })
    }

    pub const fn local_router_id(&self) -> RouterId {
        self.local_router_id
    }

    pub const fn local_link_id(&self) -> LinkId {
        self.local_link_id
    }

    pub const fn neighbor(&self) -> Option<RouterNeighbor> {
        self.neighbor
    }

    pub const fn state(&self) -> NeighborState {
        match self.neighbor {
            Some(neighbor) => neighbor.state,
            None => NeighborState::Down,
        }
    }

    pub fn hello(&self, transaction_id: u32) -> RoutingGctlMessage {
        RoutingGctlMessage::router_hello(transaction_id, self.local_hello())
    }

    pub fn receive(
        &mut self,
        now: Instant,
        message: RoutingGctlMessage,
    ) -> Result<Option<RoutingGctlMessage>> {
        match message.body {
            RoutingGctlBody::RouterHello(peer) => {
                self.accept_peer(now, peer)?;
                Ok(Some(RoutingGctlMessage::router_hello_ack(
                    message.transaction_id,
                    self.local_hello(),
                )))
            }
            RoutingGctlBody::RouterHelloAck(peer) => {
                self.accept_peer(now, peer)?;
                Ok(None)
            }
            RoutingGctlBody::RouteAdvertise(_) | RoutingGctlBody::RouteWithdraw(_) => {
                Err(Error::Unsupported)
            }
        }
    }

    pub fn expire(&mut self, now: Instant) -> bool {
        let Some(mut neighbor) = self.neighbor else {
            return false;
        };
        if neighbor.state == NeighborState::Down {
            return false;
        }

        let elapsed = now.saturating_duration_since(neighbor.last_seen);
        if elapsed >= Duration::from_millis(neighbor.hold_time_ms as u64) {
            neighbor.state = NeighborState::Down;
            self.neighbor = Some(neighbor);
            true
        } else {
            false
        }
    }

    fn local_hello(&self) -> RouterHello {
        RouterHello {
            router_id: self.local_router_id,
            link_id: self.local_link_id,
            hold_time_ms: self.local_hold_time_ms,
            metric: self.local_metric,
        }
    }

    fn accept_peer(&mut self, now: Instant, peer: RouterHello) -> Result<()> {
        if peer.router_id == self.local_router_id || peer.hold_time_ms == 0 {
            return Err(Error::InvalidField);
        }
        self.neighbor = Some(RouterNeighbor {
            router_id: peer.router_id,
            remote_link_id: peer.link_id,
            metric: peer.metric,
            hold_time_ms: peer.hold_time_ms,
            last_seen: now,
            state: NeighborState::Up,
        });
        Ok(())
    }
}
