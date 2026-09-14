# Hosted TAP testing

`smolgnet` has an optional Linux TAP adapter for testing the QDX-GNET host-facing frame path through real kernel network devices.

This is not a GNet media specification. Linux TAP requires Ethernet-shaped frames, so the host adapter wraps each `GnetFrame` in a private test-only Ethernet envelope using EtherType `0x88B5` and a four-byte shim carrying the QDX-local VCID/control-data metadata.

```text
GnetFrame
   |
TapDevice
   |
Ethernet test envelope
   |
Linux TAP A ---- Linux bridge ---- Linux TAP B
   |                                  |
TapDevice                          TapDevice
   |                                  |
GnetFrame                          GnetFrame
```

GDP, GCTL and GTS never see the Ethernet header.

## Run the real TAP test

On Linux with `iproute2` and sudo access:

```bash
bash scripts/test-tap.sh
```

The script:

1. creates `smolgnet-br0`;
2. creates `smolgnet-tap0` and `smolgnet-tap1` owned by the current user;
3. assigns deterministic locally administered MAC addresses;
4. attaches both TAPs to the bridge;
5. runs the ignored `tests/host_tap.rs` integration test;
6. removes the TAPs and bridge on exit.

The Rust test sends a data QDX-GNET frame A -> B and a control QDX-GNET frame B -> A and requires byte-for-byte and metadata equality after traversing the Linux TAP/bridge path.

Names can be overridden:

```bash
SMOLGNET_TAP_BRIDGE=mybr \
SMOLGNET_TAP_A=mytap0 \
SMOLGNET_TAP_B=mytap1 \
bash scripts/test-tap.sh
```

## Unit tests versus kernel TAP test

Normal `host-tap` tests validate the envelope codec without privileges:

```bash
cargo test --all-targets --features host-tap
```

The real kernel test is marked ignored because it requires TAP creation privileges. `scripts/test-tap.sh` supplies the topology and invokes it with `--ignored`.

CI attempts both levels: the ordinary codec/build tests and the real Linux TAP path on the Ubuntu runner.

## Filtering unrelated host traffic

A real bridge may carry unrelated Ethernet frames. `TapDevice::receive_frame()` discards frames that are not addressed to the configured test MAC/broadcast address or do not use the private GNet test EtherType. A malformed frame that does claim to be a GNet TAP frame still returns a GNet parse error.

## Routing use

The initial TAP test is deliberately only a point-to-point host-device test. Routing work should extend this into:

```text
endpoint A TAP <-> router ingress
                         |
                    GDP route lookup
                         |
endpoint B TAP <-> router egress
```

See `docs/ROUTING_TODO.md`.
