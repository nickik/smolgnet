//! Optional IP/TCP/UDP compatibility transport over an unreliable GTS stream.
//!
//! This module deliberately stops at the GTS message boundary. The later
//! `ip-compat` feature will connect it to the original smoltcp IP socket
//! subsystem; native GNet endpoints continue to use GTS directly.

use alloc::vec::Vec;

use crate::error::{Error, Result};
use crate::gts::GtsStream;
use crate::wire::gdp::SizeClass;
use crate::wire::gts::{Direction, GtsPacket, StreamProfile};

#[cfg(feature = "ip-compat")]
mod device;
#[cfg(feature = "ip-compat")]
pub use device::GtsIpDevice;

/// Unreliable GTS DATAGRAM overhead for an unsequenced variable stream.
const GTS_DATAGRAM_OVERHEAD: usize = 12;

/// GTS profile required by the IP compatibility overlay.
pub const fn overlay_profile(max_size_class: SizeClass) -> StreamProfile {
    StreamProfile::unreliable_variable(
        max_size_class,
        Direction::Bidirectional,
        false,
        false,
    )
}

/// Checks and emits complete IPv4 or IPv6 packets for the compatibility
/// overlay. It owns no transport state: TCP must observe loss and reordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpCompatDatagram {
    max_ip_packet: usize,
}

impl IpCompatDatagram {
    /// Construct the adapter for the exact overlay stream profile.
    pub fn new(profile: StreamProfile) -> Result<Self> {
        if profile != overlay_profile(profile.size_class) {
            return Err(Error::ProfileViolation);
        }
        let max_ip_packet = profile
            .size_class
            .bytes()
            .checked_sub(GTS_DATAGRAM_OVERHEAD)
            .ok_or(Error::InvalidSizeClass)?;
        Ok(Self { max_ip_packet })
    }

    pub const fn max_ip_packet(&self) -> usize {
        self.max_ip_packet
    }

    /// Validate that `packet` is a complete IP packet that fits the negotiated
    /// GTS Size Class. Deep IP validation stays in the imported smoltcp layer.
    pub fn validate_ip_packet(&self, packet: &[u8]) -> Result<()> {
        if packet.len() > self.max_ip_packet {
            return Err(Error::MessageTooLarge);
        }
        match packet.first().map(|octet| octet >> 4) {
            Some(4) if packet.len() >= 20 => Ok(()),
            Some(6) if packet.len() >= 40 => Ok(()),
            _ => Err(Error::InvalidField),
        }
    }

    /// Emit one complete IP packet as one unreliable GTS DATAGRAM.
    pub fn send(
        &self,
        stream: &mut GtsStream,
        remote_tunnel_id: u32,
        packet: &[u8],
        now: u64,
    ) -> Result<GtsPacket> {
        self.validate_ip_packet(packet)?;
        stream.send_packet(remote_tunnel_id, packet.to_vec(), false, now)
    }

    /// Validate a message dequeued from the overlay stream before handing it
    /// to the IP implementation.
    pub fn receive(&self, packet: Vec<u8>) -> Result<Vec<u8>> {
        self.validate_ip_packet(&packet)?;
        Ok(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_ip_only_as_an_unreliable_gts_datagram() {
        let profile = overlay_profile(SizeClass::Legacy1500);
        let adapter = IpCompatDatagram::new(profile).unwrap();
        let mut sender = GtsStream::new(0, profile, true, 0, 0).unwrap();
        let packet = [0x45, 0, 0, 20, 0, 0, 0, 0, 64, 6, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2];

        let frame = adapter.send(&mut sender, 9, &packet, 0).unwrap();
        assert!(matches!(frame, GtsPacket::Datagram { sequence: None, .. }));
        assert!(sender.retransmit_due(9, 10_000).is_empty());
    }

    #[test]
    fn receiver_accepts_tcp_ip_packet_after_loss_of_an_earlier_datagram() {
        let profile = overlay_profile(SizeClass::Legacy1500);
        let adapter = IpCompatDatagram::new(profile).unwrap();
        let mut sender = GtsStream::new(0, profile, true, 0, 0).unwrap();
        let mut receiver = GtsStream::new(0, profile, false, 0, 0).unwrap();
        let lost = [0x45, 0, 0, 20, 0, 0, 0, 0, 64, 6, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2];
        let delivered = [0x45, 0, 0, 20, 0, 1, 0, 0, 64, 6, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2];

        let _dropped = adapter.send(&mut sender, 9, &lost, 0).unwrap();
        let frame = adapter.send(&mut sender, 9, &delivered, 1).unwrap();
        receiver.validate_incoming(SizeClass::Ctrl32, &frame).unwrap();
        assert_eq!(receiver.receive_packet(&frame).unwrap(), None);
        assert_eq!(adapter.receive(receiver.recv().unwrap()).unwrap(), delivered);
    }

    #[test]
    fn rejects_non_ip_and_oversize_messages() {
        let adapter = IpCompatDatagram::new(overlay_profile(SizeClass::Ctrl64)).unwrap();
        assert_eq!(adapter.validate_ip_packet(&[0; 20]), Err(Error::InvalidField));
        assert_eq!(
            adapter.validate_ip_packet(&[0x60; 53]),
            Err(Error::MessageTooLarge)
        );
    }
}
