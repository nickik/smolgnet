use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::error::{Error, Result};
use crate::wire::css::ServiceSelector;
use crate::wire::gdp::SizeClass;
use crate::wire::gts::{GtsPacket, StreamProfile};

pub use crate::wire::gts::Direction;

pub const DEFAULT_RTO_MS: u64 = 500;
pub const DEFAULT_RX_SLOTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelRole {
    Initiator,
    Responder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    Connecting,
    Established,
    Closing,
    Closed,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Opening,
    Open,
    Closing,
    Closed,
    Reset,
}

#[derive(Debug, Clone)]
struct PendingTx {
    data: Vec<u8>,
    end: bool,
    sent_at: u64,
}

#[derive(Debug, Clone)]
pub struct ReliableTx {
    next_sequence: u32,
    peer_credit: u8,
    outstanding: BTreeMap<u32, PendingTx>,
    rto_ms: u64,
}

impl ReliableTx {
    pub fn new(peer_credit: u8) -> Self {
        Self {
            next_sequence: 0,
            peer_credit,
            outstanding: BTreeMap::new(),
            rto_ms: DEFAULT_RTO_MS,
        }
    }

    pub const fn peer_credit(&self) -> u8 {
        self.peer_credit
    }

    pub fn outstanding(&self) -> usize {
        self.outstanding.len()
    }

    pub fn queue(&mut self, data: Vec<u8>, end: bool, now: u64) -> Result<u32> {
        if self.peer_credit == 0 {
            return Err(Error::WouldBlock);
        }
        let seq = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.peer_credit -= 1;
        self.outstanding.insert(
            seq,
            PendingTx {
                data,
                end,
                sent_at: now,
            },
        );
        Ok(seq)
    }

    pub fn on_ack(&mut self, base: u32, bitmap: u32, credit: u8) {
        let keys: Vec<u32> = self.outstanding.keys().copied().collect();
        for k in keys {
            let acked = if k <= base {
                true
            } else {
                let d = k.wrapping_sub(base).wrapping_sub(1);
                d < 32 && (bitmap & (1u32 << d)) != 0
            };
            if acked {
                self.outstanding.remove(&k);
            }
        }
        self.peer_credit = credit;
    }

    pub fn due(&mut self, now: u64) -> Vec<(u32, Vec<u8>, bool)> {
        let mut out = Vec::new();
        for (&seq, p) in self.outstanding.iter_mut() {
            if now.saturating_sub(p.sent_at) >= self.rto_ms {
                p.sent_at = now;
                out.push((seq, p.data.clone(), p.end));
            }
        }
        out
    }

    fn reset(&mut self) {
        self.outstanding.clear();
        self.peer_credit = 0;
    }
}

#[derive(Debug, Clone)]
pub struct ReliableRx {
    next_expected: u32,
    pending: BTreeMap<u32, Vec<u8>>,
    delivered: VecDeque<Vec<u8>>,
    capacity: usize,
    seen_any: bool,
}

impl ReliableRx {
    pub fn new(capacity: usize) -> Self {
        Self {
            next_expected: 0,
            pending: BTreeMap::new(),
            delivered: VecDeque::new(),
            capacity,
            seen_any: false,
        }
    }

    fn used(&self) -> usize {
        self.pending.len() + self.delivered.len()
    }

    pub fn credit(&self) -> u8 {
        self.capacity.saturating_sub(self.used()).min(255) as u8
    }

    pub fn receive(&mut self, seq: u32, data: Vec<u8>) -> Result<bool> {
        if seq < self.next_expected {
            return Ok(false);
        }
        if seq == self.next_expected {
            if self.used() >= self.capacity {
                return Err(Error::BufferFull);
            }
            self.delivered.push_back(data);
            self.seen_any = true;
            self.next_expected = self.next_expected.wrapping_add(1);
            loop {
                let n = self.next_expected;
                if let Some(v) = self.pending.remove(&n) {
                    self.delivered.push_back(v);
                    self.next_expected = self.next_expected.wrapping_add(1);
                } else {
                    break;
                }
            }
            return Ok(true);
        }

        let distance = seq.wrapping_sub(self.next_expected);
        if distance > 32 || self.used() >= self.capacity {
            return Err(Error::BufferFull);
        }
        self.pending.entry(seq).or_insert(data);
        Ok(false)
    }

