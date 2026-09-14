# smolgnet point-to-point test concepts

## Goal

The first complete smolgnet implementation is a two-endpoint GNet stack. It replaces the old smoltcp TCP/IP test philosophy with equivalent GNet-facing tests before routing, GC3, GS3, or router forwarding are introduced.

The tests define the public API. Implementation details should move to satisfy these tests rather than forcing applications to mirror wire structures.

## Old smoltcp concept -> smolgnet concept

| Old stack test concept | smolgnet equivalent |
|---|---|
| Ethernet/IP packet parse/emit | DLP flit + GDP parse/emit |
| IP address/header tests | GDP Global/Local address-form tests |
| ICMP echo | GCTL ECHO_REQUEST/ECHO_REPLY |
| UDP datagram socket | GTS unreliable stream |
| TCP connect/listen | GTS CONNECT/CONNECT_ACK with CSS |
| TCP byte transfer | GTS reliable message stream |
| TCP window | GTS message Receive Credit |
| TCP sequence/retransmit tests | GTS per-message sequence/selective ACK/RTO |
| TCP close/reset | STREAM_CLOSE/STREAM_RESET/TUNNEL_CLOSE/RESET |
| raw IP socket | raw GDP/GCTL wire tests |

## Public API target

```rust
let cfg = EndpointConfig::new(2048);
let mut client = Endpoint::new(GdpAddress(0x0102_0304_0000_0001), cfg)?;
let mut server = Endpoint::new(GdpAddress(0x0102_0304_0000_0002), cfg)?;
let mut link = DirectLink::new();

let file = ServiceSelector::registered(0x01)?; // -FILE
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
```

This is the primary acceptance shape for the library.

## Level 1: primitive and wire tests

Test exact behavior independently of endpoint state:

- CRC-8-GNET and CRC-32-GNET check values;
- all GDP Size Classes;
- GDP Global parse/emit;
- GDP Local parse/emit and canonical address expansion;
- GDP header corruption rejection;
- GCMP common header and message registry;
- frozen ADR-0019 CREDIT_REQUEST/CREDIT bodies;
- frozen ADR-0019 SOLICIT/ADVERTISE bodies;
- frozen ADR-0019 ADDRESS_OFFER/CLAIM/ACK/NAK bodies;
- bootstrap destination behavior;
- CSS Registered-8, Short-32 and Full-128 canonicalization;
- Stream Profile packing and invalid combinations;
- every current GTS packet type;
- GTS CRC corruption rejection;
- Unchecked Payload: payload corruption accepted while metadata corruption is rejected.

## Level 2: DLP/direct-link tests

Test native point-to-point mechanics without depending on application traffic:

- logical flit representation;
- VC2 and VC4 scheduling profiles;
- numeric VCIDs hidden from GDP/GCTL/GTS;
- GDP packet -> flits -> GDP packet roundtrip;
- final partial carried word handling;
- one link credit consumed per ordinary data flit;
- transmission stops at zero credit;
- advertised credit cannot exceed real bounded receive capacity;
- received data flits must be backed by advertised credit;
- complete GDP delivery releases DLP receive-buffer capacity;
- CREDIT updates replenish newly grantable capacity;
- control lane progress is independent of ordinary data credit;
- bad GDP header desynchronizes only the affected VC until reset.

## Level 3: transport state tests

Test GTS components without an Endpoint harness:

- reliable sender consumes message credits;
- selective ACK releases outstanding messages;
- receive window reorders messages and preserves boundaries;
- receive credit reflects real occupied message slots;
- application consumption refreshes receive credit;
- retransmission becomes due after implementation RTO;
- fixed reliable streams require exact normal DATA payload size;
- fixed DATA_END may contain a shorter final message;
- incoming packets must match the negotiated reliability/class/profile;
- sequenced unreliable drops duplicate/stale datagrams;
- unsequenced unreliable preserves every accepted datagram;
- stream direction rules are enforced;
- stream reset clears queued receive and transmit/retransmission state.

## Level 4: complete two-endpoint tests

Run normal smolgnet endpoint code over an in-memory direct link:

1. Real GCTL CREDIT bootstrap from bounded endpoint buffers.
2. GCTL echo roundtrip and continued link-credit replenishment.
3. GTS CONNECT/CONNECT_ACK and CSS service selection.
4. Reliable Variable Stream 0 request/reply.
5. Reliable Fixed stream transfer and DATA_END.
6. Additional reliable stream open/ack.
7. Additional sequenced unreliable stream.
8. Message boundary preservation.
9. Application receive causes GTS receive-credit ACK refresh.
10. Stream reset leaves tunnel and sibling streams alive.
11. Graceful stream close.
12. Graceful tunnel close.
13. Tunnel RESET resets all streams.
14. Timer-driven retransmission without duplicate application delivery.
15. GDP Global endpoint operation.
16. GDP Local endpoint operation inside a configured local context.
17. Link-local-only endpoint startup.
18. SOLICIT -> ADVERTISE -> ADDRESS_OFFER -> ADDRESS_CLAIM -> ADDRESS_ACK assignment.

## Level 5: deterministic fault injection

`tests/faults.rs` exercises GTS integrity and reliability through the real endpoint/DLP path.

The harness uses a seeded deterministic pseudo-random sequence so CI sees exactly the same fault pattern on each run.

### Reliable GTS

- DATA packets are randomly made transport-visible as lost by invalidating their GTS CRC after DLP accounting;
- other DATA packets have application payload bytes corrupted while their original GTS CRC remains;
- the receiver must discard both forms;
- sender RTO must produce retransmissions;
- the final application byte stream must exactly equal the original source data;
- duplicates must not be delivered.

The current recovery test stays within one 32-packet selective-ACK window. Larger sliding-window policy is a separate transport-design issue.

### Unreliable GTS

- sequenced DATAGRAM packets receive the same seeded loss/corruption treatment;
- corrupted packets must never reach the application;
- lost packets remain lost;
- no retransmission state is created.

Literal loss of native DLP flits is intentionally not treated as a GTS test because it also affects hop-local credit accounting and DLP resynchronization. That belongs in future link-recovery tests.

## Level 6: comparative throughput

`tests/performance.rs` transfers the same deterministic 1 MiB payload through two in-memory stacks:

```text
smolgnet:
Application -> reliable GTS -> GDP -> DLP -> DirectLink -> peer

smoltcp:
Application -> TCP -> IPv4 -> Ethernet -> Loopback -> same Interface
```

Connection establishment is completed before the timed region. Both paths verify the full 1 MiB payload at the receiver.

The comparison follows the architecture of upstream smoltcp's own `loopback_benchmark.rs`, making it a useful measurement of library implementation cost without OS TUN/TAP overhead.

Run:

```bash
cargo test --release --test performance -- --ignored --nocapture
```

The benchmark prints:

- elapsed time;
- MiB/s;
- Gbit/s;
- smolgnet physical flit count;
- smoltcp poll rounds;
- smolgnet/smoltcp throughput ratio.

Performance is observational, not a CI threshold, because shared runners vary.

## Explicitly not tested yet

- routing or longest-prefix forwarding;
- Hop Limit forwarding behavior;
- GC3/GS3;
- dynamic routing;
- path discovery across routers;
- switch address learning;
- routing errors that require an actual transit router;
- native DLP recovery from physically lost flits;
- full network-wide performance simulation.

The direct-link and fault harnesses are deliberately small. They exist to move real DLP flits between real Endpoint instances and validate protocol behavior, not to become a general network simulator.
