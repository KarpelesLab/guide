#!/bin/bash
# Routes the nine curated boards with Freerouting and with guide, from the same stripped
# boards, and prints the table of docs/benchmarks.md, "PCB routing against Freerouting".
#
#     JAVA=/path/to/jre/bin/java FREEROUTING_JAR=/path/to/freerouting-2.5.0.jar \
#         scripts/freerouting/compare.sh OUT_DIR
#
# Needs Java 25 or later, Freerouting 2.5.0's jar, KiCad 10.0.3's kicad-cli and its pcbnew
# module for python3, and cargo. Nothing is installed: Freerouting's settings and logs go to
# OUT_DIR/home, not to the user's.
#
# For each board, OUT_DIR gets:
#   <board>.kicad_pcb, .kicad_pro, .dsn   the stripped board, and KiCad's DSN export of it
#   <board>.tuned-options                 Freerouting's options for the board's `tuned` run
#   <board>.<run>.ses, .log               Freerouting's session and log, for each run
#   <board>.<run>.kicad_pcb               the session imported into the stripped board
#   <board>.guide.kicad_pcb               guide's routing of the stripped board
#   *.drc.json                            kicad-cli's DRC report on each routed board
#
# RUNS limits the runs, by name, for example RUNS="default tuned". With KEEP=1, a run whose
# routed board is already in OUT_DIR is not run again.
set -u

out=${1:?usage: compare.sh OUT_DIR}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
java=${JAVA:-java}
jar=${FREEROUTING_JAR:?set FREEROUTING_JAR to freerouting-2.5.0.jar}
python=${PYTHON:-python3}

boards="gekkio/GB-BRK-LINK-A dotbots/TS4231-BOB tem-cell/lisn starshipraider/isi-channel
placebo/placebo udb-s/udb-s bugs/flea gekkio/GB-LIVE32 nitrokey/nk3-nrf-mini"

# Freerouting's runs: a name, and its options. `-mt` doesn't reach the optimizer in 2.5.0,
# so its thread count is set by its setting. The autorouter itself is single-threaded, and
# stops once its score hasn't improved for 10 passes, whatever max_passes is, so a longer
# run is the optimizer's: it goes on while a pass improves at all, not by 2.5%. `tuned`
# adds what the DSN doesn't say, as strip.py writes it for each board: the board's edge
# clearance, and no neckdown below its minimum track width.
declare -A options=(
    [default]=""
    [1-thread]="--router.optimizer.max_threads=1"
    [long]="--router.optimizer.max_threads=1 --router.optimizer.improvement_threshold=0"
    [tuned]="--router.optimizer.max_threads=1"
)
runs=${RUNS:-default 1-thread long tuned}

mkdir -p "$out/home" || exit 1
out=$(cd "$out" && pwd)
: > "$out/runs"
for run in $runs; do
    [ -n "${options[$run]+set}" ] || { echo "unknown run: $run" >&2; exit 1; }
    echo "$run" >> "$out/runs"
done

for path in $boards; do
    board=${path#*/}
    echo "== $board" >&2
    # KiCad's Python prints debug noise and leaks warnings to stderr; keep them in a log.
    "$python" -I "$here/strip.py" "$repo/testdata/kicad/$path.kicad_pcb" \
        "$out/$board.kicad_pcb" "$out/$board.dsn" "$out/$board.tuned-options" \
        > "$out/$board.strip.log" 2>&1 &&
        cp "$repo/testdata/kicad/$path.kicad_pro" "$out/$board.kicad_pro" ||
        { echo "$board: stripping failed, see $out/$board.strip.log" >&2; exit 1; }
    for run in $runs; do
        if [ "${KEEP:-}" = 1 ] && [ -e "$out/$board.$run.kicad_pcb" ]; then
            continue
        fi
        extra=${options[$run]}
        [ "$run" = tuned ] && extra="$extra $(cat "$out/$board.tuned-options")"
        # shellcheck disable=SC2086 # the options are words
        HOME="$out/home" XDG_CONFIG_HOME="$out/home/.config" XDG_DATA_HOME="$out/home/.data" \
            XDG_STATE_HOME="$out/home/.state" \
            "$java" -Duser.home="$out/home" -Djava.awt.headless=true -jar "$jar" \
            -de "$out/$board.dsn" -do "$out/$board.$run.ses" --gui.enabled=false \
            --usage_and_diagnostic_data.disable_analytics=true $extra \
            > "$out/$board.$run.log" 2>&1 ||
            { echo "$board, $run: Freerouting failed, see $out/$board.$run.log" >&2; continue; }
        echo "$run: $(grep -o "elapsed: [^,]*" "$out/$board.$run.log")" >&2
        "$python" -I "$here/import_ses.py" "$out/$board.kicad_pcb" "$out/$board.$run.ses" \
            "$out/$board.$run.kicad_pcb" > "$out/$board.$run.import.log" 2>&1 ||
            echo "$board, $run: importing failed, see $out/$board.$run.import.log" >&2
    done
done

cd "$repo" &&
    GUIDE_FREEROUTING="$out" cargo test --release --features kicad \
        curated_boards_against_freerouting -- --nocapture
