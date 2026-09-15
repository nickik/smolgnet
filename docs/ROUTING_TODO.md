# Routing TODO

This document defines the intentionally small routing support for `smolgnet`.

The goal is **not** to turn `smolgnet` into the GNet router implementation. Routing stays close to smoltcp's model: a small bounded table used by endpoints or simple test setups to choose a next-hop router for a destination.

A future dedicated router implementation / routing daemon can build a richer RIB/FIB, multiple-interface forwarding, dynamic routing, policy, metrics, and hardware programming outside `smolgnet`.

## Scope for the current milestone

```text
GDP destination
      |
small bounded route table
      |
longest-prefix match
      |
next-hop GDP router
```

No production multi-interface forwarding engine is required here.

## 1. GDP prefix type

- [x] Add canonical 64-bit `GdpPrefix`.
  - [x] Prefix lengths `0..=64`.
  - [x] Clear host bits on construction.
  - [x] `contains(GdpAddress)`.
  - [x] Deterministic equality/order.
  - [x] `/0` default prefix helper.
- [x] Reject invalid prefix lengths.
- [x] Unit tests for canonicalization and matching.

## 2. Minimal Route type

```text
Route
    destination prefix
    via router
    preferred_until
    expires_at
```

- [x] Add `Route` with destination `GdpPrefix`.
- [x] Add `via_router: GdpAddress`.
- [x] Add `preferred_until: Option<Instant>`.
- [x] Add `expires_at: Option<Instant>`.
- [x] Add normal route constructor.
- [x] Add default-route constructor.
- [x] Add default-route identification helper.

`preferred_until` is retained for compatibility with the smoltcp-style route model, but the initial lookup algorithm does not use it. This is also how current smoltcp behaves.

## 3. Bounded route table

- [x] Add fixed-capacity `RouteTable<const N: usize>`.
- [x] Support `N = 0` for configurations that need no routes.
- [x] No allocator required.
- [x] Add `len`, `capacity`, and `is_empty`.
- [x] Add route insertion.
- [x] Return `RouteTableFull` when bounded storage is full.
- [x] Add route removal by destination prefix.
- [x] Add explicit expired-route pruning.

## 4. Lookup behavior

- [x] Ignore expired routes.
- [x] Match routes whose prefix contains the destination.
- [x] Choose the matching route with the longest prefix.
- [x] Return the selected next-hop GDP address.
- [x] Fall back to `/0` when no more-specific route matches.
- [x] Keep equal-prefix tie behavior simple and deterministic.
- [x] Unit tests for longest-prefix lookup.
- [x] Unit tests for expiry fallback.

Do not add administrative distance, route origin, metrics, ECMP, policy routing, or per-interface selection to this table.

## 5. Default-route helpers

- [x] Add default route.
- [x] Return the previous default route when replacing it.
- [x] Get current default route.
- [x] Remove current default route.
- [x] Test replacement/removal behavior.

## 6. Endpoint integration

- [x] Give `Endpoint` a small fixed-capacity route table.
- [x] Preserve legacy point-to-point direct behavior when the route table is empty.
- [x] Once a route is configured, make the route table authoritative.
- [x] Allow an endpoint to configure a default router through `routes_mut()`.
- [x] Allow an endpoint to add a more-specific static prefix route.
- [x] Add configured-route lookup with `Endpoint::route(destination, now)`.
- [x] Add final next-hop selection with `Endpoint::next_hop(destination, now)`.
- [x] Add `Error::NoRoute` for an authoritative table with no match.
- [x] Keep route lookup outside GTS semantics.
- [x] Keep the packet's GDP destination unchanged; route selection only authorizes/selects the adjacent peer.
- [x] Add explicit-time route-aware send helpers:
  - [x] `connect_at(...)`
  - [x] `send_gctl_at(...)`
  - [x] `send_echo_at(...)`
- [x] Endpoint tests for:
  - [x] direct send with no routing involved;
  - [x] default-router selection;
  - [x] more-specific route overriding default;
  - [x] expired specific route falling back to default;
  - [x] no route available.

### Direct versus routed rule

```text
route table empty
    -> legacy point-to-point mode
    -> destination itself is the next hop

route table non-empty
    -> routing enabled
    -> table is authoritative
    -> longest-prefix route selects next hop
    -> no match means Error::NoRoute
```

If a destination should be considered directly reachable while routing is enabled, install a `/64` route with `via_router` equal to that destination.

Because current DLP is point-to-point, there is no additional link-layer next-hop field. The selected next hop identifies the adjacent peer logically; the encoded GDP destination always remains the final destination.

## 7. Route-table mutation API

- [x] Expose direct immutable/mutable access with `routes()` / `routes_mut()`.
- [ ] Add a smoltcp-style `update` closure only if a real caller benefits from it.

Direct bounded-table access is sufficient for the current milestone.

## 8. Timing integration

- [x] Route expiry participates in endpoint `poll_at` / `poll_delay`.
- [x] `RouteTable::next_expiry()` reports the earliest finite expiry.
- [x] An earlier route expiry preempts the conservative GTS retransmission timer.
- [x] `tick_at` prunes expired routes.
- [x] Route pruning counts as endpoint work so async runtimes can wake transmitters after routing changes.
- [x] Tests cover route-expiry scheduling and pruning.

No separate routing timer subsystem is introduced.

## 9. Higher-level routed tests

These remain test-only and deliberately do not introduce a production router object into `smolgnet`.

- [x] Add deterministic routed-topology harness using native `GnetFrame` GDP frames.
- [x] Two-hop topology: `Host A -> R1 -> R2 -> Host B`.
- [x] Real GTS connect/request/reply across two GDP forwarders.
- [x] Longest-prefix route selection.
- [x] Default-route fallback.
- [x] Hop-Limit exhaustion.
- [x] No-route drop.
- [x] Route-expiry fallback to an alternate/default path.
- [x] Diamond topology failover: `R1-R2-R4` to `R1-R3-R4`.
- [x] Verify source/destination identities survive multiple forwarding hops.
- [x] Verify Hop Limit decrements once per router.
- [x] Drop one routed reliable GTS frame and verify retransmission succeeds after RTO.
- [x] Verify Local GDP ingress can be converted to Global GDP egress without changing canonical identities.
- [x] Keep the forwarding helpers under `tests/`; they are not production router code.

Repeatable test files:

```text
tests/routed_topology.rs
tests/routed_topology_stress.rs
```

## Explicitly deferred / out of scope

- production multi-interface router object;
- interface handles in the endpoint route table;
- connected-route management;
- production packet forwarding between interfaces;
- route origin metadata;
- administrative preference;
- route metrics;
- ECMP/load balancing;
- source routing;
- policy routing;
- dynamic route exchange;
- routing protocols;
- RIB/FIB separation;
- route redistribution;
- GCTL routing errors generated by a router;
- path Size-Class discovery;
- router counters/telemetry;
- hardware/P4 route programming;
- GRouterD integration;
- multicast/group routing;
- NAT-like rewriting;
- fragmentation/reassembly;
- Ethernet bridging, ARP, or MAC neighbor discovery.

## Current status

The minimal smoltcp-like routing milestone and deterministic routed integration tests are complete in `smolgnet`:

```text
GdpPrefix
Route
RouteTable<N>
RouteTableFull
Endpoint::routes()
Endpoint::routes_mut()
Endpoint::route()
Endpoint::next_hop()
Endpoint::{connect_at,send_gctl_at,send_echo_at}
route-expiry poll_at integration
routed topology + failover + loss tests
```

Further routing work should happen in a dedicated forwarding/router component unless a concrete endpoint use case requires another small compatibility hook.
