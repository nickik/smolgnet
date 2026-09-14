# GC3 and GS3 Simulation Model

## Purpose

This document defines how `smolgnet` should model **GC3 Couplers** and **GS3 Switches** closely enough to exercise the real GNet link, credit, control-pair, GDP and transport code in deterministic static scenarios.

The goal is **not** to build a general-purpose network simulator, topology language, traffic-modeling package, or high-fidelity electrical simulator.

The goal is to make small fixed arrangements such as:

```text
A --- GC3 --- B

A --- GC3 --- B
      |
      C

A --- GS3 --- B
      |
      C

A --- GS3 --- B
C ---/     \--- D
```

run using the same `smolgnet` protocol implementation that real endpoints will use, so that we can verify:

- DLP flit movement;
- VC state;
- GCTL link-credit behavior;
- GC3 `WANT/PERMIT` arbitration;
- GS3 ingress and egress credit independence;
- GS3 destination-specific cut-through forwarding;
- bounded buffering and backpressure;
- simultaneous switched paths;
- GTS behavior over the resulting link;
- basic latency, throughput, utilization and control overhead.

The simulation should be deterministic by default and small enough to understand from a trace.

Routing is outside the scope of this document. A router model can be added later as another consumer of `GnetLinkDevice`, `DlpEndpoint`, and the incremental GDP decoder.

---

# 1. Source architecture

The simulation must preserve the core `smolgnet` split:

```text
Application / GTS
       |
GDP / GCTL
       |
DlpEndpoint
       |
GnetLinkDevice
       |
physical/simulated attachment
```

A native GNet attachment contains separate logical physical channels:

```text
CONTROL-UP      endpoint -> GC/GS
CONTROL-DOWN    GC/GS -> endpoint
DATA-UP         endpoint -> GC/GS
DATA-DOWN       GC/GS -> endpoint
```

The simulator must preserve this separation. Control-pair state must not be encoded as fake GDP packets and GDP/GCTL traffic must not be sent over the simulated control pair.

---

# 2. Simulation philosophy

The simulator should be a **small deterministic execution harness around real smolgnet components**.

Do not create a second implementation of GDP, GCTL, GTS, credits, or endpoint DLP behavior inside the simulator.

The simulator models only things that in real hardware would exist outside an endpoint's protocol stack:

- cable/link serialization;
- propagation delay if desired;
- GC3 arbitration and broadcast/repetition;
- GS3 forwarding fabric and bounded internal flit buffers;
- control-pair signal changes;
- switch attachment/egress selection;
- deterministic virtual time.

Everything else should run through normal `smolgnet` code.

The preferred rule is:

> If the real endpoint would execute the protocol behavior, use the real smolgnet implementation. If physical infrastructure would perform it, model it in the simulation component.

---

# 3. Minimal simulation kernel

A full network simulator is unnecessary. We only need a small virtual-time scheduler.

Use deterministic virtual time with a priority queue of events:

```text
SimClock
   |
   +-- next data-flit arrival
   +-- next control-state transition
   +-- GC3 quantum expiry
   +-- GS3 forwarding opportunity
   +-- endpoint poll/timer deadline
```

A conceptual API could be:

```rust
pub struct SimTime(u64);

pub enum SimEvent {
    DataFlitArrive { link: LinkId, direction: Direction, flit: Flit },
    ControlChanged { link: LinkId, direction: Direction, value: ControlValue },
    WakeEndpoint { endpoint: EndpointId },
    WakeGc3 { device: Gc3Id },
    WakeGs3 { device: Gs3Id },
}
```

The exact API is not important. The important properties are:

- deterministic ordering;
- no wall-clock sleeps;
- no threads required;
- reproducible traces;
- simulation can advance directly to the next event;
- `smolgnet` timers receive the simulated `Instant`.

This allows GTS retransmission/timer behavior to use the same timing interfaces as a real endpoint without slowing tests to wall-clock speed.

---

# 4. Time and serialization model

