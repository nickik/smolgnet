# P4 GDP testing strategy

The first success criterion for the P4 GDP project is behavioral equivalence with smolgnet's current GDP implementation, not raw throughput.

## Running the current tests

From the repository root:

```sh
cargo test --manifest-path p4-gdp/Cargo.toml --all-targets
```

The `p4-gdp` crate pins Oxide's P4/x4c repository to revision:

`4645231880ccae40efd2bc583b643944ab0f0a20`

The P4 program is compiled into Rust at build time through `p4_macro::use_p4!`, producing the same `p4rs::Pipeline` interface used by Oxide's software P4 tests.

## Implemented coverage

`tests/conformance.rs` currently constructs canonical GDP packets with smolgnet, sends the encoded bytes through the x4c-generated P4 pipeline, recombines P4 header and payload output, and requires byte-for-byte equality.

Covered now:

- valid global GDP header;
- valid local GDP header;
- all 16 current size classes;
- GCTL (`0x1`), GTS (`0x2`) and every other four-bit GDP type value;
- global hop-limit values `0`, `1`, `254`, `255`;
- local hop-limit values `0`, `1`, `14`, `15`;
- payload preservation through the transparent pipeline;
- output-port preservation in the initial pass-through pipeline.

These tests intentionally use `GdpWireConfig::default()`, where the local-form bit is `1`.

## Test levels

### 1. P4 parser/deparser tests

Feed raw GDP byte strings into the x4c-generated Rust pipeline and verify parsed behavior and emitted bytes.

Already covered:

- valid global header;
- valid local header;
- every current size class;
- every packet-type value;
- hop-limit boundaries.

Still required:

- truncated 4-byte base word;
- truncated global address section;
- truncated local address section;
- invalid reserved base-word bits if the specification requires rejection;
- corrupt CRC after P4-side CRC validation is implemented.

### 2. Differential tests

For each packet vector:

1. construct it with `src/wire/gdp.rs`;
2. encode it using smolgnet;
3. feed the same bytes to the x4c-generated P4 pipeline;
4. collect the P4 output packet;
5. where no forwarding mutation is requested, require byte-for-byte output equality.

The next improvement is to expose and compare explicit parsed GDP fields as well, so a parser bug cannot be hidden by transparent byte preservation.

### 3. CRC tests

CRC behavior is deliberately not faked in the first P4 implementation.

Before forwarding semantics are considered complete:

- reproduce the current smolgnet CRC-8 coverage exactly;
- accept known-good smolgnet-generated CRCs;
- reject deliberately corrupted CRCs;
- verify that any header mutation that participates in the CRC results in a correctly recomputed value before deparse.

If portable P4 cannot express the current CRC efficiently, isolate CRC handling behind a small target abstraction rather than changing GDP solely to suit one P4 compiler.

### 4. Forwarding semantics

Use virtual input/output ports and a small route table.

Verify:

- local destination -> local delivery;
- known remote destination -> selected output port;
- unknown destination -> defined drop behavior;
- transit packet -> hop limit decremented once;
- expired hop limit -> drop;
- payload -> unchanged;
- source address -> unchanged;
- unrelated header fields -> unchanged;
- CRC -> valid after any forwarding mutation that affects it.

### 5. Randomized conformance

Generate legal GDP headers using smolgnet, encode them to bytes, execute them through the P4/x4c pipeline, and compare results.

Important dimensions:

- address form;
- arbitrary 64-bit global addresses;
- arbitrary local IDs and prefixes;
- all size classes;
- all four-bit packet-type values;
- hop-limit values;
- varied payload contents rather than a single repeated byte.

Malformed inputs should also be generated deliberately, but correctness assertions must distinguish between malformed packets that the specification requires to be rejected and inputs for which behavior is currently unspecified.

### 6. Router pipeline tests

Once forwarding exists, model a fixed topology with virtual ports:

```text
host A -- port 0 [ P4 GDP router ] port 1 -- host B
                         |
                       port 2
                         |
                       host C
```

Inject GDP packets at each port and verify route selection, hop-limit handling, local delivery and counters.

This is intentionally a packet-forwarding test, not a complete network simulator.

## CI

`.github/workflows/p4-gdp.yml` runs independently of the main smolgnet stack tests when the P4 subproject or GDP wire implementation changes.

The CI job performs:

```sh
cargo check --all-targets
cargo test --all-targets
```

from `p4-gdp/`.

## Performance testing

Performance comes only after the conformance suite passes.

Measure separately:

- x4c-generated Rust parser throughput;
- complete x4c GDP forwarding pipeline throughput;
- smolgnet GDP encode/decode throughput;
- later, Linux packet-I/O plus x4c forwarding throughput;
- later, SmartNIC/hardware throughput.

Do not interpret generated-Rust performance as a proxy for FPGA/ASIC performance. The value of x4c initially is executable semantics and testing.

## Future smolgnet comparison

The eventual integration decision should be based on evidence from these tests. A generated implementation is not automatically preferable to the hand-written Rust implementation. The project should preserve smolgnet's small, understandable endpoint stack unless P4-derived code offers a concrete advantage.
