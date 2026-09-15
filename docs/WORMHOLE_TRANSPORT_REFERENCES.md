# Wormhole transport and fabric references

GNet combines a credit-controlled routed fabric with an end-to-end transport. It should therefore be compared with lossless HPC/interconnect designs as well as with conventional packet networks.

## Useful precedents

### Myrinet

Myrinet is a primary reference for a small-buffer wormhole/cut-through fabric with backpressure. Its GM/MX software stacks also demonstrate how useful reliable communication semantics can be layered over such a fabric.

### InfiniBand

InfiniBand is particularly relevant because it combines credit-controlled links and Virtual Lanes with end-to-end reliable transport modes such as Reliable Connection (RC). The analogy is not exact, but it is useful:

| GNet | InfiniBand concept |
| --- | --- |
| GDP address/routing | LID/GID and fabric routing |
| DLP credit | link-level credit flow control |
| DLP VC | Virtual Lane |
| GTS tunnel/session | connection / Queue Pair role |
| GTS sequence and acknowledgement | packet sequence / acknowledgement role |

InfiniBand routing work is also useful for GNet's deadlock problem. OpenSM routing algorithms such as LASH and research such as Nue demonstrate that Virtual Lanes can be used as part of topology-independent deadlock-free routing strategies. This is complementary to, not a replacement for, the VC0 escape policy described in `avoid circular dependency.md`.

### Quadrics QsNet / Elan

Quadrics is another close architectural precedent: a low-latency, flow-controlled HPC fabric with source/network routing and reliable user-level communication. It is useful when considering how much transport work should be done by endpoints versus the routed fabric.

### Cray SeaStar, Gemini and Aries

Cray interconnects provide further examples of high-performance routed fabrics using virtual channels, flow control and hardware-supported reliable communication. They are especially useful references for arbitration, congestion behaviour and large-scale routing.

### Intel Omni-Path

Omni-Path is useful as another modern lossless HPC-fabric reference, including virtual-lane-like traffic separation, flow control and reliable communication semantics.

## Why GTS should not simply copy TCP congestion behaviour

TCP evolved primarily for packet networks where congestion is commonly signalled by packet loss or explicit congestion notification. A wormhole/credit-controlled GNet link behaves differently:

```text
congestion
    -> receive credit is consumed
    -> backpressure propagates
    -> a worm/frame waits
```

This makes TCP-like end-to-end properties useful -- ordering, sequence numbers, acknowledgements, reset/recovery and retransmission when data is genuinely lost -- while making packet-loss-driven congestion control a poor default model for the fabric itself.

GTS therefore owns end-to-end transport semantics. DLP owns link-local credits, VC selection and physical scheduling. GDP owns addressing and routing. GDP and GTS must not select numeric VCIDs.

## Consequence for router scheduling

A lossless fabric still requires fair arbitration. Backpressure prevents buffer overflow; it does not guarantee fairness. A continuously busy flow must not monopolize one of a small number of data VCs and indefinitely prevent another flow from entering the link.

For the current VC4 DLP profile:

```text
VC0       control / reserved escape role
VC1-VC3   ordinary routed data
```

The router/link egress scheduler should therefore maintain per-flow queues, admit at most the available ordinary data VC capacity, grant one packet quantum to an admitted flow, and rotate fairly among waiting flows. Numeric VC assignment remains a DLP concern.

This is the production counterpart of the router-to-router scheduling experiment introduced in PR #21. The integration test previously supplied this admission policy with a test-only helper; production code must own it before more sophisticated adaptive routing is added.

## Design rule

Do not confuse three independent properties:

1. **Reliability:** GTS ensures the required end-to-end delivery/ordering semantics.
2. **Flow control:** DLP credit prevents a sender from overrunning downstream receive capacity.
3. **Fairness/deadlock freedom:** router arbitration and routing policy must independently guarantee progress and avoid circular resource dependencies.

A reliable transport cannot repair a permanently deadlocked wormhole fabric, and credit flow control by itself does not provide fair scheduling.

## Further reading

- Myrinet / GM / MX architecture and software literature.
- InfiniBand Architecture Specification, especially Reliable Connection, flow control and Virtual Lanes.
- OpenSM routing documentation, including LASH.
- ETH Zurich SPCL Nue: topology-agnostic deadlock-free InfiniBand routing with a finite number of Virtual Lanes.
- Quadrics QsNet / Elan architecture literature.
- Cray SeaStar, Gemini and Aries interconnect architecture literature.
- Intel Omni-Path architecture documentation.
- `docs/avoid circular dependency.md` for GNet's current VC0/deadlock model.
