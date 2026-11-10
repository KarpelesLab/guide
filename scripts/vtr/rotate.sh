#!/bin/bash
# Rotates LUT inputs for timing after routing, on placements of an earlier run, and has VPR
# check and time the result.
#
#     scripts/vtr/rotate.sh DIR WORK [CIRCUIT...]
#
# DIR is VPR's, as for scripts/vtr/place.sh. WORK is the WORK of a run of place.sh, or of
# scripts/vtr/loop.sh, whose placements are measured in WORK/CIRCUIT/eval; the circuits are
# all those it ran by default. PLACEMENTS says which placements to rotate on, "vpr guide" by
# default. Circuits run in parallel, $JOBS at a time (default 20). See
# scripts/vtr/rotate-circuit.sh for what is run on each, and scripts/vtr/rotate_tables.py for
# the tables in docs/benchmarks.md.

set -u

if [ $# -lt 2 ]; then
    echo "usage: $0 DIR WORK [CIRCUIT...]" >&2
    exit 2
fi
DIR=$(realpath "$1")
WORK=$(realpath "$2")
shift 2
CIRCUITS=("$@")
if [ ${#CIRCUITS[@]} -eq 0 ]; then
    for result in "$WORK"/*/result.txt; do
        CIRCUITS+=("$(basename "$(dirname "$result")")")
    done
fi
HERE=$(dirname "$(realpath "$0")")
ROOT=$(dirname "$(dirname "$HERE")")
VPR=$DIR/build/vpr/vpr
cargo build --release --features fpga --example vtr_route --manifest-path "$ROOT/Cargo.toml" ||
    exit 1
cp "$ROOT/target/release/examples/vtr_route" "$WORK/vtr_route_rotate" || exit 1

for circuit in "${CIRCUITS[@]}"; do
    run=$WORK/$circuit
    [ -f "$run/eval/result.txt" ] && run=$run/eval
    for placement in ${PLACEMENTS:-vpr guide}; do
        echo "$run $placement"
    done
done | xargs -P "${JOBS:-20}" -L1 "$HERE/rotate-circuit.sh" "$VPR" "$WORK/vtr_route_rotate"
