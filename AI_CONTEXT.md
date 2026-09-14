# smolgnet AI Context

## Purpose

`smolgnet` is the native Rust implementation of the GNet protocol suite.

The repository began as a fork of `smoltcp`, but upstream compatibility is **not** a goal. We are free to remove, rewrite, or replace the existing TCP/IP implementation as GNet functionality becomes available. The useful inheritance from smoltcp is architectural and implementation-oriented: explicit buffers, bounded resource use, no mandatory heap, poll-driven state machines, checked packet parsing, device abstractions, timers, tracing, fault injection, fuzzing, and embedded/bare-metal suitability.

The end state should be a GNet library, not a TCP/IP library with GNet added beside it.

The canonical protocol specification lives in `nickik/GNet`. When code, old chats, archived documents, or inherited smoltcp behavior conflict with the current GNet specification, the newest accepted GNet specification wins.

---

# 1. Architectural decisions already made

The following choices are intentional and should be treated as project architecture unless explicitly revisited.

## 1.1 Two device layers, not one

Do **not** force native GNet into smoltcp's whole-frame `Device` abstraction.

Use two distinct concepts:

```text
PacketDevice
GnetLinkDevice
```

### PacketDevice

A convenience/adaptation interface for complete bounded packets. It is useful for:

- early GDP/GCTL/GTS development;
- in-memory testing;
- loopback;
- UDP or host-process simulation;
- test harnesses;
- non-native encapsulations.

A packetized test device may treat one frame as one complete GDP packet.

It must be clearly documented as an adaptation or simulation medium. It is **not** the native GNet link model.

### GnetLinkDevice

The native lower-level interface to a GNet physical attachment.

It must represent the fact that a native GNet attachment has logically separate physical channels:

```text
Native GNet attachment
|
+-- data channel
|   +-- DLP flits
|   +-- GDP
|   +-- GCTL
|   +-- GTS
|
+-- control channel / dedicated control pair
    +-- bootstrap
    +-- link/infrastructure identification
    +-- physical capability negotiation
    +-- physical rate negotiation
    +-- GC3 WANT/PERMIT
    +-- GS3-local path/switch control where defined
```

Do not serialize these two channels into one fake byte stream in the native implementation.

---

## 1.2 DLP boundary

Use this layering:

```text
GnetLinkDevice   physical I/O and physical control channel
      |
DlpEndpoint      VC / credit / segment / link state
      |
Interface        GDP / GCTL / GTS endpoint dispatch
```

`DlpEndpoint` must be reusable outside an endpoint stack, especially by the future `GRouterD` router implementation.

It should not depend on application sockets.

---

## 1.3 GTS socket model

Use:

```text
GtsSocket = one GTS tunnel
StreamHandle = one subchannel inside that tunnel
```

Do **not** model every GTS stream as an independent smoltcp-style socket.

Tunnel-level state is shared across streams:

- local and remote Tunnel IDs;
- local and remote Reset IDs;
- CSS service binding;
- initiator/responder role;
- stream-ID allocation/parity;
- tunnel close/reset state;
- stale-state protection.

Streams are children of a tunnel.

---

## 1.4 No upstream-compatibility goal

Once GNet work begins, smoltcp ancestry is historical, not architectural authority.

We may eventually remove:

```text
Ethernet
ARP
IPv4
IPv6
TCP
UDP
ICMP
DHCP
DNS
6LoWPAN
RPL
IPsec
multicast
IP fragmentation
```

Do not preserve an IP abstraction merely because upstream smoltcp has one.

Preserve generic infrastructure only when it remains useful for GNet.

---

# 2. Core GNet layering

The intended stack is:

```text
Application / service API
        |
       GTS
        |
   GDP / GCTL
        |
       DLP
        |
 native GNet physical link
```

GLCP/control-pair behavior is physically adjacent to DLP but is **not** another GDP-carried protocol.

A more precise view is:

