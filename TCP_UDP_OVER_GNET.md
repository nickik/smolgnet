# TCP/UDP compatibility over GNet

## Status

This branch is the staging area for an **optional IP compatibility overlay**.
It must not change native GNet transport semantics or turn GDP into IP.

The source reference is the smoltcp tree immediately before commit
`8cb4c20` removed the inherited IP stack.  The retained source is 0BSD, as
is this repository.

## What this feature means

Applications that require conventional TCP or UDP use a private IPv4/IPv6
network over an already-established GNet service.  `smoltcp` continues to
own:

- IP addressing, routing, fragmentation policy and IP checksums;
- TCP and UDP packet parsing and checksums;
- TCP socket state, byte stream buffers, retransmission and congestion
  control; and
- the familiar `Interface`/`SocketSet` API.

GNet carries each complete IP packet as the application payload of one
**unreliable** GTS message. The overlay service has a registered CSS and
creates one GTS tunnel per peer. Stream 0 is an unreliable, variable,
bidirectional, fully-checked GTS stream. It carries both TCP and UDP IP
packets.

TCP must observe packet loss and reordering so its own retransmission and
congestion-control state remains meaningful. UDP retains its normal
unreliable semantics. The GTS stream is intentionally unsequenced: TCP has
its own sequence space, while UDP must be delivered in arrival order.

The overlay must reject an IP packet which cannot fit in its negotiated GTS
maximum Size Class.  It does **not** add GNet-layer fragmentation.  Normal
IP fragmentation may be supported by smoltcp only when the overlay MTU is
advertised correctly; initial acceptance should instead use a conservative
MTU and reject oversized packets.

The first implementation should be an adapter from a pair of GTS streams to
smoltcp's packet device boundary.  It should import the upstream smoltcp TCP,
UDP, IP, interface, storage and wire modules as one coherent subsystem, not
copy `socket/tcp.rs` or `socket/udp.rs` alone.

## Explicit non-goals

- Native GTS is not reimplemented with TCP.
- GDP headers do not gain IP protocol numbers or TCP/UDP ports.
- A GTS tunnel is not a TCP connection and a GTS stream is not a TCP socket.
- No native GNet service is required to understand IP, TCP or UDP.
- The adapter does not serialize a native DLP link into an Ethernet-like
  byte stream.

## Execution plan

1. [x] Define the branch-local overlay profile: one unreliable variable GTS
   stream, unsequenced and with full payload CRC.
2. [x] Add a small `IpCompatDatagram` boundary which validates IP packet shape,
   negotiated Size Class, and uses only GTS `DATAGRAM` packets.
3. [ ] Freeze the CSS and profile in `GNet`.
4. [x] Add the unmodified upstream `smoltcp 0.14.0` TCP/UDP/IP implementation
   as an optional `ip-compat` dependency. This is the source compatibility
   baseline; it is not copied into native GNet modules.
5. [x] Implement a poll-driven `GtsIpDevice` between the adapter and
   smoltcp's `Medium::Ip` packet-device boundary.
6. [ ] Prove UDP delivery, loss, duplication and reordering.
7. [ ] Prove TCP handshake, byte-stream transfer, retransmission, congestion
   response and close with injected GTS DATAGRAM loss.
8. [ ] Add a no-`std` bounded-buffer configuration and a compatibility-only
   fuzz/fault suite.

## Known costs and hazards

TCP on *reliable* GTS would duplicate reliability, ACK traffic,
retransmission and congestion response. A lost GTS message could be repaired
before TCP ever observes a loss, preventing meaningful TCP congestion
control and adding a second head-of-line boundary. This branch intentionally
uses unreliable GTS DATAGRAM instead. Native services should still use GTS
directly.

The GTS DATAGRAM profile has no universal port namespace. Conventional TCP
and UDP ports live only inside the opaque IP payload. The CSS, service
advertisement and policy for this compatibility service must be allocated
rather than invented locally.

The imported smoltcp subsystem is substantially larger than TCP and UDP:
its TCP/UDP wire checksums depend on IP pseudo-headers, sockets depend on the
interface context, and the interface depends on IP routing, packet parsing,
storage and a whole-packet device.  Importing only the two socket files would
produce an unmaintainable partial fork.

End-to-end checksums intentionally stack: GDP protects routing metadata, GTS
protects the encapsulated IP packet as an opaque payload, and TCP/UDP protect
their own IP pseudo-header and payload.  This is correct for the overlay but
costly on small machines.
