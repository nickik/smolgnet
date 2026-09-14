use alloc::vec;
use alloc::vec::Vec;

use crate::error::{Error, Result};
use crate::wire::gts::{
    lower_prefix_len, GtsContext, GtsLowerDecoder, GtsLowerPacket, GtsPacket, GtsType,
    StreamProfile,
};
use crate::wire::gts_tx::GtsLowerEncoder;

pub(crate) struct HandwrittenGtsLower;

fn read_u32(buf: &[u8], pos: &mut usize, limit: usize) -> Result<u32> {
    if *pos + 4 > limit {
        return Err(Error::InvalidLength);
    }
    let v = u32::from_be_bytes(
        buf[*pos..*pos + 4]
            .try_into()
            .map_err(|_| Error::InvalidLength)?,
    );
    *pos += 4;
    Ok(v)
}

fn read_u16(buf: &[u8], pos: &mut usize, limit: usize) -> Result<u16> {
    if *pos + 2 > limit {
        return Err(Error::InvalidLength);
    }
    let v = u16::from_be_bytes(
        buf[*pos..*pos + 2]
            .try_into()
            .map_err(|_| Error::InvalidLength)?,
    );
    *pos += 2;
    Ok(v)
}

fn read_u8(buf: &[u8], pos: &mut usize, limit: usize) -> Result<u8> {
    if *pos >= limit {
        return Err(Error::InvalidLength);
    }
    let v = buf[*pos];
    *pos += 1;
    Ok(v)
}

impl GtsLowerDecoder for HandwrittenGtsLower {
    fn parse_lower(buf: &[u8]) -> Result<GtsLowerPacket> {
        if buf.len() < 5 {
            return Err(Error::InvalidLength);
        }
        if buf[0] >> 4 != 0 {
            return Err(Error::Unsupported);
        }

        let ty = GtsType::from_wire(buf[0] & 0x0f)?;
        let prefix_len = lower_prefix_len(ty)?;
        if buf.len() < prefix_len + 4 {
            return Err(Error::InvalidLength);
        }

        let mut pos = 1usize;
        let limit = prefix_len;
        Ok(match ty {
            GtsType::Connect => GtsLowerPacket::Connect {
                initiator_receive_tunnel: read_u32(buf, &mut pos, limit)?,
                initiator_reset_id: read_u32(buf, &mut pos, limit)?,
                profile: read_u16(buf, &mut pos, limit)?,
                initial_receive_credit: read_u8(buf, &mut pos, limit)?,
            },
            GtsType::ConnectAck => GtsLowerPacket::ConnectAck {
                initiator_receive_tunnel: read_u32(buf, &mut pos, limit)?,
                responder_receive_tunnel: read_u32(buf, &mut pos, limit)?,
                responder_reset_id: read_u32(buf, &mut pos, limit)?,
                status: read_u8(buf, &mut pos, limit)?,
                initial_receive_credit: read_u8(buf, &mut pos, limit)?,
                reserved: read_u8(buf, &mut pos, limit)?,
            },
            GtsType::StreamOpen => GtsLowerPacket::StreamOpen {
                tunnel_id: read_u32(buf, &mut pos, limit)?,
                stream_id: read_u8(buf, &mut pos, limit)?,
                profile: read_u16(buf, &mut pos, limit)?,
                initial_receive_credit: read_u8(buf, &mut pos, limit)?,
                reserved: read_u8(buf, &mut pos, limit)?,
            },
            GtsType::StreamAck => GtsLowerPacket::StreamAck {
                tunnel_id: read_u32(buf, &mut pos, limit)?,
                stream_id: read_u8(buf, &mut pos, limit)?,
                status: read_u8(buf, &mut pos, limit)?,
                initial_receive_credit: read_u8(buf, &mut pos, limit)?,
                reserved: read_u8(buf, &mut pos, limit)?,
            },
            GtsType::Data | GtsType::DataEnd => GtsLowerPacket::Data {
                tunnel_id: read_u32(buf, &mut pos, limit)?,
                stream_id: read_u8(buf, &mut pos, limit)?,
                sequence: read_u32(buf, &mut pos, limit)?,
                end: ty == GtsType::DataEnd,
                option_probe: read_u16(buf, &mut pos, limit)?,
            },
            GtsType::Ack => GtsLowerPacket::Ack {
                tunnel_id: read_u32(buf, &mut pos, limit)?,
                stream_id: read_u8(buf, &mut pos, limit)?,
                ack_base: read_u32(buf, &mut pos, limit)?,
                receive_bitmap: read_u32(buf, &mut pos, limit)?,
                receive_credit: read_u8(buf, &mut pos, limit)?,
                reserved: read_u8(buf, &mut pos, limit)?,
            },
            GtsType::Datagram => {
                let tunnel_id = read_u32(buf, &mut pos, limit)?;
                let stream_id = read_u8(buf, &mut pos, limit)?;
                let mut option_probe = [0u8; 6];
                for octet in &mut option_probe {
                    *octet = read_u8(buf, &mut pos, limit)?;
                }
                GtsLowerPacket::Datagram {
                    tunnel_id,
                    stream_id,
                    option_probe,
                }
            }
            GtsType::StreamClose | GtsType::StreamCloseAck => GtsLowerPacket::StreamClose {
                tunnel_id: read_u32(buf, &mut pos, limit)?,
                stream_id: read_u8(buf, &mut pos, limit)?,
                final_sequence: read_u32(buf, &mut pos, limit)?,
                ack: ty == GtsType::StreamCloseAck,
            },
            GtsType::TunnelClose | GtsType::TunnelCloseAck => GtsLowerPacket::TunnelClose {
                tunnel_id: read_u32(buf, &mut pos, limit)?,
                ack: ty == GtsType::TunnelCloseAck,
            },
            GtsType::Reset => GtsLowerPacket::Reset {
                tunnel_id: read_u32(buf, &mut pos, limit)?,
                reset_id: read_u32(buf, &mut pos, limit)?,
                reason: read_u8(buf, &mut pos, limit)?,
            },
            GtsType::StreamReset | GtsType::StreamResetAck => GtsLowerPacket::StreamReset {
                tunnel_id: read_u32(buf, &mut pos, limit)?,
                stream_id: read_u8(buf, &mut pos, limit)?,
                reason: read_u8(buf, &mut pos, limit)?,
                reserved: read_u8(buf, &mut pos, limit)?,
                ack: ty == GtsType::StreamResetAck,
            },
            GtsType::Reserved => return Err(Error::Unsupported),
        })
    }
}

