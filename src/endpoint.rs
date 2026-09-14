use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::error::{Error, Result};
use crate::gts::{GtsTunnel, StreamState, TunnelRole, TunnelState};
use crate::link::{DlpConfig, DlpEndpoint, Flit, VcMode};
use crate::wire::css::ServiceSelector;
use crate::wire::gctl::{
    address_matches_prefix, normalize_prefix, AddressAck, AddressClaim, AddressNak, AddressOffer,
    Advertise, DiscoveryScope, GctlMessage, GctlType, ServiceType,
};
use crate::wire::gdp::{
    AddressForm, GdpAddress, GdpHeader, GdpPacket, GdpType, GdpWireConfig, SizeClass,
};
use crate::wire::gts::{GtsContext, GtsPacket, GtsType, StreamProfile};

pub const LINK_LOCAL_PREFIX: u64 = 0xfe80_0000_0000_0000;
pub const LINK_LOCAL_SUFFIX_MASK: u64 = 0x0000_ffff_ffff_ffff;

/// Provisional point-to-point discovery destination. A zero suffix is not a
/// normal client link-local address; smolgnet reserves it only until the GNet
/// bootstrap destination encoding is frozen.
pub const BOOTSTRAP_ADDRESS: GdpAddress = GdpAddress(LINK_LOCAL_PREFIX);

