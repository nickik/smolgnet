# Exploring x4c-generated GDP inside smolgnet

Branch: `p4-smolgnet-dataplane`

## Goal

Determine whether the x4c-generated Rust implementation of `p4/gdp.p4` can replace part of smolgnet's hand-written GDP implementation for endpoint use, not only for router forwarding.

This is an experiment. The normative GDP wire-format specification remains independent of both implementations.

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

1. Can the generated Rust be used without requiring `std` in configurations where smolgnet is currently `no_std`/`alloc`?
2. Does generated parsing reduce hand-written GDP code enough to justify the compiler/runtime dependency?
3. Is generated code faster than the current direct Rust parser after optimization?
4. Can one shared P4 definition serve endpoint parsing and router forwarding without contaminating either design with target-specific behavior?
5. Can CRC checking be made portable without duplicating the entire GDP parser in the Rust adapter?
6. Is packet allocation avoidable, or does the generated pipeline force extra copies compared with the current borrowed/slice-based path?

## Experimental phases

### Phase A — generated parser as an independent endpoint decoder

- Add a feature-gated endpoint decoder backed by x4c.
- Convert `ParsedGdp` into the existing smolgnet `GdpHeader` / `GdpPacket` representation.
- Keep the current hand-written decoder available as the reference implementation.
- Run every current GDP, GCTL and GTS test against both decoders.

### Phase B — differential endpoint execution

For every packet received by a test endpoint:

1. decode with hand-written smolgnet GDP;
2. decode with x4c-generated GDP;
3. compare all fields and errors;
4. feed the selected result into the same GCTL/GTS implementation.

Add randomized packets and malformed packets to this path.

### Phase C — generated encoder/deparser

Evaluate whether outgoing GDP construction can use the generated deparser without extra copies or awkward mutable generated structures.

Do not replace the hand-written encoder unless this is clearly simpler or faster.

### Phase D — performance and size

Benchmark separately:

- current `GdpHeader::decode` / `GdpPacket::decode`;
- x4c-generated parsing plus adapter;
- current encode;
- generated deparse;
- code size;
- allocations/copies per packet.

Test release/LTO builds, not debug builds.

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
