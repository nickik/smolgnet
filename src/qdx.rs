use crate::dlp::{Flit, GnetFrame};
use crate::error::Result;

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
