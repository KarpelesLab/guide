#!/bin/sh
# Downloads the MovingAI MAPF maps and random scenarios (ODC-By 1.0) into DIR.
DIR=${1:?usage: fetch.sh DIR}
mkdir -p "$DIR" && cd "$DIR" || exit 1
for f in mapf-map.zip mapf-scen-random.zip mapf-scen-even.zip; do
  [ -f "$f" ] || curl -fsSLO "https://movingai.com/benchmarks/mapf/$f" || exit 1
done
mkdir -p maps scen && unzip -qo mapf-map.zip -d maps && unzip -qo mapf-scen-random.zip -d scen && unzip -qo mapf-scen-even.zip -d scen
