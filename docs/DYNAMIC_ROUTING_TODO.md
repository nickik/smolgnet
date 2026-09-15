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
- [ ] Stage 1 CI passes on the exact branch head.
- [ ] Existing GCTL/wire/library tests remain green.

## Stage 2 — Neighbor discovery

- [ ] Use only `DOWN -> UP` initially.
- [ ] Send `ROUTER_HELLO` on router links.
- [ ] Reply with `ROUTER_HELLO_ACK`.
- [ ] Store neighbor RouterId, local port, local/remote LinkId, neighbor GDP address, metric, and hold timer.
- [ ] Expire neighbor state after the negotiated/simple hold timeout.
- [ ] Two-router test proves A discovers B and B discovers A without preconstructed neighbor state.

Completion gate:

- [ ] Two routers know only their own local identity/link configuration at startup.
- [ ] Both reach `UP` using GCTL only.

## Stage 3 — Forward route exchange + RIB/FIB

- [ ] Router advertises connected routes with `ROUTE_ADVERTISE`.
- [ ] Router forwards learned routes to other neighbors.
- [ ] Add local link metric when forwarding a learned route.
- [ ] Apply split horizon: never advertise a learned route back to the neighbor it came from.
- [ ] Store `learned_from: RouterId` for learned routes.
- [ ] Prefer lower metric among equivalent learned routes.
- [ ] Tie-break equal learned metrics deterministically by RouterId.
- [ ] Add a RIB that can hold connected, learned, and static alternatives.
- [ ] Generate a selected FIB from the RIB.
- [ ] Install FIB changes into the P4 forwarding table.
- [ ] Keep static routes available as overrides/fallbacks.
- [ ] Reserve `Escape` but do not implement escape routing yet.
- [ ] Remove manually configured cross-router routes from `router_to_router_simulation.rs`.

Completion gate:

- [ ] Existing router-to-router simulation passes using learned cross-router routes.
- [ ] No topology database or Dijkstra is required.
- [ ] RouterEgressScheduler and DLP VC assignment remain unchanged.

## Stage 4 — Failure + reconvergence

- [ ] Define a simple `ROUTE_WITHDRAW` wire message before implementing failure handling.
- [ ] Build a topology with two possible paths.
- [ ] Fail one router-to-router link.
- [ ] Neighbor hold timeout marks the adjacency down.
- [ ] Remove routes learned through that neighbor.
- [ ] Propagate withdrawal.
- [ ] Re-select RIB/FIB using the alternate learned route.
- [ ] Prove traffic succeeds over the alternate path.
- [ ] Restore the link and prove deterministic convergence again.

Completion gate:

- [ ] `neighbor down -> route removal -> withdrawal -> RIB/FIB replacement -> traffic restored` is covered end-to-end.
- [ ] No stale next hop remains usable after withdrawal.

## Explicitly deferred

- Link-state topology database.
- SPF/Dijkstra.
- ECMP and unequal-cost multipath.
- Congestion-aware/adaptive routing.
- Up*/down* or other deadlock-free escape topology construction.
- VC0 escape forwarding policy beyond reserving the route class/data-model hooks.
- Areas or other routing hierarchy.
- Routing authentication.
