#!/usr/bin/env python3
"""Prints the tables of docs/benchmarks.md from the results of scripts/vtr/compare.sh.

    python3 scripts/vtr/tables.py WORK [BEFORE]

Ratios are guide's over VPR's, so below 1 is better for guide everywhere. The last row of
each table is the geometric mean of the ratios, over the circuits that have both numbers.

With BEFORE, the results of an earlier run, each table also has guide's numbers from that
run, with their own ratios. VPR's quality numbers are the same in every run, as it is
deterministic, but its times are not, so a ratio of times is to VPR's time in the same run.
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


class Column:
    """A number of guide's against one of VPR's, as a value, its ratio and their mean."""

    def __init__(self, guide_key, vpr_key, digits):
        self.guide_key, self.vpr_key, self.digits = guide_key, vpr_key, digits
        self.ratios = []

    def cells(self, results, circuit, scale=1.0):
        r = results.get(circuit)
        if r is None:
            self.ratios.append(None)
            return ["-", "-"]
        guide = number(r, self.guide_key)
        guide = guide * scale if guide is not None else None
        self.ratios.append(ratio(guide, number(r, self.vpr_key)))
        return [show(guide, self.digits), show(self.ratios[-1], 3 if self.digits != 2 else 2)]

    def mean(self):
        return show(geomean(self.ratios), 3 if self.digits != 2 else 2)


def main():
    if len(sys.argv) not in (2, 3):
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    results = load(sys.argv[1])
    after = {r["circuit"]: r for r in results}
    before = {r["circuit"]: r for r in load(sys.argv[2])} if len(sys.argv) == 3 else None
    then = ["guide before", "before / VPR"] if before else []

    def earlier(column, circuit):
        return column.cells(before, circuit) if before else []

    def earlier_mean(column):
        return ["", column.mean()] if before else []

    # Minimum channel width, with the hard blocks of architectures that have them.
    hard = [
        (key, name)
        for key, name in (("mult_36", "Multipliers"), ("memory", "Memories"))
        if any(key in r for r in results)
    ]
    rows = []
    now, old = Column("guide_min_w", "vpr_min_w", 0), Column("guide_min_w", "vpr_min_w", 0)
    for r in results:
        c = r["circuit"]
        rows.append(
            [c, r.get("clbs", "-")]
            + [r.get(key, "-") for key, _ in hard]
            + [r.get("grid", "-"), show(number(r, "vpr_min_w"), 0)]
            + earlier(old, c)
            + now.cells(after, c)
            + [r.get("guide_min_w_vpr_legal", "-")]
        )
    table(
        ["Circuit", "CLBs"]
        + [name for _, name in hard]
        + ["Grid, tiles a side", "VPR"]
        + then
        + ["guide", "guide / VPR", "VPR accepts guide's route"],
        rows,
        ["Geomean", ""] + ["" for _ in hard] + ["", ""] + earlier_mean(old) + ["", now.mean(), ""],
    )

    # At the relaxed width: delay, as VPR's analysis times both.
    rows, own = [], []
    now = Column("guide_one_vpr_cpd_ns", "vpr_cpd_ns", 3)
    old = Column("guide_one_vpr_cpd_ns", "vpr_cpd_ns", 3)
    for r in results:
        c = r["circuit"]
        guide = number(r, "guide_one_vpr_cpd_ns")
        # The worst slack against a period of 0 counts setup times, as VPR's critical path
        # does. Runs before it was recorded only have the latest arrival.
        slack = number(r, "guide_one_worst_slack_ps")
        estimate = -slack if slack is not None else number(r, "guide_one_critical_path_ps")
        estimate = estimate / 1000 if estimate is not None else None
        own.append(ratio(estimate, guide))
        rows.append(
            [c, r.get("relaxed_w", "-"), show(number(r, "vpr_cpd_ns"))]
            + earlier(old, c)
            + now.cells(after, c)
            + [show(estimate), show(own[-1]), r.get("guide_one_vpr_legal", "-")]
        )
    table(
        ["Circuit", "W", "VPR, ns"]
        + (["guide before, ns", "before / VPR"] if before else [])
        + [
            "guide, ns",
            "guide / VPR",
            "guide's own STA, ns",
            "own / VPR's STA",
            "VPR accepts guide's route",
        ],
        rows,
        ["Geomean", "", ""]
        + earlier_mean(old)
        + ["", now.mean(), "", show(geomean(own)), ""],
    )

    # Wirelength.
    rows = []
    now = Column("guide_one_vpr_wirelength", "vpr_wirelength", 0)
    old = Column("guide_one_vpr_wirelength", "vpr_wirelength", 0)
    for r in results:
        c = r["circuit"]
        rows.append(
            [c, show(number(r, "vpr_wirelength"), 0)] + earlier(old, c) + now.cells(after, c)
        )
    table(
        ["Circuit", "VPR"] + then + ["guide", "guide / VPR"],
        rows,
        ["Geomean", ""] + earlier_mean(old) + ["", now.mean()],
    )

    # Runtime.
    rows = []
    now = Column("guide_one_route_s", "vpr_route_s", 2)
    old = Column("guide_one_route_s", "vpr_route_s", 2)
    many = Column("guide_many_route_s", "vpr_route_s", 2)
    for r in results:
        c = r["circuit"]
        one, one_ratio = now.cells(after, c)
        rows.append(
            [
                c,
                show(number(r, "vpr_route_s"), 2),
                show(number(r, "vpr_lookahead_s"), 2),
                r.get("vpr_iterations", "-"),
            ]
            + earlier(old, c)
            + [
                one,
                show(number(r, "guide_one_lookahead_s"), 2),
                r.get("guide_one_iterations", "-"),
                one_ratio,
            ]
            + many.cells(after, c)
            + [show(number(r, "guide_many_vpr_cpd_ns")), r.get("guide_many_vpr_legal", "-")]
        )
    table(
        ["Circuit", "VPR, s", "VPR's lookahead, s", "VPR iterations"]
        + (["guide before, s", "before / VPR"] if before else [])
        + [
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
        ["Geomean", "", "", ""]
        + (["", old.mean()] if before else [])
        + ["", "", "", now.mean(), "", many.mean(), "", ""],
    )


if __name__ == "__main__":
    main()
