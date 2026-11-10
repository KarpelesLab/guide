#!/bin/bash
# Compares guide's placer with VPR's on one circuit. Called by scripts/vtr/place.sh, which
# says more.
#
#     scripts/vtr/place-circuit.sh VPR VTR_PLACE VTR_ROUTE ARCH BLIF OUT
#
# 0. VPR packs and places the circuit, with seed 1, or that is copied from $FROM/CIRCUIT, a
#    directory of an earlier run of this script or of scripts/vtr/circuit.sh.
# 1. VPR finds its minimum channel width on its placement, and routes it at 1.3 times that,
#    rounded to an even number, writing the graph of that width.
# 2. guide places the same packing on that graph's device: vtr_place, with $PLACE_ARGS.
# 3. VPR loads guide's placement with every block fixed (--fix_clusters), which checks that it
#    is legal and measures it as VPR measures its own.
# 4. On both placements: VPR finds its minimum channel width and routes at the relaxed width
#    of step 1, and guide's router does the same, with VPR checking and timing each of its
#    routes, as scripts/vtr/circuit.sh does.
#
# Results go to OUT/result.txt, one key=value per line: vpr_* for VPR's placement and guide_*
# for guide's, then which router, as in vpr_route_* and guide_route_*.
#
# Environment: PLACE_ARGS, options for vtr_place; KEEP_GRAPHS=1 to keep the graphs;
# PLACE_FILE=FILE to take guide's placement from FILE instead of placing, which vtr_place then
# only measures, with every block fixed; ONLY_GUIDE=1, with FROM, to route guide's placement
# alone, taking VPR's minimum channel width on its own placement, and the relaxed width, from
# $FROM/CIRCUIT/result.txt.

set -u

