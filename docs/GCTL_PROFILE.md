# GCTL credit and bootstrap profile

## Status

This document records smolgnet's implementation of the **frozen GNet 0.1 profile** from `nickik/GNet` ADR-0019, "GCTL Credit and Bootstrap Wire Profile".

It is no longer an implementation-private or provisional encoding.

The canonical GNet specification remains authoritative. This document exists to make the implementation mapping and tests easy to inspect.

## Common GCMP header

```text
Version          1 byte
Message Type     1 byte
Code             1 byte
Flags            1 byte
Transaction ID   4 bytes
```

Multi-byte body values use network byte order. Remaining bytes in the selected GDP Size Class are zero padding for these typed messages and nonzero padding is rejected.

## CREDIT_REQUEST

```text
Requested Flits   4 bytes
```

`0` means: advertise whatever receive capacity is currently grantable.

## CREDIT

```text
Granted Flits     4 bytes
```

One credit represents receive capacity for exactly one physical flit at the adjacent forwarding endpoint.

smolgnet derives this value from the endpoint's actual bounded DLP receive buffer:

```text
free = rx_buffer_flits - flits currently retained in DLP receive assembly

grantable = free - credits already advertised but not yet consumed
```

## SOLICIT

```text
Service Type      2 bytes
Scope             1 byte
Reserved          1 byte = 0
```

Scope `0` is directly attached/link scope.

## ADVERTISE

```text
Service Type      2 bytes
Preference        1 byte
Reserved          1 byte = 0
Provider Address  8 bytes
Lifetime          4 bytes
Capabilities      4 bytes
```

`Service Type = 0x0001` is Router.

## ADDRESS_OFFER

```text
Router Address    8 bytes
Prefix            8 bytes
Prefix Length     1 byte
Reserved          3 bytes = 0
Candidate Address 8 bytes
Lifetime          4 bytes
```

Allowed GNet 0.1 prefix lengths are `/16`, `/32`, `/48`, and `/56`.

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

Before a provider address is known, the frozen link-scoped bootstrap destination is:

```text
FE80:0000:0000:0000
```

Normal endpoint link-local addresses use FE80/16 with a nonzero 48-bit suffix. The bootstrap destination is not a normal endpoint address and must not be routed.

## Bootstrap sequence

```text
SOLICIT(Router)
    -> ADVERTISE(Router)
    -> ADDRESS_OFFER
    -> ADDRESS_CLAIM
    -> ADDRESS_ACK or ADDRESS_NAK
```

## Control progress

The GNet specification requires a bounded way for credit/bootstrap control traffic to make progress when ordinary data credit is exhausted.

smolgnet's direct point-to-point DLP profile currently realizes this with VC0 as a small reserved control lane/window. Ordinary data credits apply to the aggregate data lanes:

```text
VcMode::Two
    VC0 control
    VC1 data

VcMode::Four
    VC0 control
    VC1-3 data
```

This control-lane realization is a smolgnet/direct-link mechanism; the GCTL wire format itself is independent of the physical realization.
