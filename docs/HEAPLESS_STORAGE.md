# Heapless Storage and SocketSet

## Purpose

smolgnet supports two memory models:

1. a caller-supplied, fixed-capacity core for bare-metal and tightly bounded systems; and
2. an alloc-backed convenience frontend for hosted development and ordinary applications.

The protocol semantics are the same. The difference is who owns storage and whether capacity can grow at runtime.

## Cargo features

The default build enables:

```text
std + alloc
```

A true no-allocation core is selected with:

```text
--no-default-features
```

In that configuration the crate does not import `alloc`. The no-allocation CI test exercises caller-owned storage, SocketSet, reliable GTS send/receive, ACK processing, retransmission and stream reset.

`std` implies `alloc`; `alloc` does not imply `std`.

## Storage primitives

### RingBuffer

`RingBuffer<'a, T>` is a fixed FIFO over a caller-provided `&mut [T]`.

It never allocates. Its maximum element count is exactly the length of the supplied slice.

### PacketBuffer / MessageBuffer

`PacketBuffer<'a, M>` uses two caller-owned regions:

```text
metadata slots     &mut [PacketMetadata<M>]
payload arena      &mut [u8]
```

It stores variable-length packets/messages while retaining packet boundaries. A packet is always kept contiguous in the byte arena.

`MessageBuffer` is the same primitive under a transport-oriented name.

### MessagePool

`MessagePool<'a, M>` is random-access bounded message storage. The caller supplies:

```text
message slots      &mut [MessageSlot<M>]
byte arena         &mut [u8]
```

The arena is divided equally among message slots. Therefore one free slot guarantees enough storage for one complete message up to the configured slot size.

This maps directly onto GTS credit semantics: advertising one receive credit means the receiver has one complete message slot available.

## SocketSet

`SocketSet<'a, T>` is backed by caller-provided `SocketStorage<T>` slots.

```text
SocketSet
  slot 0
  slot 1
  slot 2
  ...
```

Adding a socket returns a `SocketHandle` containing:

```text
slot index
generation
```

When a slot is removed and reused its generation changes. A stale handle therefore cannot accidentally address a new socket that later occupies the same slot.

The set supports:

```text
add
get
get_mut
remove
iter
iter_mut
find
```

Exhausting the supplied slots returns `BufferFull`; the set cannot grow.

Hosted/default builds also provide `OwnedSocketSet<T>`, which implements the same handle model using an alloc-backed slot vector. The hosted `Endpoint` stores GTS tunnel objects in `OwnedSocketSet` rather than directly in a map, so both hosted and bounded configurations use the same socket-management concept.

## Bounded GTS socket

`BoundedGtsSocket<'a>` is the no-allocation GTS tunnel implementation.

A caller supplies all mutable storage when creating the tunnel:

```text
stream slots
TX metadata slots
TX retransmission byte arena
RX metadata slots
RX message byte arena
```

It implements the current point-to-point GTS transport behavior needed by a bounded system:

```text
tunnel establishment
stream allocation and profiles
fixed/variable size validation
reliable send
receiver credit
ACK base + selective ACK bitmap
out-of-order receive window
retransmission timer
unreliable sequenced/unsequenced messages
stream reset
tunnel reset
```

Reliable TX messages remain in the caller-owned TX pool until acknowledged. Reliable RX messages occupy caller-owned RX slots until the application removes them. GTS receive credit is calculated from actual free receive slots rather than from an abstract configured number alone.

## Example memory layout

A small system can statically allocate all transport memory:

```rust
let mut socket_slots = [SocketStorage::EMPTY; 4];
let mut stream_slots = [BoundedStreamSlot::EMPTY; 8];
let mut tx_slots = [MessageSlot::<TxMeta>::EMPTY; 32];
let mut tx_bytes = [0u8; 32 * 512];
let mut rx_slots = [MessageSlot::<RxMeta>::EMPTY; 32];
let mut rx_bytes = [0u8; 32 * 512];
```

There is no hidden heap growth: the number of sockets, streams, outstanding retransmissions, queued received messages and maximum message storage are all explicit in the program's static memory budget.

## Current boundary

The no-allocation work in this revision covers the reusable storage layer, socket set, core GDP/CSS/GTS wire types and bounded GTS transport state.

The existing full `Endpoint`, DLP/QDX convenience implementation and typed GCTL message container remain behind the `alloc` feature. They continue to be appropriate for hosted testing and the existing point-to-point stack.

A later bare-metal endpoint integration can build the DLP/QDX/GCTL side directly on these caller-supplied primitives without changing the bounded GTS or SocketSet APIs. This revision deliberately does not claim that the entire physical-link endpoint path is already heapless.
