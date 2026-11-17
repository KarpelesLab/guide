#!/usr/bin/env python3
"""Tables of guide's MAPF results against the reference solvers.

    python3 scripts/mapf/tables.py OUT [BEFORE]
    python3 scripts/mapf/tables.py --summary OUT DATA

OUT holds what guide.sh and reference.py wrote. BEFORE, if given, is another guide.sh output
directory, shown as a column of its own. For each of the first results' six maps and agent
count: the share of the scenarios solved within the time limit, by each solver; the sum of
costs over the sum of the agents' shortest paths (a lower bound), and over CBSH2-RTC's optimum
where it found one, on the scenarios that both solved; and the median time.

With --summary, one row per map and scenario set, over the first five scenarios of every map
in OUT: how many instances each solver solved, and sums of costs over the MAPF tracker's best
known solutions and lower bounds, which scripts/mapf/tracker.py puts in DATA/tracker, on the
instances that both guide and LaCAM3 solved.
"""

import json

import collections
import glob
import os
import statistics
import sys


def read(path):
    with open(path, encoding="utf-8") as f:
        return [line.rstrip("\n").split("\t") for line in f if line.strip()]


def load_guide(directory, name):
    """guide's results: solved, sum of costs, lower bound, milliseconds, and the
    milliseconds to its first solution and whether it fell back on it, where known."""
    path = os.path.join(directory, f"guide-{name}.tsv")
    result = {}
    if not os.path.exists(path):
        return result
    for row in read(path):
        scen, count, solved, soc, lb, _makespan, ms = row[:7]
        first = float(row[13]) if len(row) > 14 else None
        fell_back = len(row) > 14 and row[14] == "1"
        result[(scen, int(count))] = (solved == "1", int(soc) if solved == "1" else None,
                                     int(lb), int(ms), first, fell_back)
    return result


def load_ref(directory, name):
    """Each solver's results, from ref-NAME.tsv and then any ref-NAME-ONLY.tsv, whose solvers
    replace those of the first."""
    result = collections.defaultdict(dict)
    paths = [os.path.join(directory, f"ref-{name}.tsv")]
    even = name.endswith("-even")
    paths += sorted(p for p in glob.glob(os.path.join(directory, f"ref-{name}-*.tsv"))
                    if even or not p.endswith("-even.tsv") and "-even-" not in p)
    for path in paths:
        if not os.path.exists(path):
            continue
        rows = read(path)
        for solver in {row[0] for row in rows}:
            result[solver] = {}
        for row in rows:
            solver, scen, count, solved, soc, ms = row[:6]
            solved = solved == "1"
            entry = (solved, int(soc) if solved else None, int(ms) if ms != "-" else None)
            result[solver][(scen, int(count))] = entry
    return result


def ratio(pairs):
    pairs = [(a, b) for a, b in pairs if a is not None and b]
    if not pairs:
        return "-"
    return f"{sum(a for a, _ in pairs) / sum(b for _, b in pairs):.3f}"


def load_tracker(data):
    """The tracker's best known solution and lower bound per scenario file and agent count."""
    best = {}
    for path in glob.glob(os.path.join(data, "tracker", "*.json")):
        stem = os.path.basename(path)[:-len(".json")]
        with open(path, encoding="utf-8") as f:
            rows = json.load(f)
        for row in rows:
            solution, lower = row.get("solution_cost"), row.get("lower_cost")
            best[(stem + ".scen", int(row["agents"]))] = (solution, lower)
    return best


def summary(out, data):
    best = load_tracker(data)
    names = sorted({os.path.basename(p)[len("guide-"):-len(".tsv")]
                    for p in glob.glob(os.path.join(out, "guide-*.tsv"))}, key=str.lower)
    for kind in ["random", "even"]:
        print(f"{kind.capitalize()} scenarios:\n")
        head = ["Map", "Agents", "Instances", "Solved: guide", "LaCAM3", "EECBS", "CBSH2-RTC",
                "guide fell back", "guide / best", "LaCAM3 / best", "guide / tracker LB",
                "guide < best", "guide first ms", "guide ms", "LaCAM3 first ms"]
        print("| " + " | ".join(head) + " |")
        print("|" + " --- |" * len(head))
        for name in names:
            if name.endswith("-even") != (kind == "even"):
                continue
            guide = load_guide(out, name)
            ref = load_ref(out, name)
            lacam = ref.get("lacam3", {})
            map_name = name[:-len("-even")] if kind == "even" else name
            scens = sorted({scen for scen, _ in guide}, key=lambda s: int(s.split("-")[-1][:-5]))
            first5 = set(scens[:5])
            keys = sorted(k for k in guide if k[0] in first5)
            if not keys:
                continue
            counts = sorted({c for _, c in keys})

            def solved(table):
                ran = [k for k in keys if k in table]
                return f"{sum(1 for k in ran if table[k][0])}" if ran else "-"

            # Both on the instances that both solved, where LaCAM3 ran.
            both = [k for k in keys if guide[k][0] and (not lacam or k in lacam and lacam[k][0])]

            def over(table, index):
                pairs = [(table[k][1], best[k][index]) for k in both
                         if k in table and k in best and best[k][index]]
                return ratio(pairs)

            def median(values):
                values = [v for v in values if v is not None]
                return str(round(statistics.median(values))) if values else "-"

            better = sum(1 for k in keys if guide[k][0] and k in best and best[k][0]
                         and guide[k][1] < best[k][0])
            cells = [map_name, f"{counts[0]}-{counts[-1]}", str(len(keys)), solved(guide),
                     solved(lacam), solved(ref.get("eecbs-1.2", {})),
                     solved(ref.get("cbsh2-rtc", {})),
                     str(sum(1 for k in keys if guide[k][5])),
                     over(guide, 0), over(lacam, 0), over(guide, 1), str(better),
                     median(guide[k][4] for k in keys if guide[k][0]),
                     median(guide[k][3] for k in keys if guide[k][0]),
                     median(lacam[k][2] for k in keys if k in lacam and lacam[k][0])]
            print("| " + " | ".join(cells) + " |")
        print()


