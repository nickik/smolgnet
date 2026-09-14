#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::error::{Error, Result};

pub use crate::wire::gdp_common::{
    AddressForm, GdpAddress, GdpAddresses, GdpHeader, GdpType, GdpWireConfig, SizeClass,
};

impl GdpHeader {
    pub fn encode_into(&self, cfg: GdpWireConfig, out: &mut [u8]) -> Result<usize> {
        crate::wire::gdp_handwritten::encode_header_into(self, cfg, out)
    }

    #[cfg(feature = "alloc")]
    pub fn encode(&self, cfg: GdpWireConfig) -> Result<Vec<u8>> {
        let mut out = alloc::vec![0u8; self.header_len()];
        self.encode_into(cfg, &mut out)?;
        Ok(out)
    }

    /// Decode GDP using the selected lower wire backend. The protocol model,
    /// addressing rules, size classes and CRC semantics live in gdp_common.
    pub fn decode(buf: &[u8], cfg: GdpWireConfig, local_prefix: u64) -> Result<Self> {
        #[cfg(feature = "p4-gdp")]
        if cfg.local_form_bit {
            #[cfg(feature = "p4-gdp-compare")]
            {
                let p4 = crate::p4_gdp::decode_header(buf, local_prefix);
                let handwritten =
                    crate::wire::gdp_handwritten::decode_header(buf, cfg, local_prefix);
                return match (p4, handwritten) {
                    (Ok(a), Ok(b)) if a == b => Ok(a),
                    (Err(a), Err(b)) if a == b => Err(a),
                    _ => Err(Error::InvalidField),
                };
            }
            #[cfg(not(feature = "p4-gdp-compare"))]
            {
                return crate::p4_gdp::decode_header(buf, local_prefix);
            }
        }

        crate::wire::gdp_handwritten::decode_header(buf, cfg, local_prefix)
    }

    #[cfg(feature = "p4-gdp")]
    #[doc(hidden)]
    pub fn decode_handwritten_reference(
        buf: &[u8],
        cfg: GdpWireConfig,
        local_prefix: u64,
    ) -> Result<Self> {
        crate::wire::gdp_handwritten::decode_header(buf, cfg, local_prefix)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdpPacketRef<'a> {
    pub header: GdpHeader,
    pub payload: &'a [u8],
}

impl<'a> GdpPacketRef<'a> {
    pub fn new(header: GdpHeader, payload: &'a [u8]) -> Result<Self> {
        if payload.len() != header.size_class.bytes() {
            return Err(Error::InvalidLength);
        }
        Ok(Self { header, payload })
    }

    pub fn decode(buf: &'a [u8], cfg: GdpWireConfig, local_prefix: u64) -> Result<Self> {
        let header = GdpHeader::decode(buf, cfg, local_prefix)?;
        let header_len = header.header_len();
        let total = header_len + header.size_class.bytes();
        if buf.len() != total {
            return Err(Error::InvalidLength);
        }
        Ok(Self {
            header,
            payload: &buf[header_len..],
        })
    }

    pub fn encode_into(&self, cfg: GdpWireConfig, out: &mut [u8]) -> Result<usize> {
        let total = self.header.header_len() + self.payload.len();
        if out.len() < total {
            return Err(Error::BufferFull);
        }
        let header_len = self.header.encode_into(cfg, out)?;
        out[header_len..total].copy_from_slice(self.payload);
        Ok(total)
    }
}

#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdpPacket {
    pub header: GdpHeader,
    pub payload: Vec<u8>,
}

#[cfg(feature = "alloc")]
impl GdpPacket {
    pub fn new(header: GdpHeader, payload: Vec<u8>) -> Result<Self> {
        if payload.len() != header.size_class.bytes() {
            return Err(Error::InvalidLength);
        }
        Ok(Self { header, payload })
    }

    pub fn encode(&self, cfg: GdpWireConfig) -> Result<Vec<u8>> {
        let mut out = alloc::vec![0u8; self.header.header_len() + self.payload.len()];
        let n = GdpPacketRef::new(self.header.clone(), &self.payload)?.encode_into(cfg, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    pub fn decode(buf: &[u8], cfg: GdpWireConfig, local_prefix: u64) -> Result<Self> {
        let borrowed = GdpPacketRef::decode(buf, cfg, local_prefix)?;
        Ok(Self {
            header: borrowed.header,
            payload: borrowed.payload.to_vec(),
        })
    }
}
