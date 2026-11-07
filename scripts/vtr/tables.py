#!/usr/bin/env python3
"""Prints the tables of docs/benchmarks.md from the results of scripts/vtr/compare.sh.

    python3 scripts/vtr/tables.py WORK

Ratios are guide's over VPR's, so below 1 is better for guide everywhere. The last row of
each table is the geometric mean of the ratios, over the circuits that have both numbers.
"""

import math
import sys
from pathlib import Path


def load(work):
    results = []
    for path in sorted(Path(work).glob("*/result.txt")):
        values = {}
        for line in path.read_text().splitlines():
            key, _, value = line.partition("=")
            values[key] = value
        if values.get("done") == "yes":
            results.append(values)
        else:
            print(f"{path.parent.name} did not finish", file=sys.stderr)
    return results


def number(values, key):
    try:
        return float(values.get(key, ""))
    except ValueError:
        return None


def ratio(a, b):
    if a is None or b is None or b == 0:
        return None
    return a / b


def geomean(ratios):
    ratios = [r for r in ratios if r is not None and r > 0]
    if not ratios:
        return None
    return math.exp(sum(math.log(r) for r in ratios) / len(ratios))


def show(value, digits=3):
    if value is None:
        return "-"
    if isinstance(value, str):
        return value
    return f"{value:.{digits}f}"


def table(header, rows, means):
    print("| " + " | ".join(header) + " |")
    print("| " + " | ".join("---" for _ in header) + " |")
    for row in rows:
        print("| " + " | ".join(row) + " |")
    print("| " + " | ".join(means) + " |")
    print()


def main():
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    results = load(sys.argv[1])

    # Minimum channel width.
    rows, ratios = [], []
    for r in results:
        vpr_w = number(r, "vpr_min_w")
        guide_w = number(r, "guide_min_w")
        ratios.append(ratio(guide_w, vpr_w))
        rows.append(
            [
                r["circuit"],
                r.get("clbs", "-"),
                r.get("grid", "-"),
                show(vpr_w, 0),
                show(guide_w, 0),
                show(ratios[-1]),
                r.get("guide_min_w_vpr_legal", "-"),
            ]
        )
    table(
        ["Circuit", "CLBs", "Grid", "VPR", "guide", "guide / VPR", "VPR accepts guide's route"],
        rows,
        ["Geomean", "", "", "", "", show(geomean(ratios)), ""],
    )

    # At the relaxed width: delay, as VPR's analysis times both.
    rows, cpd_ratios, own = [], [], []
    for r in results:
        vpr = number(r, "vpr_cpd_ns")
        guide = number(r, "guide_one_vpr_cpd_ns")
        estimate = number(r, "guide_one_critical_path_ps")
        estimate = estimate / 1000 if estimate is not None else None
        cpd_ratios.append(ratio(guide, vpr))
        own.append(ratio(estimate, guide))
        rows.append(
            [
                r["circuit"],
                r.get("relaxed_w", "-"),
                show(vpr),
                show(guide),
                show(cpd_ratios[-1]),
                show(estimate),
                show(own[-1]),
                r.get("guide_one_vpr_legal", "-"),
            ]
        )
    table(
        [
            "Circuit",
            "W",
            "VPR, ns",
            "guide, ns",
            "guide / VPR",
            "guide's own STA, ns",
            "own / VPR's STA",
            "VPR accepts guide's route",
        ],
        rows,
        ["Geomean", "", "", "", show(geomean(cpd_ratios)), "", show(geomean(own)), ""],
    )

    # Wirelength.
    rows, ratios = [], []
    for r in results:
        vpr = number(r, "vpr_wirelength")
        guide = number(r, "guide_one_vpr_wirelength")
        ratios.append(ratio(guide, vpr))
        rows.append([r["circuit"], show(vpr, 0), show(guide, 0), show(ratios[-1])])
    table(
        ["Circuit", "VPR", "guide", "guide / VPR"],
        rows,
        ["Geomean", "", "", show(geomean(ratios))],
    )

    # Runtime.
    rows, one_ratios, many_ratios = [], [], []
    for r in results:
        vpr = number(r, "vpr_route_s")
        one = number(r, "guide_one_route_s")
        many = number(r, "guide_many_route_s")
        one_ratios.append(ratio(one, vpr))
        many_ratios.append(ratio(many, vpr))
        rows.append(
            [
                r["circuit"],
                show(vpr, 2),
                show(number(r, "vpr_lookahead_s"), 2),
                r.get("vpr_iterations", "-"),
                show(one, 2),
                show(number(r, "guide_one_lookahead_s"), 2),
                r.get("guide_one_iterations", "-"),
                show(one_ratios[-1], 2),
                show(many, 2),
                show(many_ratios[-1], 2),
                show(number(r, "guide_many_vpr_cpd_ns")),
                r.get("guide_many_vpr_legal", "-"),
            ]
        )
    table(
        [
            "Circuit",
            "VPR, s",
            "VPR's lookahead, s",
            "VPR iterations",
            "guide, 1 thread, s",
            "guide's lookahead, s",
            "guide iterations",
            "/ VPR",
            "guide, threads, s",
            "/ VPR",
            "Its delay, ns",
            "VPR accepts it",
        ],
        rows,
        [
            "Geomean",
            "",
            "",
            "",
            "",
            "",
            "",
            show(geomean(one_ratios), 2),
            "",
            show(geomean(many_ratios), 2),
            "",
            "",
        ],
    )


if __name__ == "__main__":
    main()
