//! UDP underlay for a virtual GNet point-to-point link.
//!
//! A GNet peer is configured with an IP/UDP endpoint solely to reach its
//! underlay peer. The payload remains a GNet link frame. In particular, an
//! IP address is never a GDP address and UDP ports are never GTS stream IDs.
//!
//! The encapsulation is one UDP datagram per complete virtual-link frame. It
//! preserves the DLP virtual channel and traffic class so that a later DLP
//! endpoint can run unchanged above this module.

use crate::socket::udp::{RecvError, SendError, Socket, UdpMetadata};

/// The fixed UDP encapsulation header length.
pub const HEADER_LEN: usize = 8;
const MAGIC: [u8; 2] = *b"GN";
const VERSION: u8 = 1;

/// Link-local traffic class carried alongside a GNet frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficClass {
    Control,
    Data,
}

impl TrafficClass {
    const fn to_wire(self) -> u8 {
        match self {
            Self::Control => 0,
            Self::Data => 1,
        }
    }

    fn from_wire(value: u8) -> Result<Self, DecodeError> {
        match value {
            0 => Ok(Self::Control),
            1 => Ok(Self::Data),
            _ => Err(DecodeError::InvalidTrafficClass),
        }
    }
}

/// A complete GNet virtual-link frame, borrowed from the UDP payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    /// DLP virtual channel. VC 0 is reserved for link control.
    pub vcid: u8,
    pub traffic: TrafficClass,
    /// The opaque encoded GNet frame (normally GDP/GCTL/GTS bytes).
    pub payload: &'a [u8],
}

/// An invalid GNet-over-UDP datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    InvalidVirtualChannel,
    InvalidTrafficClass,
    LengthMismatch,
}

/// A failure while queueing a GNet frame for UDP transmission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransmitError {
    DatagramTooLarge,
    Udp(SendError),
}

/// Encode `frame` into an already-sized UDP payload buffer.
pub fn encode(frame: Frame<'_>, output: &mut [u8]) -> Result<(), TransmitError> {
    if frame.vcid > 3
        || frame.payload.len() > u16::MAX as usize
        || output.len() != HEADER_LEN + frame.payload.len()
    {
        return Err(TransmitError::DatagramTooLarge);
    }

    output[0..2].copy_from_slice(&MAGIC);
    output[2] = VERSION;
    output[3] = frame.vcid;
    output[4] = frame.traffic.to_wire();
    output[5] = 0;
    output[6..8].copy_from_slice(&(frame.payload.len() as u16).to_be_bytes());
    output[HEADER_LEN..].copy_from_slice(frame.payload);
    Ok(())
}

/// Decode one whole UDP payload into a virtual-link frame.
pub fn decode(input: &[u8]) -> Result<Frame<'_>, DecodeError> {
    if input.len() < HEADER_LEN {
        return Err(DecodeError::Truncated);
    }
    if input[0..2] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    if input[2] != VERSION {
        return Err(DecodeError::UnsupportedVersion);
    }
    if input[3] > 3 {
        return Err(DecodeError::InvalidVirtualChannel);
    }
    if input[5] != 0 {
        return Err(DecodeError::UnsupportedVersion);
    }
    let payload_len = u16::from_be_bytes([input[6], input[7]]) as usize;
    if input.len() != HEADER_LEN + payload_len {
        return Err(DecodeError::LengthMismatch);
    }
    Ok(Frame {
        vcid: input[3],
        traffic: TrafficClass::from_wire(input[4])?,
        payload: &input[HEADER_LEN..],
    })
}

/// Queue one virtual-link frame on a smoltcp UDP socket.
pub fn send(
    socket: &mut Socket<'_>,
    frame: Frame<'_>,
    peer: impl Into<UdpMetadata>,
) -> Result<(), TransmitError> {
    let length = HEADER_LEN
        .checked_add(frame.payload.len())
        .ok_or(TransmitError::DatagramTooLarge)?;
    let output = socket.send(length, peer).map_err(TransmitError::Udp)?;
    encode(frame, output)
}

/// Receive and decode one virtual-link frame from a smoltcp UDP socket.
///
/// `Ok(None)` means that the socket has no queued UDP datagram. A malformed
/// GNet encapsulation is consumed and reported, never forwarded upwards.
pub fn receive<'a>(
    socket: &'a mut Socket<'_>,
) -> Result<Option<(Frame<'a>, UdpMetadata)>, ReceiveError> {
    match socket.recv() {
        Ok((payload, peer)) => decode(payload)
            .map(|frame| Some((frame, peer)))
            .map_err(ReceiveError::Decode),
        Err(RecvError::Exhausted) => Ok(None),
        Err(error) => Err(ReceiveError::Udp(error)),
    }
}

