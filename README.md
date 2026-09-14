# smolgnet

`smolgnet` is a small Rust implementation of the GNet protocol suite. It began as a fork of smoltcp, but the TCP/IP implementation has been removed; the project now targets native GNet semantics and APIs.

The current implementation is the point-to-point endpoint baseline. Routing, GC3/GS3 infrastructure, and dynamic route exchange are intentionally out of scope for this milestone.

## Implemented

- native DLP flits with configurable 2-VC or 4-VC profiles
- VC selection hidden below GDP/GCTL/GTS
- caller-configured bounded receive buffer in physical flits
- receiver-driven one-credit-per-flit DLP admission
- automatic GCTL `CREDIT` and `CREDIT_REQUEST`
- continuous receive-capacity re-advertisement as DLP packets leave the receive buffer
- reserved DLP control lane so credit-control traffic cannot deadlock behind the credit it is establishing
- GDP Global and Local address forms
- frozen GDP Size Classes and CRC-8-GNET
- FE80/16 link-local endpoint bootstrap addresses
- frozen GCTL Router `SOLICIT` / `ADVERTISE`
- frozen GCTL `ADDRESS_OFFER` / `ADDRESS_CLAIM` / `ADDRESS_ACK` / `ADDRESS_NAK`
- point-to-point address-authority implementation for bootstrap testing
- GCTL ECHO and generic GCTL inbox handling
- canonical CSS Registered-8, Short-32, and Full-128 forms
- GTS CONNECT/CONNECT_ACK and CSS service selection
- additional GTS streams with even/odd opener ownership checks
- reliable fixed and variable message streams
- selective ACK, receive-credit accounting, retransmission timers, and message reordering
- receive-credit refresh when applications consume reliable messages
- unreliable fixed and variable DATAGRAM streams
- optional sequencing and unchecked-payload mode for unreliable streams
- stream direction enforcement
- strict validation of incoming stream reliability, negotiated Size Class, length, direction, and state
- DATA_END, stream close/reset, tunnel close/reset
- reset cleanup of queued receive/transmit stream state
- `no_std` + `alloc` build
- deterministic in-memory direct-link harness
- deterministic packet-loss/corruption test harness
- 1 MiB end-to-end transfer/performance tests

## Endpoint construction

Every endpoint is given its ordinary receive-buffer capacity. This is the capacity from which link-local GCTL credits are derived.

```rust
use smolgnet::*;

let mut cfg = EndpointConfig::new(2048); // receive capacity in physical flits
cfg.vc_mode = VcMode::Four;

let endpoint = Endpoint::new(
    GdpAddress(0x0102_0304_0000_0001),
    cfg,
)?;
# Ok::<(), smolgnet::Error>(())
```

An endpoint may instead begin only with a generated/configured link-local suffix:

```rust
use smolgnet::*;

let endpoint = Endpoint::unconfigured(
    0x1234,
    EndpointConfig::new(2048),
)?;

assert_eq!(endpoint.address().0 >> 48, 0xfe80);
# Ok::<(), smolgnet::Error>(())
```

## Point-to-point GTS example

```rust
use smolgnet::*;

let cfg = EndpointConfig::new(2048);
let mut client = Endpoint::new(GdpAddress(0x0102_0304_0000_0001), cfg)?;
let mut server = Endpoint::new(GdpAddress(0x0102_0304_0000_0002), cfg)?;
let mut link = DirectLink::new();

let file = ServiceSelector::registered(1)?; // -FILE
server.listen(file, ListenerConfig::default());

let profile = StreamProfile::reliable_variable(
    SizeClass::Msg128,
    Direction::Bidirectional,
);

let client_tunnel = client.connect(server.address(), file, profile)?;
link.pump(&mut client, &mut server, 0, 100_000)?;
let server_tunnel = server.accept().unwrap();

client.send(client_tunnel, 0, b"hello", 10)?;
link.pump(&mut client, &mut server, 10, 100_000)?;
assert_eq!(server.recv(server_tunnel, 0)?, Some(b"hello".to_vec()));

// recv() frees a GTS receive slot and queues an ACK carrying refreshed
// end-to-end GTS receive credit.
link.pump(&mut client, &mut server, 11, 100_000)?;
# Ok::<(), smolgnet::Error>(())
```

## Address bootstrap example

For point-to-point testing an endpoint can act as the address authority without implementing routing:

