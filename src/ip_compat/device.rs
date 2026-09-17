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
        let mut capabilities = DeviceCapabilities::default();
        capabilities.max_transmission_unit = self.mtu;
        capabilities.medium = Medium::Ip;
        capabilities
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
    use smoltcp::iface::{Config as SmolConfig, Interface as SmolInterface, SocketSet};
    use smoltcp::socket::udp;
    use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr};

    fn interface(device: &mut GtsIpDevice, address: [u8; 4]) -> SmolInterface {
        let mut interface =
            SmolInterface::new(SmolConfig::new(HardwareAddress::Ip), device, Instant::ZERO);
        interface.update_ip_addrs(|addresses| {
            addresses
                .push(IpCidr::new(
                    IpAddress::v4(address[0], address[1], address[2], address[3]),
                    24,
                ))
                .unwrap();
        });
        interface
    }

    fn udp_socket() -> udp::Socket<'static> {
        udp::Socket::new(
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0; 2048]),
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0; 2048]),
        )
    }

    fn deliver_next(
        sender: &mut GtsIpDevice,
        adapter: &IpCompatDatagram,
        tx_stream: &mut GtsStream,
        receiver: &mut GtsIpDevice,
        rx_stream: &mut GtsStream,
        now: u64,
    ) -> bool {
        let Some(frame) = sender.transmit_to_gts(adapter, tx_stream, 7, now).unwrap() else {
            return false;
        };
        rx_stream
            .validate_incoming(SizeClass::Legacy1500, &frame)
            .unwrap();
        rx_stream.receive_packet(&frame).unwrap();
        receiver
            .receive_from_gts(adapter, rx_stream.recv().unwrap())
            .unwrap();
        true
    }

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

    #[test]
    fn smoltcp_udp_delivers_after_an_earlier_gts_datagram_is_dropped() {
        let profile = overlay_profile(SizeClass::Legacy1500);
        let adapter = IpCompatDatagram::new(profile).unwrap();
        let mut sender_device = GtsIpDevice::new(adapter);
        let mut receiver_device = GtsIpDevice::new(adapter);
        let mut sender_interface = interface(&mut sender_device, [10, 0, 0, 1]);
        let mut receiver_interface = interface(&mut receiver_device, [10, 0, 0, 2]);
        let mut sender_stream = GtsStream::new(0, profile, true, 0, 0).unwrap();
        let mut receiver_stream = GtsStream::new(0, profile, false, 0, 0).unwrap();

        let mut sender_sockets = SocketSet::new(vec![]);
        let sender_handle = sender_sockets.add(udp_socket());
        sender_sockets
            .get_mut::<udp::Socket>(sender_handle)
            .bind(10001)
            .unwrap();
        let mut receiver_sockets = SocketSet::new(vec![]);
        let receiver_handle = receiver_sockets.add(udp_socket());
        receiver_sockets
            .get_mut::<udp::Socket>(receiver_handle)
            .bind(10002)
            .unwrap();

        let remote = (IpAddress::v4(10, 0, 0, 2), 10002);
        sender_sockets
            .get_mut::<udp::Socket>(sender_handle)
            .send_slice(b"dropped", remote)
            .unwrap();
        sender_interface.poll(
            Instant::from_millis(1),
            &mut sender_device,
            &mut sender_sockets,
        );
        assert!(
            sender_device
                .transmit_to_gts(&adapter, &mut sender_stream, 7, 1)
                .unwrap()
                .is_some()
        );

        sender_sockets
            .get_mut::<udp::Socket>(sender_handle)
            .send_slice(b"delivered", remote)
            .unwrap();
        sender_interface.poll(
            Instant::from_millis(2),
            &mut sender_device,
            &mut sender_sockets,
        );
        assert!(deliver_next(
            &mut sender_device,
            &adapter,
            &mut sender_stream,
            &mut receiver_device,
            &mut receiver_stream,
            2,
        ));
        receiver_interface.poll(
            Instant::from_millis(2),
            &mut receiver_device,
            &mut receiver_sockets,
        );

        let socket = receiver_sockets.get_mut::<udp::Socket>(receiver_handle);
        assert!(socket.can_recv());
        let (payload, metadata) = socket.recv().unwrap();
        assert_eq!(payload, b"delivered");
        assert_eq!(metadata.endpoint.port, 10001);
        assert!(!socket.can_recv());
    }
}
