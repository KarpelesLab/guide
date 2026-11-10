#!/bin/bash
# Builds VPR alone, from VTR at the release that docs/benchmarks.md was measured with.
#
#     scripts/vtr/build-vpr.sh DIR
#     SYNTH=1 scripts/vtr/build-vpr.sh DIR   # Odin II and ABC too, for scripts/vtr/synth.sh
#
# Clones VTR into DIR/vtr, builds into DIR/build, and leaves VPR at DIR/build/vpr/vpr.
# Nothing is installed. Graphics, the server mode, Cap'n Proto, FPGA Interchange and the
# analytic placer are all left out. So are the synthesis tools, unless SYNTH=1: the MCNC
# benchmarks start from VTR's technology-mapped BLIF files, which only need VPR. VTR's Verilog
# benchmarks need Odin II and ABC, which SYNTH=1 builds, linking DIR/vtr/build to DIR/build
# for VTR's own flow scripts to find them. Yosys is still left out.
#
# Needs git, cmake, ninja, a C++17 compiler, bison, flex, zlib and TBB (optional).

set -u

TAG=v9.0.0
COMMIT=8cb20aa5293b5ce5f6a05c88b37b38e1305d9b07

if [ $# -ne 1 ]; then
    echo "usage: $0 DIR" >&2
    exit 2
fi
DIR=$1
mkdir -p "$DIR" || exit 1

if [ ! -d "$DIR/vtr/.git" ]; then
    git clone --depth 1 --branch "$TAG" \
        https://github.com/verilog-to-routing/vtr-verilog-to-routing.git "$DIR/vtr" || exit 1
fi
HEAD=$(git -C "$DIR/vtr" rev-parse HEAD) || exit 1
if [ "$HEAD" != "$COMMIT" ]; then
    echo "$DIR/vtr is at $HEAD, not $COMMIT ($TAG)" >&2
    exit 1
fi
# VTR's build always wants Catch2, for its unit tests.
git -C "$DIR/vtr" submodule update --init --depth 1 libs/EXTERNAL/libcatch2 || exit 1

WITH_SYNTH=OFF
TARGETS=
if [ "${SYNTH:-0}" = 1 ]; then
    WITH_SYNTH=ON
    TARGETS="odin_ii abc"
fi

# GCC 15 no longer pulls these headers in through others, and VTR 9.0.0 relies on it.
INCLUDES="-include sstream -include cstdint -include algorithm -include limits -include cstring -include array -include memory"

cmake -S "$DIR/vtr" -B "$DIR/build" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    "-DCMAKE_CXX_FLAGS=$INCLUDES" \
    -DVPR_USE_EZGL=off \
    -DVPR_USE_SERVER=OFF \
    -DVTR_ENABLE_CAPNPROTO=OFF \
    -DVPR_ENABLE_INTERCHANGE=OFF \
    -DVPR_ANALYTIC_PLACE=OFF \
    -DWITH_ABC="$WITH_SYNTH" \
    -DWITH_ODIN="$WITH_SYNTH" \
    -DWITH_PARMYS=OFF \
    -DVTR_IPO_BUILD=off &&
    ninja -C "$DIR/build" vpr $TARGETS &&
    echo "VPR is at $DIR/build/vpr/vpr" || exit 1

if [ "$WITH_SYNTH" = ON ]; then
    # VTR's tree links to DIR/vtr/build/{vpr,odin_ii,abc}, which its flow scripts run.
    [ -e "$DIR/vtr/build" ] || ln -s ../build "$DIR/vtr/build" || exit 1
    echo "Odin II and ABC are in $DIR/build"
fi
