use std::io::ErrorKind;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use smolgnet::{Endpoint, Error, GnetFrame, GnetFrameDevice, LinkTraffic, Result, Vcid};

const KIND_FRAME: u8 = 1;
const KIND_CONTROL_CREDIT_RETURN: u8 = 2;
const FRAME_HEADER: usize = 4;
const MAX_RECORD: usize = 16 * 1024;

/// Test-only GNet NIC backed by an AF_UNIX SOCK_SEQPACKET socket.
///
/// SOCK_SEQPACKET preserves one QDX-GNET frame per host IPC record. The small
/// record header below is only test-harness metadata; it is not a GNet wire
/// format and never appears in the library itself.
#[derive(Debug)]
pub struct SeqPacketNic {
    fd: OwnedFd,
    pending_control_credit: u32,
}

impl SeqPacketNic {
    pub fn pair() -> std::io::Result<(Self, Self)> {
        let mut fds = [-1; 2];
        let rc = unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                fds.as_mut_ptr(),
            )
        };
        if rc < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let a = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let b = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        Ok((Self::new(a), Self::new(b)))
    }

    fn new(fd: OwnedFd) -> Self {
        Self {
            fd,
            pending_control_credit: 0,
        }
    }

    /// Adopt a descriptor inherited across exec by the test child process.
    ///
    /// # Safety
    /// `fd` must be a valid, uniquely owned AF_UNIX SOCK_SEQPACKET descriptor.
    pub unsafe fn from_inherited_fd(fd: RawFd) -> Self {
        Self::new(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Control whether this descriptor survives the next exec(). Tests clear
    /// CLOEXEC only on the endpoint intentionally handed to the child process.
    pub fn set_inheritable(&self, inheritable: bool) -> std::io::Result<()> {
        let fd = self.fd.as_raw_fd();
        let current = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if current < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let new_flags = if inheritable {
            current & !libc::FD_CLOEXEC
        } else {
            current | libc::FD_CLOEXEC
        };
        if unsafe { libc::fcntl(fd, libc::F_SETFD, new_flags) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn send_record(&self, bytes: &[u8]) -> Result<()> {
        loop {
            let n = unsafe {
                libc::send(
                    self.fd.as_raw_fd(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    libc::MSG_NOSIGNAL,
                )
            };
            if n >= 0 {
                return if n as usize == bytes.len() {
                    Ok(())
                } else {
                    Err(Error::LinkDown)
                };
            }

            let err = std::io::Error::last_os_error();
            match err.kind() {
                ErrorKind::Interrupted => continue,
                ErrorKind::WouldBlock => return Err(Error::BufferFull),
                _ => return Err(Error::LinkDown),
            }
        }
    }

    fn recv_record(&self, out: &mut [u8]) -> Result<Option<usize>> {
        loop {
            let n = unsafe {
                libc::recv(
                    self.fd.as_raw_fd(),
                    out.as_mut_ptr().cast(),
                    out.len(),
                    libc::MSG_DONTWAIT | libc::MSG_TRUNC,
                )
            };
            if n > 0 {
                let n = n as usize;
                if n > out.len() {
                    return Err(Error::MessageTooLarge);
                }
                return Ok(Some(n));
            }
            if n == 0 {
                return Ok(None);
            }

            let err = std::io::Error::last_os_error();
            match err.kind() {
                ErrorKind::Interrupted => continue,
                ErrorKind::WouldBlock => return Ok(None),
                _ => return Err(Error::LinkDown),
            }
        }
    }

    fn send_control_credit_return(&self, flits: u32) -> Result<()> {
        let mut record = [0u8; 5];
        record[0] = KIND_CONTROL_CREDIT_RETURN;
        record[1..5].copy_from_slice(&flits.to_be_bytes());
        self.send_record(&record)
    }

    fn take_control_credit(&mut self) -> u32 {
        let value = self.pending_control_credit;
        self.pending_control_credit = 0;
        value
    }

    /// Drive one endpoint against this test NIC until host IPC currently has
    /// no more work. Control-window replenishment is test-harness bookkeeping,
    /// analogous to the synchronous direct-link helpers used elsewhere.
    pub fn drive_endpoint(&mut self, endpoint: &mut Endpoint, now: u64) -> Result<usize> {
        let mut progress = 0usize;

        loop {
            match self.receive_frame()? {
                Some(frame) => {
                    let flits = frame.flit_len();
                    let control = frame.traffic == LinkTraffic::Control;
                    endpoint.receive_frame(frame, now)?;
                    if control {
                        self.send_control_credit_return(flits.min(u32::MAX as usize) as u32)?;
                    }
                    progress = progress.saturating_add(flits.max(1));
                }
                None => break,
            }
        }

        let returned = self.take_control_credit();
        if returned != 0 {
            endpoint.dlp_mut().grant_control_tx_credit(returned);
            progress = progress.saturating_add(1);
        }

        loop {
            match endpoint.poll_tx_frame() {
                Ok(Some(frame)) => {
                    let flits = frame.flit_len();
                    self.transmit_frame(frame)?;
                    progress = progress.saturating_add(flits.max(1));
                }
                Ok(None) | Err(Error::NoCredit) => break,
                Err(e) => return Err(e),
            }
        }

        Ok(progress)
    }
}

impl GnetFrameDevice for SeqPacketNic {
    fn transmit_frame(&mut self, frame: GnetFrame) -> Result<()> {
        let mut record = Vec::with_capacity(FRAME_HEADER + frame.bytes.len());
        record.push(KIND_FRAME);
        record.push(frame.vcid.get());
        record.push(match frame.traffic {
            LinkTraffic::Control => 0,
            LinkTraffic::Data => 1,
        });
        record.push(0);
        record.extend_from_slice(&frame.bytes);
        self.send_record(&record)
    }

    fn receive_frame(&mut self) -> Result<Option<GnetFrame>> {
        let mut record = [0u8; MAX_RECORD];
        loop {
            let Some(n) = self.recv_record(&mut record)? else {
                return Ok(None);
            };
            match record[0] {
                KIND_FRAME => {
                    if n < FRAME_HEADER || record[3] != 0 {
                        return Err(Error::InvalidLength);
                    }
                    let vcid = Vcid::new(record[1])?;
                    let traffic = match record[2] {
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
                    return Ok(Some(GnetFrame {
                        vcid,
                        traffic,
                        bytes: record[FRAME_HEADER..n].to_vec(),
                    }));
                }
                KIND_CONTROL_CREDIT_RETURN => {
                    if n != 5 {
                        return Err(Error::InvalidLength);
                    }
                    let value = u32::from_be_bytes(record[1..5].try_into().unwrap());
                    self.pending_control_credit =
                        self.pending_control_credit.saturating_add(value);
                }
                _ => return Err(Error::InvalidField),
            }
        }
    }
}