```text
                         Application
                              |
                             GTS
                              |
                   +----------+----------+
                   |                     |
                  GDP                   GCTL
                   |                     |
                   +----------+----------+
                              |
                         DlpEndpoint
                         /         \
                        /           \
              data-channel      control-pair state
                    |                 |
              DLP flits          GLCP / GC3 / GS3
                    \                 /
                     \               /
                       GnetLinkDevice
```

This split is fundamental.

---

# 3. Control-channel split

GNet has two different kinds of control and they must remain distinct in software.

## 3.1 Dedicated control-pair control

The separate physical control pair is for directly attached infrastructure functions only.

Typical responsibilities:

- presence / synchronization;
- bootstrap identification;
- identifying GC3 vs GS3 vs direct/native link roles;
- physical capability negotiation;
- data-rate negotiation;
- reset / physical recovery;
- GC3 `WANT` / `PERMIT`;
- GS3-local path or switch permission where the specification defines it.

These operations live below GDP and do not have GDP addresses, GDP Size Classes, or GTS semantics.

The native driver should expose the control channel separately from the data channel.

Possible conceptual API:

```rust
pub trait GnetLinkDevice {
    type DataRx<'a> where Self: 'a;
    type DataTx<'a> where Self: 'a;

    fn receive_data(&mut self, now: Instant) -> Option<Self::DataRx<'_>>;
    fn transmit_data(&mut self, now: Instant) -> Option<Self::DataTx<'_>>;

    fn control(&mut self) -> &mut impl GnetControlChannel;
    fn capabilities(&self) -> LinkCapabilities;
}
```

The exact Rust API may change, but the **separate-channel model must not**.

## 3.2 GCTL control

GCTL is completely different.

GCTL is carried as normal GDP traffic on the **data channel**.

Important examples:

```text
CREDIT_REQUEST
CREDIT
ECHO_REQUEST
ECHO_REPLY
STATUS_REQUEST
STATUS_REPLY
DESTINATION_UNREACHABLE
HOP_LIMIT_EXCEEDED
PARAMETER_PROBLEM
CLASS_UNSUPPORTED
TRANSIT_ABORTED
PATH_PROBE
PATH_REPLY
```

GCTL messages use GDP addressing and GDP Size Classes.

They are not sent over the physical control pair.

## 3.3 Cross-layer interaction

Some GCTL messages modify lower-layer state.

The main example is link credit:

```text
GCTL CREDIT_REQUEST / CREDIT
        |
        v
DlpEndpoint link-credit state
        |
        v
DLP transmitter admission
```

This is an intentional cross-layer interaction.

A credit means:

> guaranteed receive capacity for exactly one physical flit at the next forwarding endpoint on the current link.

Credits are strictly link-local.

Do **not** reinterpret credits as physical-medium permission.

## 3.4 Physical permission and receive credit are independent

A sender may transmit only if all required lower-layer conditions are satisfied.

Conceptually:

```text
may_send_data = physical_path_permitted
                AND receive_credit_available
                AND local_transmitter_ready
```

Example on GC3:

```text
PERMIT = 1
credit = 0
```

The endpoint owns the shared medium but still cannot legally send another credit-consuming flit.

Conversely:

```text
PERMIT = 0
credit > 0
```

The receiver can accept data, but the endpoint does not currently own the physical medium.

Never merge these state variables.

---

# 4. Native flit model

The baseline native data flit is:

```text
[ VCID:2 | carried data:32 ]
```

Important consequences:

- VCID is hop-local;
- the physical data path is naturally flit-oriented;
- there is no Ethernet-style MAC source/destination header;
- there is no per-packet SOF bit in the accepted baseline;
- DLP does not carry a duplicate packet-size field;
- native GDP transfer length is determined from GDP Size Class.

A native implementation should process **bursts of flits**, not incur a virtual/trait call for every individual 34-bit flit.

Suggested direction:

