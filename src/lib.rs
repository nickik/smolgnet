#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod bounded_gts;
pub mod dynamic_routing;
pub mod error;
pub mod routing;
pub mod socket;
pub mod storage;
pub mod time;
pub mod wire;

#[cfg(feature = "alloc")]
pub mod dlp;
#[cfg(feature = "alloc")]
pub mod dlp_control;
#[cfg(feature = "alloc")]
pub mod dlp_gdp;
#[cfg(feature = "alloc")]
pub mod dlp_v01;
#[cfg(feature = "alloc")]
pub mod egress_scheduler;
#[cfg(feature = "alloc")]
pub mod endpoint;
#[cfg(feature = "alloc")]
pub mod gts;
#[cfg(feature = "alloc")]
pub mod router_adjacency;
#[cfg(feature = "p4-gdp")]
pub mod p4_gdp;
#[cfg(feature = "p4-gts")]
pub mod p4_gts;
#[cfg(feature = "p4-router")]
pub mod static_router;
#[cfg(feature = "p4-router")]
pub mod cycle_router;
#[cfg(feature = "p4-switch")]
pub mod static_switch;
#[cfg(feature = "alloc")]
pub mod qdx;
#[cfg(feature = "alloc")]
pub mod runtime;
#[cfg(feature = "alloc")]
pub mod trace;

// Transitional crate-private compatibility name for the endpoint core while
// the DLP implementation lives in the explicit `dlp` module. This is not part
// of the public API and can disappear once the endpoint core is mechanically
// updated to import `crate::dlp` directly.
#[cfg(feature = "alloc")]
mod link {
    pub(crate) use crate::dlp::*;
}

pub use bounded_gts::{
    AckState as BoundedAckState, BoundedGtsSocket, BoundedStreamSlot, ReceivedMessage,
    Retransmit, RxDisposition, RxMeta, SendInfo, StreamState as BoundedStreamState,
    TunnelRole as BoundedTunnelRole, TunnelState as BoundedTunnelState, TxMeta,
};
pub use dynamic_routing::{LinkId, RouteMetric, RouteOrigin, RouterId};
pub use error::{Error, Result};
pub use routing::{GdpPrefix, PrefixLengthError, Route, RouteTable, RouteTableFull};
pub use socket::{SocketHandle, SocketSet, SocketStorage};
#[cfg(feature = "alloc")]
pub use socket::OwnedSocketSet;
pub use storage::{MessageBuffer, MessagePool, MessageSlot, PacketBuffer, PacketMetadata, RingBuffer};
pub use time::{Duration, Instant, PollAt};
pub use wire::css::{CssWire, ServiceSelector};
pub use wire::gdp::{AddressForm, GdpAddress, GdpHeader, GdpType, GdpWireConfig, SizeClass};
pub use wire::gts::{Direction, GtsType, StreamProfile};

#[cfg(feature = "alloc")]
pub use dlp::{DlpConfig, DlpEndpoint, Flit, GnetFrame, LinkTraffic, VcMode, Vcid};
#[cfg(feature = "alloc")]
pub use dlp_control::{
    DlpCapabilityKind, DlpControlFrame, DlpControlOpcode, DlpControlState, DlpDirectCable,
    DlpHelloKind, DlpManagedEndpoint, DlpNegotiatedProfile, DLP_CONTROL_FRAME_LEN,
    DLP_CONTROL_PARAMETER_UNIT_FLITS, DLP_CONTROL_VERSION, DLP_RESERVED_CONTROL_WINDOW_FLITS,
};
#[cfg(feature = "alloc")]
pub use dlp_gdp::{DlpGdpPort, GdpPacketPort};
#[cfg(feature = "alloc")]
pub use dlp_v01::{DlpLink, DlpLinkState};
#[cfg(feature = "alloc")]
pub use egress_scheduler::{EgressFlowId, RouterEgressScheduler};
#[cfg(feature = "alloc")]
pub use endpoint::{
    make_link_local, AddressAuthorityConfig, AddressState, Endpoint, EndpointConfig,
    ListenerConfig, TunnelHandle, BOOTSTRAP_ADDRESS, LINK_LOCAL_PREFIX,
};
#[cfg(feature = "alloc")]
pub use gts::{GtsStream, GtsTunnel, StreamState, TunnelRole, TunnelState};
#[cfg(feature = "alloc")]
pub use router_adjacency::{NeighborState, RouterAdjacency, RouterNeighbor};
#[cfg(feature = "p4-router")]
pub use static_router::{
    FibEntry, FibOrigin, ROUTER_BOOTSTRAP_ADDRESS, ROUTER_PORT_COUNT, RouterDisposition,
    RouterPortConfig, RouterPortId, RouterStartupConfig, StaticP4Router, StaticRouteConfig,
};
#[cfg(feature = "p4-router")]
pub use cycle_router::{CycleAwareRouter, RouterVc0Policy};
#[cfg(feature = "p4-switch")]
pub use static_switch::{
    SWITCH_PORT_COUNT, StaticP4Switch, SwitchDisposition, SwitchPortId,
};
#[cfg(feature = "alloc")]
pub use qdx::{GnetFlitDevice, GnetFrameDevice};
#[cfg(feature = "alloc")]
pub use runtime::EndpointRuntimeExt;
#[cfg(all(feature = "alloc", feature = "async"))]
pub use runtime::{AsyncEndpoint, WakerRegistration};
#[cfg(feature = "alloc")]
pub use trace::{TraceDirection, TraceEvent, TraceSink, Tracer};
#[cfg(feature = "alloc")]
pub use wire::gctl::{
    address_matches_prefix, normalize_prefix, AddressAck, AddressClaim, AddressNak, AddressOffer,
    Advertise, CreditGrant, CreditRequest, DiscoveryScope, GctlMessage, GctlType, ServiceType,
};
#[cfg(feature = "alloc")]
pub use wire::gctl_routing::{
    RouteAdvertise, RouterHello, RoutingGctlBody, RoutingGctlMessage, GCTL_ROUTE_ADVERTISE,
    GCTL_ROUTER_HELLO, GCTL_ROUTER_HELLO_ACK,
};
#[cfg(feature = "alloc")]
pub use wire::gdp::GdpPacket;
#[cfg(feature = "alloc")]
pub use wire::gts::GtsPacket;
