# Routing TODO

This document defines the routing work to do after the point-to-point smolgnet baseline.

The goal is to retain the useful routing behavior that existed in smoltcp, but express it in native GNet terms rather than carrying IP/Ethernet assumptions forward.

Dynamic route exchange is deliberately not part of the first routing milestone. The initial implementation is connected routes + manually configured static/default routes + GDP forwarding. A future routing protocol or routing daemon must be able to populate the same route table without changing GDP forwarding.

## 1. GNet route types

- [ ] Add a canonical 64-bit `GdpPrefix` / `GdpCidr` type.
  - prefix lengths 0..=64
  - canonical host bits cleared
  - `contains(GdpAddress)`
  - ordering/equality suitable for deterministic tests
- [ ] Add `Route`.
  - destination prefix
  - optional next-hop GDP address
  - output interface/link handle
  - administrative/static origin metadata
  - `preferred_until: Option<Instant>`
  - `expires_at: Option<Instant>`
- [ ] Add `RouteTableFull` or equivalent bounded-capacity error.
- [ ] Support both caller-bounded/no-alloc storage and alloc-backed convenience storage.

## 2. Preserve the useful smoltcp route-table behavior

smoltcp's route table provides arbitrary CIDR routes, default gateways, bounded storage, route mutation, expiry fields, and longest-prefix lookup. GNet should keep those capabilities with one 64-bit address family.

- [ ] Empty route table constructor.
- [ ] Fixed maximum route count for heapless targets.
- [ ] Optional zero-route configuration for very small endpoints.
- [ ] Bulk/update API so callers can edit route storage without repeated allocations.
- [ ] Add arbitrary static prefix routes.
- [ ] Remove arbitrary routes.
- [ ] Add default route (`0/0`).
- [ ] Get current default route.
- [ ] Replace current default route and return the old route.
- [ ] Remove default route and return the old route.
- [ ] Longest-prefix match.
- [ ] Ignore expired routes during lookup.
- [ ] Define exact semantics of `preferred_until`.
  - likely prefer non-deprecated routes among otherwise equivalent candidates
  - never override longer-prefix matching
- [ ] Deterministic tie breaking for equal prefix length.
- [ ] Tests equivalent to smoltcp route lookup/default/expiry tests using 64-bit GNet addresses.

## 3. GNet-specific direct/connected routes

GNet does not have an Ethernet/ARP neighbor layer. A route therefore resolves to an output link and, where needed, a GDP next hop rather than to a MAC neighbor-cache lookup.

- [ ] Represent directly connected GDP prefixes per interface.
- [ ] Directly connected route wins before less-specific static/default routes.
- [ ] Allow point-to-point links to have an explicit peer/router address.
- [ ] Define `next_hop = destination` for directly reachable destinations where appropriate.
- [ ] Define `next_hop = route.via_router` for routed destinations.
- [ ] No ARP/neighbor cache compatibility layer.
- [ ] No Ethernet MAC address in routing APIs.

## 4. Multiple interfaces

smoltcp's normal route lookup is largely interface-local. A useful GNet router must select among several native links.

- [ ] Introduce stable interface/link handles.
- [ ] Per-interface canonical local GDP address(es).
- [ ] Per-interface link-local address.
- [ ] Per-interface optional Local-GDP /48 context.
- [ ] Per-interface supported Size Classes/path constraints.
- [ ] Per-interface link state.
- [ ] Per-interface point-to-point peer information where applicable.
- [ ] Route result contains output interface + next hop.
- [ ] Route removal when an interface disappears or changes identity.

## 5. Source-address selection

Preserve the useful smoltcp behavior of selecting an appropriate source address for outgoing traffic, but use GNet's canonical address model.

- [ ] Prefer the source identity assigned to the selected output interface.
- [ ] Preserve an explicitly selected valid local source address.
- [ ] Define behavior for router-generated GCTL errors when several addresses are available.
- [ ] Local GDP encoding is selected only after the canonical source/destination and output interface are known.

## 6. Local delivery versus forwarding

Do not copy smoltcp `AnyIP` literally. Split its useful behavior into explicit GNet concepts.

- [ ] Exact local-address delivery.
- [ ] Explicit prefix-local delivery option for virtual/service/router-owned prefixes.
  - this is the GNet replacement for the useful part of smoltcp AnyIP
  - disabled by default
  - must not accidentally turn all routed traffic into local traffic
- [ ] Packets not locally owned enter the forwarding path only when forwarding is enabled.
- [ ] Endpoint-only builds can keep forwarding disabled and omit route storage entirely.

## 7. GDP forwarding pipeline

