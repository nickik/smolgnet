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