```rust
pub struct Flit {
    pub vcid: Vcid,
    pub data: u32,
}
```

with batched receive/transmit operations.

---

# 5. DlpEndpoint responsibilities

`DlpEndpoint` is the reusable hop-local data-path engine.

It should own or coordinate:

- VC state;
- per-link receive-credit accounting;
- per-link transmit-credit accounting;
- segment receive state;
- segment transmit state;
- GDP-boundary/Size-Class tracking;
- interaction with physical-path permission;
- interaction with GC3/GS3 lower-layer state;
- incremental delivery of carried GDP words upward;
- bounded local buffering where required;
- link-local error and reset state.

It should **not** own:

- GDP routing policy;
- CSS service dispatch;
- GTS tunnels;
- application sockets;
- dynamic routing protocols.

The future `GRouterD` must be able to use `DlpEndpoint` directly.

---

# 6. GLCP / control-pair state

## 6.1 Bootstrap mode

Before the attached infrastructure is known, use an explicit bootstrap state machine.

Conceptually:

```text
Disconnected
    |
Presence
    |
Bootstrap
    |
    +--> Direct/native
    +--> GC3
    +--> GS3
```

Only implement wire behavior that is actually specified.

Do not invent a generic control protocol merely to make the code convenient.

## 6.2 GC3 runtime

After GC3 identification/bootstrap, the dedicated control pair becomes continuous state:

```text
endpoint -> GC3   WANT
GC3 -> endpoint   PERMIT
```

`WANT=1` means the endpoint requests ownership of the shared data medium.

`PERMIT=1` means the endpoint may drive the data medium.

GC3 runtime does **not** use control-pair messages for:

```text
CREDIT
CREDIT_REQUEST
GRANT
END
VC allocation
receiver flow control
```

Those older concepts are superseded.

## 6.3 GS3 runtime

Keep GS3 control-pair support abstract where the protocol is still under revision.

Possible GS3-local responsibilities include:

- presence / synchronization;
- GS identification;
- rate/capability negotiation;
- reset/recovery;
- destination/path setup where required;
- physical path/transmit permission.

Receiver credit remains GCTL data-path traffic and is **not** a GS3 control-pair operation.

Do not freeze speculative GS3 numeric encodings in code.

---

# 7. GDP

GDP is the routed Layer-3 datagram protocol.

Current core fields:

```text
Version          2 bits
Type             4 bits
Size Class       4 bits
Address Form     1 bit
Header CRC-8
Hop Limit
Source
Destination
```

GDP Type assignments currently include:

```text
0x1 GCTL
0x2 GTS
```

GDP has no:

```text
payload checksum
fragmentation
sequence number
ACK
transport window
session identity
flow ID
encryption metadata
```

## 7.1 Global form

```text
Destination 64 bits
Source      64 bits
Hop Limit    8 bits
```

## 7.2 Local form

```text
Destination ID 16 bits
Source ID      16 bits
Hop Limit       4 bits
```

Local IDs are compact representations of canonical 64-bit endpoint identities inside a known local context.

The high-level implementation should normalize received Local form to canonical effective 64-bit endpoint identities as early as practical.

## 7.3 GDP Size Classes

The frozen payload sizes are:

```text
0       0
1       3
2      32
3      64
4     128
5     192
6     256
7     384
8     512
9     768
10   1024
11   1280
12   1500
13   2048
14   4096
15   8192
```

These are exact GDP payload budgets.

GDP does not fragment in transit.

## 7.4 GDP wire implementation

Follow the useful smoltcp `Packet` / `Repr` split:

```text
GdpPacket<T>   zero-copy field access / validation
GdpRepr        checked high-level representation
```

Create strict parse/emit code under `wire/`.

Do not make endpoint `Interface` the only GDP consumer.

---

# 8. Incremental GDP decoder

Native GNet needs an incremental parser in addition to complete-packet parsing.

