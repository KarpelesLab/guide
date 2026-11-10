#!/bin/sh
# Runs one curated-board benchmark of src/kicad for each board, all at once, each in its own
# process, and prints their rows in the order given. The first argument is the test's
# environment switch, such as GUIDE_LOOP, the second the test's name, and the rest the
# boards. Output goes to the directory in GUIDE_OUT, by default target/boards.
# For example: scripts/loop-boards.sh GUIDE_LOOP curated_boards_loop TS4231-BOB lisn
switch=$1
test=$2
shift 2
out=${GUIDE_OUT:-target/boards}
mkdir -p "$out"
cargo test --release --features kicad --lib --no-run >/dev/null 2>&1 || exit 1
for board in "$@"; do
    env "$switch=1" GUIDE_BOARDS="$board" cargo test --release --features kicad --lib "$test" \
        -- --nocapture >"$out/$board.log" 2>&1 &
done
wait
for board in "$@"; do
    grep "^| \`" "$out/$board.log" || echo "$board: failed, see $out/$board.log"
done