impl GtsLowerEncoder for HandwrittenGtsLower {
    fn encode_lower(lower: GtsLowerPacket) -> Result<Vec<u8>> {
        let len = lower_prefix_len(lower.packet_type())?;
        let mut out = vec![0u8; len];
        out[0] = lower.packet_type() as u8;
        let mut pos = 1usize;

        fn put_u32(out: &mut [u8], pos: &mut usize, v: u32) -> Result<()> {
            if *pos + 4 > out.len() {
                return Err(Error::InvalidLength);
            }
            out[*pos..*pos + 4].copy_from_slice(&v.to_be_bytes());
            *pos += 4;
            Ok(())
        }
        fn put_u16(out: &mut [u8], pos: &mut usize, v: u16) -> Result<()> {
            if *pos + 2 > out.len() {
                return Err(Error::InvalidLength);
            }
            out[*pos..*pos + 2].copy_from_slice(&v.to_be_bytes());
            *pos += 2;
            Ok(())
        }
        fn put_u8(out: &mut [u8], pos: &mut usize, v: u8) -> Result<()> {
            if *pos >= out.len() {
                return Err(Error::InvalidLength);
            }
            out[*pos] = v;
            *pos += 1;
            Ok(())
        }

        match lower {
            GtsLowerPacket::Connect {
                initiator_receive_tunnel,
                initiator_reset_id,
                profile,
                initial_receive_credit,
            } => {
                put_u32(&mut out, &mut pos, initiator_receive_tunnel)?;
                put_u32(&mut out, &mut pos, initiator_reset_id)?;
                put_u16(&mut out, &mut pos, profile)?;
                put_u8(&mut out, &mut pos, initial_receive_credit)?;
            }
            GtsLowerPacket::ConnectAck {
                initiator_receive_tunnel,
                responder_receive_tunnel,
                responder_reset_id,
                status,
                initial_receive_credit,
                reserved,
            } => {
                put_u32(&mut out, &mut pos, initiator_receive_tunnel)?;
                put_u32(&mut out, &mut pos, responder_receive_tunnel)?;
                put_u32(&mut out, &mut pos, responder_reset_id)?;
                put_u8(&mut out, &mut pos, status)?;
                put_u8(&mut out, &mut pos, initial_receive_credit)?;
                put_u8(&mut out, &mut pos, reserved)?;
            }
            GtsLowerPacket::StreamOpen {
                tunnel_id,
                stream_id,
                profile,
                initial_receive_credit,
                reserved,
            } => {
                put_u32(&mut out, &mut pos, tunnel_id)?;
                put_u8(&mut out, &mut pos, stream_id)?;
                put_u16(&mut out, &mut pos, profile)?;
                put_u8(&mut out, &mut pos, initial_receive_credit)?;
                put_u8(&mut out, &mut pos, reserved)?;
            }
            GtsLowerPacket::StreamAck {
                tunnel_id,
                stream_id,
                status,
                initial_receive_credit,
                reserved,
            } => {
                put_u32(&mut out, &mut pos, tunnel_id)?;
                put_u8(&mut out, &mut pos, stream_id)?;
                put_u8(&mut out, &mut pos, status)?;
                put_u8(&mut out, &mut pos, initial_receive_credit)?;
                put_u8(&mut out, &mut pos, reserved)?;
            }
            GtsLowerPacket::Data {
                tunnel_id,
                stream_id,
                sequence,
                option_probe,
                ..
            } => {
                put_u32(&mut out, &mut pos, tunnel_id)?;
                put_u8(&mut out, &mut pos, stream_id)?;
                put_u32(&mut out, &mut pos, sequence)?;
                put_u16(&mut out, &mut pos, option_probe)?;
            }
            GtsLowerPacket::Ack {
                tunnel_id,
                stream_id,
                ack_base,
                receive_bitmap,
                receive_credit,
                reserved,
            } => {
                put_u32(&mut out, &mut pos, tunnel_id)?;
                put_u8(&mut out, &mut pos, stream_id)?;
                put_u32(&mut out, &mut pos, ack_base)?;
                put_u32(&mut out, &mut pos, receive_bitmap)?;
                put_u8(&mut out, &mut pos, receive_credit)?;
                put_u8(&mut out, &mut pos, reserved)?;
            }
            GtsLowerPacket::Datagram {
                tunnel_id,
                stream_id,
                option_probe,
            } => {
                put_u32(&mut out, &mut pos, tunnel_id)?;
                put_u8(&mut out, &mut pos, stream_id)?;
                for octet in option_probe {
                    put_u8(&mut out, &mut pos, octet)?;
                }
            }
            GtsLowerPacket::StreamClose {
                tunnel_id,
                stream_id,
                final_sequence,
                ..
            } => {
                put_u32(&mut out, &mut pos, tunnel_id)?;
                put_u8(&mut out, &mut pos, stream_id)?;
                put_u32(&mut out, &mut pos, final_sequence)?;
            }
            GtsLowerPacket::TunnelClose { tunnel_id, .. } => {
                put_u32(&mut out, &mut pos, tunnel_id)?;
            }
            GtsLowerPacket::Reset {
                tunnel_id,
                reset_id,
                reason,
            } => {
                put_u32(&mut out, &mut pos, tunnel_id)?;
                put_u32(&mut out, &mut pos, reset_id)?;
                put_u8(&mut out, &mut pos, reason)?;
            }
            GtsLowerPacket::StreamReset {
                tunnel_id,
                stream_id,
                reason,
                reserved,
                ..
            } => {
                put_u32(&mut out, &mut pos, tunnel_id)?;
                put_u8(&mut out, &mut pos, stream_id)?;
                put_u8(&mut out, &mut pos, reason)?;
                put_u8(&mut out, &mut pos, reserved)?;
            }
        }

        if pos != out.len() {
            return Err(Error::InvalidLength);
        }
        Ok(out)
    }
}

/// Compatibility entry point used by `GtsPacket::encode`. The actual transmit
/// semantics now live in `gts_tx`; this module contributes only the handwritten
/// lower codec.
pub(crate) fn encode_packet(
    packet: &GtsPacket,
    ctx: GtsContext,
    profile: Option<StreamProfile>,
) -> Result<Vec<u8>> {
    crate::wire::gts_tx::encode_packet(packet, ctx, profile)
}
