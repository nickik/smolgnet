#!/usr/bin/env bash
set -euo pipefail

# Select the wire backend for each protocol layer independently.
#
# Production selections:
#   GDP_BACKEND=handwritten|p4
#   GTS_BACKEND=handwritten|p4
#
# Validation additionally accepts `compare`, which selects P4 as the active
# backend while running the handwritten implementation as a differential
# oracle. The two axes are intentionally independent.
GDP_BACKEND="${GDP_BACKEND:-handwritten}"
GTS_BACKEND="${GTS_BACKEND:-handwritten}"

features=()

case "$GDP_BACKEND" in
    handwritten) ;;
    p4) features+=("p4-gdp") ;;
    compare) features+=("p4-gdp-compare") ;;
    *)
        echo "invalid GDP_BACKEND '$GDP_BACKEND' (expected handwritten, p4, or compare)" >&2
        exit 2
        ;;
esac

case "$GTS_BACKEND" in
    handwritten) ;;
    p4) features+=("p4-gts") ;;
    compare) features+=("p4-gts-compare") ;;
    *)
        echo "invalid GTS_BACKEND '$GTS_BACKEND' (expected handwritten, p4, or compare)" >&2
        exit 2
        ;;
esac

feature_csv=""
if ((${#features[@]})); then
    feature_csv="$(IFS=,; echo "${features[*]}")"
fi

echo "smolgnet backends: GDP=$GDP_BACKEND GTS=$GTS_BACKEND${feature_csv:+ features=$feature_csv}" >&2

# Cargo options must appear before the test-binary `--` separator. Split the
# caller's arguments so this wrapper also works for release benchmarks such as:
#   backend-cargo.sh test --release --test performance -- --ignored --nocapture
before=()
after=()
seen_separator=0
for arg in "$@"; do
    if ((seen_separator)); then
        after+=("$arg")
    elif [[ "$arg" == "--" ]]; then
        seen_separator=1
    else
        before+=("$arg")
    fi
done

if ((${#before[@]} == 0)); then
    before=("build")
fi

cmd=(cargo "${before[@]}")
if [[ -n "$feature_csv" ]]; then
    cmd+=(--features "$feature_csv")
fi
if ((seen_separator)); then
    cmd+=(-- "${after[@]}")
fi

exec "${cmd[@]}"
