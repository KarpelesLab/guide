#!/usr/bin/env python3
"""Runs reference MAPF solvers on the instances that scripts/mapf/guide.sh runs.

    python3 scripts/mapf/reference.py DATA OUT SOLVERS [MAP...]

SOLVERS is a directory holding the solvers, built from source by build-reference.sh:
CBSH2-RTC/build/cbs (optimal), EECBS/build/eecbs (bounded-suboptimal, w = 1.2), and
lacam3/build/main (anytime, on one thread). Lines go to OUT/ref-MAP.tsv:

    solver  scenario  agents  solved  sum-of-costs  milliseconds  [first-sum-of-costs  first-ms]

The last two are LaCAM3's first solution, before it refines it. The environment variables
SCENS, JOBS and LIMIT are as for guide.sh.
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


def run_lacam(binary, mapfile, scen, count, limit):
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "out.txt")
        cmd = [binary, "-m", mapfile, "-i", scen, "-N", str(count), "-t", str(limit), "-o", out,
               "--no-multi-thread"]
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       timeout=limit * 3 + 60, check=False)
        values = {}
        try:
            with open(out, encoding="utf-8") as f:
                for line in f:
                    key, _, value = line.strip().partition("=")
                    values[key] = value
        except OSError:
            return ["lacam3", count, 0, "-", "-", "-", "-"]
        if values.get("solved") != "1":
            return ["lacam3", count, 0, "-", "-", "-", "-"]
        return ["lacam3", count, 1, int(values["sum_of_loss"]), int(values["comp_time"]),
                int(values["cost_initial_solution"]), int(values["comp_time_initial_solution"])]


def main():
    data, out, solvers = sys.argv[1:4]
    maps = sys.argv[4:] or list(COUNTS)
    scens = int(os.environ.get("SCENS", "25"))
    jobs = int(os.environ.get("JOBS", "8"))
    limit = int(os.environ.get("LIMIT", "60"))
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
                    tasks.append((tag, pool.submit(run_cbs, cbs, [], "cbsh2-rtc", mapfile, scen,
                                                   count, limit)))
                    tasks.append((tag, pool.submit(run_cbs, eecbs, ["--suboptimality=1.2"],
                                                   "eecbs-1.2", mapfile, scen, count, limit)))
                    tasks.append((tag, pool.submit(run_lacam, lacam, mapfile, scen, count,
                                                   limit)))
            with open(os.path.join(out, f"ref-{name}.tsv"), "w", encoding="utf-8") as f:
                for tag, task in tasks:
                    row = task.result()
                    print("\t".join(str(v) for v in [row[0], tag] + row[1:]), file=f, flush=True)


if __name__ == "__main__":
    main()