The simulator does not need analog electrical behavior.

For a GNet-3 link, represent the configured line rate as 3 Mbit/s and account for the serialized physical bits of each data flit.

Baseline flit:

```text
VCID       2 bits
carried   32 bits
----------------
physical  34 bits
```

A link therefore has a deterministic flit serialization interval derived from:

```text
34 bits / configured bit rate
```

Use integer simulation units internally so repeated runs are exact. Nanoseconds, picoseconds, or rational bit-time units are all acceptable.

The simulator may optionally add a fixed cable/processing delay, but this should default to a simple small constant or zero. The objective is protocol comparison, not cable-physics analysis.

Control-pair signals operate independently of data serialization. Changing `WANT` or `PERMIT` does not consume data-channel bandwidth.

GCTL messages do consume data-channel bandwidth because they are ordinary GDP traffic.

---

# 5. Common simulated attachment

Each endpoint should connect through a simulated implementation of `GnetLinkDevice`.

Conceptually:

```text
Smolgnet endpoint
      |
 DlpEndpoint
      |
SimGnetLinkDevice
      |
SimPort
      |
 GC3 or GS3
```

`SimGnetLinkDevice` should expose the same native semantics expected from a future real NIC driver:

- batches of received data flits;
- batches/opportunities for transmitting data flits;
- CONTROL-UP state/output;
- CONTROL-DOWN state/input;
- negotiated/configured line rate;
- link-up/reset state.

The endpoint should not know whether the opposite infrastructure is implemented in Rust simulation code or hardware.

---

# 6. GC3 model

## 6.1 Architectural rule

GC3 must remain deliberately simple.

The simulated GC3 is **not** a forwarding endpoint and therefore must not contain:

```text
GDP address table
GDP parser
GCTL parser
credit table
VC table
packet buffers
GTS state
routing state
```

GC3 has one shared data resource. At most one endpoint transmits into it at a time. The selected endpoint's data is repeated to the attached receivers.

The GC3 simulation should therefore look approximately like:

```text
                 +-------------------+
A DATA-UP ------>|                   |------> DATA-DOWN A
B DATA-UP ------>|       GC3         |------> DATA-DOWN B
C DATA-UP ------>|                   |------> DATA-DOWN C
                 |                   |
A WANT --------->| arbitration       |------> A PERMIT
B WANT --------->| only              |------> B PERMIT
C WANT --------->|                   |------> C PERMIT
                 +-------------------+
```

The Coupler does not inspect the flit payload.

## 6.2 GC3 port state

Each GC3 port needs only infrastructure-level state such as:

```rust
struct Gc3Port {
    want: bool,
    permit: bool,
    attached: bool,
}
```

Potential additional simulation-only statistics are acceptable:

```text
requested_time
permitted_time
flits_transmitted
wait_time
```

These are measurement counters, not protocol state.

## 6.3 Arbitration

Initial GC3 arbitration is round-robin among ports whose `WANT` is asserted.

The ownership quantum must be configurable because the exact normative GC3 quantum is not frozen.

For simulation, express it in a convenient exact unit such as:

```text
gc3_quantum_flits = N
```

or an equivalent time duration.

Do not hard-code one value as protocol truth.

A scenario can run the same workload with several values to see how quantum size affects fairness and throughput.

The required control sequence remains:

```text
WANT   rises
PERMIT rises
PERMIT falls
WANT   falls
```

After `PERMIT` falls, a still-busy endpoint must release `WANT` before requesting another turn.

The GC3 model should assert this rule and flag a protocol violation if an endpoint incorrectly keeps ownership semantics across the required release handshake.

## 6.4 Data repetition

When port A owns the GC3 medium and sends a flit:

```text
A DATA-UP -> GC3 -> DATA-DOWN on attached receiver ports
```

GC3 performs no destination filtering.

Every receiving endpoint processes the repeated flit through its normal native DLP path and ultimately decides whether the GDP packet is relevant.