/// A failure while receiving a GNet UDP datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveError {
    Udp(RecvError),
    Decode(DecodeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iface::{Config, Interface, SocketHandle, SocketSet};
    use crate::phy::{Loopback, Medium};
    use crate::socket::udp::PacketBuffer;
    use crate::time::Instant;
    use crate::wire::{HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};
    use smolgnet_native::wire::gts::GtsContext;
    use smolgnet_native::{
        Direction, GdpAddress, GdpHeader, GdpPacket, GdpType, GdpWireConfig, GtsPacket, GtsStream,
        SizeClass, StreamProfile,
    };

    fn udp_loopback() -> (
        Interface,
        Loopback,
        SocketSet<'static>,
        SocketHandle,
        SocketHandle,
    ) {
        let mut device = Loopback::new(Medium::Ip);
        let mut iface =
            Interface::new(Config::new(HardwareAddress::Ip), &mut device, Instant::ZERO);
        iface.update_ip_addrs(|addrs| {
            addrs
                .push(IpCidr::new(IpAddress::v4(192, 0, 2, 1), 24))
                .unwrap();
        });

        let mut sockets = SocketSet::new(vec![]);
        let make_socket = || {
            Socket::new(
                PacketBuffer::new(
                    vec![crate::socket::udp::PacketMetadata::EMPTY; 4],
                    vec![0; 512],
                ),
                PacketBuffer::new(
                    vec![crate::socket::udp::PacketMetadata::EMPTY; 4],
                    vec![0; 512],
                ),
            )
        };
        let mut sender = make_socket();
        sender.bind(4_000).unwrap();
        let mut receiver = make_socket();
        receiver.bind(4_001).unwrap();
        let sender = sockets.add(sender);
        let receiver = sockets.add(receiver);
        (iface, device, sockets, sender, receiver)
    }

    fn forward_over_udp(
        iface: &mut Interface,
        device: &mut Loopback,
        sockets: &mut SocketSet<'_>,
        sender: SocketHandle,
        receiver: SocketHandle,
        frame: Frame<'_>,
        port: u16,
    ) -> Vec<u8> {
        send(
            sockets.get_mut::<Socket>(sender),
            frame,
            IpEndpoint::new(Ipv4Address::new(192, 0, 2, 1).into(), port),
        )
        .unwrap();
        iface.poll(Instant::ZERO, device, sockets);
        iface.poll(Instant::from_millis(1), device, sockets);
        let (received, _) = receive(sockets.get_mut::<Socket>(receiver))
            .unwrap()
            .unwrap();
        received.payload.to_vec()
    }

    fn gts_context(size_class: SizeClass) -> GtsContext {
        GtsContext {
            gdp_version: 0,
            size_class,
            source: GdpAddress(0x100),
            destination: GdpAddress(0x200),
        }
    }

    fn encode_gdp_gts(payload: Vec<u8>, size_class: SizeClass) -> Vec<u8> {
        GdpPacket::new(
            GdpHeader::global(
                GdpType::Gts,
                size_class,
                15,
                GdpAddress(0x100),
                GdpAddress(0x200),
            ),
            payload,
        )
        .unwrap()
        .encode(GdpWireConfig::default())
        .unwrap()
    }

    #[test]
    fn preserves_a_complete_frame_and_link_metadata() {
        let frame = Frame {
            vcid: 2,
            traffic: TrafficClass::Data,
            payload: b"encoded GDP/GTS packet",
        };
        let mut encoded = [0u8; HEADER_LEN + 22];
        encode(frame, &mut encoded).unwrap();
        assert_eq!(decode(&encoded).unwrap(), frame);
    }

    #[test]
    fn rejects_a_trailing_or_short_udp_payload() {
        let mut encoded = [0u8; HEADER_LEN + 1];
        encode(
            Frame {
                vcid: 0,
                traffic: TrafficClass::Control,
                payload: b"x",
            },
            &mut encoded,
        )
        .unwrap();
        assert_eq!(
            decode(&encoded[..HEADER_LEN]),
            Err(DecodeError::LengthMismatch)
        );
        let mut with_trailer = encoded.to_vec();
        with_trailer.push(0);
        assert_eq!(decode(&with_trailer), Err(DecodeError::LengthMismatch));
    }

    #[test]
    fn rejects_non_gnet_udp_traffic() {
        assert_eq!(decode(b"not a GNet payload"), Err(DecodeError::BadMagic));
    }

    #[test]
    fn queues_one_complete_frame_on_a_real_smoltcp_udp_socket() {
        let rx = PacketBuffer::new(vec![crate::socket::udp::PacketMetadata::EMPTY], vec![0; 64]);
        let tx = PacketBuffer::new(vec![crate::socket::udp::PacketMetadata::EMPTY], vec![0; 64]);
        let mut socket = Socket::new(rx, tx);
        socket.bind(4_000).unwrap();

        send(
            &mut socket,
            Frame {
                vcid: 1,
                traffic: TrafficClass::Data,
                payload: b"GTS DATA",
            },
            IpEndpoint::new(Ipv4Address::new(192, 0, 2, 1).into(), 4_001),
        )
        .unwrap();

        assert_eq!(socket.send_queue(), HEADER_LEN + b"GTS DATA".len());
    }

    #[test]
    fn native_gts_reorders_and_acknowledges_across_smoltcp_udp() {
        let profile = StreamProfile::reliable_variable(SizeClass::Ctrl64, Direction::Bidirectional);
        let mut native_sender = GtsStream::new(0, profile, true, 2, 2).unwrap();
        let mut native_receiver = GtsStream::new(0, profile, false, 2, 2).unwrap();
        let first = native_sender
            .send_packet(7, b"first".to_vec(), false, 0)
            .unwrap();
        let second = native_sender
            .send_packet(7, b"second".to_vec(), false, 1)
            .unwrap();
        let (mut iface, mut device, mut sockets, udp_sender, udp_receiver) = udp_loopback();

        let mut ack = None;
        // UDP does not promise ordering, so deliver sequence 1 before 0.
        for outgoing in [second, first] {
            let size_class = outgoing.choose_size_class(Some(profile)).unwrap();
            let context = gts_context(size_class);
            let encoded =
                encode_gdp_gts(outgoing.encode(context, Some(profile)).unwrap(), size_class);
            let received = forward_over_udp(
                &mut iface,
                &mut device,
                &mut sockets,
                udp_sender,
                udp_receiver,
                Frame {
                    vcid: 1,
                    traffic: TrafficClass::Data,
                    payload: &encoded,
                },
                4_001,
            );
            let incoming_gdp = GdpPacket::decode(&received, GdpWireConfig::default(), 0).unwrap();
            assert_eq!(incoming_gdp.header.packet_type, GdpType::Gts);
            let incoming =
                GtsPacket::decode(&incoming_gdp.payload, context, Some(profile)).unwrap();
            native_receiver
                .validate_incoming(size_class, &incoming)
                .unwrap();
            ack = native_receiver.receive_packet(&incoming).unwrap().or(ack);
        }

        let ack = ack.expect("in-order delivery creates a GTS acknowledgement");
        let ack_size_class = ack.choose_size_class(None).unwrap();
        let ack_context = gts_context(ack_size_class);
        let encoded_ack = encode_gdp_gts(ack.encode(ack_context, None).unwrap(), ack_size_class);
        let received_ack = forward_over_udp(
            &mut iface,
            &mut device,
            &mut sockets,
            udp_receiver,
            udp_sender,
            Frame {
                vcid: 1,
                traffic: TrafficClass::Data,
                payload: &encoded_ack,
            },
            4_000,
        );
        let ack_gdp = GdpPacket::decode(&received_ack, GdpWireConfig::default(), 0).unwrap();
        assert_eq!(ack_gdp.header.packet_type, GdpType::Gts);
        let decoded_ack = GtsPacket::decode(&ack_gdp.payload, ack_context, None).unwrap();
        match decoded_ack {
            GtsPacket::Ack {
                ack_base,
                receive_bitmap,
                receive_credit,
                ..
            } => native_sender
                .on_ack(ack_base, receive_bitmap, receive_credit)
                .unwrap(),
            other => panic!("expected GTS ACK, received {other:?}"),
        }

        assert_eq!(native_receiver.recv(), Some(b"first".to_vec()));
        assert_eq!(native_receiver.recv(), Some(b"second".to_vec()));
        assert!(native_sender.retransmit_due(7, 500).is_empty());
    }
}
