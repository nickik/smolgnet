use crate::dlp::{DlpConfig, DlpEndpoint, Flit, GnetFrame};
use crate::error::{Error, Result};
use crate::wire::gdp::GdpPacket;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DlpLinkState {
    Down,
    Recovering,
    Up,
}

#[derive(Debug, Clone)]
pub struct DlpLink {
    endpoint: DlpEndpoint,
    state: DlpLinkState,
    generation: u32,
}

impl DlpLink {
    pub fn new(config: DlpConfig) -> Result<Self> {
        let mut endpoint = DlpEndpoint::new(config)?;
        endpoint.set_link_up(false);
        Ok(Self {
            endpoint,
            state: DlpLinkState::Down,
            generation: 0,
        })
    }

    pub const fn state(&self) -> DlpLinkState {
        self.state
    }

    pub const fn generation(&self) -> u32 {
        self.generation
    }

    pub fn endpoint(&self) -> &DlpEndpoint {
        &self.endpoint
    }

    pub fn begin_recovery(&mut self) -> Result<()> {
        if self.state != DlpLinkState::Down {
            return Err(Error::InvalidState);
        }
        self.endpoint.set_link_up(true);
        self.state = DlpLinkState::Recovering;
        Ok(())
    }

    pub fn complete_recovery(&mut self) -> Result<()> {
        if self.state != DlpLinkState::Recovering {
            return Err(Error::InvalidState);
        }
        self.state = DlpLinkState::Up;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.endpoint.set_link_up(false);
        self.state = DlpLinkState::Down;
        self.generation = self.generation.wrapping_add(1);
    }

    fn require_up(&self) -> Result<()> {
        if self.state == DlpLinkState::Up {
            Ok(())
        } else {
            Err(Error::LinkDown)
        }
    }

    pub fn queue_data_packet(&mut self, packet: &GdpPacket) -> Result<usize> {
        self.require_up()?;
        self.endpoint.queue_data_packet(packet)
    }

    pub fn queue_control_packet(&mut self, packet: &GdpPacket) -> Result<usize> {
        self.require_up()?;
        self.endpoint.queue_control_packet(packet)
    }

    pub fn poll_tx(&mut self) -> Result<Option<Flit>> {
        self.require_up()?;
        self.endpoint.poll_tx()
    }

    pub fn poll_tx_burst(&mut self, out: &mut [Flit]) -> Result<usize> {
        self.require_up()?;
        self.endpoint.poll_tx_burst(out)
    }

    pub fn poll_tx_frame(&mut self) -> Result<Option<GnetFrame>> {
        self.require_up()?;
        self.endpoint.poll_tx_frame()
    }

    pub fn receive(&mut self, flit: Flit) -> Result<Option<GdpPacket>> {
        self.require_up()?;
        self.endpoint.receive(flit)
    }

    pub fn receive_frame(&mut self, frame: GnetFrame) -> Result<GdpPacket> {
        self.require_up()?;
        self.endpoint.receive_frame(frame)
    }

    pub fn grant_peer_data_credit(
        receiver: &mut Self,
        sender: &mut Self,
        count: u32,
    ) -> Result<()> {
        receiver.require_up()?;
        sender.require_up()?;
        receiver.endpoint.note_data_credit_granted(count)?;
        sender.endpoint.grant_data_tx_credit(count);
        Ok(())
    }

    pub fn grant_peer_control_credit(sender: &mut Self, count: u32) -> Result<()> {
        sender.require_up()?;
        sender.endpoint.grant_control_tx_credit(count);
        Ok(())
    }
}
