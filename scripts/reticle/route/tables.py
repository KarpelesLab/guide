#!/usr/bin/env python3
"""Tables of run.sh's results, in Markdown.

    tables.py <out dir> [guide router, default guide] [threaded router, default guide@8]
"""

import json
import math
import os
import sys


def load(out):
    rows = {}
    for name in sorted(os.listdir(out)):
        if not name.endswith(".json"):
            continue
        for line in open(os.path.join(out, name)):
            line = line.strip()
            if line:
                d = json.loads(line)
                rows.setdefault(d["design"], {})[d["router"]] = d
    return rows


def critical(d):
    """The latest path: the slowest clock domain's, in ns, or the unit-delay count."""
    c = d.get("critical") or {}
    if "units" in c:
        return c["units"], "units"
    timed = [v for v in c.values() if v > 0]
    return (max(timed) / 1000.0, "ns") if timed else (None, None)


def geomean(xs):
    xs = [x for x in xs if x and x > 0]
    return math.exp(sum(math.log(x) for x in xs) / len(xs)) if xs else float("nan")


def main():
    out = sys.argv[1]
    g = sys.argv[2] if len(sys.argv) > 2 else "guide"
    t = sys.argv[3] if len(sys.argv) > 3 else "guide@8"
    rows = load(out)
    order = ["xc7a35t-cpg236", "ecp5-12f-CABGA256", "ice40-hx8k-ct256", "gw2a-18-pg256"]
    designs = sorted(rows, key=lambda n: (order.index(next(iter(rows[n].values()))["device"])
                                          if next(iter(rows[n].values()))["device"] in order else 9,
                                          -next(iter(rows[n].values()))["signals"]))
    print(f"| Design | Device | Signals | Graph nodes | Legal (reticle / guide) "
          f"| Time, s (reticle / guide / {t}) | Iterations | Programmable pips "
          f"| Latest path | Peak memory, MiB |")
    print("| --- | --- | ---: | ---: | --- | --- | --- | --- | --- | --- |")
    ratios = {"time": [], "time_t": [], "pips": [], "crit": [], "mem": []}
    for name in designs:
        r = rows[name].get("reticle")
        gd = rows[name].get(g)
        td = rows[name].get(t)
        if not r or not gd:
            continue
        legal = lambda d: "yes" if d and d["legal"] else ("**no**" if d else "-")
        sec = lambda d: f"{d['seconds']:.2f}" if d and "seconds" in d else "-"
        both = r["legal"] and gd["legal"]
        cr, unit = critical(r)
        cg, _ = critical(gd)
        crit = "-"
        if cr is not None and cg is not None:
            crit = f"{cr:g} / {cg:g} {unit}" if unit == "units" else f"{cr:.2f} / {cg:.2f} ns"
        mem = "-"
        if "rss_peak_kib" in r and "rss_peak_kib" in gd:
            mr = (r["rss_peak_kib"] - r["rss_before_kib"]) / 1024
            mg = (gd["rss_peak_kib"] - gd["rss_before_kib"]) / 1024
            mem = f"{mr:.0f} / {mg:.0f}"
            if both and mr > 1 and mg > 1:
                ratios["mem"].append(mg / mr)
        pips = "-"
        if "programmable" in r and "programmable" in gd:
            pips = f"{r['programmable']} / {gd['programmable']}"
        its = f"{r.get('iterations', '-')} / {gd.get('iterations', '-')}"
        print(f"| {name} | {r['device']} | {r['signals']} | {r['graph_nodes']} "
              f"| {legal(r)} / {legal(gd)} | {sec(r)} / {sec(gd)} / {sec(td)} | {its} "
              f"| {pips} | {crit} | {mem} |")
        if both and r["signals"] >= 100:
            ratios["time"].append(gd["seconds"] / max(r["seconds"], 1e-3))
            if td and td.get("legal"):
                ratios["time_t"].append(td["seconds"] / max(r["seconds"], 1e-3))
            if r["programmable"]:
                ratios["pips"].append(gd["programmable"] / r["programmable"])
            if cr and cg:
                ratios["crit"].append(cg / cr)
    print()
    print("Geometric means of guide against reticle, over the designs of 100 signals or more "
          "that both route legally:")
    print()
    for key, label in [("time", f"time ({g})"), ("time_t", f"time ({t})"),
                       ("pips", "programmable pips"), ("crit", "latest path"),
                       ("mem", "peak memory of routing")]:
        print(f"- {label}: {geomean(ratios[key]):.3f} over {len(ratios[key])} design(s)")


main()
