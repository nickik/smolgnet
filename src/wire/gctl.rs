use alloc::vec;
use alloc::vec::Vec;

use crate::error::{Error, Result};
use crate::wire::gdp::{GdpAddress, SizeClass};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GctlType {
    Solicit = 0x01,
    Advertise = 0x02,
    CreditRequest = 0x03,
    Credit = 0x04,
    NodeAnnounce = 0x05,
    ReannounceSolicit = 0x06,
    AddressOffer = 0x10,
    AddressClaim = 0x11,
    AddressAck = 0x12,
    AddressNak = 0x13,
    EchoRequest = 0x20,
    EchoReply = 0x21,
    DestinationUnreachable = 0x22,
    HopLimitExceeded = 0x23,
    ParameterProblem = 0x24,
    ClassUnsupported = 0x25,
    TransitAborted = 0x26,
    PathProbe = 0x28,
    PathReply = 0x29,
    StatusRequest = 0x30,
    StatusReply = 0x31,
    GetRequest = 0x32,
    GetReply = 0x33,
    GetNextRequest = 0x34,
    GetNextReply = 0x35,
    EventReport = 0x36,
    Experimental = 0xff,
}

impl GctlType {
    pub fn from_wire(v: u8) -> Result<Self> {
        use GctlType::*;
        Ok(match v {
            0x01 => Solicit,
            0x02 => Advertise,
            0x03 => CreditRequest,
            0x04 => Credit,
            0x05 => NodeAnnounce,
            0x06 => ReannounceSolicit,
            0x10 => AddressOffer,
            0x11 => AddressClaim,
            0x12 => AddressAck,
            0x13 => AddressNak,
            0x20 => EchoRequest,
            0x21 => EchoReply,
            0x22 => DestinationUnreachable,
            0x23 => HopLimitExceeded,
            0x24 => ParameterProblem,
            0x25 => ClassUnsupported,
            0x26 => TransitAborted,
            0x28 => PathProbe,
            0x29 => PathReply,
            0x30 => StatusRequest,
            0x31 => StatusReply,
            0x32 => GetRequest,
            0x33 => GetReply,
            0x34 => GetNextRequest,
            0x35 => GetNextReply,
            0x36 => EventReport,
            0xff => Experimental,
            _ => return Err(Error::Unsupported),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceType(pub u16);
impl ServiceType {
    pub const ROUTER: Self = Self(0x0001);
    pub const DIRECTORY: Self = Self(0x0002);
    pub const TERMINAL_SERVER: Self = Self(0x0003);
    pub const BOOT_SERVER: Self = Self(0x0004);
    pub const TIME: Self = Self(0x0005);
    pub const IDENTITY: Self = Self(0x0006);
    pub const NETWORK_MANAGEMENT: Self = Self(0x0007);
    pub const SESSION_RESERVATION: Self = Self(0x0008);
    pub const EXPERIMENTAL: Self = Self(0xffff);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DiscoveryScope { Link = 0 }
impl DiscoveryScope {
    fn from_wire(v: u8) -> Result<Self> { match v { 0 => Ok(Self::Link), _ => Err(Error::Unsupported) } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditRequest { pub requested_flits: u32 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditGrant { pub granted_flits: u32 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Solicit { pub service_type: ServiceType, pub scope: DiscoveryScope }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advertise { pub service_type: ServiceType, pub preference: u8, pub provider: GdpAddress, pub lifetime: u32, pub capabilities: u32 }

/// A node announces its current GDP identity on attachment and whenever asked
/// to reannounce. `nonce` identifies the attachment instance and lets a
/// receiver distinguish a repeated announcement from an address collision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeAnnounce {
    pub node: GdpAddress,
    pub nonce: u64,
    pub capabilities: u32,
}
impl NodeAnnounce {
    pub fn conflicts_with(self, other: Self) -> bool {
        self.node == other.node && self.nonce != other.nonce
    }
}

/// Link-scoped request emitted when a newly present router needs the attached
/// nodes to replay NODE_ANNOUNCE. The switch itself needs no GDP address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReannounceSolicit { pub scope: DiscoveryScope }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressOffer { pub router: GdpAddress, pub prefix: u64, pub prefix_len: u8, pub candidate: GdpAddress, pub lifetime: u32 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressClaim { pub candidate: GdpAddress, pub nonce: u64 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressAck { pub candidate: GdpAddress, pub lifetime: u32 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressNak { pub candidate: GdpAddress, pub reason: u8, pub retry_delay: u32 }

fn prefix_mask(prefix_len: u8) -> Result<u64> {
    match prefix_len { 16 | 32 | 48 | 56 => Ok(u64::MAX << (64 - prefix_len)), _ => Err(Error::InvalidField) }
}
pub fn normalize_prefix(prefix: u64, prefix_len: u8) -> Result<u64> { Ok(prefix & prefix_mask(prefix_len)?) }
pub fn address_matches_prefix(address: GdpAddress, prefix: u64, prefix_len: u8) -> Result<bool> {
    let mask = prefix_mask(prefix_len)?;
    Ok((address.0 & mask) == (prefix & mask))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GctlMessage {
    pub version: u8,
    pub message_type: GctlType,
    pub code: u8,
    pub flags: u8,
    pub transaction_id: u32,
    pub body: Vec<u8>,
}

impl GctlMessage {
    pub fn new(message_type: GctlType, transaction_id: u32, body: Vec<u8>) -> Self {
        Self { version: 1, message_type, code: 0, flags: 0, transaction_id, body }
    }
    pub fn encoded_len(&self) -> usize { 8 + self.body.len() }
    pub fn recommended_size_class(&self) -> Result<SizeClass> { SizeClass::smallest_for(self.encoded_len()).ok_or(Error::MessageTooLarge) }
    pub fn encode_exact(&self, total: usize) -> Result<Vec<u8>> {
        if self.version != 1 || total < self.encoded_len() { return Err(Error::InvalidLength); }
        let mut o = vec![0u8; total];
        o[0] = self.version; o[1] = self.message_type as u8; o[2] = self.code; o[3] = self.flags;
        o[4..8].copy_from_slice(&self.transaction_id.to_be_bytes());
        o[8..8 + self.body.len()].copy_from_slice(&self.body);
        Ok(o)
    }
    pub fn decode(buf: &[u8]) -> Result<Self> {
        if buf.len() < 8 { return Err(Error::InvalidLength); }
        if buf[0] != 1 { return Err(Error::Unsupported); }
        Ok(Self { version: buf[0], message_type: GctlType::from_wire(buf[1])?, code: buf[2], flags: buf[3], transaction_id: u32::from_be_bytes(buf[4..8].try_into().unwrap()), body: buf[8..].to_vec() })
    }
    fn typed_body(&self, expected: GctlType, len: usize) -> Result<&[u8]> {
        if self.message_type != expected || self.body.len() < len { return Err(Error::InvalidLength); }
        if self.body[len..].iter().any(|&b| b != 0) { return Err(Error::InvalidField); }
        Ok(&self.body[..len])
    }
    pub fn echo_reply(&self) -> Result<Self> {
        if self.message_type != GctlType::EchoRequest { return Err(Error::InvalidState); }
        Ok(Self { version: self.version, message_type: GctlType::EchoReply, code: self.code, flags: self.flags, transaction_id: self.transaction_id, body: self.body.clone() })
    }

    pub fn credit_request(transaction_id: u32, requested_flits: u32) -> Self { Self::new(GctlType::CreditRequest, transaction_id, requested_flits.to_be_bytes().to_vec()) }
    pub fn parse_credit_request(&self) -> Result<CreditRequest> { let b = self.typed_body(GctlType::CreditRequest, 4)?; Ok(CreditRequest { requested_flits: u32::from_be_bytes(b.try_into().unwrap()) }) }
    pub fn credit(transaction_id: u32, granted_flits: u32) -> Self { Self::new(GctlType::Credit, transaction_id, granted_flits.to_be_bytes().to_vec()) }
    pub fn parse_credit(&self) -> Result<CreditGrant> { let b = self.typed_body(GctlType::Credit, 4)?; Ok(CreditGrant { granted_flits: u32::from_be_bytes(b.try_into().unwrap()) }) }

    pub fn solicit(transaction_id: u32, service_type: ServiceType, scope: DiscoveryScope) -> Self {
        let mut body = Vec::with_capacity(4); body.extend_from_slice(&service_type.0.to_be_bytes()); body.push(scope as u8); body.push(0); Self::new(GctlType::Solicit, transaction_id, body)
    }
    pub fn parse_solicit(&self) -> Result<Solicit> {
        let b = self.typed_body(GctlType::Solicit, 4)?; if b[3] != 0 { return Err(Error::InvalidField); }
        Ok(Solicit { service_type: ServiceType(u16::from_be_bytes([b[0], b[1]])), scope: DiscoveryScope::from_wire(b[2])? })
    }
    pub fn advertise(transaction_id: u32, advertise: Advertise) -> Self {
        let mut body = Vec::with_capacity(20); body.extend_from_slice(&advertise.service_type.0.to_be_bytes()); body.push(advertise.preference); body.push(0); body.extend_from_slice(&advertise.provider.0.to_be_bytes()); body.extend_from_slice(&advertise.lifetime.to_be_bytes()); body.extend_from_slice(&advertise.capabilities.to_be_bytes()); Self::new(GctlType::Advertise, transaction_id, body)
    }
    pub fn parse_advertise(&self) -> Result<Advertise> {
        let b = self.typed_body(GctlType::Advertise, 20)?; if b[3] != 0 { return Err(Error::InvalidField); }
        Ok(Advertise { service_type: ServiceType(u16::from_be_bytes([b[0], b[1]])), preference: b[2], provider: GdpAddress(u64::from_be_bytes(b[4..12].try_into().unwrap())), lifetime: u32::from_be_bytes(b[12..16].try_into().unwrap()), capabilities: u32::from_be_bytes(b[16..20].try_into().unwrap()) })
    }

    pub fn node_announce(transaction_id: u32, announce: NodeAnnounce) -> Self {
        let mut body = Vec::with_capacity(20);
        body.extend_from_slice(&announce.node.0.to_be_bytes());
        body.extend_from_slice(&announce.nonce.to_be_bytes());
        body.extend_from_slice(&announce.capabilities.to_be_bytes());
        Self::new(GctlType::NodeAnnounce, transaction_id, body)
    }
    pub fn parse_node_announce(&self) -> Result<NodeAnnounce> {
        let b = self.typed_body(GctlType::NodeAnnounce, 20)?;
        Ok(NodeAnnounce { node: GdpAddress(u64::from_be_bytes(b[0..8].try_into().unwrap())), nonce: u64::from_be_bytes(b[8..16].try_into().unwrap()), capabilities: u32::from_be_bytes(b[16..20].try_into().unwrap()) })
    }
    pub fn reannounce_solicit(transaction_id: u32, scope: DiscoveryScope) -> Self {
        Self::new(GctlType::ReannounceSolicit, transaction_id, vec![scope as u8, 0, 0, 0])
    }
    pub fn parse_reannounce_solicit(&self) -> Result<ReannounceSolicit> {
        let b = self.typed_body(GctlType::ReannounceSolicit, 4)?;
        if b[1..4].iter().any(|&v| v != 0) { return Err(Error::InvalidField); }
        Ok(ReannounceSolicit { scope: DiscoveryScope::from_wire(b[0])? })
    }

    pub fn address_offer(transaction_id: u32, offer: AddressOffer) -> Result<Self> {
        let normalized = normalize_prefix(offer.prefix, offer.prefix_len)?;
        if normalized != offer.prefix || !address_matches_prefix(offer.candidate, offer.prefix, offer.prefix_len)? { return Err(Error::InvalidField); }
        let mut body = Vec::with_capacity(32); body.extend_from_slice(&offer.router.0.to_be_bytes()); body.extend_from_slice(&offer.prefix.to_be_bytes()); body.push(offer.prefix_len); body.extend_from_slice(&[0, 0, 0]); body.extend_from_slice(&offer.candidate.0.to_be_bytes()); body.extend_from_slice(&offer.lifetime.to_be_bytes()); Ok(Self::new(GctlType::AddressOffer, transaction_id, body))
    }
    pub fn parse_address_offer(&self) -> Result<AddressOffer> {
        let b = self.typed_body(GctlType::AddressOffer, 32)?; if b[17..20].iter().any(|&v| v != 0) { return Err(Error::InvalidField); }
        let offer = AddressOffer { router: GdpAddress(u64::from_be_bytes(b[0..8].try_into().unwrap())), prefix: u64::from_be_bytes(b[8..16].try_into().unwrap()), prefix_len: b[16], candidate: GdpAddress(u64::from_be_bytes(b[20..28].try_into().unwrap())), lifetime: u32::from_be_bytes(b[28..32].try_into().unwrap()) };
        if normalize_prefix(offer.prefix, offer.prefix_len)? != offer.prefix || !address_matches_prefix(offer.candidate, offer.prefix, offer.prefix_len)? { return Err(Error::InvalidField); }
        Ok(offer)
    }
    pub fn address_claim(transaction_id: u32, claim: AddressClaim) -> Self { let mut body = Vec::with_capacity(16); body.extend_from_slice(&claim.candidate.0.to_be_bytes()); body.extend_from_slice(&claim.nonce.to_be_bytes()); Self::new(GctlType::AddressClaim, transaction_id, body) }
    pub fn parse_address_claim(&self) -> Result<AddressClaim> { let b = self.typed_body(GctlType::AddressClaim, 16)?; Ok(AddressClaim { candidate: GdpAddress(u64::from_be_bytes(b[0..8].try_into().unwrap())), nonce: u64::from_be_bytes(b[8..16].try_into().unwrap()) }) }
    pub fn address_ack(transaction_id: u32, ack: AddressAck) -> Self { let mut body = Vec::with_capacity(12); body.extend_from_slice(&ack.candidate.0.to_be_bytes()); body.extend_from_slice(&ack.lifetime.to_be_bytes()); Self::new(GctlType::AddressAck, transaction_id, body) }
    pub fn parse_address_ack(&self) -> Result<AddressAck> { let b = self.typed_body(GctlType::AddressAck, 12)?; Ok(AddressAck { candidate: GdpAddress(u64::from_be_bytes(b[0..8].try_into().unwrap())), lifetime: u32::from_be_bytes(b[8..12].try_into().unwrap()) }) }
    pub fn address_nak(transaction_id: u32, nak: AddressNak) -> Self { let mut body = Vec::with_capacity(12); body.extend_from_slice(&nak.candidate.0.to_be_bytes()); body.extend_from_slice(&nak.retry_delay.to_be_bytes()); let mut msg = Self::new(GctlType::AddressNak, transaction_id, body); msg.code = nak.reason; msg }
    pub fn parse_address_nak(&self) -> Result<AddressNak> { let b = self.typed_body(GctlType::AddressNak, 12)?; Ok(AddressNak { candidate: GdpAddress(u64::from_be_bytes(b[0..8].try_into().unwrap())), reason: self.code, retry_delay: u32::from_be_bytes(b[8..12].try_into().unwrap()) }) }
}
