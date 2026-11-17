#!/usr/bin/env python3
"""Downloads the MAPF tracker's best known results for the MovingAI benchmarks.

    python3 scripts/mapf/tracker.py DATA [MAP...]

The tracker (Shen et al., *Tracking Progress in Multi-Agent Path Finding*, 2023,
<https://tracker.pathfinding.ai>) keeps, for every instance of every scenario, the best known
lower bound on the sum of costs and the best known solution's. Its web application reads them
from a public API, which this asks for the same way, one scenario at a time, into
DATA/tracker/MAP-TYPE-INDEX.json: a list of objects with `agents`, `lower_cost` and
`solution_cost`, among other fields. Files already there are kept. Without MAP, every map.
"""

import json
import os
import sys
import time
import urllib.request

API = "https://fe2410d1.pathfinding.ai/api"


def get(path):
    for attempt in range(5):
        try:
            with urllib.request.urlopen(API + path, timeout=120) as response:
                return json.load(response)
        except OSError:
            if attempt == 4:
                raise
            time.sleep(5 * (attempt + 1))
    return None


def main():
    data = sys.argv[1]
    wanted = set(sys.argv[2:])
    out = os.path.join(data, "tracker")
    os.makedirs(out, exist_ok=True)
    for entry in get("/map"):
        name = entry["map_name"]
        if wanted and name not in wanted:
            continue
        for scenario in get(f"/scenario/map/{entry['id']}"):
            path = os.path.join(out, f"{name}-{scenario['scen_type']}-{scenario['type_id']}.json")
            if os.path.exists(path):
                continue
            rows = get(f"/instance/download/scenario/{scenario['id']}")
            with open(path + ".part", "w", encoding="utf-8") as f:
                json.dump(rows, f)
            os.replace(path + ".part", path)
            print(path, len(rows), flush=True)


if __name__ == "__main__":
    main()
