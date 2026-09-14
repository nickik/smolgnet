# P4 GDP dataplane

This subproject explores an executable P4 implementation of the GNet Datagram Protocol (GDP), initially targeting Oxide Computer Company's `x4c` compiler so the P4 dataplane can be compiled into Rust and tested alongside smolgnet.

The initial goal is **not** to replace smolgnet. It is to create an independent GDP dataplane implementation, prove that the current GDP wire format and forwarding semantics map cleanly to P4, and establish a testable path toward later SmartNIC and router targets.

## Source of truth

Until this project reaches parity and the project explicitly decides otherwise, the canonical GDP wire implementation remains:

- `src/wire/gdp.rs`

The initial P4 implementation must match the current behavior there, including:

- 2-bit version field;
- 4-bit GDP type;
- 4-bit size class;
- global and local address forms;
- 64-bit global source and destination addresses;
- 16-bit local source and destination IDs with an external local prefix;
- 8-bit hop limit in global form;
- 4-bit hop limit in local form;
- CRC-8 validation over the GDP header fields currently covered by smolgnet;
- the current 16 size classes;
- GCTL (`0x1`) and GTS (`0x2`) GDP types.

Do not add fields merely because they appeared in older GNet design discussions. In particular, the P4 project should track the actual current GDP wire format in smolgnet.

## Initial scope

The first milestone is a pure GDP dataplane. It should implement enough behavior to answer three questions:

1. Can GDP be parsed and emitted cleanly in P4?
2. Can `x4c` generate useful Rust for the GDP dataplane?
3. Can the generated implementation be tested for behavioral equivalence with smolgnet's hand-written Rust implementation?

The initial dataplane should support:

- parsing valid global and local GDP headers;
- rejecting malformed or truncated headers;
- decoding GDP type and size class;
- checking the address-form bit;
- exposing effective source and destination information to the pipeline;
- validating or reproducing the GDP header CRC-8 where practical in the target;
- decrementing hop limit for transit forwarding;
- dropping packets whose hop limit has expired;
- exact-match and prefix-style forwarding experiments for 64-bit GDP destinations;
- selecting local delivery versus forwarding versus drop;
- preserving the payload unchanged;
- deparsing the resulting GDP packet.

## Explicitly out of scope initially

The first implementation does **not** need to implement:

- GTS connection state;
- retransmission;
- timers;
- GCTL state machines;
- routing-protocol logic;
- dynamic route discovery;
- GC3 or GS3;
- link-layer credit handling;
- SmartNIC-specific DMA or host queues;
- vendor-specific P4 externs;
- a production router daemon.

Those belong either in smolgnet, a control plane such as GRouterD, or later target-specific integration.

## Why x4c first

`x4c` gives this project a useful first target because it allows the P4 program to become executable Rust. That gives us a way to test the P4 semantics without immediately depending on FPGA hardware or a particular SmartNIC SDK.

The desired development model is:

```text
GDP specification / smolgnet wire behavior
                |
                v
             gdp.p4
                |
                v
              x4c
                |
                v
        generated Rust pipeline
                |
                v
      differential/conformance tests
                |
        +-------+-------+
        |               |
        v               v
   smolgnet Rust    generated Rust
```

The generated Rust is a dataplane implementation, not a replacement for the full smolgnet stack.

## Relationship to smolgnet

For now, keep the two implementations separate.

smolgnet remains responsible for the host-side protocol stack and higher-level behavior, including GTS. The P4 project focuses on packet-processing semantics that can later run in software, a SmartNIC, or a switch/router ASIC.

Once the P4 implementation is stable and well tested, we can evaluate whether smolgnet should:

- continue using its current GDP implementation unchanged;
- share generated/parser code with the P4 implementation;
- use an x4c-generated GDP dataplane behind an optional feature;
- or use P4 only for router/NIC offload while retaining the hand-written host stack.

No decision is made by this subproject yet.

## Planned layout

```text
p4-gdp/
├── README.md
├── TODO.md
├── TESTING.md
└── p4/
    └── gdp.p4        # added once the x4c build/test harness is established
```

## Design rule

Keep `gdp.p4` as portable P4 as long as possible. Target-specific code should live outside the core dataplane or behind clearly isolated adapters. The same logical GDP pipeline should eventually be usable by:

- x4c-generated Rust;
- software simulation/test targets;
- a SmartNIC P4 compiler;
- a hardware switch/router target.
