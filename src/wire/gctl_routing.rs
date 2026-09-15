use alloc::vec;
use alloc::vec::Vec;

use crate::dynamic_routing::{LinkId, RouteMetric, RouteOrigin, RouterId};
use crate::error::{Error, Result};
use crate::routing::GdpPrefix;
use crate::wire::gdp::GdpAddress;

pub const GCTL_ROUTER_HELLO: u8 = 0x40;
pub const GCTL_ROUTER_HELLO_ACK: u8 = 0x41;
pub const GCTL_ROUTE_ADVERTISE: u8 = 0x42;
pub const GCTL_ROUTE_WITHDRAW: u8 = 0x43;
pub const GCTL_ROUTING_VERSION: u8 = 1;
pub const GCTL_HEADER_LEN: usize = 8;
pub const ROUTER_HELLO_BODY_LEN: usize = 24;
pub const ROUTE_ADVERTISE_BODY_LEN: usize = 24;
pub const ROUTE_WITHDRAW_BODY_LEN: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterHello {
    pub router_id: RouterId,
    pub link_id: LinkId,
    pub hold_time_ms: u32,
    pub metric: RouteMetric,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteAdvertise {
    pub advertiser: RouterId,
    pub prefix: GdpPrefix,
    pub origin: RouteOrigin,
    pub metric: RouteMetric,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteWithdraw {
    pub advertiser: RouterId,
    pub prefix: GdpPrefix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingGctlBody {
    RouterHello(RouterHello),
    RouterHelloAck(RouterHello),
    RouteAdvertise(RouteAdvertise),
    RouteWithdraw(RouteWithdraw),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutingGctlMessage {
    pub transaction_id: u32,
    pub body: RoutingGctlBody,
}

impl RoutingGctlMessage {
    pub fn router_hello(transaction_id: u32, hello: RouterHello) -> Self {
        Self {
            transaction_id,
            body: RoutingGctlBody::RouterHello(hello),
        }
    }

    pub fn router_hello_ack(transaction_id: u32, hello: RouterHello) -> Self {
        Self {
            transaction_id,
            body: RoutingGctlBody::RouterHelloAck(hello),
        }
    }

    pub fn route_advertise(transaction_id: u32, route: RouteAdvertise) -> Self {
        Self {
            transaction_id,
            body: RoutingGctlBody::RouteAdvertise(route),
        }
    }

    pub fn route_withdraw(transaction_id: u32, route: RouteWithdraw) -> Self {
        Self {
            transaction_id,
            body: RoutingGctlBody::RouteWithdraw(route),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let (message_type, body) = match self.body {
            RoutingGctlBody::RouterHello(hello) => (GCTL_ROUTER_HELLO, encode_hello(hello)),
            RoutingGctlBody::RouterHelloAck(hello) => (GCTL_ROUTER_HELLO_ACK, encode_hello(hello)),
            RoutingGctlBody::RouteAdvertise(route) => {
                (GCTL_ROUTE_ADVERTISE, encode_route_advertise(route))
            }
            RoutingGctlBody::RouteWithdraw(route) => {
                (GCTL_ROUTE_WITHDRAW, encode_route_withdraw(route))
            }
        };

        let mut out = vec![0u8; GCTL_HEADER_LEN + body.len()];
        out[0] = GCTL_ROUTING_VERSION;
        out[1] = message_type;
        out[2] = 0;
        out[3] = 0;
        out[4..8].copy_from_slice(&self.transaction_id.to_be_bytes());
        out[8..].copy_from_slice(&body);
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self> {
        if buf.len() != GCTL_HEADER_LEN + ROUTER_HELLO_BODY_LEN {
            return Err(Error::InvalidLength);
        }
        if buf[0] != GCTL_ROUTING_VERSION {
            return Err(Error::Unsupported);
        }
        if buf[2] != 0 || buf[3] != 0 {
            return Err(Error::InvalidField);
        }

        let transaction_id = u32::from_be_bytes(buf[4..8].try_into().unwrap());
        let body = match buf[1] {
            GCTL_ROUTER_HELLO => RoutingGctlBody::RouterHello(decode_hello(&buf[8..])?),
            GCTL_ROUTER_HELLO_ACK => RoutingGctlBody::RouterHelloAck(decode_hello(&buf[8..])?),
            GCTL_ROUTE_ADVERTISE => {
                RoutingGctlBody::RouteAdvertise(decode_route_advertise(&buf[8..])?)
            }
            GCTL_ROUTE_WITHDRAW => {
                RoutingGctlBody::RouteWithdraw(decode_route_withdraw(&buf[8..])?)
            }
            _ => return Err(Error::Unsupported),
        };
        Ok(Self {
            transaction_id,
            body,
        })
    }
}

fn encode_hello(hello: RouterHello) -> [u8; ROUTER_HELLO_BODY_LEN] {
    let mut body = [0u8; ROUTER_HELLO_BODY_LEN];
    body[0..8].copy_from_slice(&hello.router_id.0.to_be_bytes());
    body[8..16].copy_from_slice(&hello.link_id.0.to_be_bytes());
    body[16..20].copy_from_slice(&hello.hold_time_ms.to_be_bytes());
    body[20..24].copy_from_slice(&hello.metric.0.to_be_bytes());
    body
}

fn decode_hello(body: &[u8]) -> Result<RouterHello> {
    if body.len() != ROUTER_HELLO_BODY_LEN {
        return Err(Error::InvalidLength);
    }
    Ok(RouterHello {
        router_id: RouterId::new(u64::from_be_bytes(body[0..8].try_into().unwrap()))?,
        link_id: LinkId::new(u64::from_be_bytes(body[8..16].try_into().unwrap()))?,
        hold_time_ms: u32::from_be_bytes(body[16..20].try_into().unwrap()),
        metric: RouteMetric(u32::from_be_bytes(body[20..24].try_into().unwrap())),
    })
}

fn encode_route_advertise(route: RouteAdvertise) -> [u8; ROUTE_ADVERTISE_BODY_LEN] {
    let mut body = [0u8; ROUTE_ADVERTISE_BODY_LEN];
    body[0..8].copy_from_slice(&route.advertiser.0.to_be_bytes());
    body[8..16].copy_from_slice(&route.prefix.network().0.to_be_bytes());
    body[16] = route.prefix.prefix_len();
    body[17] = route.origin as u8;
    body[18] = 0;
    body[19] = 0;
    body[20..24].copy_from_slice(&route.metric.0.to_be_bytes());
    body
}

fn decode_route_advertise(body: &[u8]) -> Result<RouteAdvertise> {
    if body.len() != ROUTE_ADVERTISE_BODY_LEN {
        return Err(Error::InvalidLength);
    }
    if body[18] != 0 || body[19] != 0 {
        return Err(Error::InvalidField);
    }

    let advertiser = RouterId::new(u64::from_be_bytes(body[0..8].try_into().unwrap()))?;
    let network = GdpAddress(u64::from_be_bytes(body[8..16].try_into().unwrap()));
    let prefix_len = body[16];
    let prefix = GdpPrefix::new(network, prefix_len).map_err(|_| Error::InvalidField)?;
    if prefix.network() != network {
        return Err(Error::NonCanonical);
    }

    Ok(RouteAdvertise {
        advertiser,
        prefix,
        origin: RouteOrigin::from_wire(body[17])?,
        metric: RouteMetric(u32::from_be_bytes(body[20..24].try_into().unwrap())),
    })
}

fn encode_route_withdraw(route: RouteWithdraw) -> [u8; ROUTE_WITHDRAW_BODY_LEN] {
    let mut body = [0u8; ROUTE_WITHDRAW_BODY_LEN];
    body[0..8].copy_from_slice(&route.advertiser.0.to_be_bytes());
    body[8..16].copy_from_slice(&route.prefix.network().0.to_be_bytes());
    body[16] = route.prefix.prefix_len();
    body
}

fn decode_route_withdraw(body: &[u8]) -> Result<RouteWithdraw> {
    if body.len() != ROUTE_WITHDRAW_BODY_LEN {
        return Err(Error::InvalidLength);
    }
    if body[17..24].iter().any(|byte| *byte != 0) {
        return Err(Error::InvalidField);
    }

    let advertiser = RouterId::new(u64::from_be_bytes(body[0..8].try_into().unwrap()))?;
    let network = GdpAddress(u64::from_be_bytes(body[8..16].try_into().unwrap()));
    let prefix_len = body[16];
    let prefix = GdpPrefix::new(network, prefix_len).map_err(|_| Error::InvalidField)?;
    if prefix.network() != network {
        return Err(Error::NonCanonical);
    }

    Ok(RouteWithdraw { advertiser, prefix })
}
