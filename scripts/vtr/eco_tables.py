#!/usr/bin/env python3
"""The tables of engineering changes in docs/benchmarks.md, from the output of scripts/vtr/eco.sh.

    python3 scripts/vtr/eco_tables.py OUT

Per circuit, means over seeds of what changed, and geometric means over seeds of incremental
against scratch for wire, delay, work and time.
"""
import glob
import math
import sys
from collections import defaultdict

rows = []
for path in sorted(glob.glob(sys.argv[1] + "/*.txt")):
    for line in open(path):
        fields = dict(kv.split("=", 1) for kv in line.split() if "=" in kv)
        if fields:
            rows.append(fields)

ORDER = ["tseng", "clma", "ex1010", "ch_intrinsics", "diffeq1", "blob_merge", "or1200",
         "stereovision2", "bgm", "LU8PEEng"]
seen = sorted({r["circuit"] for r in rows})
circuits = [c for c in ORDER if c in seen] + [c for c in seen if c not in ORDER]


def gmean(values):
    values = [v for v in values if v > 0]
    return math.exp(sum(math.log(v) for v in values) / len(values)) if values else float("nan")


def pick(change, percent, method):
    out = defaultdict(list)
    for r in rows:
        if r["change"] == change and r["percent"] == percent and r["method"] == method:
            out[r["circuit"]].append(r)
    return out


def route_table(change):
    print(f"\n### {change}\n")
    print("| Circuit | Change | Unchanged: scratch | Unchanged: incremental | Nodes changed: scratch | Nodes changed: incremental | Wire | Critical path | Expansions | Time | Legal |")
    print("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for percent in ["1", "5"]:
        s, i = pick(change, percent, "scratch"), pick(change, percent, "incremental")
        agg = defaultdict(list)
        for c in circuits:
            if c not in s or c not in i:
                continue
            ss = sorted(s[c], key=lambda r: r["seed"])
            ii = sorted(i[c], key=lambda r: r["seed"])
            us = sum(float(r["unchanged"]) for r in ss) / len(ss)
            ui = sum(float(r["unchanged"]) for r in ii) / len(ii)
            ns = sum(float(r["nodes_changed"]) for r in ss) / len(ss)
            ni = sum(float(r["nodes_changed"]) for r in ii) / len(ii)
            wire = gmean([float(b["wirelength"]) / float(a["wirelength"]) for a, b in zip(ss, ii)])
            cpd = gmean([float(b["critical_path_ps"]) / float(a["critical_path_ps"]) for a, b in zip(ss, ii)])
            exp = gmean([max(float(b["expansions"]), 1) / float(a["expansions"]) for a, b in zip(ss, ii)])
            t = gmean([max(float(b["route_s"]), 1e-3) / max(float(a["route_s"]), 1e-3) for a, b in zip(ss, ii)])
            legal = f"{sum(r['legal'] == 'true' for r in ss)}/{sum(r['legal'] == 'true' for r in ii)}"
            for k, v in [("us", us), ("ui", ui), ("ns", ns), ("ni", ni), ("wire", wire), ("cpd", cpd), ("exp", exp), ("t", t)]:
                agg[k].append(v)
            print(f"| {c} | {percent}% | {100*us:.1f}% | {100*ui:.1f}% | {ns:.3f} | {ni:.3f} | {wire:.3f} | {cpd:.3f} | {exp:.3f} | {t:.3f} | {legal} |")
        if agg:
            m = lambda k: sum(agg[k]) / len(agg[k])
            print(f"| **Mean** | {percent}% | {100*m('us'):.1f}% | {100*m('ui'):.1f}% | {m('ns'):.3f} | {m('ni'):.3f} | {gmean(agg['wire']):.3f} | {gmean(agg['cpd']):.3f} | {gmean(agg['exp']):.3f} | {gmean(agg['t']):.3f} | |")


def place_table():
    print("\n### place_blocks\n")
    print("| Circuit | Added | Moved: scratch | Moved: incremental | Mean displacement: scratch | Mean displacement: incremental | BB estimate | Routed wire | Critical path | Place time | Legal |")
    print("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for percent in ["1", "5"]:
        s, i = pick("place_blocks", percent, "scratch"), pick("place_blocks", percent, "incremental")
        agg = defaultdict(list)
        for c in circuits:
            if c not in s or c not in i:
                continue
            ss = sorted(s[c], key=lambda r: r["seed"])
            ii = sorted(i[c], key=lambda r: r["seed"])
            ms = sum(float(r["moved"]) for r in ss) / len(ss)
            mi = sum(float(r["moved"]) for r in ii) / len(ii)
            ds = sum(float(r["mean_displacement"]) for r in ss) / len(ss)
            di = sum(float(r["mean_displacement"]) for r in ii) / len(ii)
            bb = gmean([float(b["bb_estimate"]) / float(a["bb_estimate"]) for a, b in zip(ss, ii)])
            wire = gmean([float(b["wirelength"]) / float(a["wirelength"]) for a, b in zip(ss, ii) if a["route_legal"] == "true" and b["route_legal"] == "true"])
            cpd = gmean([float(b["critical_path_ps"]) / float(a["critical_path_ps"]) for a, b in zip(ss, ii) if a["critical_path_ps"] != "none" and b["critical_path_ps"] != "none"])
            t = gmean([float(b["place_s"]) / float(a["place_s"]) for a, b in zip(ss, ii)])
            legal = f"{sum(r['legal'] == 'true' and r['route_legal'] == 'true' for r in ss)}/{sum(r['legal'] == 'true' and r['route_legal'] == 'true' for r in ii)}"
            added = ii[0]["added"]
            for k, v in [("ms", ms), ("mi", mi), ("ds", ds), ("di", di), ("bb", bb), ("wire", wire), ("cpd", cpd), ("t", t)]:
                agg[k].append(v)
            print(f"| {c} | {added} | {100*ms:.1f}% | {100*mi:.1f}% | {ds:.2f} | {di:.3f} | {bb:.3f} | {wire:.3f} | {cpd:.3f} | {t:.3f} | {legal} |")
        if agg:
            m = lambda k: sum(agg[k]) / len(agg[k])
            print(f"| **Mean** | {percent}% | {100*m('ms'):.1f}% | {100*m('mi'):.1f}% | {m('ds'):.2f} | {m('di'):.3f} | {gmean(agg['bb']):.3f} | {gmean(agg['wire']):.3f} | {gmean(agg['cpd']):.3f} | {gmean(agg['t']):.3f} | |")


route_table("route_nets")
route_table("route_moves")
place_table()
