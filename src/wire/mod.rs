pub mod crc;
pub mod css;
pub mod gdp;
pub(crate) mod gdp_common;
pub(crate) mod gdp_handwritten;
pub mod gts;

#[cfg(feature = "alloc")]
pub(crate) mod gts_handwritten;
#[cfg(feature = "alloc")]
pub(crate) mod gts_tx;

#[cfg(feature = "alloc")]
pub mod gctl;