A future router must be able to process the header and begin forwarding before the entire GDP payload has arrived.

Suggested event model:

```text
HeaderStarted
DestinationAvailable
HeaderComplete
HeaderValidated
PayloadWord
PacketComplete
HeaderInvalid
```

Possible architecture:

```text
native flits
    |
DlpEndpoint
    |
GdpStreamDecoder
    |
    +--> endpoint packet assembler
    |
    +--> future GRouterD cut-through forwarder
```

Do not design GDP only around `&[u8]` complete packets.

---

# 9. Critical unresolved GDP/DLP issue

If the GDP header CRC fails, the GDP Size Class may also be corrupt.

Because DLP currently has no independent trustworthy packet boundary, the receiver must not simply interpret arbitrary following carried words as a fresh GDP header.

The exact resynchronization rule remains unresolved in the GNet specification.

Therefore:

- do not silently invent a packet boundary;
- do not add an SOF bit without a spec decision;
- do not invent DLP length fields;
- do not add periodic CRC windows/check blocks unless the architecture is explicitly revised;
- represent this state as unresolved/unsupported/TODO in native-link code.

A packetized simulation medium does not solve this native-link issue; it only bypasses it for high-level testing.

---

# 10. GCTL

GCTL is GDP Type `0x1` and uses the normal data channel.

The GCMP common header is:

```text
Version         1 byte
Message Type    1 byte
Code            1 byte
Flags           1 byte
Transaction ID  4 bytes
```

Important message classes include:

```text
SOLICIT
ADVERTISE
CREDIT_REQUEST
CREDIT
ADDRESS_OFFER
ADDRESS_CLAIM
ADDRESS_ACK
ADDRESS_NAK
ECHO_REQUEST
ECHO_REPLY
DESTINATION_UNREACHABLE
HOP_LIMIT_EXCEEDED
PARAMETER_PROBLEM
CLASS_UNSUPPORTED
TRANSIT_ABORTED
PATH_PROBE
PATH_REPLY
STATUS_REQUEST
STATUS_REPLY
GET_REQUEST
GET_REPLY
GET_NEXT_REQUEST
GET_NEXT_REPLY
EVENT_REPORT
```

## 10.1 Automatic GCTL handling

Routine mandatory protocol behavior should be implemented by `Interface` / `DlpEndpoint`, not forced through application sockets.

Examples:

- consume `CREDIT` and update link state;
- answer `CREDIT_REQUEST` from local receive capacity;
- answer ECHO;
- generate supported automatic errors;
- maintain status counters.

A separate GCTL request API/socket may exist for active operations such as:

- STATUS;
- PATH_PROBE;
- GET;
- GET_NEXT.

## 10.2 Credit path

The software path should be explicit:

```text
GDP parser
   |
GCTL parser
   |
CREDIT / CREDIT_REQUEST
   |
link-local credit manager
   |
DlpEndpoint TX/RX admission
```

Do not route credit state through GTS.

Do not place credit messages on the dedicated control pair.

---

# 11. CSS

CSS identifies the logical service selected by GTS CONNECT.

There is one canonical 128-bit namespace.

Wire representations:

```text
Registered-8
Short-32
Full-128
```

Internally normalize to:

```rust
pub struct ServiceSelector([u8; 16]);
```

Always emit the shortest canonical representation.

CSS is selected once during GTS CONNECT and remains bound to the tunnel.

Later STREAM_OPEN operations do not carry CSS.

---

# 12. GTS

GTS is GDP Type `0x2`.

A tunnel contains multiple streams.

Identifiers:

```text
Tunnel ID   32 bits, receiver-local
Reset ID    32 bits, receiver-local
Stream ID    8 bits, tunnel-local
Sequence    32 bits where applicable
```

CONNECT initiator owns even Stream IDs.

Responder owns odd Stream IDs.

Stream 0 is created by CONNECT.

## 12.1 GTS is message-preserving

One:

