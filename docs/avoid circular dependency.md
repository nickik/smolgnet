# Avoid circular dependency in wormhole routing

## Why this matters

Wormhole routing does not normally buffer a complete packet at every router. The head of a packet advances while the rest of the packet occupies resources behind it. That gives low latency and small router buffers, but it also creates a specific failure mode: a packet can hold one channel or virtual-channel resource while waiting for another.

If several packets do that in a circle, every packet can wait forever.

This is not ordinary congestion. Congestion eventually clears if some packet can advance. A wormhole deadlock is a closed resource wait cycle in which no participant can advance and therefore no participant can release the resource it already holds.

A useful mental model is the **channel-dependency graph (CDG)**:

- each node is a channel resource, normally a physical link plus a VCID;
- an edge `A -> B` exists when a legal route can hold `A` while requesting `B`;
- a dependency cycle means a traffic pattern can potentially create a circular wait.

Dally and Seitz formalized this approach and showed how virtual channels can be used to split channel resources and remove dependency cycles. Later work, especially Duato's, showed that adaptive routing can still be deadlock-free even when the adaptive part contains cycles, provided there is a deadlock-free escape subnetwork.

## The four-router example used by the tests

The `cycles` tests use four routers in a directed ring:

```text
        L0        L1        L2        L3
   R0 ------> R1 ------> R2 ------> R3 -----+
    ^                                         |
    +-----------------------------------------+
```

Four two-hop flows are admitted simultaneously:

```text
F0: L0 -> L1
F1: L1 -> L2
F2: L2 -> L3
F3: L3 -> L0
```

After every worm acquires its first resource, the state is:

```text
F0 holds L0 and wants L1
F1 holds L1 and wants L2
F2 holds L2 and wants L3
F3 holds L3 and wants L0
```

That is the closed dependency cycle:

```text
L0 -> L1 -> L2 -> L3 -> L0
```

Because wormhole packets retain resources behind the head, none of the four packets can simply release its current link while waiting for the next one. If every requested resource is occupied by another member of the same set, the network is stuck.

## What VC0 is for

This branch makes the router VC0 policy explicit:

```rust
RouterVc0Policy::NormalData
RouterVc0Policy::EscapeOnly
```

### `NormalData`

Ordinary traffic may use all four VCIDs:

```text
VC0 VC1 VC2 VC3
```

This maximizes the ordinary resource pool, but VC0 can now become part of the same circular dependency as every other VC.

The deadlock test deliberately places all four worms on VC0:

```text
F0 holds L0/VC0 -> wants L1/VC0
F1 holds L1/VC0 -> wants L2/VC0
F2 holds L2/VC0 -> wants L3/VC0
F3 holds L3/VC0 -> wants L0/VC0
```

There is no reserved escape resource. The test asserts that every requested `(link, VC0)` is held by the next worm, so no head can move.

### `EscapeOnly`

Ordinary traffic is restricted to:

```text
VC1 VC2 VC3
```

VC0 is held back as an escape resource.

The same dependency cycle can still form on an ordinary VC, for example VC1:

```text
F0 holds L0/VC1 -> wants L1/VC1
F1 holds L1/VC1 -> wants L2/VC1
F2 holds L2/VC1 -> wants L3/VC1
F3 holds L3/VC1 -> wants L0/VC1
```

But now the corresponding escape resources are free:

```text
F0 can request L1/VC0
F1 can request L2/VC0
F2 can request L3/VC0
F3 can request L0/VC0
```

In the deliberately minimal two-hop test, entering VC0 is the final hop, so no packet creates a further `VC0 -> VC0` dependency. Every blocked flow therefore has a route out of the circular wait.

## Reserving VC0 is necessary but not sufficient

The important rule is not merely "have one spare VC."

A general escape design needs all of the following:

1. **Normal traffic must not consume the escape resources.** Otherwise the escape network can be blocked by the same traffic it is meant to rescue.
2. **The escape routing function must itself be deadlock-free.** Its channel-dependency graph must be acyclic, or another proven deadlock-free mechanism must be used.
3. **Once a packet enters the escape VC, it must not return to the unrestricted/adaptive VC set.** Returning would reintroduce dependencies from the escape graph back into the cyclic graph.
4. **Escape progress must not depend on resources that ordinary traffic can permanently occupy.** Buffer and credit rules have to preserve the same separation as VC allocation.

This is the key lesson from Duato-style routing: the adaptive part may contain cycles, but a packet must always have access to a connected deadlock-free subset of resources that can eventually deliver it.

## What the current test proves

The test proves a small, concrete property rather than claiming a general theorem about all GNet topologies:

- four real `CycleAwareRouter` instances compute the two-hop clockwise paths;
- with `RouterVc0Policy::NormalData`, VC0 is in the ordinary pool and the four flows can construct a closed wait cycle on VC0;
- with `RouterVc0Policy::EscapeOnly`, ordinary traffic cannot consume VC0;
- the same four-way ordinary dependency cycle can form on VC1;
- every blocked packet then has a distinct free next-link VC0 resource;
- because the test route terminates after that escape hop, the VC0 dependency graph for this test is acyclic.

This is intentionally a resource-level wormhole test. The current software DLP frame model transmits complete GDP frames between adjacent nodes and therefore does not naturally hold a partial worm across several routers. The cycle simulator models the reservation semantics that the eventual flit-level/router hardware must implement.

