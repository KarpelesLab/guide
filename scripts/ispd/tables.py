#!/usr/bin/env python3
"""Tables for docs/benchmarks.md from the logs of scripts/ispd/run.sh, against published results.

    python3 scripts/ispd/tables.py WORK [--suite ispd08|ispd07-2d|ispd07-3d]

Published numbers, as their sources print them:

- ISPD 2008 contest: C. Sze, G.-J. Nam, M. Yildiz, "ISPD 2008 Global Routing Contest", ISPD
  2008, final results slides (ispd.cc/slides/ispd2008-files/S7-3.pdf): total overflow, and
  wirelength as eval2008.pl counts it, and wall time in seconds on a 2.8 GHz Opteron 8220 with
  at most 4 threads. NTHU-Route won.
- NCTU-GR 2.0: W.-H. Liu, W.-C. Kao, Y.-L. Li, K.-Y. Chao, IEEE TCAD 32(5), 2013, Tables V and
  VI: SGR, its single-threaded router with one set of parameters, and FastRoute 4.1, run on the
  same 3.0 GHz Xeon: wirelength in units of 1e5 and minutes.
- NTHU-Route 2.0: Y.-J. Chang, Y.-T. Lee, T.-C. Wang, ICCAD 2008, Table III (ISPD 2007, vias
  counted 3 times) and Table IV (ISPD 2008): wirelength in units of 1e5 and minutes, on a
  2.2 GHz Opteron.
"""

import re
import sys
from pathlib import Path

ISPD08 = [
    "adaptec1", "adaptec2", "adaptec3", "adaptec4", "adaptec5",
    "bigblue1", "bigblue2", "bigblue3", "bigblue4",
    "newblue1", "newblue2", "newblue3", "newblue4", "newblue5", "newblue6", "newblue7",
]
ISPD07 = ["adaptec1", "adaptec2", "adaptec3", "adaptec4", "adaptec5",
          "newblue1", "newblue2", "newblue3"]

# The contest's winner, NTHU-Route: (total overflow, wirelength, seconds).
CONTEST = {
    "adaptec1": (0, 5349595, 475), "adaptec2": (0, 5230979, 104),
    "adaptec3": (0, 13111262, 481), "adaptec4": (0, 12172692, 135),
    "adaptec5": (0, 15555389, 1030), "bigblue1": (0, 5631048, 586),
    "bigblue2": (0, 9059102, 594), "bigblue3": (0, 13075301, 259),
    "bigblue4": (182, 23075825, 7533), "newblue1": (0, 4652916, 306),
    "newblue2": (0, 7571354, 67), "newblue3": (31454, 10649470, 7746),
    "newblue4": (152, 12989866, 4023), "newblue5": (0, 23166430, 854),
    "newblue6": (0, 17695878, 818), "newblue7": (68, 35357343, 8433),
}
# The best total overflow of any entrant of the contest.
CONTEST_BEST_TOF = {
    "bigblue4": 182, "newblue3": 31106, "newblue4": 142, "newblue7": 68,
}
# NCTU-GR 2.0's SGR, and FastRoute 4.1 as NCTU-GR 2.0 ran it: (overflow, wirelength e5, min).
SGR = {
    "adaptec1": (0, 53.04, 2.52), "adaptec2": (0, 51.51, 0.65), "adaptec3": (0, 129.24, 3.40),
    "adaptec4": (0, 120.53, 1.23), "adaptec5": (0, 154.01, 6.03), "newblue1": (0, 45.76, 2.37),
    "newblue2": (0, 74.51, 0.64), "newblue5": (0, 228.68, 5.26), "newblue6": (0, 175.49, 4.44),
    "bigblue1": (0, 56.29, 4.09), "bigblue2": (0, 88.66, 4.21), "bigblue3": (0, 129.35, 1.91),
    "newblue3": (31710, 105.36, 143.34), "newblue4": (144, 127.27, 17.33),
    "newblue7": (58, 342.90, 85.67), "bigblue4": (194, 225.00, 60.23),
}
FASTROUTE41 = {
    "adaptec1": (0, 53.73, 3.31), "adaptec2": (0, 52.17, 0.95), "adaptec3": (0, 130.82, 3.69),
    "adaptec4": (0, 121.24, 1.25), "adaptec5": (0, 155.81, 6.70), "newblue1": (0, 46.33, 12.01),
    "newblue2": (0, 75.12, 0.85), "newblue5": (0, 230.94, 9.82), "newblue6": (0, 177.87, 8.78),
    "bigblue1": (0, 56.64, 4.22), "bigblue2": (0, 91.18, 12.12), "bigblue3": (0, 130.04, 2.06),
    "newblue3": (31276, 108.40, 15.99), "newblue4": (136, 130.46, 65.23),
    "newblue7": (54, 353.38, 868.74), "bigblue4": (130, 230.24, 93.25),
}
# NTHU-Route 2.0 on ISPD 2007, vias counted 3 times: (overflow, wirelength e5, minutes).
NTHU07_3D = {
    "adaptec1": (0, 88.81, 10.0), "adaptec2": (0, 90.88, 2.1), "adaptec3": (0, 200.84, 10.9),
    "adaptec4": (0, 185.99, 2.6), "adaptec5": (0, 260.18, 23.0), "newblue1": (0, 90.96, 6.2),
    "newblue2": (0, 134.63, 1.2), "newblue3": (31454, 166.95, 122.4),
}
NTHU07_2D = {
    "adaptec1": (0, 55.91, 9.7), "adaptec2": (0, 54.01, 1.9), "adaptec3": (0, 134.39, 10.3),
    "adaptec4": (0, 127.81, 2.2), "adaptec5": (0, 157.03, 20.5), "newblue1": (0, 48.60, 5.5),
    "newblue2": (0, 78.54, 1.1), "newblue3": (31454, 110.82, 120.9),
}


