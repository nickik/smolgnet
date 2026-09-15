# P4 GDP Router v0.1

Status: implementation branch for the first production-style multi-port GNet router.

This router sits strictly above the frozen DLP/GDP packet boundary.

```text
                         GRouterD
                            |
                       control plane
                            |
                 routes / policy / links
                            |
DLP -> GdpPacket -> P4 ingress -> LPM/forward -> queues -> GdpPacket -> DLP
                       |           |
                       |           +-- CPU punt
                       +-- hop-limit / policy
```

## Boundary rule

The router receives and emits complete `GdpPacket` values. It never sees or manipulates:

- DLP VCIDs;
- DLP credits;
- physical flits;
- burst boundaries;
- partial receive state;
- DLP reset/recovery internals.

`DlpGdpPort` is the intended adapter below each router port.

## P4 responsibilities

The x4c-generated P4 dataplane owns:

- GDP header parsing;
- ingress-port + GDP-type policy lookup;
- global route lookup;
- true longest-prefix matching;
- default routes;
- local/punt/drop/forward decisions;
- transit Hop Limit expiry and decrement.

GDP CRC-8 excludes Hop Limit, so transit forwarding can decrement Hop Limit without recomputing the GDP header CRC.

## 64-bit GDP LPM

The existing x4c GDP representation keeps a 64-bit address as two 32-bit fields. x4c supports LPM on 32-bit fields, so the router implements complete 64-bit LPM as two stages:

```text
/33..64 : destination_hi exact + destination_lo LPM
/0..32  : destination_hi LPM
```

The `/33..64` table runs first. Only when it has no decision does the `/0..32` table run. This gives the same longest-prefix result as one native 64-bit LPM table while keeping the stable split-address x4c representation.

## Control plane / GRouterD boundary

`P4Router` currently supplies the data structures and update calls GRouterD will drive:

- `install_adjacency(id, port, next_hop)`;
- `remove_adjacency(id)`;
- `install_route(prefix, prefix_len, preference, target)`;
- `remove_route(id)`;
- `set_policy(ingress_port, GDP type, action)`;
- `clear_policy(...)`;
- `set_port_up(port, bool)`.

Route and policy updates rebuild and atomically replace the software x4c pipeline. A future hardware target should map these same operations onto hardware table programming rather than rebuilding the dataplane.

## Next-hop model

GDP preserves the end-to-end destination. The router therefore does not rewrite the GDP destination to a next hop.

Instead a route selects an adjacency:

```text
Route -> Adjacency -> { egress port, next-hop GDP address }
```

For the current point-to-point DLP profile the egress port uniquely identifies the adjacent router/node. The next-hop address remains control-plane metadata. Later GC3/GS3 neighbor/switch resolution can consume that metadata without changing GDP forwarding semantics.

## Route preference and failover

Multiple candidate routes may exist for the same prefix. Lower `preference` wins among candidates whose target adjacency is usable.

When a port goes down:

1. queued packets for that failed port are discarded;
2. the port becomes ineligible for route programming;
3. the P4 tables are rebuilt;
4. the next-best candidate for the same prefix becomes active.

When the port returns, rebuilding restores the preferred candidate.

This keeps liveness/link policy in GRouterD/Rust while forwarding remains in P4.

## Policy and CPU exceptions

Policy is evaluated before route lookup using:

```text
ingress port + GDP packet type
```

Actions are:

- allow;
- drop;
- punt to CPU.

Routes may also explicitly target CPU or drop. CPU delivery is modeled as a reserved internal pipeline port and is exposed through `poll_cpu()` rather than through DLP.

A CPU punt/local delivery does not consume a GDP routing hop.

## Per-port queues / QoS

The first router queue model has three strict-priority software queues per physical port:

1. `Control` — GCTL;
2. `Interactive` — small GDP packets (`Empty`, `Tiny3`, `Ctrl32`, `Ctrl64`);
3. `Bulk` — remaining GDP traffic.

This is router-local scheduling, not a new GDP wire field. The queues are deliberately above DLP: DLP still owns only its local control/data VC scheduling and credit system.

The queue shell can later be replaced by hardware queue metadata/egress scheduling without changing the P4 routing contract.

## Counters

The router tracks:

- received packets;
- forwarded packets;
- CPU punts;
- drops;
- queue drops;
- route-table rebuilds;
- per-port RX/TX/drop counts.

These counters are currently in the Rust router shell because x4c is the executable software dataplane. A hardware P4 target should map suitable counters into hardware while retaining equivalent control-plane semantics.

## Tests

`tests/p4_router.rs` covers:

- `/0`, `/32`, `/48`, `/64` LPM precedence;
- default route fallback;
- transit Hop Limit decrement;
- Hop Limit expiry;
- preferred/backup route failover on link down/up;
- CPU route punt;
- ingress policy punt/drop;
- control/interactive/bulk queue priority;
- router and per-port counters.

## Explicitly deferred

- dynamic routing protocol between routers;
- GRouterD process/RPC transport;
- ECMP;
- weighted multipath;
- traffic shaping/rate control;
- hardware P4Runtime integration;
- Tofino/FPGA target-specific table programming;
- GC3/GS3 attachment/switch behavior;
- neighbor discovery beyond configured adjacency metadata.

The next router milestone should wire multiple `DlpGdpPort` instances to `P4Router`, then run the existing routed-topology and GTS tests through this production router instead of the test-only forwarder.