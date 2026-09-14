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

    control_tx_queue: VecDeque<Flit>,
    data_tx_queue: VecDeque<Flit>,
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

    /// Records that a GCTL CREDIT message advertising `count` flits has been
    /// queued. The advertised window cannot exceed actual free receive space.
    pub fn note_data_credit_granted(&mut self, count: u32) -> Result<()> {
        if count > self.grantable_data_credit() {
            return Err(Error::BufferFull);
        }
        self.data_credit_outstanding = self.data_credit_outstanding.saturating_add(count);
        Ok(())
    }

    /// Adds receive capacity learned from the peer via GCTL CREDIT.
    pub fn grant_data_tx_credit(&mut self, count: u32) {
        self.data_tx_credit = self.data_tx_credit.saturating_add(count);
    }

    /// The control lane has a statically provisioned sliding window. Direct
    /// point-to-point link glue uses this to seed/refund the window; GCTL data
    /// CREDIT messages do not modify it.
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

    pub fn queued_flits(&self) -> usize {
        self.control_tx_queue.len() + self.data_tx_queue.len()
    }

    pub fn queued_data_flits(&self) -> usize {
        self.data_tx_queue.len()
    }

    pub fn queued_control_flits(&self) -> usize {
        self.control_tx_queue.len()
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

    /// Low-level explicit-VC entry point used by DLP tests and later router
    /// code. Endpoint/GDP/GTS code should use queue_control_packet or
    /// queue_data_packet instead.
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
        let queue = match traffic {
            LinkTraffic::Control => &mut self.control_tx_queue,
            LinkTraffic::Data => &mut self.data_tx_queue,
        };
        for i in 0..n {
            let start = i * 4;
            let mut b = [0u8; 4];
            let end = (start + 4).min(bytes.len());
            b[..end - start].copy_from_slice(&bytes[start..end]);
            queue.push_back(Flit {
                vcid,
                data: u32::from_be_bytes(b),
            });
        }
        Ok(n)
    }

    /// Fill `out` with as many currently sendable flits as possible.
    ///
    /// Control traffic retains priority, but a blocked control queue does not
    /// prevent data traffic with available data credit from filling the rest
    /// of the burst. Credits are consumed in one accounting operation per
    /// queue rather than one public API crossing per physical flit.
    pub fn poll_tx_burst(&mut self, out: &mut [Flit]) -> Result<usize> {
        if !self.link_up {
            return Err(Error::LinkDown);
        }
        if out.is_empty() {
            return Ok(0);
        }

        let mut written = 0usize;

        let control_n = out
            .len()
            .min(self.control_tx_queue.len())
            .min(self.control_tx_credit as usize);
        for slot in &mut out[..control_n] {
            *slot = self
                .control_tx_queue
                .pop_front()
                .expect("control burst count matched queue length");
        }
        self.control_tx_credit -= control_n as u32;
        written += control_n;

        let data_room = out.len() - written;
        let data_n = data_room
            .min(self.data_tx_queue.len())
            .min(self.data_tx_credit as usize);
        for slot in &mut out[written..written + data_n] {
            *slot = self
                .data_tx_queue
                .pop_front()
                .expect("data burst count matched queue length");
        }
        self.data_tx_credit -= data_n as u32;
        written += data_n;

        if written != 0 {
            return Ok(written);
        }
        if self.control_tx_queue.is_empty() && self.data_tx_queue.is_empty() {
            Ok(0)
        } else {
            Err(Error::NoCredit)
        }
    }

    /// Compatibility one-flit API. Native devices should prefer
    /// `poll_tx_burst` so a NIC/driver boundary can transfer many flits at once.
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

    /// Process a slice of physical flits and append every completed GDP packet
    /// to `packets`. This keeps the device-facing API burst-oriented while the
    /// current GDP reassembly logic remains deliberately simple.
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
