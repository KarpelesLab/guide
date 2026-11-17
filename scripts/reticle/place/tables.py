#!/usr/bin/env python3
"""Tables for docs/benchmarks.md from reticle-place's output lines.

usage: tables.py RESULTS...   (files of lines `design=... key=value ...`, as suite runs print)

Per design: runs that passed every check out of runs made, for each placer; and over the runs
where both passed, the mean placement time, reticle's wirelength, pips and route time, with
guide's as a ratio of reticle's, then geometric means of the ratios over the designs.
"""
import math
import sys
from collections import defaultdict


def runs(paths):
    for path in paths:
        for line in open(path):
            fields = dict(f.split("=", 1) for f in line.split() if "=" in f)
            if "design" in fields and "placer" in fields:
                yield fields


def mean(xs):
    return sum(xs) / len(xs) if xs else float("nan")


def main():
    by = defaultdict(lambda: defaultdict(list))
    order = []
    for r in runs(sys.argv[1:]):
        if r["design"] not in order:
            order.append(r["design"])
        by[r["design"]][r["placer"]].append(r)
    print("| design | cells | passed, reticle | passed, guide | place s, reticle | guide | "
          "wirelength, reticle | guide | pips, reticle | guide | route s, reticle | guide |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    ratios = defaultdict(list)
    for d in order:
        rt, gd = by[d].get("reticle", []), by[d].get("guide", [])
        ok = lambda rs: [r for r in rs if r.get("ok") == "yes"]
        ro, go = ok(rt), ok(gd)
        cells = next((r["cells"] for r in rt + gd if "cells" in r), "")
        row = [d, cells, f"{len(ro)}/{len(rt)}", f"{len(go)}/{len(gd)}"]
        if ro and go:
            for key in ("place_s", "hpwl", "pips", "route_s"):
                a = mean([float(r[key]) for r in ro])
                b = mean([float(r[key]) for r in go])
                if key.endswith("_s"):
                    row += [f"{a:.1f}", f"{b:.1f}"]
                else:
                    row += [f"{a:.0f}", f"{b:.0f} ({b / a:.3f})" if a else f"{b:.0f}"]
                if a > 0 and b > 0:
                    ratios[key].append(b / a)
        else:
            row += ["-"] * 8
        print("| " + " | ".join(row) + " |")
    print()
    for key, rs in ratios.items():
        g = math.exp(sum(math.log(x) for x in rs) / len(rs))
        print(f"geometric mean of guide / reticle, {key}: {g:.3f} over {len(rs)} designs")


if __name__ == "__main__":
    main()
