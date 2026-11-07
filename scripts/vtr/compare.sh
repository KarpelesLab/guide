#!/bin/bash
# Compares guide's router with VPR's on the MCNC circuits of VTR's regression tests.
#
#     scripts/vtr/build-vpr.sh DIR                  # once
#     scripts/vtr/compare.sh DIR WORK [CIRCUIT...]  # all 20 circuits by default
#     python3 scripts/vtr/tables.py WORK            # the tables in docs/benchmarks.md
#
# The circuits are the 20 largest MCNC circuits, as VTR's vpr_reg_mcnc_equiv test has them:
# technology-mapped to 6-input LUTs, in vtr_flow/benchmarks/blif/wiremap6, and routed on
# vtr_flow/arch/timing/k6_N10_40nm.xml. They stay in VTR's tree and are never copied here.
# See scripts/vtr/circuit.sh for what is run on each.
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
if [ ${#CIRCUITS[@]} -eq 0 ]; then
    CIRCUITS=(alu4 apex2 apex4 bigkey clma des diffeq dsip elliptic ex1010 ex5p frisc misex3 pdc
        s298 s38417 s38584.1 seq spla tseng)
fi

HERE=$(dirname "$(realpath "$0")")
ROOT=$(dirname "$(dirname "$HERE")")
VPR=$DIR/build/vpr/vpr
BENCH=$DIR/vtr/vtr_flow/benchmarks/blif/wiremap6
ARCH=$DIR/vtr/vtr_flow/arch/timing/k6_N10_40nm.xml
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

for circuit in "${CIRCUITS[@]}"; do
    echo "$circuit"
done | xargs -P "${JOBS:-20}" -I{} \
    "$HERE/circuit.sh" "$VPR" "$WORK/vtr_route" "$ARCH" "$BENCH/{}.pre-vpr.blif" "$WORK/{}"

for circuit in "${CIRCUITS[@]}"; do
    grep -q '^done=yes' "$WORK/$circuit/result.txt" 2> /dev/null || echo "$circuit did not finish" >&2
done
