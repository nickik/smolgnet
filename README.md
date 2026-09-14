# smolgnet

`smolgnet` is a small Rust implementation of the GNet protocol suite. It began as a fork of smoltcp, but the TCP/IP implementation has been removed; the project now targets native GNet semantics and APIs.

The current implementation is the point-to-point endpoint baseline. Routing, GC3/GS3 infrastructure, and dynamic route exchange are intentionally out of scope for this milestone.

## Implemented

- native DLP flits with a configurable 2-VC or 4-VC profile
- VC selection hidden below GDP/GCTL/GTS
- caller-configured bounded receive buffer in physical flits
- receiver-driven one-credit-per-flit DLP admission
- automatic GCTL `CREDIT` and `CREDIT_REQUEST`
- continuous receive-capacity re-advertisement as DLP packets leave the receive buffer
- a reserved DLP control lane so credit-control traffic cannot deadlock behind the credit it is establishing
- GDP Global and Local address forms
- frozen GDP Size Classes and CRC-8-GNET
- FE80/16 link-local endpoint bootstrap addresses
- frozen GCTL Router `SOLICIT` / `ADVERTISE`
- frozen GCTL `ADDRESS_OFFER` / `ADDRESS_CLAIM` / `ADDRESS_ACK` / `ADDRESS_NAK`
- frozen bootstrap destination `FE80:0000:0000:0000`
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
- deterministic loss/corruption fault tests
- 1 MiB release-mode throughput comparison against stock smoltcp 0.14 TCP loopback
- `no_std` + `alloc` build
- deterministic in-memory direct-link harness for endpoint tests

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

`smolgnet` deliberately keeps the two credit systems separate:

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

The wire layouts for:

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

are frozen by GNet ADR-0019 and implemented by `wire::gctl`. The bootstrap GDP destination `FE80:0000:0000:0000`, real-buffer-derived link-credit semantics, and bounded control-progress requirement are also frozen GNet 0.1 behavior.

`docs/GCTL_WIRE_PROFILE.md` summarizes the profile. The normative source remains the `nickik/GNet` specification repository.

The current use of VC0 as a small reserved control lane is a smolgnet point-to-point implementation choice satisfying the bounded-control-progress requirement; it is not part of the GCTL body encoding itself.

## Fault tests

`tests/faults.rs` provides deterministic seeded fault injection for GTS DATA/DATA_END/DATAGRAM traffic.

The reliable test corrupts or makes packets transport-visible as lost after DLP accounting, verifies GTS CRC rejection, advances the retransmission timer, and confirms the exact original byte sequence is eventually delivered once and in order.

The unreliable test applies the same kind of faults to a sequenced DATAGRAM stream and verifies corrupt/lost messages are not delivered and are not retransmitted.

Literal loss of native DLP flits is intentionally a separate problem: losing physical flits also loses the receiver-credit consumption event and interacts with DLP resynchronization/recovery. The GTS fault test therefore isolates transport reliability from that lower-layer recovery problem.

## Performance comparison

`tests/performance.rs` transfers the same deterministic 1 MiB payload through:

1. smolgnet reliable GTS over GDP/DLP and the in-memory `DirectLink`;
2. stock smoltcp 0.14 TCP over its in-memory Ethernet `Loopback` device.

This is intended as a library CPU-cost comparison rather than a physical-network benchmark. It excludes connection establishment from the timed section and verifies the complete payload at the receiver.

Run it with:

```bash
cargo test --release --test performance -- --ignored --nocapture
```

CI also runs this comparison and prints the observed MiB/s, Gbit/s, elapsed time, and smolgnet/smoltcp ratio. Performance is not a pass/fail gate because shared-runner timing varies.

The original upstream smoltcp project also has both an in-memory loopback benchmark and a separate OS TUN/TAP benchmark. A future smolgnet host adapter can add the latter style once a native host-device interface exists; the in-memory comparison is the cleaner first comparison of stack implementation cost.

## Tests

```bash
cargo test --all-targets
cargo check --no-default-features
cargo test --release --test performance -- --ignored --nocapture
```

The test suite covers wire encoding, Local and Global GDP, DLP buffer/credit invariants, VC2/VC4 transparency, GCTL discovery/address assignment, reliable/unreliable GTS, fixed and variable stream validation, ACK/receive-credit behavior, retransmission, stream reset, tunnel reset, graceful close, deterministic loss/corruption, and comparative throughput.

`docs/TEST_CONCEPTS.md` documents the broader test strategy. `AI_CONTEXT.md` records the architectural rules for future AI-assisted development.

## Specification status caveats

The implementation follows the current `nickik/GNet` specifications. GDP Address Form bit polarity remains configurable where the canonical specification intentionally permits it. GTS timer values are implementation constants until normative timing values are frozen. Native DLP recovery after corrupted/lost framing information remains a separate unresolved lower-layer area and is not silently invented by the GTS fault harness.

## License

0BSD, inherited from the original smoltcp fork.