pub fn make_link_local(suffix: u64) -> Result<GdpAddress> {
    let suffix = suffix & LINK_LOCAL_SUFFIX_MASK;
    if suffix == 0 {
        return Err(Error::InvalidField);
    }
    Ok(GdpAddress(LINK_LOCAL_PREFIX | suffix))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TunnelHandle(pub u32);

#[derive(Debug, Clone, Copy)]
pub struct ListenerConfig {
    pub receive_slots: u8,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self { receive_slots: 32 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EndpointConfig {
    /// Ordinary link receive storage in physical flits. This is the capacity
    /// advertised to the adjacent endpoint with GCTL CREDIT.
    pub rx_buffer_flits: u32,
    pub vc_mode: VcMode,
    pub gts_receive_slots: u8,
    /// Optional /48 local GDP context used by the compact 16-bit address form.
    pub local_context_prefix: Option<u64>,
    pub prefer_local_gdp: bool,
    /// Re-advertise link receive capacity once at least this many flits can be
    /// safely granted. A value of 1 gives continuous packet-by-packet updates.
    pub credit_update_threshold: u32,
    /// Reserved control-lane window, separate from ordinary advertised buffer.
    pub control_window_flits: u32,
    pub gdp_wire: GdpWireConfig,
}

impl EndpointConfig {
    pub fn new(rx_buffer_flits: u32) -> Self {
        Self {
            rx_buffer_flits,
            vc_mode: VcMode::Two,
            gts_receive_slots: 32,
            local_context_prefix: None,
            prefer_local_gdp: false,
            credit_update_threshold: 1,
            control_window_flits: 32,
            gdp_wire: GdpWireConfig::default(),
        }
    }

    fn validate(self) -> Result<Self> {
        if self.rx_buffer_flits == 0
            || self.gts_receive_slots == 0
            || self.control_window_flits == 0
            || self.credit_update_threshold == 0
        {
            return Err(Error::InvalidField);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressState {
    Static,
    LinkLocalOnly,
    Soliciting {
        transaction_id: u32,
    },
    Offered {
        transaction_id: u32,
        router: GdpAddress,
        candidate: GdpAddress,
    },
    Assigned {
        address: GdpAddress,
        router: GdpAddress,
        lifetime: u32,
    },
    Rejected {
        reason: u8,
        retry_delay: u32,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct AddressAuthorityConfig {
    pub prefix: u64,
    pub prefix_len: u8,
    pub lifetime: u32,
    pub preference: u8,
    pub capabilities: u32,
}

impl AddressAuthorityConfig {
    pub fn new(prefix: u64, prefix_len: u8, lifetime: u32) -> Result<Self> {
        Ok(Self {
            prefix: normalize_prefix(prefix, prefix_len)?,
            prefix_len,
            lifetime,
            preference: 0,
            capabilities: 0,
        })
    }
}

#[derive(Debug, Clone)]
struct PendingAuthorityOffer {
    client: GdpAddress,
    candidate: GdpAddress,
}

#[derive(Debug, Clone)]
struct AddressAuthority {
    config: AddressAuthorityConfig,
    next_host: u64,
    pending: BTreeMap<u32, PendingAuthorityOffer>,
    leases: BTreeMap<GdpAddress, GdpAddress>,
}

impl AddressAuthority {
    fn new(config: AddressAuthorityConfig) -> Self {
        Self {
            config,
            next_host: 1,
            pending: BTreeMap::new(),
            leases: BTreeMap::new(),
        }
    }

    fn candidate_in_use(&self, candidate: GdpAddress) -> bool {
        self.leases.contains_key(&candidate)
            || self.pending.values().any(|p| p.candidate == candidate)
    }

    fn allocate_candidate(&mut self, router: GdpAddress) -> Result<GdpAddress> {
        let host_bits = 64 - self.config.prefix_len as u32;
        let host_mask = (1u64 << host_bits) - 1;
        // Point-to-point/bootstrap use normally finds the first candidate. The
        // bounded search also makes tiny /56 pools fail deterministically.
        let attempts = host_mask.min(65_535).max(1);
        for _ in 0..attempts {
            let host = self.next_host & host_mask;
            self.next_host = (self.next_host + 1) & host_mask;
            if self.next_host == 0 {
                self.next_host = 1;
            }
            if host == 0 {
                continue;
            }
            let candidate = GdpAddress(self.config.prefix | host);
            if candidate != router && !self.candidate_in_use(candidate) {
                return Ok(candidate);
            }
        }
        Err(Error::BufferFull)
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingClientOffer {
    transaction_id: u32,
    source: GdpAddress,
    offer: AddressOffer,
}

#[derive(Debug, Clone)]
struct TunnelRecord {
    peer: GdpAddress,
    tunnel: GtsTunnel,
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    config: EndpointConfig,
    primary_address: GdpAddress,
    link_local_address: GdpAddress,
    address_state: AddressState,
    router_address: Option<GdpAddress>,
    pending_client_offer: Option<PendingClientOffer>,
    authority: Option<AddressAuthority>,
    next_claim_nonce: u64,

    dlp: DlpEndpoint,
    peer_link_local: Option<GdpAddress>,
    credit_request_pending: bool,
    next_control_transaction: u32,

    listeners: BTreeMap<ServiceSelector, ListenerConfig>,
    tunnels: BTreeMap<u32, TunnelRecord>,
    accepted: VecDeque<TunnelHandle>,
    next_tunnel: u32,
    next_reset: u32,
    echo_replies: BTreeMap<u32, Vec<u8>>,
    gctl_inbox: VecDeque<(GdpAddress, GctlMessage)>,
    last_rx_address_form: Option<AddressForm>,
}

impl Endpoint {
    pub fn new(address: GdpAddress, config: EndpointConfig) -> Result<Self> {
        let config = config.validate()?;
        let suffix = address.0 & LINK_LOCAL_SUFFIX_MASK;
        let link_local_address = make_link_local(if suffix == 0 { 1 } else { suffix })?;
        Self::build(address, link_local_address, AddressState::Static, config)
    }

    pub fn unconfigured(link_local_suffix: u64, config: EndpointConfig) -> Result<Self> {
        let config = config.validate()?;
        let link_local_address = make_link_local(link_local_suffix)?;
        Self::build(
            link_local_address,
            link_local_address,
            AddressState::LinkLocalOnly,
            config,
        )
    }

    fn build(
        primary_address: GdpAddress,
        link_local_address: GdpAddress,
        address_state: AddressState,
        config: EndpointConfig,
    ) -> Result<Self> {
        let local_prefix = config.local_context_prefix.unwrap_or(0) & !0xffff;
        let mut dlp_cfg = DlpConfig::new(config.rx_buffer_flits, config.vc_mode)?;
        dlp_cfg.gdp = config.gdp_wire;
        dlp_cfg.local_prefix = local_prefix;
        dlp_cfg.control_window_flits = config.control_window_flits;
        Ok(Self {
            config,
            primary_address,
            link_local_address,
            address_state,
            router_address: None,
            pending_client_offer: None,
            authority: None,
            next_claim_nonce: 1,
            dlp: DlpEndpoint::new(dlp_cfg)?,
            peer_link_local: None,
            credit_request_pending: false,
            next_control_transaction: 0x8000_0000,
            listeners: BTreeMap::new(),
            tunnels: BTreeMap::new(),
            accepted: VecDeque::new(),
            next_tunnel: 1,
            next_reset: 0x1000_0001,
            echo_replies: BTreeMap::new(),
            gctl_inbox: VecDeque::new(),
            last_rx_address_form: None,
        })
    }

    pub const fn address(&self) -> GdpAddress {
        self.primary_address
    }

    pub const fn link_local_address(&self) -> GdpAddress {
        self.link_local_address
    }

    pub const fn address_state(&self) -> AddressState {
        self.address_state
    }

    pub const fn router_address(&self) -> Option<GdpAddress> {
        self.router_address
    }

    pub const fn last_rx_address_form(&self) -> Option<AddressForm> {
        self.last_rx_address_form
    }

    pub fn dlp(&self) -> &DlpEndpoint {
        &self.dlp
    }

    pub fn dlp_mut(&mut self) -> &mut DlpEndpoint {
        &mut self.dlp
    }

    pub fn enable_address_authority(&mut self, config: AddressAuthorityConfig) {
        self.authority = Some(AddressAuthority::new(config));
    }

    fn next_control_transaction(&mut self) -> u32 {
        let id = self.next_control_transaction;
        self.next_control_transaction = self.next_control_transaction.wrapping_add(1).max(1);
        id
    }

    fn alloc_tunnel(&mut self) -> (u32, u32) {
        let t = self.next_tunnel;
        self.next_tunnel = self.next_tunnel.wrapping_add(1).max(1);
        let r = self.next_reset;
        self.next_reset = self.next_reset.wrapping_add(1).max(1);
        (t, r)
    }

    pub fn listen(&mut self, css: ServiceSelector, config: ListenerConfig) {
        self.listeners.insert(css, config);
    }

    pub fn accept(&mut self) -> Option<TunnelHandle> {
        self.accepted.pop_front()
    }

    pub fn tunnel_state(&self, h: TunnelHandle) -> Result<TunnelState> {
        Ok(self
            .tunnels
            .get(&h.0)
            .ok_or(Error::UnknownTunnel)?
            .tunnel
            .state)
    }

    pub fn stream_state(&self, h: TunnelHandle, id: u8) -> Result<StreamState> {
        Ok(self
            .tunnels
            .get(&h.0)
            .ok_or(Error::UnknownTunnel)?
            .tunnel
            .streams
            .get(&id)
            .ok_or(Error::UnknownStream)?
            .state)
    }

    pub fn stream_peer_credit(&self, h: TunnelHandle, id: u8) -> Result<Option<u8>> {
        Ok(self
            .tunnels
            .get(&h.0)
            .ok_or(Error::UnknownTunnel)?
            .tunnel
            .streams
            .get(&id)
            .ok_or(Error::UnknownStream)?
            .peer_credit())
    }

    fn local_gts_credit(&self, profile: StreamProfile, local_is_opener: bool) -> u8 {
        if profile.unreliable {
            return 0;
        }
        let receives = if local_is_opener {
            profile.direction.peer_may_send()
        } else {
            profile.direction.opener_may_send()
        };
        if receives {
            self.config.gts_receive_slots
        } else {
            0
        }
    }

    fn make_gdp_header(
        &self,
        packet_type: GdpType,
        class: SizeClass,
        source: GdpAddress,
        destination: GdpAddress,
    ) -> Result<GdpHeader> {
        if self.config.prefer_local_gdp && destination != BOOTSTRAP_ADDRESS {
            if let Some(prefix) = self.config.local_context_prefix {
                let prefix = prefix & !0xffff;
                if (source.0 & !0xffff) == prefix && (destination.0 & !0xffff) == prefix {
                    return GdpHeader::local(
                        packet_type,
                        class,
                        15,
                        prefix,
                        source.0 as u16,
                        destination.0 as u16,
                    );
                }
            }
        }
        Ok(GdpHeader::global(
            packet_type,
            class,
            64,
            source,
            destination,
        ))
    }

    fn send_gctl_from(
        &mut self,
        source: GdpAddress,
        remote: GdpAddress,
        msg: GctlMessage,
        class: Option<SizeClass>,
    ) -> Result<()> {
        let class = match class {
            Some(c) => c,
            None => msg.recommended_size_class()?,
        };
        let payload = msg.encode_exact(class.bytes())?;
        let h = self.make_gdp_header(GdpType::Gctl, class, source, remote)?;
        self.dlp
            .queue_control_packet(&GdpPacket::new(h, payload)?)?;
        Ok(())
    }

    pub fn send_gctl(
        &mut self,
        remote: GdpAddress,
        msg: GctlMessage,
        class: SizeClass,
    ) -> Result<()> {
        self.send_gctl_from(self.primary_address, remote, msg, Some(class))
    }

    fn send_gts(
        &mut self,
        remote: GdpAddress,
        packet: GtsPacket,
        profile: Option<StreamProfile>,
    ) -> Result<()> {
        let class = packet.choose_size_class(profile)?;
        let source = self.primary_address;
        let ctx = GtsContext {
            gdp_version: 0,
            size_class: class,
            source,
            destination: remote,
        };
        let payload = packet.encode(ctx, profile)?;
        let h = self.make_gdp_header(GdpType::Gts, class, source, remote)?;
        self.dlp.queue_data_packet(&GdpPacket::new(h, payload)?)?;
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Link-local receive-credit management.
    // ---------------------------------------------------------------------

    fn on_link_attached(&mut self, peer_link_local: GdpAddress) -> Result<()> {
        self.peer_link_local = Some(peer_link_local);
        self.advertise_link_credit(true)
    }

    fn advertise_link_credit(&mut self, force: bool) -> Result<()> {
        let Some(peer) = self.peer_link_local else {
            return Ok(());
        };
        let grant = self.dlp.grantable_data_credit();
        if grant == 0 {
            return Ok(());
        }
        if !force && grant < self.config.credit_update_threshold {
            return Ok(());
        }
        let tx = self.next_control_transaction();
        let msg = GctlMessage::credit(tx, grant);
        self.send_gctl_from(self.link_local_address, peer, msg, None)?;
        self.dlp.note_data_credit_granted(grant)?;
        Ok(())
    }

    fn maybe_request_link_credit(&mut self) -> Result<()> {
        if self.dlp.queued_data_flits() == 0
            || self.dlp.data_tx_credit() != 0
            || self.credit_request_pending
        {
            return Ok(());
        }
        let Some(peer) = self.peer_link_local else {
            return Ok(());
        };
        let requested = self.dlp.queued_data_flits().min(u32::MAX as usize) as u32;
        let tx = self.next_control_transaction();
        let msg = GctlMessage::credit_request(tx, requested.max(1));
        self.send_gctl_from(self.link_local_address, peer, msg, None)?;
        self.credit_request_pending = true;
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Discovery/address assignment.
    // ---------------------------------------------------------------------

    pub fn solicit_router(&mut self, transaction_id: u32) -> Result<()> {
        if self.address_state == AddressState::Static {
            return Err(Error::InvalidState);
        }
        self.address_state = AddressState::Soliciting { transaction_id };
        let msg = GctlMessage::solicit(transaction_id, ServiceType::ROUTER, DiscoveryScope::Link);
        self.send_gctl_from(self.link_local_address, BOOTSTRAP_ADDRESS, msg, None)
    }

    fn handle_router_solicit(&mut self, src: GdpAddress, msg: &GctlMessage) -> Result<bool> {
        let solicit = msg.parse_solicit()?;
        if solicit.service_type != ServiceType::ROUTER {
            return Ok(false);
        }
        let provider = self.primary_address;
        let (advertise, offer) = {
            let Some(authority) = self.authority.as_mut() else {
                return Ok(false);
            };
            let candidate = authority.allocate_candidate(provider)?;
            let offer = AddressOffer {
                router: provider,
                prefix: authority.config.prefix,
                prefix_len: authority.config.prefix_len,
                candidate,
                lifetime: authority.config.lifetime,
            };
            authority.pending.insert(
                msg.transaction_id,
                PendingAuthorityOffer {
                    client: src,
                    candidate,
                },
            );
            let advertise = Advertise {
                service_type: ServiceType::ROUTER,
                preference: authority.config.preference,
                provider,
                lifetime: authority.config.lifetime,
                capabilities: authority.config.capabilities,
            };
            (advertise, offer)
        };

        self.send_gctl_from(
            self.link_local_address,
            src,
            GctlMessage::advertise(msg.transaction_id, advertise),
            None,
        )?;
        self.send_gctl_from(
            self.link_local_address,
            src,
            GctlMessage::address_offer(msg.transaction_id, offer)?,
            None,
        )?;
        Ok(true)
    }

    fn handle_address_offer(&mut self, src: GdpAddress, msg: &GctlMessage) -> Result<()> {
        let offer = msg.parse_address_offer()?;
        match self.address_state {
            AddressState::Soliciting { transaction_id } if transaction_id == msg.transaction_id => {}
            _ => return Err(Error::InvalidState),
        }
        if !address_matches_prefix(offer.candidate, offer.prefix, offer.prefix_len)? {
            return Err(Error::InvalidField);
        }
        self.router_address = Some(offer.router);
        self.pending_client_offer = Some(PendingClientOffer {
            transaction_id: msg.transaction_id,
            source: src,
            offer,
        });
        self.address_state = AddressState::Offered {
            transaction_id: msg.transaction_id,
            router: offer.router,
            candidate: offer.candidate,
        };
        let nonce = self.next_claim_nonce;
        self.next_claim_nonce = self.next_claim_nonce.wrapping_add(1).max(1);
        let claim = AddressClaim {
            candidate: offer.candidate,
            nonce,
        };
        self.send_gctl_from(
            self.link_local_address,
            src,
            GctlMessage::address_claim(msg.transaction_id, claim),
            None,
        )
    }

    fn handle_address_claim(&mut self, src: GdpAddress, msg: &GctlMessage) -> Result<bool> {
        let claim = msg.parse_address_claim()?;
        let Some(authority) = self.authority.as_mut() else {
            return Ok(false);
        };
        let valid = authority
            .pending
            .get(&msg.transaction_id)
            .map(|p| p.client == src && p.candidate == claim.candidate)
            .unwrap_or(false);

        if valid {
            let pending = authority
                .pending
                .remove(&msg.transaction_id)
                .ok_or(Error::InvalidState)?;
            authority.leases.insert(pending.candidate, src);
            let ack = AddressAck {
                candidate: pending.candidate,
                lifetime: authority.config.lifetime,
            };
            self.send_gctl_from(
                self.link_local_address,
                src,
                GctlMessage::address_ack(msg.transaction_id, ack),
                None,
            )?;
        } else {
            let nak = AddressNak {
                candidate: claim.candidate,
                reason: 1,
                retry_delay: 1,
            };
            self.send_gctl_from(
                self.link_local_address,
                src,
                GctlMessage::address_nak(msg.transaction_id, nak),
                None,
            )?;
        }
        Ok(true)
    }

    fn handle_address_ack(&mut self, msg: &GctlMessage) -> Result<()> {
        let ack = msg.parse_address_ack()?;
        let pending = self.pending_client_offer.ok_or(Error::InvalidState)?;
        if pending.transaction_id != msg.transaction_id || pending.offer.candidate != ack.candidate {
            return Err(Error::InvalidField);
        }
        self.primary_address = ack.candidate;
        self.router_address = Some(pending.offer.router);
        self.address_state = AddressState::Assigned {
            address: ack.candidate,
            router: pending.offer.router,
            lifetime: ack.lifetime,
        };
        if pending.offer.prefix_len == 48 {
            let prefix = pending.offer.prefix & !0xffff;
            self.config.local_context_prefix = Some(prefix);
            self.dlp.set_local_prefix(prefix);
        }
        self.pending_client_offer = None;
        Ok(())
    }

    fn handle_address_nak(&mut self, msg: &GctlMessage) -> Result<()> {
        let nak = msg.parse_address_nak()?;
        self.address_state = AddressState::Rejected {
            reason: nak.reason,
            retry_delay: nak.retry_delay,
        };
        self.pending_client_offer = None;
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Application-facing GTS operations.
    // ---------------------------------------------------------------------

    pub fn connect(
        &mut self,
        remote: GdpAddress,
        css: ServiceSelector,
        profile: StreamProfile,
    ) -> Result<TunnelHandle> {
        profile.validate()?;
        let (local, reset) = self.alloc_tunnel();
        let credit = self.local_gts_credit(profile, true);
        let tunnel = GtsTunnel::initiator(local, reset, css, profile, credit)?;
        let packet = GtsPacket::Connect {
            initiator_receive_tunnel: local,
            initiator_reset_id: reset,
            profile,
            initial_receive_credit: credit,
            css,
        };
        self.tunnels.insert(local, TunnelRecord { peer: remote, tunnel });
        self.send_gts(remote, packet, None)?;
        Ok(TunnelHandle(local))
    }

    pub fn open_stream(&mut self, h: TunnelHandle, profile: StreamProfile) -> Result<u8> {
        profile.validate()?;
        let configured_credit = self.config.gts_receive_slots;
        let (peer, id, credit, remote_id) = {
            let rec = self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;
            let id = rec.tunnel.alloc_stream_id()?;
            let receives = if profile.unreliable {
                false
            } else {
                profile.direction.peer_may_send()
            };
            let credit = if receives { configured_credit } else { 0 };
            rec.tunnel.add_stream(id, profile, true, credit, 0)?;
            rec.tunnel.stream_mut(id)?.mark_opening();
            (rec.peer, id, credit, rec.tunnel.remote_id()?)
        };
        self.send_gts(
            peer,
            GtsPacket::StreamOpen {
                tunnel_id: remote_id,
                stream_id: id,
                profile,
                initial_receive_credit: credit,
            },
            None,
        )?;
        Ok(id)
    }

    pub fn send(
        &mut self,
        h: TunnelHandle,
        stream_id: u8,
        data: &[u8],
        now: u64,
    ) -> Result<()> {
        self.send_inner(h, stream_id, data, false, now)
    }

    pub fn send_end(
        &mut self,
        h: TunnelHandle,
        stream_id: u8,
        data: &[u8],
        now: u64,
    ) -> Result<()> {
        self.send_inner(h, stream_id, data, true, now)
    }

    fn send_inner(
        &mut self,
        h: TunnelHandle,
        stream_id: u8,
        data: &[u8],
        end: bool,
        now: u64,
    ) -> Result<()> {
        let (peer, profile, packet) = {
            let rec = self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;
            if rec.tunnel.state != TunnelState::Established {
                return Err(Error::InvalidState);
            }
            let remote = rec.tunnel.remote_id()?;
            let s = rec.tunnel.stream_mut(stream_id)?;
            let profile = s.profile;
            let packet = s.send_packet(remote, data.to_vec(), end, now)?;
            (rec.peer, profile, packet)
        };
        self.send_gts(peer, packet, Some(profile))
    }

    pub fn recv(&mut self, h: TunnelHandle, stream_id: u8) -> Result<Option<Vec<u8>>> {
        let (peer, profile, message, ack) = {
            let rec = self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;
            let remote = rec.tunnel.remote_id()?;
            let s = rec.tunnel.stream_mut(stream_id)?;
            let profile = s.profile;
            let message = s.recv();
            let ack = if message.is_some() {
                s.receive_credit_ack(remote)
            } else {
                None
            };
            (rec.peer, profile, message, ack)
        };
        if let Some(ack) = ack {
            self.send_gts(peer, ack, Some(profile))?;
        }
        Ok(message)
    }

    pub fn reset_stream(&mut self, h: TunnelHandle, stream_id: u8, reason: u8) -> Result<()> {
        let (peer, remote) = {
            let rec = self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;
            rec.tunnel.stream_mut(stream_id)?.reset();
            (rec.peer, rec.tunnel.remote_id()?)
        };
        self.send_gts(
            peer,
            GtsPacket::StreamReset {
                tunnel_id: remote,
                stream_id,
                reason,
                ack: false,
            },
            None,
        )
    }

    pub fn close_stream(&mut self, h: TunnelHandle, stream_id: u8) -> Result<()> {
        let (peer, remote, final_sequence) = {
            let rec = self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;
            let s = rec.tunnel.stream_mut(stream_id)?;
            if !matches!(s.state, StreamState::Open | StreamState::Closing) {
                return Err(Error::InvalidState);
            }
            s.state = StreamState::Closing;
            let final_sequence = if s.profile.unreliable { 0 } else { 0 };
            (rec.peer, rec.tunnel.remote_id()?, final_sequence)
        };
        self.send_gts(
            peer,
            GtsPacket::StreamClose {
                tunnel_id: remote,
                stream_id,
                final_sequence,
                ack: false,
            },
            None,
        )
    }

    pub fn close_tunnel(&mut self, h: TunnelHandle) -> Result<()> {
        let (peer, remote) = {
            let rec = self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;
            if rec
                .tunnel
                .streams
                .values()
                .any(|s| !matches!(s.state, StreamState::Closed | StreamState::Reset))
            {
                return Err(Error::InvalidState);
            }
            rec.tunnel.state = TunnelState::Closing;
            (rec.peer, rec.tunnel.remote_id()?)
        };
        self.send_gts(
            peer,
            GtsPacket::TunnelClose {
                tunnel_id: remote,
                ack: false,
            },
            None,
        )
    }

    pub fn reset_tunnel(&mut self, h: TunnelHandle, reason: u8) -> Result<()> {
        let (peer, remote, reset) = {
            let rec = self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;
            let remote = rec.tunnel.remote_id()?;
            let reset = rec.tunnel.remote_reset_id.ok_or(Error::InvalidState)?;
            rec.tunnel.reset();
            (rec.peer, remote, reset)
        };
        self.send_gts(
            peer,
            GtsPacket::Reset {
                tunnel_id: remote,
                reset_id: reset,
                reason,
            },
            None,
        )
    }

    pub fn send_echo(
        &mut self,
        remote: GdpAddress,
        transaction_id: u32,
        body: &[u8],
        class: SizeClass,
    ) -> Result<()> {
        let m = GctlMessage::new(GctlType::EchoRequest, transaction_id, body.to_vec());
        self.send_gctl(remote, m, class)
    }

    pub fn take_echo_reply(&mut self, transaction_id: u32) -> Option<Vec<u8>> {
        self.echo_replies.remove(&transaction_id)
    }

    pub fn take_gctl(&mut self) -> Option<(GdpAddress, GctlMessage)> {
        self.gctl_inbox.pop_front()
    }

    pub fn poll_tx_flit(&mut self) -> Result<Option<Flit>> {
        self.maybe_request_link_credit()?;
        self.dlp.poll_tx()
    }

    pub fn receive_flit(&mut self, flit: Flit, now: u64) -> Result<bool> {
        if let Some(packet) = self.dlp.receive(flit)? {
            self.handle_gdp(packet, now)?;
            self.advertise_link_credit(false)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn accepts_destination(&self, destination: GdpAddress) -> bool {
        destination == self.primary_address
            || destination == self.link_local_address
            || destination == BOOTSTRAP_ADDRESS
    }

    fn handle_gdp(&mut self, packet: GdpPacket, now: u64) -> Result<()> {
        if !self.accepts_destination(packet.header.destination()) {
            return Err(Error::InvalidField);
        }
        self.last_rx_address_form = Some(packet.header.addresses.form());
        let src = packet.header.source();
        match packet.header.packet_type {
            GdpType::Gctl => self.handle_gctl(src, packet),
            GdpType::Gts => {
                if packet.header.destination() == BOOTSTRAP_ADDRESS {
                    return Err(Error::InvalidField);
                }
                self.handle_gts(src, packet, now)
            }
            GdpType::Reserved(_) => Err(Error::Unsupported),
        }
    }

    fn handle_gctl(&mut self, src: GdpAddress, packet: GdpPacket) -> Result<()> {
        let msg = GctlMessage::decode(&packet.payload)?;
        match msg.message_type {
            GctlType::CreditRequest => {
                let request = msg.parse_credit_request()?;
                let available = self.dlp.grantable_data_credit();
                let grant = if request.requested_flits == 0 {
                    available
                } else {
                    available.min(request.requested_flits)
                };
                if grant > 0 {
                    let peer = self.peer_link_local.unwrap_or(src);
                    let reply = GctlMessage::credit(msg.transaction_id, grant);
                    self.send_gctl_from(self.link_local_address, peer, reply, None)?;
                    self.dlp.note_data_credit_granted(grant)?;
                }
                Ok(())
            }
            GctlType::Credit => {
                let credit = msg.parse_credit()?;
                self.dlp.grant_data_tx_credit(credit.granted_flits);
                self.credit_request_pending = false;
                Ok(())
            }
            GctlType::EchoRequest => {
                let reply = msg.echo_reply()?;
                self.send_gctl_from(self.primary_address, src, reply, Some(packet.header.size_class))
            }
            GctlType::EchoReply => {
                self.echo_replies.insert(msg.transaction_id, msg.body);
                Ok(())
            }
            GctlType::Solicit => {
                if self.handle_router_solicit(src, &msg)? {
                    Ok(())
                } else {
                    self.gctl_inbox.push_back((src, msg));
                    Ok(())
                }
            }
            GctlType::Advertise => {
                let advertise = msg.parse_advertise()?;
                if advertise.service_type == ServiceType::ROUTER {
                    if let AddressState::Soliciting { transaction_id } = self.address_state {
                        if transaction_id == msg.transaction_id {
                            self.router_address = Some(advertise.provider);
                        }
                    }
                }
                self.gctl_inbox.push_back((src, msg));
                Ok(())
            }
            GctlType::AddressOffer => self.handle_address_offer(src, &msg),
            GctlType::AddressClaim => {
                if self.handle_address_claim(src, &msg)? {
                    Ok(())
                } else {
                    self.gctl_inbox.push_back((src, msg));
                    Ok(())
                }
            }
            GctlType::AddressAck => self.handle_address_ack(&msg),
            GctlType::AddressNak => self.handle_address_nak(&msg),
            _ => {
                self.gctl_inbox.push_back((src, msg));
                Ok(())
            }
        }
    }

    fn gts_profile_for(&self, payload: &[u8]) -> Result<Option<StreamProfile>> {
        if payload.is_empty() {
            return Err(Error::InvalidLength);
        }
        let ty = GtsType::from_wire(payload[0] & 0xf)?;
        let stream_scoped = matches!(
            ty,
            GtsType::Data
                | GtsType::DataEnd
                | GtsType::Datagram
                | GtsType::Ack
                | GtsType::StreamClose
                | GtsType::StreamCloseAck
                | GtsType::StreamReset
                | GtsType::StreamResetAck
        );
        if !stream_scoped {
            return Ok(None);
        }
        if payload.len() < 6 {
            return Err(Error::InvalidLength);
        }
        let local = u32::from_be_bytes(payload[1..5].try_into().unwrap());
        let stream = payload[5];
        let rec = self.tunnels.get(&local).ok_or(Error::UnknownTunnel)?;
        Ok(Some(
            rec.tunnel
                .streams
                .get(&stream)
                .ok_or(Error::UnknownStream)?
                .profile,
        ))
    }

    fn remote_stream_id_valid(role: TunnelRole, id: u8) -> bool {
        if id == 0 {
            return false;
        }
        match role {
            TunnelRole::Initiator => id % 2 == 1,
            TunnelRole::Responder => id % 2 == 0,
        }
    }

    fn handle_gts(&mut self, src: GdpAddress, packet: GdpPacket, _now: u64) -> Result<()> {
        let profile = self.gts_profile_for(&packet.payload)?;
        let ctx = GtsContext {
            gdp_version: packet.header.version,
            size_class: packet.header.size_class,
            source: src,
            destination: self.primary_address,
        };
        let g = GtsPacket::decode(&packet.payload, ctx, profile)?;

        match g {
            GtsPacket::Connect {
                initiator_receive_tunnel,
                initiator_reset_id,
                profile,
                initial_receive_credit,
                css,
            } => {
                let listener = *self.listeners.get(&css).ok_or(Error::UnknownService)?;
                let (local, reset) = self.alloc_tunnel();
                let local_credit = if profile.unreliable {
                    0
                } else if profile.direction.opener_may_send() {
                    listener.receive_slots
                } else {
                    0
                };
                let tunnel = GtsTunnel::responder(
                    local,
                    reset,
                    initiator_receive_tunnel,
                    initiator_reset_id,
                    css,
                    profile,
                    local_credit,
                    initial_receive_credit,
                )?;
                self.tunnels.insert(local, TunnelRecord { peer: src, tunnel });
                self.accepted.push_back(TunnelHandle(local));
                self.send_gts(
                    src,
                    GtsPacket::ConnectAck {
                        initiator_receive_tunnel,
                        responder_receive_tunnel: local,
                        responder_reset_id: reset,
                        status: 0,
                        initial_receive_credit: local_credit,
                    },
                    None,
                )
            }
            GtsPacket::ConnectAck {
                initiator_receive_tunnel,
                responder_receive_tunnel,
                responder_reset_id,
                status,
                initial_receive_credit,
            } => {
                if status != 0 {
                    return Err(Error::UnknownService);
                }
                let rec = self
                    .tunnels
                    .get_mut(&initiator_receive_tunnel)
                    .ok_or(Error::UnknownTunnel)?;
                let p = rec
                    .tunnel
                    .streams
                    .get(&0)
                    .ok_or(Error::UnknownStream)?
                    .profile;
                if p.unreliable && initial_receive_credit != 0 {
                    return Err(Error::ProfileViolation);
                }
                rec.tunnel.establish_initiator(
                    responder_receive_tunnel,
                    responder_reset_id,
                    initial_receive_credit,
                )
            }
            GtsPacket::StreamOpen {
                tunnel_id,
                stream_id,
                profile,
                initial_receive_credit,
            } => {
                let configured_credit = self.config.gts_receive_slots;
                let (peer, remote, credit) = {
                    let rec = self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;
                    if !Self::remote_stream_id_valid(rec.tunnel.role, stream_id) {
                        return Err(Error::ProfileViolation);
                    }
                    if profile.unreliable && initial_receive_credit != 0 {
                        return Err(Error::ProfileViolation);
                    }
                    let credit = if profile.unreliable {
                        0
                    } else if profile.direction.opener_may_send() {
                        configured_credit
                    } else {
                        0
                    };
                    rec.tunnel
                        .add_stream(stream_id, profile, false, credit, initial_receive_credit)?;
                    (rec.peer, rec.tunnel.remote_id()?, credit)
                };
                self.send_gts(
                    peer,
                    GtsPacket::StreamAck {
                        tunnel_id: remote,
                        stream_id,
                        status: 0,
                        initial_receive_credit: credit,
                    },
                    None,
                )
            }
            GtsPacket::StreamAck {
                tunnel_id,
                stream_id,
                status,
                initial_receive_credit,
            } => {
                if status != 0 {
                    return Err(Error::ProfileViolation);
                }
                let rec = self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;
                let s = rec.tunnel.stream_mut(stream_id)?;
                if s.profile.unreliable && initial_receive_credit != 0 {
                    return Err(Error::ProfileViolation);
                }
                s.apply_open_ack(initial_receive_credit)
            }
            data @ GtsPacket::Data {
                tunnel_id,
                stream_id,
                ..
            } => {
                let (peer, remote, ack, profile) = {
                    let rec = self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;
                    let s = rec.tunnel.stream_mut(stream_id)?;
                    s.validate_incoming(packet.header.size_class, &data)?;
                    let profile = s.profile;
                    let ack = s.receive_packet(&data)?;
                    (rec.peer, rec.tunnel.remote_id()?, ack, profile)
                };
                if let Some(GtsPacket::Ack {
                    stream_id,
                    ack_base,
                    receive_bitmap,
                    receive_credit,
                    ..
                }) = ack
                {
                    self.send_gts(
                        peer,
                        GtsPacket::Ack {
                            tunnel_id: remote,
                            stream_id,
                            ack_base,
                            receive_bitmap,
                            receive_credit,
                        },
                        Some(profile),
                    )?;
                }
                Ok(())
            }
            datagram @ GtsPacket::Datagram {
                tunnel_id,
                stream_id,
                ..
            } => {
                let rec = self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;
                let s = rec.tunnel.stream_mut(stream_id)?;
                s.validate_incoming(packet.header.size_class, &datagram)?;
                s.receive_packet(&datagram)?;
                Ok(())
            }
            ack @ GtsPacket::Ack {
                tunnel_id,
                stream_id,
                ack_base,
                receive_bitmap,
                receive_credit,
            } => {
                let rec = self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;
                let s = rec.tunnel.stream_mut(stream_id)?;
                s.validate_incoming(packet.header.size_class, &ack)?;
                s.on_ack(ack_base, receive_bitmap, receive_credit)
            }
            close @ GtsPacket::StreamClose {
                tunnel_id,
                stream_id,
                final_sequence,
                ack: false,
            } => {
                let (peer, remote) = {
                    let rec = self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;
                    let s = rec.tunnel.stream_mut(stream_id)?;
                    s.validate_incoming(packet.header.size_class, &close)?;
                    s.state = StreamState::Closed;
                    (rec.peer, rec.tunnel.remote_id()?)
                };
                self.send_gts(
                    peer,
                    GtsPacket::StreamClose {
                        tunnel_id: remote,
                        stream_id,
                        final_sequence,
                        ack: true,
                    },
                    None,
                )
            }
            close @ GtsPacket::StreamClose {
                tunnel_id,
                stream_id,
                ack: true,
                ..
            } => {
                let rec = self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;
                let s = rec.tunnel.stream_mut(stream_id)?;
                s.validate_incoming(packet.header.size_class, &close)?;
                s.state = StreamState::Closed;
                Ok(())
            }
            GtsPacket::StreamReset {
                tunnel_id,
                stream_id,
                reason,
                ack: false,
            } => {
                let (peer, remote) = {
                    let rec = self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;
                    rec.tunnel.stream_mut(stream_id)?.reset();
                    (rec.peer, rec.tunnel.remote_id()?)
                };
                self.send_gts(
                    peer,
                    GtsPacket::StreamReset {
                        tunnel_id: remote,
                        stream_id,
                        reason,
                        ack: true,
                    },
                    None,
                )
            }
            GtsPacket::StreamReset {
                tunnel_id,
                stream_id,
                ack: true,
                ..
            } => {
                self.tunnels
                    .get_mut(&tunnel_id)
                    .ok_or(Error::UnknownTunnel)?
                    .tunnel
                    .stream_mut(stream_id)?
                    .reset();
                Ok(())
            }
            GtsPacket::TunnelClose {
                tunnel_id,
                ack: false,
            } => {
                let (peer, remote) = {
                    let rec = self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;
                    if rec
                        .tunnel
                        .streams
                        .values()
                        .any(|s| !matches!(s.state, StreamState::Closed | StreamState::Reset))
                    {
                        return Err(Error::InvalidState);
                    }
                    rec.tunnel.state = TunnelState::Closed;
                    (rec.peer, rec.tunnel.remote_id()?)
                };
                self.send_gts(
                    peer,
                    GtsPacket::TunnelClose {
                        tunnel_id: remote,
                        ack: true,
                    },
                    None,
                )
            }
            GtsPacket::TunnelClose {
                tunnel_id,
                ack: true,
            } => {
                self.tunnels
                    .get_mut(&tunnel_id)
                    .ok_or(Error::UnknownTunnel)?
                    .tunnel
                    .state = TunnelState::Closed;
                Ok(())
            }
            GtsPacket::Reset {
                tunnel_id,
                reset_id,
                ..
            } => {
                let rec = self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;
                if reset_id != rec.tunnel.local_reset_id {
                    return Err(Error::InvalidField);
                }
                rec.tunnel.reset();
                Ok(())
            }
        }
    }

    pub fn tick(&mut self, now: u64) -> Result<usize> {
        let mut pending = Vec::new();
        for rec in self.tunnels.values_mut() {
            if rec.tunnel.state != TunnelState::Established {
                continue;
            }
            let remote = rec.tunnel.remote_id()?;
            for s in rec.tunnel.streams.values_mut() {
                let profile = s.profile;
                for pkt in s.retransmit_due(remote, now) {
                    pending.push((rec.peer, profile, pkt));
                }
            }
        }
        let n = pending.len();
        for (peer, profile, pkt) in pending {
            self.send_gts(peer, pkt, Some(profile))?;
        }
        Ok(n)
    }
}

#[derive(Debug, Clone)]
pub struct DirectLink {
    attached: bool,
}

impl DirectLink {
    pub const fn new() -> Self {
        Self { attached: false }
    }

    pub fn attach(&mut self, a: &mut Endpoint, b: &mut Endpoint) -> Result<()> {
        if self.attached {
            return Ok(());
        }

        // The reserved control lane is established by the direct physical link
        // profile. Ordinary data receive capacity is deliberately *not* seeded
        // here; each endpoint advertises it using GCTL CREDIT below.
        a.dlp_mut()
            .grant_control_tx_credit(b.dlp().control_window_flits());
        b.dlp_mut()
            .grant_control_tx_credit(a.dlp().control_window_flits());

        a.on_link_attached(b.link_local_address())?;
        b.on_link_attached(a.link_local_address())?;
        self.attached = true;
        Ok(())
    }

    pub fn pump(
        &mut self,
        a: &mut Endpoint,
        b: &mut Endpoint,
        now: u64,
        max_flits: usize,
    ) -> Result<usize> {
        self.attach(a, b)?;
        let mut moved = 0;
        loop {
            if moved >= max_flits {
                return Err(Error::BufferFull);
            }
            let mut progress = false;

            match a.poll_tx_flit() {
                Ok(Some(f)) => {
                    let control = f.vcid.is_control();
                    b.receive_flit(f, now)?;
                    if control {
                        // Control receive storage is immediately reusable after
                        // synchronous point-to-point processing.
                        a.dlp_mut().grant_control_tx_credit(1);
                    }
                    moved += 1;
                    progress = true;
                }
                Ok(None) | Err(Error::NoCredit) => {}
                Err(e) => return Err(e),
            }

            match b.poll_tx_flit() {
                Ok(Some(f)) => {
                    let control = f.vcid.is_control();
                    a.receive_flit(f, now)?;
                    if control {
                        b.dlp_mut().grant_control_tx_credit(1);
                    }
                    moved += 1;
                    progress = true;
                }
                Ok(None) | Err(Error::NoCredit) => {}
                Err(e) => return Err(e),
            }

            if !progress {
                break;
            }
        }
        Ok(moved)
    }
}

impl Default for DirectLink {
    fn default() -> Self {
        Self::new()
    }
}
