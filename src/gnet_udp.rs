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
    use crate::socket::udp::PacketBuffer;
    use crate::wire::{IpEndpoint, Ipv4Address};

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
}
