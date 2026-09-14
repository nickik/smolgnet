#!/usr/bin/env bash
set -euo pipefail

run_case() {
    local gdp="$1"
    local gts="$2"
    echo
    echo "================================================================"
    echo "Testing GDP_BACKEND=$gdp GTS_BACKEND=$gts"
    echo "================================================================"
    GDP_BACKEND="$gdp" GTS_BACKEND="$gts" \
        bash scripts/backend-cargo.sh test --all-targets
}

# Complete production backend matrix. Every higher-level test is run in every
# supported GDP/GTS combination so neither layer can accidentally depend on the
# implementation selected for the other layer.
run_case handwritten handwritten
run_case p4          handwritten
run_case handwritten p4
run_case p4          p4

# Differential modes are validation backends rather than production choices.
# Cross them with the opposite layer as well, including both compare modes at
# once, to guarantee the oracle machinery is composable with every P4 choice.
run_case compare     handwritten
run_case compare     p4
run_case handwritten compare
run_case p4          compare
run_case compare     compare