def read(work, names):
    out = {}
    for name in names:
        log = Path(work) / f"{name}.log"
        err = Path(work) / f"{name}.err"
        if not log.exists() or not log.read_text().strip():
            continue
        fields = dict(re.findall(r"(\w+)=(\S+)", log.read_text()))
        lines = err.read_text().splitlines() if err.exists() else []
        if lines:
            fields.update(re.findall(r"(\w+)=(\S+)", lines[-1]))
        out[name] = fields
    return out


def sep(value):
    """A whole number with its thousands apart."""
    return f"{int(value):,}".replace(",", " ")


def geomean(values):
    product = 1.0
    for v in values:
        product *= v
    return product ** (1.0 / len(values)) if values else float("nan")


def ratio(ours, published):
    """WL over a published WL, when both are free of overflow."""
    tof, wl = ours
    ptof, pwl = published
    if tof == 0 and ptof == 0:
        return wl / pwl
    return None


def ispd08(runs):
    print("| Benchmark | TOF | MOF | WL | Wire | Vias | Iterations | Wall s | CPU s |")
    print("| --- " * 9 + "|")
    for name in ISPD08:
        if name not in runs:
            continue
        r = runs[name]
        print(f"| {name} | {r['tof']} | {r['mof']} | {sep(r['wl'])} | {sep(r['wire'])} "
              f"| {sep(r['vias'])} | {r['iterations']} | {float(r['time']):.0f} "
              f"| {float(r['cpu']):.0f} |")
    print()
    print("| Benchmark | TOF: guide | NTHU-Route (contest) | Best of contest | SGR | FastRoute 4.1 "
          "| WL / NTHU-Route | WL / SGR | WL / FastRoute 4.1 | Seconds: guide, NTHU-Route, SGR |")
    print("| --- " * 10 + "|")
    ratios = {"nthu": [], "sgr": [], "fr": []}
    free = 0
    for name in ISPD08:
        if name not in runs:
            continue
        r = runs[name]
        ours = (int(r["tof"]), int(r["wl"]))
        free += ours[0] == 0
        nthu, sgr, fr = CONTEST[name], SGR[name], FASTROUTE41[name]
        best = CONTEST_BEST_TOF.get(name, 0)
        cells = []
        for key, published in (("nthu", (nthu[0], nthu[1])),
                               ("sgr", (sgr[0], sgr[1] * 1e5)),
                               ("fr", (fr[0], fr[1] * 1e5))):
            q = ratio(ours, published)
            if q is None:
                cells.append("")
            else:
                ratios[key].append(q)
                cells.append(f"{q:.3f}")
        print(f"| {name} | {ours[0]} | {nthu[0]} | {best} | {sgr[0]} | {fr[0]} "
              f"| {cells[0]} | {cells[1]} | {cells[2]} "
              f"| {float(r['time']):.0f}, {nthu[2]}, {sgr[2] * 60:.0f} |")
    print()
    print(f"Overflow-free: {free} of {len(runs)}; NTHU-Route 12, SGR 12, FastRoute 4.1 12")
    for key, label in (("nthu", "NTHU-Route (contest)"), ("sgr", "NCTU-GR 2.0 SGR"),
                       ("fr", "FastRoute 4.1")):
        v = ratios[key]
        if v:
            print(f"Wirelength / {label}, geometric mean over {len(v)} overflow-free on both: "
                  f"{geomean(v):.4f}")


def ispd07(runs, table):
    print("| Benchmark | TOF | MOF | WL | Wire | Vias | Iterations | Wall s | CPU s "
          "| NTHU-Route 2.0: TOF, WL, min | WL / NTHU-Route 2.0 |")
    print("| --- " * 11 + "|")
    ratios = []
    for name in ISPD07:
        if name not in runs:
            continue
        r = runs[name]
        ours = (int(r["tof"]), int(r["wl"]))
        ptof, pwl, minutes = table[name]
        q = ratio(ours, (ptof, pwl * 1e5))
        cell = ""
        if q is not None:
            ratios.append(q)
            cell = f"{q:.3f}"
        iterations = r.get("iterations", "")
        wall = f"{float(r['time']):.0f}" if "time" in r else ""
        cpu = f"{float(r['cpu']):.0f}" if "cpu" in r else ""
        print(f"| {name} | {r['tof']} | {r['mof']} | {sep(r['wl'])} | {sep(r['wire'])} "
              f"| {sep(r['vias'])} | {iterations} | {wall} | {cpu} | {ptof}, {pwl:.2f}e5, {minutes} | {cell} |")
    print()
    if ratios:
        print(f"Wirelength / NTHU-Route 2.0, geometric mean over {len(ratios)}: "
              f"{geomean(ratios):.4f}")


def main():
    args = sys.argv[1:]
    suite = "ispd08"
    if "--suite" in args:
        i = args.index("--suite")
        suite = args[i + 1]
        del args[i:i + 2]
    work = args[0]
    if suite == "ispd08":
        ispd08(read(work, ISPD08))
    elif suite == "ispd07-2d":
        ispd07(read(work, ISPD07), NTHU07_2D)
    else:
        ispd07(read(work, ISPD07), NTHU07_3D)


if __name__ == "__main__":
    main()
