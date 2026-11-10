#!/bin/bash
# Synthesizes VTR's Verilog benchmarks for scripts/vtr/compare.sh, with VTR's own flow.
#
#     SYNTH=1 scripts/vtr/build-vpr.sh DIR      # once, for Odin II and ABC
#     scripts/vtr/synth.sh DIR OUT [CIRCUIT...]  # the 21 of vtr_reg_qor_chain by default
#
# Runs VTR's run_vtr_flow.py from Odin II to ABC, as VTR's vtr_reg_qor_chain test does
# (vtr_reg_nightly_test3_odin), for the architecture that test uses,
# k6_frac_N10_frac_chain_mem32K_40nm.xml, and leaves OUT/CIRCUIT.pre-vpr.blif for each, which
# is what that flow gives VPR. Logs and intermediate files go to OUT/CIRCUIT/. The circuits
# stay in VTR's tree, and what this makes of them is never copied into this repository.
#
# VTR's flow scripts import prettytable, which has to be installed for python3 or on its
# PYTHONPATH. Circuits run in parallel, $JOBS at a time (default 21). The largest, mcml and
# LU32PEEng, take about an hour each in Odin II.

set -u

if [ $# -lt 2 ]; then
    echo "usage: $0 DIR OUT [CIRCUIT...]" >&2
    exit 2
fi
DIR=$(realpath "$1")
OUT=$2
shift 2
CIRCUITS=("$@")
if [ ${#CIRCUITS[@]} -eq 0 ]; then
    CIRCUITS=(arm_core bgm blob_merge boundtop ch_intrinsics diffeq1 diffeq2 mkDelayWorker32B
        mkPktMerge mkSMAdapter4B or1200 raygentop sha spree stereovision0 stereovision1
        stereovision2 stereovision3 LU8PEEng LU32PEEng mcml)
fi
FLOW=$DIR/vtr/vtr_flow
ARCH=$FLOW/arch/timing/k6_frac_N10_frac_chain_mem32K_40nm.xml
[ -x "$DIR/vtr/odin_ii/odin_ii" ] && [ -x "$DIR/vtr/abc/abc" ] || {
    echo "no Odin II or ABC in $DIR: run SYNTH=1 scripts/vtr/build-vpr.sh $DIR" >&2
    exit 1
}
mkdir -p "$OUT" || exit 1
OUT=$(realpath "$OUT")

one() {
    local circuit=$1
    rm -rf "${OUT:?}/$circuit"
    if python3 "$FLOW/scripts/run_vtr_flow.py" "$FLOW/benchmarks/verilog/$circuit.v" "$ARCH" \
        -start odin -ending_stage abc -temp_dir "$OUT/$circuit" > "$OUT/$circuit.log" 2>&1 &&
        cp "$OUT/$circuit/$circuit.abc.blif" "$OUT/$circuit.pre-vpr.blif"; then
        echo "$circuit: done"
    else
        echo "$circuit: failed, see $OUT/$circuit.log" >&2
    fi
}
export -f one
export FLOW ARCH OUT

for circuit in "${CIRCUITS[@]}"; do
    echo "$circuit"
done | xargs -P "${JOBS:-21}" -I{} bash -c 'one "$1"' _ {}