## What GNet should do for arbitrary topologies

GNet is not restricted to a rectangular mesh, so a fixed XY/dimension-order escape rule is not sufficient as the universal design.

A practical direction is:

```text
VC1-VC3: normal/adaptive routing
VC0:     deterministic escape routing
```

For an arbitrary connected topology, VC0 can use a topology-derived acyclic rule. A classic example is **up*/down*** routing:

1. choose a root and build a spanning tree;
2. orient links as up or down relative to the tree/root;
3. allow zero or more up transitions followed by zero or more down transitions;
4. prohibit a down-to-up turn;
5. once traffic enters VC0, keep it on VC0 until delivery.

Up*/down* was used in DEC's Autonet family for irregular point-to-point networks and is directly relevant to the kind of topology GNet is likely to encounter. It trades some path optimality for a simple deadlock-free routing structure.

That should be a later implementation milestone. The current `cycles` branch establishes the policy boundary and the regression test that prevents VC0 from accidentally becoming ordinary traffic again.

## Design consequences

### Router configuration

The router policy is explicit instead of implicit:

```text
NormalData  -> normal VC set = {0,1,2,3}, no dedicated escape VC
EscapeOnly  -> normal VC set = {1,2,3}, dedicated escape VC = 0
```

`EscapeOnly` is the safe default for cycle-aware routing design.

### Router/link boundary

GDP and GTS should still not select numeric VCIDs. VC choice is a router/link-resource decision. The router forwarding plane determines the next physical link; the wormhole scheduler determines whether that hop uses an ordinary VC or the escape VC.

### Control traffic

The current DLP v0.1 implementation already treats VC0 specially for control. A production design that also uses VC0 as the network escape class must define the arbitration and buffering relationship between control and escape data explicitly. Control must retain guaranteed progress; simply mixing control and arbitrary escape data into one FIFO would create a new dependency problem.

The tests in this branch therefore establish the **routing resource policy**, not a final wire-level multiplexing rule for GCTL and escape traffic.

## Rules to preserve in future implementations

- Never silently add VC0 to the ordinary data allocator when escape mode is enabled.
- Never permit `escape -> normal` VC transitions.
- Never claim deadlock freedom merely because VC0 exists.
- Verify the escape routing function's channel-dependency graph separately.
- Include buffer/credit resources in the dependency analysis, not only physical links.
- Test adversarial simultaneous traffic patterns, not only random traffic.
- Keep a deterministic regression that intentionally constructs a closed wait cycle.
- When the flit-level multi-router simulator exists, reproduce this exact four-router scenario with real head/body/tail resource retention.

## References

Canonical and directly relevant sources:

1. William J. Dally and Charles L. Seitz, **"Deadlock-Free Message Routing in Multiprocessor Interconnection Networks"**, IEEE Transactions on Computers, 36(5), 1987, pp. 547-553. DOI: https://doi.org/10.1109/TC.1987.1676939 . Open Caltech technical-report version: https://authors.library.caltech.edu/records/fd0yr-br438
2. José Duato, **"A New Theory of Deadlock-Free Adaptive Routing in Wormhole Networks"**, IEEE Transactions on Parallel and Distributed Systems, 4(12), 1993, pp. 1320-1331. DOI: https://doi.org/10.1109/71.250114 . Public PDF: https://www.csl.cornell.edu/courses/ece5750/duato.tpds93.pdf
3. William J. Dally and Hiromichi Aoki, **"Deadlock-Free Adaptive Routing in Multicomputer Networks Using Virtual Channels"**, IEEE Transactions on Parallel and Distributed Systems, 4(4), 1993, pp. 466-475. The paper develops virtual-channel classes and deterministic escape behavior for adaptive routing.
4. Christopher J. Glass and Lionel M. Ni, **"The Turn Model for Adaptive Routing"**, ISCA 1992, pp. 278-287. DOI: https://doi.org/10.1145/146628.140384 . It shows another way to eliminate dependency cycles: prohibit selected turns instead of relying only on additional VCs.
5. Thomas L. Rodeheffer et al., **"Autonet: A High-Speed, Self-Configuring Local Area Network Using Point-to-Point Links"**, IEEE JSAC, 1991. Microsoft Research page: https://www.microsoft.com/en-us/research/publication/autonet-a-high-speed-self-configuring-local-area-network-using-point-to-point-links/ . Autonet is relevant because it applies deadlock-free routing to an irregular point-to-point topology rather than a regular mesh.
6. William J. Dally, **"Virtual-Channel Flow Control"**, IEEE Transactions on Parallel and Distributed Systems, 3(2), 1992. This is useful background on why virtual channels separate resource allocation from a physical channel and how they affect throughput and blocking.

## Short version

The dangerous state is:

```text
A holds X, wants Y
B holds Y, wants Z
C holds Z, wants X
```

Adding VC0 helps only if ordinary traffic cannot consume it and VC0 follows a deadlock-free routing discipline.

For GNet the intended model is therefore:

```text
VC1-VC3 = fast/normal/adaptive resources
VC0     = reserved escape resources
```

with a later topology-independent escape algorithm, most likely a spanning-tree/up*/down*-style rule for arbitrary graphs.