The simulation may omit loopback to the transmitting endpoint unless a real hardware reason for sender echo is later specified. This is a simulation detail and must not affect protocol semantics.

## 6.5 GC3 and credits

GC3 must not know that credits exist.

Example:

```text
A --- GC3 --- B

A <--- GCTL link credits ---> B
```

`CREDIT_REQUEST` and `CREDIT` are actual GDP/GCTL traffic emitted by the two smolgnet endpoints. GC3 merely repeats the data flits carrying those packets.

Therefore the simulation automatically captures the real overhead of credit traffic on a busy shared medium.

This is important: a credit exchange is not a zero-cost simulator callback.

## 6.6 GC3 transmit gate

At endpoint A, the normal DLP transmit logic should observe two independent conditions:

```text
physical permission: PERMIT
receiver capacity:   link_flit_credit
```

A flit can be sent only when the required conditions allow it.

The simulator should be able to trace four cases explicitly:

```text
PERMIT=0 credit=0
PERMIT=0 credit>0
PERMIT=1 credit=0
PERMIT=1 credit>0
```

This is one of the key behaviors the GC3 model exists to verify.

## 6.7 Important GC3 open issue exposed by simulation

The current GC3 design says ownership is time-bounded and that a large GDP transfer may be paused and resumed across multiple ownership turns.

At the same time:

- GC3 does not parse packets;
- GC3 does not allocate or interpret VCIDs;
- DLP has no source address;
- all receivers see the shared data stream;
- another endpoint may receive the next ownership turn.

This creates a modeling question that must not be hidden by the simulator:

> If endpoint A pauses in the middle of a DLP/GDP segment and endpoint B becomes the next GC3 owner, how does a receiver distinguish B's first flit from continuation of A's paused segment?

With only hop-local 2-bit VCIDs and no GC3 sender tag, blindly interleaving transmitters inside active segments can become ambiguous.

The simulator should make this visible rather than silently solving it.

Until the GNet specification resolves the exact safe switching boundary, provide a configurable GC3 policy:

```text
StrictSegmentBoundary
ExperimentalTimeQuantum
```

### StrictSegmentBoundary

The Coupler may expire a quantum, but ownership changes only when the current endpoint reaches a DLP-safe segment boundary.

This is useful for bringing up the rest of the stack, but must be marked as a conservative simulation policy rather than the final normative GC3 rule.

### ExperimentalTimeQuantum

Permit may be removed exactly at the configured physical boundary, following the current time-quantum concept.

The DLP receivers then run normally. If interleaving makes receiver state ambiguous, the simulation should report the failure/ambiguity.

This mode is specifically useful for validating whether the present DLP/GC3 rules are actually sufficient.

Do not invent hidden source tags or extra flit fields inside the simulator.

---

# 7. GS3 model

## 7.1 Architectural rule

GS3 is fundamentally different from GC3.

GS3 **is an active forwarding endpoint**.

It has:

- independent physical ports;
- ingress receive capacity;
- egress credit relationships;
- a local GDP-address-to-port attachment map;
- small internal flit buffers;
- destination-specific forwarding;
- egress/path arbitration;
- simultaneous non-conflicting paths.

A high-level model is:

```text
          +---------------------------------+
Port A -->| ingress A ->                    |--> Port B
Port C -->| ingress C -> forwarding fabric  |--> Port D
Port E -->| ingress E ->                    |--> Port F
          +---------------------------------+
```

Unlike GC3, data received on port A is not repeated to every port.

## 7.2 GS3 ports

Each port should contain two distinct sides of link state:

```rust
struct Gs3Port {
    ingress: IngressState,
    egress: EgressState,
    attachment_state: AttachmentState,
    control: GsControlState,
}
```

Important concepts include:

```text
ingress buffer capacity
credits GS3 has advertised to attached endpoint
egress credits received from attached endpoint
current incoming segment/VC state
current outgoing segment/VC state
link/control-pair readiness
```

