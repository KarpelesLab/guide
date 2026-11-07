#!/bin/bash
# Compares guide with VPR on one circuit. Called by scripts/vtr/compare.sh, which says more.
#
#     scripts/vtr/circuit.sh VPR VTR_ROUTE ARCH BLIF OUT
#
# VPR packs and places the circuit once, with seed 1. Then, on that placement:
#
# 1. VPR finds its minimum channel width by its own binary search.
# 2. guide finds its own on graphs that VPR writes, with VPR's rules: even widths only, as
#    the architecture's wires are unidirectional, 50 iterations at each, and a binary search
#    between a width that fails and one that routes. It starts from VPR's width.
# 3. At 1.3 times VPR's minimum, rounded to an even number as VTR's flow does, VPR routes
#    and guide routes on the graph VPR wrote for that run, single-threaded, and then with
#    $GUIDE_THREADS threads and batches of 32.
#
# Every route of guide's that is reported was loaded back into VPR with --analysis, which
# checks it as it checks its own and then times it. Results go to OUT/result.txt, one
# key=value per line.
#
# Environment: GUIDE_ARGS, more options for vtr_route; GUIDE_THREADS, default 16;
# KEEP_GRAPHS=1 to keep the graphs, which take hundreds of MB on the larger circuits.

set -u

