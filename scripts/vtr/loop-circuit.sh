#!/bin/bash
# Narrows the channels that guide's placement of one circuit needs by placing and routing in a
# loop, then measures the result as scripts/vtr/place-circuit.sh measures a placement. Called
# by scripts/vtr/loop.sh, which says more.
#
#     scripts/vtr/loop-circuit.sh VPR VTR_LOOP VTR_PLACE VTR_ROUTE ARCH BLIF BASE OUT
#
# BASE is the directory of a run of place-circuit.sh on the circuit: the one-pass flow, whose
# placement, guide.orig.place, the loop starts from, and whose minimum channel width with
# guide's router, W, it starts below.
#
# 1. At W - 2, vtr_loop routes the placement with guide's router, and where that fails places
#    again around what the router overused and routes again, a few rounds, with $LOOP_ARGS.
#    If it ends with a legal routing, its placement is the next one to start from, and the
#    loop goes on at a width 2 narrower; otherwise it stops.
# 2. place-circuit.sh measures the last placement that routed, or the one-pass placement if
#    none did, as it measures its own: VPR checks it, and both routers find their minimum
#    channel width on it and route it at the relaxed width, with VPR checking guide's routes.
#
# Results go to OUT/result.txt: loop_* for the loop, then place-circuit.sh's keys for the
# placement it ended with, guide_* among them.

set -u

if [ $# -ne 8 ]; then
    echo "usage: $0 VPR VTR_LOOP VTR_PLACE VTR_ROUTE ARCH BLIF BASE OUT" >&2
    exit 2
fi
VPR=$1
LOOP=$2
PLACE=$3
ROUTE=$4
ARCH=$5
BLIF=$6
BASE=$(realpath "$7")
OUT=$8
NAME=$(basename "$BLIF" .blif)
NAME=${NAME%.pre-vpr}
LOOP_ARGS=${LOOP_ARGS:-}
HERE=$(dirname "$(realpath "$0")")

mkdir -p "$OUT/loop" && cd "$OUT" || exit 1
OUT=$PWD
RESULT=$OUT/result.txt
: > "$RESULT"
record() {
    echo "$1=$2" >> "$RESULT"
}
base_value() {
    grep "^$1=" "$BASE/result.txt" | cut -d= -f2
}

W0=$(base_value guide_route_guide_min_w)
RELAXED=$(base_value relaxed_w)
if [ -z "$W0" ] || [ "$W0" = none ] || [ -z "$RELAXED" ] || [ ! -f "$BASE/guide.orig.place" ]; then
    record error base
    exit 1
fi
record loop_start_w "$W0"

cd loop || exit 1
cp "$ARCH" arch.xml && cp "$BLIF" "$NAME.blif" && cp "$BASE/$NAME.net" "$BASE/vpr.orig.place" . &&
    cp "$BASE/guide.orig.place" start.place || exit 1
NETID="SHA256:$(sha256sum "$NAME.net" | cut -d' ' -f1)"

# The graph at width $1 goes in wW/rr.xml, as place-circuit.sh makes it.
graph_at() {
    local w=$1 dir=w$1
    [ -f "$dir/rr.xml" ] && return 0
    mkdir -p "$dir"
    (
        cd "$dir" &&
            "$VPR" ../arch.xml "../$NAME.blif" --net_file "../$NAME.net" \
                --place_file ../vpr.orig.place -j 1 --route --route_chan_width "$w" \
                --route_file graph.route --write_rr_graph rr.xml --max_router_iterations 1 \
                > graph.log 2>&1
    )
    [ -f "$dir/rr.xml" ]
}
graph_at "$RELAXED" || {
    record error relaxed_graph
    exit 1
}

# 1. The loop, narrower and narrower.
CURRENT=start.place
REACHED=none
STEPS=0
STARTED=$(date +%s.%N)
W=$((W0 - 2))
while [ "$W" -ge 2 ]; do
    graph_at "$W" || break
    STEPS=$((STEPS + 1))
    "$LOOP" --arch arch.xml --rr-graph "w$RELAXED/rr.xml" --route-rr-graph "w$W/rr.xml" \
        --net "$NAME.net" --place "$CURRENT" --out "loop_w$W.place" \
        --netlist-file "$NAME.net" --netlist-id "$NETID" $LOOP_ARGS \
        > "loop_w$W.txt" 2> "loop_w$W.err"
    status=$?
    rm -f "w$W/rr.xml"
    if [ "$status" -ne 0 ] || [ ! -f "loop_w$W.place" ]; then
        break
    fi
    CURRENT=loop_w$W.place
    REACHED=$W
    W=$((W - 2))
done
ENDED=$(date +%s.%N)
record loop_steps "$STEPS"
record loop_reached_w "$REACHED"
record loop_s "$(python3 -I -c "print(round($ENDED - $STARTED, 3))")"
record loop_place_s "$(cat loop_w*.txt 2> /dev/null | tr ' ' '\n' | grep '^loop_s=' |
    cut -d= -f2 | python3 -I -c 'import sys; print(round(sum(float(x) for x in sys.stdin), 3))')"
rm -f "w$RELAXED/rr.xml"

# 2. Measure what the loop ended with.
cd "$OUT" || exit 1
FROM=$(dirname "$BASE") ONLY_GUIDE=1 PLACE_FILE="$OUT/loop/$CURRENT" \
    "$HERE/place-circuit.sh" "$VPR" "$PLACE" "$ROUTE" "$ARCH" "$BLIF" "$OUT/eval"
cat "$OUT/eval/result.txt" >> "$RESULT"
