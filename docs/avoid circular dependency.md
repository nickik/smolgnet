# Avoid circular dependency in wormhole routing

## Why this matters

Wormhole routing does not normally buffer a complete packet at every router. The head of a packet advances while the rest of the packet occupies resources behind it. That gives low latency and small router buffers, but it also creates a specific failure mode: a packet can hold one channel or virtual-channel resource while waiting for another.

If several packets do that in a circle, every packet can wait forever.

This is not ordinary congestion. Congestion eventually clears if some packet can advance. A wormhole deadlock is a closed resource wait cycle in which no participant can advance and therefore no participant can release the resource it already holds.

A useful first model is the **channel-dependency graph (CDG)**:

- each node is a channel resource, normally a physical link plus a VCID;
- an edge `A -> B` exists when a legal route can hold `A` while requesting `B`;
- a dependency cycle means the routing function can potentially construct a circular wait.

Dally and Seitz formalized this approach. For deterministic wormhole routing, absence of cycles in the channel-dependency graph is the classic necessary-and-sufficient deadlock-free condition. Their construction also shows how virtual channels can split physical-channel resources into classes that remove cycles.

Adaptive routing is subtler. Duato showed that the adaptive resource set may itself contain cycles and still be deadlock-free if packets always retain access to a connected deadlock-free escape subset. Later work by Schwiebert and Jayasimha introduced the **channel waiting graph**, which removes dependencies that cannot actually participate in a deadlock configuration. This is useful because a raw CDG cycle can be conservative for adaptive routing.

## Static possibility versus an actual deadlocked state

There are two different questions:

1. **Can this routing algorithm ever deadlock?**
2. **Is this particular network state deadlocked right now?**

For the first question, analyze the routing/resource rules. A CDG or equivalent dependency model is built from all legal resource transitions. For deterministic routing, a cycle is enough to show that a corresponding deadlock state can be constructed. For adaptive routing, use a stronger criterion such as Duato's escape-subnetwork conditions or a channel-waiting-graph analysis.

For the second question, inspect actual reservations and waits. The tests in this branch build a **wait-for graph** from the simulated wormhole state:

```text
flow A holds resource X and requests Y
flow B holds resource Y and requests Z
flow C holds resource Z and requests X
```

This creates:

```text
A -> B -> C -> A
```

where an edge means "this blocked flow is waiting for a resource held by that blocked flow." A closed cycle in that graph is direct evidence of circular wait in the simulated state.

The tests deliberately do not classify every blocked flow as deadlocked. A packet may be queued behind a deadlocked component without itself belonging to the cycle. This matters for diagnostics and recovery.

## Why timeout alone is not deadlock detection

A router can observe that a packet has made no progress for some number of cycles, but that does not prove deadlock. Long packets, transient congestion, unfair arbitration, or heavy load can all create long stalls.

Published work on deadlock detection in wormhole networks specifically notes that timeout-based schemes can generate false deadlock detections, especially under heavy load and with long packets. Timeout can therefore be useful as a trigger for inspection or recovery, but it should not be treated as a proof of circular dependency.

For GNet, a useful future runtime diagnostic sequence is:

```text
1. packet/VC exceeds a stall threshold
2. record the resource it holds and the resource it requests
3. follow ownership of the requested resource
4. repeat until:
   - a free resource is reached      -> blocked, not circular deadlock
   - a previously visited flow/resource is reached -> closed wait cycle
5. report the exact cycle members and resources
```

This is a diagnostic mechanism. The architectural goal should still be **deadlock avoidance by construction** using the VC0 escape discipline, rather than relying on runtime recovery.

## The basic four-router example

The first tests use four routers in a directed ring:

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

After every worm acquires its first resource:

```text
F0 holds L0 and wants L1
F1 holds L1 and wants L2
F2 holds L2 and wants L3
F3 holds L3 and wants L0
```

The wait cycle is:

```text
F0 -> F1 -> F2 -> F3 -> F0
```

and the resource dependency is:

