use alloc::collections::VecDeque;

use crate::dlp::{DlpConfig, Flit, GnetFrame};
use crate::dlp_v01::{DlpLink, DlpLinkState};
use crate::error::Result;
use crate::wire::gdp::{GdpPacket, GdpType};

/// Router-facing DLP <-> GDP contract.
///
/// Implementors expose only complete GDP packets. Routers and other GDP
/// forwarding engines must not depend on VCIDs, flits, DLP credits, burst
/// scheduling, or receive reassembly.
pub trait GdpPacketPort {
    /// Return the next complete GDP packet received from this link.
    fn poll_gdp(&mut self) -> Option<GdpPacket>;

    /// Submit one complete GDP packet to this link for transmission.
    ///
    /// DLP decides the control/data lane and numeric VCID. The caller does not.
    fn transmit_gdp(&mut self, packet: GdpPacket) -> Result<()>;
}

/// Concrete packet boundary around one point-to-point DLP link.
///
/// The `physical_*` methods belong below the router boundary. A router should
/// receive only `&mut dyn GdpPacketPort` (or a generic constrained by that
/// trait), which makes DLP implementation details unavailable by construction.
#[derive(Debug, Clone)]
pub struct DlpGdpPort {
    link: DlpLink,
    received: VecDeque<GdpPacket>,
}

impl DlpGdpPort {
    pub fn new(config: DlpConfig) -> Result<Self> {
        Ok(Self {
            link: DlpLink::new(config)?,
            received: VecDeque::new(),
        })
    }

    pub const fn state(&self) -> DlpLinkState {
        self.link.state()
    }

    pub const fn generation(&self) -> u32 {
        self.link.generation()
    }

    pub fn begin_recovery(&mut self) -> Result<()> {
        self.link.begin_recovery()
    }

    pub fn complete_recovery(&mut self) -> Result<()> {
        self.link.complete_recovery()
    }

    /// Reset all DLP-local state and discard packets completed by the previous
    /// link generation but not yet consumed by GDP.
    pub fn reset(&mut self) {
        self.link.reset();
        self.received.clear();
    }

    // ------------------------------------------------------------------
    // Physical/link side. These APIs intentionally expose DLP details and
    // therefore must not be used by a GDP forwarding engine.
    // ------------------------------------------------------------------

    pub fn physical_receive_flit(&mut self, flit: Flit) -> Result<bool> {
        if let Some(packet) = self.link.receive(flit)? {
            self.received.push_back(packet);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn physical_receive_frame(&mut self, frame: GnetFrame) -> Result<()> {
        let packet = self.link.receive_frame(frame)?;
        self.received.push_back(packet);
        Ok(())
    }

    pub fn physical_poll_tx_flit(&mut self) -> Result<Option<Flit>> {
        self.link.poll_tx()
    }

    pub fn physical_poll_tx_burst(&mut self, out: &mut [Flit]) -> Result<usize> {
        self.link.poll_tx_burst(out)
    }

    pub fn physical_poll_tx_frame(&mut self) -> Result<Option<GnetFrame>> {
        self.link.poll_tx_frame()
    }

    pub fn grant_peer_data_credit(
        receiver: &mut Self,
        sender: &mut Self,
        count: u32,
    ) -> Result<()> {
        DlpLink::grant_peer_data_credit(&mut receiver.link, &mut sender.link, count)
    }

    pub fn grant_peer_control_credit(sender: &mut Self, count: u32) -> Result<()> {
        DlpLink::grant_peer_control_credit(&mut sender.link, count)
    }
}

impl GdpPacketPort for DlpGdpPort {
    fn poll_gdp(&mut self) -> Option<GdpPacket> {
        self.received.pop_front()
    }

    fn transmit_gdp(&mut self, packet: GdpPacket) -> Result<()> {
        match packet.header.packet_type {
            GdpType::Gctl => {
                self.link.queue_control_packet(&packet)?;
            }
            GdpType::Gts | GdpType::Reserved(_) => {
                self.link.queue_data_packet(&packet)?;
            }
        }
        Ok(())
    }
}
