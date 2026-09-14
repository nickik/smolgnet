#![cfg(feature = "p4-gdp")]

use bitvec::prelude::*;
use p4rs::packet_in;

use crate::error::{Error, Result};
use crate::wire::gdp::{GdpAddress, GdpAddresses, GdpHeader, GdpType, GdpWireConfig, SizeClass};

p4_macro::use_p4!(
    p4 = "p4-gdp/p4/gdp.p4",
    pipeline_name = "gdp",
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedAddresses {
    Global { destination: u64, source: u64 },
    Local { destination: u16, source: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedGdp {
    pub version: u8,
    pub packet_type: u8,
    pub size_class: u8,
    pub local_form: bool,
    pub reserved: u8,
    pub crc8: u8,
    pub hop_limit: u8,
    pub addresses: ParsedAddresses,
}

/// Parse one GDP header with the x4c-generated parser.
///
/// The current P4 program encodes the standard GDP convention where the form
/// bit is one for local addressing. A caller using an inverted GdpWireConfig
/// must use the handwritten fallback until the P4 target grows configurable
/// form-bit semantics.
pub fn parse_header(bytes: &[u8]) -> Result<ParsedGdp> {
    if bytes.len() < 4 {
        return Err(Error::InvalidLength);
    }

    let word = u32::from_be_bytes(bytes[..4].try_into().map_err(|_| Error::InvalidLength)?);
    let local_form = ((word >> 21) & 1) != 0;
    let header_len = if local_form { 8 } else { 20 };
    if bytes.len() < header_len {
        return Err(Error::InvalidLength);
    }

    // p4rs packet_in::extract slices directly. Validate the complete header
    // above so malformed/truncated input cannot panic inside generated code.
    let mut pkt = packet_in::new(bytes);
    let mut hdr = headers_t::default();
    let mut ingress = ingress_metadata_t::default();
    if !parse_start(&mut pkt, &mut hdr, &mut ingress) {
        return Err(Error::InvalidField);
    }

    // x4c currently exposes <=8-bit fields in network bit order. Wider header
    // fields are byte-reversed by generated Header::set(), hence load_le().
    let version = hdr.base.version.load_be::<u8>();
    let packet_type = hdr.base.packet_type.load_be::<u8>();
    let size_class = hdr.base.size_class.load_be::<u8>();
    let parsed_local_form = hdr.base.local_form.load_be::<u8>() != 0;
    let reserved = hdr.base.reserved.load_be::<u8>();
    let crc8 = hdr.base.crc8.load_be::<u8>();
    let raw_hop = hdr.base.hop.load_be::<u8>();

    if parsed_local_form != local_form {
        return Err(Error::InvalidAddressForm);
    }

    let addresses = if local_form {
        ParsedAddresses::Local {
            destination: hdr.local_addr.destination.load_le::<u16>(),
            source: hdr.local_addr.source.load_le::<u16>(),
        }
    } else {
        let dhi = hdr.global_addr.destination_hi.load_le::<u32>() as u64;
        let dlo = hdr.global_addr.destination_lo.load_le::<u32>() as u64;
        let shi = hdr.global_addr.source_hi.load_le::<u32>() as u64;
        let slo = hdr.global_addr.source_lo.load_le::<u32>() as u64;
        ParsedAddresses::Global {
            destination: (dhi << 32) | dlo,
            source: (shi << 32) | slo,
        }
    };

    Ok(ParsedGdp {
        version,
        packet_type,
        size_class,
        local_form,
        reserved,
        crc8,
        hop_limit: if local_form { raw_hop & 0x0f } else { raw_hop },
        addresses,
    })
}

pub fn decode_header(bytes: &[u8], local_prefix: u64) -> Result<GdpHeader> {
    let parsed = parse_header(bytes)?;
    let size_class = SizeClass::from_wire(parsed.size_class)?;
    let packet_type = GdpType::from_wire(parsed.packet_type);

    let addresses = match parsed.addresses {
        ParsedAddresses::Global { destination, source } => GdpAddresses::Global {
            destination: GdpAddress(destination),
            source: GdpAddress(source),
        },
        ParsedAddresses::Local { destination, source } => GdpAddresses::Local {
            destination,
            source,
            prefix: local_prefix & !0xffff,
        },
    };

    let header = GdpHeader {
        version: parsed.version,
        packet_type,
        size_class,
        hop_limit: parsed.hop_limit,
        addresses,
    };

    if header.crc8(GdpWireConfig::default()) != parsed.crc8 {
        return Err(Error::InvalidCrc);
    }

    Ok(header)
}

/// Validate a complete GDP packet with the x4c parser and the GDP size table.
pub fn parse_and_validate(bytes: &[u8]) -> Result<ParsedGdp> {
    let parsed = parse_header(bytes)?;
    let size_class = SizeClass::from_wire(parsed.size_class)?;
    let header_len = if parsed.local_form { 8 } else { 20 };
    if bytes.len() != header_len + size_class.bytes() {
        return Err(Error::InvalidLength);
    }

    let local_prefix = 0u64;
    let header = decode_header(bytes, local_prefix)?;
    if header.version != parsed.version {
        return Err(Error::InvalidField);
    }

    Ok(parsed)
}
