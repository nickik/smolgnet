# P4 GDP testing strategy

The first success criterion for the P4 GDP project is behavioral equivalence with smolgnet's current GDP implementation, not raw throughput.

## Test levels

### 1. P4 parser tests

Feed raw GDP byte strings into the x4c-generated Rust pipeline and verify parsed fields and acceptance/drop behavior.

Required cases:

- valid global header;
- valid local header;
- every current size class;
- GCTL and GTS packet types;
- reserved packet types;
- hop-limit boundary values;
- truncated 4-byte base word;
- truncated global address section;
- truncated local address section;
- corrupt CRC.

### 2. Differential tests

For each packet vector:

1. construct or decode it with `src/wire/gdp.rs`;
2. feed the same bytes to the x4c-generated P4 pipeline;
3. compare the interpreted GDP fields;
4. where no forwarding mutation is requested, require byte-for-byte output equality.

This gives us two independent implementations of the GDP format.

### 3. Forwarding semantics

Use virtual input/output ports and a small route table.

Verify:

- local destination -> local delivery;
- known remote destination -> selected output port;
- unknown destination -> defined drop behavior;
- transit packet -> hop limit decremented once;
- expired hop limit -> drop;
- payload -> unchanged;
- unrelated header fields -> unchanged.

### 4. Randomized conformance

Generate legal GDP headers using smolgnet, encode them to bytes, execute them through the P4/x4c pipeline, and compare results.

Important dimensions:

- address form;
- arbitrary 64-bit global addresses;
- arbitrary local IDs and prefixes;
- all size classes;
- all four-bit packet-type values;
- hop-limit values.

Malformed inputs should also be generated deliberately, but correctness assertions must distinguish between malformed packets that the specification requires to be rejected and inputs for which behavior is currently unspecified.

### 5. Router pipeline tests

Once forwarding exists, model a fixed topology with virtual ports:

```text
host A -- port 0 [ P4 GDP router ] port 1 -- host B
                         |
                       port 2
                         |
                       host C
```

Inject GDP packets at each port and verify route selection, hop-limit handling, local delivery and counters.

This is intentionally a packet-forwarding test, not a complete network simulator.

## Performance testing

Performance comes only after the conformance suite passes.

Measure separately:

- x4c-generated Rust parser throughput;
- complete x4c GDP forwarding pipeline throughput;
- smolgnet GDP encode/decode throughput;
- later, Linux packet-I/O plus x4c forwarding throughput;
- later, SmartNIC/hardware throughput.

Do not interpret generated-Rust performance as a proxy for FPGA/ASIC performance. The value of x4c initially is executable semantics and testing.

## Future smolgnet comparison

The eventual integration decision should be based on evidence from these tests. A generated implementation is not automatically preferable to the hand-written Rust implementation. The project should preserve smolgnet's small, understandable endpoint stack unless P4-derived code offers a concrete advantage.