    pub fn ack(&self) -> Option<(u32, u32, u8)> {
        if !self.seen_any {
            return None;
        }
        let base = self.next_expected.wrapping_sub(1);
        let mut bitmap = 0u32;
        for &seq in self.pending.keys() {
            let d = seq.wrapping_sub(base).wrapping_sub(1);
            if d < 32 {
                bitmap |= 1u32 << d;
            }
        }
        Some((base, bitmap, self.credit()))
    }

    pub fn recv(&mut self) -> Option<Vec<u8>> {
        self.delivered.pop_front()
    }

    fn reset(&mut self) {
        self.pending.clear();
        self.delivered.clear();
        self.next_expected = 0;
        self.seen_any = false;
    }
}

#[derive(Debug, Clone)]
pub struct GtsStream {
    pub id: u8,
    pub profile: StreamProfile,
    pub state: StreamState,
    pub opened_by_local: bool,
    reliable_tx: Option<ReliableTx>,
    reliable_rx: Option<ReliableRx>,
    unrel_tx_sequence: u32,
    unrel_rx_highest: Option<u32>,
    unrel_delivered: VecDeque<Vec<u8>>,
    local_ended: bool,
    remote_ended: bool,
}

impl GtsStream {
    pub fn new(
        id: u8,
        profile: StreamProfile,
        opened_by_local: bool,
        local_receive_credit: u8,
        peer_receive_credit: u8,
    ) -> Result<Self> {
        profile.validate()?;
        let reliable_tx = if profile.unreliable {
            None
        } else {
            Some(ReliableTx::new(peer_receive_credit))
        };
        let reliable_rx = if profile.unreliable {
            None
        } else {
            Some(ReliableRx::new(local_receive_credit.max(1) as usize))
        };
        Ok(Self {
            id,
            profile,
            state: StreamState::Open,
            opened_by_local,
            reliable_tx,
            reliable_rx,
            unrel_tx_sequence: 0,
            unrel_rx_highest: None,
            unrel_delivered: VecDeque::new(),
            local_ended: false,
            remote_ended: false,
        })
    }

    fn local_may_send(&self, local_is_opener: bool) -> bool {
        if local_is_opener {
            self.profile.direction.opener_may_send()
        } else {
            self.profile.direction.peer_may_send()
        }
    }

    fn remote_may_send(&self, local_is_opener: bool) -> bool {
        if local_is_opener {
            self.profile.direction.peer_may_send()
        } else {
            self.profile.direction.opener_may_send()
        }
    }

    fn fixed_reliable_data_len(&self) -> Result<usize> {
        self.profile
            .size_class
            .bytes()
            .checked_sub(14)
            .ok_or(Error::InvalidLength)
    }

    fn fixed_reliable_end_capacity(&self) -> Result<usize> {
        self.profile
            .size_class
            .bytes()
            .checked_sub(16)
            .ok_or(Error::InvalidLength)
    }

    fn fixed_unreliable_data_len(&self) -> Result<usize> {
        let overhead = if self.profile.sequenced { 14 } else { 10 };
        self.profile
            .size_class
            .bytes()
            .checked_sub(overhead)
            .ok_or(Error::InvalidLength)
    }

    fn validate_send_len(&self, data_len: usize, end: bool) -> Result<()> {
        if self.profile.unreliable {
            if end {
                return Err(Error::ProfileViolation);
            }
            if self.profile.variable {
                let overhead = if self.profile.sequenced { 16 } else { 12 };
                if data_len + overhead > self.profile.size_class.bytes() {
                    return Err(Error::MessageTooLarge);
                }
            } else if data_len != self.fixed_unreliable_data_len()? {
                return Err(Error::InvalidLength);
            }
            return Ok(());
        }

        if self.profile.variable {
            if data_len + 16 > self.profile.size_class.bytes() {
                return Err(Error::MessageTooLarge);
            }
        } else if end {
            if data_len > self.fixed_reliable_end_capacity()? {
                return Err(Error::MessageTooLarge);
            }
        } else if data_len != self.fixed_reliable_data_len()? {
            return Err(Error::InvalidLength);
        }
        Ok(())
    }

