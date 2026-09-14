#!/usr/bin/env bash
set -euo pipefail

BRIDGE="${SMOLGNET_TAP_BRIDGE:-smolgnet-br0}"
TAP_A="${SMOLGNET_TAP_A:-smolgnet-tap0}"
TAP_B="${SMOLGNET_TAP_B:-smolgnet-tap1}"
OWNER="${SUDO_USER:-${USER}}"

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "This test requires Linux TAP support." >&2
  exit 1
fi

command -v ip >/dev/null 2>&1 || {
  echo "The 'ip' command from iproute2 is required." >&2
  exit 1
}

cleanup() {
  sudo ip link del "$TAP_A" 2>/dev/null || true
  sudo ip link del "$TAP_B" 2>/dev/null || true
  sudo ip link del "$BRIDGE" 2>/dev/null || true
}
trap cleanup EXIT

cleanup

sudo ip link add name "$BRIDGE" type bridge
sudo ip tuntap add dev "$TAP_A" mode tap user "$OWNER"
sudo ip tuntap add dev "$TAP_B" mode tap user "$OWNER"

sudo ip link set dev "$TAP_A" address 02:00:00:00:00:01
sudo ip link set dev "$TAP_B" address 02:00:00:00:00:02
sudo ip link set dev "$TAP_A" master "$BRIDGE"
sudo ip link set dev "$TAP_B" master "$BRIDGE"
sudo ip link set dev "$BRIDGE" up
sudo ip link set dev "$TAP_A" up
sudo ip link set dev "$TAP_B" up

# Keep the test bridge protocol-neutral. The Ethernet envelope exists only to
# let Linux carry QDX-GNET test frames between two TAP file descriptors.
sudo ip addr flush dev "$BRIDGE" || true
sudo ip addr flush dev "$TAP_A" || true
sudo ip addr flush dev "$TAP_B" || true

SMOLGNET_TAP_A="$TAP_A" \
SMOLGNET_TAP_B="$TAP_B" \
cargo test --features host-tap --test host_tap -- --ignored --nocapture
