"""Strips a KiCad board of its routing, as guide's routing test does, and exports its DSN.

    python3 -I strip.py board.kicad_pcb stripped.kicad_pcb stripped.dsn [options]

Takes out every track, arc and via, and every copper zone with its fill, teardrops included,
and keeps rule areas (keepouts). The second and later footprints of one reference are
renamed `REF_2`, `REF_3` and so on, which KiCad's Specctra exporter needs and which changes
no copper. Then saves the board, and exports it with KiCad's own `ExportSpecctraDSN`, as
"Export Specctra DSN" does for Freerouting, with the path in its first line cut to the
file's name.

With a fourth argument, writes there the Freerouting options that tell it what the DSN
can't: the board's clearance between copper and its edge, which Freerouting otherwise takes
from its own settings, and no automatic neckdown, which narrows tracks at pads below the
board's minimum track width, which the DSN doesn't carry either.
"""

import os
import sys

import pcbnew

source, stripped, dsn = sys.argv[1:4]
board = pcbnew.LoadBoard(source)
seen = {}
for f in board.GetFootprints():
    ref = f.GetReference()
    seen[ref] = seen.get(ref, 0) + 1
    if seen[ref] > 1:
        f.SetReference("%s_%d" % (ref, seen[ref]))
copper = [z for z in board.Zones() if not z.GetIsRuleArea()]
tracks = list(board.GetTracks())
for item in copper + tracks:
    board.Remove(item)
board.Save(stripped)
board = pcbnew.LoadBoard(stripped)
if not pcbnew.ExportSpecctraDSN(board, dsn):
    sys.exit("ExportSpecctraDSN failed")
with open(dsn) as f:
    lines = f.read().split("\n")
lines[0] = '(pcb "%s"' % os.path.basename(dsn)
with open(dsn, "w") as f:
    f.write("\n".join(lines))
if len(sys.argv) > 4:
    edge = board.GetDesignSettings().m_CopperEdgeClearance / 1000.0
    with open(sys.argv[4], "w") as f:
        f.write(
            "--router.copper_to_edge_clearance_um=%g --router.automatic_neckdown=false\n"
            % edge
        )
