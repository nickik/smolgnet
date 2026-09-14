use alloc::vec;
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
        let total = ctx.size_class.bytes();
        if total < 4 {
            return Err(Error::InvalidLength);
        }
        let ty = self.packet_type();
        let mut o = vec![0u8; total];
        o[0] = ty as u8;
        let crc_off = total - 4;
        let mut meta_end = crc_off;
        let mut pos = 1;

        macro_rules! u32w {
            ($v:expr) => {{
                if pos + 4 > crc_off { return Err(Error::InvalidLength); }
                o[pos..pos + 4].copy_from_slice(&$v.to_be_bytes());
                pos += 4;
            }};
        }
        macro_rules! u16w {
            ($v:expr) => {{
                if pos + 2 > crc_off { return Err(Error::InvalidLength); }
                o[pos..pos + 2].copy_from_slice(&$v.to_be_bytes());
                pos += 2;
            }};
        }
        macro_rules! u8w {
            ($v:expr) => {{
                if pos >= crc_off { return Err(Error::InvalidLength); }
                o[pos] = $v;
                pos += 1;
            }};
        }

        match self {
            Self::Connect {
                initiator_receive_tunnel,
                initiator_reset_id,
                profile: sp,
                initial_receive_credit,
                css,
            } => {
                sp.validate()?;
                if sp.unreliable && *initial_receive_credit != 0 {
                    return Err(Error::ProfileViolation);
                }
                u32w!(*initiator_receive_tunnel);
                u32w!(*initiator_reset_id);
                u16w!(sp.to_wire()?);
                u8w!(*initial_receive_credit);
                let enc = css.encode()?;
                if pos + enc.len() > crc_off {
                    return Err(Error::InvalidLength);
                }
                o[pos..pos + enc.len()].copy_from_slice(&enc);
                pos += enc.len();
            }
            Self::ConnectAck {
                initiator_receive_tunnel,
                responder_receive_tunnel,
                responder_reset_id,
                status,
                initial_receive_credit,
            } => {
                u32w!(*initiator_receive_tunnel);
                u32w!(*responder_receive_tunnel);
                u32w!(*responder_reset_id);
                u8w!(*status);
                u8w!(*initial_receive_credit);
                u8w!(0);
            }
            Self::StreamOpen {
                tunnel_id,
                stream_id,
                profile: sp,
                initial_receive_credit,
            } => {
                sp.validate()?;
                if sp.unreliable && *initial_receive_credit != 0 {
                    return Err(Error::ProfileViolation);
                }
                u32w!(*tunnel_id);
                u8w!(*stream_id);
                u16w!(sp.to_wire()?);
                u8w!(*initial_receive_credit);
                u8w!(0);
            }
            Self::StreamAck {
                tunnel_id,
                stream_id,
                status,
                initial_receive_credit,
            } => {
                u32w!(*tunnel_id);
                u8w!(*stream_id);
                u8w!(*status);
                u8w!(*initial_receive_credit);
                u8w!(0);
            }
            Self::Data {
                tunnel_id,
                stream_id,
                sequence,
                data,
                end,
            } => {
                let p = profile.ok_or(Error::ProfileViolation)?.validate()?;
                if p.unreliable || (!p.variable && ctx.size_class != p.size_class) {
                    return Err(Error::ProfileViolation);
                }
                if p.variable && (ctx.size_class as u8) > (p.size_class as u8) {
                    return Err(Error::ProfileViolation);
                }
                u32w!(*tunnel_id);
                u8w!(*stream_id);
                u32w!(*sequence);
                if p.variable || *end {
                    u16w!(data.len() as u16);
                }
                meta_end = pos;
                if pos + data.len() > crc_off {
                    return Err(Error::MessageTooLarge);
                }
                o[pos..pos + data.len()].copy_from_slice(data);
                pos += data.len();
                if !p.variable && !*end && pos != crc_off {
                    return Err(Error::InvalidLength);
                }
            }
            Self::Ack {
                tunnel_id,
                stream_id,
                ack_base,
                receive_bitmap,
                receive_credit,
            } => {
                u32w!(*tunnel_id);
                u8w!(*stream_id);
                u32w!(*ack_base);
                u32w!(*receive_bitmap);
                u8w!(*receive_credit);
                u8w!(0);
            }
            Self::Datagram {
                tunnel_id,
                stream_id,
                sequence,
                data,
            } => {
                let p = profile.ok_or(Error::ProfileViolation)?.validate()?;
                if !p.unreliable || (!p.variable && ctx.size_class != p.size_class) {
                    return Err(Error::ProfileViolation);
                }
                if p.variable && (ctx.size_class as u8) > (p.size_class as u8) {
                    return Err(Error::ProfileViolation);
                }
                u32w!(*tunnel_id);
                u8w!(*stream_id);
                if p.sequenced {
                    u32w!(sequence.ok_or(Error::ProfileViolation)?);
                } else if sequence.is_some() {
                    return Err(Error::ProfileViolation);
                }
                if p.variable {
                    u16w!(data.len() as u16);
                }
                meta_end = pos;
                if pos + data.len() > crc_off {
                    return Err(Error::MessageTooLarge);
                }
                o[pos..pos + data.len()].copy_from_slice(data);
                pos += data.len();
                if !p.variable && pos != crc_off {
                    return Err(Error::InvalidLength);
                }
            }
            Self::StreamClose {
                tunnel_id,
                stream_id,
                final_sequence,
                ..
            } => {
                u32w!(*tunnel_id);
                u8w!(*stream_id);
                u32w!(*final_sequence);
            }
            Self::TunnelClose { tunnel_id, .. } => {
                u32w!(*tunnel_id);
            }
            Self::Reset {
                tunnel_id,
                reset_id,
                reason,
            } => {
                u32w!(*tunnel_id);
                u32w!(*reset_id);
                u8w!(*reason);
            }
            Self::StreamReset {
                tunnel_id,
                stream_id,
                reason,
                ..
            } => {
                u32w!(*tunnel_id);
                u8w!(*stream_id);
                u8w!(*reason);
                u8w!(0);
            }
        }

        let unchecked = profile
            .map(|p| p.unreliable && p.unchecked_payload)
            .unwrap_or(false)
            && matches!(self, Self::Datagram { .. });
        let crc = compute_crc(ctx, &o[..if unchecked { meta_end } else { crc_off }]);
        o[crc_off..].copy_from_slice(&crc.to_be_bytes());
        Ok(o)
    }

    pub fn decode(buf: &[u8], ctx: GtsContext, profile: Option<StreamProfile>) -> Result<Self> {
        if buf.len() != ctx.size_class.bytes() || buf.len() < 5 {
            return Err(Error::InvalidLength);
        }
        let crc_off = buf.len() - 4;
        let ty = GtsType::from_wire(buf[0] & 0xf)?;
        if buf[0] >> 4 != 0 {
            return Err(Error::Unsupported);
        }
        let mut pos = 1;

        macro_rules! u32r {
            () => {{
                if pos + 4 > crc_off { return Err(Error::InvalidLength); }
                let v = u32::from_be_bytes(buf[pos..pos + 4].try_into().unwrap());
                pos += 4;
                v
            }};
        }
        macro_rules! u16r {
            () => {{
                if pos + 2 > crc_off { return Err(Error::InvalidLength); }
                let v = u16::from_be_bytes(buf[pos..pos + 2].try_into().unwrap());
                pos += 2;
                v
            }};
        }
        macro_rules! u8r {
            () => {{
                if pos >= crc_off { return Err(Error::InvalidLength); }
                let v = buf[pos];
                pos += 1;
                v
            }};
        }

        let mut meta_end = crc_off;
        let packet = match ty {
            GtsType::Connect => {
                let it = u32r!();
                let rid = u32r!();
                let sp = StreamProfile::from_wire(u16r!())?;
                let cr = u8r!();
                if sp.unreliable && cr != 0 {
                    return Err(Error::ProfileViolation);
                }
                let (css, n) = ServiceSelector::decode(&buf[pos..crc_off])?;
                pos += n;
                if buf[pos..crc_off].iter().any(|&b| b != 0) {
                    return Err(Error::InvalidField);
                }
                Self::Connect {
                    initiator_receive_tunnel: it,
                    initiator_reset_id: rid,
                    profile: sp,
                    initial_receive_credit: cr,
                    css,
                }
            }
            GtsType::ConnectAck => {
                let a = u32r!();
                let b = u32r!();
                let r = u32r!();
                let s = u8r!();
                let c = u8r!();
                let z = u8r!();
                if z != 0 {
                    return Err(Error::InvalidField);
                }
                Self::ConnectAck {
                    initiator_receive_tunnel: a,
                    responder_receive_tunnel: b,
                    responder_reset_id: r,
                    status: s,
                    initial_receive_credit: c,
                }
            }
            GtsType::StreamOpen => {
                let t = u32r!();
                let s = u8r!();
                let sp = StreamProfile::from_wire(u16r!())?;
                let c = u8r!();
                let z = u8r!();
                if z != 0 || (sp.unreliable && c != 0) {
                    return Err(Error::ProfileViolation);
                }
                Self::StreamOpen {
                    tunnel_id: t,
                    stream_id: s,
                    profile: sp,
                    initial_receive_credit: c,
                }
            }
            GtsType::StreamAck => {
                let t = u32r!();
                let s = u8r!();
                let st = u8r!();
                let c = u8r!();
                let z = u8r!();
                if z != 0 {
                    return Err(Error::InvalidField);
                }
                Self::StreamAck {
                    tunnel_id: t,
                    stream_id: s,
                    status: st,
                    initial_receive_credit: c,
                }
            }
            GtsType::Data | GtsType::DataEnd => {
                let sp = profile.ok_or(Error::ProfileViolation)?.validate()?;
                if sp.unreliable
                    || (!sp.variable && ctx.size_class != sp.size_class)
                    || (sp.variable && (ctx.size_class as u8) > (sp.size_class as u8))
                {
                    return Err(Error::ProfileViolation);
                }
                let t = u32r!();
                let s = u8r!();
                let seq = u32r!();
                let valid = if sp.variable || ty == GtsType::DataEnd {
                    Some(u16r!() as usize)
                } else {
                    None
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
                Self::Data {
                    tunnel_id: t,
                    stream_id: s,
                    sequence: seq,
                    data,
                    end: ty == GtsType::DataEnd,
                }
            }
            GtsType::Ack => {
                let t = u32r!();
                let s = u8r!();
                let b = u32r!();
                let bm = u32r!();
                let c = u8r!();
                let z = u8r!();
                if z != 0 {
                    return Err(Error::InvalidField);
                }
                Self::Ack {
                    tunnel_id: t,
                    stream_id: s,
                    ack_base: b,
                    receive_bitmap: bm,
                    receive_credit: c,
                }
            }
            GtsType::Datagram => {
                let sp = profile.ok_or(Error::ProfileViolation)?.validate()?;
                if !sp.unreliable
                    || (!sp.variable && ctx.size_class != sp.size_class)
                    || (sp.variable && (ctx.size_class as u8) > (sp.size_class as u8))
                {
                    return Err(Error::ProfileViolation);
                }
                let t = u32r!();
                let s = u8r!();
                let seq = if sp.sequenced { Some(u32r!()) } else { None };
                let valid = if sp.variable {
                    Some(u16r!() as usize)
                } else {
                    None
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
                Self::Datagram {
                    tunnel_id: t,
                    stream_id: s,
                    sequence: seq,
                    data,
                }
            }
            GtsType::StreamClose | GtsType::StreamCloseAck => {
                let t = u32r!();
                let s = u8r!();
                let f = u32r!();
                Self::StreamClose {
                    tunnel_id: t,
                    stream_id: s,
                    final_sequence: f,
                    ack: ty == GtsType::StreamCloseAck,
                }
            }
            GtsType::TunnelClose | GtsType::TunnelCloseAck => {
                let t = u32r!();
                Self::TunnelClose {
                    tunnel_id: t,
                    ack: ty == GtsType::TunnelCloseAck,
                }
            }
            GtsType::Reset => {
                let t = u32r!();
                let r = u32r!();
                let rs = u8r!();
                Self::Reset {
                    tunnel_id: t,
                    reset_id: r,
                    reason: rs,
                }
            }
            GtsType::StreamReset | GtsType::StreamResetAck => {
                let t = u32r!();
                let s = u8r!();
                let r = u8r!();
                let z = u8r!();
                if z != 0 {
                    return Err(Error::InvalidField);
                }
                Self::StreamReset {
                    tunnel_id: t,
                    stream_id: s,
                    reason: r,
                    ack: ty == GtsType::StreamResetAck,
                }
            }
            GtsType::Reserved => return Err(Error::Unsupported),
        };

        let unchecked = profile
            .map(|sp| sp.unreliable && sp.unchecked_payload)
            .unwrap_or(false)
            && ty == GtsType::Datagram;
        let got = u32::from_be_bytes(buf[crc_off..].try_into().unwrap());
        let want = compute_crc(ctx, &buf[..if unchecked { meta_end } else { crc_off }]);
        if got != want {
            return Err(Error::InvalidCrc);
        }
        Ok(packet)
    }
}

fn compute_crc(ctx: GtsContext, gts: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update_bits((ctx.gdp_version & 3) as u64, 2);
    c.update_bits(0x2, 4);
    c.update_bits(ctx.size_class as u8 as u64, 4);
    c.update_bits(ctx.source.0, 64);
    c.update_bits(ctx.destination.0, 64);
    c.update_bytes(gts);
    c.finalize()
}
