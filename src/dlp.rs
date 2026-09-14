use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::error::{Error, Result};
use crate::wire::gdp::{AddressForm, GdpPacket, GdpWireConfig, SizeClass};

/// Number of virtual channels exposed by the physical/link profile.
///
/// Higher protocol layers never select a numeric VCID. VC 0 is reserved by
/// DLP for link/control traffic; data packets are distributed over the
/// remaining channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcMode {
    Two,
    Four,
}

impl VcMode {
    pub const fn count(self) -> u8 {
        match self {
            Self::Two => 2,
            Self::Four => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Vcid(u8);

impl Vcid {
    pub const CONTROL: Self = Self(0);
    pub const VC1: Self = Self(1);
    pub const VC2: Self = Self(2);
    pub const VC3: Self = Self(3);

    pub fn new(v: u8) -> Result<Self> {
        if v < 4 {
            Ok(Self(v))
        } else {
            Err(Error::InvalidField)
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub const fn is_control(self) -> bool {
        self.0 == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flit {
    pub vcid: Vcid,
    pub data: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkTraffic {
    Control,
    Data,
}

/// A complete DLP/GNET frame at the QDX-GNET-style host/device boundary.
///
/// The bytes are the actual encoded GDP frame. `vcid` and `traffic` are
/// link-local metadata used by the software model; GDP/GCTL/GTS do not see or
/// select them. A real QDX-GNET controller can keep the equivalent state in
/// hardware while DMA moves the contiguous frame buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GnetFrame {
    pub vcid: Vcid,
    pub traffic: LinkTraffic,
    pub bytes: Vec<u8>,
}

impl GnetFrame {
    pub fn flit_len(&self) -> usize {
        (self.bytes.len() + 3) / 4
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DlpConfig {
    pub gdp: GdpWireConfig,
    pub local_prefix: u64,
    pub vc_mode: VcMode,
    /// Receive storage available to ordinary data flits.
    pub rx_buffer_flits: u32,
    /// Statically provisioned control receive window. This is kept separate
    /// from the dynamically advertised data receive buffer so CREDIT traffic
    /// cannot deadlock waiting for the credit it is trying to advertise.
    pub control_window_flits: u32,
}

impl DlpConfig {
    pub fn new(rx_buffer_flits: u32, vc_mode: VcMode) -> Result<Self> {
        if rx_buffer_flits == 0 {
            return Err(Error::InvalidField);
        }
        Ok(Self {
            gdp: GdpWireConfig::default(),
            local_prefix: 0,
            vc_mode,
            rx_buffer_flits,
            control_window_flits: 32,
        })
    }
}

#[derive(Debug, Clone)]
struct TxFrame {
    vcid: Vcid,
    traffic: LinkTraffic,
    bytes: Vec<u8>,
    /// Raw-flit backends advance this cursor. The normal QDX frame path keeps
    /// it at zero and moves the complete buffer in one operation.
    flit_offset: usize,
}

impl TxFrame {
    fn flit_len(&self) -> usize {
        (self.bytes.len() + 3) / 4
    }

    fn remaining_flits(&self) -> usize {
        self.flit_len().saturating_sub(self.flit_offset)
    }

    fn next_flit(&mut self) -> Option<Flit> {
        if self.flit_offset >= self.flit_len() {
            return None;
        }
        let start = self.flit_offset * 4;
        let end = (start + 4).min(self.bytes.len());
        let mut word = [0u8; 4];
        word[..end - start].copy_from_slice(&self.bytes[start..end]);
        self.flit_offset += 1;
        Some(Flit {
            vcid: self.vcid,
            data: u32::from_be_bytes(word),
        })
    }

    fn into_frame(self) -> Result<GnetFrame> {
        if self.flit_offset != 0 {
            return Err(Error::InvalidState);
        }
        Ok(GnetFrame {
            vcid: self.vcid,
            traffic: self.traffic,
            bytes: self.bytes,
        })
    }
}

#[derive(Debug, Clone)]
struct RxSegment {
    bytes: Vec<u8>,
    expected_bytes: usize,
    expected_flits: usize,
    received_flits: usize,
    traffic: LinkTraffic,
}

#[derive(Debug, Clone)]
pub struct DlpEndpoint {
    cfg: GdpWireConfig,
    local_prefix: u64,
    vc_mode: VcMode,
    rx_buffer_flits: u32,
    control_window_flits: u32,

    data_tx_credit: u32,
    control_tx_credit: u32,
    data_credit_outstanding: u32,
    data_rx_in_use: u32,

    control_tx_queue: VecDeque<TxFrame>,
    data_tx_queue: VecDeque<TxFrame>,
    rx: [Option<RxSegment>; 4],
    desynchronized: [bool; 4],
    next_data_vc: u8,
    link_up: bool,
}

impl DlpEndpoint {
    pub fn new(config: DlpConfig) -> Result<Self> {
        if config.rx_buffer_flits == 0 || config.control_window_flits == 0 {
            return Err(Error::InvalidField);
        }
        Ok(Self {
            cfg: config.gdp,
            local_prefix: config.local_prefix & !0xffff,
            vc_mode: config.vc_mode,
            rx_buffer_flits: config.rx_buffer_flits,
            control_window_flits: config.control_window_flits,
            data_tx_credit: 0,
            control_tx_credit: 0,
            data_credit_outstanding: 0,
            data_rx_in_use: 0,
            control_tx_queue: VecDeque::new(),
            data_tx_queue: VecDeque::new(),
            rx: [None, None, None, None],
            desynchronized: [false; 4],
            next_data_vc: 1,
            link_up: true,
        })
    }

    pub const fn vc_mode(&self) -> VcMode {
        self.vc_mode
    }

    pub const fn vc_count(&self) -> u8 {
        self.vc_mode.count()
    }

    pub const fn rx_buffer_flits(&self) -> u32 {
        self.rx_buffer_flits
    }

    pub const fn rx_buffer_in_use(&self) -> u32 {
        self.data_rx_in_use
    }

    pub const fn control_window_flits(&self) -> u32 {
        self.control_window_flits
    }

    pub const fn data_tx_credit(&self) -> u32 {
        self.data_tx_credit
    }

    pub const fn control_tx_credit(&self) -> u32 {
        self.control_tx_credit
    }

    pub const fn data_credit_outstanding(&self) -> u32 {
        self.data_credit_outstanding
    }

    pub fn available_rx_flits(&self) -> u32 {
        self.rx_buffer_flits.saturating_sub(self.data_rx_in_use)
    }

    /// Capacity that may still safely be advertised to the peer.
    pub fn grantable_data_credit(&self) -> u32 {
        self.available_rx_flits()
            .saturating_sub(self.data_credit_outstanding)
    }

    pub fn note_data_credit_granted(&mut self, count: u32) -> Result<()> {
        if count > self.grantable_data_credit() {
            return Err(Error::BufferFull);
        }
        self.data_credit_outstanding = self.data_credit_outstanding.saturating_add(count);
        Ok(())
    }

    pub fn grant_data_tx_credit(&mut self, count: u32) {
        self.data_tx_credit = self.data_tx_credit.saturating_add(count);
    }

    pub fn grant_control_tx_credit(&mut self, count: u32) {
        self.control_tx_credit = self.control_tx_credit.saturating_add(count);
    }

    pub fn set_local_prefix(&mut self, prefix: u64) {
        self.local_prefix = prefix & !0xffff;
    }

    pub fn set_link_up(&mut self, up: bool) {
        self.link_up = up;
        if !up {
            self.control_tx_queue.clear();
            self.data_tx_queue.clear();
            self.rx = [None, None, None, None];
            self.desynchronized = [false; 4];
            self.data_tx_credit = 0;
            self.control_tx_credit = 0;
            self.data_credit_outstanding = 0;
            self.data_rx_in_use = 0;
        }
    }

    fn queue_flits(queue: &VecDeque<TxFrame>) -> usize {
        queue.iter().map(TxFrame::remaining_flits).sum()
    }

    pub fn queued_flits(&self) -> usize {
        Self::queue_flits(&self.control_tx_queue) + Self::queue_flits(&self.data_tx_queue)
    }

    pub fn queued_data_flits(&self) -> usize {
        Self::queue_flits(&self.data_tx_queue)
    }

    pub fn queued_control_flits(&self) -> usize {
        Self::queue_flits(&self.control_tx_queue)
    }

    fn choose_data_vc(&mut self) -> Vcid {
        let count = self.vc_mode.count();
        let vc = self.next_data_vc;
        self.next_data_vc += 1;
        if self.next_data_vc >= count {
            self.next_data_vc = 1;
        }
        Vcid(vc)
    }

    pub fn queue_control_packet(&mut self, packet: &GdpPacket) -> Result<usize> {
        self.queue_packet_on(LinkTraffic::Control, Vcid::CONTROL, packet)
    }

    pub fn queue_data_packet(&mut self, packet: &GdpPacket) -> Result<usize> {
        let vc = self.choose_data_vc();
        self.queue_packet_on(LinkTraffic::Data, vc, packet)
    }

    /// Queue one encoded GDP frame without pre-materializing physical flits.
    pub fn queue_packet_on(
        &mut self,
        traffic: LinkTraffic,
        vcid: Vcid,
        packet: &GdpPacket,
    ) -> Result<usize> {
        if !self.link_up {
            return Err(Error::LinkDown);
        }
        if vcid.get() >= self.vc_mode.count() {
            return Err(Error::InvalidField);
        }
        if traffic == LinkTraffic::Control && !vcid.is_control() {
            return Err(Error::InvalidField);
        }
        if traffic == LinkTraffic::Data && vcid.is_control() {
            return Err(Error::InvalidField);
        }

        let bytes = packet.encode(self.cfg)?;
        let n = (bytes.len() + 3) / 4;
        let frame = TxFrame {
            vcid,
            traffic,
            bytes,
            flit_offset: 0,
        };
        match traffic {
            LinkTraffic::Control => self.control_tx_queue.push_back(frame),
            LinkTraffic::Data => self.data_tx_queue.push_back(frame),
        }
        Ok(n)
    }

    fn pop_sendable_frame(
        queue: &mut VecDeque<TxFrame>,
        credit: &mut u32,
    ) -> Result<Option<GnetFrame>> {
        let Some(front) = queue.front() else {
            return Ok(None);
        };
        if front.flit_offset != 0 {
            return Err(Error::InvalidState);
        }
        let needed = front.flit_len();
        if needed > *credit as usize {
            return Ok(None);
        }
        *credit -= needed as u32;
        queue
            .pop_front()
            .expect("front frame existed")
            .into_frame()
            .map(Some)
    }

    /// QDX-GNET-style whole-frame transmit path.
    ///
    /// This is the normal software/device boundary. It preserves DLP credit
    /// accounting but does not create one Rust object/API transition per flit.
    pub fn poll_tx_frame(&mut self) -> Result<Option<GnetFrame>> {
        if !self.link_up {
            return Err(Error::LinkDown);
        }

        if let Some(frame) = Self::pop_sendable_frame(
            &mut self.control_tx_queue,
            &mut self.control_tx_credit,
        )? {
            return Ok(Some(frame));
        }
        if let Some(frame) =
            Self::pop_sendable_frame(&mut self.data_tx_queue, &mut self.data_tx_credit)?
        {
            return Ok(Some(frame));
        }

        if self.control_tx_queue.is_empty() && self.data_tx_queue.is_empty() {
            Ok(None)
        } else {
            Err(Error::NoCredit)
        }
    }

    fn emit_queue_flits(
        queue: &mut VecDeque<TxFrame>,
        credit: &mut u32,
        out: &mut [Flit],
    ) -> usize {
        let mut written = 0usize;
        while written < out.len() && *credit != 0 {
            let Some(front) = queue.front_mut() else {
                break;
            };
            let Some(flit) = front.next_flit() else {
                queue.pop_front();
                continue;
            };
            out[written] = flit;
            written += 1;
            *credit -= 1;
            if front.remaining_flits() == 0 {
                queue.pop_front();
            }
        }
        written
    }

    /// Raw-flit backend API. Flits are materialized lazily from contiguous
    /// queued frames only when a raw physical/simulation backend asks for them.
    pub fn poll_tx_burst(&mut self, out: &mut [Flit]) -> Result<usize> {
        if !self.link_up {
            return Err(Error::LinkDown);
        }
        if out.is_empty() {
            return Ok(0);
        }

        let control_n = Self::emit_queue_flits(
            &mut self.control_tx_queue,
            &mut self.control_tx_credit,
            out,
        );
        let data_n = Self::emit_queue_flits(
            &mut self.data_tx_queue,
            &mut self.data_tx_credit,
            &mut out[control_n..],
        );
        let written = control_n + data_n;

        if written != 0 {
            return Ok(written);
        }
        if self.control_tx_queue.is_empty() && self.data_tx_queue.is_empty() {
            Ok(0)
        } else {
            Err(Error::NoCredit)
        }
    }

    pub fn poll_tx(&mut self) -> Result<Option<Flit>> {
        let mut one = [Flit {
            vcid: Vcid::CONTROL,
            data: 0,
        }];
        match self.poll_tx_burst(&mut one)? {
            0 => Ok(None),
            1 => Ok(Some(one[0])),
            _ => unreachable!(),
        }
    }

    fn release_data_segment(&mut self, flits: usize) {
        self.data_rx_in_use = self.data_rx_in_use.saturating_sub(flits as u32);
    }

    /// QDX-GNET-style whole-frame receive path.
    pub fn receive_frame(&mut self, frame: GnetFrame) -> Result<GdpPacket> {
        if !self.link_up {
            return Err(Error::LinkDown);
        }
        if frame.vcid.get() >= self.vc_mode.count() {
            return Err(Error::InvalidField);
        }
        if frame.traffic == LinkTraffic::Control && !frame.vcid.is_control() {
            return Err(Error::InvalidField);
        }
        if frame.traffic == LinkTraffic::Data && frame.vcid.is_control() {
            return Err(Error::InvalidField);
        }

        let flits = frame.flit_len();
        if frame.traffic == LinkTraffic::Data {
            if self.data_credit_outstanding < flits as u32 {
                return Err(Error::CreditViolation);
            }
            if self.available_rx_flits() < flits as u32 {
                return Err(Error::BufferFull);
            }
            self.data_credit_outstanding -= flits as u32;
            self.data_rx_in_use += flits as u32;
        }

        let decoded = GdpPacket::decode(&frame.bytes, self.cfg, self.local_prefix);
        if frame.traffic == LinkTraffic::Data {
            self.release_data_segment(flits);
        }
        decoded
    }

    /// Raw physical-flit receive path.
    pub fn receive(&mut self, flit: Flit) -> Result<Option<GdpPacket>> {
        if !self.link_up {
            return Err(Error::LinkDown);
        }
        let idx = flit.vcid.get() as usize;
        if idx >= self.vc_mode.count() as usize {
            return Err(Error::InvalidField);
        }
        if self.desynchronized[idx] {
            return Err(Error::Desynchronized);
        }

        let traffic = if flit.vcid.is_control() {
            LinkTraffic::Control
        } else {
            LinkTraffic::Data
        };

        if traffic == LinkTraffic::Data {
            if self.data_credit_outstanding == 0 {
                return Err(Error::CreditViolation);
            }
            if self.data_rx_in_use >= self.rx_buffer_flits {
                return Err(Error::BufferFull);
            }
            self.data_credit_outstanding -= 1;
            self.data_rx_in_use += 1;
        }

        if self.rx[idx].is_none() {
            let w = flit.data;
            let size = match SizeClass::from_wire(((w >> 22) & 0xf) as u8) {
                Ok(v) => v,
                Err(e) => {
                    if traffic == LinkTraffic::Data {
                        self.release_data_segment(1);
                    }
                    self.desynchronized[idx] = true;
                    return Err(e);
                }
            };
            let form = self.cfg.form_from_bit(((w >> 21) & 1) != 0);
            let header_len = match form {
                AddressForm::Global => 20,
                AddressForm::Local => 8,
            };
            let total = header_len + size.bytes();
            self.rx[idx] = Some(RxSegment {
                bytes: Vec::with_capacity(((total + 3) / 4) * 4),
                expected_bytes: total,
                expected_flits: (total + 3) / 4,
                received_flits: 0,
                traffic,
            });
        }

        let s = self.rx[idx].as_mut().expect("segment exists");
        if s.traffic != traffic {
            return Err(Error::InvalidState);
        }
        s.bytes.extend_from_slice(&flit.data.to_be_bytes());
        s.received_flits += 1;
        if s.received_flits < s.expected_flits {
            return Ok(None);
        }

        let mut done = self.rx[idx].take().expect("completed segment exists");
        done.bytes.truncate(done.expected_bytes);
        if done.traffic == LinkTraffic::Data {
            self.release_data_segment(done.received_flits);
        }

        match GdpPacket::decode(&done.bytes, self.cfg, self.local_prefix) {
            Ok(packet) => Ok(Some(packet)),
            Err(e) => {
                self.desynchronized[idx] = true;
                Err(e)
            }
        }
    }

    pub fn receive_burst(
        &mut self,
        flits: &[Flit],
        packets: &mut Vec<GdpPacket>,
    ) -> Result<usize> {
        let before = packets.len();
        for &flit in flits {
            if let Some(packet) = self.receive(flit)? {
                packets.push(packet);
            }
        }
        Ok(packets.len() - before)
    }

    pub fn reset_vc(&mut self, vcid: Vcid) -> Result<()> {
        if vcid.get() >= self.vc_mode.count() {
            return Err(Error::InvalidField);
        }
        let i = vcid.get() as usize;
        if let Some(segment) = self.rx[i].take() {
            if segment.traffic == LinkTraffic::Data {
                self.release_data_segment(segment.received_flits);
            }
        }
        self.desynchronized[i] = false;
        Ok(())
    }
}