```text
DATA
DATA_END
DATAGRAM
```

represents one application message unit.

Reliable GTS preserves:

- delivery;
- order;
- message boundaries.

Do not reuse TCP byte-stream semantics internally.

An OS/library may provide a byte-stream facade above GTS by concatenating delivered messages, but that is not the GTS wire model.

## 12.2 Stream Profile

```text
bit 15      Unreliable
bit 14      Variable
bit 13      Sequenced
bit 12      Unchecked Payload
bits 11..10 Direction
bits 9..4   Reserved = 0
bits 3..0   Size Class
```

Direction:

```text
00 invalid / reserved
01 opener -> peer
10 peer -> opener
11 bidirectional
```

Reliable streams are inherently sequenced; their `Sequenced` bit is zero.

Unchecked Payload is valid only for unreliable streams.

Fixed streams use exactly one GDP Size Class.

Variable streams may choose a class up to their negotiated maximum.

## 12.3 Reliable streams

Reliable streams use:

```text
32-bit Sequence
ACK Base
32-bit selective ACK bitmap
Receive Credit
retransmission
adaptive RTO
```

Receive Credit counts message packets.

For a variable stream, one GTS receive credit guarantees capacity for one message up to the negotiated maximum Size Class.

This is **end-to-end GTS receive credit** and is completely distinct from the hop-local DLP/GCTL flit credit discussed earlier.

There are therefore two different credit systems:

```text
DLP/GCTL link credit
    scope: one physical hop
    unit: one flit

GTS reliable receive credit
    scope: one transport stream direction
    unit: one GTS message packet
```

Never merge them.

## 12.4 Unreliable streams

Unreliable streams use DATAGRAM.

They have:

```text
no ACK
no retransmission
no GTS receive credit
```

They may optionally use sequence numbers for duplicate/stale suppression and loss observation.

## 12.5 Stream reset

`STREAM_RESET` terminates one stream in both directions while leaving the tunnel and sibling streams alive.

`STREAM_RESET_ACK` acknowledges it.

Tunnel RESET is separate and destroys the entire tunnel.

---

# 13. GTS CRC

Every normal GTS packet carries CRC-32.

CRC-32-GNET parameters:

```text
poly    0x04C11DB7
init    0xFFFFFFFF
refin   false
refout  false
xorout  0xFFFFFFFF
check   "123456789" -> 0xFC891918
```

CRC code should support incremental/discontiguous input.

Normal coverage:

```text
canonical GDP pseudo-header
GTS metadata
payload
padding
```

Unchecked Payload mode still covers:

```text
canonical GDP pseudo-header
GTS metadata
```

but excludes application payload and padding.

Do not remove the CRC field in Unchecked Payload mode.

---

# 14. smolgnet source architecture

The target source structure should move toward:

```text
src/
    phy/
        native.rs
        packet.rs
        loopback.rs
        tracer.rs
        fault_injector.rs

    link/
        mod.rs
        flit.rs
        dlp.rs
        endpoint.rs
        vc.rs
        credit.rs
        glcp.rs
        gc3.rs
        gs3.rs
        segment.rs

    wire/
        mod.rs
        crc.rs
        gdp.rs
        gctl.rs
        css.rs
        gts.rs

    iface/
        mod.rs
        interface.rs
        gctl.rs
        packet.rs

    socket/
        mod.rs
        raw_gdp.rs
        gctl.rs
        gts/
            mod.rs
            tunnel.rs
            stream.rs
            reliable.rs
            datagram.rs
            ack.rs
            timer.rs

    storage/
        ring_buffer.rs
        packet_buffer.rs
        message_buffer.rs
        reliable_window.rs

    time.rs
    rand.rs
```

Exact filenames may change, but preserve the conceptual boundaries.

---

# 15. Storage and bounded-memory model

Retain smoltcp's strongest implementation principle: explicit, caller-controlled, bounded buffers.

