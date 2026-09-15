# DLP Link Control v0.1

Status: **freeze candidate** for the native point-to-point DLP software reference.

The implementation follows the current GNet architectural split:

- the dedicated physical control pair performs attachment/bootstrap, capability negotiation, VC/profile establishment and reset/recovery;
- receiver credit is strictly hop-local but is carried by `GCTL CREDIT_REQUEST` / `GCTL CREDIT` on the normal DLP data path;
- GDP/GTS only become usable after control-pair negotiation and initial data-path credit synchronization complete.

## Architecture

```text
GTS
 |
GDP
 |
+---------------- normal DLP data path ----------------+
|                                                     |
| VC1..N  GDP/GTS data                                |
| VC0     GDP/GCTL CREDIT_REQUEST / CREDIT             |
|                                                     |
+-----------------------------------------------------+
 |
DLP datapath
 |
+---------------- physical control pair --------------+
| HELLO                                               |
| CAPABILITIES offer / selection / confirmation       |
| LINK_PARAMETERS offer / selection / confirmation    |
| RESET                                               |
+-----------------------------------------------------+
 |
physical link
```

There is deliberately no steady-state `CREDIT`, `CREDIT_REQUEST`, `GRANT`, `RESET_ACK` or `LINK_UP` opcode on the physical control pair.

A reset is not itself a reliable transaction. A new `HELLO(initial)` with a new link generation independently replaces old hop-local state, matching the GNet GLCP design.

## 32-bit GLCP control flits

Every physical-control operation is exactly one 32-bit logical control flit unless a separately specified continuation format is used. These bootstrap flits carry no DLP VCID and consume no DLP receive credit.

### HELLO

The implementation uses the accepted GNet 0.1 layout:

```text
31      28 27      24 23  22 21          16 15          10 9        0
+----------+----------+------+------+--------------+--------------+----------+
| opcode=1 | version=1| kind | SGEN |     PGEN     |   reserved   |
+----------+----------+------+------+--------------+--------------+----------+
```

`kind=0` is `initial`; `kind=1` is `acknowledgement`. Sender and peer generations are six bits. Generation zero is not used as a live sender generation.

### CAPABILITIES

The accepted baseline layout is used directly:

```text
31      28 27      24 23          18 17  16 15      12 11     9 8        0
+----------+----------+--------------+------+------+----------+---------+----------+
| opcode=2 | version=1|     SGEN     | kind | profile  | rate  | reserved |
+----------+----------+--------------+------+------+----------+---------+----------+
```

Kinds are:

- `0 = offer`
- `1 = selection`
- `2 = confirmation`

The executable smolgnet model retains its current `VcMode::Two` / `VcMode::Four` profile model and deterministically selects the highest common mode. Rate negotiation chooses the highest common advertised rate bit.

### RESET

The accepted GNet 0.1 reset layout is used:

```text
31      28 27      24 23          18 17      14 13       0
+----------+----------+--------------+----------+----------+
| opcode=3 | version=1|     SGEN     |  reason  | reserved |
+----------+----------+--------------+----------+----------+
```

Acceptance of RESET invalidates queued DLP traffic, partial GDP reassembly, VC state, advertised/received credit and negotiated profile state for the current link generation.

## LINK_PARAMETERS extension

The current GNet documents reserve advanced capability negotiation for larger receive windows and implementation acceleration parameters but do not yet assign an exact compact wire encoding for buffer/burst values. The smolgnet v0.1 executable reference therefore uses one generation-scoped 32-bit extension opcode:

```text
31      28 27      24 23          18 17  16 15       8 7        0
+----------+----------+--------------+------+-----------+----------+
| opcode=8 | version=1|     SGEN     | kind | RX units  | burst u. |
+----------+----------+--------------+------+-----------+----------+
```

`kind` uses the same offer/selection/confirmation values as CAPABILITIES. RX and burst units are multiples of four physical flits, allowing values from 4 through 1020 flits in this v0.1 encoding.

The selection contains:

- the offered receiver's own RX window;
- the smaller of the two offered burst limits.

Both sides confirm the exact selected values before installing the DLP datapath.

## Fixed VC0 baseline

The v0.1 managed link reserves **32 physical flits** of receive capacity for VC0 link-local GCTL traffic.

This is a fixed compatibility property, not ordinary receiver credit and not a peer-specific value discovered out of band. `DlpManagedEndpoint` therefore rejects a managed-link configuration that attempts to use a different VC0 window.

The fixed window is large enough for the baseline GCTL credit-control packets and prevents a data-credit deadlock: a sender with zero ordinary data credit can still send `GCTL CREDIT_REQUEST`.

## Bring-up state machine

```text
DOWN
 |
 | carrier present
 v
HELLO
 |
 | initial / acknowledgement exchange
 v
NEGOTIATE
 |
 | CAPABILITIES offer/select/confirm
 | LINK_PARAMETERS offer/select/confirm
 v
CREDIT_SYNC
 |
 | instantiate fresh negotiated DLP datapath
 | enable fixed 32-flit VC0 compatibility window
 | exchange initial GCTL CREDIT on normal data path
 v
UP
```

No ordinary receive credit is seeded by HELLO, CAPABILITIES or LINK_PARAMETERS. Those messages establish capabilities and limits only.

`DlpDirectCable::attach` reports the link usable only after both peers have completed the first real GCTL credit exchange and therefore have nonzero ordinary transmit credit.

The cable model does not inspect peer configuration to accomplish this. After carrier assertion, all peer-visible state is conveyed by physical-control or DLP-data traffic.

