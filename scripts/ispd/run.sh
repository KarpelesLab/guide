#!/bin/bash
# Routes the ISPD 2008 global routing contest's benchmarks, and the ISPD 2007 ones, with the
# `ispd_route` example, and scores each result.
#
#     scripts/ispd/fetch.sh DIR                  # once
#     scripts/ispd/run.sh DIR WORK [NAME...]     # all 16 ISPD 2008 benchmarks by default
#     python3 scripts/ispd/tables.py WORK        # the tables in docs/benchmarks.md
#
#     SUITE=ispd07-2d scripts/ispd/run.sh DIR WORK   # the eight 2D benchmarks of 2007
#     SUITE=ispd07-3d scripts/ispd/run.sh DIR WORK   # the eight 3D ones, scored as in 2007
#     SUITE=ispd07-3d ROUTES=WORK08 scripts/ispd/run.sh DIR WORK
#
# The 3D designs of 2007 are those of 2008, and routing does not depend on how vias are
# scored, so ROUTES can name the WORK of an ISPD 2008 run, whose route files are then scored
# with 2007's via cost instead of routing again.
#
# NAME is a benchmark's name without its suffix, such as adaptec1. Benchmarks run $JOBS at a
# time (default 1), each on $THREADS threads (default 16). GUIDE_ARGS passes more options to
# every run. Each run leaves WORK/NAME.route, WORK/NAME.log with one line of key=value pairs,
# and WORK/NAME.err. With EVAL=1, eval2008.pl from DIR/eval scores each route file as well,
# into WORK/NAME.eval, which takes as long as routing and far more memory.
#
# The 2007 contest counted 3 for each via in the wirelength, and 2008 1. Both are scored by
# this crate's evaluator, which follows eval2008.pl, with the via cost of the year.

set -u

if [ $# -lt 2 ]; then
    echo "usage: $0 DIR WORK [NAME...]" >&2
    exit 2
fi
DIR=$(realpath "$1")
WORK=$2
shift 2
SUITE=${SUITE:-ispd08}
JOBS=${JOBS:-1}
THREADS=${THREADS:-16}
mkdir -p "$WORK" || exit 1

case "$SUITE" in
ispd08)
    ALL=(adaptec1 adaptec2 adaptec3 adaptec4 adaptec5 bigblue1 bigblue2 bigblue3 bigblue4
        newblue1 newblue2 newblue3 newblue4 newblue5 newblue6 newblue7)
    PATTERN='3d'
    VIA=1
    ;;
ispd07-2d)
    ALL=(adaptec1 adaptec2 adaptec3 adaptec4 adaptec5 newblue1 newblue2 newblue3)
    PATTERN='2d'
    VIA=3
    ;;
ispd07-3d)
    ALL=(adaptec1 adaptec2 adaptec3 adaptec4 adaptec5 newblue1 newblue2 newblue3)
    PATTERN='3d'
    VIA=3
    ;;
*)
    echo "unknown SUITE $SUITE" >&2
    exit 2
    ;;
esac
NAMES=("$@")
if [ ${#NAMES[@]} -eq 0 ]; then
    NAMES=("${ALL[@]}")
fi

cargo build --release --features asic --example ispd_route || exit 1
BIN=$(realpath target/release/examples/ispd_route)

run_one() {
    local name=$1
    local file
    file=$(ls "$DIR"/bench/"$name".*."$PATTERN".*gr 2>/dev/null | head -n 1)
    if [ -z "$file" ]; then
        echo "$name: no $PATTERN benchmark in $DIR/bench" >&2
        return 1
    fi
    if [ -n "${ROUTES:-}" ]; then
        "$BIN" "$file" --score "$ROUTES/$name.route" --via-cost "$VIA" \
            > "$WORK/$name.log" 2> "$WORK/$name.err"
        echo "$name: $(cat "$WORK/$name.log")"
        return
    fi
    # shellcheck disable=SC2086
    /usr/bin/time -f "time=%e cpu=%U maxrss_kb=%M" "$BIN" "$file" --threads "$THREADS" \
        --via-cost "$VIA" --out "$WORK/$name.route" ${GUIDE_ARGS:-} \
        > "$WORK/$name.log" 2> "$WORK/$name.err"
    if [ "${EVAL:-0}" = 1 ]; then
        perl "$DIR/eval/eval2008/eval2008.pl" "$file" "$WORK/$name.route" \
            > "$WORK/$name.eval" 2>&1
    fi
    echo "$name: $(cat "$WORK/$name.log") $(tail -n 1 "$WORK/$name.err")"
}

running=0
for name in "${NAMES[@]}"; do
    run_one "$name" &
    running=$((running + 1))
    if [ "$running" -ge "$JOBS" ]; then
        wait -n
        running=$((running - 1))
    fi
done
wait