Useful generic concepts to preserve:

- `RingBuffer`;
- packet/message metadata rings;
- separate packet-count and byte-capacity limits;
- no mandatory heap;
- heapless operation where practical.

GTS is particularly well suited to packet/message buffers because its wire semantics preserve message boundaries.

For reliable streams use explicit structures such as:

```text
ReliableTxWindow
ReliableRxWindow
```

An initial reliable receive window may be limited to 32 messages, matching the selective ACK bitmap, even though the wire credit field can represent more.

For a Variable reliable stream, receive capacity must be conservative enough that every advertised credit can accept one maximum-class message.

---

# 16. Packetized development medium

Before native DLP is complete, implement a packetized GDP test/adaptation medium.

Examples:

```text
MemoryGdpDevice
LoopbackGdpDevice
UdpGdpTunnel
```

This permits early development of:

- GDP parse/emit;
- CRC-8;
- GCTL;
- CSS;
- GTS CONNECT;
- GTS streams;
- ACK/retransmission;
- application APIs.

Do not let this temporary convenience redefine the native architecture.

The final native path remains flit-aware and control-pair-aware.

---

# 17. Recommended implementation order

## Phase 0 — establish fork point

- record the smoltcp commit/version used as the starting point;
- make sure inherited tests pass before destructive changes;
- add a short upstream-base note;
- stop treating future upstream merges as a project requirement.

## Phase 1 — rename internals

Rename crate/package/documentation from smoltcp to smolgnet.

Add GNet-oriented feature names.

Old IP features may remain temporarily only as implementation references.

## Phase 2 — core types

Implement:

```text
GdpAddress
GdpLocalId
GdpPrefix
GdpType
GdpSizeClass
Vcid
Flit
ServiceSelector
TunnelId
ResetId
StreamId
StreamProfile
```

## Phase 3 — CRC

Implement and test:

```text
CRC-8-GNET
CRC-32-GNET
```

Use golden vectors before protocol integration.

## Phase 4 — GDP wire format

Implement `GdpPacket<T>` and `GdpRepr`.

Support Global and Local forms, CRC-8, exact Size Class validation and canonical address expansion.

## Phase 5 — packetized GDP medium

Create a non-native complete-packet simulation/adaptation device.

Use it to unblock upper-layer work.

## Phase 6 — GCTL

Implement common GCMP parsing and initial mandatory messages.

Automatic link-credit/ECHO/error behavior belongs in lower/interface logic.

## Phase 7 — CSS

Implement canonical 128-bit service identity plus Registered-8, Short-32 and Full-128 encoding.

## Phase 8 — GTS wire packets

Implement every current GTS packet type and Stream Profile encoding.

No socket state machine inside `wire/`.

## Phase 9 — GTS storage

Implement bounded message buffers and reliable TX/RX windows.

## Phase 10 — GTS tunnel socket

Implement one socket per tunnel with child stream handles.

## Phase 11 — reliable GTS

Implement sequencing, selective ACK, receive credit, retransmission, timers and stale-state guards.

Keep unfrozen timing constants as implementation constants, not normative protocol claims.

## Phase 12 — native GnetLinkDevice

Implement separate data and control channels.

## Phase 13 — GLCP/GC3/GS3 lower-layer state

Implement bootstrap and GC3 WANT/PERMIT.

Keep unresolved GS3 semantics abstract.

## Phase 14 — DlpEndpoint

Implement flit/VC/segment/credit state and interactions with physical permission.

## Phase 15 — incremental GDP decoder

Make native GDP parsing suitable for endpoint assembly and future cut-through routing.

## Phase 16 — test infrastructure

Adapt/preserve:

```text
loopback
tracing
fault injection
fuzzing
```

Add GNet-specific faults:

```text
packet loss
packet corruption
flit loss
flit corruption
credit starvation
VC stalls
PERMIT delays
control-pair resets
```

## Phase 17 — remove inherited IP stack

