"""Tables of LUT input rotation, from a run of scripts/vtr/rotate.sh.

    python3 scripts/vtr/rotate_tables.py WORK

By circuit and placement: LUTs rotated, the critical path that VPR's analysis finds of guide's
routes with the netlist as it was and as rotated, and of VPR's routes of each, with the ratios
of rotated over not; then the geometric means. Every route has to pass VPR's checks.
"""

import math
import os
import sys


def read(path):
    values = {}
    try:
        with open(path) as f:
            for line in f:
                if '=' in line:
                    key, value = line.rstrip('\n').split('=', 1)
                    values[key] = value
    except (FileNotFoundError, NotADirectoryError):
        pass
    return values


def geomean(ratios):
    ratios = [r for r in ratios if r]
    return math.exp(sum(math.log(r) for r in ratios) / len(ratios)) if ratios else float('nan')


def main():
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    work = sys.argv[1]
    print('| Circuit | Placement | LUTs rotated | guide\'s routes, ns | Rotated, ns | / not | '
          'VPR\'s routes, ns | Rotated, ns | / not | Wire / not, VPR | Legal |')
    print('| --- ' * 11 + '|')
    means = {}
    for circuit in sorted(os.listdir(work)):
        for placement in ('vpr', 'guide'):
            r = read(os.path.join(work, circuit, f'rotate.{placement}', 'result.txt'))
            if r.get('done') != 'yes':
                continue
            try:
                g0 = float(r['plain_guide_route_cpd_ns'])
                g1 = float(r['rotated_guide_route_cpd_ns'])
                v0 = float(r['plain_vpr_route_cpd_ns'])
                v1 = float(r['rotated_vpr_route_cpd_ns'])
                w0 = float(r['plain_vpr_route_wirelength'])
                w1 = float(r['rotated_vpr_route_wirelength'])
            except (KeyError, ValueError):
                continue
            legal = all(r.get(k) == 'yes' for k in (
                'plain_guide_route_legal', 'rotated_guide_route_legal', 'plain_vpr_route_legal',
                'rotated_vpr_route_legal'))
            m = means.setdefault(placement, ([], [], []))
            m[0].append(g1 / g0)
            m[1].append(v1 / v0)
            m[2].append(w1 / w0)
            print(f'| {circuit} | {placement} | {r.get("rotated_luts")} of '
                  f'{r.get("rotatable_luts")} | {g0:g} | {g1:g} | {g1 / g0:.3f} | {v0:g} | '
                  f'{v1:g} | {v1 / v0:.3f} | {w1 / w0:.3f} | {"yes" if legal else "no"} |')
    for placement, (g, v, w) in means.items():
        print(f'| Geomean | {placement} |  |  |  | {geomean(g):.3f} |  |  | {geomean(v):.3f} | '
              f'{geomean(w):.3f} |  |')


if __name__ == '__main__':
    main()
