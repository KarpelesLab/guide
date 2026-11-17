#!/bin/sh
# Runs guide on MovingAI MAPF instances, in parallel over scenarios.
#
#   scripts/mapf/guide.sh DATA OUT [MAP...] [-- mapf_bench options]
#
# DATA is where scripts/mapf/fetch.sh put the benchmarks. For each map, scenarios 1 to $SCENS
# (default 25) of the $KIND set (random, the default, or even) are each run with the map's
# agent counts below, one process per scenario, $JOBS at a time (default 8), under a time limit
# of $LIMIT seconds per instance (default 60). Each process runs under `nice -n 19` and with at
# most $MEMORY KB of address space (default 12000000, 12 GB), so that a run that grows can't
# take the machine with it. Without MAP, the six maps of the first results;
# MAP may be "all" for every map in DATA. Lines go to OUT/guide-MAP.tsv for the random set and
# OUT/guide-MAP-even.tsv for the even one; see examples/mapf_bench.rs for the columns.
DATA=${1:?usage: guide.sh DATA OUT [MAP...] [-- options]}
OUT=${2:?usage: guide.sh DATA OUT [MAP...] [-- options]}
shift 2
MAPS=
while [ $# -gt 0 ] && [ "$1" != "--" ]; do MAPS="$MAPS $1"; shift; done
[ "$1" = "--" ] && shift
[ -n "$MAPS" ] || MAPS="empty-32-32 random-32-32-20 room-32-32-4 maze-32-32-2 warehouse-10-20-10-2-1 den520d"
[ "$MAPS" = " all" ] && MAPS=$(cd "$DATA/maps" && ls ./*.map | sed 's|^\./||; s|\.map$||')
SCENS=${SCENS:-25}
JOBS=${JOBS:-8}
LIMIT=${LIMIT:-60}
KIND=${KIND:-random}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cargo build --release --quiet --features mapf --example mapf_bench --manifest-path "$ROOT/Cargo.toml" || exit 1
TARGET=$(cargo metadata --format-version 1 --no-deps --manifest-path "$ROOT/Cargo.toml" |
  sed 's/.*"target_directory":"\([^"]*\)".*/\1/') || exit 1
BIN="$TARGET/release/examples/mapf_bench"
mkdir -p "$OUT" || exit 1

# The six maps of the first results keep their counts; every other map has a tenth of its
# scenarios' agents, two tenths, and so on to all of them. With PARTS=n, every map has an n-th
# of them, two n-ths, and so on.
PARTS=${PARTS:-0}
counts() {
  if [ "$PARTS" -gt 0 ]; then
    max=$(($(wc -l < "$DATA/scen/scen-$KIND/$1-$KIND-1.scen") - 1))
    seq 1 "$PARTS" | while read -r k; do printf '%s' "$((max * k / PARTS))"; [ "$k" -lt "$PARTS" ] && printf ','; done
    echo
    return
  fi
  case "$1" in
    empty-32-32) echo 50,100,150,200,250,300,350,400,450,500 ;;
    random-32-32-20) echo 25,50,75,100,125,150,175,200,225,250 ;;
    room-32-32-4) echo 10,20,30,40,50,60,70,80,90,100 ;;
    maze-32-32-2) echo 10,20,30,40,50,60,70,80,90,100 ;;
    warehouse-10-20-10-2-1) echo 50,100,150,200,250,300,350,400,450,500 ;;
    den520d) echo 100,200,300,400,500,600,700,800,900,1000 ;;
    *)
      max=$(($(wc -l < "$DATA/scen/scen-$KIND/$1-$KIND-1.scen") - 1))
      seq 1 10 | while read -r k; do printf '%s' "$((max * k / 10))"; [ "$k" -lt 10 ] && printf ','; done
      echo
      ;;
  esac
}

# One process per map and scenario, all maps at once, each into a part of its own.
suffix=
[ "$KIND" = random ] || suffix="-$KIND"
PARTDIR="$OUT/parts$suffix"
mkdir -p "$PARTDIR" || exit 1
MEMORY=${MEMORY:-12000000}
export BIN DATA KIND LIMIT PARTDIR MEMORY
OPTS="$*"
export OPTS
for map in $MAPS; do
  c=$(counts "$map")
  for s in $(seq 1 "$SCENS"); do echo "$map $s $c"; done
done | xargs -P "$JOBS" -L 1 sh -c '# shellcheck disable=SC2086
  ulimit -v "$MEMORY" && exec nice -n 19 "$BIN" "$DATA/maps/$1.map" "$DATA/scen/scen-$KIND/$1-$KIND-$2.scen" "$3" --time "$LIMIT" \
  $OPTS > "$PARTDIR/$1-$2.tsv"' sh || exit 1
for map in $MAPS; do
  for s in $(seq 1 "$SCENS"); do cat "$PARTDIR/$map-$s.tsv"; done > "$OUT/guide-$map$suffix.tsv" || exit 1
done
rm -r "$PARTDIR"