def main():
    if sys.argv[1] == "--summary":
        summary(sys.argv[2], sys.argv[3])
        return
    out = sys.argv[1]
    before = sys.argv[2] if len(sys.argv) > 2 else None
    order = ["empty-32-32", "random-32-32-20", "room-32-32-4", "maze-32-32-2",
             "warehouse-10-20-10-2-1", "den520d"]
    names = [n for n in order if os.path.exists(os.path.join(out, f"guide-{n}.tsv"))]
    solvers = ["cbsh2-rtc", "eecbs-1.2", "lacam3", "lacam3-mt"]
    for name in names:
        guide = load_guide(out, name)
        prior = load_guide(before, name) if before else {}
        ref = load_ref(out, name)
        lb = {key: value[2] for key, value in guide.items()}
        counts = sorted({count for _, count in guide})
        print(f"#### {name}\n")
        head = ["Agents", "guide"] + (["before"] if before else []) + solvers
        print("Solved within the limit, of each count's scenarios:\n")
        print("| " + " | ".join(head) + " |")
        print("|" + " --- |" * len(head))
        for count in counts:
            keys = [k for k in guide if k[1] == count]
            cells = [str(count)]
            for table in [guide] + ([prior] if before else []) + [ref[s] for s in solvers]:
                ran = [k for k in keys if k in table]
                solved = sum(1 for k in ran if table[k][0])
                cells.append(f"{solved}/{len(ran)}" if ran else "-")
            print("| " + " | ".join(cells) + " |")
        print()
        print("Sum of costs over the lower bound, on the scenarios that each solved, then over "
              "CBSH2-RTC's optimum on those that CBSH2-RTC solved too, and median milliseconds "
              "to a solution, LaCAM3's first:\n")
        head = ["Agents", "guide / LB"] + (["before / LB"] if before else []) + [
            "LaCAM3 / LB", "LaCAM3 threads / LB", "EECBS / LB", "guide / opt", "LaCAM3 / opt", "EECBS / opt",
            "guide ms", "guide first ms"] + (["before ms"] if before else []) + [
                "LaCAM3 first ms", "EECBS ms",
                                                                "CBSH2 ms"]
        print("| " + " | ".join(head) + " |")
        print("|" + " --- |" * len(head))
        for count in counts:
            keys = sorted(k for k in guide if k[1] == count)

            def over_lb(table):
                return ratio([(table[k][1], lb[k]) for k in keys if k in table and table[k][0]])

            def over_opt(table):
                opt = ref["cbsh2-rtc"]
                return ratio([(table[k][1], opt[k][1]) for k in keys
                              if k in table and table[k][0] and k in opt and opt[k][0]])

            def median_ms(table, index):
                times = [table[k][index] for k in keys
                         if k in table and table[k][0] and table[k][index] is not None]
                return str(round(statistics.median(times))) if times else "-"

            cells = [str(count), over_lb(guide)]
            if before:
                cells.append(over_lb(prior))
            cells += [over_lb(ref["lacam3"]), over_lb(ref["lacam3-mt"]),
                      over_lb(ref["eecbs-1.2"]), over_opt(guide), over_opt(ref["lacam3"]),
                      over_opt(ref["eecbs-1.2"]), median_ms(guide, 3), median_ms(guide, 4)]
            if before:
                cells.append(median_ms(prior, 3))
            cells += [median_ms(ref["lacam3"], 2), median_ms(ref["eecbs-1.2"], 2),
                      median_ms(ref["cbsh2-rtc"], 2)]
            print("| " + " | ".join(cells) + " |")
        print()


if __name__ == "__main__":
    main()
