#!/bin/sh
set -eu

A=${1:-gnettap0}
B=${2:-gnettap1}
BR=${3:-gnetbr0}
OWNER=${SUDO_USER:-${USER:-$(id -un)}}

cleanup() {
    sudo ip link del "$BR" 2>/dev/null || true
    sudo ip tuntap del dev "$A" mode tap 2>/dev/null || true
    sudo ip tuntap del dev "$B" mode tap 2>/dev/null || true
}

if [ "${4:-}" = "clean" ]; then
    cleanup
    exit 0
fi

cleanup
sudo ip tuntap add dev "$A" mode tap user "$OWNER"
sudo ip tuntap add dev "$B" mode tap user "$OWNER"
sudo ip link add "$BR" type bridge
sudo ip link set "$A" master "$BR"
sudo ip link set "$B" master "$BR"
sudo ip link set "$A" up
sudo ip link set "$B" up
sudo ip link set "$BR" up

cat <<EOF
Created TAP pair:
  $A <-> $BR <-> $B

Run:
  SMOLGNET_TAP_A=$A SMOLGNET_TAP_B=$B cargo test --release --test tap_performance -- --ignored --nocapture

Cleanup:
  $0 $A $B $BR clean
EOF
