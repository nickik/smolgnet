use crate::error::{Error, Result};
use crate::wire::gdp_common::{
    AddressForm, GdpAddress, GdpAddresses, GdpHeader, GdpType, GdpWireConfig, SizeClass,
};

pub(crate) fn encode_header_into(
    header: &GdpHeader,
    cfg: GdpWireConfig,
    out: &mut [u8],
) -> Result<usize> {
    if header.version > 3 || header.packet_type.to_wire() > 15 {
        return Err(Error::InvalidField);
    }
    if matches!(header.addresses, GdpAddresses::Local { .. }) && header.hop_limit > 15 {
        return Err(Error::InvalidField);
    }

    let need = header.header_len();
    if out.len() < need {
        return Err(Error::BufferFull);
    }

    let form_bit = cfg.form_bit(header.addresses.form()) as u32;
    let crc = header.crc8(cfg) as u32;
    let hop = match header.addresses {
        GdpAddresses::Global { .. } => header.hop_limit as u32,
        GdpAddresses::Local { .. } => (header.hop_limit & 0x0f) as u32,
    };
    let word1 = ((header.version as u32 & 3) << 30)
        | ((header.packet_type.to_wire() as u32 & 0x0f) << 26)
        | ((header.size_class as u8 as u32 & 0x0f) << 22)
        | (form_bit << 21)
        | (crc << 8)
        | hop;
    out[..4].copy_from_slice(&word1.to_be_bytes());

    match header.addresses {
        GdpAddresses::Global {
            destination,
            source,
        } => {
            out[4..12].copy_from_slice(&destination.0.to_be_bytes());
            out[12..20].copy_from_slice(&source.0.to_be_bytes());
        }
        GdpAddresses::Local {
            destination, source, ..
        } => {
            let word = ((destination as u32) << 16) | source as u32;
            out[4..8].copy_from_slice(&word.to_be_bytes());
        }
    }

    Ok(need)
}

pub(crate) fn decode_header(
    buf: &[u8],
    cfg: GdpWireConfig,
    local_prefix: u64,
) -> Result<GdpHeader> {
    if buf.len() < 4 {
        return Err(Error::InvalidLength);
    }

    let word = u32::from_be_bytes(buf[0..4].try_into().map_err(|_| Error::InvalidLength)?);
    let version = ((word >> 30) & 3) as u8;
    let packet_type = GdpType::from_wire(((word >> 26) & 0x0f) as u8);
    let size_class = SizeClass::from_wire(((word >> 22) & 0x0f) as u8)?;
    let form = cfg.form_from_bit(((word >> 21) & 1) != 0);
    let received_crc = ((word >> 8) & 0xff) as u8;

    let header = match form {
        AddressForm::Global => {
            if buf.len() < 20 {
                return Err(Error::InvalidLength);
            }
            GdpHeader {
                version,
                packet_type,
                size_class,
                hop_limit: (word & 0xff) as u8,
                addresses: GdpAddresses::Global {
                    destination: GdpAddress(u64::from_be_bytes(
                        buf[4..12].try_into().map_err(|_| Error::InvalidLength)?,
                    )),
                    source: GdpAddress(u64::from_be_bytes(
                        buf[12..20].try_into().map_err(|_| Error::InvalidLength)?,
                    )),
                },
            }
        }
        AddressForm::Local => {
            if buf.len() < 8 {
                return Err(Error::InvalidLength);
            }
            let ids = u32::from_be_bytes(
                buf[4..8].try_into().map_err(|_| Error::InvalidLength)?,
            );
            GdpHeader {
                version,
                packet_type,
                size_class,
                hop_limit: (word & 0x0f) as u8,
                addresses: GdpAddresses::Local {
                    destination: (ids >> 16) as u16,
                    source: ids as u16,
                    prefix: local_prefix & !0xffff,
                },
            }
        }
    };

    if header.crc8(cfg) != received_crc {
        return Err(Error::InvalidCrc);
    }
    Ok(header)
}
