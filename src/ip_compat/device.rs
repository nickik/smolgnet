use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

use super::IpCompatDatagram;
use crate::error::Result;
use crate::gts::GtsStream;
use crate::wire::gts::GtsPacket;

/// A whole-IP-packet device for the unmodified smoltcp TCP/UDP implementation.
///
/// This is an adaptation layer only. It accepts and produces IP packets at the
/// smoltcp `Medium::Ip` boundary; it is not a GNet DLP device.
#[derive(Debug)]
pub struct GtsIpDevice {
    ingress: VecDeque<Vec<u8>>,
    egress: VecDeque<Vec<u8>>,
    mtu: usize,
}

impl GtsIpDevice {
    pub fn new(adapter: IpCompatDatagram) -> Self {
        Self {
            ingress: VecDeque::new(),
            egress: VecDeque::new(),
            mtu: adapter.max_ip_packet(),
        }
    }

    pub const fn mtu(&self) -> usize {
        self.mtu
    }

    /// Queue a GTS DATAGRAM payload for the smoltcp ingress path.
    pub fn receive_from_gts(&mut self, adapter: &IpCompatDatagram, packet: Vec<u8>) -> Result<()> {
        self.ingress.push_back(adapter.receive(packet)?);
        Ok(())
    }

    /// Turn the oldest complete smoltcp IP packet into one GTS DATAGRAM.
    ///
    /// The packet remains queued if the GTS stream cannot currently accept it.
    pub fn transmit_to_gts(
        &mut self,
        adapter: &IpCompatDatagram,
        stream: &mut GtsStream,
        remote_tunnel_id: u32,
        now: u64,
    ) -> Result<Option<GtsPacket>> {
        let Some(packet) = self.egress.front() else {
            return Ok(None);
        };
        let frame = adapter.send(stream, remote_tunnel_id, packet, now)?;
        self.egress.pop_front();
        Ok(Some(frame))
    }

    pub fn pending_ingress(&self) -> usize {
        self.ingress.len()
    }

    pub fn pending_egress(&self) -> usize {
        self.egress.len()
    }
}

impl Device for GtsIpDevice {
    type RxToken<'a>
        = GtsIpRxToken
    where
        Self: 'a;
    type TxToken<'a>
        = GtsIpTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        self.ingress.pop_front().map(|buffer| {
            (
                GtsIpRxToken { buffer },
                GtsIpTxToken {
                    egress: &mut self.egress,
                    mtu: self.mtu,
                },
            )
        })
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(GtsIpTxToken {
            egress: &mut self.egress,
            mtu: self.mtu,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            max_transmission_unit: self.mtu,
            medium: Medium::Ip,
            ..DeviceCapabilities::default()
        }
    }
}

pub struct GtsIpRxToken {
    buffer: Vec<u8>,
}

impl RxToken for GtsIpRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.buffer)
    }
}

pub struct GtsIpTxToken<'a> {
    egress: &'a mut VecDeque<Vec<u8>>,
    mtu: usize,
}

impl TxToken for GtsIpTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        // smoltcp observes the MTU in `capabilities`; retaining an oversize
        // result here would only defer a deterministic adapter rejection.
        // The token contract still requires exactly `len` writable bytes.
        let mut buffer = vec![0; len];
        let result = f(&mut buffer);
        if len <= self.mtu {
            self.egress.push_back(buffer);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ip_compat::overlay_profile;
    use crate::wire::gdp::SizeClass;
    use crate::wire::gts::GtsPacket;

    #[test]
    fn smoltcp_device_egress_becomes_one_unreliable_gts_datagram() {
        let adapter = IpCompatDatagram::new(overlay_profile(SizeClass::Legacy1500)).unwrap();
        let mut device = GtsIpDevice::new(adapter);
        let mut stream =
            GtsStream::new(0, overlay_profile(SizeClass::Legacy1500), true, 0, 0).unwrap();

        let token = device.transmit(Instant::from_millis(0)).unwrap();
        token.consume(20, |packet| {
            packet.copy_from_slice(&[
                0x45, 0, 0, 20, 0, 0, 0, 0, 64, 6, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2,
            ]);
        });
        let frame = device
            .transmit_to_gts(&adapter, &mut stream, 7, 0)
            .unwrap()
            .unwrap();
        assert!(matches!(frame, GtsPacket::Datagram { sequence: None, .. }));
    }
}
