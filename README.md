# smolgnet

`smolgnet` is a small Rust implementation of the GNet protocol suite. It began as a fork of smoltcp, but the TCP/IP implementation has been removed; the project now targets native GNet semantics and APIs.

The current implementation is the complete point-to-point endpoint baseline. Routing, GC3/GS3 infrastructure, and dynamic route exchange are intentionally out of scope for this milestone.

## Implemented

- 34-bit logical DLP flits: 2-bit VCID + 32 carried bits
- bounded DLP segment assembly and one-credit-per-flit transmit admission
- GDP Global and Local address forms
- frozen GDP Size Classes and CRC-8-GNET
- GCTL common messages and point-to-point ECHO
- canonical CSS Registered-8, Short-32, and Full-128 forms
- GTS CONNECT/CONNECT_ACK and CSS service selection
- additional GTS streams
- reliable fixed and variable message streams
- selective ACK, receive-credit accounting, retransmission timers, and message reordering
- unreliable fixed and variable DATAGRAM streams
- optional sequencing and unchecked-payload mode for unreliable streams
- stream direction enforcement
- DATA_END, stream close/reset, tunnel close/reset
- `no_std` + `alloc` build
- deterministic in-memory direct-link harness for endpoint tests

## Example

```rust
use smolgnet::*;
use smolgnet::wire::gts::Direction;

let mut client = Endpoint::new(GdpAddress(0x0102_0304_0000_0001));
let mut server = Endpoint::new(GdpAddress(0x0102_0304_0000_0002));
let mut link = DirectLink::new(1_000_000);

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
# Ok::<(), smolgnet::Error>(())
```

## Tests

```bash
cargo test --all-targets
cargo check --no-default-features
```

The test strategy is documented in `docs/TEST_CONCEPTS.md`. `AI_CONTEXT.md` records the architectural rules for future AI-assisted development.

## Specification status caveats

The implementation follows the current `nickik/GNet` specifications. Where the protocol specification deliberately remains open, smolgnet does not invent wire formats. In particular, GCTL `CREDIT_REQUEST`/`CREDIT` body packing is still draft; the DLP flit-credit mechanism is implemented and tested directly. GDP Address Form bit polarity is configurable because the bit position and meaning are frozen but numeric polarity is not yet frozen. GTS timer values are implementation constants until normative timing values are frozen.

## License

0BSD, inherited from the original smoltcp fork.
