impl Endpoint {
    /// Notify the endpoint that a native point-to-point link is attached.
    ///
    /// `peer_control_window_flits` is the peer's guaranteed receive capacity
    /// for the reserved data-path control VC. Ordinary data credit is
    /// established with GCTL CREDIT on the normal data path.
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

    /// Bind a peer after native DLP/GLCP bring-up without using the historical
    /// `link_attached` shortcut. This is used by the managed link-control path:
    /// bootstrap has already made the data path usable, then the first ordinary
    /// receive-credit advertisement is emitted as GCTL CREDIT on that data path.
    pub(crate) fn bind_managed_link_peer(
        &mut self,
        peer_link_local: GdpAddress,
        peer_control_window_flits: u32,
    ) -> Result<()> {
        if peer_control_window_flits == 0 {
            return Err(Error::InvalidField);
        }
        self.peer_link_local = Some(peer_link_local);
        self.credit_request_pending = false;
        self.dlp.grant_control_tx_credit(peer_control_window_flits);
        self.advertise_link_credit(true)
    }

    /// Retry a managed-link CREDIT_REQUEST when queued data no longer fits in
    /// the remaining transmit-credit balance. Credit is measured in flits, so
    /// a non-zero tail can still be unusable for the next complete GDP frame.
    ///
    /// This is deliberately best-effort because it is called from the bounded
    /// stalled-poll retry path. If the control queue cannot accept the request,
    /// the pending flag remains clear and the next retry interval tries again.
    pub(crate) fn retry_managed_link_credit_request(&mut self) {
        self.credit_request_pending = false;

        let queued = self.dlp.queued_data_flits();
        let credit = self.dlp.data_tx_credit() as usize;
        if queued == 0 || queued <= credit {
            return;
        }
        let Some(peer) = self.peer_link_local else {
            return;
        };

        let requested = queued.saturating_sub(credit).min(u32::MAX as usize) as u32;
        let tx = self.next_control_transaction();
        let msg = GctlMessage::credit_request(tx, requested.max(1));
        if self
            .send_gctl_from(self.link_local_address, peer, msg, None)
            .is_ok()
        {
            self.credit_request_pending = true;
        }
    }

    fn validate_managed_credit_grant(&self, packet: &GdpPacket, peer_limit: u32) -> Result<()> {
        if packet.header.packet_type != GdpType::Gctl {
            return Ok(());
        }
        let msg = GctlMessage::decode(&packet.payload)?;
        if msg.message_type != GctlType::Credit {
            return Ok(());
        }
        let grant = msg.parse_credit()?.granted_flits;
        if grant == 0 {
            return Err(Error::InvalidField);
        }
        let current = self.dlp.data_tx_credit();
        let remaining = peer_limit.saturating_sub(current);
        if grant > remaining {
            return Err(Error::CreditViolation);
        }
        Ok(())
    }

    /// Managed raw-flit receive path with a negotiated peer credit ceiling.
    /// CREDIT/CREDIT_REQUEST remain ordinary GDP/GCTL packets; this only adds
    /// the link-level invariant that duplicate/malformed grants cannot expand
    /// the sender balance beyond the peer's negotiated receive capacity.
    pub(crate) fn receive_managed_flit(
        &mut self,
        flit: Flit,
        now: u64,
        peer_credit_limit: u32,
    ) -> Result<bool> {
        if let Some(packet) = self.dlp.receive(flit)? {
            self.validate_managed_credit_grant(&packet, peer_credit_limit)?;
            self.handle_gdp(packet, now)?;
            self.advertise_link_credit(false)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Managed whole-frame receive path with the same negotiated credit ceiling
    /// as `receive_managed_flit`.
    pub(crate) fn receive_managed_frame(
        &mut self,
        frame: GnetFrame,
        now: u64,
        peer_credit_limit: u32,
    ) -> Result<bool> {
        let packet = self.dlp.receive_frame(frame)?;
        self.validate_managed_credit_grant(&packet, peer_credit_limit)?;
        self.handle_gdp(packet, now)?;
        self.advertise_link_credit(false)?;
        Ok(true)
    }

    /// Notify the endpoint that the native link is no longer usable.
    pub fn link_detached(&mut self) {
        self.peer_link_local = None;
        self.credit_request_pending = false;
        self.dlp.set_link_up(false);
    }
}
