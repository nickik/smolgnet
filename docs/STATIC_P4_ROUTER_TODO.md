# Static P4 Router TODO

Status: design/implementation plan for the first production-style GNet router in `smolgnet`.

This router is deliberately **static-routing only**. Routers do not exchange routes with neighboring routers. The complete route configuration is supplied to Rust management code at startup. File parsing/persistent configuration is deferred; tests and simulations construct the startup configuration directly in memory.

The architectural split is:

```text
                         Rust management plane
                  startup config / ports / nodes
                    address authority / switch state
                               |
                               | compile/program
                               v
 DLP/GDP packet ---> P4 GDP fast path ---> selected egress port
                           |
                           +---- punt ---> Rust management plane
```

P4 owns the per-packet forwarding fast path. Rust owns state, protocol transactions and table programming.

## Fixed design rules

- Routers exchange **no routing protocol** in this milestone.
- Static routes appear in memory at router startup.
- P4 performs global GDP longest-prefix forwarding and Hop-Limit processing.
- Rust does not perform ordinary transit route lookup packet-by-packet.
- Rust handles bootstrap/control packets punted by P4.
- The GDP destination remains the final destination; routing does not rewrite it.
- DLP remains outside the router management design. The router consumes/emits complete GDP packets plus ingress/egress port identity.
- A directly attached GS switch is informed that the attached endpoint is a router using GLCP `ROUTER_PRESENT` after link establishment.
- Host discovery/address configuration uses the frozen GCTL sequence:

```text
SOLICIT(Router)
  -> ADVERTISE(Router)
  -> ADDRESS_OFFER
  -> ADDRESS_CLAIM
  -> ADDRESS_ACK / ADDRESS_NAK
```

- The router records configured nodes and address leases even when the local switch handles the final address-to-switch-port mapping.

## 1. Startup model + P4 FIB skeleton

- [x] Create a dedicated static-router module from `main`.
- [x] Define in-memory startup configuration:
  - [x] router interfaces/ports;
  - [x] per-interface router link-local address;
  - [x] optional connected routed prefix;
  - [x] optional router routed address;
  - [x] static routes;
  - [x] optional next-hop metadata.
- [x] Validate startup configuration before enabling forwarding.
- [x] Build Rust management state from startup configuration.
- [x] Add node table types even though protocol-driven population comes later.
- [x] Add per-port switch-registration state.
- [x] Add P4 fast-path program with:
  - [x] 64-bit global GDP LPM;
  - [x] `/0` default route;
  - [x] `/1..32` and `/33..64` prefixes;
  - [x] CPU punt action;
  - [x] transit Hop-Limit decrement/expiry;
  - [x] no GDP destination rewrite.
- [x] Compile connected/static routes and router-local addresses into the P4 tables at startup.
- [x] Punt the reserved bootstrap GDP address and router-owned addresses to Rust.
- [x] Add deterministic tests proving startup config controls P4 forwarding.

The first implementation stops here. Later items build on this state model instead of inventing a second router representation.

## 2. Port/link lifecycle and switch registration

- [ ] Add explicit per-port lifecycle:

```text
DOWN -> LINK_UP -> REGISTERING -> READY
```

- [ ] Consume a link-up indication from the eventual DLP/GLCP implementation.
- [ ] For GS-attached ports, emit GLCP `ROUTER_PRESENT(router-link-local)`.
- [ ] Parse/validate `ROUTER_PRESENT_ACK`.
- [ ] Mark the switch registration active only after ACK.
- [ ] On link-down/reset:
  - [ ] clear switch registration;
  - [ ] clear ephemeral node/transaction state for that interface;
  - [ ] disable routes whose egress depends on the failed port;
  - [ ] reprogram P4 atomically.
- [ ] Coupler-attached ports must not attempt switch registration.

## 3. CPU-punt/control-plane packet path

- [ ] Define P4 punt reasons/metadata rather than treating every punt identically.
- [ ] Punt at least:
  - [ ] bootstrap destination `FE80:0000:0000:0000`;
  - [ ] router-owned GDP addresses;
  - [ ] GCTL packets that require router management;
  - [ ] route/parameter exceptions that require a generated GCTL error.
- [ ] Rust management dispatcher parses the punted complete GDP packet.
- [ ] Rust-generated replies are reinjected with an explicit egress port.
- [ ] Keep ordinary transit packets entirely out of Rust.

## 4. Router discovery service

- [ ] On each READY client-facing interface, accept link-scoped `SOLICIT(Router)`.
- [ ] Require the reserved bootstrap destination for initial solicitation.
- [ ] Return `ADVERTISE(Router)` directly to the soliciting link-local source.
- [ ] Echo Transaction ID and use the interface's router provider address.
- [ ] Maintain bounded transaction state keyed by `(port, transaction_id, client_link_local)`.
- [ ] Reject/expire stale transactions deterministically.

