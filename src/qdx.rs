use alloc::collections::VecDeque;
use alloc::rc::Rc;
use core::cell::RefCell;

use crate::endpoint::{DirectLink, Endpoint};
use crate::error::{Error, Result};
use crate::link::{Flit, GnetFrame, LinkTraffic};

/// CPU-facing device level used by QDX-GNET-style interfaces.
///
/// QDX-GNET RECEIVE/TRANSMIT operate on complete GNET frames in host memory.
/// The controller is responsible for the lower physical-flit mechanics.
pub trait GnetFrameDevice {
    fn transmit_frame(&mut self, frame: GnetFrame) -> Result<()>;
    fn receive_frame(&mut self) -> Result<Option<GnetFrame>>;
}

/// Lower-level device level for simple/raw hardware and link simulations.
/// Normal QDX-GNET endpoint traffic should prefer `GnetFrameDevice`.
pub trait GnetFlitDevice {
    fn transmit_flits(&mut self, flits: &[Flit]) -> Result<usize>;
    fn receive_flits(&mut self, out: &mut [Flit]) -> Result<usize>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VirtualNicSide {
    A,
    B,
}

#[derive(Debug)]
struct VirtualNicShared {
    a_to_b: VecDeque<GnetFrame>,
    b_to_a: VecDeque<GnetFrame>,
    capacity_frames: usize,
    link_up: bool,
}

/// A pure user-space virtual GNet NIC.
///
/// A pair carries native `GnetFrame` values directly. There is no Ethernet,
/// IP, TUN/TAP, MAC addressing, or host network stack involved. The queues are
/// only a software stand-in for two directly connected GNet NICs.
#[derive(Debug, Clone)]
pub struct VirtualNic {
    side: VirtualNicSide,
    shared: Rc<RefCell<VirtualNicShared>>,
}

impl VirtualNic {
    /// Create two directly connected virtual NICs with bounded frame queues.
    pub fn pair(capacity_frames: usize) -> Result<(Self, Self)> {
        if capacity_frames == 0 {
            return Err(Error::InvalidField);
        }
        let shared = Rc::new(RefCell::new(VirtualNicShared {
            a_to_b: VecDeque::new(),
            b_to_a: VecDeque::new(),
            capacity_frames,
            link_up: true,
        }));
        Ok((
            Self {
                side: VirtualNicSide::A,
                shared: shared.clone(),
            },
            Self {
                side: VirtualNicSide::B,
                shared,
            },
        ))
    }

    pub fn capacity_frames(&self) -> usize {
        self.shared.borrow().capacity_frames
    }

    pub fn pending_rx_frames(&self) -> usize {
        let shared = self.shared.borrow();
        match self.side {
            VirtualNicSide::A => shared.b_to_a.len(),
            VirtualNicSide::B => shared.a_to_b.len(),
        }
    }

    pub fn link_up(&self) -> bool {
        self.shared.borrow().link_up
    }

    /// Bring the whole direct virtual link up or down.
    pub fn set_link_up(&mut self, up: bool) {
        self.shared.borrow_mut().link_up = up;
    }
}

impl GnetFrameDevice for VirtualNic {
    fn transmit_frame(&mut self, frame: GnetFrame) -> Result<()> {
        let mut shared = self.shared.borrow_mut();
        if !shared.link_up {
            return Err(Error::LinkDown);
        }
        let capacity = shared.capacity_frames;
        let queue = match self.side {
            VirtualNicSide::A => &mut shared.a_to_b,
            VirtualNicSide::B => &mut shared.b_to_a,
        };
        if queue.len() >= capacity {
            return Err(Error::BufferFull);
        }
        queue.push_back(frame);
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<Option<GnetFrame>> {
        let mut shared = self.shared.borrow_mut();
        if !shared.link_up {
            return Err(Error::LinkDown);
        }
        Ok(match self.side {
            VirtualNicSide::A => shared.b_to_a.pop_front(),
            VirtualNicSide::B => shared.a_to_b.pop_front(),
        })
    }
}

/// End-to-end test harness built from two `VirtualNic`s.
///
/// This drives two normal smolgnet endpoints through the same host-facing
/// `GnetFrameDevice` API that a QDX-GNET controller would implement. It exists
/// specifically so integration tests do not need TUN, TAP, Ethernet, or IP.
#[derive(Debug, Clone)]
pub struct VirtualNicLink {
    attach_link: DirectLink,
    a: VirtualNic,
    b: VirtualNic,
}

impl VirtualNicLink {
    pub fn new(capacity_frames: usize) -> Result<Self> {
        let (a, b) = VirtualNic::pair(capacity_frames)?;
        Ok(Self {
            attach_link: DirectLink::new(),
            a,
            b,
        })
    }

    pub fn nic_a_mut(&mut self) -> &mut VirtualNic {
        &mut self.a
    }