Do not mirror ingress and egress credit counts.

## 7.3 Attachment map

GS3 needs a map:

```text
canonical/local GDP address -> physical port
```

The GNet specification states that the switch learns an endpoint's usable GDP address after the endpoint announces it.

The exact announcement mechanism should not be invented in this simulator if the wire encoding is not yet frozen.

For the first fixed simulations, support a static scenario-provided attachment map:

```text
A address -> port 1
B address -> port 2
C address -> port 3
```

Later, when the address-announcement behavior is fully implemented in `smolgnet`, replace or supplement this with real data-path learning.

The forwarding fabric should use the same attachment-map interface in both cases.

## 7.4 Ingress processing

An arriving flit consumes actual GS3 ingress capacity.

The switch should keep only small bounded buffers.

Conceptually:

```text
port A DATA-UP
      |
      v
ingress flit buffer
      |
      v
incremental GDP header inspection
      |
      v
destination / egress known
      |
      v
output arbitration
      |
      v
egress flit stream
```

Because DLP VCIDs are hop-local, the switch should not assume that an ingress VCID is copied unchanged onto the egress link.

Ingress and egress DLP state are separate.

## 7.5 Header inspection and cut-through

GS3 should use the same incremental GDP decoder intended for GRouterD.

The switch can observe the destination before the entire packet has arrived, but it should not forward a GDP packet as valid until the required header validation policy is satisfied.

A simple safe initial model is:

```text
receive enough GDP header flits
validate GDP header CRC
resolve destination port
begin cut-through forwarding
stream remaining payload flits
```

For Global GDP this means only a small fixed header delay before forwarding begins; for Local form the delay is smaller.

Do not buffer the complete GDP packet merely because it simplifies the simulator.

The entire purpose is to exercise small-buffer wormhole/cut-through behavior.

## 7.6 Independent credit relationships

The defining GS3 credit model is:

```text
A <--- link credits ---> GS3 <--- link credits ---> B
```

These are two separate relationships.

Suppose GS3 ingress A has room for eight more flits while B currently advertises zero receive credit.

GS3 may still advertise/retain ingress credit toward A and accept those flits until its own bounded ingress/path buffer fills.

When its internal buffer is full, it stops replenishing A's credits.

When B later returns credit, GS3 drains buffered flits to B and can again replenish A's credit.

This gives the simulation a concrete backpressure chain:

```text
B stops accepting
      |
GS3 egress stalls
      |
GS3 buffer fills
      |
GS3 stops returning ingress credit
      |
A eventually runs out of link_flit_credit
      |
A stops sending
```

No explicit end-to-end switch credit propagation is required.

## 7.7 GCTL credit handling at GS3

Unlike GC3, GS3 must process the link-local credit GCTL messages directed at its own port relationship.

When endpoint A sends a `GCTL CREDIT_REQUEST` for A's link to GS3:

```text
A -> GDP/GCTL -> GS3
```

GS3 consumes it and returns a real `GCTL CREDIT` packet describing GS3's own ingress capacity.

This reply:

- travels on the data channel;
- consumes link bandwidth;
- is generated by the GS3 model's GCTL/link-credit engine;
- does not depend on destination B's current credit balance.

Similarly, the GS3 egress side obtains credit from B through the normal GCTL exchange between GS3 and B.

The simulator should therefore include enough GDP/GCTL endpoint logic inside the GS3 to act as a link-credit peer, without turning GS3 into a general GTS endpoint.

A clean implementation is to reuse lower-level `smolgnet` wire/GCTL/link components rather than hand-encode credit packets in simulation code.

## 7.8 Internal buffering

The GS3 simulation needs bounded internal flit storage, but it does not need a detailed ASIC model.

Start with configurable per-ingress or per-ingress/egress queues:

```text
gs3_ingress_capacity_flits = N
```

Track:

- current occupancy;
- maximum occupancy;
- time blocked by egress credit;
- time blocked by output contention;
- drops/aborts if the model reaches an impossible state.

