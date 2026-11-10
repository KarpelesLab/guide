#!/bin/bash
# Compares guide's router with VPR's on VTR's benchmarks.
#
#     scripts/vtr/build-vpr.sh DIR                  # once
#     scripts/vtr/compare.sh DIR WORK [CIRCUIT...]  # all 20 MCNC circuits by default
#     python3 scripts/vtr/tables.py WORK            # the tables in docs/benchmarks.md
#
#     SYNTH=1 scripts/vtr/build-vpr.sh DIR          # once, for the Verilog suite
#     scripts/vtr/synth.sh DIR BLIF                 # once
#     SUITE=verilog BLIF=BLIF scripts/vtr/compare.sh DIR WORK [CIRCUIT...]
#
# SUITE=mcnc, the default: the 20 largest MCNC circuits, as VTR's vpr_reg_mcnc_equiv test has
# them, technology-mapped to 6-input LUTs, in vtr_flow/benchmarks/blif/wiremap6, and routed on
# vtr_flow/arch/timing/k6_N10_40nm.xml.
#
# SUITE=verilog: VTR's Verilog benchmarks, the 21 circuits of its vtr_reg_qor_chain test,
# synthesized by scripts/vtr/synth.sh into $BLIF, and routed on
# vtr_flow/arch/timing/k6_frac_N10_frac_chain_mem32K_40nm.xml, VTR's flagship architecture,
# with fracturable LUTs, carry chains, memories and multipliers.
#
# The circuits stay outside of this repository. See scripts/vtr/circuit.sh for what is run on
# each.
#
# Circuits run in parallel, $JOBS at a time (default 20), each VPR run on one thread. Each
# circuit's guide runs are single-threaded but for one, which takes $GUIDE_THREADS (default
# 16), so runtimes are only comparable on a machine with cores to spare. GUIDE_ARGS passes
# more options to every run of vtr_route.

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

cargo build --release --features fpga --example vtr_route --manifest-path "$ROOT/Cargo.toml" ||
    exit 1
# A copy, so that rebuilding during a run doesn't change it.
mkdir -p "$WORK" || exit 1
WORK=$(realpath "$WORK")
cp "$ROOT/target/release/examples/vtr_route" "$WORK/vtr_route" || exit 1
git -C "$ROOT" rev-parse HEAD > "$WORK/guide_commit" 2> /dev/null
git -C "$DIR/vtr" rev-parse HEAD > "$WORK/vtr_commit"
echo "${GUIDE_ARGS:-}" > "$WORK/guide_args"
echo "$SUITE" > "$WORK/suite"

for circuit in "${CIRCUITS[@]}"; do
    echo "$circuit"
done | xargs -P "${JOBS:-20}" -I{} \
    "$HERE/circuit.sh" "$VPR" "$WORK/vtr_route" "$ARCH" "$BENCH/{}.pre-vpr.blif" "$WORK/{}"

for circuit in "${CIRCUITS[@]}"; do
    grep -q '^done=yes' "$WORK/$circuit/result.txt" 2> /dev/null || echo "$circuit did not finish" >&2
done
