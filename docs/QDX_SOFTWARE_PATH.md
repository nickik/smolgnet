# QDX-GNET Software Path

## Purpose

`smolgnet` supports two deliberately different lower-layer software paths.

The normal host-facing path models the existing QDX-GNET frame interface. Raw physical flits remain available for conformance tests, fault injection, simple hardware, and future router/link simulation.

```text
Application
    |
   GTS
    |
   GDP
    |
 DLP frame/credit state
    |
 QDX-GNET frame interface
    |
--------------------------- host/device boundary
    |
 VC scheduling / credits / flits
    |
 controller / physical link
```

QDX-GNET `RECEIVE` and `TRANSMIT` operate on complete GNET frames in host memory. The CPU therefore should not normally perform one API call, queue operation, or interrupt for every physical flit.

## Two backend levels

### QDX-GNET frame path

The normal endpoint/device path is represented by complete encoded GNET frames:

```rust
pub struct GnetFrame {
    pub vcid: Vcid,
    pub traffic: LinkTraffic,
    pub bytes: Vec<u8>,
}
```

`Endpoint::poll_tx_frame()` and `Endpoint::receive_frame()` expose this path.

DLP still accounts for the exact number of physical flits represented by the frame, so link credit semantics are unchanged:

```text
physical flits represented by frame
              |
              v
        link credit count
```

A data frame cannot be transmitted unless the peer has advertised enough flit receive capacity for the entire frame. On receive, exactly that number of outstanding advertised credits is consumed and the buffer capacity is returned when processing completes.

The encoded GDP frame and its CRCs are still parsed and validated normally.

Because QDX supplies an explicit frame boundary, a bad GDP header CRC rejects that frame without making the next QDX frame ambiguous. This differs from the raw-flit path, where a corrupt Size Class can lose the native stream boundary and requires DLP resynchronization.

### Raw flit path

The raw path remains available through `Flit`, `poll_tx_burst`, `receive_burst`, and the `GnetFlitDevice` concept.

It is intended for:

- DLP and VC conformance testing;
- link-credit tests;
- physical corruption/loss experiments;
- simple/raw GNet interface hardware;
- future GC3/GS3/router simulation;
- testing native resynchronization behavior.

Raw flits are now materialized lazily from the same contiguous queued frame. `smolgnet` no longer stores every outgoing frame as hundreds of `Flit` objects.

## TX storage

The internal DLP TX queue stores contiguous encoded frames rather than `VecDeque<Flit>`:

```text
TxFrame
  VCID
  traffic class
  contiguous encoded GDP bytes
  raw-flit cursor (only used by raw backend)
```

The QDX path transfers the complete buffer directly. The raw backend generates 32-bit carried words only as they are requested.

## RX storage

The QDX path passes the contiguous frame directly to the GDP decoder. It does not rebuild a frame four bytes at a time.

The raw path retains per-VC incremental assembly because that is required to model the physical protocol faithfully.

## CRC implementation

CRC-8-GNET and CRC-32-GNET retain incremental APIs. Byte processing is table-driven rather than bit-at-a-time.

CRC state can be updated over multiple discontiguous spans:

```text
GDP/GTS pseudo-header
        |
GTS metadata
        |
application payload
        |
CRC state
```

This allows future scatter/gather and zero-copy work without concatenating all checksum input into a temporary buffer.

## Performance test

`tests/performance.rs` transfers the same deterministic 1 MiB payload through:

1. the legacy one-flit-at-a-time smolgnet path;
2. the burst raw-flit path;
3. the QDX-GNET whole-frame path;
4. smoltcp TCP over its in-memory Loopback device.

A representative GitHub Actions release run after the frame and CRC changes measured:

```text
smolgnet legacy flits     25.22 MiB/s
smolgnet burst flits      66.72 MiB/s
smolgnet QDX frames       84.65 MiB/s
smoltcp TCP loopback      97.99 MiB/s
```

The QDX path therefore reached about 86.4% of smoltcp throughput, or about 13.6% lower throughput, on that runner. The benchmark is not a CI performance gate because hosted-runner timing varies.

This establishes that the earlier roughly 4x difference was an implementation artifact, not an inherent cost of GNet.

## Remaining optimization opportunities

The agreed first target was to get the normal local GNet path within roughly 20% of smoltcp. That target is met.

There is still avoidable copying above the QDX boundary:

```text
application slice
   -> GTS-owned Vec
   -> encoded GTS Vec
   -> GDP payload/frame Vec
```

Future work can use caller buffers or scatter/gather descriptors so QDX-GNET can DMA header and application spans directly. The incremental CRC implementation is already suitable for this.

This remaining work is an implementation optimization. It must not change GDP, GCTL, GTS, credit, or message-preservation semantics.
