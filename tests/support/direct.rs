use smolgnet::{Endpoint, Error, Flit, Result, Vcid};

pub const DEFAULT_DLP_BURST_FLITS: usize = 256;

const EMPTY_FLIT: Flit = Flit {
    vcid: Vcid::CONTROL,
    data: 0,
};

#[derive(Debug, Clone)]
pub struct DirectLink {
    attached: bool,
}

impl DirectLink {
    pub const fn new() -> Self {
        Self { attached: false }
    }

    pub fn attach(&mut self, a: &mut Endpoint, b: &mut Endpoint) -> Result<()> {
        if self.attached {
            return Ok(());
        }

        let a_control = a.dlp().control_window_flits();
        let b_control = b.dlp().control_window_flits();
        let a_peer = a.link_local_address();
        let b_peer = b.link_local_address();

        a.link_attached(b_peer, b_control)?;
        b.link_attached(a_peer, a_control)?;
        self.attached = true;
        Ok(())
    }

    pub fn pump(
        &mut self,
        a: &mut Endpoint,
        b: &mut Endpoint,
        now: u64,
        max_flits: usize,
    ) -> Result<usize> {
        self.attach(a, b)?;
        let mut moved = 0;
        loop {
            if moved >= max_flits {
                return Err(Error::BufferFull);
            }
            let mut progress = false;

            match a.poll_tx_flit() {
                Ok(Some(f)) => {
                    let control = f.vcid.is_control();
                    b.receive_flit(f, now)?;
                    if control {
                        a.dlp_mut().grant_control_tx_credit(1);
                    }
                    moved += 1;
                    progress = true;
                }
                Ok(None) | Err(Error::NoCredit) => {}
                Err(e) => return Err(e),
            }

            match b.poll_tx_flit() {
                Ok(Some(f)) => {
                    let control = f.vcid.is_control();
                    a.receive_flit(f, now)?;
                    if control {
                        b.dlp_mut().grant_control_tx_credit(1);
                    }
                    moved += 1;
                    progress = true;
                }
                Ok(None) | Err(Error::NoCredit) => {}
                Err(e) => return Err(e),
            }

            if !progress {
                return Ok(moved);
            }
        }
    }
}

impl Default for DirectLink {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct BurstDirectLink {
    attach_link: DirectLink,
    burst_flits: usize,
}

impl BurstDirectLink {
    pub const fn new() -> Self {
        Self {
            attach_link: DirectLink::new(),
            burst_flits: DEFAULT_DLP_BURST_FLITS,
        }
    }

    pub fn with_burst_flits(burst_flits: usize) -> Result<Self> {
        if burst_flits == 0 || burst_flits > DEFAULT_DLP_BURST_FLITS {
            return Err(Error::InvalidField);
        }
        Ok(Self {
            attach_link: DirectLink::new(),
            burst_flits,
        })
    }

    pub fn attach(&mut self, a: &mut Endpoint, b: &mut Endpoint) -> Result<()> {
        self.attach_link.attach(a, b)
    }

    fn pump_direction(
        &self,
        sender: &mut Endpoint,
        receiver: &mut Endpoint,
        now: u64,
        room: usize,
        scratch: &mut [Flit; DEFAULT_DLP_BURST_FLITS],
    ) -> Result<usize> {
        let cap = room.min(self.burst_flits);
        if cap == 0 {
            return Ok(0);
        }

        let n = match sender.dlp_mut().poll_tx_burst(&mut scratch[..cap]) {
            Ok(n) => n,
            Err(Error::NoCredit) => 0,
            Err(e) => return Err(e),
        };
        if n == 0 {
            return Ok(0);
        }

        let mut control_flits = 0u32;
        for &flit in &scratch[..n] {
            if flit.vcid.is_control() {
                control_flits += 1;
            }
            receiver.receive_flit(flit, now)?;
        }

        if control_flits != 0 {
            sender.dlp_mut().grant_control_tx_credit(control_flits);
        }

        Ok(n)
    }

    pub fn pump(
        &mut self,
        a: &mut Endpoint,
        b: &mut Endpoint,
        now: u64,
        max_flits: usize,
    ) -> Result<usize> {
        self.attach(a, b)?;
        let mut moved = 0usize;
        let mut a_to_b = [EMPTY_FLIT; DEFAULT_DLP_BURST_FLITS];
        let mut b_to_a = [EMPTY_FLIT; DEFAULT_DLP_BURST_FLITS];

        loop {
            if moved >= max_flits {
                return Err(Error::BufferFull);
            }

            let mut progress = false;
            let n = self.pump_direction(a, b, now, max_flits - moved, &mut a_to_b)?;
            if n != 0 {
                moved += n;
                progress = true;
            }

            if moved >= max_flits {
                return Err(Error::BufferFull);
            }

            let n = self.pump_direction(b, a, now, max_flits - moved, &mut b_to_a)?;
            if n != 0 {
                moved += n;
                progress = true;
            }

            if !progress {
                return Ok(moved);
            }
        }
    }
}

impl Default for BurstDirectLink {
    fn default() -> Self {
        Self::new()
    }
}