The normal credit mechanism should prevent overflow. An overflow should therefore be considered a protocol/implementation error unless a deliberate fault test caused it.

## 7.9 Output arbitration

If two ingress streams want the same egress port, choose a deterministic fair policy such as round-robin.

Example:

```text
A -->\
      >--- egress B
C -->/
```

Only one can use B's outgoing data channel at a time.

But disjoint transfers can proceed simultaneously:

```text
A ---> B
C ---> D
```

The simulator must allow both paths to serialize flits concurrently on their independent links.

This is the central throughput distinction between GC3 and GS3.

## 7.10 GS3 control pair

The exact remaining GS3 control-pair runtime protocol is still under revision.

Do not invent its final wire encoding merely to complete the simulator.

Represent it behind an interface such as:

```rust
trait GsControlModel {
    fn ingress_ready(&self, port: PortId) -> bool;
    fn egress_permitted(&self, port: PortId) -> bool;
    fn poll(...);
}
```

For the first GS3 model, a simple reference profile may assume that after bootstrap every healthy point-to-point port is physically ready, while all real flow control is still exercised through GCTL credits and switch output arbitration.

When GS3 control-pair path setup/permission semantics are frozen, add a second implementation without changing the GS3 forwarding core.

This prevents speculative control-pair behavior from contaminating the forwarding model.

---

# 8. Endpoint model

A simulated endpoint should be a thin harness around actual `smolgnet` objects.

Conceptually:

```rust
struct SimEndpoint {
    link: SimGnetLinkDevice,
    dlp: DlpEndpoint,
    iface: Interface,
    sockets: SocketSet,
}
```

The exact ownership model may differ due to Rust borrowing constraints.

The endpoint should be capable of:

- static GDP address configuration;
- sending/receiving GCTL;
- opening GTS tunnels;
- sending fixed scripted application messages;
- exposing trace/statistics hooks.

Do not create a special simulator-only GNet stack.

---

# 9. Static scenario model

Keep scenario definition deliberately small.

A scenario can be ordinary Rust test code rather than a new configuration language.

Example concept:

```rust
Scenario::new()
    .endpoint("A", addr_a)
    .endpoint("B", addr_b)
    .endpoint("C", addr_c)
    .gc3("lan", 3_000_000, 8)
    .connect("A", "lan", 1)
    .connect("B", "lan", 2)
    .connect("C", "lan", 3)
    .flow("A", "B", Workload::messages(100, 512))
    .flow("C", "B", Workload::messages(100, 512));
```

The API above is illustrative, not frozen.

Prefer explicit Rust fixtures so it remains easy to debug and refactor with protocol changes.

---

# 10. Workload model

Do not build a synthetic traffic-research framework.

We need only a few deterministic workloads:

```text
SingleTransfer
TwoIndependentTransfers
TwoToOneContention
BidirectionalTransfer
ManySmallMessages
FewLargeMessages
CreditStarvationTest
SlowReceiverTest
```

Each workload should use actual GDP/GTS messages generated by smolgnet where practical.

Examples:

### Small-message workload

```text
1000 messages
32 or 64 byte GDP payload class
one sender, one receiver
```

### Bulk workload

```text
repeated 1024 / 1500 / 4096 byte GDP classes
sender always has data available
```

### Contention workload

```text
A -> B
C -> B
```

### Independent GS3 workload

```text
A -> B
C -> D
```

This workload should clearly show the GS3 aggregate-throughput advantage over GC3.

---

# 11. Metrics

Keep metrics small and protocol-focused.

Every simulation should be able to report:

```text
simulated elapsed time
application messages generated
application messages delivered
application payload bytes delivered
GDP packets/flits transmitted
data-channel utilization
useful application goodput
GCTL/control flits and bytes
```

GC3-specific metrics:

```text
WANT wait time per endpoint
PERMIT time per endpoint
ownership changes
quantum expirations
medium idle time
medium utilization
```

