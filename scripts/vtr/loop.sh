#!/bin/bash
# Measures placement and routing in a loop against the one-pass flow on VTR's benchmarks.
#
#     scripts/vtr/build-vpr.sh DIR                              # once
#     scripts/vtr/place.sh DIR BASE [CIRCUIT...]                # the one-pass flow
#     scripts/vtr/loop.sh DIR BASE WORK [CIRCUIT...]            # the loop, from BASE
#     SUITE=verilog BLIF=BLIF scripts/vtr/loop.sh DIR BASE WORK [CIRCUIT...]
#     python3 scripts/vtr/loop_tables.py BASE WORK...           # the tables in docs/benchmarks.md
#
# BASE is the WORK of a run of scripts/vtr/place.sh on the same suite, whose placement of each
# circuit the loop starts from, below the narrowest channel guide's router routed it in. SUITE
# and BLIF are as for place.sh, and the circuits are all of the suite's by default.
#
# Circuits run in parallel, $JOBS at a time (default 20), each run of VPR and of guide on one
# thread. LOOP_ARGS passes options to every run of vtr_loop. See scripts/vtr/loop-circuit.sh
# for what is run on each circuit.

set -u

if [ $# -lt 3 ]; then
    echo "usage: $0 DIR BASE WORK [CIRCUIT...]" >&2
    exit 2
fi
DIR=$(realpath "$1")
BASE=$(realpath "$2")
WORK=$3
shift 3
CIRCUITS=("$@")
SUITE=${SUITE:-mcnc}
case "$SUITE" in
mcnc)
    BENCH=$DIR/vtr/vtr_flow/benchmarks/blif/wiremap6
    ARCH=$DIR/vtr/vtr_flow/arch/timing/k6_N10_40nm.xml
    if [ ${#CIRCUITS[@]} -eq 0 ]; then
        CIRCUITS=(alu4 apex2 apex4 bigkey clma des diffeq dsip elliptic ex1010 ex5p frisc misex3
            pdc s298 s38417 s38584.1 seq spla tseng)
    fi
    ;;
verilog)
    [ -n "${BLIF:-}" ] || {
        echo "SUITE=verilog needs BLIF, the output directory of scripts/vtr/synth.sh" >&2
        exit 2
    }
    BENCH=$(realpath "$BLIF")
    ARCH=$DIR/vtr/vtr_flow/arch/timing/k6_frac_N10_frac_chain_mem32K_40nm.xml
    if [ ${#CIRCUITS[@]} -eq 0 ]; then
        CIRCUITS=(arm_core bgm blob_merge boundtop ch_intrinsics diffeq1 diffeq2 mkDelayWorker32B
            mkPktMerge mkSMAdapter4B or1200 raygentop sha spree stereovision0 stereovision1
            stereovision2 stereovision3 LU8PEEng LU32PEEng mcml)
    fi
    ;;
*)
    echo "SUITE is mcnc or verilog, not $SUITE" >&2
    exit 2
    ;;
esac

HERE=$(dirname "$(realpath "$0")")
ROOT=$(dirname "$(dirname "$HERE")")
VPR=$DIR/build/vpr/vpr
[ -x "$VPR" ] || {
    echo "no VPR at $VPR: run scripts/vtr/build-vpr.sh $DIR" >&2
    exit 1
}

cargo build --release --features fpga --example vtr_loop --example vtr_place --example vtr_route \
    --manifest-path "$ROOT/Cargo.toml" || exit 1
mkdir -p "$WORK" || exit 1
WORK=$(realpath "$WORK")
for example in vtr_loop vtr_place vtr_route; do
    cp "$ROOT/target/release/examples/$example" "$WORK/$example" || exit 1
done
git -C "$ROOT" rev-parse HEAD > "$WORK/guide_commit" 2> /dev/null
echo "${LOOP_ARGS:-}" > "$WORK/loop_args"
echo "$SUITE" > "$WORK/suite"
echo "$BASE" > "$WORK/base"
export LOOP_ARGS="${LOOP_ARGS:-}"

for circuit in "${CIRCUITS[@]}"; do
    echo "$circuit"
done | xargs -P "${JOBS:-20}" -I{} \
    "$HERE/loop-circuit.sh" "$VPR" "$WORK/vtr_loop" "$WORK/vtr_place" "$WORK/vtr_route" \
    "$ARCH" "$BENCH/{}.pre-vpr.blif" "$BASE/{}" "$WORK/{}"

for circuit in "${CIRCUITS[@]}"; do
    grep -q '^done=yes' "$WORK/$circuit/result.txt" 2> /dev/null || echo "$circuit did not finish" >&2
done
