# Two-Port Static P4 Router

PR #15 is intentionally limited to one executable milestone: a **two physical port GNet router with static routes and a P4 GDP forwarding plane**.

```text
                 static startup FIB
                        |
                        v
GDP/DLP -> port 0 -> [ P4 router ] -> port 1 -> GDP/DLP
GDP/DLP <- port 0 <- [            ] <- port 1 <- GDP/DLP
```

## Fixed contract

- Exactly two physical ports exist: `0` and `1`.
- CPU punt is the internal P4 port `2`; it is not a physical network port.
- Both physical links are assumed available in this milestone. DLP link lifecycle is outside the router object.
- Routing configuration is supplied in memory at startup.
- There is no dynamic routing protocol.
- P4 performs global GDP longest-prefix matching and Hop-Limit processing.
- `/0..32` and `/33..64` prefixes are supported by the split x4c tables.
- Connected prefixes and static routes may overlap; longest-prefix match chooses the route.
- Duplicate identical forwarding prefixes are rejected.
- Router-owned and bootstrap `/64` destinations are punted to the CPU and override covering routes.
- Local-form GDP is never transit-routed.
- A transit packet preserves its GDP destination exactly.
- A transit packet decrements Hop Limit exactly once.
- Hop Limit `0/1` is not forwarded.
- A route selecting the ingress physical port is treated as a hairpin and dropped in this first router.
- A static `next_hop`, when present, must be a non-reserved link-local GDP address. It is adjacency metadata only and never replaces the GDP destination.

## Validation checklist

- [x] Require exactly two physical ports.
- [x] Require canonical port IDs `0`, `1`.
- [x] Validate unique router link-local addresses.
- [x] Validate connected prefix/router-address pairs.
- [x] Validate static route egress ports.
- [x] Reject duplicate forwarding prefixes.
- [x] Validate optional next-hop addresses.
- [x] Protect bootstrap and router-owned `/64` destinations.
- [x] Test port 0 -> port 1 forwarding.
- [x] Test port 1 -> port 0 forwarding.
- [x] Test destination preservation and one Hop-Limit decrement.
- [x] Test default route and more-specific route selection.
- [x] Test the split LPM boundary at `/32` and `/33`.
- [x] Test long-prefix boundary behavior at `/63` and `/64`.
- [x] Test router-local/bootstrap CPU punt.
- [x] Test local-form non-transit behavior.
- [x] Test Hop-Limit expiry.
- [x] Test same-port hairpin drop.
- [x] Bound CI runtime and expose test output.
- [x] Verify committed formatting with `rustfmt --check` rather than modifying CI checkout sources.

## Explicitly deferred

The following do **not** belong in PR #15:

- GS3 switch registration;
- node/lease database;
- address allocation and router discovery;
- port/link state machines;
- live FIB updates;
- route persistence;
- dynamic routing;
- ECMP;
- QoS/counters;
- DLP credit or VC state inside the router.

The next independent milestone is an **8-port GS3 switch**. Only after both the two-port router and eight-port switch work independently should we build a combined router/switch simulation.
