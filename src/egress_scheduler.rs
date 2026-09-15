use alloc::collections::{BTreeMap, VecDeque};

use crate::dlp::VcMode;
use crate::dlp_v01::DlpLink;
use crate::error::{Error, Result};
use crate::wire::gdp::{GdpAddress, GdpPacket};

/// Stable identity for one router egress flow.
///
/// `transport` lets callers distinguish multiple GTS tunnels/streams between
/// the same GDP endpoints without teaching the scheduler about GTS itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EgressFlowId {
    pub source: GdpAddress,
    pub destination: GdpAddress,
    pub transport: u64,
}

impl EgressFlowId {
    pub const fn new(source: GdpAddress, destination: GdpAddress, transport: u64) -> Self {
        Self {
            source,
            destination,
            transport,
        }
    }
}

#[derive(Debug, Clone)]
struct FlowQueue {
    packets: VecDeque<GdpPacket>,
    scheduled: bool,
}

/// Fair packet-quantum scheduler in front of one DLP egress link.
///
/// DLP remains the owner of numeric VCIDs. This component only decides which
/// flows may present the next packet to DLP. Each non-empty flow appears once
/// in `ready`, receives at most one packet quantum when selected, and is then
/// appended to the tail if it still has work. Consequently a permanently busy
/// flow cannot monopolize an ordinary data VC.
#[derive(Debug, Clone)]
pub struct RouterEgressScheduler {
    flows: BTreeMap<EgressFlowId, FlowQueue>,
    ready: VecDeque<EgressFlowId>,
    queued_packets: usize,
}

impl Default for RouterEgressScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl RouterEgressScheduler {
    pub const fn new() -> Self {
        Self {
            flows: BTreeMap::new(),
            ready: VecDeque::new(),
            queued_packets: 0,
        }
    }

    pub const fn queued_packets(&self) -> usize {
        self.queued_packets
    }

    pub fn flow_count(&self) -> usize {
        self.flows.len()
    }

    pub fn enqueue(&mut self, flow: EgressFlowId, packet: GdpPacket) {
        let entry = self.flows.entry(flow).or_insert_with(|| FlowQueue {
            packets: VecDeque::new(),
            scheduled: false,
        });
        entry.packets.push_back(packet);
        self.queued_packets += 1;
        if !entry.scheduled {
            entry.scheduled = true;
            self.ready.push_back(flow);
        }
    }

    /// Admit one fair scheduling round into DLP.
    ///
    /// A round may contain at most the number of ordinary data VCs: one in
    /// VC2 mode and three in VC4 mode. The DLP queue must be empty before a
    /// new round is admitted so those packet quanta correspond to recyclable
    /// VC capacity rather than an unbounded software backlog.
    pub fn schedule_round(&mut self, link: &mut DlpLink) -> Result<usize> {
        if link.endpoint().queued_data_flits() != 0 {
            return Err(Error::InvalidState);
        }

        let capacity = match link.endpoint().vc_mode() {
            VcMode::Two => 1,
            VcMode::Four => 3,
        };
        let mut admitted = 0;

        while admitted < capacity {
            let Some(flow_id) = self.ready.pop_front() else {
                break;
            };

            let (packet, remains) = {
                let flow = self
                    .flows
                    .get_mut(&flow_id)
                    .expect("ready flow must have a queue");
                flow.scheduled = false;
                let packet = flow
                    .packets
                    .pop_front()
                    .expect("ready flow must contain a packet");
                (packet, !flow.packets.is_empty())
            };

            link.queue_data_packet(&packet)?;
            self.queued_packets -= 1;
            admitted += 1;

            if remains {
                let flow = self
                    .flows
                    .get_mut(&flow_id)
                    .expect("flow remains while being rescheduled");
                flow.scheduled = true;
                self.ready.push_back(flow_id);
            } else {
                self.flows.remove(&flow_id);
            }
        }

        Ok(admitted)
    }
}
