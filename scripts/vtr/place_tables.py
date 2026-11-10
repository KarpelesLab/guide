#!/usr/bin/env python3
"""Prints the tables of docs/benchmarks.md's FPGA placement section from runs of place.sh.

    python3 scripts/vtr/place_tables.py WORK...

Each WORK is the output directory of a run of scripts/vtr/place.sh. Circuits are listed in
alphabetical order, from every WORK given, and the last row of each table is the geometric
mean of its ratios. Ratios are guide's over VPR's, so below 1 is better for guide.
"""

import math
import os
import sys


def read(path):
    result = {}
    with open(path) as f:
        for line in f:
            key, _, value = line.strip().partition("=")
            if key:
                result[key] = value
    return result


def number(value):
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def ratio(a, b):
    a, b = number(a), number(b)
    if a is None or b is None or b == 0:
        return None
    return a / b


def geomean(values):
    values = [v for v in values if v is not None and v > 0]
    if not values:
        return ""
    return math.exp(sum(math.log(v) for v in values) / len(values))


def fmt(value, digits=3):
    if value is None or value == "":
        return ""
    if isinstance(value, float):
        return f"{value:.{digits}f}"
    return str(value)


def table(header, rows, ratio_columns):
    print("| " + " | ".join(header) + " |")
    print("|" + " --- |" * len(header))
    for row in rows:
        print("| " + " | ".join(fmt(cell) for cell in row) + " |")
    mean = ["Geomean"] + [""] * (len(header) - 1)
    for column in ratio_columns:
        mean[column] = fmt(geomean([row[column] for row in rows]))
    print("| " + " | ".join(mean) + " |")
    print()


def main():
    results = []
    for work in sys.argv[1:]:
        for name in sorted(os.listdir(work)):
            path = os.path.join(work, name, "result.txt")
            if os.path.isfile(path):
                results.append(read(path))
    results.sort(key=lambda r: r.get("circuit", ""))

    def blocks(r):
        parts = [r.get("clbs", "")]
        for kind in ("mult_36", "memory"):
            if kind in r:
                parts.append(r[kind])
        return " / ".join(parts)

    def seconds(value):
        v = number(value)
        return None if v is None else v

    print("Placement, as VPR measures it:\n")
    rows = []
    for r in results:
        rows.append([
            r.get("circuit"),
            blocks(r),
            r.get("grid"),
            r.get("vpr_bb"),
            r.get("guide_vpr_bb"),
            ratio(r.get("guide_vpr_bb"), r.get("vpr_bb")),
            r.get("vpr_est_cpd_ns"),
            r.get("guide_vpr_est_cpd_ns"),
            ratio(r.get("guide_vpr_est_cpd_ns"), r.get("vpr_est_cpd_ns")),
            seconds(r.get("vpr_place_s")),
            seconds(r.get("guide_place_place_s")),
            ratio(r.get("guide_place_place_s"), r.get("vpr_place_s")),
            r.get("guide_place_chains"),
            r.get("guide_vpr_legal"),
        ])
    table(
        ["Circuit", "Blocks", "Grid", "VPR BB", "guide BB", "/ VPR", "VPR est. CPD, ns",
         "guide est. CPD, ns", "/ VPR", "VPR, s", "guide, s", "/ VPR", "Chains",
         "VPR accepts guide's"],
        rows, [5, 8, 11])

    for router, label in (("vpr", "VPR's router"), ("guide", "guide's router")):
        print(f"Routed by {label}:\n")
        rows = []
        for r in results:
            if router == "vpr":
                w_vpr, w_guide = r.get("vpr_route_vpr_min_w"), r.get("guide_route_vpr_min_w")
                cpd_vpr, cpd_guide = r.get("vpr_route_vpr_cpd_ns"), r.get("guide_route_vpr_cpd_ns")
                wl_vpr = r.get("vpr_route_vpr_wirelength")
                wl_guide = r.get("guide_route_vpr_wirelength")
                legal = r.get("guide_route_vpr_legal")
            else:
                w_vpr, w_guide = r.get("vpr_route_guide_min_w"), r.get("guide_route_guide_min_w")
                cpd_vpr = r.get("vpr_route_guide_cpd_ns")
                cpd_guide = r.get("guide_route_guide_cpd_ns")
                wl_vpr = r.get("vpr_route_guide_vpr_wirelength")
                wl_guide = r.get("guide_route_guide_vpr_wirelength")
                legal = "yes" if (r.get("guide_route_guide_vpr_legal") == "yes"
                                  and r.get("guide_route_guide_min_w_vpr_legal") == "yes") \
                    else "no"
            rows.append([
                r.get("circuit"),
                w_vpr,
                w_guide,
                ratio(w_guide, w_vpr),
                r.get("relaxed_w"),
                cpd_vpr,
                cpd_guide,
                ratio(cpd_guide, cpd_vpr),
                wl_vpr,
                wl_guide,
                ratio(wl_guide, wl_vpr),
                legal,
            ])
        table(
            ["Circuit", "Min W, VPR's placement", "Min W, guide's", "/ VPR", "W",
             "CPD, VPR's placement, ns", "CPD, guide's, ns", "/ VPR", "Wire, VPR's placement",
             "Wire, guide's", "/ VPR", "Routes legal"],
            rows, [3, 7, 10])


if __name__ == "__main__":
    main()
