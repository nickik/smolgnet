#![cfg(feature = "p4-gts")]

use alloc::vec::Vec;
use bitvec::prelude::*;
use p4rs::{packet_in, Header};

use crate::error::Error;
use crate::wire::gts::{
    lower_prefix_len, GtsContext, GtsLowerDecoder, GtsLowerPacket, GtsPacket, GtsType,
    StreamProfile,
};
use crate::wire::gts_tx::GtsLowerEncoder;

p4_macro::use_p4!(
    p4 = "p4-gts/p4/gts.p4",
    pipeline_name = "gts",
);

/// x4c-backed lower GTS wire codec.
///
/// This deliberately owns only the type-dependent profile-independent wire
/// fields. Negotiated stream profiles, payload placement/padding, CRC policy
/// and all transport state are handled once by the common GTS semantic layers.
pub(crate) struct P4GtsLower;

impl GtsLowerDecoder for P4GtsLower {
    fn parse_lower(bytes: &[u8]) -> crate::error::Result<GtsLowerPacket> {
        if bytes.len() < 5 {
            return Err(Error::InvalidLength);
        }
        if bytes[0] >> 4 != 0 {
            return Err(Error::Unsupported);
        }

        let ty = GtsType::from_wire(bytes[0] & 0x0f)?;
        let prefix_len = lower_prefix_len(ty)?;
        if bytes.len() < prefix_len + 4 {
            return Err(Error::InvalidLength);
        }

        // p4rs packet_in::extract slices directly. The prefix-length check
        // above ensures malformed/truncated packets cannot panic in generated
        // parser code.
        let mut pkt = packet_in::new(bytes);
        let mut hdr = headers_t::default();
        let mut ingress = ingress_metadata_t::default();
        if !parse_start(&mut pkt, &mut hdr, &mut ingress) {
            return Err(Error::Unsupported);
        }

        let parsed_type = hdr.kind.packet_type.load_be::<u8>();
        if parsed_type != ty as u8 {
            return Err(Error::InvalidField);
        }

        Ok(match ty {
            GtsType::Connect => GtsLowerPacket::Connect {
                initiator_receive_tunnel: hdr.connect.initiator_receive_tunnel.load_le::<u32>(),
                initiator_reset_id: hdr.connect.initiator_reset_id.load_le::<u32>(),
                profile: hdr.connect.profile.load_le::<u16>(),
                initial_receive_credit: hdr.connect.initial_receive_credit.load_be::<u8>(),
            },
            GtsType::ConnectAck => GtsLowerPacket::ConnectAck {
                initiator_receive_tunnel: hdr.connect_ack.initiator_receive_tunnel.load_le::<u32>(),
                responder_receive_tunnel: hdr.connect_ack.responder_receive_tunnel.load_le::<u32>(),
                responder_reset_id: hdr.connect_ack.responder_reset_id.load_le::<u32>(),
                status: hdr.connect_ack.status.load_be::<u8>(),
                initial_receive_credit: hdr.connect_ack.initial_receive_credit.load_be::<u8>(),
                reserved: hdr.connect_ack.reserved.load_be::<u8>(),
            },
            GtsType::StreamOpen => GtsLowerPacket::StreamOpen {
                tunnel_id: hdr.stream_open.tunnel_id.load_le::<u32>(),
                stream_id: hdr.stream_open.stream_id.load_be::<u8>(),
                profile: hdr.stream_open.profile.load_le::<u16>(),
                initial_receive_credit: hdr.stream_open.initial_receive_credit.load_be::<u8>(),
                reserved: hdr.stream_open.reserved.load_be::<u8>(),
            },
            GtsType::StreamAck => GtsLowerPacket::StreamAck {
                tunnel_id: hdr.stream_ack.tunnel_id.load_le::<u32>(),
                stream_id: hdr.stream_ack.stream_id.load_be::<u8>(),
                status: hdr.stream_ack.status.load_be::<u8>(),
                initial_receive_credit: hdr.stream_ack.initial_receive_credit.load_be::<u8>(),
                reserved: hdr.stream_ack.reserved.load_be::<u8>(),
            },
            GtsType::Data | GtsType::DataEnd => GtsLowerPacket::Data {
                tunnel_id: hdr.data.tunnel_id.load_le::<u32>(),
                stream_id: hdr.data.stream_id.load_be::<u8>(),
                sequence: hdr.data.sequence.load_le::<u32>(),
                end: ty == GtsType::DataEnd,
                option_probe: hdr.data_option.option_probe.load_le::<u16>(),
            },
            GtsType::Ack => GtsLowerPacket::Ack {
                tunnel_id: hdr.ack.tunnel_id.load_le::<u32>(),
                stream_id: hdr.ack.stream_id.load_be::<u8>(),
                ack_base: hdr.ack.ack_base.load_le::<u32>(),
                receive_bitmap: hdr.ack.receive_bitmap.load_le::<u32>(),
                receive_credit: hdr.ack.receive_credit.load_be::<u8>(),
                reserved: hdr.ack.reserved.load_be::<u8>(),
            },
            GtsType::Datagram => GtsLowerPacket::Datagram {
                tunnel_id: hdr.datagram.tunnel_id.load_le::<u32>(),
                stream_id: hdr.datagram.stream_id.load_be::<u8>(),
                option_probe: [
                    hdr.datagram_options.option0.load_be::<u8>(),
                    hdr.datagram_options.option1.load_be::<u8>(),
                    hdr.datagram_options.option2.load_be::<u8>(),
                    hdr.datagram_options.option3.load_be::<u8>(),
                    hdr.datagram_options.option4.load_be::<u8>(),
                    hdr.datagram_options.option5.load_be::<u8>(),
                ],
            },
            GtsType::StreamClose | GtsType::StreamCloseAck => GtsLowerPacket::StreamClose {
                tunnel_id: hdr.stream_close.tunnel_id.load_le::<u32>(),
                stream_id: hdr.stream_close.stream_id.load_be::<u8>(),
                final_sequence: hdr.stream_close.final_sequence.load_le::<u32>(),
                ack: ty == GtsType::StreamCloseAck,
            },
            GtsType::TunnelClose | GtsType::TunnelCloseAck => GtsLowerPacket::TunnelClose {
                tunnel_id: hdr.tunnel_close.tunnel_id.load_le::<u32>(),
                ack: ty == GtsType::TunnelCloseAck,
            },
            GtsType::Reset => GtsLowerPacket::Reset {
                tunnel_id: hdr.reset.tunnel_id.load_le::<u32>(),
                reset_id: hdr.reset.reset_id.load_le::<u32>(),
                reason: hdr.reset.reason.load_be::<u8>(),
            },
            GtsType::StreamReset | GtsType::StreamResetAck => GtsLowerPacket::StreamReset {
                tunnel_id: hdr.stream_reset.tunnel_id.load_le::<u32>(),
                stream_id: hdr.stream_reset.stream_id.load_be::<u8>(),
                reason: hdr.stream_reset.reason.load_be::<u8>(),
                reserved: hdr.stream_reset.reserved.load_be::<u8>(),
                ack: ty == GtsType::StreamResetAck,
            },
            GtsType::Reserved => return Err(Error::Unsupported),
        })
    }
}

