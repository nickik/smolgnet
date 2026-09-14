#![allow(clippy::too_many_arguments)]

use bitvec::prelude::*;
use p4rs::{packet_in, Pipeline};

p4_macro::use_p4!(
    p4 = "p4/gdp.p4",
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

impl ParsedGdp {
    pub const fn header_len(self) -> usize {
        if self.local_form { 8 } else { 20 }
    }

    pub const fn payload_len(self) -> usize {
        size_class_bytes(self.size_class)
    }

    pub const fn packet_len(self) -> usize {
        self.header_len() + self.payload_len()
    }

    /// Independent implementation of GDP CRC-8 for x4c conformance testing.
    /// Hop limit is deliberately not covered by GDP CRC-8.
    pub fn recomputed_crc8(self) -> u8 {
        let mut crc = 0u8;
        crc8_update_bits(&mut crc, self.version as u64, 2);
        crc8_update_bits(&mut crc, self.packet_type as u64, 4);
        crc8_update_bits(&mut crc, self.size_class as u64, 4);
        crc8_update_bit(&mut crc, self.local_form);
        match self.addresses {
            ParsedAddresses::Global { destination, source } => {
                crc8_update_bits(&mut crc, destination, 64);
                crc8_update_bits(&mut crc, source, 64);
            }
            ParsedAddresses::Local { destination, source } => {
                crc8_update_bits(&mut crc, destination as u64, 16);
                crc8_update_bits(&mut crc, source as u64, 16);
            }
        }
        crc
    }

    pub fn crc_is_valid(self) -> bool {
        self.recomputed_crc8() == self.crc8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P4GdpError {
    Truncated,
    InvalidLength { expected: usize, actual: usize },
    ParserRejected,
    InvalidCrc { expected: u8, actual: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdpOutput {
    pub port: u16,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDisposition {
    Forward,
    Local,
    Drop,
}

/// Safe adapter around the x4c-generated parser.
///
/// p4rs packet extraction currently slices packet input directly, so we check
/// the minimum GDP header length before invoking generated parser code. We also
/// enforce GDP's exact size-class packet length here.
pub fn parse_gdp(bytes: &[u8]) -> Result<ParsedGdp, P4GdpError> {
    if bytes.len() < 4 {
        return Err(P4GdpError::Truncated);
    }

    let word = u32::from_be_bytes(bytes[..4].try_into().expect("length checked"));
    let local_form = ((word >> 21) & 1) != 0;
    let header_len = if local_form { 8 } else { 20 };
    if bytes.len() < header_len {
        return Err(P4GdpError::Truncated);
    }

    let mut pkt = packet_in::new(bytes);
    let mut hdr = headers_t::default();
    let mut ingress = ingress_metadata_t::default();
    if !parse_start(&mut pkt, &mut hdr, &mut ingress) {
        return Err(P4GdpError::ParserRejected);
    }

    // x4c stores <=8-bit fields directly as Msb0 network-order bit vectors.
    // Wider fields are byte-reversed by its generated Header::set(), so they
    // continue to use load_le below. Keeping this distinction here makes the
    // adapter explicit and catches x4c representation changes in conformance CI.
    let version = hdr.base.version.load_be::<u8>();
    let packet_type = hdr.base.packet_type.load_be::<u8>();
    let size_class = hdr.base.size_class.load_be::<u8>();
    let reserved = hdr.base.reserved.load_be::<u8>();
    let crc8 = hdr.base.crc8.load_be::<u8>();
    let raw_hop = hdr.base.hop.load_be::<u8>();

    let addresses = if local_form {
        ParsedAddresses::Local {
            destination: hdr.local_addr.destination.load_le::<u16>(),
            source: hdr.local_addr.source.load_le::<u16>(),
        }
    } else {
        let destination_hi = hdr.global_addr.destination_hi.load_le::<u32>() as u64;
        let destination_lo = hdr.global_addr.destination_lo.load_le::<u32>() as u64;
        let source_hi = hdr.global_addr.source_hi.load_le::<u32>() as u64;
        let source_lo = hdr.global_addr.source_lo.load_le::<u32>() as u64;
        ParsedAddresses::Global {
            destination: (destination_hi << 32) | destination_lo,
            source: (source_hi << 32) | source_lo,
        }
    };

    let parsed = ParsedGdp {
        version,
        packet_type,
        size_class,
        local_form,
        reserved,
        crc8,
        hop_limit: if local_form { raw_hop & 0x0f } else { raw_hop },
        addresses,
    };

    let expected = parsed.packet_len();
    if bytes.len() != expected {
        return Err(P4GdpError::InvalidLength {
            expected,
            actual: bytes.len(),
        });
    }

    Ok(parsed)
}

pub fn validate_gdp(bytes: &[u8]) -> Result<ParsedGdp, P4GdpError> {
    let parsed = parse_gdp(bytes)?;
    let expected = parsed.recomputed_crc8();
    if parsed.crc8 != expected {
        return Err(P4GdpError::InvalidCrc {
            expected,
            actual: parsed.crc8,
        });
    }
    Ok(parsed)
}

/// x4c-backed GDP forwarding plane.
///
/// CRC and packet-length validation are performed by the Rust adapter because
/// x4c's SoftNPU target does not currently provide a portable GDP CRC-8 extern.
/// Route lookup, local/forward/drop and transit hop-limit handling are P4.
pub struct GdpPipeline {
    inner: main_pipeline,
}

impl GdpPipeline {
    pub fn new(radix: u16) -> Self {
        Self {
            inner: main_pipeline::new(radix),
        }
    }

    pub fn add_global_route(
        &mut self,
        destination: u64,
        port: u16,
        disposition: RouteDisposition,
    ) {
        let hi = (destination >> 32) as u32;
        let lo = destination as u32;
        let mut key = hi.to_le_bytes().to_vec();
        key.extend_from_slice(&lo.to_le_bytes());
        self.inner.add_table_entry(
            "ingress.global_routes",
            disposition.action_name(),
            &key,
            &disposition.parameter_data(port),
            0,
        );
    }

    pub fn add_local_route(
        &mut self,
        destination: u16,
        port: u16,
        disposition: RouteDisposition,
    ) {
        self.inner.add_table_entry(
            "ingress.local_routes",
            disposition.action_name(),
            &destination.to_le_bytes(),
            &disposition.parameter_data(port),
            0,
        );
    }

    pub fn process(
        &mut self,
        ingress_port: u16,
        bytes: &[u8],
    ) -> Result<Vec<GdpOutput>, P4GdpError> {
        validate_gdp(bytes)?;
        let mut input = packet_in::new(bytes);
        let output = self.inner.process_packet(ingress_port, &mut input);
        Ok(output
            .into_iter()
            .map(|(packet, port)| {
                let mut bytes = packet.header_data;
                bytes.extend_from_slice(packet.payload_data);
                GdpOutput { port, bytes }
            })
            .collect())
    }

    pub fn table_ids(&self) -> Vec<&str> {
        self.inner.get_table_ids()
    }
}

impl RouteDisposition {
    const fn action_name(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Local => "deliver_local",
            Self::Drop => "drop",
        }
    }

    fn parameter_data(self, port: u16) -> Vec<u8> {
        match self {
            Self::Forward | Self::Local => port.to_le_bytes().to_vec(),
            Self::Drop => Vec::new(),
        }
    }
}

pub const fn size_class_bytes(size_class: u8) -> usize {
    match size_class & 0x0f {
        0 => 0,
        1 => 3,
        2 => 32,
        3 => 64,
        4 => 128,
        5 => 192,
        6 => 256,
        7 => 384,
        8 => 512,
        9 => 768,
        10 => 1024,
        11 => 1280,
        12 => 1500,
        13 => 2048,
        14 => 4096,
        15 => 8192,
        _ => unreachable!(),
    }
}

fn crc8_update_bit(crc: &mut u8, bit: bool) {
    let top = (*crc & 0x80) != 0;
    *crc <<= 1;
    if top ^ bit {
        *crc ^= 0x07;
    }
}

fn crc8_update_bits(crc: &mut u8, value: u64, bits: usize) {
    for i in (0..bits).rev() {
        crc8_update_bit(crc, ((value >> i) & 1) != 0);
    }
}
