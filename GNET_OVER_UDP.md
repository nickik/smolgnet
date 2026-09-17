# GNet over UDP underlay

This branch (`gnet-over-udp-underlay`) starts from `f670c59`, immediately
before the repository replaced its smoltcp origin with the native GNet stack.
It retains smoltcp's IP/UDP implementation as an optional *underlay*.

## Layering

`smoltcp IP/UDP -> virtual point-to-point link -> DLP -> GDP/GCTL/GTS`

The UDP endpoint is configured transport reachability only. It is not a GNet
address, route, service selector, or GTS stream identifier. Each UDP payload
contains exactly one encoded virtual-link frame with its DLP VC and traffic
class. There is no Ethernet, ARP, IP fragmentation, or IP routing in the GNet
protocol above it.

## Execution plan

- [x] Create a clean branch from the pre-native-GNet smoltcp fork point.
- [x] Add a versioned, length-delimited UDP encapsulation and malformed-input tests.
- [x] Bind it to the inherited smoltcp UDP socket API.
- [x] Interoperate with the pinned native `smolgnet` GTS implementation through
      the complete smoltcp UDP polling path: reordered reliable DATA, GTS
      reassembly, and a returned GTS ACK.
- [ ] Import/implement the GNet DLP frame endpoint above this underlay.
- [ ] Define link-loss behaviour: either DLP acknowledgement/retransmission or
      an explicitly best-effort virtual link; credit-only DLP is insufficient.
- [ ] Bind GDP, GCTL and GTS to the DLP endpoint and test two UDP peers.
- [ ] Add NAT/peer-migration policy, MTU discovery/ceiling, keepalives and
      anti-amplification/rate limits.
- [ ] Add authenticated encryption outside or alongside the UDP payload if it
      crosses untrusted networks.

## Important limitations

UDP gives datagram boundaries and a checksum, but not ordering, delivery,
peer identity, congestion control, path-MTU discovery, or confidentiality.
GTS can repair loss for reliable end-to-end streams, but DLP credit/control
traffic must itself survive loss or the virtual link can deadlock. The current
encapsulation therefore establishes the correct boundary and validates input;
it must not be advertised as a reliable WAN link until DLP's loss strategy is
defined and tested.

## Additive smoltcp restoration

`main` must not be history-rewritten. Instead, make an additive migration
branch from this pre-replacement foundation and merge it normally after the
following checkpoints:

1. Keep the full upstream smoltcp `phy`, `iface`, `wire`, `socket`, and
   `storage` APIs intact.
2. Bring the current native GNet implementation in as a separate `gnet` core
   crate/module, without name collisions with IP's `wire`, `socket`, `time`,
   `storage`, or error types.
3. Make the GNet UDP link depend on the preserved smoltcp UDP socket API.
4. Move this cross-stack GTS-over-UDP test into that merged tree and run both
   the upstream smoltcp suite and the native GNet suite unchanged.
5. Only then provide an additive `smolgnet` facade/re-export policy; retain a
   compatibility path for existing smoltcp users instead of deleting their API.
