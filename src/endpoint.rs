mod core {
    include!("endpoint/core.rs");
    include!("endpoint/link_api.rs");
}

pub use core::{
    make_link_local, AddressAuthorityConfig, AddressState, Endpoint, EndpointConfig,
    ListenerConfig, TunnelHandle, BOOTSTRAP_ADDRESS, LINK_LOCAL_PREFIX,
};
