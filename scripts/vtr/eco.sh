#!/bin/bash
# Measures engineering changes on circuits that scripts/vtr/place.sh has run, keeping their
# graphs: small synthetic changes, handled from scratch and incrementally, by
# examples/vtr_eco.rs, which says what the changes are.
#
#     KEEP_GRAPHS=1 scripts/vtr/place.sh DIR WORK [CIRCUIT...]   # once, for the graphs
#     scripts/vtr/eco.sh WORK OUT [CIRCUIT...]                   # all circuits of WORK by default
#     python3 scripts/vtr/eco_tables.py OUT                      # the tables in docs/benchmarks.md
#
# Each circuit is measured on VPR's packing and placement, on the graph at the relaxed width
# that place.sh routes at, with seeds 1, 2 and 3 ($SEEDS), $JOBS runs at a time (default 20),
# each on one thread.

set -u

if [ $# -lt 2 ]; then
    echo "usage: $0 WORK OUT [CIRCUIT...]" >&2
    exit 2
fi
WORK=$(realpath "$1")
OUT=$2
shift 2
CIRCUITS=("$@")
if [ ${#CIRCUITS[@]} -eq 0 ]; then
    for dir in "$WORK"/*/; do
        [ -f "$dir/result.txt" ] && CIRCUITS+=("$(basename "$dir")")
    done
fi

HERE=$(dirname "$(realpath "$0")")
ROOT=$(dirname "$(dirname "$HERE")")
cargo build --release --features fpga --example vtr_eco --manifest-path "$ROOT/Cargo.toml" ||
    exit 1
mkdir -p "$OUT" || exit 1
OUT=$(realpath "$OUT")
# A copy, so that rebuilding during a run doesn't change it.
cp "$ROOT/target/release/examples/vtr_eco" "$OUT/vtr_eco" || exit 1
git -C "$ROOT" rev-parse HEAD > "$OUT/guide_commit" 2> /dev/null

run_one() {
    local circuit=$1 seed=$2 dir="$WORK/$1"
    local width
    width=$(grep -m1 '^relaxed_w=' "$dir/result.txt" | cut -d= -f2)
    if [ ! -f "$dir/w$width/rr.xml" ]; then
        echo "$circuit: no graph at width $width; run place.sh with KEEP_GRAPHS=1" >&2
        return 1
    fi
    "$OUT/vtr_eco" --arch "$dir/arch.xml" --rr-graph "$dir/w$width/rr.xml" \
        --net "$dir/$circuit.net" --place "$dir/vpr.orig.place" --seed "$seed" \
        2> "$OUT/$circuit.$seed.err" | sed "s/^/circuit=$circuit /" > "$OUT/$circuit.$seed.txt"
}
export -f run_one
export WORK OUT

for circuit in "${CIRCUITS[@]}"; do
    for seed in ${SEEDS:-1 2 3}; do
        echo "$circuit $seed"
    done
done | xargs -P "${JOBS:-20}" -L1 bash -c 'run_one "$0" "$1"'
