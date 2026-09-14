use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

use crate::error::{Error, Result};
use crate::link::{GnetFrame, LinkTraffic, Vcid};
use crate::qdx::GnetFrameDevice;

/// IEEE 802 local experimental EtherType 1. This is used only by the hosted
/// TAP adapter and is not part of the GNet wire specification.
pub const GNET_TAP_ETHERTYPE: u16 = 0x88b5;
const ETH_HEADER: usize = 14;
const SHIM_LEN: usize = 4;
const SHIM_VERSION: u8 = 1;
const MAX_TAP_FRAME: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapAddressing {
    pub local_mac: [u8; 6],
    pub peer_mac: [u8; 6],
}

impl TapAddressing {
    pub const fn new(local_mac: [u8; 6], peer_mac: [u8; 6]) -> Self {
        Self { local_mac, peer_mac }
    }
}

/// Encode one QDX-GNET host frame inside a debugging-only Ethernet envelope.
pub fn encode_tap_frame(frame: &GnetFrame, addressing: TapAddressing) -> Vec<u8> {
    let mut out = Vec::with_capacity(ETH_HEADER + SHIM_LEN + frame.bytes.len());
    out.extend_from_slice(&addressing.peer_mac);
    out.extend_from_slice(&addressing.local_mac);
    out.extend_from_slice(&GNET_TAP_ETHERTYPE.to_be_bytes());
    out.push(SHIM_VERSION);
    out.push(frame.vcid.get());
    out.push(match frame.traffic { LinkTraffic::Control => 0, LinkTraffic::Data => 1 });
    out.push(0);
    out.extend_from_slice(&frame.bytes);
    out
}

/// Decode the hosted TAP envelope back into the normal QDX-GNET frame type.
pub fn decode_tap_frame(bytes: &[u8], addressing: TapAddressing) -> Result<GnetFrame> {
    if bytes.len() < ETH_HEADER + SHIM_LEN {
        return Err(Error::InvalidLength);
    }
    let dst: [u8; 6] = bytes[0..6].try_into().unwrap();
    if dst != addressing.local_mac && dst != [0xff; 6] {
        return Err(Error::InvalidField);
    }
    let ethertype = u16::from_be_bytes([bytes[12], bytes[13]]);
    if ethertype != GNET_TAP_ETHERTYPE || bytes[14] != SHIM_VERSION || bytes[17] != 0 {
        return Err(Error::InvalidField);
    }
    let vcid = Vcid::new(bytes[15])?;
    let traffic = match bytes[16] {
        0 => LinkTraffic::Control,
        1 => LinkTraffic::Data,
        _ => return Err(Error::InvalidField),
    };
    if traffic == LinkTraffic::Control && !vcid.is_control() {
        return Err(Error::InvalidField);
    }
    if traffic == LinkTraffic::Data && vcid.is_control() {
        return Err(Error::InvalidField);
    }
    Ok(GnetFrame {
        vcid,
        traffic,
        bytes: bytes[ETH_HEADER + SHIM_LEN..].to_vec(),
    })
}

/// Linux TAP-backed QDX-GNET frame adapter.
///
/// This is a host/testing adaptation. Native systems should use a real
/// QDX-GNET controller/driver instead of Ethernet encapsulation.
#[derive(Debug)]
pub struct TapDevice {
    file: File,
    addressing: TapAddressing,
}

impl TapDevice {
    pub fn from_file(file: File, addressing: TapAddressing) -> Self {
        Self { file, addressing }
    }

    #[cfg(target_os = "linux")]
    pub fn open(name: &str, addressing: TapAddressing) -> std::io::Result<Self> {
        const TUNSETIFF: libc::c_ulong = 0x4004_54ca;
        const IFF_TAP: libc::c_short = 0x0002;
        const IFF_NO_PI: libc::c_short = 0x1000;

        if name.is_empty() || name.len() >= libc::IFNAMSIZ {
            return Err(std::io::Error::new(ErrorKind::InvalidInput, "invalid TAP name"));
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/net/tun")?;

        let mut ifr: libc::ifreq = unsafe { core::mem::zeroed() };
        for (dst, src) in ifr.ifr_name.iter_mut().zip(name.as_bytes()) {
            *dst = *src as libc::c_char;
        }
        unsafe {
            ifr.ifr_ifru.ifru_flags = IFF_TAP | IFF_NO_PI;
            if libc::ioctl(file.as_raw_fd(), TUNSETIFF, &ifr) < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(Self::from_file(file, addressing))
    }

    pub fn addressing(&self) -> TapAddressing { self.addressing }
}

impl GnetFrameDevice for TapDevice {
    fn transmit_frame(&mut self, frame: GnetFrame) -> Result<()> {
        let bytes = encode_tap_frame(&frame, self.addressing);
        self.file.write_all(&bytes).map_err(|_| Error::LinkDown)
    }

    fn receive_frame(&mut self) -> Result<Option<GnetFrame>> {
        let mut buffer = vec![0u8; MAX_TAP_FRAME];
        match self.file.read(&mut buffer) {
            Ok(0) => Ok(None),
            Ok(n) => decode_tap_frame(&buffer[..n], self.addressing).map(Some),
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(_) => Err(Error::LinkDown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_codec_roundtrips_qdx_metadata_and_bytes() {
        let addresses = TapAddressing::new([2, 0, 0, 0, 0, 1], [2, 0, 0, 0, 0, 2]);
        let peer_view = TapAddressing::new(addresses.peer_mac, addresses.local_mac);
        let frame = GnetFrame {
            vcid: Vcid::VC2,
            traffic: LinkTraffic::Data,
            bytes: vec![1, 2, 3, 4, 5],
        };
        let encoded = encode_tap_frame(&frame, addresses);
        assert_eq!(decode_tap_frame(&encoded, peer_view).unwrap(), frame);
    }
}
