# Advanced GNet Router Design

Status: deferred design notes beyond the fixed two-port static router implemented by PR #15.

PR #15 intentionally proves only the smallest useful router datapath:

```text
Port 0 <--> [ P4 GDP router ] <--> Port 1
                 |
             static FIB
```

The first router has exactly two physical ports, assumes both links are available, uses startup-only static routes, and forwards complete GDP packets. The ideas below are intentionally not part of that milestone.

## Development sequence

The next independent hardware-style milestone is an **8-port GNet switch**. Only after both components work separately should they be combined:

```text
2-port router       8-port switch
     |                    |
     +------ later -------+
```

This keeps routing failures, switching failures, and link-control failures separately testable.

## Port and link lifecycle

A later router should replace the PR #15 static-link assumption with explicit per-port state:

```text
DOWN -> LINK_UP -> REGISTERING -> READY
```

Future behavior:

- consume DLP/GLCP link-up and reset indications;
- keep DLP credits, VC state, flits, and partial reassembly below the router boundary;
- clear ephemeral management state when a link is reset;
- disable routes whose egress depends on a failed port;
- reprogram the forwarding plane atomically;
- restore registration and adjacency state when the link returns.

The router boundary remains:

```text
DLP -> complete GDP packet + ingress port -> router
router -> complete GDP packet + egress port -> DLP
```

## GS switch registration

When a router port is attached to a GNet switch, the router should identify itself after link establishment.

Proposed sequence:

```text
link established
    -> ROUTER_PRESENT(router-link-local)
    -> ROUTER_PRESENT_ACK
    -> port READY
```

Rules:

- switch registration is required only for GS-attached ports;
- a coupler/direct point-to-point link does not perform GS registration;
- reset or link loss invalidates registration;
- switch registration must not leak into the P4 GDP forwarding rules.

## CPU punt and management plane

The P4 fast path should eventually attach an explicit punt reason rather than treating every CPU delivery identically.

Likely reasons include:

- bootstrap/router discovery;
- packet addressed to the router itself;
- GCTL service requiring router management;
- Hop-Limit expiry if an error response is required;
- unsupported/invalid forwarding parameters;
- management diagnostics.

Ordinary transit traffic should remain entirely in the forwarding plane.

## Router discovery

A future client-facing router interface can expose the frozen GCTL bootstrap sequence:

```text
SOLICIT(Router)
  -> ADVERTISE(Router)
  -> ADDRESS_OFFER
  -> ADDRESS_CLAIM
  -> ADDRESS_ACK / ADDRESS_NAK
```

The initial solicitation uses the reserved bootstrap destination. Transactions should be bounded and keyed by values such as:

```text
(port, transaction_id, client_link_local)
```

Stale transactions must expire deterministically.

## Address authority

An address-authority interface may own a configured routed prefix such as `/16`, `/32`, `/48`, or `/56`.

The allocator should never issue:

- the router's own routed address;
- an active lease;
- an outstanding offered address;
- reserved bootstrap/link-local space.

`ADDRESS_CLAIM` must match the original port, transaction, offered candidate, client identity, and offer lifetime before the router commits the address.

## Node and lease database

Future management state can record:

- client link-local address;
- routed address;
- router interface;
- lease state and expiry;
- bootstrap transaction/nonce information;
- generation or last-update information.

This remains control-plane state. A router should not install a per-host P4 route when a connected prefix already forwards the complete LAN toward its switch.

## Router and switch address convergence

After address assignment, the switch may need its own local mapping update such as `ADDRESS_ANNOUNCE`.

The router lease database and switch forwarding database are separate authorities:

- router owns address assignment;
- switch owns local egress-port mapping;
- a switch mapping failure is a management fault, not permission to corrupt the router lease state;
- readdressing and lease expiry must converge both views.

## Live static-route management

A later static router can support controlled route changes without introducing a routing protocol:

- add/replace/remove static routes;
- enable or disable routes according to port state;
- retain backup candidates for a prefix;
- atomically update/rebuild P4 tables;
- expose a monotonically increasing FIB generation;
- keep administrator configuration/RIB state separate from compiled P4 entries.

No route advertisements are implied by this feature.

## Adjacency and next hop

Route and adjacency should remain distinct concepts:

```text
Route(prefix)
    -> Adjacency
        -> egress port
        -> optional next-hop GDP address
```

The final GDP destination is never rewritten to the next hop. A directly connected route can use an interface adjacency; an off-link static route can name a next-hop router on that interface.

PR #15 retains `next_hop` only as validated link-local metadata. It does not yet implement an adjacency-resolution protocol.

## Fast-path extensions

Potential later P4 work:

- forwarding-field validation;
- per-port counters;
- per-route counters where practical;
- QoS/queue classification;
- CPU/control queue backpressure;
- explicit error/punt metadata;
- atomic table generations;
- eventually more than two physical router ports if a real product requires it.

The current two-port implementation should not be generalized prematurely merely because the software model can support it.

## 8-port switch milestone

The next separate component should be a fixed **8-port GS switch**.

Initial target:

```text
P0 --\
P1 ---\
P2 ----\
P3 ----- [ GS switch ]
P4 ----/
P5 ---/
P6 --/
P7 -/
```

The switch should be tested independently from the router. It should eventually cover:

- exactly eight physical ports;
- GS3 ingress/egress forwarding state;
- independent ingress and egress credit handling;
- local node/address mapping;
- router-port identification;
- link reset and relearning;
- bounded deterministic simulation tests.

GC3 remains credit-transparent/stateless where specified; link-local credit behavior must stay below the GDP routing layer.

## Router + switch composition

Only after the two-port router and eight-port switch are independently green should the simulation compose them.

First combined topology:

```text
hosts -- 8-port GS -- router port 0
                       router port 1 -- remote link/network
```

Useful combined tests:

- switch recognizes router attachment;
- host traffic reaches the router through the switch;
- router forwards GDP to its opposite port;
- return traffic crosses the router and is delivered by the switch to the correct host;
- Hop Limit changes only at the router;
- switch forwarding never rewrites GDP destination;
- router never sees DLP/GS credit implementation details;
- reset/re-registration remains bounded and diagnosable.

## Later multi-router simulation

After router+switch composition works:

```text
LAN A -- GS -- Router A ==== Router B -- GS -- LAN B
```

Both routers can initially use static routes. This can validate GTS traffic across multiple GDP hops without introducing a dynamic routing protocol.

## Persistence and administration

Deferred until the runtime model is stable:

- configuration file format;
- boot-time parser;
- persistent leases if useful;
- atomic configuration replacement;
- administrative CLI/API;
- diagnostics and status export.

## Explicit non-goals for the near-term work

- dynamic router-to-router route exchange;
- distance-vector or link-state routing protocol;
- automatic inter-domain routing;
- NAT/address rewriting;
- Ethernet bridging/ARP inside GDP routing;
- DLP credit/VC implementation inside the router;
- combining router and switch before each component works independently.