GS3-specific metrics:

```text
ingress occupancy/high-water mark
egress credit stall time
output-contention stall time
per-port utilization
concurrent active path count
switch aggregate useful throughput
```

Credit metrics:

```text
CREDIT_REQUEST count
CREDIT count
flit credits advertised
flit-credit stall time
```

GTS metrics may include:

```text
GTS receive-credit stalls
ACK count
retransmissions
```

but the infrastructure simulator should not duplicate GTS accounting already available in the stack.

Latency can be recorded simply as message injection-to-delivery time. We do not need a sophisticated statistics package; min/mean/max and optionally a few percentiles are enough.

---

# 12. Comparison methodology

GC3 and GS3 should be compared using the **same endpoints, same GNet stack, same GDP/GTS messages, same line rate and same workload**.

Only the infrastructure component changes.

Example:

```text
Scenario A

A -> B continuously
C -> D continuously

all links 3 Mbit/s
```

Run once as:

```text
A --\
C --- GC3 --- B/D
```

and once as independent GS3 ports.

Expected architectural result:

- GC3 shares one 3 Mbit/s data resource across all active senders;
- GS3 can sustain multiple independent 3 Mbit/s paths when egresses do not conflict.

For a two-to-one workload:

```text
A -> B
C -> B
```

both systems face a 3 Mbit/s bottleneck toward B, but their arbitration, buffering and credit behavior differ.

The simulator should show those mechanisms rather than merely outputting a theoretical bandwidth equation.

---

# 13. Tracing

A readable trace is more important than elaborate visualization.

Support optional events such as:

```text
0.000000 A WANT=1
0.000003 GC3 A PERMIT=1
0.000014 A TX VC1 0x....
0.000025 B RX VC1 0x....
...
0.000100 GC3 A PERMIT=0
0.000101 A WANT=0
0.000102 C PERMIT=1
```

and for GS3:

```text
A:RX flit
GS3:p1 ingress occupancy=3
GS3 GDP destination=B -> p2
GS3:p2 blocked credit=0
B sends GCTL CREDIT
GS3:p2 credit=8
GS3:p2 TX flit
GS3:p1 frees slot / replenishes ingress capacity
```

Trace output should be deterministic and suitable for golden tests.

---

# 14. Assertions and invariants

The simulator is primarily a verification tool. Add strong assertions.

## GC3 invariants

- no more than one port has `PERMIT=1`;
- only a permitted port may drive the shared data resource;
- GC3 never changes credit state;
- GC3 never parses GDP/GCTL;
- release handshake ordering is respected;
- all receiving ports observe identical repeated flits for one owner transmission.

## GS3 invariants

- ingress credit advertised never exceeds real guaranteed ingress capacity;
- egress transmission never occurs without downstream link credit;
- an ingress and an egress credit balance are never implicitly mirrored;
- internal buffers never exceed configured capacity;
- one egress transmits at most one selected ingress stream at a time;
- disjoint egresses may transmit concurrently;
- DLP VC state is hop-local and ingress VC identity is not assumed to be egress VC identity;
- malformed GDP is never forwarded as valid.

## Common invariants

- GCTL credit traffic uses the data channel;
- physical control-pair state does not consume data bandwidth;
- GTS receive credit is not used as DLP link credit;
- simulation time never runs backward;
- repeated run with identical inputs produces identical output.

---

# 15. Suggested source structure

Add a small simulation tree separate from protocol implementation:

```text
src/
    ... real smolgnet stack ...

sim/
    mod.rs
    clock.rs
    link.rs
    endpoint.rs
    gc3.rs
    gs3.rs
    scenario.rs
    metrics.rs
    trace.rs
```

Alternatively, if simulation should not be part of the library API, place it under:

```text
tests/sim/
```

The infrastructure models should use public/internal `smolgnet` link abstractions rather than duplicate protocol internals.

A useful compromise is:

