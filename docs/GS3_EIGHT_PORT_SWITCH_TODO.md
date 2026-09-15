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

## Completed milestone

- [x] Require exactly eight physical ports.
- [x] Register and remove node-to-port entries.
- [x] Reject invalid port IDs.
- [x] Forward port 0 -> port 7.
- [x] Forward port 7 -> port 0.
- [x] Exercise every non-hairpin ingress/egress port pair.
- [x] Preserve the complete GDP packet byte-for-byte.
- [x] Do not decrement GDP Hop Limit.
- [x] Drop unknown destinations.
- [x] Drop same-port hairpins.
- [x] Move a node from one port to another.
- [x] Support multiple nodes behind one port.
- [x] Keep local-form GDP off the transit switching path.
- [x] Add a P4 exact-match fast path matching the Rust-visible contract.
- [x] Reuse smolgnet's existing x4c/P4 GDP parser and SoftNPU model.
- [x] Bound CI runtime and expose test output.

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
