use alloc::collections::VecDeque;
use core::ops::{Deref, DerefMut};

use crate::dlp::{DlpConfig, Flit, GnetFrame, VcMode};
use crate::endpoint::{Endpoint, EndpointConfig};
use crate::error::{Error, Result};
use crate::wire::gdp::GdpAddress;

pub const DLP_CONTROL_VERSION: u8 = 1;
pub const DLP_CONTROL_FRAME_LEN: usize = 4;
pub const DLP_CONTROL_PARAMETER_UNIT_FLITS: u32 = 4;
/// GNet 0.1 reserves this many physical flits for VC0 link-local GCTL.
/// It is a baseline link property, not receiver data credit and therefore is
/// neither discovered by peeking at the peer nor negotiated as ordinary credit.
pub const DLP_RESERVED_CONTROL_WINDOW_FLITS: u32 = 32;

const FLAG_VC2: u8 = 1 << 0;
const FLAG_VC4: u8 = 1 << 1;
const RATE_ALL: u8 = 0b111;
const CREDIT_REQUEST_RETRY_POLLS: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DlpControlOpcode {
    Hello = 0x1,
    Capabilities = 0x2,
    Reset = 0x3,
    LinkParameters = 0x8,
}

impl DlpControlOpcode {
    fn from_wire(v: u8) -> Result<Self> {
        match v {
            0x1 => Ok(Self::Hello),
            0x2 => Ok(Self::Capabilities),
            0x3 => Ok(Self::Reset),
            0x8 => Ok(Self::LinkParameters),
            _ => Err(Error::Unsupported),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DlpHelloKind {
    Initial = 0,
    Acknowledgement = 1,
}

impl DlpHelloKind {
    fn from_wire(v: u8) -> Result<Self> {
        match v {
            0 => Ok(Self::Initial),
            1 => Ok(Self::Acknowledgement),
            _ => Err(Error::InvalidField),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DlpCapabilityKind {
    Offer = 0,
    Selection = 1,
    Confirmation = 2,
}

impl DlpCapabilityKind {
    fn from_wire(v: u8) -> Result<Self> {
        match v {
            0 => Ok(Self::Offer),
            1 => Ok(Self::Selection),
            2 => Ok(Self::Confirmation),
            _ => Err(Error::InvalidField),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DlpControlState {
    Down,
    Hello,
    Negotiate,
    CreditSync,
    Up,
    Resetting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DlpNegotiatedProfile {
    pub vc_mode: VcMode,
    pub burst_flits: u32,
    pub peer_rx_buffer_flits: u32,
    pub peer_control_window_flits: u32,
}

/// One 32-bit logical GLCP control flit.
///
/// HELLO, CAPABILITIES and RESET follow the canonical GNet 0.1 layouts. The
/// LinkParameters opcode is the smolgnet v0.1 extension used to negotiate the
/// receive-window and burst limits required by the executable DLP model. It is
/// still exactly one 32-bit logical control flit and is generation-scoped like
/// the baseline operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DlpControlFrame {
    Hello {
        kind: DlpHelloKind,
        sender_generation: u8,
        peer_generation: u8,
    },
    Capabilities {
        kind: DlpCapabilityKind,
        sender_generation: u8,
        profiles: u8,
        rates: u8,
    },
    Reset {
        sender_generation: u8,
        reason: u8,
    },
    LinkParameters {
        kind: DlpCapabilityKind,
        sender_generation: u8,
        rx_window_units: u8,
        burst_units: u8,
    },
}

impl DlpControlFrame {
    pub const fn opcode(self) -> DlpControlOpcode {
        match self {
            Self::Hello { .. } => DlpControlOpcode::Hello,
            Self::Capabilities { .. } => DlpControlOpcode::Capabilities,
            Self::Reset { .. } => DlpControlOpcode::Reset,
            Self::LinkParameters { .. } => DlpControlOpcode::LinkParameters,
        }
    }

    pub const fn sender_generation(self) -> u8 {
        match self {
            Self::Hello {
                sender_generation, ..
            }
            | Self::Capabilities {
                sender_generation, ..
            }
            | Self::Reset {
                sender_generation, ..
            }
            | Self::LinkParameters {
                sender_generation, ..
            } => sender_generation,
        }
    }

    pub fn encode(self) -> [u8; DLP_CONTROL_FRAME_LEN] {
        let word = match self {
            Self::Hello {
                kind,
                sender_generation,
                peer_generation,
            } => {
                ((DlpControlOpcode::Hello as u32) << 28)
                    | ((DLP_CONTROL_VERSION as u32) << 24)
                    | ((kind as u32) << 22)
                    | (((sender_generation & 0x3f) as u32) << 16)
                    | (((peer_generation & 0x3f) as u32) << 10)
            }
            Self::Capabilities {
                kind,
                sender_generation,
                profiles,
                rates,
            } => {
                ((DlpControlOpcode::Capabilities as u32) << 28)
                    | ((DLP_CONTROL_VERSION as u32) << 24)
                    | (((sender_generation & 0x3f) as u32) << 18)
                    | ((kind as u32) << 16)
                    | (((profiles & 0x0f) as u32) << 12)
                    | (((rates & 0x07) as u32) << 9)
            }
            Self::Reset {
                sender_generation,
                reason,
            } => {
                ((DlpControlOpcode::Reset as u32) << 28)
                    | ((DLP_CONTROL_VERSION as u32) << 24)
                    | (((sender_generation & 0x3f) as u32) << 18)
                    | (((reason & 0x0f) as u32) << 14)
            }
            Self::LinkParameters {
                kind,
                sender_generation,
                rx_window_units,
                burst_units,
            } => {
                ((DlpControlOpcode::LinkParameters as u32) << 28)
                    | ((DLP_CONTROL_VERSION as u32) << 24)
                    | (((sender_generation & 0x3f) as u32) << 18)
                    | ((kind as u32) << 16)
                    | ((rx_window_units as u32) << 8)
                    | burst_units as u32
            }
        };
        word.to_be_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != DLP_CONTROL_FRAME_LEN {
            return Err(Error::InvalidLength);
        }
        let word = u32::from_be_bytes(bytes.try_into().unwrap());
        let opcode = DlpControlOpcode::from_wire(((word >> 28) & 0xf) as u8)?;
        let version = ((word >> 24) & 0xf) as u8;
        if version != DLP_CONTROL_VERSION {
            return Err(Error::Unsupported);
        }

        match opcode {
            DlpControlOpcode::Hello => {
                if word & 0x3ff != 0 {
                    return Err(Error::NonCanonical);
                }
                let kind = DlpHelloKind::from_wire(((word >> 22) & 0x3) as u8)?;
                let sender_generation = ((word >> 16) & 0x3f) as u8;
                let peer_generation = ((word >> 10) & 0x3f) as u8;
                if sender_generation == 0 {
                    return Err(Error::InvalidField);
                }
                match kind {
                    DlpHelloKind::Initial if peer_generation != 0 => {
                        return Err(Error::InvalidField)
                    }
                    DlpHelloKind::Acknowledgement if peer_generation == 0 => {
                        return Err(Error::InvalidField)
                    }
                    _ => {}
                }
                Ok(Self::Hello {
                    kind,
                    sender_generation,
                    peer_generation,
                })
            }
            DlpControlOpcode::Capabilities => {
                if word & 0x1ff != 0 {
                    return Err(Error::NonCanonical);
                }
                let sender_generation = ((word >> 18) & 0x3f) as u8;
                let kind = DlpCapabilityKind::from_wire(((word >> 16) & 0x3) as u8)?;
                let profiles = ((word >> 12) & 0xf) as u8;
                let rates = ((word >> 9) & 0x7) as u8;
                if sender_generation == 0 || profiles == 0 || rates == 0 {
                    return Err(Error::InvalidField);
                }
                if matches!(kind, DlpCapabilityKind::Selection | DlpCapabilityKind::Confirmation)
                    && (!profiles.is_power_of_two() || !rates.is_power_of_two())
                {
                    return Err(Error::InvalidField);
                }
                Ok(Self::Capabilities {
                    kind,
                    sender_generation,
                    profiles,
                    rates,
                })
            }
            DlpControlOpcode::Reset => {
                if word & 0x3fff != 0 {
                    return Err(Error::NonCanonical);
                }
                let sender_generation = ((word >> 18) & 0x3f) as u8;
                let reason = ((word >> 14) & 0xf) as u8;
                if sender_generation == 0 {
                    return Err(Error::InvalidField);
                }
                Ok(Self::Reset {
                    sender_generation,
                    reason,
                })
            }
            DlpControlOpcode::LinkParameters => {
                let sender_generation = ((word >> 18) & 0x3f) as u8;
                let kind = DlpCapabilityKind::from_wire(((word >> 16) & 0x3) as u8)?;
                let rx_window_units = ((word >> 8) & 0xff) as u8;
                let burst_units = (word & 0xff) as u8;
                if sender_generation == 0 || rx_window_units == 0 || burst_units == 0 {
                    return Err(Error::InvalidField);
                }
                Ok(Self::LinkParameters {
                    kind,
                    sender_generation,
                    rx_window_units,
                    burst_units,
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PeerCapabilityOffer {
    profiles: u8,
    rates: u8,
}

#[derive(Debug, Clone, Copy)]
struct PeerParameterOffer {
    rx_buffer_flits: u32,
    max_burst_flits: u32,
}

#[derive(Debug, Clone)]
pub struct DlpManagedEndpoint {
    endpoint: Endpoint,
    endpoint_config: EndpointConfig,
    max_burst_flits: u32,
    state: DlpControlState,
    negotiated: Option<DlpNegotiatedProfile>,

    local_generation: u8,
    peer_generation: Option<u8>,
    control_tx: VecDeque<DlpControlFrame>,
    hello_ack_sent: bool,
    hello_ack_received: bool,

    cap_offer_sent: bool,
    peer_cap_offer: Option<PeerCapabilityOffer>,
    selected_mode: Option<VcMode>,
    selected_rate: Option<u8>,
    cap_selection_sent: bool,
    cap_confirmation_sent: bool,
    cap_confirmation_received: bool,

    params_offer_sent: bool,
    peer_params_offer: Option<PeerParameterOffer>,
    selected_peer_rx: Option<u32>,
    selected_burst: Option<u32>,
    params_selection_sent: bool,
    params_confirmation_sent: bool,
    params_confirmation_received: bool,

    credit_stall_polls: u8,
}

impl DlpManagedEndpoint {
    pub fn new(address: GdpAddress, config: EndpointConfig, max_burst_flits: u32) -> Result<Self> {
        validate_parameter_value(config.rx_buffer_flits)?;
        validate_parameter_value(max_burst_flits)?;
        if config.control_window_flits != DLP_RESERVED_CONTROL_WINDOW_FLITS {
            return Err(Error::InvalidField);
        }
        let mut endpoint = Endpoint::new(address, config)?;
        endpoint.dlp_mut().set_link_up(false);
        Ok(Self::build(endpoint, config, max_burst_flits))
    }

    pub fn unconfigured(
        link_local_suffix: u64,
        config: EndpointConfig,
        max_burst_flits: u32,
    ) -> Result<Self> {
        validate_parameter_value(config.rx_buffer_flits)?;
        validate_parameter_value(max_burst_flits)?;
        if config.control_window_flits != DLP_RESERVED_CONTROL_WINDOW_FLITS {
            return Err(Error::InvalidField);
        }
        let mut endpoint = Endpoint::unconfigured(link_local_suffix, config)?;
        endpoint.dlp_mut().set_link_up(false);
        Ok(Self::build(endpoint, config, max_burst_flits))
    }

    fn build(endpoint: Endpoint, endpoint_config: EndpointConfig, max_burst_flits: u32) -> Self {
        Self {
            endpoint,
            endpoint_config,
            max_burst_flits,
            state: DlpControlState::Down,
            negotiated: None,
            local_generation: 1,
            peer_generation: None,
            control_tx: VecDeque::new(),
            hello_ack_sent: false,
            hello_ack_received: false,
            cap_offer_sent: false,
            peer_cap_offer: None,
            selected_mode: None,
            selected_rate: None,
            cap_selection_sent: false,
            cap_confirmation_sent: false,
            cap_confirmation_received: false,
            params_offer_sent: false,
            peer_params_offer: None,
            selected_peer_rx: None,
            selected_burst: None,
            params_selection_sent: false,
            params_confirmation_sent: false,
            params_confirmation_received: false,
            credit_stall_polls: 0,
        }
    }

    pub const fn control_state(&self) -> DlpControlState {
        self.state
    }

    pub const fn negotiated_profile(&self) -> Option<DlpNegotiatedProfile> {
        self.negotiated
    }

    pub const fn local_generation(&self) -> u8 {
        self.local_generation
    }

    pub const fn peer_generation(&self) -> Option<u8> {
        self.peer_generation
    }

    pub const fn control_pending(&self) -> bool {
        !self.control_tx.is_empty()
    }

    pub fn carrier_up(&mut self) -> Result<()> {
        if self.state != DlpControlState::Down {
            return Err(Error::InvalidState);
        }
        self.restart_negotiation(false, false);
        Ok(())
    }

    pub fn carrier_down(&mut self) {
        self.endpoint.link_detached();
        self.control_tx.clear();
        self.peer_generation = None;
        self.clear_negotiation_state();
        self.state = DlpControlState::Down;
    }

    pub fn request_reset(&mut self, reason: u32) -> Result<()> {
        if self.state == DlpControlState::Down {
            return Err(Error::LinkDown);
        }
        if reason > 0x0f {
            return Err(Error::InvalidField);
        }
        let old_generation = self.local_generation;
        self.endpoint.link_detached();
        self.control_tx.clear();
        self.control_tx.push_back(DlpControlFrame::Reset {
            sender_generation: old_generation,
            reason: reason as u8,
        });
        self.local_generation = next_generation(self.local_generation);
        self.peer_generation = None;
        self.clear_negotiation_state();
        self.state = DlpControlState::Hello;
        self.queue_hello_initial();
        Ok(())
    }

    pub fn poll_control(&mut self) -> Option<[u8; DLP_CONTROL_FRAME_LEN]> {
        self.control_tx.pop_front().map(DlpControlFrame::encode)
    }

    pub fn receive_control(&mut self, bytes: &[u8]) -> Result<()> {
        let frame = DlpControlFrame::decode(bytes)?;
        match frame {
            DlpControlFrame::Hello { .. } => self.on_hello(frame),
            DlpControlFrame::Reset { .. } => self.on_reset(frame),
            DlpControlFrame::Capabilities { .. } => {
                if !self.frame_is_current(frame) {
                    return Ok(());
                }
                self.on_capabilities(frame)
            }
            DlpControlFrame::LinkParameters { .. } => {
                if !self.frame_is_current(frame) {
                    return Ok(());
                }
                self.on_link_parameters(frame)
            }
        }
    }

    fn frame_is_current(&self, frame: DlpControlFrame) -> bool {
        self.peer_generation == Some(frame.sender_generation())
    }

    fn data_path_active(&self) -> bool {
        matches!(self.state, DlpControlState::CreditSync | DlpControlState::Up)
    }

    fn maybe_retry_data_credit_request(&mut self) {
        if self.state != DlpControlState::Up {
            self.credit_stall_polls = 0;
            return;
        }
        let queued = self.endpoint.dlp().queued_data_flits();
        let credit = self.endpoint.dlp().data_tx_credit() as usize;
        if queued == 0 || queued <= credit {
            self.credit_stall_polls = 0;
            return;
        }
        self.credit_stall_polls = self.credit_stall_polls.saturating_add(1);
        if self.credit_stall_polls >= CREDIT_REQUEST_RETRY_POLLS {
            self.endpoint.retry_managed_link_credit_request();
            self.credit_stall_polls = 0;
        }
    }

    pub fn poll_tx_flit(&mut self) -> Result<Option<Flit>> {
        if !self.data_path_active() {
            return Err(Error::LinkDown);
        }
        self.maybe_retry_data_credit_request();
        self.endpoint.poll_tx_flit()
    }

    pub fn poll_tx_frame(&mut self) -> Result<Option<GnetFrame>> {
        if !self.data_path_active() {
            return Err(Error::LinkDown);
        }
        self.maybe_retry_data_credit_request();
        self.endpoint.poll_tx_frame()
    }

    pub fn receive_flit(&mut self, flit: Flit, now: u64) -> Result<bool> {
        if !self.data_path_active() {
            return Err(Error::LinkDown);
        }
        let limit = self
            .negotiated
            .ok_or(Error::InvalidState)?
            .peer_rx_buffer_flits;
        self.endpoint.receive_managed_flit(flit, now, limit)
    }

    pub fn receive_frame(&mut self, frame: GnetFrame, now: u64) -> Result<bool> {
        if !self.data_path_active() {
            return Err(Error::LinkDown);
        }
        let limit = self
            .negotiated
            .ok_or(Error::InvalidState)?
            .peer_rx_buffer_flits;
        self.endpoint.receive_managed_frame(frame, now, limit)
    }

    fn restart_negotiation(&mut self, increment_generation: bool, preserve_control: bool) {
        self.endpoint.link_detached();
        if increment_generation {
            self.local_generation = next_generation(self.local_generation);
        }
        if !preserve_control {
            self.control_tx.clear();
        }
        self.clear_negotiation_state();
        self.state = DlpControlState::Hello;
        self.queue_hello_initial();
    }

    fn clear_negotiation_state(&mut self) {
        self.negotiated = None;
        self.hello_ack_sent = false;
        self.hello_ack_received = false;
        self.cap_offer_sent = false;
        self.peer_cap_offer = None;
        self.selected_mode = None;
        self.selected_rate = None;
        self.cap_selection_sent = false;
        self.cap_confirmation_sent = false;
        self.cap_confirmation_received = false;
        self.params_offer_sent = false;
        self.peer_params_offer = None;
        self.selected_peer_rx = None;
        self.selected_burst = None;
        self.params_selection_sent = false;
        self.params_confirmation_sent = false;
        self.params_confirmation_received = false;
        self.credit_stall_polls = 0;
    }

    fn queue_hello_initial(&mut self) {
        self.control_tx.push_back(DlpControlFrame::Hello {
            kind: DlpHelloKind::Initial,
            sender_generation: self.local_generation,
            peer_generation: 0,
        });
    }

    fn queue_hello_ack(&mut self, peer_generation: u8) {
        self.control_tx.push_back(DlpControlFrame::Hello {
            kind: DlpHelloKind::Acknowledgement,
            sender_generation: self.local_generation,
            peer_generation,
        });
        self.hello_ack_sent = true;
    }

    fn on_hello(&mut self, frame: DlpControlFrame) -> Result<()> {
        let DlpControlFrame::Hello {
            kind,
            sender_generation,
            peer_generation,
        } = frame
        else {
            unreachable!()
        };

        match kind {
            DlpHelloKind::Initial => {
                if let Some(current) = self.peer_generation {
                    if sender_generation != current {
                        // While a carrier remains present, a legitimate new
                        // peer generation is exactly the next generation. Any
                        // other mismatched HELLO is stale/reordered traffic.
                        if sender_generation != next_generation(current) {
                            return Ok(());
                        }
                        let was_live = matches!(
                            self.state,
                            DlpControlState::Negotiate
                                | DlpControlState::CreditSync
                                | DlpControlState::Up
                        );
                        self.restart_negotiation(was_live, false);
                    }
                } else if self.state == DlpControlState::Down {
                    return Err(Error::InvalidState);
                }
                self.peer_generation = Some(sender_generation);
                self.queue_hello_ack(sender_generation);
                self.maybe_start_capability_exchange()?;
            }
            DlpHelloKind::Acknowledgement => {
                if peer_generation != self.local_generation {
                    return Ok(());
                }
                match self.peer_generation {
                    Some(g) if g != sender_generation => {
                        if sender_generation != next_generation(g) {
                            return Ok(());
                        }
                        self.restart_negotiation(true, false);
                        self.peer_generation = Some(sender_generation);
                        self.queue_hello_ack(sender_generation);
                    }
                    None => {
                        self.peer_generation = Some(sender_generation);
                        self.queue_hello_ack(sender_generation);
                    }
                    Some(_) => {}
                }
                self.hello_ack_received = true;
                self.maybe_start_capability_exchange()?;
            }
        }
        Ok(())
    }

    fn maybe_start_capability_exchange(&mut self) -> Result<()> {
        if !self.hello_ack_sent || !self.hello_ack_received || self.peer_generation.is_none() {
            return Ok(());
        }
        self.state = DlpControlState::Negotiate;
        if !self.cap_offer_sent {
            self.control_tx.push_back(DlpControlFrame::Capabilities {
                kind: DlpCapabilityKind::Offer,
                sender_generation: self.local_generation,
                profiles: supported_mode_flags(self.endpoint_config.vc_mode),
                rates: RATE_ALL,
            });
            self.cap_offer_sent = true;
        }
        if !self.params_offer_sent {
            self.control_tx.push_back(DlpControlFrame::LinkParameters {
                kind: DlpCapabilityKind::Offer,
                sender_generation: self.local_generation,
                rx_window_units: encode_parameter_units(self.endpoint_config.rx_buffer_flits)?,
                burst_units: encode_parameter_units(self.max_burst_flits)?,
            });
            self.params_offer_sent = true;
        }
        Ok(())
    }

    fn on_capabilities(&mut self, frame: DlpControlFrame) -> Result<()> {
        let DlpControlFrame::Capabilities {
            kind,
            profiles,
            rates,
            ..
        } = frame
        else {
            unreachable!()
        };
        if self.state != DlpControlState::Negotiate {
            return Err(Error::InvalidState);
        }

        match kind {
            DlpCapabilityKind::Offer => {
                if let Some(old) = self.peer_cap_offer {
                    if old.profiles != profiles || old.rates != rates {
                        return Err(Error::InvalidField);
                    }
                } else {
                    self.peer_cap_offer = Some(PeerCapabilityOffer { profiles, rates });
                }
                let mode = choose_vc_mode(self.endpoint_config.vc_mode, profiles)?;
                let rate = choose_rate(RATE_ALL, rates)?;
                self.selected_mode = Some(mode);
                self.selected_rate = Some(rate);
                if !self.cap_selection_sent {
                    self.control_tx.push_back(DlpControlFrame::Capabilities {
                        kind: DlpCapabilityKind::Selection,
                        sender_generation: self.local_generation,
                        profiles: selected_mode_flag(mode),
                        rates: rate,
                    });
                    self.cap_selection_sent = true;
                }
            }
            DlpCapabilityKind::Selection => {
                let offer = self.peer_cap_offer.ok_or(Error::InvalidState)?;
                let expected_mode = choose_vc_mode(self.endpoint_config.vc_mode, offer.profiles)?;
                let expected_rate = choose_rate(RATE_ALL, offer.rates)?;
                if profiles != selected_mode_flag(expected_mode) || rates != expected_rate {
                    return Err(Error::InvalidField);
                }
                if !self.cap_confirmation_sent {
                    self.control_tx.push_back(DlpControlFrame::Capabilities {
                        kind: DlpCapabilityKind::Confirmation,
                        sender_generation: self.local_generation,
                        profiles,
                        rates,
                    });
                    self.cap_confirmation_sent = true;
                }
            }
            DlpCapabilityKind::Confirmation => {
                let mode = self.selected_mode.ok_or(Error::InvalidState)?;
                let rate = self.selected_rate.ok_or(Error::InvalidState)?;
                if profiles != selected_mode_flag(mode) || rates != rate {
                    return Err(Error::InvalidField);
                }
                self.cap_confirmation_received = true;
            }
        }
        self.maybe_install_profile()
    }

    fn on_link_parameters(&mut self, frame: DlpControlFrame) -> Result<()> {
        let DlpControlFrame::LinkParameters {
            kind,
            rx_window_units,
            burst_units,
            ..
        } = frame
        else {
            unreachable!()
        };
        if self.state != DlpControlState::Negotiate {
            return Err(Error::InvalidState);
        }
        let rx = decode_parameter_units(rx_window_units)?;
        let burst = decode_parameter_units(burst_units)?;

        match kind {
            DlpCapabilityKind::Offer => {
                if let Some(old) = self.peer_params_offer {
                    if old.rx_buffer_flits != rx || old.max_burst_flits != burst {
                        return Err(Error::InvalidField);
                    }
                } else {
                    self.peer_params_offer = Some(PeerParameterOffer {
                        rx_buffer_flits: rx,
                        max_burst_flits: burst,
                    });
                }
                let selected_burst = self.max_burst_flits.min(burst);
                self.selected_peer_rx = Some(rx);
                self.selected_burst = Some(selected_burst);
                if !self.params_selection_sent {
                    self.control_tx.push_back(DlpControlFrame::LinkParameters {
                        kind: DlpCapabilityKind::Selection,
                        sender_generation: self.local_generation,
                        rx_window_units: encode_parameter_units(rx)?,
                        burst_units: encode_parameter_units(selected_burst)?,
                    });
                    self.params_selection_sent = true;
                }
            }
            DlpCapabilityKind::Selection => {
                let offer = self.peer_params_offer.ok_or(Error::InvalidState)?;
                let expected_burst = self.max_burst_flits.min(offer.max_burst_flits);
                if rx != self.endpoint_config.rx_buffer_flits || burst != expected_burst {
                    return Err(Error::InvalidField);
                }
                if !self.params_confirmation_sent {
                    self.control_tx.push_back(DlpControlFrame::LinkParameters {
                        kind: DlpCapabilityKind::Confirmation,
                        sender_generation: self.local_generation,
                        rx_window_units,
                        burst_units,
                    });
                    self.params_confirmation_sent = true;
                }
            }
            DlpCapabilityKind::Confirmation => {
                let peer_rx = self.selected_peer_rx.ok_or(Error::InvalidState)?;
                let selected_burst = self.selected_burst.ok_or(Error::InvalidState)?;
                if rx != peer_rx || burst != selected_burst {
                    return Err(Error::InvalidField);
                }
                self.params_confirmation_received = true;
            }
        }
        self.maybe_install_profile()
    }

    fn maybe_install_profile(&mut self) -> Result<()> {
        if self.state != DlpControlState::Negotiate
            || !self.cap_confirmation_sent
            || !self.cap_confirmation_received
            || !self.params_confirmation_sent
            || !self.params_confirmation_received
        {
            return Ok(());
        }

        let mode = self.selected_mode.ok_or(Error::InvalidState)?;
        let burst = self.selected_burst.ok_or(Error::InvalidState)?;
        let peer_rx = self.selected_peer_rx.ok_or(Error::InvalidState)?;
        let mut cfg = DlpConfig::new(self.endpoint_config.rx_buffer_flits, mode)?;
        cfg.gdp = self.endpoint_config.gdp_wire;
        cfg.local_prefix = self.endpoint_config.local_context_prefix.unwrap_or(0) & !0xffff;
        cfg.control_window_flits = DLP_RESERVED_CONTROL_WINDOW_FLITS;
        *self.endpoint.dlp_mut() = crate::dlp::DlpEndpoint::new(cfg)?;
        self.negotiated = Some(DlpNegotiatedProfile {
            vc_mode: mode,
            burst_flits: burst,
            peer_rx_buffer_flits: peer_rx,
            peer_control_window_flits: DLP_RESERVED_CONTROL_WINDOW_FLITS,
        });
        self.state = DlpControlState::CreditSync;
        Ok(())
    }

    fn on_reset(&mut self, frame: DlpControlFrame) -> Result<()> {
        let DlpControlFrame::Reset {
            sender_generation, ..
        } = frame
        else {
            unreachable!()
        };
        if self.peer_generation != Some(sender_generation) {
            return Ok(());
        }
        self.endpoint.link_detached();
        self.control_tx.clear();
        self.peer_generation = None;
        self.local_generation = next_generation(self.local_generation);
        self.clear_negotiation_state();
        self.state = DlpControlState::Hello;
        self.queue_hello_initial();
        Ok(())
    }

    fn bind_credit_peer(&mut self, peer: GdpAddress) -> Result<()> {
        if self.state != DlpControlState::CreditSync {
            return Err(Error::InvalidState);
        }
        self.endpoint
            .bind_managed_link_peer(peer, DLP_RESERVED_CONTROL_WINDOW_FLITS)
    }

    fn finish_credit_sync(&mut self) -> Result<()> {
        if self.state != DlpControlState::CreditSync {
            return Err(Error::InvalidState);
        }
        if self.endpoint.dlp().data_tx_credit() == 0 {
            return Err(Error::NoCredit);
        }
        self.state = DlpControlState::Up;
        Ok(())
    }
}

impl Deref for DlpManagedEndpoint {
    type Target = Endpoint;

    fn deref(&self) -> &Self::Target {
        &self.endpoint
    }
}

impl DerefMut for DlpManagedEndpoint {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.endpoint
    }
}

#[derive(Debug, Clone, Default)]
pub struct DlpDirectCable {
    carrier: bool,
    credit_bound: bool,
}

impl DlpDirectCable {
    pub const fn new() -> Self {
        Self {
            carrier: false,
            credit_bound: false,
        }
    }

    fn both_up(a: &DlpManagedEndpoint, b: &DlpManagedEndpoint) -> bool {
        a.control_state() == DlpControlState::Up && b.control_state() == DlpControlState::Up
    }

    fn both_ready_for_credit(a: &DlpManagedEndpoint, b: &DlpManagedEndpoint) -> bool {
        matches!(a.control_state(), DlpControlState::CreditSync | DlpControlState::Up)
            && matches!(b.control_state(), DlpControlState::CreditSync | DlpControlState::Up)
    }

    pub fn attach(&mut self, a: &mut DlpManagedEndpoint, b: &mut DlpManagedEndpoint) -> Result<()> {
        if !self.carrier {
            if a.control_state() == DlpControlState::Down {
                a.carrier_up()?;
            }
            if b.control_state() == DlpControlState::Down {
                b.carrier_up()?;
            }
            self.carrier = true;
        }

        if !Self::both_ready_for_credit(a, b) {
            self.credit_bound = false;
            for _ in 0..512 {
                let progress = Self::pump_control(a, b)?;
                if Self::both_ready_for_credit(a, b) {
                    break;
                }
                if !progress {
                    return Err(Error::InvalidState);
                }
            }
        }
        if !Self::both_ready_for_credit(a, b) {
            return Err(Error::InvalidState);
        }

        if !self.credit_bound {
            let a_peer = b.link_local_address();
            let b_peer = a.link_local_address();
            if a.control_state() == DlpControlState::CreditSync {
                a.bind_credit_peer(a_peer)?;
            }
            if b.control_state() == DlpControlState::CreditSync {
                b.bind_credit_peer(b_peer)?;
            }
            self.credit_bound = true;

            for _ in 0..512 {
                if a.dlp().data_tx_credit() != 0 && b.dlp().data_tx_credit() != 0 {
                    break;
                }
                let progress = Self::pump_data_once(a, b, 0)?;
                if !progress {
                    return Err(Error::InvalidState);
                }
            }
            if a.dlp().data_tx_credit() == 0 || b.dlp().data_tx_credit() == 0 {
                return Err(Error::NoCredit);
            }
            if a.control_state() == DlpControlState::CreditSync {
                a.finish_credit_sync()?;
            }
            if b.control_state() == DlpControlState::CreditSync {
                b.finish_credit_sync()?;
            }
        }

        if !Self::both_up(a, b) {
            return Err(Error::InvalidState);
        }
        Ok(())
    }

    fn pump_control(a: &mut DlpManagedEndpoint, b: &mut DlpManagedEndpoint) -> Result<bool> {
        let mut progress = false;
        if let Some(frame) = a.poll_control() {
            b.receive_control(&frame)?;
            progress = true;
        }
        if let Some(frame) = b.poll_control() {
            a.receive_control(&frame)?;
            progress = true;
        }
        Ok(progress)
    }

    fn pump_data_direction(
        sender: &mut DlpManagedEndpoint,
        receiver: &mut DlpManagedEndpoint,
        now: u64,
    ) -> Result<bool> {
        if !sender.data_path_active() || !receiver.data_path_active() {
            return Ok(false);
        }
        match sender.poll_tx_flit() {
            Ok(Some(flit)) => {
                let reserved_control = flit.vcid.is_control();
                receiver.receive_flit(flit, now)?;
                if reserved_control {
                    sender.dlp_mut().grant_control_tx_credit(1);
                }
                Ok(true)
            }
            Ok(None) | Err(Error::NoCredit) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn pump_data_once(
        a: &mut DlpManagedEndpoint,
        b: &mut DlpManagedEndpoint,
        now: u64,
    ) -> Result<bool> {
        let mut progress = false;
        if Self::pump_data_direction(a, b, now)? {
            progress = true;
        }
        if Self::pump_data_direction(b, a, now)? {
            progress = true;
        }
        Ok(progress)
    }

    pub fn pump(
        &mut self,
        a: &mut DlpManagedEndpoint,
        b: &mut DlpManagedEndpoint,
        now: u64,
        max_transfers: usize,
    ) -> Result<usize> {
        if !Self::both_up(a, b) {
            self.credit_bound = false;
        }
        self.attach(a, b)?;

        let mut moved = 0usize;
        loop {
            if moved >= max_transfers {
                return Err(Error::BufferFull);
            }
            let mut progress = false;

            if Self::pump_control(a, b)? {
                moved += 1;
                progress = true;
                if !Self::both_up(a, b) {
                    self.credit_bound = false;
                    self.attach(a, b)?;
                }
            }

            if Self::pump_data_once(a, b, now)? {
                moved += 1;
                progress = true;
            }

            if !progress {
                return Ok(moved);
            }
        }
    }
}

fn validate_parameter_value(value: u32) -> Result<()> {
    encode_parameter_units(value).map(|_| ())
}

fn encode_parameter_units(value: u32) -> Result<u8> {
    if value == 0 || value % DLP_CONTROL_PARAMETER_UNIT_FLITS != 0 {
        return Err(Error::InvalidField);
    }
    let units = value / DLP_CONTROL_PARAMETER_UNIT_FLITS;
    if units == 0 || units > u8::MAX as u32 {
        return Err(Error::InvalidField);
    }
    Ok(units as u8)
}

fn decode_parameter_units(units: u8) -> Result<u32> {
    if units == 0 {
        return Err(Error::InvalidField);
    }
    Ok(units as u32 * DLP_CONTROL_PARAMETER_UNIT_FLITS)
}

fn next_generation(generation: u8) -> u8 {
    if generation >= 63 {
        1
    } else {
        generation + 1
    }
}

fn supported_mode_flags(max_mode: VcMode) -> u8 {
    match max_mode {
        VcMode::Two => FLAG_VC2,
        VcMode::Four => FLAG_VC2 | FLAG_VC4,
    }
}

fn selected_mode_flag(mode: VcMode) -> u8 {
    match mode {
        VcMode::Two => FLAG_VC2,
        VcMode::Four => FLAG_VC4,
    }
}

fn choose_vc_mode(local_max: VcMode, peer_flags: u8) -> Result<VcMode> {
    let local = supported_mode_flags(local_max);
    let common = local & peer_flags;
    if common & FLAG_VC4 != 0 {
        Ok(VcMode::Four)
    } else if common & FLAG_VC2 != 0 {
        Ok(VcMode::Two)
    } else {
        Err(Error::Unsupported)
    }
}

fn choose_rate(local_rates: u8, peer_rates: u8) -> Result<u8> {
    let common = (local_rates & peer_rates) & 0x07;
    if common & 0b100 != 0 {
        Ok(0b100)
    } else if common & 0b010 != 0 {
        Ok(0b010)
    } else if common & 0b001 != 0 {
        Ok(0b001)
    } else {
        Err(Error::Unsupported)
    }
}