if [ $# -ne 5 ]; then
    echo "usage: $0 VPR VTR_ROUTE ARCH BLIF OUT" >&2
    exit 2
fi
VPR=$1
ROUTE=$2
ARCH=$3
BLIF=$4
OUT=$5
NAME=$(basename "$BLIF" .blif)
NAME=${NAME%.pre-vpr}
GUIDE_ARGS=${GUIDE_ARGS:-}
THREADS=${GUIDE_THREADS:-16}

mkdir -p "$OUT" && cd "$OUT" || exit 1
cp "$ARCH" arch.xml && cp "$BLIF" "$NAME.blif" || exit 1
RESULT=$PWD/result.txt
: > "$RESULT"

record() {
    echo "$1=$2" >> "$RESULT"
}

# The value after a fixed prefix on the first line of a log that has it.
field() {
    grep -m1 -F "$2" "$1" | sed -e "s/.*$2 *//" -e 's/[ ,].*//'
}

# VPR's options for using the packing and placement from step 0.
LOADED="--net_file ../$NAME.net --place_file ../$NAME.place -j 1"

# 0. Pack and place.
"$VPR" arch.xml "$NAME.blif" --pack --place --seed 1 -j 1 > pack_place.log 2>&1 || {
    record error pack_place
    exit 1
}
record circuit "$NAME"
record clbs "$(grep -m1 -E '^ *clb +[0-9]+' pack_place.log | awk '{print $2}')"
record grid "$(grep -m1 'FPGA sized to' pack_place.log | sed -e 's/.*sized to //' -e 's/ .*//')"

# 1. VPR's own minimum channel width.
mkdir -p vpr_min && (
    cd vpr_min &&
        "$VPR" ../arch.xml "../$NAME.blif" $LOADED --route --route_file vpr.route > route.log 2>&1
)
W_VPR=$(field vpr_min/route.log "Best routing used a channel width factor of")
W_VPR=${W_VPR%.}
if [ -z "$W_VPR" ]; then
    record error vpr_min_w
    exit 1
fi
record vpr_min_w "$W_VPR"
record vpr_min_w_search_s "$(field vpr_min/route.log '# Routing took')"

# Writes the graph at a width into wW/rr.xml, routing with VPR there if $2 is "route", and
# otherwise for a single iteration, which is enough to write the graph.
graph_at() {
    local w=$1 dir=w$1
    [ -f "$dir/rr.xml" ] && return 0
    mkdir -p "$dir"
    local iterations="--max_router_iterations 1"
    [ "${2:-}" = route ] && iterations=""
    (
        cd "$dir" &&
            "$VPR" ../arch.xml "../$NAME.blif" $LOADED --route --route_chan_width "$w" \
                --route_file vpr.route --write_rr_graph rr.xml $iterations > vpr_route.log 2>&1
    )
    [ -f "$dir/rr.xml" ]
}

# Routes with guide at a width: $2 is a label for the files, and the rest go to vtr_route.
# Succeeds if guide's route is legal, by guide's validator.
guide_at() {
    local w=$1 label=$2
    shift 2
    graph_at "$w" || return 2
    # VPR 9.0.0 writes the placement again on every run, with the path of the netlist in its
    # header (print_place in vpr_api.cpp is outside the `if` of the analytical placer), so
    # the digest that a route file has to name is that of the file as it is now.
    local id
    id="SHA256:$(sha256sum "$NAME.place" | cut -d' ' -f1)"
    (
        cd "w$w" &&
            "$ROUTE" --arch ../arch.xml --rr-graph rr.xml --net "../$NAME.net" \
                --place "../$NAME.place" --placement-id "$id" --out "$label.route" \
                $GUIDE_ARGS "$@" > "$label.txt" 2> "$label.err"
    )
}

# Has VPR load and check guide's route at a width, and time it.
vpr_check() {
    local w=$1 label=$2
    (
        cd "w$w" &&
            "$VPR" ../arch.xml "../$NAME.blif" $LOADED --analysis --route_chan_width "$w" \
                --read_rr_graph rr.xml --route_file "$label.route" > "$label.check.log" 2>&1
    )
    grep -q 'Completed routing consistency check successfully' "w$w/$label.check.log" &&
        grep -q 'Circuit successfully routed' "w$w/$label.check.log"
}

# 2. guide's minimum channel width.
LO=0
HI=0
if guide_at "$W_VPR" search --max-iterations 50; then
    HI=$W_VPR
    STEP=2
    while [ $((HI - STEP)) -gt "$LO" ] && [ $((HI - STEP)) -ge 2 ]; do
        W=$((HI - STEP))
        if guide_at "$W" search --max-iterations 50; then
            HI=$W
            STEP=$((STEP * 2))
        else
            LO=$W
            break
        fi
    done
else
    LO=$W_VPR
    STEP=2
    while [ "$HI" -eq 0 ] && [ $((LO + STEP)) -le $((4 * W_VPR)) ]; do
        W=$((LO + STEP))
        if guide_at "$W" search --max-iterations 50; then
            HI=$W
        else
            LO=$W
            STEP=$((STEP * 2))
        fi
    done
fi
if [ "$HI" -eq 0 ]; then
    record guide_min_w none
else
    while [ $((HI - LO)) -gt 2 ]; do
        W=$(((LO + HI) / 2))
        W=$((W + W % 2))
        [ "$W" -ge "$HI" ] && W=$((HI - 2))
        if guide_at "$W" search --max-iterations 50; then
            HI=$W
        else
            LO=$W
        fi
    done
    record guide_min_w "$HI"
    record guide_min_w_fails_at "$LO"
    if vpr_check "$HI" search; then
        record guide_min_w_vpr_legal yes
    else
        record guide_min_w_vpr_legal no
    fi
fi

# 3. At the relaxed width.
W_RELAXED=$(python3 -I -c "print(int(2 * round($W_VPR * 1.3 / 2)))")
record relaxed_w "$W_RELAXED"
rm -rf "w$W_RELAXED"
graph_at "$W_RELAXED" route || {
    record error relaxed_graph
    exit 1
}
LOG=w$W_RELAXED/vpr_route.log
if grep -q 'Circuit successfully routed' "$LOG"; then
    record vpr_legal yes
else
    record vpr_legal no
fi
record vpr_cpd_ns "$(field "$LOG" 'Final critical path delay (least slack):')"
record vpr_wirelength "$(field "$LOG" 'Total wirelength:')"
record vpr_route_s "$(field "$LOG" '# Routing took')"
record vpr_lookahead_s "$(field "$LOG" '# Computing router lookahead map took')"
record vpr_iterations "$(field "$LOG" 'Successfully routed after')"

for run in "one 1 1" "many $THREADS 32"; do
    set -- $run
    label=$1
    guide_at "$W_RELAXED" "$label" --threads "$2" --batch "$3"
    TXT=w$W_RELAXED/$label.txt
    for key in legal iterations wirelength critical_path_ps route_s read_s; do
        record "guide_${label}_$key" "$(tr ' ' '\n' < "$TXT" | grep "^$key=" | cut -d= -f2)"
    done
    if vpr_check "$W_RELAXED" "$label"; then
        record "guide_${label}_vpr_legal" yes
    else
        record "guide_${label}_vpr_legal" no
    fi
    CHECK=w$W_RELAXED/$label.check.log
    record "guide_${label}_vpr_cpd_ns" "$(field "$CHECK" 'Final critical path delay (least slack):')"
    record "guide_${label}_vpr_wirelength" "$(field "$CHECK" 'Total wirelength:')"
done

if [ "${KEEP_GRAPHS:-0}" != 1 ]; then
    rm -f w*/rr.xml
fi
record done yes
