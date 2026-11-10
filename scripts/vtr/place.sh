#!/bin/bash
# Compares guide's placer with VPR's on VTR's benchmarks, routing both placements with both
# routers.
#
#     scripts/vtr/build-vpr.sh DIR                       # once
#     scripts/vtr/place.sh DIR WORK [CIRCUIT...]         # all 20 MCNC circuits by default
#     SUITE=verilog BLIF=BLIF scripts/vtr/place.sh DIR WORK [CIRCUIT...]
#     python3 scripts/vtr/place_tables.py WORK...        # the tables in docs/benchmarks.md
#
# SUITE and BLIF are as for scripts/vtr/compare.sh, which says what the suites are. FROM=WORK
# takes VPR's packing and placement of each circuit from an earlier run of compare.sh or of
# this script, which saves packing and placing again: VPR's are the same for a seed.
#
# Circuits run in parallel, $JOBS at a time (default 20), each run of VPR and of guide on one
# thread. PLACE_ARGS passes options to every run of vtr_place. See
# scripts/vtr/place-circuit.sh for what is run on each circuit.

set -u

if [ $# -lt 2 ]; then
    echo "usage: $0 DIR WORK [CIRCUIT...]" >&2
    exit 2
fi
DIR=$(realpath "$1")
WORK=$2
shift 2
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
if [ -n "${FROM:-}" ]; then
    FROM=$(realpath "$FROM")
    export FROM
fi

cargo build --release --features fpga --example vtr_place --example vtr_route \
    --manifest-path "$ROOT/Cargo.toml" || exit 1
# Copies, so that rebuilding during a run doesn't change them.
mkdir -p "$WORK" || exit 1
WORK=$(realpath "$WORK")
cp "$ROOT/target/release/examples/vtr_place" "$WORK/vtr_place" || exit 1
cp "$ROOT/target/release/examples/vtr_route" "$WORK/vtr_route" || exit 1
git -C "$ROOT" rev-parse HEAD > "$WORK/guide_commit" 2> /dev/null
git -C "$DIR/vtr" rev-parse HEAD > "$WORK/vtr_commit"
echo "${PLACE_ARGS:-}" > "$WORK/place_args"
echo "$SUITE" > "$WORK/suite"

for circuit in "${CIRCUITS[@]}"; do
    echo "$circuit"
done | xargs -P "${JOBS:-20}" -I{} \
    "$HERE/place-circuit.sh" "$VPR" "$WORK/vtr_place" "$WORK/vtr_route" "$ARCH" \
    "$BENCH/{}.pre-vpr.blif" "$WORK/{}"

for circuit in "${CIRCUITS[@]}"; do
    grep -q '^done=yes' "$WORK/$circuit/result.txt" 2> /dev/null || echo "$circuit did not finish" >&2
done
