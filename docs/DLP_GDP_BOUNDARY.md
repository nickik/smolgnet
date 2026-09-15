# Frozen DLP ↔ GDP Router Boundary

Status: **frozen for DLP v0.1 and the next router implementation**.

```text
GTS
 |
GDP
 |
+-------------------------------------------+
| router / forwarding packet boundary       |
| complete validated `GdpPacket` only       |
+-------------------------------------------+
 |
DLP
 |
physical/link
```

The router does not operate on DLP frames, VCIDs, flits, bursts, receive credit, or partial reassembly state.

## Public router contract

The router-facing API is intentionally only:

```rust
pub trait GdpPacketPort {
    fn poll_gdp(&mut self) -> Option<GdpPacket>;
    fn transmit_gdp(&mut self, packet: GdpPacket) -> Result<()>;
}
```

A forwarding engine should be written against `GdpPacketPort`, not `DlpEndpoint`, `DlpLink`, `Flit`, or `GnetFrame`.

The interface is deliberately packet-oriented and ownership-oriented:

- RX yields one complete `GdpPacket`.
- TX accepts one complete `GdpPacket`.
- A router may inspect or modify GDP fields required for forwarding, such as destination and Hop Limit.
- The router does not specify a VCID or DLP traffic class.

## Ingress contract

DLP may deliver a packet upward only after:

1. all physical flits/frame bytes have arrived;
2. per-VC reassembly has completed;
3. the GDP length implied by address form and size class is complete;
4. the GDP packet has decoded successfully;
5. GDP CRC validation has succeeded.

Partial input is never visible to the router.

Malformed input remains below the boundary and follows DLP per-VC desynchronization/recovery rules.

Packets completed in an old DLP generation but not yet consumed by GDP are discarded on link reset.

## Egress contract

The forwarding engine submits only a complete GDP packet.

DLP determines link-local transmission details:

```text
GDP GCTL             -> DLP control traffic -> VC0
GDP GTS/reserved     -> DLP data traffic    -> VC1..N
```

For VC4, DLP owns the `1,2,3,1,2,3,...` data-VC assignment. For VC2, DLP owns VC1.

DLP also owns:

- available transmit credit;
- `NoCredit` behavior;
- control-before-data scheduling;
- burst construction;
- frame-to-flit conversion;
- link reset/recovery.

The router must never choose a data VC to improve load balancing. That is a DLP implementation decision.

## Interface identity and routing metadata

A multi-port router associates its own interface/port identifier with each `GdpPacketPort` instance.

Port identity is **not encoded into the DLP ↔ GDP packet contract**. This keeps one DLP link reusable by hosts, routers, simulations, software devices, and FPGA implementations.

A router may maintain metadata beside the packet, for example:

```text
RouterIngress {
    interface_id,
    GdpPacket,
}
```

but `interface_id` belongs to the router, not DLP and not GDP wire format.

Likewise, next-hop/interface selection is a router decision above this boundary.

## Link state

DLP link state is observed through the port implementation/control plane rather than embedded in packets.

Current `DlpGdpPort` exposes:

- DLP lifecycle state;
- DLP generation;
- recovery/reset operations;
- physical ingress/egress methods below the packet boundary.

A future router should use link up/down events to install, suppress, or invalidate routes as appropriate, but forwarding code still exchanges GDP packets only.

## P4 boundary

This is the intended boundary for the P4 GDP dataplane.

```text
                  control plane
                       |
                 route/policy state
                       |
DLP port -> complete GDP packet -> P4 GDP forwarding -> complete GDP packet -> DLP port
```

P4 may implement GDP parsing, validation required by the forwarding pipeline, Hop-Limit processing, route lookup, next-hop selection, counters, QoS classification, and exception/punt decisions.

P4 must not implement or depend on:

- DLP receive credit;
- `CREDIT_REQUEST` mechanics;
- VC allocation;
- control/data flit scheduling;
- partial GDP reassembly from DLP flits;
- DLP reset synchronization.

Those remain in DLP/Rust/Bluespec/RTL/FPGA below the boundary.

## Hardware mapping

For a hardware router the same logical split should survive even if the packet never enters host RAM:

```text
SerDes / PHY
     |
DLP hardware block
  - credits
  - VC FIFOs
  - reassembly
  - frame completion
     |
complete GDP packet descriptor / packet buffer
     |
GDP P4 forwarding pipeline
     |
complete GDP packet descriptor / packet buffer
     |
DLP hardware block
     |
SerDes / PHY
```

The implementation may use streaming hardware internally, but semantically the P4 pipeline sees a completed GDP packet. A partially received DLP frame must not become a routable GDP packet.

## Frozen invariants

The following are now architecture invariants:

1. GDP forwarding consumes complete GDP packets, never DLP flits.
2. GDP forwarding emits complete GDP packets, never DLP flits.
3. DLP VCIDs are invisible to GDP routing.
4. DLP credit is invisible to GDP routing.
5. DLP chooses control/data lane and numeric VCID.
6. DLP reset invalidates unconsumed packets from the previous link generation.
7. Router port identity is metadata owned by the router, not DLP wire state.
8. A P4 GDP implementation starts above this boundary.
9. GC3/GS3 remain outside this point-to-point contract.

Any future implementation that requires a router to inspect DLP credits or VCIDs should be treated as an architectural regression unless this contract is deliberately revised.