```text
src/phy/sim.rs            simulated GnetLinkDevice primitive

tests/sim/                GC3/GS3/scenario harness
```

so the reusable simulated NIC exists in the library while the comparison harness remains test code.

---

# 16. Recommended implementation sequence

## Step 1 — virtual point-to-point link

Before GC3 or GS3, implement two endpoints connected by a simulated native GNet link:

```text
A ---------------- B
```

Exercise:

- data flits;
- separate control channels;
- DLP credits through GCTL;
- basic GDP/GCTL exchange.

This proves the simulation clock and native device abstraction.

## Step 2 — two-node GC3

```text
A --- GC3 --- B
```

Implement:

- WANT/PERMIT;
- round-robin arbitration;
- shared flit repetition;
- configurable quantum;
- real GCTL credits through the data channel.

Verify `PERMIT` and credit independence.

## Step 3 — multi-node GC3

```text
A ---\
B ---- GC3
C ---/
```

Test contention, fairness and release behavior.

Specifically investigate the mid-segment ownership-switch ambiguity rather than hiding it.

## Step 4 — two-node GS3

```text
A --- GS3 --- B
```

Implement:

- static attachment map;
- bounded ingress buffering;
- independent A<->GS3 and GS3<->B credits;
- incremental GDP header inspection;
- cut-through after header validation.

## Step 5 — GS3 egress contention

```text
A --\
     GS3 --- B
C --/
```

Verify output arbitration and backpressure.

## Step 6 — GS3 independent paths

```text
A ---> B
C ---> D
```

Verify both transfers run concurrently and aggregate throughput exceeds the single shared GC3 medium.

## Step 7 — actual GTS workloads

Run GTS message streams over both infrastructures rather than synthetic raw GDP alone.

This validates that link-level control behavior composes correctly with transport-level ACK and receive-credit behavior.

---

# 17. What the first useful comparison should answer

The first performance comparison does not need to claim realistic enterprise-network performance.

It should answer concrete implementation questions such as:

1. Does GC3 arbitration preserve forward progress with several busy senders?
2. How much GC3 data bandwidth is consumed by real GCTL credit traffic?
3. Does physical `PERMIT` interact correctly with receiver flit credit?
4. Does a slow receiver naturally backpressure its sender through credits?
5. Does GS3 correctly decouple source-to-switch credit from switch-to-destination credit?
6. How much buffering does GS3 need before backpressure reaches an ingress?
7. Can two disjoint GS3 transfers actually progress concurrently through the implementation?
8. What throughput improvement does GS3 provide over GC3 for independent conversations at the same 3 Mbit/s port rate?
9. Under two-to-one contention, does the GS3 egress scheduler behave fairly?
10. Do GTS reliable streams continue to behave correctly when the lower layers stall and resume?
11. Does the current GC3 time-quantum/pause model reveal an unresolved DLP multiplexing problem?

Those answers are more valuable at this stage than a sophisticated network-performance simulator.

---

# 18. Non-goals

Do not add, unless later explicitly needed:

```text
arbitrary graph topology language
large-scale internet simulation
routing convergence simulation
probabilistic traffic generators
realistic CPU instruction timing
analog signal/cable models
queueing-theory framework
statistical experiment suite
GUI topology editor
packet-capture replacement
full hardware switch-fabric timing model
```

A handful of deterministic Rust scenarios should remain sufficient.

---

# 19. Design principle

The simulation exists to make GNet implementation assumptions executable.

For GC3, model as little intelligence as possible:

```text
control arbitration + shared repetition
```

For GS3, model exactly the intelligence its architecture requires:

```text
active ingress endpoint
+ destination lookup
+ bounded flit buffering
+ independent downstream credit
+ output arbitration
+ cut-through forwarding
```

Everything above those infrastructure functions should remain ordinary `smolgnet` protocol code.

This gives us a useful progression:

```text
endpoint <-> endpoint
        |
       GC3
        |
       GS3
        |
 future router
```

without turning `smolgnet` into a general networking simulation package.