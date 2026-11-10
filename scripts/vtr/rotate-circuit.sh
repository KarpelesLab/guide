#!/bin/bash
# Rotates the inputs of a circuit's LUTs for timing after routing, and has VPR check and time
# the result. Called by scripts/vtr/rotate.sh, which says more.
#
#     scripts/vtr/rotate-circuit.sh VPR VTR_ROUTE DIR PLACEMENT
#
# DIR is the directory of a run of scripts/vtr/place-circuit.sh on the circuit, and PLACEMENT
# is vpr or guide, for the placement DIR/PLACEMENT.orig.place.
#
# 1. guide's router routes the placement at the relaxed width of DIR's run, and then, with the
#    delay of every connection known, vtr_route --rotate-luts moves each LUT's latest inputs to
#    its fastest pins, and writes the packed netlist with them.
# 2. VPR checks and times guide's routes, with the netlist as it was, and as rotated, with
#    --analysis.
# 3. VPR's router routes the placement at the same width, with the netlist as it was and as
#    rotated.
#
# Results go to DIR/rotate.PLACEMENT/result.txt.

set -u

if [ $# -ne 4 ]; then
    echo "usage: $0 VPR VTR_ROUTE DIR PLACEMENT" >&2
    exit 2
fi
VPR=$1
ROUTE=$2
DIR=$(realpath "$3")
PLACEMENT=$4
NAME=$(grep '^circuit=' "$DIR/result.txt" | cut -d= -f2)
W=$(grep '^relaxed_w=' "$DIR/result.txt" | cut -d= -f2)
[ -n "$NAME" ] && [ -n "$W" ] || exit 1

OUT=$DIR/rotate.$PLACEMENT
mkdir -p "$OUT" && cd "$OUT" || exit 1
RESULT=$OUT/result.txt
: > "$RESULT"
record() {
    echo "$1=$2" >> "$RESULT"
}
field() {
    grep -m1 -F "$2" "$1" | sed -e "s/.*$2 *//" -e 's/[ ,].*//'
}
legal() {
    grep -q 'Completed routing consistency check successfully' "$1" &&
        grep -q 'Circuit successfully routed' "$1" && echo yes || echo no
}
digest() {
    echo "SHA256:$(sha256sum "$1" | cut -d' ' -f1)"
}
cp "$DIR/arch.xml" "$DIR/$NAME.blif" "$DIR/$NAME.net" . || exit 1

# The graph at the relaxed width.
if [ ! -f rr.xml ]; then
    "$VPR" arch.xml "$NAME.blif" --net_file "$NAME.net" --place_file "$DIR/vpr.orig.place" -j 1 \
        --route --route_chan_width "$W" --route_file graph.route --write_rr_graph rr.xml \
        --max_router_iterations 1 > graph.log 2>&1
fi

# VPR rewrites a placement file whose header doesn't name the netlist as it was given, so each
# placement names its netlist so.
sed -e "1s/.*/Netlist_File: $NAME.net Netlist_ID: $(digest "$NAME.net")/" \
    "$DIR/$PLACEMENT.orig.place" > plain.place
"$ROUTE" --arch arch.xml --rr-graph rr.xml --net "$NAME.net" --place plain.place \
    --placement-id "$(digest plain.place)" --out plain.route --rotate-luts rotated.net \
    > route.txt 2> route.err
for key in legal critical_path_ps rotatable_luts rotated_luts slack_before_rotation_ps \
    slack_after_rotation_ps rotation_s route_s; do
    record "$key" "$(tr ' ' '\n' < route.txt | grep "^$key=" | cut -d= -f2)"
done
[ -f rotated.net ] || {
    record done no
    exit 1
}
sed -e "1s/.*/Netlist_File: rotated.net Netlist_ID: $(digest rotated.net)/" plain.place \
    > rotated.place
sed -e "1s/.*/Placement_File: rotated.place Placement_ID: $(digest rotated.place)/" plain.route \
    > rotated.route

for version in plain rotated; do
    net=$NAME.net
    [ "$version" = rotated ] && net=rotated.net
    "$VPR" arch.xml "$NAME.blif" --net_file "$net" --place_file "$version.place" -j 1 \
        --analysis --route_chan_width "$W" --read_rr_graph rr.xml --route_file "$version.route" \
        > "$version.check.log" 2>&1
    record "${version}_guide_route_legal" "$(legal "$version.check.log")"
    record "${version}_guide_route_cpd_ns" \
        "$(field "$version.check.log" 'Final critical path delay (least slack):')"
    mkdir -p "vpr.$version" && (
        cd "vpr.$version" &&
            "$VPR" ../arch.xml "../$NAME.blif" --net_file "../$net" \
                --place_file "../$version.place" -j 1 --route --route_chan_width "$W" \
                --read_rr_graph ../rr.xml --route_file vpr.route > route.log 2>&1
    )
    record "${version}_vpr_route_legal" "$(legal "vpr.$version/route.log")"
    record "${version}_vpr_route_cpd_ns" \
        "$(field "vpr.$version/route.log" 'Final critical path delay (least slack):')"
    record "${version}_vpr_route_wirelength" "$(field "vpr.$version/route.log" 'Total wirelength:')"
done
rm -f rr.xml vpr.*/vpr.route
record done yes
