# Dynamic topology and routing TODO

Branch: `dynamic-topology-routing`

Goal: replace primarily static cross-router forwarding with GCTL-learned topology, an explicit RIB/FIB split, and deterministic shortest-path routing. The forwarding pipeline remains `StaticP4Router -> RouterEgressScheduler -> DlpLink`; topology discovery and route computation are control-plane functions.

## Stage 1 — Spec + data model

- [ ] Track the matching GNet spec branch `dynamic-topology-routing` and do not invent wire semantics only in this repository.
- [ ] Add `RouterId` as a stable 64-bit router identity independent of interface GDP addresses.
- [ ] Add `LinkId` for router-to-router adjacency identification.
- [ ] Define route origin/source explicitly: `Connected`, `Learned`, `Static`, and reserved `Escape`.
- [ ] Define route administrative preference independently from route origin.
- [ ] Define GCTL router-adjacency messages (`ROUTER_HELLO` / acknowledgement or equivalent final spec names).
- [ ] Define topology advertisement wire model: origin router, sequence, lifetime/age, links, connected prefixes, metric/cost, capabilities as required.
- [ ] Define acceptance rules for topology advertisements: newer sequence wins, duplicates are idempotent, stale/expired entries are removed.
- [ ] Add Rust wire types/codecs and round-trip/malformed-input tests matching the spec exactly.
- [ ] Add production topology/routing data types without coupling route computation to P4.

Completion gate:

- [ ] Spec wire layout is written first and implementation matches it byte-for-byte.
- [ ] Router/link IDs, route origins, metrics, and topology advertisements have unit tests.
- [ ] Existing static router/DLP/GCTL tests remain green.

## Stage 2 — Adjacency + topology database

- [ ] Add router-neighbor state with at least `Down`, discovery/seen, and bidirectionally `Up` states.
- [ ] Exchange router identity, link identity, local reachable prefix information, metric, and hold/liveness information over GCTL.
- [ ] Learn a router-to-router neighbor from GCTL rather than preconstructing the neighbor relationship in the test.
- [ ] Build a topology database keyed by `RouterId`.
- [ ] Store each origin router's latest accepted sequence, connected prefixes, links, and expiry state.
- [ ] Flood accepted topology advertisements to other router adjacencies except the ingress adjacency.
- [ ] Prevent advertisement loops with origin+sequence duplicate suppression.
- [ ] Withdraw/expire topology state when the originating adjacency or advertisement lifetime expires.
- [ ] Add a two-router integration test: each router learns the other router and its directly connected prefixes.

Completion gate:

- [ ] Two routers start knowing only their own connected interfaces and configured identity/link settings.
- [ ] After GCTL exchange, both topology databases contain both routers and their connected prefixes.
- [ ] No static cross-router route is required to populate the topology database.

## Stage 3 — SPF + RIB/FIB

- [ ] Add an explicit RIB capable of holding competing connected, learned, and static routes.
- [ ] Add deterministic route selection using administrative preference first and path cost where appropriate.
- [ ] Implement shortest-path calculation over the topology database (initially deterministic single-next-hop SPF/Dijkstra).
- [ ] Convert learned topology prefixes into learned RIB routes with next-hop router and egress port.
- [ ] Add an explicit FIB containing only selected forwarding entries.
- [ ] Provide a control-plane update path from FIB changes into `StaticP4Router` forwarding-table state.
- [ ] Preserve static-route support as an override/fallback source rather than deleting it.
- [ ] Reserve `Escape` as a distinct future route class; do not implement VC0 escape routing in this stage.
- [ ] Replace manually configured cross-router routes in `router_to_router_simulation.rs` with learned routes.

Completion gate:

- [ ] Existing router-to-router topology passes with only connected interfaces plus GCTL topology exchange configured.
- [ ] Learned routes appear in the RIB, selected routes appear in the FIB, and P4 forwards from that FIB.
- [ ] Static-vs-learned precedence is covered by tests.
- [ ] Router egress scheduling and DLP VC assignment continue to work unchanged.

## Stage 4 — Failure + reconvergence

- [ ] Add a topology with at least two possible paths between source and destination networks.
- [ ] Establish all adjacencies and prove the deterministic preferred path before failure.
- [ ] Remove/fail one router-to-router link.
- [ ] Detect adjacency loss via explicit link-down indication and/or hold timeout according to the spec.
- [ ] Originate/flood the changed topology state.
- [ ] Remove or invalidate stale topology entries/routes.
- [ ] Re-run SPF and update RIB/FIB.
- [ ] Prove traffic reconverges onto the alternate path without manually installing a replacement route.
- [ ] Restore the link and prove deterministic convergence back to the preferred topology when appropriate.

Completion gate:

- [ ] Failure test proves: adjacency down -> topology update -> SPF recompute -> RIB/FIB replacement -> successful traffic on alternate path.
- [ ] No stale next hop remains usable after withdrawal.
- [ ] Existing P4, GCTL, scheduler, DLP, and stable-network CI remain green.

## Explicitly deferred

- ECMP and unequal-cost multipath.
- Congestion-aware/adaptive routing.
- Up*/down* or other deadlock-free escape topology construction.
- VC0 escape forwarding policy beyond reserving the route class/data-model hooks.
- Areas, route reflectors, designated-router concepts, or other large-network hierarchy.
- Authentication/cryptographic routing security.
