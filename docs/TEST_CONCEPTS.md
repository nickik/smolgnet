# smolgnet point-to-point test concepts

## Goal

The first complete smolgnet implementation is a two-endpoint GNet stack. It replaces the old smoltcp TCP/IP test philosophy with equivalent GNet-facing tests before routing, GC3, GS3, or router forwarding are introduced.

The tests define the public API. Implementation details should move to satisfy these tests rather than forcing applications to mirror wire structures.

## Old smoltcp concept -> smolgnet concept

| Old stack test concept | smolgnet equivalent |
|---|---|
| Ethernet/IP packet parse/emit | DLP flit + GDP parse/emit |
| IPv4/IPv6 address/header tests | GDP Global/Local address-form tests |
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
let mut client = Endpoint::new(GdpAddress(0x0102_0304_0000_0001));
let mut server = Endpoint::new(GdpAddress(0x0102_0304_0000_0002));

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

This is the primary acceptance test for the library.

## Level 1: primitive and wire tests

Test exact behavior independently of endpoint state:

- CRC-8-GNET and CRC-32-GNET check values;
- all GDP Size Classes;
- GDP Global parse/emit;
- GDP Local parse/emit and canonical address expansion;
- GDP header corruption rejection;
- GCMP common header and message registry;
- CSS Registered-8, Short-32 and Full-128 canonicalization;
- Stream Profile packing and invalid combinations;
- every current GTS packet type;
- GTS CRC corruption rejection;
- Unchecked Payload: payload corruption accepted while metadata corruption is rejected.

## Level 2: DLP/direct-link tests

Test native point-to-point mechanics without GTS:

- 34-bit logical flit representation;
- VCID validation;
- GDP packet -> flits -> GDP packet roundtrip;
- final partial carried word handling;
- one link credit consumed per flit;
- transmission stops at zero credit;
- independent interleaved receive state for VCs;
- bad GDP header causes the VC to enter desynchronized state until reset.

The current GCTL CREDIT/CREDIT_REQUEST body encoding is not frozen. Tests therefore exercise the actual DLP credit counter directly rather than inventing a wire format. Once that body is frozen, an integration test must replace the bootstrap/direct credit injection.

## Level 3: transport state tests

Test GTS components without an Endpoint harness:

- reliable sender consumes message credits;
- selective ACK releases outstanding messages;
- receive window reorders messages and preserves boundaries;
- receive credit reflects real occupied message slots;
- retransmission becomes due after implementation RTO;
- sequenced unreliable drops duplicate/stale datagrams;
- unsequenced unreliable preserves every accepted datagram;
- stream direction rules are enforced.

## Level 4: complete two-endpoint tests

Run normal smolgnet endpoint code over an in-memory direct link:

1. GCTL echo roundtrip.
2. GTS CONNECT/CONNECT_ACK and CSS service selection.
3. Reliable Variable Stream 0 request/reply.
4. Additional reliable stream open/ack.
5. Additional sequenced unreliable stream.
6. Message boundary preservation.
7. Stream reset leaves tunnel and sibling streams alive.
8. Graceful stream close.
9. Graceful tunnel close.
10. Tunnel RESET.
11. Timer-driven retransmission generation.

## Explicitly not tested yet

- routing or longest-prefix forwarding;
- Hop Limit forwarding behavior;
- GC3/GS3;
- dynamic routing;
- path discovery across routers;
- switch address learning;
- routing errors that require an actual transit router;
- network-wide performance simulation.

The direct-link harness is deliberately small. It exists to move real DLP flits between two real Endpoint instances, not to become a general network simulator.
