use alloc::vec;
use alloc::vec::Vec;

use crate::error::{Error, Result};
use crate::wire::gts::{
    compute_crc, lower_prefix_len, GtsContext, GtsLowerPacket, GtsPacket, StreamProfile,
};

/// Lower GTS transmit backend.
///
/// The common layer has already validated the negotiated stream profile,
/// selected payload placement/padding, and decided CRC coverage. A backend only
/// serializes the fixed/profile-independent prefix represented by
/// `GtsLowerPacket`.
pub(crate) trait GtsLowerEncoder {
    fn encode_lower(lower: GtsLowerPacket) -> Result<Vec<u8>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GtsTxPlan {
    lower: GtsLowerPacket,
    /// Bytes after the profile-independent lower prefix and before the CRC.
    /// This includes payload and any required zero padding.
    tail: Vec<u8>,
    /// Number of GTS bytes covered by CRC. Normally this is the entire packet
    /// before CRC. For an unchecked DATAGRAM it ends after metadata.
    crc_coverage_end: usize,
}

fn zero_tail(total: usize, lower: GtsLowerPacket) -> Result<Vec<u8>> {
    let crc_off = total.checked_sub(4).ok_or(Error::InvalidLength)?;
    let prefix = lower_prefix_len(lower.packet_type())?;
    if prefix > crc_off {
        return Err(Error::InvalidLength);
    }
    Ok(vec![0u8; crc_off - prefix])
}

fn prepare_encode(
    packet: &GtsPacket,
    ctx: GtsContext,
    profile: Option<StreamProfile>,
) -> Result<GtsTxPlan> {
    let total = ctx.size_class.bytes();
    let crc_off = total.checked_sub(4).ok_or(Error::InvalidLength)?;

    let (lower, tail, crc_coverage_end) = match packet {
        GtsPacket::Connect {
            initiator_receive_tunnel,
            initiator_reset_id,
            profile: sp,
            initial_receive_credit,
            css,
        } => {
            let sp = sp.validate()?;
            if sp.unreliable && *initial_receive_credit != 0 {
                return Err(Error::ProfileViolation);
            }
            let lower = GtsLowerPacket::Connect {
                initiator_receive_tunnel: *initiator_receive_tunnel,
                initiator_reset_id: *initiator_reset_id,
                profile: sp.to_wire()?,
                initial_receive_credit: *initial_receive_credit,
            };
            let mut tail = zero_tail(total, lower)?;
            let css = css.encode()?;
            if css.len() > tail.len() {
                return Err(Error::InvalidLength);
            }
            tail[..css.len()].copy_from_slice(&css);
            (lower, tail, crc_off)
        }
        GtsPacket::ConnectAck {
            initiator_receive_tunnel,
            responder_receive_tunnel,
            responder_reset_id,
            status,
            initial_receive_credit,
        } => {
            let lower = GtsLowerPacket::ConnectAck {
                initiator_receive_tunnel: *initiator_receive_tunnel,
                responder_receive_tunnel: *responder_receive_tunnel,
                responder_reset_id: *responder_reset_id,
                status: *status,
                initial_receive_credit: *initial_receive_credit,
                reserved: 0,
            };
            (lower, zero_tail(total, lower)?, crc_off)
        }
        GtsPacket::StreamOpen {
            tunnel_id,
            stream_id,
            profile: sp,
            initial_receive_credit,
        } => {
            let sp = sp.validate()?;
            if sp.unreliable && *initial_receive_credit != 0 {
                return Err(Error::ProfileViolation);
            }
            let lower = GtsLowerPacket::StreamOpen {
                tunnel_id: *tunnel_id,
                stream_id: *stream_id,
                profile: sp.to_wire()?,
                initial_receive_credit: *initial_receive_credit,
                reserved: 0,
            };
            (lower, zero_tail(total, lower)?, crc_off)
        }
        GtsPacket::StreamAck {
            tunnel_id,
            stream_id,
            status,
            initial_receive_credit,
        } => {
            let lower = GtsLowerPacket::StreamAck {
                tunnel_id: *tunnel_id,
                stream_id: *stream_id,
                status: *status,
                initial_receive_credit: *initial_receive_credit,
                reserved: 0,
            };
            (lower, zero_tail(total, lower)?, crc_off)
        }
        GtsPacket::Data {
            tunnel_id,
            stream_id,
            sequence,
            data,
            end,
        } => {
            let sp = profile.ok_or(Error::ProfileViolation)?.validate()?;
            if sp.unreliable || (!sp.variable && ctx.size_class != sp.size_class) {
                return Err(Error::ProfileViolation);
            }
            if sp.variable && (ctx.size_class as u8) > (sp.size_class as u8) {
                return Err(Error::ProfileViolation);
            }

            // DATA's P4 lower form always includes a two-byte option probe.
            // Build the bytes after the fixed 10-byte DATA prefix once. For a
            // variable stream / DATA_END the first two bytes are valid length;
            // for fixed DATA they are simply the first two payload bytes.
            if crc_off < 12 {
                return Err(Error::InvalidLength);
            }
            let mut rest = vec![0u8; crc_off - 10];
            let mut pos = 0usize;
            if sp.variable || *end {
                let n = u16::try_from(data.len()).map_err(|_| Error::MessageTooLarge)?;
                rest[..2].copy_from_slice(&n.to_be_bytes());
                pos = 2;
            }
            if pos + data.len() > rest.len() {
                return Err(Error::MessageTooLarge);
            }
            rest[pos..pos + data.len()].copy_from_slice(data);
            pos += data.len();
            if !sp.variable && !*end && pos != rest.len() {
                return Err(Error::InvalidLength);
            }

            let lower = GtsLowerPacket::Data {
                tunnel_id: *tunnel_id,
                stream_id: *stream_id,
                sequence: *sequence,
                end: *end,
                option_probe: u16::from_be_bytes([rest[0], rest[1]]),
            };
            (lower, rest[2..].to_vec(), crc_off)
        }
        GtsPacket::Ack {
            tunnel_id,
            stream_id,
            ack_base,
            receive_bitmap,
            receive_credit,
        } => {
            let lower = GtsLowerPacket::Ack {
                tunnel_id: *tunnel_id,
                stream_id: *stream_id,
                ack_base: *ack_base,
                receive_bitmap: *receive_bitmap,
                receive_credit: *receive_credit,
                reserved: 0,
            };
            (lower, zero_tail(total, lower)?, crc_off)
        }
        GtsPacket::Datagram {
            tunnel_id,
            stream_id,
            sequence,
            data,
        } => {
            let sp = profile.ok_or(Error::ProfileViolation)?.validate()?;
            if !sp.unreliable || (!sp.variable && ctx.size_class != sp.size_class) {
                return Err(Error::ProfileViolation);
            }
            if sp.variable && (ctx.size_class as u8) > (sp.size_class as u8) {
                return Err(Error::ProfileViolation);
            }
            if crc_off < 12 {
                return Err(Error::InvalidLength);
            }

            // The P4 lower form probes six bytes after tunnel+stream. Build
            // that region once from sequence/length/payload, so both lower
            // encoders see the same profile-independent representation.
            let mut rest = vec![0u8; crc_off - 6];
            let mut pos = 0usize;
            if sp.sequenced {
                let seq = sequence.ok_or(Error::ProfileViolation)?;
                rest[..4].copy_from_slice(&seq.to_be_bytes());
                pos = 4;
            } else if sequence.is_some() {
                return Err(Error::ProfileViolation);
            }
            if sp.variable {
                let n = u16::try_from(data.len()).map_err(|_| Error::MessageTooLarge)?;
                rest[pos..pos + 2].copy_from_slice(&n.to_be_bytes());
                pos += 2;
            }
            let meta_end = 6 + pos;
            if pos + data.len() > rest.len() {
                return Err(Error::MessageTooLarge);
            }
            rest[pos..pos + data.len()].copy_from_slice(data);
            pos += data.len();
            if !sp.variable && pos != rest.len() {
                return Err(Error::InvalidLength);
            }

            let lower = GtsLowerPacket::Datagram {
                tunnel_id: *tunnel_id,
                stream_id: *stream_id,
                option_probe: rest[..6].try_into().map_err(|_| Error::InvalidLength)?,
            };
            let coverage = if sp.unchecked_payload { meta_end } else { crc_off };
            (lower, rest[6..].to_vec(), coverage)
        }
        GtsPacket::StreamClose {
            tunnel_id,
            stream_id,
            final_sequence,
            ack,
        } => {
            let lower = GtsLowerPacket::StreamClose {
                tunnel_id: *tunnel_id,
                stream_id: *stream_id,
                final_sequence: *final_sequence,
                ack: *ack,
            };
            (lower, zero_tail(total, lower)?, crc_off)
        }
        GtsPacket::TunnelClose { tunnel_id, ack } => {
            let lower = GtsLowerPacket::TunnelClose {
                tunnel_id: *tunnel_id,
                ack: *ack,
            };
            (lower, zero_tail(total, lower)?, crc_off)
        }
        GtsPacket::Reset {
            tunnel_id,
            reset_id,
            reason,
        } => {
            let lower = GtsLowerPacket::Reset {
                tunnel_id: *tunnel_id,
                reset_id: *reset_id,
                reason: *reason,
            };
            (lower, zero_tail(total, lower)?, crc_off)
        }
        GtsPacket::StreamReset {
            tunnel_id,
            stream_id,
            reason,
            ack,
        } => {
            let lower = GtsLowerPacket::StreamReset {
                tunnel_id: *tunnel_id,
                stream_id: *stream_id,
                reason: *reason,
                reserved: 0,
                ack: *ack,
            };
            (lower, zero_tail(total, lower)?, crc_off)
        }
    };

    let prefix_len = lower_prefix_len(lower.packet_type())?;
    if prefix_len + tail.len() != crc_off || crc_coverage_end > crc_off {
        return Err(Error::InvalidLength);
    }

    Ok(GtsTxPlan {
        lower,
        tail,
        crc_coverage_end,
    })
}

fn finish_encode<E: GtsLowerEncoder>(plan: GtsTxPlan, ctx: GtsContext) -> Result<Vec<u8>> {
    let total = ctx.size_class.bytes();
    let crc_off = total.checked_sub(4).ok_or(Error::InvalidLength)?;
    let prefix_len = lower_prefix_len(plan.lower.packet_type())?;
    let prefix = E::encode_lower(plan.lower)?;
    if prefix.len() != prefix_len || prefix_len + plan.tail.len() != crc_off {
        return Err(Error::InvalidLength);
    }

    let mut out = vec![0u8; total];
    out[..prefix_len].copy_from_slice(&prefix);
    out[prefix_len..crc_off].copy_from_slice(&plan.tail);
    let crc = compute_crc(ctx, &out[..plan.crc_coverage_end]);
    out[crc_off..].copy_from_slice(&crc.to_be_bytes());
    Ok(out)
}

fn encode_with<E: GtsLowerEncoder>(
    packet: &GtsPacket,
    ctx: GtsContext,
    profile: Option<StreamProfile>,
) -> Result<Vec<u8>> {
    finish_encode::<E>(prepare_encode(packet, ctx, profile)?, ctx)
}

pub(crate) fn encode_packet(
    packet: &GtsPacket,
    ctx: GtsContext,
    profile: Option<StreamProfile>,
) -> Result<Vec<u8>> {
    #[cfg(feature = "p4-gts-compare")]
    {
        use crate::p4_gts::P4GtsLower;
        use crate::wire::gts_handwritten::HandwrittenGtsLower;

        let handwritten = encode_with::<HandwrittenGtsLower>(packet, ctx, profile);
        let p4 = encode_with::<P4GtsLower>(packet, ctx, profile);
        if handwritten != p4 {
            return Err(Error::InvalidField);
        }
        return p4;
    }

    #[cfg(all(feature = "p4-gts", not(feature = "p4-gts-compare")))]
    {
        use crate::p4_gts::P4GtsLower;
        return encode_with::<P4GtsLower>(packet, ctx, profile);
    }

    #[cfg(not(feature = "p4-gts"))]
    {
        use crate::wire::gts_handwritten::HandwrittenGtsLower;
        encode_with::<HandwrittenGtsLower>(packet, ctx, profile)
    }
}

#[cfg(feature = "p4-gts")]
pub(crate) fn encode_handwritten_reference(
    packet: &GtsPacket,
    ctx: GtsContext,
    profile: Option<StreamProfile>,
) -> Result<Vec<u8>> {
    use crate::wire::gts_handwritten::HandwrittenGtsLower;
    encode_with::<HandwrittenGtsLower>(packet, ctx, profile)
}

#[cfg(feature = "p4-gts")]
pub(crate) fn encode_p4_reference(
    packet: &GtsPacket,
    ctx: GtsContext,
    profile: Option<StreamProfile>,
) -> Result<Vec<u8>> {
    use crate::p4_gts::P4GtsLower;
    encode_with::<P4GtsLower>(packet, ctx, profile)
}
