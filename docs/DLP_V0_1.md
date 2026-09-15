# DLP v0.1 — Point-to-Point Link Contract

Status: **frozen for the smolgnet point-to-point implementation**.

This document defines DLP behavior below GDP. GC3, GS3, shared-medium attachment, switching, and routing are out of scope.

The router-facing packet interface is frozen separately in [`DLP_GDP_BOUNDARY.md`](DLP_GDP_BOUNDARY.md). That contract is authoritative for the boundary between DLP and GDP forwarding.

## Layer boundary

```text
GDP packet
   |
DLP frame / VC selection / credit / scheduling
   |
32-bit flit stream or whole-frame device/DMA boundary
   |
physical point-to-point link
```

GDP and GTS never choose a VCID and never manage DLP credit. VCIDs are link-local and disappear at the DLP/GDP boundary.

The software model supports two equivalent boundaries:

- `GnetFrame`: complete encoded GDP frame plus link-local VC/traffic metadata.
- `Flit`: one 32-bit physical transfer plus link-local VCID.

The frame path is the normal QDX/driver/DMA boundary. The flit path is the reference physical/simulation model. Both must produce the same GDP bytes and consume the same credit.

## Flit and frame format

A DLP flit carries:

```text
VCID + 32 data bits
```

VCID is physical/link metadata, not part of GDP bytes.

DLP v0.1 adds no byte header around GDP. The receiver derives frame length from the GDP address form and size class in the first word. Final-flit padding is not delivered upward.

## VC profiles

| Profile | VCIDs | Meaning |
| --- | --- | --- |
| VC2 | 0, 1 | VC0 control, VC1 data |
| VC4 | 0, 1, 2, 3 | VC0 control, VC1–3 data |

VC0 is always control-only. Upper layers submit control or data, never numeric data VCIDs.

Data assignment is deterministic round-robin:

- VC2: `1, 1, 1, ...`
- VC4: `1, 2, 3, 1, 2, 3, ...`

A frame stays on one VC until complete.

## Credit model

Credit is measured in 32-bit flits.

Data credit represents real adjacent receive storage. The invariant is:

```text
advertised data credit + receive storage in use
    <= configured receive buffer capacity
```

Each transmitted data flit consumes one unit. Receiving data without previously advertised credit is a `CreditViolation`.

Control credit is independent. VC0 has a statically provisioned receive window so `CREDIT` and `CREDIT_REQUEST` cannot deadlock behind exhausted data credit.

## CREDIT and CREDIT_REQUEST

The point-to-point profile carries link-local credit messages as GCTL over VC0.

`CREDIT` advertises real receive capacity, increments the peer's transmit credit, and records the same amount as outstanding advertised receive credit locally. It may not exceed grantable capacity.

`CREDIT_REQUEST` is emitted by the endpoint integration when data is queued, transmit data credit is zero, and no request is already pending. It travels on VC0 and does not itself create credit. The peer answers with currently grantable capacity.

The existing `Endpoint` integration remains responsible for encoding/handling these GCTL messages. The DLP state machine owns the resulting link-local credit counters.

## Scheduling

Control has strict priority over data.

For each transmit poll or burst:

1. emit sendable VC0/control work first;
2. use remaining opportunity for data;
3. return `NoCredit` when work is queued but cannot progress;
4. return no work when queues are empty.

Bursting is a software/device batching optimization only. It must not alter ordering, VC assignment, credit use, or reconstructed GDP bytes.

## Receive and desynchronization

Each VC has independent reassembly state. The first flit determines expected GDP length.

Malformed or CRC-invalid GDP desynchronizes only the affected VC. Other VCs remain usable. `reset_vc(vcid)` discards partial state and clears that VC's desynchronization marker.

## Link lifecycle and recovery

The frozen lifecycle is:

```text
Down -> Recovering -> Up
  ^                    |
  +------- reset ------+
```

`Down` means no traffic, empty queues/partial receive state, and zero transmit/outstanding credit.

`Recovering` establishes fresh physical synchronization while all old link state remains invalid. Upper-layer submission remains disabled.

`Up` enables normal DLP operation. Data credit still starts at zero after recovery and must be advertised again.

A full reset increments the software generation and invalidates all queued traffic, partial receives, desynchronization state, credit, and completed packets not yet delivered upward to GDP.

## Deterministic conformance

The reference model tests:

- exact 32-bit flit words against encoded GDP bytes;
- exact VC2 and VC4 sequences;
- single-flit/burst equivalence;
- control-before-data scheduling;
- independent control/data credit;
- rejection of unadvertised data;
- prevention of over-advertisement;
- per-VC desynchronization isolation;
- reset clearing queues and credit;
- recovery with credit re-established from zero;
- deterministic complete packet exchange between two endpoints;
- incomplete physical input never crossing the GDP router boundary;
- router forwarding through the packet-only `GdpPacketPort` contract;
- automatic DLP lane/VC selection from GDP packet type.

## Hardware boundary

DLP v0.1 is **not a P4 target**.

P4 remains appropriate above this boundary for GDP parsing/classification/forwarding. DLP's stateful scheduling and credit machinery maps more naturally to Bluespec/RTL/FPGA logic.

```text
software / GDP / router
        |
   frame descriptor
        |
+---------------------------+
| DLP hardware block        |
| - VC allocator            |
| - control/data scheduler  |
| - credit counters         |
| - TX frame -> flits       |
| - RX flits -> frame       |
| - per-VC reassembly       |
| - reset/recovery state    |
+---------------------------+
        |
 physical MAC/SerDes/link
```

Keep in common Rust semantics/reference model:

- canonical VC rules;
- credit invariants;
- scheduling policy;
- lifecycle/reset semantics;
- malformed-input behavior;
- frame/flit equivalence;
- whole-frame QDX/software interface.

Move to Bluespec/RTL/FPGA:

- flit ingress/egress datapath;
- per-VC FIFOs and reassembly registers;
- credit counters/datapath;
- round-robin data VC allocator;
- control-priority arbiter;
- first-word frame-length extraction;
- reset synchronizers and hardware epoch/generation handling;
- DMA/FIFO connection to QDX-facing frame descriptors.

The future hardware implementation should be differential-tested against Rust: identical queued frames, grants and reset events must produce the same VCID/flit sequence and equivalent final state.

## Explicitly out of scope

- GC3 WANT/PERMIT;
- GC3 duplicate-address detection;
- GS3 switching;
- NODE_ANNOUNCE;
- multi-drop arbitration;
- GDP routing;
- P4 implementation of DLP.
