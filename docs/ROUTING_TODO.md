# Routing TODO

This document defines the intentionally small routing support for `smolgnet`.

The goal is **not** to turn `smolgnet` into the GNet router implementation. For now, routing should stay close to smoltcp's model: a small bounded table used by endpoints or simple test setups to choose a next-hop router for a destination.

A future dedicated router implementation / routing daemon can build a richer RIB/FIB, multiple-interface forwarding, dynamic routing, policy, metrics, and hardware programming outside `smolgnet`.

## Scope for the current milestone

Keep only:

```text
GDP destination
      |
small bounded route table
      |
longest-prefix match
      |
next-hop GDP router
```

No multi-interface forwarding engine is required here.

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

Match smoltcp's useful route fields as closely as practical:

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

This is intentionally simpler than an alloc-backed RIB. If a future router needs large/dynamic tables, that should live in router-specific code rather than making the core endpoint route table complicated.

## 4. Lookup behavior

- [x] Ignore expired routes.
- [x] Match routes whose prefix contains the destination.
- [x] Choose the matching route with the longest prefix.
- [x] Return the selected next-hop GDP address.
- [x] Fall back to `/0` when no more-specific route matches.
- [x] Keep equal-prefix tie behavior simple and deterministic.
- [x] Unit tests for longest-prefix lookup.
- [x] Unit tests for expiry fallback.

Do not add administrative distance, route origin, metrics, ECMP, policy routing, or per-interface selection to this table yet.

## 5. Default-route helpers

Equivalent to smoltcp's default-gateway convenience API:

- [x] Add default route.
- [x] Return the previous default route when replacing it.
- [x] Get current default route.
- [x] Remove current default route.
- [x] Test replacement/removal behavior.

## 6. Endpoint integration

- [x] Give `Endpoint` a small fixed-capacity route table.
- [x] Preserve direct point-to-point behavior when the route table is empty.
- [x] Allow an endpoint to configure a default router through `routes_mut()`.
- [x] Allow an endpoint to add a more-specific static prefix route.
- [x] Add endpoint-level next-hop lookup with `Endpoint::route(destination, now)`.
- [x] Keep route lookup outside GTS semantics.
- [x] Test more-specific route overriding default.
- [x] Test empty/no-route behavior.
- [ ] Integrate route selection into packet TX once direct/on-link destination semantics are explicitly defined.
- [ ] Keep the packet's GDP destination unchanged when TX starts using a selected next hop.
- [ ] Add an endpoint-level expiry fallback test.

The current route API is intentionally advisory: `Endpoint::route()` returns a configured next-hop router, while ordinary point-to-point transmission remains unchanged. This avoids inventing an on-link-prefix rule just to force route lookup into TX.

## 7. Route-table mutation API

Keep this small and endpoint-oriented.

- [x] Expose direct immutable/mutable access with `routes()` / `routes_mut()`.
- [ ] Decide whether a smoltcp-style `update` closure adds enough value beyond direct bounded-table access.
- [ ] If useful, add a controlled `update` closure API without allocation.
- [ ] Add tests ensuring mutation cannot corrupt `len` bookkeeping.

Do not add a large route-management API until a real user requires it.

## 8. Timing integration

- [ ] Decide whether route expiry needs to participate in endpoint `poll_at` / `poll_delay`.
- [ ] If not, document that expired routes are lazily ignored and optionally pruned by callers.
- [ ] If yes, schedule only the earliest route expiry; do not add a general routing timer subsystem.

## 9. Higher-level routed tests

Only add enough test infrastructure to verify that normal endpoint traffic can use a configured next-hop.

- [ ] Two endpoints with a simple forwarding test helper between them.
- [ ] Verify GDP destination identity is preserved while next-hop selection changes.
- [ ] Verify ordinary GTS traffic is unaffected by endpoint route lookup.
- [ ] Verify a GTS request/response can traverse a simple routed test topology once forwarding support exists elsewhere.

The forwarding helper may remain test-only. It is not evidence that `smolgnet` should grow into the production router implementation.

## Explicitly deferred / out of scope

The following belong in a future dedicated router/control-plane implementation unless a very small compatibility hook is required in `smolgnet`:

- multi-interface router object;
- interface handles in the route table;
- connected-route management;
- packet forwarding between interfaces;
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

The minimal smoltcp-like routing primitives exist in `src/routing.rs`, and `Endpoint` now owns a small route table:

```text
GdpPrefix
Route
RouteTable<N>
RouteTableFull
Endpoint::routes()
Endpoint::routes_mut()
Endpoint::route()
```

The next routing-specific step is to define **on-link/direct-destination semantics** before allowing transmit paths to automatically substitute a configured next hop.