```rust
use smolgnet::*;

let prefix = 0x1234_5678_9abc_0000;
let mut client = Endpoint::unconfigured(0x11, EndpointConfig::new(2048))?;
let mut router = Endpoint::new(
    GdpAddress(prefix | 0xfffe),
    EndpointConfig::new(2048),
)?;
router.enable_address_authority(
    AddressAuthorityConfig::new(prefix, 48, 3600)?,
);

let mut link = DirectLink::new();
client.solicit_router(1)?;
link.pump(&mut client, &mut router, 0, 100_000)?;

assert!(matches!(client.address_state(), AddressState::Assigned { .. }));
# Ok::<(), smolgnet::Error>(())
```

The test authority performs only GCTL discovery/address assignment. It is not a GDP router and contains no route table.

## Link credit and GTS credit are different

```text
GCTL / DLP link credit
    scope: adjacent physical forwarding endpoint
    unit:  one physical flit

GTS receive credit
    scope: one reliable stream direction
    unit:  one GTS message packet
```

Neither credit system is inferred from the other.

## Frozen GCTL credit/bootstrap profile

The exact GNet 0.1 body layouts for:

```text
CREDIT_REQUEST
CREDIT
SOLICIT
ADVERTISE
ADDRESS_OFFER
ADDRESS_CLAIM
ADDRESS_ACK
ADDRESS_NAK
```

are frozen by `nickik/GNet` ADR-0019, **GCTL Credit and Bootstrap Wire Profile**.

The reserved link-scoped bootstrap destination is:

```text
FE80:0000:0000:0000
```

smolgnet implements these formats directly in `wire::gctl`. See `docs/GCTL_PROFILE.md` for the implementation mapping.

## Tests

```bash
cargo test --all-targets
cargo check --no-default-features
```

The test suite covers wire encoding, Local and Global GDP, DLP buffer/credit invariants, VC2/VC4 transparency, GCTL discovery/address assignment, reliable/unreliable GTS, fixed and variable stream validation, ACK/receive-credit behavior, retransmission, stream reset, tunnel reset, graceful close, loss/corruption recovery, and a deterministic 1 MiB end-to-end transfer.

### 1 MiB in-memory throughput

The normal `performance` integration test moves a deterministic 1 MiB random buffer through two complete smolgnet endpoint stacks using reliable GTS and verifies every byte.

For useful timing output:

```bash
cargo test --release --test performance -- --nocapture
```

### smolgnet vs smoltcp

`examples/perf_compare.rs` mirrors the structure of smoltcp's own loopback TCP benchmark and runs both implementations against the same deterministic byte buffer.

```bash
cargo run --release --example perf_compare
```

Default payload is 1 MiB. Override it with:

```bash
SMOLGNET_BENCH_BYTES=67108864 cargo run --release --example perf_compare
```

The comparison is intended as an implementation-throughput check, not a claim that GTS and TCP provide identical semantics.

### Linux TAP/bridge benchmark

An ignored test crosses the real Linux TAP/bridge path. It batches GNet flits into test Ethernet frames so kernel I/O overhead is measurable without one syscall per flit.

```bash
./tools/setup_tap_pair.sh
SMOLGNET_TAP_A=gnettap0 SMOLGNET_TAP_B=gnettap1 \
  cargo test --release --test tap_performance -- --ignored --nocapture
./tools/setup_tap_pair.sh gnettap0 gnettap1 gnetbr0 clean
```

This TAP encapsulation exists only for host benchmarking; it is not a GNet wire-format definition.

## Fault-injection scope

The deterministic fault test drops complete GDP/GTS packets or corrupts the GTS CRC while preserving DLP packet boundaries. This validates GTS recovery, duplicate handling, ACK loss, and unreliable-datagram discard behavior.

It intentionally does **not** drop arbitrary GDP header flits. Losing/corrupting a Size-Class-bearing header flit exercises the separate unresolved DLP resynchronization problem and must not be hidden by the GTS test harness.

`docs/TEST_CONCEPTS.md` documents the broader test strategy. `AI_CONTEXT.md` records architectural rules for future AI-assisted development.

## Remaining specification caveats

GDP Address Form bit polarity is configurable because the bit position and meaning are frozen but numeric polarity is not yet frozen. GTS timer values are implementation constants until normative timing values are frozen. DLP resynchronization after an untrustworthy GDP header/Size Class remains open.

## License

0BSD, inherited from the original smoltcp fork.