    /// Validate a received stream packet against negotiated reliability,
    /// Size Class, fixed/variable semantics, direction and stream state.
    pub fn validate_incoming(&self, size_class: SizeClass, packet: &GtsPacket) -> Result<()> {
        if matches!(self.state, StreamState::Closed | StreamState::Reset) {
            return Err(Error::InvalidState);
        }

        match packet {
            GtsPacket::Data { data, end, .. } => {
                if self.profile.unreliable || self.remote_ended {
                    return Err(Error::ProfileViolation);
                }
                if !self.remote_may_send(self.opened_by_local) {
                    return Err(Error::DirectionViolation);
                }
                if self.profile.variable {
                    if (size_class as u8) > (self.profile.size_class as u8)
                        || data.len() + 16 > size_class.bytes()
                    {
                        return Err(Error::ProfileViolation);
                    }
                } else {
                    if size_class != self.profile.size_class {
                        return Err(Error::ProfileViolation);
                    }
                    if *end {
                        if data.len() > self.fixed_reliable_end_capacity()? {
                            return Err(Error::InvalidLength);
                        }
                    } else if data.len() != self.fixed_reliable_data_len()? {
                        return Err(Error::InvalidLength);
                    }
                }
            }
            GtsPacket::Datagram { data, sequence, .. } => {
                if !self.profile.unreliable {
                    return Err(Error::ProfileViolation);
                }
                if !self.remote_may_send(self.opened_by_local) {
                    return Err(Error::DirectionViolation);
                }
                if self.profile.sequenced != sequence.is_some() {
                    return Err(Error::ProfileViolation);
                }
                if self.profile.variable {
                    let overhead = if self.profile.sequenced { 16 } else { 12 };
                    if (size_class as u8) > (self.profile.size_class as u8)
                        || data.len() + overhead > size_class.bytes()
                    {
                        return Err(Error::ProfileViolation);
                    }
                } else if size_class != self.profile.size_class
                    || data.len() != self.fixed_unreliable_data_len()?
                {
                    return Err(Error::InvalidLength);
                }
            }
            GtsPacket::Ack { .. } => {
                if self.profile.unreliable {
                    return Err(Error::ProfileViolation);
                }
            }
            GtsPacket::StreamClose { final_sequence, .. } => {
                if self.profile.unreliable && *final_sequence != 0 {
                    return Err(Error::ProfileViolation);
                }
            }
            GtsPacket::StreamReset { .. } => {}
            _ => return Err(Error::ProfileViolation),
        }
        Ok(())
    }

    pub fn send_packet(
        &mut self,
        tunnel_id: u32,
        data: Vec<u8>,
        end: bool,
        now: u64,
    ) -> Result<GtsPacket> {
        if self.state != StreamState::Open {
            return Err(Error::InvalidState);
        }
        if self.local_ended {
            return Err(Error::InvalidState);
        }
        if !self.local_may_send(self.opened_by_local) {
            return Err(Error::DirectionViolation);
        }
        self.validate_send_len(data.len(), end)?;

        if self.profile.unreliable {
            let seq = if self.profile.sequenced {
                let s = self.unrel_tx_sequence;
                self.unrel_tx_sequence = self.unrel_tx_sequence.wrapping_add(1);
                Some(s)
            } else {
                None
            };
            Ok(GtsPacket::Datagram {
                tunnel_id,
                stream_id: self.id,
                sequence: seq,
                data,
            })
        } else {
            let tx = self.reliable_tx.as_mut().expect("reliable tx exists");
            let seq = tx.queue(data.clone(), end, now)?;
            if end {
                self.local_ended = true;
            }
            Ok(GtsPacket::Data {
                tunnel_id,
                stream_id: self.id,
                sequence: seq,
                data,
                end,
            })
        }
    }

