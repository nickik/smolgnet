impl Endpoint {
    /// Notify the endpoint that a native point-to-point link is attached.
    ///
    /// `peer_control_window_flits` is the peer's guaranteed receive capacity
    /// for the reserved DLP control lane. Ordinary data credit is still
    /// established with GCTL CREDIT on the data path; it is deliberately not
    /// seeded here.
    pub fn link_attached(
        &mut self,
        peer_link_local: GdpAddress,
        peer_control_window_flits: u32,
    ) -> Result<()> {
        if peer_control_window_flits == 0 {
            return Err(Error::InvalidField);
        }
        self.dlp.set_link_up(true);
        self.dlp.grant_control_tx_credit(peer_control_window_flits);
        self.on_link_attached(peer_link_local)
    }

    /// Notify the endpoint that the native link is no longer usable.
    pub fn link_detached(&mut self) {
        self.peer_link_local = None;
        self.credit_request_pending = false;
        self.dlp.set_link_up(false);
    }
}
