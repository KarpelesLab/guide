"""Imports a Specctra session into a KiCad board, as "Import Specctra Session" does.

    python3 -I import_ses.py board.kicad_pcb session.ses routed.kicad_pcb

KiCad's own `ImportSpecctraSES` replaces the board's tracks and vias with the session's.
"""

import sys

import pcbnew

board = pcbnew.LoadBoard(sys.argv[1])
if not pcbnew.ImportSpecctraSES(board, sys.argv[2]):
    sys.exit("ImportSpecctraSES failed")
board.Save(sys.argv[3])