    pub fn receive_packet(&mut self, packet: &GtsPacket) -> Result<Option<GtsPacket>> {
        if matches!(self.state, StreamState::Closed | StreamState::Reset) {
            return Err(Error::InvalidState);
        }
        if !self.remote_may_send(self.opened_by_local) {
            return Err(Error::DirectionViolation);
        }

        match packet {
            GtsPacket::Data {
                tunnel_id,
                stream_id,
                sequence,
                data,
                end,
            } if !self.profile.unreliable => {
                if *stream_id != self.id || self.remote_ended {
                    return Err(Error::InvalidState);
                }
                let rx = self.reliable_rx.as_mut().expect("reliable rx exists");
                rx.receive(*sequence, data.clone())?;
                if *end {
                    self.remote_ended = true;
                }
                if let Some((base, bm, credit)) = rx.ack() {
                    Ok(Some(GtsPacket::Ack {
                        tunnel_id: *tunnel_id,
                        stream_id: *stream_id,
                        ack_base: base,
                        receive_bitmap: bm,
                        receive_credit: credit,
                    }))
                } else {
                    Ok(None)
                }
            }
            GtsPacket::Datagram {
                stream_id,
                sequence,
                data,
                ..
            } if self.profile.unreliable => {
                if *stream_id != self.id {
                    return Err(Error::UnknownStream);
                }
                if self.profile.sequenced {
                    let s = sequence.ok_or(Error::ProfileViolation)?;
                    if let Some(h) = self.unrel_rx_highest {
                        if s <= h {
                            return Ok(None);
                        }
                    }
                    self.unrel_rx_highest = Some(s);
                }
                self.unrel_delivered.push_back(data.clone());
                Ok(None)
            }
            _ => Err(Error::ProfileViolation),
        }
    }

    pub fn on_ack(&mut self, base: u32, bitmap: u32, credit: u8) -> Result<()> {
        if self.state == StreamState::Reset || self.profile.unreliable {
            return Err(Error::ProfileViolation);
        }
        self.reliable_tx
            .as_mut()
            .ok_or(Error::ProfileViolation)?
            .on_ack(base, bitmap, credit);
        Ok(())
    }

    pub fn recv(&mut self) -> Option<Vec<u8>> {
        if self.profile.unreliable {
            self.unrel_delivered.pop_front()
        } else {
            self.reliable_rx.as_mut().expect("reliable rx exists").recv()
        }
    }

    /// Current ACK/credit snapshot after the application has consumed data.
    pub fn receive_credit_ack(&self, tunnel_id: u32) -> Option<GtsPacket> {
        if self.profile.unreliable || self.state == StreamState::Reset {
            return None;
        }
        let rx = self.reliable_rx.as_ref()?;
        rx.ack().map(|(ack_base, receive_bitmap, receive_credit)| GtsPacket::Ack {
            tunnel_id,
            stream_id: self.id,
            ack_base,
            receive_bitmap,
            receive_credit,
        })
    }

    pub fn retransmit_due(&mut self, tunnel_id: u32, now: u64) -> Vec<GtsPacket> {
        let Some(tx) = self.reliable_tx.as_mut() else {
            return Vec::new();
        };
        tx.due(now)
            .into_iter()
            .map(|(sequence, data, end)| GtsPacket::Data {
                tunnel_id,
                stream_id: self.id,
                sequence,
                data,
                end,
            })
            .collect()
    }

    pub fn peer_credit(&self) -> Option<u8> {
        self.reliable_tx.as_ref().map(|x| x.peer_credit())
    }

    pub fn apply_open_ack(&mut self, peer_credit: u8) -> Result<()> {
        if self.state != StreamState::Opening {
            return Err(Error::InvalidState);
        }
        if let Some(tx) = self.reliable_tx.as_mut() {
            tx.peer_credit = peer_credit;
        }
        self.state = StreamState::Open;
        Ok(())
    }

    pub fn mark_opening(&mut self) {
        self.state = StreamState::Opening;
    }