fn set_header<H: Header>(header: &mut H, bytes: &[u8]) -> crate::error::Result<()> {
    header.set(bytes).map_err(|_| Error::InvalidLength)?;
    header.set_valid();
    Ok(())
}

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_be_bytes());
}

impl GtsLowerEncoder for P4GtsLower {
    fn encode_lower(lower: GtsLowerPacket) -> crate::error::Result<Vec<u8>> {
        let mut hdr = headers_t::default();
        set_header(&mut hdr.kind, &[lower.packet_type() as u8])?;

        match lower {
            GtsLowerPacket::Connect {
                initiator_receive_tunnel,
                initiator_reset_id,
                profile,
                initial_receive_credit,
            } => {
                let mut b = Vec::with_capacity(11);
                push_u32(&mut b, initiator_receive_tunnel);
                push_u32(&mut b, initiator_reset_id);
                push_u16(&mut b, profile);
                b.push(initial_receive_credit);
                set_header(&mut hdr.connect, &b)?;
            }
            GtsLowerPacket::ConnectAck {
                initiator_receive_tunnel,
                responder_receive_tunnel,
                responder_reset_id,
                status,
                initial_receive_credit,
                reserved,
            } => {
                let mut b = Vec::with_capacity(15);
                push_u32(&mut b, initiator_receive_tunnel);
                push_u32(&mut b, responder_receive_tunnel);
                push_u32(&mut b, responder_reset_id);
                b.extend_from_slice(&[status, initial_receive_credit, reserved]);
                set_header(&mut hdr.connect_ack, &b)?;
            }
            GtsLowerPacket::StreamOpen {
                tunnel_id,
                stream_id,
                profile,
                initial_receive_credit,
                reserved,
            } => {
                let mut b = Vec::with_capacity(9);
                push_u32(&mut b, tunnel_id);
                b.push(stream_id);
                push_u16(&mut b, profile);
                b.extend_from_slice(&[initial_receive_credit, reserved]);
                set_header(&mut hdr.stream_open, &b)?;
            }
            GtsLowerPacket::StreamAck {
                tunnel_id,
                stream_id,
                status,
                initial_receive_credit,
                reserved,
            } => {
                let mut b = Vec::with_capacity(8);
                push_u32(&mut b, tunnel_id);
                b.extend_from_slice(&[stream_id, status, initial_receive_credit, reserved]);
                set_header(&mut hdr.stream_ack, &b)?;
            }
            GtsLowerPacket::Data {
                tunnel_id,
                stream_id,
                sequence,
                option_probe,
                ..
            } => {
                let mut b = Vec::with_capacity(9);
                push_u32(&mut b, tunnel_id);
                b.push(stream_id);
                push_u32(&mut b, sequence);
                set_header(&mut hdr.data, &b)?;
                set_header(&mut hdr.data_option, &option_probe.to_be_bytes())?;
            }
            GtsLowerPacket::Ack {
                tunnel_id,
                stream_id,
                ack_base,
                receive_bitmap,
                receive_credit,
                reserved,
            } => {
                let mut b = Vec::with_capacity(15);
                push_u32(&mut b, tunnel_id);
                b.push(stream_id);
                push_u32(&mut b, ack_base);
                push_u32(&mut b, receive_bitmap);
                b.extend_from_slice(&[receive_credit, reserved]);
                set_header(&mut hdr.ack, &b)?;
            }
            GtsLowerPacket::Datagram {
                tunnel_id,
                stream_id,
                option_probe,
            } => {
                let mut b = Vec::with_capacity(5);
                push_u32(&mut b, tunnel_id);
                b.push(stream_id);
                set_header(&mut hdr.datagram, &b)?;
                set_header(&mut hdr.datagram_options, &option_probe)?;
            }
            GtsLowerPacket::StreamClose {
                tunnel_id,
                stream_id,
                final_sequence,
                ..
            } => {
                let mut b = Vec::with_capacity(9);
                push_u32(&mut b, tunnel_id);
                b.push(stream_id);
                push_u32(&mut b, final_sequence);
                set_header(&mut hdr.stream_close, &b)?;
            }
            GtsLowerPacket::TunnelClose { tunnel_id, .. } => {
                set_header(&mut hdr.tunnel_close, &tunnel_id.to_be_bytes())?;
            }
            GtsLowerPacket::Reset {
                tunnel_id,
                reset_id,
                reason,
            } => {
                let mut b = Vec::with_capacity(9);
                push_u32(&mut b, tunnel_id);
                push_u32(&mut b, reset_id);
                b.push(reason);
                set_header(&mut hdr.reset, &b)?;
            }
            GtsLowerPacket::StreamReset {
                tunnel_id,
                stream_id,
                reason,
                reserved,
                ..
            } => {
                let mut b = Vec::with_capacity(7);
                push_u32(&mut b, tunnel_id);
                b.extend_from_slice(&[stream_id, reason, reserved]);
                set_header(&mut hdr.stream_reset, &b)?;
            }
        }

        // x4c's SoftNPU generated deparser is the generated headers_t
        // serialization used by process_packet. Calling the same generated
        // to_bitvec path here lets endpoint TX use P4 without running ingress
        // tables or reparsing a synthetic packet.
        let bits = hdr.to_bitvec();
        let out = bits.as_raw_slice().to_vec();
        if out.len() != lower_prefix_len(lower.packet_type())? {
            return Err(Error::InvalidLength);
        }
        Ok(out)
    }
}

/// Public test/benchmark entry point. Production GTS decoding goes through
/// GtsPacket::decode so the common semantic layer is always applied.
#[doc(hidden)]
pub fn parse_lower_for_test(bytes: &[u8]) -> crate::error::Result<GtsLowerPacket> {
    P4GtsLower::parse_lower(bytes)
}

/// Encode through the complete common TX semantic layer and handwritten lower
/// backend. Useful as the independent oracle for P4 TX conformance tests.
#[doc(hidden)]
pub fn encode_handwritten_reference(
    packet: &GtsPacket,
    ctx: GtsContext,
    profile: Option<StreamProfile>,
) -> crate::error::Result<Vec<u8>> {
    crate::wire::gts_tx::encode_handwritten_reference(packet, ctx, profile)
}

/// Encode through the complete common TX semantic layer and x4c lower backend.
#[doc(hidden)]
pub fn encode_p4_reference(
    packet: &GtsPacket,
    ctx: GtsContext,
    profile: Option<StreamProfile>,
) -> crate::error::Result<Vec<u8>> {
    crate::wire::gts_tx::encode_p4_reference(packet, ctx, profile)
}
