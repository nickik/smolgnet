use crate::error::{Error, Result};
use crate::storage::{MessagePool, MessageSlot};
use crate::wire::css::ServiceSelector;
use crate::wire::gts::StreamProfile;

pub const DEFAULT_RTO_MS: u64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelRole { Initiator, Responder }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState { Connecting, Established, Closing, Closed, Reset }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState { Opening, Open, Closing, Closed, Reset }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxMeta {
    pub stream_id: u8,
    pub sequence: u32,
    pub end: bool,
    pub sent_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RxDisposition { Pending, Delivered }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RxMeta {
    pub stream_id: u8,
    pub sequence: u32,
    pub end: bool,
    pub disposition: RxDisposition,
    pub order: u64,
}

#[derive(Debug, Clone, Copy)]
struct StreamCore {
    id: u8,
    profile: StreamProfile,
    state: StreamState,
    opened_by_local: bool,
    next_tx_sequence: u32,
    next_expected: u32,
    peer_credit: u8,
    receive_capacity: u8,
    seen_reliable: bool,
    unrel_rx_highest: Option<u32>,
    unrel_tx_sequence: u32,
    local_ended: bool,
    remote_ended: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct BoundedStreamSlot {
    stream: Option<StreamCore>,
}

impl BoundedStreamSlot {
    pub const EMPTY: Self = Self { stream: None };
    pub const fn is_empty(&self) -> bool { self.stream.is_none() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AckState {
    pub ack_base: u32,
    pub receive_bitmap: u32,
    pub receive_credit: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendInfo {
    pub sequence: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceivedMessage {
    pub len: usize,
    pub end: bool,
}

#[derive(Debug)]
pub struct Retransmit<'a> {
    pub stream_id: u8,
    pub sequence: u32,
    pub end: bool,
    pub data: &'a [u8],
}

/// Heapless GTS tunnel/socket state.
///
/// Every byte of retransmission and receive storage is supplied by the caller.
/// The implementation never allocates and is usable with `default-features =
/// false`. The byte arenas are divided into fixed maximum-message slots; this
/// matches GTS credit semantics, where one credit promises capacity for one
/// complete message up to the negotiated maximum class.
#[derive(Debug)]
pub struct BoundedGtsSocket<'a> {
    pub role: TunnelRole,
    pub state: TunnelState,
    pub local_receive_id: u32,
    pub remote_receive_id: Option<u32>,
    pub local_reset_id: u32,
    pub remote_reset_id: Option<u32>,
    pub css: ServiceSelector,
    streams: &'a mut [BoundedStreamSlot],
    tx: MessagePool<'a, TxMeta>,
    rx: MessagePool<'a, RxMeta>,
    next_stream_id: u8,
    next_delivery_order: u64,
    rto_ms: u64,
}

impl<'a> BoundedGtsSocket<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        role: TunnelRole,
        local_receive_id: u32,
        local_reset_id: u32,
        css: ServiceSelector,
        streams: &'a mut [BoundedStreamSlot],
        tx_slots: &'a mut [MessageSlot<TxMeta>],
        tx_bytes: &'a mut [u8],
        rx_slots: &'a mut [MessageSlot<RxMeta>],
        rx_bytes: &'a mut [u8],
    ) -> Result<Self> {
        if streams.is_empty() { return Err(Error::InvalidLength); }
        for slot in streams.iter_mut() { *slot = BoundedStreamSlot::EMPTY; }
        Ok(Self {
            role,
            state: if role == TunnelRole::Initiator { TunnelState::Connecting } else { TunnelState::Established },
            local_receive_id,
            remote_receive_id: None,
            local_reset_id,
            remote_reset_id: None,
            css,
            streams,
            tx: MessagePool::new(tx_slots, tx_bytes)?,
            rx: MessagePool::new(rx_slots, rx_bytes)?,
            next_stream_id: if role == TunnelRole::Initiator { 2 } else { 1 },
            next_delivery_order: 1,
            rto_ms: DEFAULT_RTO_MS,
        })
    }

    pub fn set_rto_ms(&mut self, value: u64) -> Result<()> {
        if value == 0 { return Err(Error::InvalidField); }
        self.rto_ms = value;
        Ok(())
    }

    pub fn establish(&mut self, remote_receive_id: u32, remote_reset_id: u32) -> Result<()> {
        if !matches!(self.state, TunnelState::Connecting | TunnelState::Established) {
            return Err(Error::InvalidState);
        }
        self.remote_receive_id = Some(remote_receive_id);
        self.remote_reset_id = Some(remote_reset_id);
        self.state = TunnelState::Established;
        Ok(())
    }

    fn stream_index(&self, id: u8) -> Result<usize> {
        self.streams.iter().position(|s| s.stream.map(|x| x.id) == Some(id)).ok_or(Error::UnknownStream)
    }

    fn stream(&self, id: u8) -> Result<&StreamCore> {
        self.streams[self.stream_index(id)?].stream.as_ref().ok_or(Error::UnknownStream)
    }

    fn stream_mut(&mut self, id: u8) -> Result<&mut StreamCore> {
        let idx = self.stream_index(id)?;
        self.streams[idx].stream.as_mut().ok_or(Error::UnknownStream)
    }

    pub fn stream_state(&self, id: u8) -> Result<StreamState> { Ok(self.stream(id)?.state) }
    pub fn peer_credit(&self, id: u8) -> Result<u8> { Ok(self.stream(id)?.peer_credit) }

    pub fn alloc_stream_id(&mut self) -> Result<u8> {
        if self.state != TunnelState::Established { return Err(Error::InvalidState); }
        let id = self.next_stream_id;
        if id > 253 { return Err(Error::BufferFull); }
        self.next_stream_id = self.next_stream_id.wrapping_add(2);
        Ok(id)
    }

    pub fn add_stream(
        &mut self,
        id: u8,
        profile: StreamProfile,
        opened_by_local: bool,
        receive_capacity: u8,
        peer_credit: u8,
    ) -> Result<()> {
        profile.validate()?;
        if self.streams.iter().any(|s| s.stream.map(|x| x.id) == Some(id)) {
            return Err(Error::InvalidState);
        }
        let slot = self.streams.iter_mut().find(|s| s.stream.is_none()).ok_or(Error::BufferFull)?;
        slot.stream = Some(StreamCore {
            id,
            profile,
            state: StreamState::Open,
            opened_by_local,
            next_tx_sequence: 0,
            next_expected: 0,
            peer_credit: if profile.unreliable { 0 } else { peer_credit },
            receive_capacity: if profile.unreliable { 0 } else { receive_capacity },
            seen_reliable: false,
            unrel_rx_highest: None,
            unrel_tx_sequence: 0,
            local_ended: false,
            remote_ended: false,
        });
        Ok(())
    }

    fn local_may_send(s: &StreamCore) -> bool {
        if s.opened_by_local { s.profile.direction.opener_may_send() } else { s.profile.direction.peer_may_send() }
    }

    fn remote_may_send(s: &StreamCore) -> bool {
        if s.opened_by_local { s.profile.direction.peer_may_send() } else { s.profile.direction.opener_may_send() }
    }

    fn fixed_reliable_data_len(profile: StreamProfile) -> Result<usize> {
        profile.size_class.bytes().checked_sub(14).ok_or(Error::InvalidLength)
    }

    fn fixed_reliable_end_capacity(profile: StreamProfile) -> Result<usize> {
        profile.size_class.bytes().checked_sub(16).ok_or(Error::InvalidLength)
    }

    fn fixed_unreliable_data_len(profile: StreamProfile) -> Result<usize> {
        let overhead = if profile.sequenced { 14 } else { 10 };
        profile.size_class.bytes().checked_sub(overhead).ok_or(Error::InvalidLength)
    }

    fn validate_send_len(profile: StreamProfile, data_len: usize, end: bool) -> Result<()> {
        if profile.unreliable {
            if end { return Err(Error::ProfileViolation); }
            if profile.variable {
                let overhead = if profile.sequenced { 16 } else { 12 };
                if data_len + overhead > profile.size_class.bytes() { return Err(Error::MessageTooLarge); }
            } else if data_len != Self::fixed_unreliable_data_len(profile)? {
                return Err(Error::InvalidLength);
            }
        } else if profile.variable {
            if data_len + 16 > profile.size_class.bytes() { return Err(Error::MessageTooLarge); }
        } else if end {
            if data_len > Self::fixed_reliable_end_capacity(profile)? { return Err(Error::MessageTooLarge); }
        } else if data_len != Self::fixed_reliable_data_len(profile)? {
            return Err(Error::InvalidLength);
        }
        Ok(())
    }

    pub fn send(&mut self, stream_id: u8, data: &[u8], end: bool, now: u64) -> Result<SendInfo> {
        let idx = self.stream_index(stream_id)?;
        let snapshot = self.streams[idx].stream.ok_or(Error::UnknownStream)?;
        if snapshot.state != StreamState::Open || snapshot.local_ended { return Err(Error::InvalidState); }
        if !Self::local_may_send(&snapshot) { return Err(Error::DirectionViolation); }
        Self::validate_send_len(snapshot.profile, data.len(), end)?;

        if snapshot.profile.unreliable {
            let sequence = if snapshot.profile.sequenced {
                let seq = snapshot.unrel_tx_sequence;
                self.streams[idx].stream.as_mut().unwrap().unrel_tx_sequence = seq.wrapping_add(1);
                Some(seq)
            } else { None };
            return Ok(SendInfo { sequence });
        }

        if snapshot.peer_credit == 0 { return Err(Error::WouldBlock); }
        let sequence = snapshot.next_tx_sequence;
        self.tx.insert(TxMeta { stream_id, sequence, end, sent_at: now }, data)?;
        let stream = self.streams[idx].stream.as_mut().unwrap();
        stream.next_tx_sequence = sequence.wrapping_add(1);
        stream.peer_credit -= 1;
        if end { stream.local_ended = true; }
        Ok(SendInfo { sequence: Some(sequence) })
    }

    fn tx_acked(sequence: u32, base: u32, bitmap: u32) -> bool {
        if sequence <= base { return true; }
        let d = sequence.wrapping_sub(base).wrapping_sub(1);
        d < 32 && (bitmap & (1u32 << d)) != 0
    }

    pub fn on_ack(&mut self, stream_id: u8, ack: AckState) -> Result<()> {
        let stream = self.stream_mut(stream_id)?;
        if stream.profile.unreliable || stream.state == StreamState::Reset { return Err(Error::ProfileViolation); }
        stream.peer_credit = ack.receive_credit;
        for idx in 0..self.tx.capacity() {
            if let Some(meta) = self.tx.meta(idx) {
                if meta.stream_id == stream_id && Self::tx_acked(meta.sequence, ack.ack_base, ack.receive_bitmap) {
                    let _ = self.tx.remove(idx)?;
                }
            }
        }
        Ok(())
    }

    pub fn retransmit_due(&mut self, now: u64) -> Result<Option<Retransmit<'_>>> {
        let mut selected = None;
        for idx in 0..self.tx.capacity() {
            if let Some(meta) = self.tx.meta(idx) {
                if now.saturating_sub(meta.sent_at) >= self.rto_ms {
                    selected = Some((idx, meta));
                    break;
                }
            }
        }
        let Some((idx, mut meta)) = selected else { return Ok(None); };
        meta.sent_at = now;
        self.tx.set_meta(idx, meta)?;
        let (_, data) = self.tx.get(idx).ok_or(Error::InvalidState)?;
        Ok(Some(Retransmit { stream_id: meta.stream_id, sequence: meta.sequence, end: meta.end, data }))
    }

    fn rx_count_for(&self, stream_id: u8) -> usize {
        let mut count = 0;
        self.rx.for_each(|_, meta| { if meta.stream_id == stream_id { count += 1; } });
        count
    }

    pub fn ack_state(&self, stream_id: u8) -> Result<Option<AckState>> {
        let stream = self.stream(stream_id)?;
        if stream.profile.unreliable || !stream.seen_reliable { return Ok(None); }
        let base = stream.next_expected.wrapping_sub(1);
        let mut bitmap = 0u32;
        self.rx.for_each(|_, meta| {
            if meta.stream_id == stream_id && meta.disposition == RxDisposition::Pending {
                let d = meta.sequence.wrapping_sub(base).wrapping_sub(1);
                if d < 32 { bitmap |= 1u32 << d; }
            }
        });
        let local_free = (stream.receive_capacity as usize).saturating_sub(self.rx_count_for(stream_id));
        let credit = local_free.min(self.rx.free()).min(255) as u8;
        Ok(Some(AckState { ack_base: base, receive_bitmap: bitmap, receive_credit: credit }))
    }

    fn mark_contiguous_delivered(&mut self, stream_id: u8) -> Result<()> {
        loop {
            let expected = self.stream(stream_id)?.next_expected;
            let found = self.rx.find(|meta| {
                meta.stream_id == stream_id
                    && meta.sequence == expected
                    && meta.disposition == RxDisposition::Pending
            });
            let Some(idx) = found else { break; };
            let mut meta = self.rx.meta(idx).ok_or(Error::InvalidState)?;
            meta.disposition = RxDisposition::Delivered;
            meta.order = self.next_delivery_order;
            self.next_delivery_order = self.next_delivery_order.wrapping_add(1).max(1);
            self.rx.set_meta(idx, meta)?;
            let stream = self.stream_mut(stream_id)?;
            stream.next_expected = stream.next_expected.wrapping_add(1);
            if meta.end { stream.remote_ended = true; }
        }
        Ok(())
    }

    pub fn receive(
        &mut self,
        stream_id: u8,
        sequence: Option<u32>,
        data: &[u8],
        end: bool,
    ) -> Result<Option<AckState>> {
        let idx = self.stream_index(stream_id)?;
        let snapshot = self.streams[idx].stream.ok_or(Error::UnknownStream)?;
        if matches!(snapshot.state, StreamState::Closed | StreamState::Reset) || snapshot.remote_ended {
            return Err(Error::InvalidState);
        }
        if !Self::remote_may_send(&snapshot) { return Err(Error::DirectionViolation); }
        Self::validate_send_len(snapshot.profile, data.len(), end)?;

        if snapshot.profile.unreliable {
            let seq = if snapshot.profile.sequenced {
                sequence.ok_or(Error::ProfileViolation)?
            } else {
                if sequence.is_some() { return Err(Error::ProfileViolation); }
                0
            };
            if snapshot.profile.sequenced {
                if let Some(highest) = snapshot.unrel_rx_highest {
                    if seq <= highest { return Ok(None); }
                }
                self.streams[idx].stream.as_mut().unwrap().unrel_rx_highest = Some(seq);
            }
            let order = self.next_delivery_order;
            self.next_delivery_order = self.next_delivery_order.wrapping_add(1).max(1);
            self.rx.insert(RxMeta { stream_id, sequence: seq, end: false, disposition: RxDisposition::Delivered, order }, data)?;
            return Ok(None);
        }

        let seq = sequence.ok_or(Error::ProfileViolation)?;
        if seq < snapshot.next_expected { return self.ack_state(stream_id); }
        let distance = seq.wrapping_sub(snapshot.next_expected);
        if distance > 32 { return Err(Error::BufferFull); }
        if self.rx_count_for(stream_id) >= snapshot.receive_capacity as usize { return Err(Error::BufferFull); }
        if self.rx.find(|m| m.stream_id == stream_id && m.sequence == seq).is_some() {
            return self.ack_state(stream_id);
        }

        let disposition = if seq == snapshot.next_expected { RxDisposition::Delivered } else { RxDisposition::Pending };
        let order = if disposition == RxDisposition::Delivered {
            let order = self.next_delivery_order;
            self.next_delivery_order = self.next_delivery_order.wrapping_add(1).max(1);
            order
        } else { 0 };
        self.rx.insert(RxMeta { stream_id, sequence: seq, end, disposition, order }, data)?;
        {
            let stream = self.streams[idx].stream.as_mut().unwrap();
            stream.seen_reliable = true;
            if disposition == RxDisposition::Delivered {
                stream.next_expected = stream.next_expected.wrapping_add(1);
                if end { stream.remote_ended = true; }
            }
        }
        self.mark_contiguous_delivered(stream_id)?;
        self.ack_state(stream_id)
    }

    pub fn recv_into(&mut self, stream_id: u8, out: &mut [u8]) -> Result<Option<ReceivedMessage>> {
        self.stream(stream_id)?;
        let mut best: Option<(usize, RxMeta)> = None;
        for idx in 0..self.rx.capacity() {
            if let Some(meta) = self.rx.meta(idx) {
                if meta.stream_id == stream_id && meta.disposition == RxDisposition::Delivered {
                    if best.map(|(_, b)| meta.order < b.order).unwrap_or(true) { best = Some((idx, meta)); }
                }
            }
        }
        let Some((idx, meta)) = best else { return Ok(None); };
        let (_, len) = self.rx.remove_into(idx, out)?;
        Ok(Some(ReceivedMessage { len, end: meta.end }))
    }

    pub fn reset_stream(&mut self, stream_id: u8) -> Result<()> {
        let idx = self.stream_index(stream_id)?;
        for slot in 0..self.tx.capacity() {
            if self.tx.meta(slot).map(|m| m.stream_id == stream_id).unwrap_or(false) { let _ = self.tx.remove(slot)?; }
        }
        for slot in 0..self.rx.capacity() {
            if self.rx.meta(slot).map(|m| m.stream_id == stream_id).unwrap_or(false) { let _ = self.rx.remove(slot)?; }
        }
        let s = self.streams[idx].stream.as_mut().unwrap();
        s.state = StreamState::Reset;
        s.local_ended = true;
        s.remote_ended = true;
        s.peer_credit = 0;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.state = TunnelState::Reset;
        self.tx.clear();
        self.rx.clear();
        for slot in self.streams.iter_mut() {
            if let Some(stream) = slot.stream.as_mut() {
                stream.state = StreamState::Reset;
                stream.local_ended = true;
                stream.remote_ended = true;
                stream.peer_credit = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MessageSlot;
    use crate::wire::gdp::SizeClass;
    use crate::wire::gts::{Direction, StreamProfile};

    fn socket<'a>(
        streams: &'a mut [BoundedStreamSlot],
        tx_slots: &'a mut [MessageSlot<TxMeta>],
        tx_bytes: &'a mut [u8],
        rx_slots: &'a mut [MessageSlot<RxMeta>],
        rx_bytes: &'a mut [u8],
    ) -> BoundedGtsSocket<'a> {
        BoundedGtsSocket::new(
            TunnelRole::Initiator,
            1,
            2,
            ServiceSelector([1; 16]),
            streams,
            tx_slots,
            tx_bytes,
            rx_slots,
            rx_bytes,
        ).unwrap()
    }

    #[test]
    fn reliable_window_reorders_acks_and_reuses_credit_without_alloc() {
        let mut streams = [BoundedStreamSlot::EMPTY; 2];
        let mut tx_slots = [MessageSlot::<TxMeta>::EMPTY; 4];
        let mut tx_bytes = [0u8; 512];
        let mut rx_slots = [MessageSlot::<RxMeta>::EMPTY; 4];
        let mut rx_bytes = [0u8; 512];
        let mut s = socket(&mut streams, &mut tx_slots, &mut tx_bytes, &mut rx_slots, &mut rx_bytes);
        s.establish(10, 11).unwrap();
        let p = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
        s.add_stream(0, p, true, 4, 4).unwrap();

        let a = s.receive(0, Some(1), b"second", false).unwrap().unwrap();
        assert_eq!(a.ack_base, u32::MAX);
        assert_eq!(a.receive_bitmap & 0b10, 0b10);
        let b = s.receive(0, Some(0), b"first", false).unwrap().unwrap();
        assert_eq!(b.ack_base, 1);

        let mut out = [0u8; 32];
        let m1 = s.recv_into(0, &mut out).unwrap().unwrap();
        assert_eq!(&out[..m1.len], b"first");
        let m2 = s.recv_into(0, &mut out).unwrap().unwrap();
        assert_eq!(&out[..m2.len], b"second");
        assert_eq!(s.ack_state(0).unwrap().unwrap().receive_credit, 4);
    }

    #[test]
    fn reliable_tx_retransmits_and_ack_frees_caller_storage() {
        let mut streams = [BoundedStreamSlot::EMPTY; 2];
        let mut tx_slots = [MessageSlot::<TxMeta>::EMPTY; 2];
        let mut tx_bytes = [0u8; 256];
        let mut rx_slots = [MessageSlot::<RxMeta>::EMPTY; 2];
        let mut rx_bytes = [0u8; 256];
        let mut s = socket(&mut streams, &mut tx_slots, &mut tx_bytes, &mut rx_slots, &mut rx_bytes);
        s.establish(10, 11).unwrap();
        let p = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
        s.add_stream(0, p, true, 2, 2).unwrap();
        assert_eq!(s.send(0, b"hello", false, 0).unwrap().sequence, Some(0));
        let r = s.retransmit_due(500).unwrap().unwrap();
        assert_eq!(r.sequence, 0);
        assert_eq!(r.data, b"hello");
        s.on_ack(0, AckState { ack_base: 0, receive_bitmap: 0, receive_credit: 2 }).unwrap();
        assert!(s.retransmit_due(1000).unwrap().is_none());
    }

    #[test]
    fn reset_discards_all_bounded_state() {
        let mut streams = [BoundedStreamSlot::EMPTY; 2];
        let mut tx_slots = [MessageSlot::<TxMeta>::EMPTY; 2];
        let mut tx_bytes = [0u8; 256];
        let mut rx_slots = [MessageSlot::<RxMeta>::EMPTY; 2];
        let mut rx_bytes = [0u8; 256];
        let mut s = socket(&mut streams, &mut tx_slots, &mut tx_bytes, &mut rx_slots, &mut rx_bytes);
        s.establish(10, 11).unwrap();
        let p = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
        s.add_stream(0, p, true, 2, 2).unwrap();
        s.send(0, b"hello", false, 0).unwrap();
        s.receive(0, Some(0), b"world", false).unwrap();
        s.reset();
        assert_eq!(s.state, TunnelState::Reset);
        assert_eq!(s.stream_state(0).unwrap(), StreamState::Reset);
        assert!(s.retransmit_due(1000).unwrap().is_none());
    }
}
