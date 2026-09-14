#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::error::{Error, Result};
use crate::wire::crc::Crc32;
use crate::wire::css::ServiceSelector;
use crate::wire::gdp::{GdpAddress, SizeClass};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GtsType {
    Reserved = 0,
    Connect = 1,
    ConnectAck = 2,
    StreamOpen = 3,
    StreamAck = 4,
    Data = 5,
    Ack = 6,
    DataEnd = 7,
    StreamClose = 8,
    StreamCloseAck = 9,
    TunnelClose = 10,
    TunnelCloseAck = 11,
    Reset = 12,
    Datagram = 13,
    StreamReset = 14,
    StreamResetAck = 15,
}

impl GtsType {
    pub fn from_wire(v: u8) -> Result<Self> {
        use GtsType::*;
        Ok(match v & 0xf {
            0 => Reserved,
            1 => Connect,
            2 => ConnectAck,
            3 => StreamOpen,
            4 => StreamAck,
            5 => Data,
            6 => Ack,
            7 => DataEnd,
            8 => StreamClose,
            9 => StreamCloseAck,
            10 => TunnelClose,
            11 => TunnelCloseAck,
            12 => Reset,
            13 => Datagram,
            14 => StreamReset,
            15 => StreamResetAck,
            _ => unreachable!(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    OpenerToPeer,
    PeerToOpener,
    Bidirectional,
}

impl Direction {
    pub const fn bits(self) -> u16 {
        match self {
            Self::OpenerToPeer => 1,
            Self::PeerToOpener => 2,
            Self::Bidirectional => 3,
        }
    }

    pub fn from_bits(v: u16) -> Result<Self> {
        match v {
            1 => Ok(Self::OpenerToPeer),
            2 => Ok(Self::PeerToOpener),
            3 => Ok(Self::Bidirectional),
            _ => Err(Error::ProfileViolation),
        }
    }

    pub const fn opener_may_send(self) -> bool {
        matches!(self, Self::OpenerToPeer | Self::Bidirectional)
    }

    pub const fn peer_may_send(self) -> bool {
        matches!(self, Self::PeerToOpener | Self::Bidirectional)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamProfile {
    pub unreliable: bool,
    pub variable: bool,
    pub sequenced: bool,
    pub unchecked_payload: bool,
    pub direction: Direction,
    pub size_class: SizeClass,
}

impl StreamProfile {
    pub fn reliable_fixed(size_class: SizeClass, direction: Direction) -> Self {
        Self {
            unreliable: false,
            variable: false,
            sequenced: false,
            unchecked_payload: false,
            direction,
            size_class,
        }
    }

    pub fn reliable_variable(max_class: SizeClass, direction: Direction) -> Self {
        Self {
            unreliable: false,
            variable: true,
            sequenced: false,
            unchecked_payload: false,
            direction,
            size_class: max_class,
        }
    }

    pub fn unreliable_fixed(
        size_class: SizeClass,
        direction: Direction,
        sequenced: bool,
        unchecked_payload: bool,
    ) -> Self {
        Self {
            unreliable: true,
            variable: false,
            sequenced,
            unchecked_payload,
            direction,
            size_class,
        }
    }

    pub fn unreliable_variable(
        max_class: SizeClass,
        direction: Direction,
        sequenced: bool,
        unchecked_payload: bool,
    ) -> Self {
        Self {
            unreliable: true,
            variable: true,
            sequenced,
            unchecked_payload,
            direction,
            size_class: max_class,
        }
    }

    pub fn validate(self) -> Result<Self> {
        if !self.unreliable && (self.sequenced || self.unchecked_payload) {
            return Err(Error::ProfileViolation);
        }
        Ok(self)
    }

    pub fn to_wire(self) -> Result<u16> {
        self.validate()?;
        Ok(((self.unreliable as u16) << 15)
            | ((self.variable as u16) << 14)
            | ((self.sequenced as u16) << 13)
            | ((self.unchecked_payload as u16) << 12)
            | (self.direction.bits() << 10)
            | (self.size_class as u8 as u16))
    }

    pub fn from_wire(v: u16) -> Result<Self> {
        if v & 0x03f0 != 0 {
            return Err(Error::ProfileViolation);
        }
        Self {
            unreliable: v & 0x8000 != 0,
            variable: v & 0x4000 != 0,
            sequenced: v & 0x2000 != 0,
            unchecked_payload: v & 0x1000 != 0,
            direction: Direction::from_bits((v >> 10) & 3)?,
            size_class: SizeClass::from_wire((v & 0xf) as u8)?,
        }
        .validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GtsContext {
    pub gdp_version: u8,
    pub size_class: SizeClass,
    pub source: GdpAddress,
    pub destination: GdpAddress,
}

/// Profile-independent GTS fields produced by either lower wire decoder.
///
/// DATA always probes two octets after sequence. For a variable stream (and
/// always DATA_END) those octets are the valid-data length; for fixed DATA
/// they are simply the first two payload octets and the common layer ignores
/// the probe. DATAGRAM similarly probes six octets so every combination of
/// sequenced/variable can be interpreted after the negotiated profile is
/// supplied by the higher GTS layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum GtsLowerPacket {
    Connect {
        initiator_receive_tunnel: u32,
        initiator_reset_id: u32,
        profile: u16,
        initial_receive_credit: u8,
    },
    ConnectAck {
        initiator_receive_tunnel: u32,
        responder_receive_tunnel: u32,
        responder_reset_id: u32,
        status: u8,
        initial_receive_credit: u8,
        reserved: u8,
    },
    StreamOpen {
        tunnel_id: u32,
        stream_id: u8,
        profile: u16,
        initial_receive_credit: u8,
        reserved: u8,
    },
    StreamAck {
        tunnel_id: u32,
        stream_id: u8,
        status: u8,
        initial_receive_credit: u8,
        reserved: u8,
    },
    Data {
        tunnel_id: u32,
        stream_id: u8,
        sequence: u32,
        end: bool,
        option_probe: u16,
    },
    Ack {
        tunnel_id: u32,
        stream_id: u8,
        ack_base: u32,
        receive_bitmap: u32,
        receive_credit: u8,
        reserved: u8,
    },
    Datagram {
        tunnel_id: u32,
        stream_id: u8,
        option_probe: [u8; 6],
    },
    StreamClose {
        tunnel_id: u32,
        stream_id: u8,
        final_sequence: u32,
        ack: bool,
    },
    TunnelClose {
        tunnel_id: u32,
        ack: bool,
    },
    Reset {
        tunnel_id: u32,
        reset_id: u32,
        reason: u8,
    },
    StreamReset {
        tunnel_id: u32,
        stream_id: u8,
        reason: u8,
        reserved: u8,
        ack: bool,
    },
}

impl GtsLowerPacket {
    pub const fn packet_type(self) -> GtsType {
        match self {
            Self::Connect { .. } => GtsType::Connect,
            Self::ConnectAck { .. } => GtsType::ConnectAck,
            Self::StreamOpen { .. } => GtsType::StreamOpen,
            Self::StreamAck { .. } => GtsType::StreamAck,
            Self::Data { end: false, .. } => GtsType::Data,
            Self::Data { end: true, .. } => GtsType::DataEnd,
            Self::Ack { .. } => GtsType::Ack,
            Self::Datagram { .. } => GtsType::Datagram,
            Self::StreamClose { ack: false, .. } => GtsType::StreamClose,
            Self::StreamClose { ack: true, .. } => GtsType::StreamCloseAck,
            Self::TunnelClose { ack: false, .. } => GtsType::TunnelClose,
            Self::TunnelClose { ack: true, .. } => GtsType::TunnelCloseAck,
            Self::Reset { .. } => GtsType::Reset,
            Self::StreamReset { ack: false, .. } => GtsType::StreamReset,
            Self::StreamReset { ack: true, .. } => GtsType::StreamResetAck,
        }
    }
}

pub(crate) trait GtsLowerDecoder {
    fn parse_lower(buf: &[u8]) -> Result<GtsLowerPacket>;
}

pub(crate) fn lower_prefix_len(ty: GtsType) -> Result<usize> {
    Ok(match ty {
        GtsType::Connect => 12,
        GtsType::ConnectAck => 16,
        GtsType::StreamOpen => 10,
        GtsType::StreamAck => 9,
        GtsType::Data | GtsType::DataEnd => 12,
        GtsType::Ack => 16,
        GtsType::Datagram => 12,
        GtsType::StreamClose | GtsType::StreamCloseAck => 10,
        GtsType::TunnelClose | GtsType::TunnelCloseAck => 5,
        GtsType::Reset => 10,
        GtsType::StreamReset | GtsType::StreamResetAck => 8,
        GtsType::Reserved => return Err(Error::Unsupported),
    })
}

#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GtsPacket {
    Connect {
        initiator_receive_tunnel: u32,
        initiator_reset_id: u32,
        profile: StreamProfile,
        initial_receive_credit: u8,
        css: ServiceSelector,
    },
    ConnectAck {
        initiator_receive_tunnel: u32,
        responder_receive_tunnel: u32,
        responder_reset_id: u32,
        status: u8,
        initial_receive_credit: u8,
    },
    StreamOpen {
        tunnel_id: u32,
        stream_id: u8,
        profile: StreamProfile,
        initial_receive_credit: u8,
    },
    StreamAck {
        tunnel_id: u32,
        stream_id: u8,
        status: u8,
        initial_receive_credit: u8,
    },
    Data {
        tunnel_id: u32,
        stream_id: u8,
        sequence: u32,
        data: Vec<u8>,
        end: bool,
    },
    Ack {
        tunnel_id: u32,
        stream_id: u8,
        ack_base: u32,
        receive_bitmap: u32,
        receive_credit: u8,
    },
    Datagram {
        tunnel_id: u32,
        stream_id: u8,
        sequence: Option<u32>,
        data: Vec<u8>,
    },
    StreamClose {
        tunnel_id: u32,
        stream_id: u8,
        final_sequence: u32,
        ack: bool,
    },
    TunnelClose {
        tunnel_id: u32,
        ack: bool,
    },
    Reset {
        tunnel_id: u32,
        reset_id: u32,
        reason: u8,
    },
    StreamReset {
        tunnel_id: u32,
        stream_id: u8,
        reason: u8,
        ack: bool,
    },
}

#[cfg(feature = "alloc")]
impl GtsPacket {
    pub fn packet_type(&self) -> GtsType {
        match self {
            Self::Connect { .. } => GtsType::Connect,
            Self::ConnectAck { .. } => GtsType::ConnectAck,
            Self::StreamOpen { .. } => GtsType::StreamOpen,
            Self::StreamAck { .. } => GtsType::StreamAck,
            Self::Data { end: false, .. } => GtsType::Data,
            Self::Data { end: true, .. } => GtsType::DataEnd,
            Self::Ack { .. } => GtsType::Ack,
            Self::Datagram { .. } => GtsType::Datagram,
            Self::StreamClose { ack: false, .. } => GtsType::StreamClose,
            Self::StreamClose { ack: true, .. } => GtsType::StreamCloseAck,
            Self::TunnelClose { ack: false, .. } => GtsType::TunnelClose,
            Self::TunnelClose { ack: true, .. } => GtsType::TunnelCloseAck,
            Self::Reset { .. } => GtsType::Reset,
            Self::StreamReset { ack: false, .. } => GtsType::StreamReset,
            Self::StreamReset { ack: true, .. } => GtsType::StreamResetAck,
        }
    }

    pub fn minimum_len(&self, profile: Option<StreamProfile>) -> Result<usize> {
        Ok(match self {
            Self::Connect { css, .. } => 1 + 4 + 4 + 2 + 1 + css.encode()?.len() + 4,
            Self::ConnectAck { .. } => 1 + 4 + 4 + 4 + 1 + 1 + 1 + 4,
            Self::StreamOpen { .. } => 1 + 4 + 1 + 2 + 1 + 1 + 4,
            Self::StreamAck { .. } => 1 + 4 + 1 + 1 + 1 + 1 + 4,
            Self::Ack { .. } => 20,
            Self::StreamClose { .. } => 1 + 4 + 1 + 4 + 4,
            Self::TunnelClose { .. } => 1 + 4 + 4,
            Self::Reset { .. } => 1 + 4 + 4 + 1 + 4,
            Self::StreamReset { .. } => 12,
            Self::Data { data, end, .. } => {
                let p = profile.ok_or(Error::ProfileViolation)?;
                if p.unreliable {
                    return Err(Error::ProfileViolation);
                }
                10 + if p.variable || *end { 2 } else { 0 } + data.len() + 4
            }
            Self::Datagram { data, .. } => {
                let p = profile.ok_or(Error::ProfileViolation)?;
                if !p.unreliable {
                    return Err(Error::ProfileViolation);
                }
                6 + if p.sequenced { 4 } else { 0 }
                    + if p.variable { 2 } else { 0 }
                    + data.len()
                    + 4
            }
        })
    }

    pub fn choose_size_class(&self, profile: Option<StreamProfile>) -> Result<SizeClass> {
        if let Some(p) = profile {
            match self {
                Self::Data { end, .. } => {
                    if !p.variable {
                        let min = self.minimum_len(Some(p))?;
                        if *end {
                            if min > p.size_class.bytes() {
                                return Err(Error::MessageTooLarge);
                            }
                        } else if min != p.size_class.bytes() {
                            return Err(Error::InvalidLength);
                        }
                        return Ok(p.size_class);
                    }
                    let need = self.minimum_len(Some(p))?;
                    let c = SizeClass::smallest_for(need).ok_or(Error::MessageTooLarge)?;
                    if (c as u8) > (p.size_class as u8) {
                        return Err(Error::MessageTooLarge);
                    }
                    return Ok(c);
                }
                Self::Datagram { .. } => {
                    if !p.variable {
                        let min = self.minimum_len(Some(p))?;
                        if min != p.size_class.bytes() {
                            return Err(Error::InvalidLength);
                        }
                        return Ok(p.size_class);
                    }
                    let need = self.minimum_len(Some(p))?;
                    let c = SizeClass::smallest_for(need).ok_or(Error::MessageTooLarge)?;
                    if (c as u8) > (p.size_class as u8) {
                        return Err(Error::MessageTooLarge);
                    }
                    return Ok(c);
                }
                _ => {}
            }
        }
        SizeClass::smallest_for(self.minimum_len(profile)?).ok_or(Error::MessageTooLarge)
    }

    pub fn encode(&self, ctx: GtsContext, profile: Option<StreamProfile>) -> Result<Vec<u8>> {
        crate::wire::gts_handwritten::encode_packet(self, ctx, profile)
    }

    pub fn decode(buf: &[u8], ctx: GtsContext, profile: Option<StreamProfile>) -> Result<Self> {
        if buf.len() != ctx.size_class.bytes() || buf.len() < 5 {
            return Err(Error::InvalidLength);
        }

        #[cfg(feature = "p4-gts-compare")]
        {
            use crate::p4_gts::P4GtsLower;
            use crate::wire::gts_handwritten::HandwrittenGtsLower;

            let handwritten = HandwrittenGtsLower::parse_lower(buf);
            let p4 = P4GtsLower::parse_lower(buf);
            if handwritten != p4 {
                return Err(Error::InvalidField);
            }
            return finish_decode(buf, ctx, profile, p4?);
        }

        #[cfg(all(feature = "p4-gts", not(feature = "p4-gts-compare")))]
        {
            use crate::p4_gts::P4GtsLower;
            return finish_decode(buf, ctx, profile, P4GtsLower::parse_lower(buf)?);
        }

        #[cfg(not(feature = "p4-gts"))]
        {
            use crate::wire::gts_handwritten::HandwrittenGtsLower;
            finish_decode(buf, ctx, profile, HandwrittenGtsLower::parse_lower(buf)?)
        }
    }

    #[cfg(feature = "p4-gts")]
    #[doc(hidden)]
    pub fn decode_handwritten_reference(
        buf: &[u8],
        ctx: GtsContext,
        profile: Option<StreamProfile>,
    ) -> Result<Self> {
        use crate::wire::gts_handwritten::HandwrittenGtsLower;
        if buf.len() != ctx.size_class.bytes() || buf.len() < 5 {
            return Err(Error::InvalidLength);
        }
        finish_decode(buf, ctx, profile, HandwrittenGtsLower::parse_lower(buf)?)
    }

    #[cfg(feature = "p4-gts")]
    #[doc(hidden)]
    pub fn decode_p4_reference(
        buf: &[u8],
        ctx: GtsContext,
        profile: Option<StreamProfile>,
    ) -> Result<Self> {
        use crate::p4_gts::P4GtsLower;
        if buf.len() != ctx.size_class.bytes() || buf.len() < 5 {
            return Err(Error::InvalidLength);
        }
        finish_decode(buf, ctx, profile, P4GtsLower::parse_lower(buf)?)
    }
}

#[cfg(feature = "alloc")]
fn finish_decode(
    buf: &[u8],
    ctx: GtsContext,
    profile: Option<StreamProfile>,
    lower: GtsLowerPacket,
) -> Result<GtsPacket> {
    let crc_off = buf.len() - 4;
    let ty = lower.packet_type();
    let mut meta_end = crc_off;

    let packet = match lower {
        GtsLowerPacket::Connect {
            initiator_receive_tunnel,
            initiator_reset_id,
            profile: profile_wire,
            initial_receive_credit,
        } => {
            let stream_profile = StreamProfile::from_wire(profile_wire)?;
            if stream_profile.unreliable && initial_receive_credit != 0 {
                return Err(Error::ProfileViolation);
            }
            let pos = 12;
            let (css, n) = ServiceSelector::decode(&buf[pos..crc_off])?;
            if buf[pos + n..crc_off].iter().any(|&b| b != 0) {
                return Err(Error::InvalidField);
            }
            GtsPacket::Connect {
                initiator_receive_tunnel,
                initiator_reset_id,
                profile: stream_profile,
                initial_receive_credit,
                css,
            }
        }
        GtsLowerPacket::ConnectAck {
            initiator_receive_tunnel,
            responder_receive_tunnel,
            responder_reset_id,
            status,
            initial_receive_credit,
            reserved,
        } => {
            if reserved != 0 {
                return Err(Error::InvalidField);
            }
            GtsPacket::ConnectAck {
                initiator_receive_tunnel,
                responder_receive_tunnel,
                responder_reset_id,
                status,
                initial_receive_credit,
            }
        }
        GtsLowerPacket::StreamOpen {
            tunnel_id,
            stream_id,
            profile: profile_wire,
            initial_receive_credit,
            reserved,
        } => {
            let stream_profile = StreamProfile::from_wire(profile_wire)?;
            if reserved != 0 || (stream_profile.unreliable && initial_receive_credit != 0) {
                return Err(Error::ProfileViolation);
            }
            GtsPacket::StreamOpen {
                tunnel_id,
                stream_id,
                profile: stream_profile,
                initial_receive_credit,
            }
        }
        GtsLowerPacket::StreamAck {
            tunnel_id,
            stream_id,
            status,
            initial_receive_credit,
            reserved,
        } => {
            if reserved != 0 {
                return Err(Error::InvalidField);
            }
            GtsPacket::StreamAck {
                tunnel_id,
                stream_id,
                status,
                initial_receive_credit,
            }
        }
        GtsLowerPacket::Data {
            tunnel_id,
            stream_id,
            sequence,
            end,
            option_probe,
        } => {
            let sp = profile.ok_or(Error::ProfileViolation)?.validate()?;
            if sp.unreliable
                || (!sp.variable && ctx.size_class != sp.size_class)
                || (sp.variable && (ctx.size_class as u8) > (sp.size_class as u8))
            {
                return Err(Error::ProfileViolation);
            }

            let (pos, valid) = if end || sp.variable {
                (12usize, Some(option_probe as usize))
            } else {
                (10usize, None)
            };
            meta_end = pos;
            let n = valid.unwrap_or(crc_off - pos);
            if pos + n > crc_off {
                return Err(Error::InvalidLength);
            }
            let data = buf[pos..pos + n].to_vec();
            if valid.is_some() && buf[pos + n..crc_off].iter().any(|&b| b != 0) {
                return Err(Error::InvalidField);
            }
            GtsPacket::Data {
                tunnel_id,
                stream_id,
                sequence,
                data,
                end,
            }
        }
        GtsLowerPacket::Ack {
            tunnel_id,
            stream_id,
            ack_base,
            receive_bitmap,
            receive_credit,
            reserved,
        } => {
            if reserved != 0 {
                return Err(Error::InvalidField);
            }
            GtsPacket::Ack {
                tunnel_id,
                stream_id,
                ack_base,
                receive_bitmap,
                receive_credit,
            }
        }
        GtsLowerPacket::Datagram {
            tunnel_id,
            stream_id,
            option_probe,
        } => {
            let sp = profile.ok_or(Error::ProfileViolation)?.validate()?;
            if !sp.unreliable
                || (!sp.variable && ctx.size_class != sp.size_class)
                || (sp.variable && (ctx.size_class as u8) > (sp.size_class as u8))
            {
                return Err(Error::ProfileViolation);
            }

            let mut option_pos = 0usize;
            let sequence = if sp.sequenced {
                let value = u32::from_be_bytes([
                    option_probe[0],
                    option_probe[1],
                    option_probe[2],
                    option_probe[3],
                ]);
                option_pos = 4;
                Some(value)
            } else {
                None
            };
            let valid = if sp.variable {
                let value = u16::from_be_bytes([
                    option_probe[option_pos],
                    option_probe[option_pos + 1],
                ]) as usize;
                option_pos += 2;
                Some(value)
            } else {
                None
            };
            let pos = 6 + option_pos;
            meta_end = pos;
            let n = valid.unwrap_or(crc_off - pos);
            if pos + n > crc_off {
                return Err(Error::InvalidLength);
            }
            let data = buf[pos..pos + n].to_vec();
            if valid.is_some() && buf[pos + n..crc_off].iter().any(|&b| b != 0) {
                return Err(Error::InvalidField);
            }
            GtsPacket::Datagram {
                tunnel_id,
                stream_id,
                sequence,
                data,
            }
        }
        GtsLowerPacket::StreamClose {
            tunnel_id,
            stream_id,
            final_sequence,
            ack,
        } => GtsPacket::StreamClose {
            tunnel_id,
            stream_id,
            final_sequence,
            ack,
        },
        GtsLowerPacket::TunnelClose { tunnel_id, ack } => {
            GtsPacket::TunnelClose { tunnel_id, ack }
        }
        GtsLowerPacket::Reset {
            tunnel_id,
            reset_id,
            reason,
        } => GtsPacket::Reset {
            tunnel_id,
            reset_id,
            reason,
        },
        GtsLowerPacket::StreamReset {
            tunnel_id,
            stream_id,
            reason,
            reserved,
            ack,
        } => {
            if reserved != 0 {
                return Err(Error::InvalidField);
            }
            GtsPacket::StreamReset {
                tunnel_id,
                stream_id,
                reason,
                ack,
            }
        }
    };

    let unchecked = profile
        .map(|sp| sp.unreliable && sp.unchecked_payload)
        .unwrap_or(false)
        && ty == GtsType::Datagram;
    let got = u32::from_be_bytes(
        buf[crc_off..]
            .try_into()
            .map_err(|_| Error::InvalidLength)?,
    );
    let want = compute_crc(ctx, &buf[..if unchecked { meta_end } else { crc_off }]);
    if got != want {
        return Err(Error::InvalidCrc);
    }

    Ok(packet)
}

pub(crate) fn compute_crc(ctx: GtsContext, gts: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update_bits((ctx.gdp_version & 3) as u64, 2);
    c.update_bits(0x2, 4);
    c.update_bits(ctx.size_class as u8 as u64, 4);
    c.update_bits(ctx.source.0, 64);
    c.update_bits(ctx.destination.0, 64);
    c.update_bytes(gts);
    c.finalize()
}
