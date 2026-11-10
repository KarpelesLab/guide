#!/bin/sh
# Runs guide on MovingAI MAPF instances, in parallel over scenarios.
#
#   scripts/mapf/guide.sh DATA OUT [MAP...] [-- mapf_bench options]
#
# DATA is where scripts/mapf/fetch.sh put the benchmarks. For each map, scenarios 1 to $SCENS
# (default 25) of the random set are each run with the map's agent counts below, one process
# per scenario, $JOBS at a time (default 8), under a time limit of $LIMIT seconds per
# instance (default 60). Lines go to OUT/guide-MAP.tsv; see examples/mapf_bench.rs for the
# columns.
DATA=${1:?usage: guide.sh DATA OUT [MAP...] [-- options]}
OUT=${2:?usage: guide.sh DATA OUT [MAP...] [-- options]}
shift 2
MAPS=
while [ $# -gt 0 ] && [ "$1" != "--" ]; do MAPS="$MAPS $1"; shift; done
[ "$1" = "--" ] && shift
[ -n "$MAPS" ] || MAPS="empty-32-32 random-32-32-20 room-32-32-4 maze-32-32-2 warehouse-10-20-10-2-1 den520d"
SCENS=${SCENS:-25}
JOBS=${JOBS:-8}
LIMIT=${LIMIT:-60}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cargo build --release --quiet --features mapf --example mapf_bench --manifest-path "$ROOT/Cargo.toml" || exit 1
BIN="$ROOT/target/release/examples/mapf_bench"
mkdir -p "$OUT" || exit 1

counts() {
  case "$1" in
    empty-32-32) echo 50,100,150,200,250,300,350,400,450,500 ;;
    random-32-32-20) echo 25,50,75,100,125,150,175,200,225,250 ;;
    room-32-32-4) echo 10,20,30,40,50,60,70,80,90,100 ;;
    maze-32-32-2) echo 10,20,30,40,50,60,70,80,90,100 ;;
    warehouse-10-20-10-2-1) echo 50,100,150,200,250,300,350,400,450,500 ;;
    den520d) echo 100,200,300,400,500,600,700,800,900,1000 ;;
    *) echo 10,20,50,100 ;;
  esac
}

for map in $MAPS; do
  seq 1 "$SCENS" | xargs -P "$JOBS" -I{} "$BIN" "$DATA/maps/$map.map" \
    "$DATA/scen/scen-random/$map-random-{}.scen" "$(counts "$map")" --time "$LIMIT" "$@" \
    > "$OUT/guide-$map.tsv" || exit 1
done