## Receiver-credit semantics

> One GNet credit is guaranteed receive capacity for one physical flit at the adjacent forwarding endpoint.

### Initial credit

After control-pair negotiation:

1. each side creates a fresh `DlpEndpoint` with the negotiated VC mode;
2. the fixed 32-flit VC0 compatibility window is enabled so link-local GCTL cannot deadlock;
3. each side binds the adjacent peer's link-local GDP address;
4. each side computes actual `grantable_data_credit()`;
5. each side emits `GCTL CREDIT` on VC0 of the normal data path;
6. only that transmitted grant becomes outstanding advertised receive credit.

### Steady-state return

When data receive storage is released, the receiver computes:

```text
grantable = free receive flits - already advertised but unconsumed credit
```

If the configured batching threshold is reached, it sends another `GCTL CREDIT` on VC0/data.

If queued data requires more flits than current transmit credit, the sender emits `GCTL CREDIT_REQUEST` on VC0/data. The receiver answers with at most the requested amount and never more than real grantable capacity.

A lost CREDIT_REQUEST is retried after a bounded number of stalled transmit polls.

### Safety invariants

The managed path enforces:

```text
transmit credit <= negotiated peer RX capacity
advertised credit <= actual local grantable capacity
received data flits <= outstanding advertised credit
```

An excessive GCTL CREDIT that would push transmit credit beyond the negotiated peer RX window is rejected with `CreditViolation`.

Data arriving without sufficient advertised credit is likewise rejected.

### Reserved VC0 capacity

VC0 is the normal-data-path lane used by link-local GCTL. Its fixed receive reserve means credit-control messages do not depend on the ordinary data credit they are managing.

Returning a consumed VC0 slot is an implementation-local reserved-lane operation. It is not an ordinary receiver-credit grant and never changes the ordinary data-credit balance.

## Generation and stale-state handling

Every negotiated control operation is scoped to the peer generation established by HELLO.

- CAPABILITIES and LINK_PARAMETERS from a non-current generation are discarded.
- a duplicate HELLO for the current generation is harmless;
- while carrier remains present, only the exact next six-bit generation may supersede the current generation;
- older or skipped-generation HELLO traffic is treated as stale/reordered and discarded;
- RESET is accepted only for the currently established peer generation;
- accepting or initiating RESET clears the remembered peer generation before the new HELLO exchange, so a duplicate old RESET cannot repeatedly destroy the new negotiation;
- carrier-down clears the known peer generation entirely, allowing a newly attached peer to establish any valid nonzero initial generation.

This generation boundary prevents stale control traffic from restoring old VC or credit state after recovery.

## Reset / partial receive recovery

Reset or a new generation discards:

- queued data and link-control GDP packets;
- partially received GDP frames;
- VC desynchronization/reassembly state;
- transmit ordinary credit;
- outstanding advertised ordinary credit;
- reserved control-lane state;
- negotiated profile and burst/window values.

Negotiation then begins again and fresh GCTL credit is exchanged. Old credit cannot survive into the new link generation.

## GTS end-to-end proof

The integration path is:

```text
two unconfigured hosts
        |
        v
32-bit GLCP HELLO
        |
        v
CAPABILITIES + LINK_PARAMETERS negotiation
        |
        v
initial GCTL CREDIT over VC0/data
        |
        v
DLP UP
        |
        v
GTS CONNECT / CONNECT_ACK
        |
        v
reliable bidirectional stream traffic
```

The hardening suite additionally exhausts link credit on an established GTS stream, deliberately loses the first GCTL CREDIT_REQUEST, verifies bounded retransmission, applies the returned GCTL CREDIT and continues the same stream.

## Hardening coverage

`tests/dlp_link_control.rs` covers:

- exact 32-bit HELLO, CAPABILITIES, RESET and LINK_PARAMETERS encodings;
- reserved-field and unsupported-version rejection;
- malformed HELLO / parameter fields;
- asymmetric VC capability negotiation;
- asymmetric receive buffers and burst limits;
- initial credit via GCTL rather than the physical control pair;
- steady-state GCTL CREDIT generation on the data path;
- two initially unconfigured hosts establishing GTS;
- lost GCTL CREDIT_REQUEST retry;
- incompatible capability rejection;
- stale-generation control traffic;
- duplicate HELLO handling;
- selection-before-offer rejection;
- credit overflow / underflow;
- excessive GCTL CREDIT rejection;
- reset during partial GDP reception;
- asymmetric reset;
- complete renegotiation and fresh GTS after reset.

## Software / FPGA boundary

A hardware implementation can own:

```text
carrier detection
32-bit physical control-pair RX/TX
HELLO generations
CAPABILITIES / LINK_PARAMETERS state
RESET handling
negotiated profile registers
fixed VC0 reserve
ordinary credit counters
VC scheduling and GDP reassembly
LINK_UP indication to software
```

`GCTL CREDIT_REQUEST` and `GCTL CREDIT` remain ordinary GDP packets architecturally carried on the DLP data path. Hardware may recognize and accelerate them without changing that wire architecture.

The GDP/P4 boundary remains complete GDP packets; P4/router code does not receive DLP flits, credit counters or partial reassembly state.

## Freeze criterion

The DLP v0.1 semantics in this branch are intended to be frozen once the dedicated DLP link-control CI and the existing full GNet backend matrix pass on this exact branch head. No merge is implied by that freeze; PR #14 remains a review branch until explicitly merged.