```text
L0 -> L1 -> L2 -> L3 -> L0
```

Because wormhole packets retain resources behind the head, none of the four can release its current channel while waiting for the next one.

## More complex regression scenarios

The branch now tests several patterns that are closer to the difficult cases described by the literature than a one-resource-per-packet ring.

### 1. Multi-hop worms with wraparound

A worm can occupy several channels at once. The test constructs three packets, each already stretched across two links:

```text
F0 holds L0,L1 -> wants L2
F1 holds L2,L3 -> wants L4
F2 holds L4,L5 -> wants L0
```

This creates a three-flow circular wait only after the packets have accumulated multiple occupied hops. The final request returns to the beginning of the ring:

```text
L0,L1 -> L2,L3 -> L4,L5 -> L0
```

This catches an important misconception: deadlock is not limited to neighboring one-hop packets. A worm's body may retain a long chain of channels while its head waits much farther ahead.

### 2. Dependency cycle across different normal VC classes

A deadlock cycle does not have to stay on one VCID. If the router permits transitions among VC1, VC2, and VC3, the dependency graph can close across those classes:

```text
F0 holds L0/VC1, L1/VC2 -> wants L2/VC3
F1 holds L2/VC3, L3/VC1 -> wants L4/VC2
F2 holds L4/VC2, L5/VC3 -> wants L0/VC1
```

The test verifies that this is still a real closed wait cycle. The relevant object is the full resource `(link, VCID)`, not just the physical link and not just a single VC class.

### 3. Multiple deadlocked components plus an innocent blocked packet

The test also creates two independent cycles simultaneously:

```text
cycle A: F0 -> F1 -> F2 -> F0
cycle B: F3 -> F4 -> F3
```

and a sixth flow:

```text
F5 -> F1
```

F5 is blocked behind a deadlocked component, but it is not itself part of a circular wait. The detector must return two cycles and exclude F5 from their membership.

This distinction is important for future recovery logic: aborting every stalled or transitively blocked packet would be unnecessarily destructive.

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

The basic bad-mode test deliberately places all four worms on VC0 and verifies a closed wait cycle.

### `EscapeOnly`

Ordinary traffic is restricted to:

```text
VC1 VC2 VC3
```

VC0 is held back as an escape resource.

The ordinary/adaptive part of the network may still form cycles. That is permitted only because blocked packets retain a route into an independently deadlock-free VC0 subnetwork.

## Reserving VC0 is necessary but not sufficient

The important rule is not merely "have one spare VC."

A general escape design needs all of the following:

1. **Normal traffic must not consume the escape resources.** Otherwise the escape network can be blocked by the same traffic it is meant to rescue.
2. **The escape routing function must itself be deadlock-free.** Its channel-dependency graph must be acyclic, or another proven deadlock-free mechanism must be used.
3. **Once a packet enters the escape VC, it must not return to the unrestricted/adaptive VC set.** Returning would reintroduce dependencies from the escape graph back into the cyclic graph.
4. **Escape progress must not depend on resources that ordinary traffic can permanently occupy.** Buffer and credit rules have to preserve the same separation as VC allocation.
5. **Arbitration must eventually serve escape traffic.** A logically free escape path is not useful if an implementation can starve it forever.

This is the key lesson from Duato-style routing: the adaptive part may contain cycles, but packets must have access to a connected deadlock-free subset that can deliver them.

## How the complex tests model a safe escape network

The complex tests intentionally do not "escape" by replaying the same cyclic ring route on VC0. Doing that would merely recreate the original cycle on another VC.

Instead each escape route uses VC0 and follows a strictly increasing **escape rank**:

```text
rank 10 -> rank 20 -> rank 30
```

No legal escape path may transition to an equal or lower rank. Therefore an escape dependency cannot return to an earlier resource and close a cycle.

This is a compact test representation of the same principle used by deterministic deadlock-free routing disciplines: impose an ordering on resources and only permit transitions that move forward in that ordering.

