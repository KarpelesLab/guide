#!/bin/sh
# Routes every design of designs.txt with reticle's router and with guide's, and writes one
# line of JSON per router and design to <out>/<name>.json; tables.py makes the tables.
#
#   scripts/reticle/route/run.sh <out dir> [name...]
#
# Environment:
#   RETICLE   reticle's checkout (default: beside guide's)
#   ROUTERS   the routers to run (default: reticle,guide), see src/main.rs; a router
#             with threads (guide@8) is best run with JOBS=1, so that it has the cores
#   JOBS      designs at once (default: 4); a design is placed once, and its routers
#             run on that placement one after the other
#   ARGS      more arguments for every design, such as --timing-driven
#   CARGO_TARGET_DIR  where to build
#
# Everything runs under `nice -n 19`.
#
# The device databases are reticle's: `reticle fetch prjxray-db`, `prjtrellis-db` and
# `apicula` put them in ~/.cache/reticle, where this finds them.
here=$(cd "$(dirname "$0")" && pwd)
out=${1:?usage: run.sh <out dir> [name...]}
shift
mkdir -p "$out" && out=$(cd "$out" && pwd) || exit 1
reticle=${RETICLE:-$here/../../../../reticle}
reticle=$(cd "$reticle" && pwd) || exit 1
routers=${ROUTERS:-reticle,guide}
jobs=${JOBS:-4}

(cd "$here" && nice -n 19 cargo build --release --quiet) || exit 1
bin=${CARGO_TARGET_DIR:-$here/target}/release/reticle-route

grep -v '^#' "$here/designs.txt" | grep -v '^ *$' | while read -r name device rcf top rest; do
    if [ $# -gt 0 ]; then
        wanted=no
        for w in "$@"; do [ "$w" = "$name" ] && wanted=yes; done
        [ $wanted = yes ] || continue
    fi
    echo "$name $device $rcf $top $rest" | sed 's/ *$//'
done | xargs -P "$jobs" -L 1 sh -c '
    name=$1 device=$2 rcf=$3 top=$4
    shift 4
    cd "'"$reticle"'" || exit 1
    sources=$(python3 -I "'"$here"'/sources.py" . "$top")
    nice -n 19 "'"$bin"'" --name "$name" --device "$device" --constraints "$rcf" \
        --routers "'"$routers"'" '"${ARGS:-}"' "$@" $sources \
        > "'"$out"'/$name.json" 2> "'"$out"'/$name.log"
    echo "$name: exit $?"
' sh
