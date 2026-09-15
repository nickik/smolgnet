# Dynamic topology and routing TODO

Branch: `dynamic-topology-routing`

The first dynamic-routing revision stays deliberately small. It uses GCTL neighbor discovery plus forward route exchange. It does not build a topology database or run SPF/Dijkstra.

## Stage 1 — Spec + data model

- [x] Define `RouterId` as a non-zero random 64-bit identity independent of GDP interface addresses.
- [x] Generate RouterId from the operating system random source under `std`; persistence is the router application's responsibility.
- [x] Define non-zero 64-bit `LinkId`.
- [x] Define `RouteOrigin`: `Connected`, `Learned`, `Static`, reserved `Escape`.
- [x] Keep administrative preference separate from route metric.
- [x] Define a simple unsigned 32-bit route metric with default link metric `100`.
- [x] Reserve GCTL `ROUTER_HELLO = 0x40`.
- [x] Reserve GCTL `ROUTER_HELLO_ACK = 0x41`.
- [x] Reserve GCTL `ROUTE_ADVERTISE = 0x42`.
- [x] Define exact 32-byte wire formats for all three messages.
- [x] Implement Rust wire codecs matching the spec.
- [x] Add exact-wire, round-trip, random-ID, and malformed-input tests.

Completion gate:

- [x] GNet spec branch describes the same fields and byte layout as the Rust implementation.
- [x] Stage 1 CI passes on the exact branch head.
- [x] Existing GCTL/wire/library tests remain green.

## Stage 2 — Neighbor discovery

- [x] Use only `DOWN -> UP` initially.
- [x] Send `ROUTER_HELLO` on router links.
- [x] Reply with `ROUTER_HELLO_ACK` using the same transaction ID.
- [x] Store neighbor RouterId, remote LinkId, metric, advertised hold time, last-seen time, and state per local adjacency object.
- [x] Keep local port association outside the wire protocol by binding one adjacency object to one router-facing port/link.
- [x] Expire neighbor state after the peer-advertised hold timeout.
- [x] Allow a later valid HELLO/ACK to bring an expired neighbor back `UP`.
- [x] Reject zero hold time and self-RouterId HELLO/ACK messages.
- [x] Two-router test proves A discovers B and B discovers A without preconstructed neighbor state.
- [x] Keep route messages separate from the adjacency state machine.

Completion gate:

- [x] Two routers know only their own local identity/link configuration at startup.
- [x] Both reach `UP` using GCTL only.
- [x] Repeated HELLO refreshes liveness and peer parameters.
- [x] Hold-time expiry returns the adjacency to `DOWN`.
- [x] Stage 2-specific GCTL test suite is green.

## Stage 3 — Forward route exchange + RIB/FIB

- [x] Router advertises connected routes with `ROUTE_ADVERTISE`.
- [x] Router forwards learned routes to other neighbors.
- [x] Add local link metric when forwarding a learned route.
- [x] Apply split horizon: never advertise a learned route back to the neighbor it came from.
- [x] Store `learned_from: RouterId` for learned routes.
- [x] Prefer lower metric among equivalent learned routes.
- [x] Tie-break equal learned metrics deterministically by RouterId.
- [x] Add a RIB that can hold connected, learned, and static alternatives.
- [x] Generate a selected FIB from the RIB.
- [x] Install learned selected routes into the P4 forwarding table through `DynamicP4Router`.
- [x] Keep static routes available as overrides/fallbacks.
- [x] Reserve `Escape` but do not implement escape routing yet.
- [x] Remove manually configured cross-router routes from `router_to_router_simulation.rs`.

Completion gate:

- [x] Existing router-to-router simulation passes using learned cross-router routes.
- [x] No topology database or Dijkstra is required.
- [x] RouterEgressScheduler and DLP VC assignment remain unchanged.

## Stage 4 — Failure + reconvergence

- [x] Define `ROUTE_WITHDRAW = 0x43` with an exact 32-byte wire format and reserved-byte validation.
- [x] Keep competing learned candidates so an alternate path can already exist before failure.
- [x] Support explicit neighbor/link-down route removal.
- [x] Connect Stage 2 hold-time expiry directly to learned-route removal and P4 FIB rebuild.
- [x] Remove only routes learned through the failed/withdrawing neighbor.
- [x] Keep withdrawals neighbor-scoped and idempotent.
- [x] Generate triggered `ROUTE_ADVERTISE` or `ROUTE_WITHDRAW` updates for changed prefixes.
- [x] Re-select RIB/FIB using the retained alternate learned route.
- [x] Rebuild the P4 forwarding table before subsequent traffic.
- [x] Prove stale forwarding disappears completely when no alternate remains.
- [x] Build an A-B/C-D two-path test and prove preferred-path traffic before failure.
- [x] Withdraw B's path and prove A reconverges through C end-to-end to D.
- [x] Restore B's path and prove deterministic convergence back to the lower-cost route.

Completion gate:

- [x] `neighbor down -> route removal -> withdrawal -> RIB/FIB replacement -> traffic restored` is covered end-to-end.
- [x] A real adjacency hold timeout causes the same route/FIB transition.
- [x] No stale next hop remains usable after withdrawal; with no alternate the P4 router drops the destination.
- [x] GCTL routing/wire tests and Stable Network Simulation pass on the verified Stage 4 code head.

## Explicitly deferred

- Link-state topology database.
- SPF/Dijkstra.
- ECMP and unequal-cost multipath.
- Hold-down timers, route poisoning, and poisoned reverse.
- Congestion-aware/adaptive routing.
- Up*/down* or other deadlock-free escape topology construction.
- VC0 escape forwarding policy beyond reserving the route class/data-model hooks.
- Areas or other routing hierarchy.
- Routing authentication.
