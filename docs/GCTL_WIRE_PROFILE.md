# GCTL credit and bootstrap wire profile

## Status

This document summarizes the **frozen GNet 0.1 wire profile** implemented by smolgnet for:

```text
CREDIT_REQUEST
CREDIT
SOLICIT
ADVERTISE
ADDRESS_OFFER
ADDRESS_CLAIM
ADDRESS_ACK
ADDRESS_NAK
```

The normative source is `nickik/GNet`, especially **ADR-0019: GCTL Credit and Bootstrap Wire Profile**. If this document ever disagrees with the canonical GNet repository, the canonical specification wins.

## Common GCMP header

All messages use the existing 8-byte GCTL common header:

```text
Version          1 byte
Message Type     1 byte
Code             1 byte
Flags            1 byte
Transaction ID   4 bytes
```

Multi-byte body values use network byte order. Unused bytes in the selected GDP Size Class are zero padding.

## CREDIT_REQUEST

```text
Requested Flits   4 bytes
```

A value of zero means "advertise whatever receive capacity is currently grantable".

## CREDIT

```text
Granted Flits     4 bytes
```

One link credit guarantees receive capacity for exactly one physical flit at the adjacent forwarding endpoint.

The receiver derives grantable credit from real bounded storage:

```text
grantable = free receive-flit capacity
            - already advertised but unconsumed credit
```

Link credit is aggregate across the ordinary data lanes. It is distinct from both GTS receive credit and physical-medium permission.

## SOLICIT

```text
Service Type      2 bytes
Scope             1 byte
Reserved          1 byte = 0
```

Scope `0` is directly attached/link scope. Service Type `0x0001` is Router.

## ADVERTISE

```text
Service Type      2 bytes
Preference        1 byte
Reserved          1 byte = 0
Provider Address  8 bytes
Lifetime          4 bytes
Capabilities      4 bytes
```

`ADVERTISE` echoes the requested service type and transaction.

## ADDRESS_OFFER

```text
Router Address    8 bytes
Prefix            8 bytes
Prefix Length     1 byte
Reserved          3 bytes = 0
Candidate Address 8 bytes
Lifetime          4 bytes
```

Prefix Length is one of `/16`, `/32`, `/48`, or `/56`. Prefix must be normalized and Candidate Address must lie inside it.

## ADDRESS_CLAIM

```text
Candidate Address 8 bytes
Claim Nonce       8 bytes
```

## ADDRESS_ACK

```text
Confirmed Address 8 bytes
Lifetime          4 bytes
```

## ADDRESS_NAK

```text
Candidate Address 8 bytes
Retry Delay       4 bytes
```

The common GCMP `Code` field carries the rejection reason.

## Bootstrap destination

Before a router/provider address is known, link-scoped discovery uses the frozen bootstrap GDP destination:

```text
FE80:0000:0000:0000
```

This value is link-scoped and must not be routed. Normal generated GNet 0.1 client link-local addresses use a nonzero 48-bit suffix under `FE80/16`, so they do not collide with this bootstrap destination.

The bootstrap exchange is:

```text
SOLICIT(Router)
    -> ADVERTISE(Router)
    -> ADDRESS_OFFER
    -> ADDRESS_CLAIM
    -> ADDRESS_ACK / ADDRESS_NAK
```

## smolgnet receive-buffer model

Each `EndpointConfig` supplies `rx_buffer_flits`.

For ordinary data traffic:

```text
free = rx_buffer_flits - flits currently retained in DLP receive assembly

grantable = free - credits already advertised but not yet consumed
```

A received data flit:

1. requires one previously advertised peer credit;
2. consumes that credit;
3. consumes one unit of receive storage;
4. remains in DLP receive assembly until its GDP packet is complete;
5. releases receive storage when the complete packet is handed upward;
6. causes newly grantable capacity to be advertised through GCTL CREDIT according to the endpoint update threshold.

## Control progress

GNet requires bounded control progress even when ordinary data credit is exhausted.

The current direct smolgnet profile implements this with VC 0 as a small statically provisioned control lane:

```text
VcMode::Two
    VC0   control
    VC1   data

VcMode::Four
    VC0   control
    VC1-3 data
```

Ordinary advertised link credit applies to the aggregate data lanes, not independently per numeric VCID. GDP, normal GCTL functions, and GTS do not choose numeric data VCIDs; DLP assigns them internally.

This particular VC scheduling mechanism is a smolgnet implementation choice. The GCTL message bodies, bootstrap destination, credit unit, and real-buffer-derived credit semantics above are frozen GNet 0.1 behavior.
