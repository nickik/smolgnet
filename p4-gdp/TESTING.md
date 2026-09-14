# P4 GDP testing strategy

The primary success criterion for the P4 GDP project is behavioral equivalence with smolgnet's current GDP implementation. Throughput work comes after correctness.

## Running the tests

From the repository root:

```sh
cargo test --manifest-path p4-gdp/Cargo.toml --all-targets
```

The `p4-gdp` crate pins Oxide's P4/x4c repository to revision:

`4645231880ccae40efd2bc583b643944ab0f0a20`

The P4 program is compiled into Rust at build time through `p4_macro::use_p4!` and executed through `p4rs::Pipeline`.

## The two complementary ways we test against smolgnet

### 1. Differential wire testing

smolgnet remains the current GDP wire-format oracle.

```text
                   one test case
                        |
             +----------+----------+
             |                     |
             v                     v
      smolgnet GDP            x4c/P4 GDP
      encode/decode          parse/validate
             |                     |
             +----------+----------+
                        |
                compare fields,
                errors and bytes
```

`tests/conformance.rs` constructs canonical GDP packets with smolgnet and compares the same bytes with the x4c implementation.

Current coverage includes:

- global and local GDP forms;
- explicit version, type, size-class, hop-limit and address comparisons;
- all 16 current size classes;
- all 16 four-bit GDP type values;
- global and local hop-limit boundaries;
- truncated base/global/local headers;
- incorrect packet lengths implied by size class;
- valid and corrupt GDP CRC-8;
- byte-for-byte output preservation for local delivery/no wire mutation;
- deterministic randomized legal packets;
- forwarding, local delivery, unknown route and explicit drop;
- transit hop decrement and hop expiry;
- source-address and payload preservation.

This test style is intentionally asymmetric: smolgnet constructs the canonical packet, while the P4 implementation independently interprets it. That makes parser representation bugs visible rather than having both implementations share parsing code.

### 2. Real smolgnet endpoint interoperability

`tests/smolgnet_integration.rs` tests at a higher level. Two actual smolgnet endpoints are connected through the x4c-generated P4 GDP dataplane:

```text
  smolgnet Endpoint A
          |
     poll_tx_frame()
          |
          v
 +-------------------+
 | x4c GDP P4        |
 | parse / route /   |
 | hop / deparse     |
 +-------------------+
          |
     receive_frame()
          |
          v
  smolgnet Endpoint B
```

The P4 code does not understand GTS connection state. It only forwards GDP. That is deliberate and tests the intended architecture: smolgnet provides the endpoint transport stack while P4 supplies the dataplane.

The current interoperability tests exercise:

- smolgnet link setup and GCTL credit traffic through P4;
- GCTL echo request/reply through P4 in both directions;
- a real smolgnet reliable GTS connection through P4;
- an application message sent over that GTS connection and received by the other smolgnet endpoint;
- the local GDP address form using the P4 local-route table.

This is stronger than comparing synthetic packets because the packets are produced and consumed by the full smolgnet endpoint implementation.

## CRC-8 treatment

GDP CRC-8 currently covers:

- version;
- GDP type;
- size class;
- address-form bit;
- source and destination address fields.

It deliberately does **not** cover hop limit. Therefore ordinary router forwarding can decrement hop limit without recalculating the CRC.

For the initial x4c target, CRC-8 validation lives in a thin Rust target adapter around the generated pipeline. It is an implementation independent of smolgnet's `Crc8`, so the differential test still has value.

This is a target limitation, not a GDP architectural decision. The current x4c SoftNPU environment does not provide a portable GDP-style CRC-8 primitive. A hardware target should use an appropriate checksum/hash extern or a small isolated target adapter rather than changing GDP to fit x4c.

## Safe malformed-input testing

The current `p4rs::packet_in::extract` implementation assumes enough input bytes are available and can panic on a truncated header. `p4-gdp` therefore places a small safe framing adapter before invoking generated parser code.

The adapter verifies:

1. enough bytes exist for the 4-byte GDP base word;
2. enough bytes exist for the selected 8-byte local or 20-byte global header;
3. the complete packet length exactly matches the GDP size class;
4. CRC-8 is valid.

This lets malformed input be tested deterministically without converting an ordinary bad packet into a Rust panic.

## Forwarding tests

The P4 program now owns the forwarding decisions. Its current tables are exact-match tables for:

- global 64-bit GDP destinations, represented internally as two 32-bit fields for x4c;
- local 16-bit GDP destinations.

Actions are:

- `deliver_local` — select an output/local port without consuming hop limit;
- `forward` — select an output port and consume one hop;
- `drop` — emit no packet.

Tests verify that unknown routes drop, explicit drop routes drop, hop `0`/`1` expires for transit forwarding, and successful forwarding decrements hop exactly once.

Prefix routing is intentionally deferred until GDP route-prefix semantics are fixed. The exact-match table is sufficient to validate the P4 architecture and interoperability now.

## Randomized conformance

A deterministic seeded randomized test generates hundreds of valid packets varying:

- global versus local form;
- arbitrary 64-bit global addresses;
- arbitrary 16-bit local IDs;
- all size classes;
- all four-bit packet-type values;
- global and local hop limits.

Each generated packet is decoded by smolgnet and independently parsed/CRC-validated by `p4-gdp`; all interpreted GDP fields must agree.

Future additions should include randomized malformed input and richer payload patterns.

## Next interoperability tests

The useful next tests are network-behavior tests rather than more parser cases:

1. **Selective loss:** configure the test dataplane to drop a chosen GTS data frame and verify smolgnet retransmission/recovery.
2. **Two P4 hops:** Endpoint A -> P4 router 1 -> P4 router 2 -> Endpoint B, verifying hop limit decrements twice while GTS remains unaware of the routers.
3. **Mixed local/global boundaries:** local-form traffic on one side of a routing context and global form across a routed boundary once those semantics are finalized.
4. **BMv2 cross-check:** run the same `gdp.p4` logic through a second P4 implementation so x4c itself is no longer the only P4 execution engine.
5. **Recorded vectors:** retain interesting smolgnet-generated GCTL/GTS packets as regression vectors that can later be replayed against x4c, BMv2 and hardware.

The two-hop and loss tests are especially useful because they test the architectural boundary: P4 routers should remain stateless with respect to GTS while smolgnet provides end-to-end reliability.

## CI

`.github/workflows/p4-gdp.yml` runs independently of the main smolgnet tests when the P4 subproject or GDP wire implementation changes.

The CI job performs:

```sh
cargo check --all-targets
cargo test --all-targets
```

from `p4-gdp/`.

## Performance testing

Performance comes only after conformance remains stable.

Measure separately:

- x4c-generated Rust parser throughput;
- complete x4c GDP forwarding-pipeline throughput;
- smolgnet GDP encode/decode throughput;
- end-to-end smolgnet traffic through the x4c dataplane;
- later, Linux packet-I/O plus x4c forwarding throughput;
- later, SmartNIC/hardware throughput.

Do not interpret generated-Rust performance as a proxy for FPGA/ASIC performance. x4c's immediate value here is executable P4 semantics and differential testing.

## Future smolgnet integration decision

These tests deliberately keep smolgnet and the P4 implementation independent. A generated implementation is not automatically preferable to the hand-written Rust implementation.

Only after correctness and performance comparisons should we decide whether smolgnet should consume any generated P4 code. It may remain cleaner to keep smolgnet as the endpoint stack and use P4 only for NIC offload and router dataplanes.