For a future arbitrary-topology GNet implementation, the rank can be derived from a topology algorithm rather than being synthetic test data.

## What GNet should do for arbitrary topologies

GNet is not restricted to a rectangular mesh, so a fixed XY/dimension-order escape rule is not sufficient as the universal design.

A practical direction is:

```text
VC1-VC3: normal/adaptive routing
VC0:     deterministic escape routing
```

For an arbitrary connected topology, VC0 can use a topology-derived deadlock-free rule. A classic example is **up*/down*** routing:

1. choose a root and build a spanning tree;
2. orient channels as up or down relative to the tree/root;
3. allow zero or more up transitions followed by zero or more down transitions;
4. prohibit a down-to-up transition;
5. once traffic enters VC0, keep it on VC0 until delivery.

This prohibits the turn pattern that would close the dependency cycle. Up*/down*-style routing is particularly relevant to irregular point-to-point networks, which is much closer to the intended GNet environment than a fixed 2-D mesh.

Another classic technique is the **turn model**: identify turns whose combination creates dependency cycles and prohibit enough of them to make the routing relation acyclic while retaining useful adaptivity.

## Detection in tests versus detection in production

### In the tests

The detector is exact for the represented state:

```text
resource owner map:
    (link, VCID) -> flow

wait edge:
    flow A -> flow B
    when A requests a resource currently held by B
```

Then the test performs graph traversal and extracts every closed wait cycle.

This lets the regression assert not only "the network stopped" but:

- which flows form the circular wait;
- which stalled flows are merely waiting behind it;
- that multiple independent cycles are found separately;
- that each requested normal resource has a VC0 alternative when escape mode is enabled.

### In production

There are three levels of protection:

**1. Offline design proof — preferred**

Analyze the legal routing/resource transitions and prove the VC0 subnetwork deadlock-free. This prevents deadlock rather than detecting it after the fact.

**2. Runtime telemetry — useful**

Routers can expose:

- current VC owner;
- requested next resource;
- stall age;
- escape transition state;
- credit/buffer owner;
- whether the packet is already committed to VC0.

Management/debug software can reconstruct a wait-for graph from that data.

**3. Runtime recovery — optional, not the primary design**

A timeout or watchdog may trigger cycle inspection. If a real closed cycle is confirmed, one packet could theoretically be ejected, aborted, rerouted, or moved to a reserved recovery resource. Literature contains recovery-based designs, but they add machinery and correctness cases that GNet does not need if the escape path is sound.

The design target for GNet should remain avoidance by construction.

## What the current tests prove

The current branch proves several concrete properties:

- four real `CycleAwareRouter` instances compute the basic two-hop clockwise paths;
- with `RouterVc0Policy::NormalData`, VC0 belongs to the ordinary pool and can be consumed by a closed dependency cycle;
- with `RouterVc0Policy::EscapeOnly`, normal traffic is limited to VC1-VC3;
- a multi-hop worm can hold multiple links and close a dependency only several hops later;
- a deadlock cycle may span VC1, VC2, and VC3 rather than one VC;
- multiple independent deadlock components can be detected in one state;
- a blocked flow that does not belong to a cycle is not falsely labeled as a cycle member;
- complex VC0 escape paths are constrained by a strictly increasing rank, making their represented dependency graph acyclic.

This remains a resource-level wormhole test. The current software DLP frame model transmits complete GDP frames between adjacent nodes and therefore does not naturally hold a partial worm across several routers. The cycle simulator models reservation semantics that eventual flit-level/router hardware must implement.

## Design consequences

### Router configuration

```text
NormalData  -> normal VC set = {0,1,2,3}, no dedicated escape VC
EscapeOnly  -> normal VC set = {1,2,3}, dedicated escape VC = 0
```

`EscapeOnly` is the safe default for cycle-aware routing design.

### Router/link boundary

GDP and GTS should still not select numeric VCIDs. VC choice is a router/link-resource decision. The router forwarding plane determines the next physical link; the wormhole scheduler determines whether that hop uses an ordinary VC or the escape VC.

