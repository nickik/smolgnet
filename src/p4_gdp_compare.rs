#![cfg(feature = "p4-gdp-compare")]

use bitvec::prelude::*;
use p4rs::packet_in;

p4_macro::use_p4!(
    p4 = "p4-gdp/p4/gdp.p4",
    pipeline_name = "gdp",
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParsedAddresses {
    Global { destination: u64, source: u64 },
    Local { destination: u16, source: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParsedGdp {
    pub version: u8,
    pub packet_type: u8,
    pub size_class: u8,
    pub local_form: bool,
    pub crc8: u8,
    pub hop_limit: u8,
    pub addresses: ParsedAddresses,
}

pub(crate) fn parse_and_validate(bytes: &[u8]) -> core::result::Result<ParsedGdp, ()> {
    if bytes.len() < 4 {
        return Err(());
    }
    let word = u32::from_be_bytes(bytes[..4].try_into().map_err(|_| ())?);
    let local_form = ((word >> 21) & 1) != 0;
    let header_len = if local_form { 8 } else { 20 };
    if bytes.len() < header_len {
        return Err(());
    }

    let mut pkt = packet_in::new(bytes);
    let mut hdr = headers_t::default();
    let mut ingress = ingress_metadata_t::default();
    if !parse_start(&mut pkt, &mut hdr, &mut ingress) {
        return Err(());
    }

    // x4c currently exposes sub-byte/byte fields in network bit order, while
    // its generated Header::set() reverses wider fields. Mirror the adapter
    // used by p4-gdp conformance tests so this host-stack experiment tests the
    // same generated representation rather than inventing a second mapping.
    let version = hdr.base.version.load_be::<u8>();
    let packet_type = hdr.base.packet_type.load_be::<u8>();
    let size_class = hdr.base.size_class.load_be::<u8>();
    let crc8 = hdr.base.crc8.load_be::<u8>();
    let raw_hop = hdr.base.hop.load_be::<u8>();
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

    let parsed = ParsedGdp {
        version,
        packet_type,
        size_class,
        local_form,
        crc8,
        hop_limit: if local_form { raw_hop & 0x0f } else { raw_hop },
        addresses,
    };

    let payload_len = match size_class {
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
        _ => return Err(()),
    };
    if bytes.len() != header_len + payload_len {
        return Err(());
    }

    if recompute_crc(parsed) != parsed.crc8 {
        return Err(());
    }
    Ok(parsed)
}

fn recompute_crc(parsed: ParsedGdp) -> u8 {
    let mut crc = 0u8;
    update_bits(&mut crc, parsed.version as u64, 2);
    update_bits(&mut crc, parsed.packet_type as u64, 4);
    update_bits(&mut crc, parsed.size_class as u64, 4);
    update_bit(&mut crc, parsed.local_form);
    match parsed.addresses {
        ParsedAddresses::Global { destination, source } => {
            update_bits(&mut crc, destination, 64);
            update_bits(&mut crc, source, 64);
        }
        ParsedAddresses::Local { destination, source } => {
            update_bits(&mut crc, destination as u64, 16);
            update_bits(&mut crc, source as u64, 16);
        }
    }
    crc
}

fn update_bit(crc: &mut u8, bit: bool) {
    let top = (*crc & 0x80) != 0;
    *crc <<= 1;
    if top ^ bit {
        *crc ^= 0x07;
    }
}

fn update_bits(crc: &mut u8, value: u64, bits: usize) {
    for i in (0..bits).rev() {
        update_bit(crc, ((value >> i) & 1) != 0);
    }
}
