# P4 GDP TODO

## Current status

The x4c-first implementation is now executable rather than documentation-only.

Current files:

- `p4/gdp.p4` — initial GDP parser/pass-through dataplane;
- `p4/core.p4` — minimal core declarations needed by the pinned x4c target;
- `src/lib.rs` — `p4_macro::use_p4!` binding that generates Rust at compile time;
- `tests/conformance.rs` — differential tests built from smolgnet GDP packets;
- `.github/workflows/p4-gdp.yml` — independent P4 GDP CI job.

Pinned Oxide P4/x4c revision:

`4645231880ccae40efd2bc583b643944ab0f0a20`

## Phase 0 — establish the x4c development loop

- [x] Pin the x4c revision used by the project.
- [x] Add reproducible Cargo dependencies for the pinned x4c/p4rs implementation.
- [x] Add a minimal P4 program that compiles through the x4c Rust macro path.
- [x] Add a Rust harness that can execute the generated pipeline.
- [x] Make the harness feed raw packet bytes in and collect resulting packet bytes/port out.
- [x] Add CI for the independent `p4-gdp` crate.

## Phase 1 — encode the current GDP wire format

- [x] Implement the current 32-bit GDP base word layout from `src/wire/gdp.rs`.
- [x] Implement the global address form.
- [x] Implement the local address form using the default `GdpWireConfig` form-bit convention.
- [x] Parse all 4 bits of the size-class field; differential tests exercise all 16 current classes.
- [x] Parse all 4 bits of the GDP type field; differential tests exercise GCTL, GTS and reserved values.
- [x] Parse the global 8-bit hop-limit field.
- [x] Parse the local form's low 4-bit hop limit as encoded by smolgnet.
- [ ] Validate the five currently reserved base-word bits.
- [ ] Decide whether non-default/inverted `GdpWireConfig::local_form_bit` must be supported by P4 or treated as a host-test-only compatibility option.
- [ ] Determine the cleanest portable-P4 treatment of the existing CRC-8 calculation.
- [x] Establish byte-for-byte transparent deparse testing for packets that the pipeline does not modify.

## Phase 2 — differential tests against smolgnet

- [x] Create common test vectors for global GDP packets by constructing them with smolgnet.
- [x] Create common test vectors for local GDP packets by constructing them with smolgnet.
- [x] Cover every size class.
- [x] Cover GCTL, GTS and reserved GDP type values.
- [x] Cover global and local hop-limit boundaries.
- [ ] Cover malformed/truncated packets.
- [ ] Cover invalid CRC once P4-side CRC validation exists.
- [ ] Compare explicit parsed field values, not only transparent byte output.
- [x] Verify byte-for-byte P4/x4c output equality with `GdpPacket::encode` where the pipeline makes no forwarding modification.
- [ ] Add randomized/property-style packet generation.
- [ ] Add payload-integrity patterns beyond repeated test bytes.

## Phase 3 — minimal forwarding dataplane

Do not begin this until the Phase 1 parser/deparser behavior is stable.

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
