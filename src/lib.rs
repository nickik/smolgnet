#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod burst;
pub mod endpoint;
pub mod error;
pub mod gts;
pub mod link;
pub mod wire;

pub use burst::{BurstDirectLink, DEFAULT_DLP_BURST_FLITS};
pub use endpoint::{
    make_link_local, AddressAuthorityConfig, AddressState, DirectLink, Endpoint, EndpointConfig,
    ListenerConfig, TunnelHandle, BOOTSTRAP_ADDRESS, LINK_LOCAL_PREFIX,
};
pub use error::{Error, Result};
pub use gts::{
    Direction, GtsStream, GtsTunnel, StreamState, TunnelRole, TunnelState,
};
pub use link::{DlpConfig, DlpEndpoint, Flit, LinkTraffic, VcMode, Vcid};
pub use wire::css::{CssWire, ServiceSelector};
pub use wire::gctl::{
    address_matches_prefix, normalize_prefix, AddressAck, AddressClaim, AddressNak, AddressOffer,
    Advertise, CreditGrant, CreditRequest, DiscoveryScope, GctlMessage, GctlType, ServiceType,
};
pub use wire::gdp::{
    AddressForm, GdpAddress, GdpHeader, GdpPacket, GdpType, GdpWireConfig, SizeClass,
};
pub use wire::gts::{GtsPacket, GtsType, StreamProfile};