### Control traffic

The current DLP v0.1 implementation already treats VC0 specially for control. A production design that also uses VC0 as the network escape class must define arbitration and buffering between control and escape data explicitly. Control must retain guaranteed progress; simply mixing control and arbitrary escape data into one FIFO could create another dependency problem.

The tests in this branch therefore establish the **routing resource policy**, not a final wire-level multiplexing rule for GCTL and escape traffic.

## Rules to preserve in future implementations

- Never silently add VC0 to the ordinary data allocator when escape mode is enabled.
- Never permit `escape -> normal` VC transitions.
- Never claim deadlock freedom merely because VC0 exists.
- Verify the VC0 routing function's dependency graph separately.
- Include buffer/credit resources in the dependency analysis, not only physical links.
- Do not equate "stalled for N cycles" with "deadlocked."
- Keep ownership/request telemetry sufficient to reconstruct a wait-for graph.
- Test multi-hop worms whose bodies occupy several links simultaneously.
- Test cycles that cross different normal VC classes.
- Test multiple simultaneous deadlocked components.
- Test blocked flows that are outside the actual circular wait.
- When the flit-level multi-router simulator exists, reproduce these scenarios with real head/body/tail resource retention.

## References

1. William J. Dally and Charles L. Seitz, **"Deadlock-Free Message Routing in Multiprocessor Interconnection Networks"**, IEEE Transactions on Computers, 36(5), 1987, pp. 547-553. DOI: https://doi.org/10.1109/TC.1987.1676939 . Open Caltech technical-report version: https://authors.library.caltech.edu/records/fd0yr-br438
2. José Duato, **"A New Theory of Deadlock-Free Adaptive Routing in Wormhole Networks"**, IEEE Transactions on Parallel and Distributed Systems, 4(12), 1993, pp. 1320-1331. DOI: https://doi.org/10.1109/71.250114 . Public PDF: https://www.csl.cornell.edu/courses/ece5750/duato.tpds93.pdf
3. José Duato, **"A Necessary and Sufficient Condition for Deadlock-Free Adaptive Routing in Wormhole Networks"**, IEEE Transactions on Parallel and Distributed Systems, 6(10), 1995, pp. 1055-1067. DOI: https://doi.org/10.1109/71.473515
4. Loren Schwiebert and D. N. Jayasimha, **"A Necessary and Sufficient Condition for Deadlock-Free Wormhole Routing"**, Journal of Parallel and Distributed Computing, 32(1), 1996, pp. 103-117. DOI: https://doi.org/10.1006/jpdc.1996.0008 . Introduces the channel waiting graph to exclude dependencies that cannot participate in a real deadlock configuration.
5. Christopher J. Glass and Lionel M. Ni, **"The Turn Model for Adaptive Routing"**, ISCA 1992, pp. 278-287. DOI: https://doi.org/10.1145/146628.140384
6. William J. Dally, **"Virtual-Channel Flow Control"**, IEEE Transactions on Parallel and Distributed Systems, 3(2), 1992. DOI: https://doi.org/10.1109/71.127260
7. Soojung Lee, **"A deadlock detection mechanism for true fully adaptive routing in regular wormhole networks"**, Computer Communications, 30(8), 2007, pp. 1826-1840. DOI: https://doi.org/10.1016/j.comcom.2007.02.013 . Discusses limitations and false detections of timeout-based schemes and proposes more accurate runtime detection.

## Short version

The dangerous state is not merely "nothing moved recently." It is a closed ownership/wait cycle:

```text
A holds X, wants Y
B holds Y, wants Z
C holds Z, wants X
```

A packet can hold several resources, and the cycle can return only after many hops. It can also cross several normal VC classes.

VC0 helps only if:

```text
VC1-VC3 = ordinary/adaptive resources
VC0     = reserved escape resources
```

and VC0 itself follows a routing discipline whose resource dependencies cannot form a cycle.
