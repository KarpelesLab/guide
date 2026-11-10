#!/usr/bin/env python3
"""Runs reference MAPF solvers on the instances that scripts/mapf/guide.sh runs.

    python3 scripts/mapf/reference.py DATA OUT SOLVERS [MAP...]

SOLVERS is a directory holding the solvers, built from source by build-reference.sh:
CBSH2-RTC/build/cbs (optimal), EECBS/build/eecbs (bounded-suboptimal, w = 1.2), and
lacam3/build/main (anytime, on one thread, and as "lacam3-mt" with its default threads, only
when ONLY asks for it). Lines go to OUT/ref-MAP.tsv:

    solver  scenario  agents  solved  sum-of-costs  milliseconds  [first-ms]

For LaCAM3, the milliseconds are those to its first solution, which it refines until its time
is up, and the sum of costs is that of its last. It reports two costs: `soc`, the sum of costs
as everyone else has it, and `sum_of_loss`, which leaves out waits on the goal in the middle of
a plan too and is what LaCAM3 minimizes. This takes `soc`. The environment variables SCENS,
JOBS and LIMIT are as for guide.sh, and ONLY, a comma-separated list of solvers, runs only
those, into OUT/ref-MAP-ONLY.tsv, which tables.py reads after OUT/ref-MAP.tsv.
"""

import concurrent.futures
import os
import subprocess
import sys
import tempfile

COUNTS = {
    "empty-32-32": range(50, 501, 50),
    "random-32-32-20": range(25, 251, 25),
    "room-32-32-4": range(10, 101, 10),
    "maze-32-32-2": range(10, 101, 10),
    "warehouse-10-20-10-2-1": range(50, 501, 50),
    "den520d": range(100, 1001, 100),
}


def run_cbs(binary, extra, name, mapfile, scen, count, limit):
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "out.csv")
        cmd = [binary, "-m", mapfile, "-a", scen, "-k", str(count), "-t", str(limit), "-o", out]
        subprocess.run(cmd + extra, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       timeout=limit * 3 + 60, check=False)
        try:
            with open(out, encoding="utf-8") as f:
                rows = f.read().splitlines()
        except OSError:
            return [name, count, 0, "-", "-"]
        header = rows[0].split(",")
        values = rows[-1].split(",")
        row = dict(zip(header, values))
        runtime = float(row["runtime"])
        cost = int(float(row["solution cost"]))
        solved = cost >= 0 and runtime < limit
        return [name, count, int(solved), cost if solved else "-", round(runtime * 1000)]


def run_lacam(binary, name, mapfile, scen, count, limit):
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "out.txt")
        cmd = [binary, "-m", mapfile, "-i", scen, "-N", str(count), "-t", str(limit), "-o", out]
        if name == "lacam3":
            cmd.append("--no-multi-thread")
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       timeout=limit * 3 + 60, check=False)
        values = {}
        try:
            with open(out, encoding="utf-8") as f:
                for line in f:
                    key, _, value = line.strip().partition("=")
                    values[key] = value
        except OSError:
            return [name, count, 0, "-", "-"]
        if values.get("solved") != "1":
            return [name, count, 0, "-", "-"]
        return [name, count, 1, int(values["soc"]), int(values["comp_time_initial_solution"])]


def main():
    data, out, solvers = sys.argv[1:4]
    maps = sys.argv[4:] or list(COUNTS)
    scens = int(os.environ.get("SCENS", "25"))
    jobs = int(os.environ.get("JOBS", "8"))
    limit = int(os.environ.get("LIMIT", "60"))
    only = [name for name in os.environ.get("ONLY", "").split(",") if name]
    suffix = "-" + "-".join(only) if only else ""
    os.makedirs(out, exist_ok=True)
    cbs = os.path.join(solvers, "CBSH2-RTC/build/cbs")
    eecbs = os.path.join(solvers, "EECBS/build/eecbs")
    lacam = os.path.join(solvers, "lacam3/build/main")
    for name in maps:
        mapfile = os.path.join(data, "maps", name + ".map")
        tasks = []
        with concurrent.futures.ThreadPoolExecutor(jobs) as pool:
            for index in range(1, scens + 1):
                scen = os.path.join(data, "scen", "scen-random", f"{name}-random-{index}.scen")
                for count in COUNTS.get(name, [10, 20, 50, 100]):
                    tag = os.path.basename(scen)
                    if not only or "cbsh2-rtc" in only:
                        tasks.append((tag, pool.submit(run_cbs, cbs, [], "cbsh2-rtc", mapfile,
                                                       scen, count, limit)))
                    if not only or "eecbs-1.2" in only:
                        tasks.append((tag, pool.submit(run_cbs, eecbs, ["--suboptimality=1.2"],
                                                       "eecbs-1.2", mapfile, scen, count, limit)))
                    if not only or "lacam3" in only:
                        tasks.append((tag, pool.submit(run_lacam, lacam, "lacam3", mapfile, scen,
                                                       count, limit)))
                    if "lacam3-mt" in only:
                        tasks.append((tag, pool.submit(run_lacam, lacam, "lacam3-mt", mapfile,
                                                       scen, count, limit)))
            path = os.path.join(out, f"ref-{name}{suffix}.tsv")
            with open(path, "w", encoding="utf-8") as f:
                for tag, task in tasks:
                    row = task.result()
                    print("\t".join(str(v) for v in [row[0], tag] + row[1:]), file=f, flush=True)


if __name__ == "__main__":
    main()
