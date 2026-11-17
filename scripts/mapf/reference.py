#!/usr/bin/env python3
"""Runs reference MAPF solvers on the instances that scripts/mapf/guide.sh runs.

    python3 scripts/mapf/reference.py DATA OUT SOLVERS [MAP...|all]

SOLVERS is a directory holding the solvers, built from source by build-reference.sh:
CBSH2-RTC/build/cbs (optimal), EECBS/build/eecbs (bounded-suboptimal, w = 1.2), and
lacam3/build/main (anytime, on one thread, and as "lacam3-mt" with its default threads, only
when ONLY asks for it). Lines go to OUT/ref-MAP.tsv:

    solver  scenario  agents  solved  sum-of-costs  milliseconds  [first-ms]

For LaCAM3, the milliseconds are those to its first solution, which it refines until its time
is up, and the sum of costs is that of its last. It reports two costs: `soc`, the sum of costs
as everyone else has it, and `sum_of_loss`, which leaves out waits on the goal in the middle of
a plan too and is what LaCAM3 minimizes. This takes `soc`. The environment variables SCENS,
JOBS, LIMIT, KIND, PARTS and MEMORY are as for guide.sh, as are the maps and their agent counts, and ONLY,
a comma-separated list of solvers, runs only those, into OUT/ref-MAP-ONLY.tsv, which tables.py
reads after OUT/ref-MAP.tsv. The even set goes to OUT/ref-MAP-even.tsv.

CBSH2-RTC and EECBS run each scenario's counts in increasing order, and once one fails within
the limit, the larger counts of that scenario are counted as failed without running them: a
search that runs out of time on n agents would on more, but for rare luck, and running them
would take a minute each for nothing. LaCAM3 runs every instance.
"""

import concurrent.futures
import os
import resource
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


def limited():
    """In the child: the lowest priority, and at most MEMORY KB of address space, so that a
    solver that grows fails rather than taking the machine with it."""
    os.nice(19)
    memory = int(os.environ.get("MEMORY", "12000000")) * 1024
    resource.setrlimit(resource.RLIMIT_AS, (memory, memory))


def run(cmd, limit):
    """Runs a solver, limited, and kills it if it overstays its limit by far, which then
    counts as a failure."""
    try:
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       preexec_fn=limited, timeout=limit * 3 + 60, check=False)
    except subprocess.TimeoutExpired:
        pass


def run_cbs(binary, extra, name, mapfile, scen, count, limit):
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "out.csv")
        cmd = [binary, "-m", mapfile, "-a", scen, "-k", str(count), "-t", str(limit), "-o", out]
        run(cmd + extra, limit)
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
        run(cmd, limit)
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


def counts(data, name, kind):
    """The map's agent counts: those of the first results' six maps, or a tenth of its
    scenarios' agents, two tenths, and so on to all of them; with PARTS=n, for every map, an
    n-th of them, two n-ths, and so on."""
    parts = int(os.environ.get("PARTS", "0"))
    if name in COUNTS and not parts:
        return list(COUNTS[name])
    with open(os.path.join(data, "scen", f"scen-{kind}", f"{name}-{kind}-1.scen"),
              encoding="utf-8") as f:
        most = sum(1 for line in f if line.strip()) - 1
    parts = parts or 10
    return [most * k // parts for k in range(1, parts + 1)]


def run_in_order(binary, extra, name, mapfile, scen, counts_, limit):
    """Runs `counts_` in order, and counts those after a failure as failed."""
    rows = []
    for count in counts_:
        if rows and rows[-1][2] == 0:
            rows.append([name, count, 0, "-", "-"])
            continue
        rows.append(run_cbs(binary, extra, name, mapfile, scen, count, limit))
    return rows


def main():
    data, out, solvers = sys.argv[1:4]
    maps = sys.argv[4:] or list(COUNTS)
    kind = os.environ.get("KIND", "random")
    if maps == ["all"]:
        maps = sorted(f[:-4] for f in os.listdir(os.path.join(data, "maps")) if f.endswith(".map"))
    scens = int(os.environ.get("SCENS", "25"))
    jobs = int(os.environ.get("JOBS", "8"))
    limit = int(os.environ.get("LIMIT", "60"))
    only = [name for name in os.environ.get("ONLY", "").split(",") if name]
    suffix = "-" + "-".join(only) if only else ""
    os.makedirs(out, exist_ok=True)
    cbs = os.path.join(solvers, "CBSH2-RTC/build/cbs")
    eecbs = os.path.join(solvers, "EECBS/build/eecbs")
    lacam = os.path.join(solvers, "lacam3/build/main")
    tasks = []
    with concurrent.futures.ThreadPoolExecutor(jobs) as pool:
        for name in maps:
            mapfile = os.path.join(data, "maps", name + ".map")
            for index in range(1, scens + 1):
                scen = os.path.join(data, "scen", f"scen-{kind}", f"{name}-{kind}-{index}.scen")
                tag = os.path.basename(scen)
                todo = counts(data, name, kind)
                if not only or "cbsh2-rtc" in only:
                    tasks.append((name, tag, pool.submit(run_in_order, cbs, [], "cbsh2-rtc",
                                                         mapfile, scen, todo, limit)))
                if not only or "eecbs-1.2" in only:
                    tasks.append((name, tag, pool.submit(run_in_order, eecbs,
                                                         ["--suboptimality=1.2"], "eecbs-1.2",
                                                         mapfile, scen, todo, limit)))
                for count in todo:
                    for solver in ["lacam3", "lacam3-mt"]:
                        if (solver == "lacam3" and (not only or solver in only)) or \
                                (solver == "lacam3-mt" and solver in only):
                            task = pool.submit(run_lacam, lacam, solver, mapfile, scen, count,
                                               limit)
                            tasks.append((name, tag, task))
        files = {}
        for name, tag, task in tasks:
            if name not in files:
                kind_suffix = "" if kind == "random" else f"-{kind}"
                path = os.path.join(out, f"ref-{name}{kind_suffix}{suffix}.tsv")
                files[name] = open(path, "w", encoding="utf-8")
            result = task.result()
            rows = result if isinstance(result[0], list) else [result]
            for row in rows:
                print("\t".join(str(v) for v in [row[0], tag] + row[1:]), file=files[name],
                      flush=True)
        for f in files.values():
            f.close()


if __name__ == "__main__":
    main()