- [ ] Decode enough GDP header to obtain canonical source/destination, Size Class, address form and Hop Limit.
- [ ] Validate GDP header CRC before route commitment.
- [ ] Decide local delivery versus forwarding.
- [ ] Longest-prefix route lookup on canonical 64-bit destination.
- [ ] Select egress interface and next hop.
- [ ] Decrement Hop Limit on forwarded packets.
- [ ] Do not recalculate GDP CRC merely because Hop Limit changed; Hop Limit is intentionally excluded from CRC-8-GNET.
- [ ] Re-encode Global versus Local GDP form for the egress link according to that link's configured context.
- [ ] Preserve canonical endpoint identities across Local/Global re-encoding.
- [ ] Preserve GDP Type and Size Class unchanged unless a future protocol explicitly defines otherwise.

## 8. Forwarding errors / GCTL

- [ ] No route -> `DESTINATION_UNREACHABLE`.
- [ ] Hop Limit exhausted -> `HOP_LIMIT_EXCEEDED`.
- [ ] Invalid header/parameter -> `PARAMETER_PROBLEM` where recovery permits.
- [ ] Egress cannot carry the requested Size Class -> `CLASS_UNSUPPORTED`.
- [ ] Forwarding/link failure after acceptance -> determine when `TRANSIT_ABORTED` is appropriate.
- [ ] Never generate an error in response to an error when that would create loops.
- [ ] Rate-limit router-generated control errors.

## 9. Size Class and path behavior

GDP has no fragmentation, so routing must not inherit smoltcp's fragmentation machinery.

- [ ] No fragmentation/reassembly layer.
- [ ] Route/interface lookup can reject unsupported packet Size Classes.
- [ ] Define whether a route may advertise a maximum supported Size Class as cached path information.
- [ ] Keep path Size-Class discovery as a separate GCTL concern rather than silently fragmenting.
- [ ] Tests for forwarding 64B..8192B classes across interfaces with different support.

## 10. Scheduler and route lifetime integration

- [ ] Route expiry participates in `poll_at` / `poll_delay` when necessary.
- [ ] Expired routes need not be eagerly removed if lookup can ignore them safely.
- [ ] Provide explicit pruning for long-running routers.
- [ ] Route-table changes wake blocked transmitters where async support is enabled.

## 11. Hosted TAP routing tests

Use the TAP adapter only as a host test transport. TAP/Ethernet headers are never visible to GDP routing.

- [ ] Two-interface software router process with one TAP per GNet interface.
- [ ] Endpoint A -> router -> Endpoint B forwarding test.
- [ ] Verify longest-prefix selection with two possible egress links.
- [ ] Verify default route.
- [ ] Verify no-route GCTL error.
- [ ] Verify Hop Limit decrement/exhaustion.
- [ ] Verify Local GDP on one side can forward as Global GDP on another side and retain canonical identities.
- [ ] Verify GTS tunnel traffic survives packet-by-packet routing.
- [ ] Verify reliable GTS retransmission through a routed loss/fault scenario.

## 12. Router-facing API

Keep the forwarding engine usable by small embedded routers, Unix test programs, Cosmic OS, and future QDX/PLIO hardware.

- [ ] Separate RIB-like route configuration from fast forwarding lookup.
- [ ] Small immutable/compiled FIB representation where useful.
- [ ] Route add/replace/delete/query API.
- [ ] Interface add/remove/up/down API.
- [ ] Counters for packets/flits forwarded, dropped, no-route, hop-limit and class errors.
- [ ] Tracer events for ingress route decision, egress selection and forwarding error.
- [ ] No requirement for a background routing daemon.

## 13. Future dynamic routing hook

Not part of the initial routing implementation, but the static design must leave a clean insertion point.

- [ ] Route origin/owner field so a future protocol can install and withdraw routes.
- [ ] Administrative preference/metric field only if needed by multiple route producers.
- [ ] Atomic route replacement/update API.
- [ ] Independent routing daemon can populate routes without becoming part of GDP.
- [ ] Do not embed a specific dynamic routing algorithm into the forwarding engine.

## Explicit non-goals for the first routing milestone

- raw GDP application sockets
- Ethernet bridging
- ARP or MAC neighbor discovery
- packet fragmentation/reassembly
- multicast/group routing
- dynamic routing protocol
- NAT-like address rewriting

The first target should be deliberately small:

```text
multiple GNet interfaces
        +
connected/static/default routes
        +
64-bit longest-prefix lookup
        +
GDP Hop Limit forwarding
        +
GCTL forwarding errors
        +
TAP-based routed integration tests
```

Once this works reliably, dynamic routing can be reconsidered as a separate protocol/control-plane component.
