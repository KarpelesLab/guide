#!/usr/bin/env python3
"""Tables for docs/benchmarks.md from reticle-place's output lines.

usage: tables.py [--times-seed N] RESULTS...
    (files of lines `design=... key=value ...`, as runs print; a later line for the same
    design, placer and seed replaces an earlier one)

Per design: runs that passed every check out of runs made, for each placer; and over the runs
where both passed, the mean placement time, reticle's wirelength, pips and route time, with
guide's as a ratio of reticle's, then geometric means of the ratios over the designs. With
--times-seed, times are of that seed's runs alone, for results gathered under different loads.
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
    args = sys.argv[1:]
    times_seed = None
    if args[:1] == ["--times-seed"]:
        times_seed, args = args[1], args[2:]
    latest = {}
    order = []
    for r in runs(args):
        if r["design"] not in order:
            order.append(r["design"])
        latest[(r["design"], r["placer"], r.get("seed"))] = r
    by = defaultdict(lambda: defaultdict(list))
    for (d, p, _), r in latest.items():
        by[d][p].append(r)
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
                timed = key.endswith("_s") and times_seed is not None
                pick = lambda rs: [r for r in rs if not timed or r.get("seed") == times_seed]
                a = mean([float(r[key]) for r in pick(ro)])
                b = mean([float(r[key]) for r in pick(go)])
                if key.endswith("_s"):
                    row += [f"{a:.1f}", f"{b:.1f}"]
                else:
                    row += [f"{a:.0f}", f"{b:.0f} ({b / a:.3f})" if a else f"{b:.0f}"]
                if a > 0 and b > 0 and not math.isnan(a) and not math.isnan(b):
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
