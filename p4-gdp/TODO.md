# P4 GDP TODO

## Current status

The x4c-first GDP implementation now has a real forwarding dataplane and is tested both differentially against smolgnet and end-to-end between real smolgnet endpoints.

Current files:

- `p4/gdp.p4` — GDP parser plus global/local route tables and forwarding actions;
- `p4/core.p4` — minimal core declarations needed by the pinned x4c target;
- `src/lib.rs` — x4c binding, safe parser/CRC adapter, route programming and packet-processing wrapper;
- `tests/conformance.rs` — field-level, malformed-input, CRC, randomized and forwarding differential tests;
- `tests/smolgnet_integration.rs` — real smolgnet GCTL/GTS traffic forwarded through the x4c-generated P4 dataplane;
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
- [ ] Validate the five currently reserved base-word bits once their required wire semantics are fixed.
- [ ] Decide whether non-default/inverted `GdpWireConfig::local_form_bit` must be supported by P4 or treated as a host-test-only compatibility option.
- [x] Implement GDP CRC-8 validation for the x4c target in the Rust target adapter using an implementation independent of smolgnet.
- [x] Document why forwarding does not need CRC recalculation: the current GDP CRC deliberately excludes hop limit.
- [ ] For hardware targets, map GDP CRC-8 onto a target checksum/hash extern or a small target-specific adapter without changing GDP.
- [x] Establish byte-for-byte transparent deparse testing for packets that the pipeline does not modify.

## Phase 2 — differential tests against smolgnet

- [x] Create common test vectors for global GDP packets by constructing them with smolgnet.
- [x] Create common test vectors for local GDP packets by constructing them with smolgnet.
- [x] Cover every size class.
- [x] Cover GCTL, GTS and reserved GDP type values.
- [x] Cover global and local hop-limit boundaries.
- [x] Cover malformed/truncated base, global and local packets without allowing x4c packet extraction to panic.
- [x] Cover wrong packet lengths implied by the GDP size class.
- [x] Cover invalid CRC and require agreement with smolgnet rejection behavior.
- [x] Compare explicit parsed field values between x4c and smolgnet.
- [x] Verify byte-for-byte P4/x4c output equality with `GdpPacket::encode` where the pipeline makes no forwarding modification.
- [x] Add deterministic randomized conformance generation for address forms, addresses, types, size classes and hop limits.
- [ ] Add randomized malformed-packet generation.
- [ ] Add payload-integrity patterns beyond repeated test bytes.

## Phase 2.5 — smolgnet interoperability

- [x] Put the x4c-generated GDP dataplane between two real smolgnet `Endpoint`s.
- [x] Forward smolgnet-generated link setup/control-credit GCTL traffic through P4.
- [x] Run GCTL echo request/reply through P4 in both directions.
- [x] Establish a real reliable GTS connection through P4.
- [x] Transfer a GTS application message through P4 and receive it with smolgnet.
- [x] Exercise smolgnet's local GDP address form through the P4 local-route table.
- [ ] Add loss/retry tests where the P4 test dataplane intentionally drops selected GTS packets.
- [ ] Add multi-hop tests with two independently instantiated P4 GDP pipelines.

## Phase 3 — minimal forwarding dataplane

- [x] Add destination forwarding tables for global and local GDP forms.
- [x] Add `local`, `forward` and `drop` outcomes.
- [x] Decrement hop limit only for transit packets.
- [x] Drop expired transit packets.
- [x] Preserve source address and payload.
- [x] Keep output-port metadata outside the on-wire GDP header.
- [x] Test unknown-destination and explicit-drop behavior.
- [x] Verify a forwarded packet remains CRC-valid after hop-limit mutation.
- [ ] Add route-prefix support after GDP routing-prefix semantics are fixed; current tests use exact destinations.
- [ ] Add counters for parse errors, CRC errors, expired hop limit, local delivery and forwarded packets.

## Phase 4 — software router experiment

- [x] Build an in-memory Rust wrapper around the x4c pipeline with virtual port numbers and programmable routes.
- [x] Connect the in-memory pipeline directly to smolgnet endpoint frame I/O for deterministic interoperability tests.
- [ ] Generalize the test wrapper into a reusable packet-I/O/dataplane interface only when GRouterD work resumes.
- [ ] Add a Linux high-speed I/O backend only after correctness tests remain stable.
- [ ] Measure forwarding throughput separately from smolgnet endpoint throughput.
- [x] Keep control-plane route calculation outside P4.

## Phase 5 — evaluate smolgnet integration

Only after the independent implementation has good conformance coverage:

- [ ] Compare maintainability of hand-written GDP versus generated Rust.
- [ ] Compare performance.
- [ ] Determine whether x4c-generated code is suitable for production host use or mainly for testing/router dataplanes.
- [ ] Evaluate an optional smolgnet P4 dataplane feature.
- [ ] Do not remove the existing GDP implementation merely to eliminate duplication; require a clear correctness, portability or performance advantage.

## Later hardware work

- [ ] Add BMv2/Mininet as a second independent P4 execution target before hardware.
- [ ] Select one obtainable P4 SmartNIC target.
- [ ] Compile the same core GDP P4 pipeline for real hardware.
- [ ] Isolate vendor-specific externs/adapters from `gdp.p4`.
- [ ] Test host local-delivery via PCIe queues.
- [ ] Test line-rate GDP forwarding.
- [ ] Explore a multi-port hardware GNet router using the same dataplane.