    pub fn reset(&mut self) {
        self.state = StreamState::Reset;
        if let Some(tx) = self.reliable_tx.as_mut() {
            tx.reset();
        }
        if let Some(rx) = self.reliable_rx.as_mut() {
            rx.reset();
        }
        self.unrel_delivered.clear();
        self.unrel_rx_highest = None;
        self.local_ended = true;
        self.remote_ended = true;
    }
}

#[derive(Debug, Clone)]
pub struct GtsTunnel {
    pub role: TunnelRole,
    pub state: TunnelState,
    pub local_receive_id: u32,
    pub remote_receive_id: Option<u32>,
    pub local_reset_id: u32,
    pub remote_reset_id: Option<u32>,
    pub css: ServiceSelector,
    pub streams: BTreeMap<u8, GtsStream>,
    next_stream_id: u8,
}

impl GtsTunnel {
    pub fn initiator(
        local_receive_id: u32,
        local_reset_id: u32,
        css: ServiceSelector,
        stream0_profile: StreamProfile,
        local_credit: u8,
    ) -> Result<Self> {
        let mut streams = BTreeMap::new();
        streams.insert(
            0,
            GtsStream::new(0, stream0_profile, true, local_credit, 0)?,
        );
        Ok(Self {
            role: TunnelRole::Initiator,
            state: TunnelState::Connecting,
            local_receive_id,
            remote_receive_id: None,
            local_reset_id,
            remote_reset_id: None,
            css,
            streams,
            next_stream_id: 2,
        })
    }

    pub fn responder(
        local_receive_id: u32,
        local_reset_id: u32,
        remote_receive_id: u32,
        remote_reset_id: u32,
        css: ServiceSelector,
        stream0_profile: StreamProfile,
        local_credit: u8,
        peer_credit: u8,
    ) -> Result<Self> {
        let mut streams = BTreeMap::new();
        streams.insert(
            0,
            GtsStream::new(0, stream0_profile, false, local_credit, peer_credit)?,
        );
        Ok(Self {
            role: TunnelRole::Responder,
            state: TunnelState::Established,
            local_receive_id,
            remote_receive_id: Some(remote_receive_id),
            local_reset_id,
            remote_reset_id: Some(remote_reset_id),
            css,
            streams,
            next_stream_id: 1,
        })
    }

    pub fn establish_initiator(
        &mut self,
        remote_receive_id: u32,
        remote_reset_id: u32,
        peer_credit: u8,
    ) -> Result<()> {
        if self.state != TunnelState::Connecting {
            return Err(Error::InvalidState);
        }
        self.remote_receive_id = Some(remote_receive_id);
        self.remote_reset_id = Some(remote_reset_id);
        self.state = TunnelState::Established;
        let s = self.streams.get_mut(&0).expect("stream 0 exists");
        if let Some(tx) = s.reliable_tx.as_mut() {
            tx.peer_credit = peer_credit;
        }
        Ok(())
    }

    pub fn alloc_stream_id(&mut self) -> Result<u8> {
        if self.state != TunnelState::Established {
            return Err(Error::InvalidState);
        }
        let id = self.next_stream_id;
        if id > 253 {
            return Err(Error::BufferFull);
        }
        self.next_stream_id = self.next_stream_id.wrapping_add(2);
        Ok(id)
    }

    pub fn add_stream(
        &mut self,
        id: u8,
        profile: StreamProfile,
        opened_by_local: bool,
        local_credit: u8,
        peer_credit: u8,
    ) -> Result<()> {
        if self.state != TunnelState::Established || self.streams.contains_key(&id) {
            return Err(Error::InvalidState);
        }
        self.streams.insert(
            id,
            GtsStream::new(id, profile, opened_by_local, local_credit, peer_credit)?,
        );
        Ok(())
    }

    pub fn stream_mut(&mut self, id: u8) -> Result<&mut GtsStream> {
        self.streams.get_mut(&id).ok_or(Error::UnknownStream)
    }

    pub fn remote_id(&self) -> Result<u32> {
        self.remote_receive_id.ok_or(Error::InvalidState)
    }

    pub fn reset(&mut self) {
        self.state = TunnelState::Reset;
        for stream in self.streams.values_mut() {
            stream.reset();
        }
    }
}
