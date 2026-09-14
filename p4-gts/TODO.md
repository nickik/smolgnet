# P4 GTS TODO

GTS is now split into one common semantic/transport implementation plus two interchangeable lower wire decoders.

```text
                    negotiated GTS state
                 streams / profiles / timers
                           |
                    common GTS semantics
                  CRC / payload / validation
                           |
              +------------+------------+
              |                         |
     handwritten lower             P4/x4c lower
       wire decoder                 wire decoder
              |                         |
              +------------+------------+
                           |
                        GTS bytes
```

The important rule is that negotiated profile state is owned once by the higher GTS layer. The P4 backend must not become a second independent stream-state database merely to parse packets.

## Complete

- [x] split handwritten wire parsing out of the common GTS semantic layer
- [x] define a common profile-independent lower representation
- [x] handwritten lower decoder produces the common representation
- [x] P4/x4c lower decoder produces the same representation
- [x] common Rust layer performs StreamProfile validation once
- [x] common Rust layer performs CSS decoding once
- [x] common Rust layer interprets DATA/DATA_END payload length once
- [x] common Rust layer interprets DATAGRAM sequence/length once
- [x] common Rust layer performs GTS CRC32 validation once
- [x] common Rust layer constructs GtsPacket once
- [x] P4 probes two DATA option octets regardless of fixed/variable profile
- [x] P4 probes six DATAGRAM option octets regardless of profile
- [x] fixed DATA treats the two probe octets as payload
- [x] DATAGRAM interprets 0/2/4/6 probe octets according to StreamProfile
- [x] `p4-gts` selects the P4 lower decoder
- [x] `p4-gts-compare` parses every packet with both lower decoders before common semantics
- [x] all control forms covered by conformance tests
- [x] reliable fixed/variable DATA and DATA_END covered
- [x] all four DATAGRAM sequenced/variable combinations covered
- [x] checked and unchecked DATAGRAM CRC behavior covered
- [x] malformed/truncated and CRC errors compared
- [x] P4 GDP + P4 GTS combination exercised in CI

## Why no P4 stream-profile table

A normal P4 pipeline parses before ingress match-action tables execute. A `(tunnel, stream) -> profile` table therefore cannot directly decide how many bytes the same parser pass should extract without target-specific recirculation or parser metadata supplied externally.

More importantly, smolgnet already has the negotiated StreamProfile in the higher GTS stream state. Duplicating that state in the P4 pipeline would add synchronization work without reducing protocol complexity.

The current option-probe representation avoids that duplication:

```text
DATA
  fixed prefix + 2-byte probe

DATAGRAM
  fixed prefix + 6-byte probe
```

The common semantic layer decides how many probe bytes are metadata and how many remain payload. This representation is suitable both for handwritten Rust and for a future hardware P4 parser.

A hardware target may later add a profile table for early rejection or queue steering, but it is an optimization, not part of canonical GTS parsing.

## Next - strengthen lower-backend equivalence

- [ ] randomized valid packet generation for every GTS type
- [ ] randomized malformed reserved fields
- [ ] randomized truncation at every byte boundary
- [ ] boundary tests for every GDP size class usable by GTS
- [ ] DATA valid length 0 / maximum / one-too-large
- [ ] DATAGRAM valid length 0 / maximum / one-too-large
- [ ] sequence values around wrapping boundaries
- [ ] compare lower representation directly for all generated packets
- [ ] compare final common GtsPacket result for all generated packets

The handwritten lower decoder remains the independent oracle while P4 matures.

## Transmit path

Transmit is still handwritten. Do not mix receive-parser work with deparser work until receive equivalence is stable.

- [ ] define a common lower transmit representation
- [ ] move handwritten fixed-header emission behind that representation
- [ ] determine whether x4c-generated Header/deparser support can emit canonical GTS headers cleanly
- [ ] P4/deparser output byte-for-byte matches handwritten output
- [ ] compare errors for invalid profiles and impossible size classes
- [ ] benchmark encoding separately from decoding

Only then consider a `p4-gts` transmit backend.

## CRC offload

CRC currently belongs to the common semantic layer so both lower parsers share exactly the same coverage rules.

- [ ] define target-independent GTS CRC coverage vectors
- [ ] test every packet type against `compute_crc`
- [ ] evaluate a P4 checksum/hash extern for hardware targets
- [ ] retain the common Rust implementation as the reference

Do not duplicate CRC policy separately in each parser.

## Fast-path P4 opportunities after parsing is stable

These can use a programmed profile/stream table without making it authoritative:

- [ ] reject unknown receive tunnel/stream
- [ ] reject DATA for a stream known to be unreliable
- [ ] reject DATAGRAM for a stream known to be reliable
- [ ] validate fixed or maximum size class early
- [ ] classify control vs reliable data vs unreliable datagram
- [ ] steer `(tunnel, stream)` to CPU/queue IDs
- [ ] packet/byte counters per stream where hardware permits

Rust remains the state owner and programs any such table as a cache/fast-path view.

## Keep in higher GTS / transport engine

- tunnel negotiation
- stream negotiation
- StreamProfile ownership
- retransmission timers
- retransmission queues
- receive reordering/window state
- ACK policy
- receive-credit policy
- application queues
- close/reset state machines
- buffer ownership

## Performance matrix

CI must measure both the lower parser and complete endpoint throughput.

Lower decode:

```text
handwritten GTS lower
P4/x4c GTS lower
```

Complete endpoint combinations:

```text
handwritten GDP + handwritten GTS
P4 GDP          + handwritten GTS
handwritten GDP + P4 GTS
P4 GDP          + P4 GTS
```

Track:

- ns per GTS decode
- Mpps for representative GTS packet shapes
- 1 MiB reliable-stream throughput
- end-to-end Gbit/s
- allocations/copies where practical
- binary size later if the runtime difference becomes material

The goal is not simply for P4 to win a parser microbenchmark. The architectural goal is one GTS semantic implementation, exact interchangeable lower decoders, and a P4 representation that can move into a NIC/router dataplane later.
