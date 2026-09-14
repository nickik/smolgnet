# P4 GDP TODO

## Phase 0 — establish the x4c development loop

- [ ] Pin the x4c revision used by the project.
- [ ] Add reproducible build instructions for x4c.
- [ ] Add a minimal P4 program that compiles through x4c.
- [ ] Add a Rust harness that can execute the generated pipeline.
- [ ] Make the harness feed raw packet bytes in and collect resulting packet bytes/actions out.
- [ ] Add CI once the compiler/toolchain can be installed reproducibly.

## Phase 1 — encode the current GDP wire format

- [ ] Implement the current GDP fixed header fields from `src/wire/gdp.rs`.
- [ ] Implement the global address form.
- [ ] Implement the local address form.
- [ ] Represent all 16 current size classes.
- [ ] Represent GDP GCTL and GTS types plus reserved values.
- [ ] Parse the global 8-bit hop limit.
- [ ] Parse the local 4-bit hop limit.
- [ ] Determine the cleanest portable-P4 treatment of the existing CRC-8 calculation.
- [ ] Deparse byte-for-byte equivalent GDP headers.

## Phase 2 — differential tests against smolgnet

- [ ] Create common test vectors for global GDP packets.
- [ ] Create common test vectors for local GDP packets.
- [ ] Cover every size class.
- [ ] Cover GCTL, GTS and reserved GDP type values.
- [ ] Cover hop-limit boundaries.
- [ ] Cover malformed/truncated packets.
- [ ] Cover invalid CRC.
- [ ] Verify that P4/x4c parsing agrees with `GdpHeader::decode`.
- [ ] Verify that P4/x4c deparsing agrees byte-for-byte with `GdpHeader::encode` where the pipeline makes no forwarding modification.
- [ ] Add randomized/property-style packet generation where practical.

## Phase 3 — minimal forwarding dataplane

- [ ] Add destination forwarding table.
- [ ] Add `local`, `forward` and `drop` outcomes.
- [ ] Decrement hop limit only for transit packets.
- [ ] Drop expired transit packets.
- [ ] Preserve source address and payload.
- [ ] Define next-hop/output-port metadata outside the on-wire GDP header.
- [ ] Add counters for parse errors, CRC errors, expired hop limit, local delivery and forwarded packets.

## Phase 4 — software router experiment

- [ ] Build a small Rust packet-I/O harness around the x4c pipeline.
- [ ] Start with in-memory virtual ports for deterministic tests.
- [ ] Add a Linux high-speed I/O backend only after correctness tests pass.
- [ ] Measure forwarding throughput separately from smolgnet endpoint throughput.
- [ ] Keep control-plane route calculation outside P4.

## Phase 5 — evaluate smolgnet integration

Only after the independent implementation has good conformance coverage:

- [ ] Compare maintainability of hand-written GDP versus generated Rust.
- [ ] Compare performance.
- [ ] Determine whether x4c-generated code is suitable for production host use or mainly for testing/router dataplanes.
- [ ] Evaluate an optional smolgnet P4 dataplane feature.
- [ ] Do not remove the existing GDP implementation merely to eliminate duplication; require a clear correctness, portability or performance advantage.

## Later hardware work

- [ ] Select one obtainable P4 SmartNIC target.
- [ ] Compile the same core GDP P4 pipeline for real hardware.
- [ ] Isolate vendor-specific externs/adapters from `gdp.p4`.
- [ ] Test host local-delivery via PCIe queues.
- [ ] Test line-rate GDP forwarding.
- [ ] Explore a multi-port hardware GNet router using the same dataplane.
