#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod bounded_gts;
pub mod error;
pub mod socket;
pub mod storage;
pub mod time;
pub mod wire;

#[cfg(feature = "alloc")]
pub mod burst;
#[cfg(feature = "alloc")]
pub mod endpoint;
#[cfg(feature = "alloc")]
pub mod gts;
#[cfg(feature = "alloc")]
pub mod link;
#[cfg(feature = "p4-gdp")]
pub mod p4_gdp;
#[cfg(feature = "alloc")]
pub mod qdx;
#[cfg(feature = "alloc")]
pub mod runtime;
#[cfg(feature = "alloc")]
pub mod trace;

pub use bounded_gts::{
    AckState as BoundedAckState, BoundedGtsSocket, BoundedStreamSlot, ReceivedMessage,
    Retransmit, RxDisposition, RxMeta, SendInfo, StreamState as BoundedStreamState,
    TunnelRole as BoundedTunnelRole, TunnelState as BoundedTunnelState, TxMeta,
};
pub use error::{Error, Result};
pub use socket::{SocketHandle, SocketSet, SocketStorage};
#[cfg(feature = "alloc")]
pub use socket::OwnedSocketSet;
pub use storage::{MessageBuffer, MessagePool, MessageSlot, PacketBuffer, PacketMetadata, RingBuffer};
pub use time::{Duration, Instant, PollAt};
pub use wire::css::{CssWire, ServiceSelector};
pub use wire::gdp::{AddressForm, GdpAddress, GdpHeader, GdpType, GdpWireConfig, SizeClass};
pub use wire::gts::{Direction, GtsType, StreamProfile};

#[cfg(feature = "alloc")]
pub use burst::{BurstDirectLink, DEFAULT_DLP_BURST_FLITS};
#[cfg(feature = "alloc")]
pub use endpoint::{
    make_link_local, AddressAuthorityConfig, AddressState, DirectLink, Endpoint, EndpointConfig,
    ListenerConfig, TunnelHandle, BOOTSTRAP_ADDRESS, LINK_LOCAL_PREFIX,
};
#[cfg(feature = "alloc")]
pub use gts::{GtsStream, GtsTunnel, StreamState, TunnelRole, TunnelState};
#[cfg(feature = "alloc")]
pub use link::{DlpConfig, DlpEndpoint, Flit, GnetFrame, LinkTraffic, VcMode, Vcid};
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
