#!/bin/bash
# Downloads the ISPD 2007 and 2008 global routing contest benchmarks, and the contests'
# evaluation scripts, from ispd.cc.
#
#     scripts/ispd/fetch.sh DIR
#
# Leaves DIR/bench/*.gr (unzipped) and DIR/eval/ with eval2008.pl and the ISPD 2007 scripts.
# Nothing here is redistributed: the files stay outside of this repository, and are fetched
# from the contests' own pages, which post them for research use without a licence of their
# own:
#
#   http://www.ispd.cc/contests/07/rcontest/     ISPD 2007: adaptec1-5, newblue1-3, 2D and 3D
#   http://www.ispd.cc/contests/08/ispd08rc.html ISPD 2008: the eight 3D ones of 2007, and
#                                                bigblue1-4, newblue4-7
#
# Needs curl, gzip and unzip. Files already there are not fetched again.

set -u

if [ $# -ne 1 ]; then
    echo "usage: $0 DIR" >&2
    exit 2
fi
DIR=$1
mkdir -p "$DIR/bench" "$DIR/eval" || exit 1

ISPD07=http://www.ispd.cc/contests/07/rcontest
ISPD08=http://www.ispd.cc/contests/08

BENCH07=(
    adaptec1.capo70 35.50.90
    adaptec2.mpl60 35.20.100
    adaptec3.dragon70 30.50.90
    adaptec4.aplace60 30.50.90
    adaptec5.mfar50 50.20.100
    newblue1.ntup50 30.50.90
    newblue2.fastplace90 50.20.100
    newblue3.kraftwerk80 40.50.90
)
BENCH08=(
    bigblue1.capo60.3d.50.10.100
    bigblue2.mpl60.3d.40.60.60
    bigblue3.aplace70.3d.50.10.90.m8
    bigblue4.fastplace70.3d.80.20.80
    newblue4.mpl50.3d.40.10.95
    newblue5.ntup50.3d.40.10.100
    newblue6.mfar80.3d.60.10.100
    newblue7.kraftwerk70.3d.80.20.82.m8
)

fetch() {
    local url=$1 out=$2
    if [ -s "$out" ]; then
        return 0
    fi
    curl -fsSL --retry 3 -o "$out.part" "$url" && mv "$out.part" "$out"
}

fetch_gr() {
    local url=$1 name=$2
    local gr="$DIR/bench/$name.gr"
    if [ -s "$gr" ]; then
        return 0
    fi
    fetch "$url" "$gr.gz" && gzip -dc "$gr.gz" > "$gr.part" && mv "$gr.part" "$gr" &&
        rm -f "$gr.gz"
}

status=0
for ((i = 0; i < ${#BENCH07[@]}; i += 2)); do
    base=${BENCH07[i]}
    suffix=${BENCH07[i + 1]}
    for dim in 2d 3d; do
        fetch_gr "$ISPD07/benchmark/$base.$dim.$suffix.gr.gz" "$base.$dim.$suffix" || status=1
    done
done
for name in "${BENCH08[@]}"; do
    fetch_gr "$ISPD08/benchmark/$name.gr.gz" "$name" || status=1
done

# The ISPD 2008 evaluation script, and the 2007 ones.
fetch "$ISPD08/eval2008.zip" "$DIR/eval/eval2008.zip" || status=1
if [ -s "$DIR/eval/eval2008.zip" ] && [ ! -e "$DIR/eval/eval2008.pl" ]; then
    unzip -o -q "$DIR/eval/eval2008.zip" -d "$DIR/eval" || status=1
fi
for f in grc_eval grc.pm; do
    if [ ! -s "$DIR/eval/$f" ]; then
        fetch "$ISPD07/$f.gz" "$DIR/eval/$f.gz" && gzip -df "$DIR/eval/$f.gz" || status=1
    fi
done

ls -l "$DIR/bench"
exit $status
