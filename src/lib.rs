#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod bounded_gts;
pub mod error;
pub mod routing;
pub mod socket;
pub mod storage;
pub mod time;
pub mod wire;

#[cfg(feature = "alloc")]
pub mod dlp;
#[cfg(feature = "alloc")]
pub mod dlp_gdp;
#[cfg(feature = "alloc")]
pub mod dlp_v01;
#[cfg(feature = "alloc")]
pub mod endpoint;
#[cfg(feature = "alloc")]
pub mod gts;
#[cfg(feature = "p4-gdp")]
pub mod p4_gdp;
#[cfg(feature = "p4-gts")]
pub mod p4_gts;
#[cfg(feature = "p4-router")]
pub mod p4_router;
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
pub use dlp_gdp::{DlpGdpPort, GdpPacketPort};
#[cfg(feature = "alloc")]
pub use dlp_v01::{DlpLink, DlpLinkState};
#[cfg(feature = "alloc")]
pub use endpoint::{
    make_link_local, AddressAuthorityConfig, AddressState, Endpoint, EndpointConfig,
    ListenerConfig, TunnelHandle, BOOTSTRAP_ADDRESS, LINK_LOCAL_PREFIX,
};
#[cfg(feature = "alloc")]
pub use gts::{GtsStream, GtsTunnel, StreamState, TunnelRole, TunnelState};
#[cfg(feature = "p4-router")]
pub use p4_router::{
    AdjacencyId, P4Router, PolicyAction, PortCounters, QueueClass, RouteId, RouteTarget,
    RouterAdjacency, RouterCounters, RouterPortId, RouterRoute,
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
pub use wire::gdp::GdpPacket;
#[cfg(feature = "alloc")]
pub use wire::gts::GtsPacket;