Once the GNet stack is self-sufficient and tested, remove obsolete TCP/IP protocol code aggressively.

---

# 18. First milestone

The first useful working milestone should be:

```text
smolgnet endpoint A
        |
packetized test medium
        |
smolgnet endpoint B

GDP Global packet
GCTL ECHO_REQUEST
GCTL ECHO_REPLY
```

Required tests:

```text
GDP parse/emit
GDP CRC-8
Global 64-bit addresses
Local address expansion
Size Class validation
malformed GDP rejection
GCMP common header
ECHO request/reply
```

The next milestone should be:

```text
CSS
 -> GTS CONNECT
 -> Stream 0 Reliable Variable
 -> one application message
 -> ACK
```

Native flit/DLP/control-pair work follows after upper layers are already testable over the packetized development medium.

---

# 19. Router compatibility requirement

Routing is not part of the initial smolgnet endpoint implementation, but lower layers must be reusable by `GRouterD`.

Specifically:

- `GnetLinkDevice` must not depend on endpoint sockets;
- `DlpEndpoint` must be usable by forwarding software;
- GDP must support incremental parsing;
- complete-packet buffering must not be mandatory for the parser;
- lower layers must expose enough state for cut-through forwarding later;
- no endpoint-only assumptions should be baked into VC or credit handling.

Future GRouterD path:

```text
GnetLinkDevice
      |
DlpEndpoint
      |
GdpStreamDecoder
      |
forwarding decision
      |
DlpEndpoint
      |
GnetLinkDevice
```

---

# 20. Protocol concepts that must not be conflated

Keep these distinctions explicit in names and APIs:

```text
physical control pair        != GCTL
PERMIT/path permission       != DLP receive credit
DLP/GCTL flit credit         != GTS receive credit
VCID                         != GTS Stream ID
GDP address                  != CSS service identity
GDP Local form               != a second address identity
GDP Size Class               != IP-style fragmentation/MTU negotiation
GTS tunnel                   != GTS stream
GTS reliable message stream  != TCP byte stream
packetized test medium       != native DLP
```

Avoid generic names such as `credit` when scope is ambiguous. Prefer names like:

```text
link_flit_credit
gts_receive_credit
physical_permit
```

---

# 21. Unresolved specification areas

Do not silently invent solutions for unresolved GNet specification areas.

Current important examples include:

- DLP resynchronization after corrupt GDP header/Size Class;
- some GS3 control-pair operations and encodings;
- exact compact GCTL credit field packing/batching;
- some bootstrap/address-configuration packet encodings;
- exact GTS delayed-ACK/RTO/reset retransmission timing;
- some detailed malformed-control error mappings.

Use explicit TODOs, unsupported states, traits/abstractions, or test-only adapters until the specification is frozen.

Do not add speculative wire fields to make implementation easier.

---

# 22. Source-of-truth priority

When determining protocol behavior, use roughly this priority:

```text
newest accepted ADR
newest frozen normative protocol/packet-format document
Specification Status
current draft protocol document
current architecture document
implementation notes
historical/archived documents
old chats
inherited smoltcp behavior
```

Always check `nickik/GNet` before making or freezing a wire-format assumption.

---

# 23. Implementation principles

Preserve these smoltcp qualities:

```text
no mandatory heap
bounded memory
caller-supplied buffers
zero-copy where practical
poll-driven state machines
strict checked parsing
fuzzable wire decoders
embedded/bare-metal suitability
clear separation between wire parsing and protocol state
```

But do not preserve these inherited assumptions when they conflict with GNet:

```text
Ethernet frame as universal L2 packet
MAC addressing
IP-centric interface state
IP MTU/fragmentation
port-based transport dispatch
TCP byte-stream semantics
UDP endpoint semantics
whole-frame-only native device processing
```

The objective is a small, clean, native GNet stack with reusable lower layers, not compatibility with the Internet protocol architecture.
