use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use smolgnet::{
    DirectLink, Endpoint, Error, GnetFrame, GnetFrameDevice, LinkTraffic, Result,
};

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

/// Test-only pure user-space virtual GNet NIC pair.
#[derive(Debug, Clone)]
pub struct VirtualNic {
    side: VirtualNicSide,
    shared: Rc<RefCell<VirtualNicShared>>,
}

impl VirtualNic {
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

    pub fn pending_rx_frames(&self) -> usize {
        let shared = self.shared.borrow();
        match self.side {
            VirtualNicSide::A => shared.b_to_a.len(),
            VirtualNicSide::B => shared.a_to_b.len(),
        }
    }

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