## 5. Address authority / allocator

- [ ] Give each address-authority interface one configured `/16`, `/32`, `/48`, or `/56` prefix.
- [ ] Deterministically allocate a free candidate address from that prefix.
- [ ] Never allocate:
  - [ ] the router's own routed address;
  - [ ] currently leased addresses;
  - [ ] outstanding offered addresses;
  - [ ] reserved bootstrap/link-local space.
- [ ] Emit `ADDRESS_OFFER` after `ADVERTISE` (back-to-back is allowed).
- [ ] Validate `ADDRESS_CLAIM` against:
  - [ ] ingress port;
  - [ ] transaction ID;
  - [ ] offered candidate;
  - [ ] client link-local identity;
  - [ ] current offer lifetime.
- [ ] Commit with `ADDRESS_ACK` or reject with `ADDRESS_NAK`.
- [ ] Record lease lifetime and claim nonce.

## 6. Node/lease database

- [ ] Maintain a router node record containing at least:
  - [ ] link-local address;
  - [ ] routed address;
  - [ ] router interface/port;
  - [ ] lease state;
  - [ ] transaction/nonce information needed for bootstrap safety;
  - [ ] lease expiry;
  - [ ] last update generation/time where useful.
- [ ] Index by routed address and link-local address without duplicating authority state.
- [ ] Support lease renewal/reconfiguration.
- [ ] Remove expired leases.
- [ ] A node record is management state; do not add per-host P4 routes when a connected prefix already forwards the whole LAN toward its switch.

## 7. Address-change and switch-update integration

- [ ] Model the host's subsequent GLCP `ADDRESS_ANNOUNCE` as switch-local state, not a router routing message.
- [ ] Provide simulation hooks so the switch can confirm the newly configured node address.
- [ ] Treat switch mapping failure as a management fault without corrupting the router lease database.
- [ ] Define readdress/lease-expiry behavior so router and switch converge on the same active addresses.

## 8. Static route management after startup

Routes remain administrator supplied, but Rust should support controlled changes without restarting:

- [ ] add/replace/remove a static route;
- [ ] enable/disable a route when its port changes state;
- [ ] preserve preference/backup candidates for the same prefix;
- [ ] atomically rebuild or update P4 tables;
- [ ] expose a monotonically increasing FIB generation;
- [ ] keep the RIB/config representation separate from compiled P4 entries.

No neighbor route advertisements or dynamic route learning are added.

## 9. Next-hop / adjacency state

- [ ] Keep route and adjacency distinct:

```text
Route(prefix) -> Adjacency -> egress port + optional next-hop GDP address
```

- [ ] Direct connected prefixes use an interface adjacency.
- [ ] Static off-link routes may name a specific next-hop router.
- [ ] Do not rewrite the GDP destination to the next hop.
- [ ] Later GS/DLP integration may use adjacency metadata to establish the correct local switch/path context.

## 10. Fast-path completeness

- [ ] P4 validates the fields required for forwarding.
- [ ] Global LPM remains entirely in P4.
- [ ] Local-form GDP is never transit-routed.
- [ ] Hop Limit is decremented exactly once per transit router.
- [ ] Hop 0/1 is dropped/punted according to the final GCTL error policy.
- [ ] Add per-port and per-route counters where the target supports them.
- [ ] Add QoS/queue classification without moving route lookup into Rust.
- [ ] Define CPU/control queue backpressure.

## 11. Integration tests

Build deterministic simulated networks around the production router object:

- [ ] Router + GS + one fresh host:
  - [ ] router registers with switch;
  - [ ] host discovers router;
  - [ ] host receives/claims address;
  - [ ] switch learns host address;
  - [ ] router records node.
- [ ] Two LANs on one router: configured hosts exchange GTS through P4 forwarding.
- [ ] Two statically configured routers: GTS crosses both routers with no routing protocol.
- [ ] Default route and longest-prefix override.
- [ ] Node lease expiry/reconfiguration.
- [ ] Link/switch reset and router re-registration.
- [ ] Route updates do not interrupt unrelated P4 forwarding.
- [ ] Ordinary transit test asserts the Rust management dispatcher is never invoked.

## 12. Later configuration persistence

Deferred until the in-memory model is stable:

- [ ] configuration file format;
- [ ] boot-time parser;
- [ ] persistent address leases if desired;
- [ ] atomic configuration replacement;
- [ ] administrative CLI/API;
- [ ] diagnostics/status export.

The Rust constructor/config structures in step 1 are intentionally the semantic model that a future file parser will populate.

## Explicitly out of scope

- dynamic router-to-router route exchange;
- distance-vector/link-state protocols;
- automatic inter-domain routing;
- ECMP in the first static implementation;
- NAT/address rewriting;
- Ethernet bridging/ARP;
- DLP credit/VC implementation inside the router;
- persistent configuration format in the first milestone.
