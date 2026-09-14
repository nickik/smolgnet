# Exploring x4c-generated GDP inside smolgnet

Branch: `p4-smolgnet-dataplane`

## Goal

Determine whether the x4c-generated Rust implementation of `p4/gdp.p4` can replace part of smolgnet's hand-written GDP implementation for endpoint use, not only for router forwarding.

This is an experiment. The normative GDP wire-format specification remains independent of both implementations.

## Current result

Phase A/B differential decode is now implemented and passing.

With feature `p4-gdp-compare` enabled, smolgnet compiles `p4-gdp/p4/gdp.p4` directly with the pinned Oxide x4c Rust generator. Every successful `GdpPacket::decode` is then independently parsed and CRC-validated by the generated P4 parser and compared against the hand-written Rust result.

The comparison covers:

- version;
- GDP type;
- size class;
- global/local address form;
- hop limit;
- global 64-bit source/destination;
- local 16-bit source/destination;
- exact GDP packet length;
- CRC-8 validation.

Because the comparison is inside `GdpPacket::decode`, it is exercised by both complete-frame reception and the existing raw-flit reassembly path. The complete smolgnet GDP/GCTL/GTS/link/fault test suite passes with this feature enabled.

The ordinary `no_std + alloc` build also remains passing because the x4c dependencies are feature-gated behind `p4-gdp-compare` and currently require `std`.

One x4c representation detail is intentionally documented in the adapter: sub-byte/byte fields use network-order `load_be`, while generated fields wider than 8 bits currently require `load_le` because of x4c's generated header representation. The independent conformance tests are expected to catch any future x4c change here.

## Current boundary

The P4 implementation currently receives complete GDP packets. It does **not** implement the lower link layer, flits, link credits, WANT/PERMIT signalling, or GPP/GC3-style scheduling. Those remain below GDP.

For endpoint use, the initial candidate split is:

```text
application / GTS / GCTL
          |
       smolgnet
          |
  GDP endpoint adapter
          |
 x4c-generated P4 parser/dataplane
          |
      link layer
```

The generated P4 code may take responsibility for:

- GDP header parsing;
- wire-format field extraction;
- packet length validation support;
- destination/source extraction;
- packet-type and size-class classification;
- CRC validation through a small target adapter until x4c has a suitable portable CRC primitive;
- optionally local-vs-forward/drop classification.

smolgnet should retain:

- GTS session state;
- retransmission and timers;
- GCTL endpoint behavior;
- buffers and application API;
- local-prefix policy;
- lower-link flit/credit behavior;
- protocol errors and policy which are not naturally represented by P4.

## Main questions

1. Can the generated Rust eventually be used without requiring `std` in configurations where smolgnet is currently `no_std`/`alloc`?
2. Does generated parsing reduce hand-written GDP code enough to justify the compiler/runtime dependency?
3. Is generated code faster than the current direct Rust parser after optimization?
4. Can one shared P4 definition serve endpoint parsing and router forwarding without contaminating either design with target-specific behavior?
5. Can CRC checking be made portable without duplicating the entire GDP parser in the Rust adapter?
6. Is packet allocation avoidable, or does the generated pipeline force extra copies compared with the current borrowed/slice-based path?

## Experimental phases

### Phase A — generated parser as an independent endpoint decoder — DONE

- Feature-gated x4c GDP parser is compiled directly inside smolgnet.
- The current hand-written decoder remains the reference implementation.
- Current GDP, GCTL, GTS, link and fault tests execute successfully with differential checking enabled.

### Phase B — differential endpoint execution — DONE for valid traffic

For every successfully decoded packet received by smolgnet:

1. decode with hand-written smolgnet GDP;
2. decode/validate independently with x4c-generated GDP;
3. compare all GDP fields;
4. continue through the same GCTL/GTS implementation.

Further work should extend the comparison to malformed/error-result equivalence, not only successful decodes.

### Phase C — generated decoder as the selected result

The next stronger experiment is to convert the x4c parse result into the existing `GdpHeader`/`GdpPacket` representation and let endpoint execution consume that result, while optionally running the hand-written parser as the oracle in tests.

This determines whether the hand-written GDP decoder can actually disappear from a normal `std` smolgnet build rather than merely being checked by P4.

### Phase D — generated encoder/deparser

Evaluate whether outgoing GDP construction can use the generated deparser without extra copies or awkward mutable generated structures.

Do not replace the hand-written encoder unless this is clearly simpler or faster.

### Phase E — performance and size

Benchmark separately:

- current `GdpHeader::decode` / `GdpPacket::decode`;
- x4c-generated parsing plus adapter;
- current encode;
- generated deparse;
- code size;
- allocations/copies per packet.

Test release/LTO builds, not debug builds.

### Later separate experiment — high-speed link dataplane

A separate project should test the lower GNet link dataplane at high speed, including VCIDs, flits, credit accounting, scheduling/backpressure and burst behavior. smolgnet already has a software model of these mechanisms, so it can provide the behavioral oracle for that work. Do not mix that experiment into the GDP endpoint branch yet.

## Success criteria

The P4-derived endpoint implementation should only replace hand-written GDP code if it provides at least one concrete advantage without losing portability:

- materially less protocol-specific parsing code;
- stronger single-source-of-truth guarantees between software and hardware dataplanes;
- equal or better performance;
- easier conformance testing;
- easier future offload to a SmartNIC.

If generated Rust is slower, substantially larger, `std`-dependent, or requires more copying, it can still remain valuable as an independent executable oracle and router implementation rather than the normal smolgnet endpoint path.

## Important non-goal

Do not move GTS reliability, timers, retransmission, or lower-link credit/flit handling into P4 merely to maximize generated code usage. P4 should be used where its packet-processing model is a natural fit.
