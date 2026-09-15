# Eight-Port GS3 Switch

PR #16 is intentionally limited to one executable milestone: an **eight physical port GNet switch** that forwards complete GDP packets using a node-to-port forwarding table.

```text
P0 --\
P1 ---\
P2 ----\
P3 ----- [ GS3 switch ]
P4 ----/
P5 ---/
P6 --/
P7 -/
```

## Fixed contract

- Exactly eight physical ports exist: `0..7`.
- The switch itself has no GDP address.
- Input/output at this layer is a complete GDP packet plus physical port number.
- GDP packets are forwarded unchanged: the switch does not decrement Hop Limit and does not rewrite source or destination.
- A node forwarding table maps exact 64-bit global GDP node addresses to physical ports.
- The table is explicitly managed in this first milestone; automatic GS3 NODE_ANNOUNCE learning comes later.
- A known destination is sent to exactly one egress port.
- A destination mapped to the ingress port is dropped rather than hairpinned.
- An unknown global destination is dropped in this first milestone. Flooding/broadcast behavior is not inferred.
- Local-form GDP is not switched between ports in this first milestone.
- Invalid ingress/egress ports are rejected.
- Re-registering a node on another port moves the forwarding entry deterministically.
- Multiple nodes may map to the same port.

## Test-first checklist

- [ ] Require exactly eight physical ports.
- [ ] Register and remove node-to-port entries.
- [ ] Reject invalid port IDs.
- [ ] Forward port 0 -> port 7.
- [ ] Forward port 7 -> port 0.
- [ ] Preserve the complete GDP packet byte-for-byte.
- [ ] Do not decrement GDP Hop Limit.
- [ ] Drop unknown destinations.
- [ ] Drop same-port hairpins.
- [ ] Move a node from one port to another.
- [ ] Support multiple nodes behind one port.
- [ ] Keep local-form GDP off the transit switching path.
- [ ] Add a P4 exact-match fast path matching the Rust-visible contract.
- [ ] Bound CI runtime and expose test output.

## Explicitly deferred

- router integration;
- GS3 NODE_ANNOUNCE protocol handling;
- DLP/GC3 link and credit state;
- switch registration with a router;
- address allocation/discovery;
- flooding or broadcast semantics;
- multicast;
- QoS and counters;
- persistence.

Only after the two-port router and this eight-port switch work independently do we compose them in a later milestone.