    pub fn nic_b_mut(&mut self) -> &mut VirtualNic {
        &mut self.b
    }

    pub fn attach(&mut self, a: &mut Endpoint, b: &mut Endpoint) -> Result<()> {
        self.attach_link.attach(a, b)
    }

    fn pump_direction(
        sender: &mut Endpoint,
        sender_nic: &mut VirtualNic,
        receiver_nic: &mut VirtualNic,
        receiver: &mut Endpoint,
        now: u64,
    ) -> Result<(usize, bool)> {
        let mut moved_flits = 0usize;
        let mut progress = false;

        match sender.poll_tx_frame() {
            Ok(Some(frame)) => {
                sender_nic.transmit_frame(frame)?;
                progress = true;
            }
            Ok(None) | Err(Error::NoCredit) => {}
            Err(e) => return Err(e),
        }

        while let Some(frame) = receiver_nic.receive_frame()? {
            let flits = frame.flit_len();
            let control = frame.traffic == LinkTraffic::Control;
            receiver.receive_frame(frame, now)?;
            if control {
                // The test NIC has no hidden control-plane protocol. Once the
                // peer consumes the frame, its reserved control receive space
                // is immediately reusable, matching the existing direct QDX
                // software model.
                sender
                    .dlp_mut()
                    .grant_control_tx_credit(flits.min(u32::MAX as usize) as u32);
            }
            moved_flits = moved_flits.saturating_add(flits);
            progress = true;
        }

        Ok((moved_flits, progress))
    }

    pub fn pump(
        &mut self,
        a: &mut Endpoint,
        b: &mut Endpoint,
        now: u64,
        max_equivalent_flits: usize,
    ) -> Result<usize> {
        self.attach(a, b)?;
        let mut moved = 0usize;

        loop {
            if moved >= max_equivalent_flits {
                return Err(Error::BufferFull);
            }
            let mut progress = false;

            let (n, p) = Self::pump_direction(a, &mut self.a, &mut self.b, b, now)?;
            moved = moved.saturating_add(n);
            progress |= p;

            if moved >= max_equivalent_flits {
                return Err(Error::BufferFull);
            }

            let (n, p) = Self::pump_direction(b, &mut self.b, &mut self.a, a, now)?;
            moved = moved.saturating_add(n);
            progress |= p;

            if !progress {
                return Ok(moved);
            }
        }
    }
}

/// In-memory model of the QDX-GNET host-facing frame interface.
///
/// It intentionally transfers complete encoded GNET frames rather than
/// materializing every physical DLP flit. Credit accounting still uses the
/// exact equivalent physical-flit count, so the flow-control behavior remains
/// the same as on the raw-flit path.
#[derive(Debug, Clone)]
pub struct QdxDirectLink {
    attach_link: DirectLink,
}

impl QdxDirectLink {
    pub const fn new() -> Self {
        Self {
            attach_link: DirectLink::new(),
        }
    }

    pub fn attach(&mut self, a: &mut Endpoint, b: &mut Endpoint) -> Result<()> {
        self.attach_link.attach(a, b)
    }

    fn pump_direction(
        sender: &mut Endpoint,
        receiver: &mut Endpoint,
        now: u64,
    ) -> Result<usize> {
        let frame = match sender.poll_tx_frame() {
            Ok(Some(frame)) => frame,
            Ok(None) | Err(Error::NoCredit) => return Ok(0),
            Err(e) => return Err(e),
        };

        let flits = frame.flit_len();
        let control = frame.traffic == LinkTraffic::Control;
        receiver.receive_frame(frame, now)?;

        if control {
            // The in-memory QDX model completes control RECEIVE synchronously,
            // so the reserved control window can be returned in one operation.
            sender
                .dlp_mut()
                .grant_control_tx_credit(flits.min(u32::MAX as usize) as u32);
        }

        Ok(flits)
    }

    pub fn pump(
        &mut self,
        a: &mut Endpoint,
        b: &mut Endpoint,
        now: u64,
        max_equivalent_flits: usize,
    ) -> Result<usize> {
        self.attach(a, b)?;
        let mut moved = 0usize;

        loop {
            if moved >= max_equivalent_flits {
                return Err(Error::BufferFull);
            }
            let mut progress = false;

            let n = Self::pump_direction(a, b, now)?;
            if n != 0 {
                moved = moved.saturating_add(n);
                progress = true;
            }

            if moved >= max_equivalent_flits {
                return Err(Error::BufferFull);
            }

            let n = Self::pump_direction(b, a, now)?;
            if n != 0 {
                moved = moved.saturating_add(n);
                progress = true;
            }

            if !progress {
                return Ok(moved);
            }
        }
    }
}

impl Default for QdxDirectLink {
    fn default() -> Self {
        Self::new()
    }
}