if [ $# -ne 6 ]; then
    echo "usage: $0 VPR VTR_PLACE VTR_ROUTE ARCH BLIF OUT" >&2
    exit 2
fi
VPR=$1
PLACE=$2
ROUTE=$3
ARCH=$4
BLIF=$5
OUT=$6
NAME=$(basename "$BLIF" .blif)
NAME=${NAME%.pre-vpr}
PLACE_ARGS=${PLACE_ARGS:-}

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

# The same, on the last line that has it.
last_field() {
    grep -F "$2" "$1" | tail -1 | sed -e "s/.*$2 *//" -e 's/[ ,].*//'
}

# 0. Pack and place.
if [ -n "${FROM:-}" ] && [ -f "$FROM/$NAME/$NAME.net" ] && [ -f "$FROM/$NAME/pack_place.log" ]; then
    # A run of scripts/vtr/circuit.sh keeps VPR's placement as CIRCUIT.place, and one of this
    # script as vpr.orig.place.
    from_place=$FROM/$NAME/$NAME.place
    [ -f "$from_place" ] || from_place=$FROM/$NAME/vpr.orig.place
    cp "$FROM/$NAME/$NAME.net" "$FROM/$NAME/pack_place.log" . &&
        cp "$from_place" vpr.place || exit 1
else
    "$VPR" arch.xml "$NAME.blif" --pack --place --seed 1 -j 1 > pack_place.log 2>&1 || {
        record error pack_place
        exit 1
    }
    mv "$NAME.place" vpr.place || exit 1
fi
# A copy that no run of VPR rewrites.
cp vpr.place vpr.orig.place
record circuit "$NAME"
record clbs "$(grep -m1 -E '^ *clb +[0-9]+' pack_place.log | awk '{print $2}')"
for block in mult_36 memory; do
    count=$(grep -m1 -E "^ *$block +[0-9]+" pack_place.log | awk '{print $2}')
    [ -n "$count" ] && record "$block" "$count"
done
record grid "$(grep -m1 'FPGA sized to' pack_place.log | sed -e 's/.*sized to //' -e 's/ .*//')"
record vpr_place_s "$(field pack_place.log '# Placement took')"
record vpr_delay_model_s "$(field pack_place.log '# Computing placement delta delay look-up took')"
record vpr_bb "$(last_field pack_place.log 'BB estimate of min-dist (placement) wire length:')"
record vpr_est_cpd_ns "$(field pack_place.log 'Placement estimated critical path delay (least slack):')"
NETID="SHA256:$(sha256sum "$NAME.net" | cut -d' ' -f1)"

# Routes placement $1 (vpr or guide) with VPR, in directory $2, with more options after.
vpr_route() {
    local placement=$1 dir=$2
    shift 2
    mkdir -p "$dir" && (
        cd "$dir" &&
            "$VPR" ../arch.xml "../$NAME.blif" --net_file "../$NAME.net" \
                --place_file "../$placement.place" -j 1 --route --route_file vpr.route "$@" \
                > route.log 2>&1
    )
}

# 1. VPR's minimum channel width on its placement, and the relaxed width.
if [ "${ONLY_GUIDE:-0}" = 1 ] && [ -f "$FROM/$NAME/result.txt" ]; then
    W_VPR=$(grep '^vpr_route_vpr_min_w=' "$FROM/$NAME/result.txt" | cut -d= -f2)
else
    vpr_route vpr vpr.vpr_min
    W_VPR=$(field vpr.vpr_min/route.log "Best routing used a channel width factor of")
    W_VPR=${W_VPR%.}
fi
if [ -z "$W_VPR" ]; then
    record error vpr_min_w
    exit 1
fi
record vpr_route_vpr_min_w "$W_VPR"
W_RELAXED=$(python3 -I -c "print(int(2 * round($W_VPR * 1.3 / 2)))")
record relaxed_w "$W_RELAXED"

# The graph at width $1 goes in wW/rr.xml. Graphs don't depend on the placement.
graph_at() {
    local w=$1 dir=w$1
    [ -f "$dir/rr.xml" ] && return 0
    mkdir -p "$dir"
    (
        cd "$dir" &&
            "$VPR" ../arch.xml "../$NAME.blif" --net_file "../$NAME.net" \
                --place_file ../vpr.place -j 1 --route --route_chan_width "$w" \
                --route_file graph.route --write_rr_graph rr.xml --max_router_iterations 1 \
                > graph.log 2>&1
    )
    [ -f "$dir/rr.xml" ]
}
graph_at "$W_RELAXED" || {
    record error relaxed_graph
    exit 1
}

# 2. guide's placement.
if [ -n "${PLACE_FILE:-}" ]; then
    cp "$PLACE_FILE" given.place || exit 1
    PLACE_ARGS="$PLACE_ARGS --fix given.place"
fi
"$PLACE" --arch arch.xml --rr-graph "w$W_RELAXED/rr.xml" --net "$NAME.net" --out guide.place \
    --netlist-file "../$NAME.net" --netlist-id "$NETID" --evaluate vpr.orig.place $PLACE_ARGS \
    > place.txt 2> place.err
for key in legal blocks nets chains chained_blocks bb_estimate start_bb_estimate worst_slack_ps \
    temperatures moves read_s delay_model_s place_s other_bb_estimate other_worst_slack_ps; do
    record "guide_place_$key" "$(tr ' ' '\n' < place.txt | grep "^$key=" | cut -d= -f2)"
done
if [ ! -f guide.place ]; then
    record error guide_place
    exit 1
fi
cp guide.place guide.orig.place

# 3. VPR loads guide's placement with every block fixed.
mkdir -p vpr_check && (
    cd vpr_check &&
        grep -v -e '^Netlist_File' -e '^Array' ../guide.orig.place > fixed.place &&
        "$VPR" ../arch.xml "../$NAME.blif" --net_file "../$NAME.net" -j 1 --place \
            --fix_clusters fixed.place --place_file check.place > place.log 2>&1
)
if grep -q 'Completed placement consistency check successfully' vpr_check/place.log &&
    [ "$(grep -v '^#' vpr_check/check.place | grep -v -e '^Netlist' -e '^Array' | awk 'NF {print $1, $2, $3, $4}' | sort)" = \
        "$(grep -v '^#' guide.orig.place | grep -v -e '^Netlist' -e '^Array' | awk 'NF {print $1, $2, $3, $4}' | sort)" ]; then
    record guide_vpr_legal yes
else
    record guide_vpr_legal no
fi
record guide_vpr_bb "$(last_field vpr_check/place.log 'BB estimate of min-dist (placement) wire length:')"
record guide_vpr_est_cpd_ns "$(field vpr_check/place.log 'Placement estimated critical path delay (least slack):')"

# 4. Routing both placements.

# Routes placement $1 with guide at width $2: $3 is a label for the files, and the rest go to
# vtr_route. Succeeds if guide's route is legal, by guide's validator.
guide_at() {
    local placement=$1 w=$2 label=$3
    shift 3
    graph_at "$w" || return 2
    # VPR rewrites a placement file whenever it loads one, so the digest a route file names is
    # that of the file as it is now.
    local id
    id="SHA256:$(sha256sum "$placement.place" | cut -d' ' -f1)"
    (
        cd "w$w" &&
            "$ROUTE" --arch ../arch.xml --rr-graph rr.xml --net "../$NAME.net" \
                --place "../$placement.place" --placement-id "$id" --out "$label.route" \
                "$@" > "$label.txt" 2> "$label.err"
    )
}

# Has VPR load and check a route of guide's, and time it.
vpr_check() {
    local placement=$1 w=$2 label=$3
    (
        cd "w$w" &&
            "$VPR" ../arch.xml "../$NAME.blif" --net_file "../$NAME.net" \
                --place_file "../$placement.place" -j 1 --analysis --route_chan_width "$w" \
                --read_rr_graph rr.xml --route_file "$label.route" > "$label.check.log" 2>&1
    )
    grep -q 'Completed routing consistency check successfully' "w$w/$label.check.log" &&
        grep -q 'Circuit successfully routed' "w$w/$label.check.log"
}

PLACEMENTS="vpr guide"
[ "${ONLY_GUIDE:-0}" = 1 ] && PLACEMENTS=guide
for placement in $PLACEMENTS; do
    # VPR's router.
    if [ "$placement" = guide ]; then
        vpr_route guide guide.vpr_min
        W=$(field guide.vpr_min/route.log "Best routing used a channel width factor of")
        W=${W%.}
        record guide_route_vpr_min_w "${W:-none}"
    fi
    vpr_route "$placement" "$placement.vpr_relaxed" --route_chan_width "$W_RELAXED"
    LOG=$placement.vpr_relaxed/route.log
    if grep -q 'Circuit successfully routed' "$LOG"; then
        record "${placement}_route_vpr_legal" yes
    else
        record "${placement}_route_vpr_legal" no
    fi
    record "${placement}_route_vpr_cpd_ns" "$(field "$LOG" 'Final critical path delay (least slack):')"
    record "${placement}_route_vpr_wirelength" "$(field "$LOG" 'Total wirelength:')"
    record "${placement}_route_vpr_s" "$(field "$LOG" '# Routing took')"

    # guide's router, at the relaxed width.
    guide_at "$placement" "$W_RELAXED" "$placement.relaxed"
    TXT=w$W_RELAXED/$placement.relaxed.txt
    for key in legal iterations wirelength route_s; do
        record "${placement}_route_guide_$key" "$(tr ' ' '\n' < "$TXT" | grep "^$key=" | cut -d= -f2)"
    done
    if vpr_check "$placement" "$W_RELAXED" "$placement.relaxed"; then
        record "${placement}_route_guide_vpr_legal" yes
    else
        record "${placement}_route_guide_vpr_legal" no
    fi
    CHECK=w$W_RELAXED/$placement.relaxed.check.log
    record "${placement}_route_guide_cpd_ns" "$(field "$CHECK" 'Final critical path delay (least slack):')"
    record "${placement}_route_guide_vpr_wirelength" "$(field "$CHECK" 'Total wirelength:')"

    # guide's minimum channel width, from VPR's on the same placement.
    if [ "$placement" = vpr ]; then
        START=$W_VPR
    else
        START=$(grep '^guide_route_vpr_min_w=' "$RESULT" | cut -d= -f2)
        [ "$START" = none ] && START=$W_VPR
    fi
    search() {
        guide_at "$placement" "$1" "$placement.search" --max-iterations 50
    }
    LO=0
    HI=0
    if search "$START"; then
        HI=$START
        STEP=2
        while [ $((HI - STEP)) -gt "$LO" ] && [ $((HI - STEP)) -ge 2 ]; do
            W=$((HI - STEP))
            if search "$W"; then
                HI=$W
                STEP=$((STEP * 2))
            else
                LO=$W
                break
            fi
        done
    else
        LO=$START
        STEP=2
        while [ "$HI" -eq 0 ] && [ $((LO + STEP)) -le $((4 * START)) ]; do
            W=$((LO + STEP))
            if search "$W"; then
                HI=$W
            else
                LO=$W
                STEP=$((STEP * 2))
            fi
        done
    fi
    if [ "$HI" -eq 0 ]; then
        record "${placement}_route_guide_min_w" none
    else
        while [ $((HI - LO)) -gt 2 ]; do
            W=$(((LO + HI) / 2))
            W=$((W + W % 2))
            [ "$W" -ge "$HI" ] && W=$((HI - 2))
            if search "$W"; then
                HI=$W
            else
                LO=$W
            fi
        done
        record "${placement}_route_guide_min_w" "$HI"
        # The search's last route is not always at the minimum, so route there again.
        search "$HI"
        if vpr_check "$placement" "$HI" "$placement.search"; then
            record "${placement}_route_guide_min_w_vpr_legal" yes
        else
            record "${placement}_route_guide_min_w_vpr_legal" no
        fi
    fi
done

if [ "${KEEP_GRAPHS:-0}" != 1 ]; then
    rm -f w*/rr.xml
fi
record done yes
