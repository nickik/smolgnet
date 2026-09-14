#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod endpoint;
pub mod error;
pub mod gts;
pub mod link;
pub mod wire;

pub use endpoint::{DirectLink, Endpoint, ListenerConfig, TunnelHandle};
pub use error::{Error, Result};
pub use gts::{Direction, GtsStream, GtsTunnel, TunnelRole, TunnelState};
pub use link::{DlpEndpoint, Flit, Vcid};
pub use wire::css::{CssWire, ServiceSelector};
pub use wire::gctl::{GctlMessage, GctlType};
pub use wire::gdp::{AddressForm, GdpAddress, GdpHeader, GdpPacket, GdpType, GdpWireConfig, SizeClass};
pub use wire::gts::{GtsPacket, GtsType, StreamProfile};
