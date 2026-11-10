"""Tables of placement and routing in a loop against the one-pass flow, from runs of
scripts/vtr/place.sh and scripts/vtr/loop.sh.

    python3 scripts/vtr/loop_tables.py BASE LOOP [BASE LOOP...]

Each pair is a one-pass run and the loop that started from it, as for one seed. With one pair,
this prints a table by circuit; with several, also the geometric means of each pair, and the
widths of each circuit over the pairs. Ratios are of the loop over the one-pass flow, and every
mean is over the circuits that both finished.
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


def number(values, key):
    try:
        return float(values[key])
    except (KeyError, ValueError):
        return None


def geomean(ratios):
    ratios = [r for r in ratios if r]
    if not ratios:
        return float('nan')
    return math.exp(sum(math.log(r) for r in ratios) / len(ratios))


def pair(base, loop):
    """Per circuit, the one-pass and the loop's measures."""
    rows = {}
    for circuit in sorted(os.listdir(loop)):
        result = read(os.path.join(loop, circuit, 'result.txt'))
        one = read(os.path.join(base, circuit, 'result.txt'))
        if result.get('done') != 'yes' or not one:
            continue
        legal = all(result.get(k) == 'yes' for k in (
            'guide_vpr_legal', 'guide_route_vpr_legal', 'guide_route_guide_vpr_legal',
            'guide_route_guide_min_w_vpr_legal'))
        rows[circuit] = {
            'vpr_w': number(one, 'vpr_route_vpr_min_w'),
            'one_gw': number(one, 'guide_route_guide_min_w'),
            'loop_gw': number(result, 'guide_route_guide_min_w'),
            'one_vw': number(one, 'guide_route_vpr_min_w'),
            'loop_vw': number(result, 'guide_route_vpr_min_w'),
            'one_cpd': number(one, 'guide_route_vpr_cpd_ns'),
            'loop_cpd': number(result, 'guide_route_vpr_cpd_ns'),
            'one_gcpd': number(one, 'guide_route_guide_cpd_ns'),
            'loop_gcpd': number(result, 'guide_route_guide_cpd_ns'),
            'one_wire': number(one, 'guide_route_vpr_wirelength'),
            'loop_wire': number(result, 'guide_route_vpr_wirelength'),
            'one_bb': number(one, 'guide_place_bb_estimate'),
            'loop_bb': number(result, 'guide_place_bb_estimate'),
            'place_s': number(one, 'guide_place_place_s'),
            'loop_s': number(result, 'loop_place_s'),
            'reached': result.get('loop_reached_w', 'none'),
            'legal': legal,
        }
    return rows


def ratio(row, a, b):
    if row[a] and row[b]:
        return row[b] / row[a]
    return None


def fmt(value, digits=3):
    if value is None:
        return ''
    if isinstance(value, float) and value.is_integer() and digits == 0:
        return str(int(value))
    return f'{value:.{digits}f}'


def by_circuit(rows):
    print('| Circuit | VPR\'s placement, VPR | One pass, guide | Loop, guide | / one pass | '
          'One pass, VPR | Loop, VPR | / one pass | CPD / one pass, VPR | Wire / one pass, VPR | '
          'Place, s | Loop, s | Legal |')
    print('| --- ' * 13 + '|')
    keys = ('one_gw', 'loop_gw'), ('one_vw', 'loop_vw'), ('one_cpd', 'loop_cpd'), \
        ('one_wire', 'loop_wire')
    means = {k: [] for k in keys}
    for circuit, row in rows.items():
        cells = [circuit, fmt(row['vpr_w'], 0), fmt(row['one_gw'], 0), fmt(row['loop_gw'], 0),
                 fmt(ratio(row, 'one_gw', 'loop_gw')), fmt(row['one_vw'], 0),
                 fmt(row['loop_vw'], 0), fmt(ratio(row, 'one_vw', 'loop_vw')),
                 fmt(ratio(row, 'one_cpd', 'loop_cpd')), fmt(ratio(row, 'one_wire', 'loop_wire')),
                 fmt(row['place_s'], 1), fmt(row['loop_s'], 1), 'yes' if row['legal'] else 'no']
        for k in keys:
            means[k].append(ratio(row, *k))
        print('| ' + ' | '.join(cells) + ' |')
    print('| Geomean |  |  |  | ' + fmt(geomean(means[keys[0]])) + ' |  |  | '
          + fmt(geomean(means[keys[1]])) + ' | ' + fmt(geomean(means[keys[2]])) + ' | '
          + fmt(geomean(means[keys[3]])) + ' |  |  |  |')


def summary(pairs):
    print('| Pair | Circuits | Min W / one pass, guide | Min W / one pass, VPR | '
          'One pass / VPR\'s placement, VPR | Loop / VPR\'s placement, VPR | CPD / one pass, VPR | '
          'Wire / one pass, VPR | Time, one pass + loop / one pass | All legal |')
    print('| --- ' * 10 + '|')
    for label, rows in pairs:
        g = geomean([ratio(r, 'one_gw', 'loop_gw') for r in rows.values()])
        v = geomean([ratio(r, 'one_vw', 'loop_vw') for r in rows.values()])
        a = geomean([ratio(r, 'vpr_w', 'one_vw') for r in rows.values()])
        b = geomean([ratio(r, 'vpr_w', 'loop_vw') for r in rows.values()])
        c = geomean([ratio(r, 'one_cpd', 'loop_cpd') for r in rows.values()])
        w = geomean([ratio(r, 'one_wire', 'loop_wire') for r in rows.values()])
        t = geomean([(r['place_s'] + r['loop_s']) / r['place_s']
                     for r in rows.values() if r['place_s'] and r['loop_s'] is not None])
        legal = all(r['legal'] for r in rows.values())
        print(f'| {label} | {len(rows)} | {g:.3f} | {v:.3f} | {a:.3f} | {b:.3f} | {c:.3f} | '
              f'{w:.3f} | {t:.2f} | {"yes" if legal else "no"} |')


def main():
    args = sys.argv[1:]
    if len(args) < 2 or len(args) % 2:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    pairs = []
    for k in range(0, len(args), 2):
        pairs.append((os.path.basename(args[k + 1].rstrip('/')), pair(args[k], args[k + 1])))
    if len(pairs) == 1:
        by_circuit(pairs[0][1])
    else:
        summary(pairs)
        print()
        circuits = sorted(set(c for _, rows in pairs for c in rows))
        print('| Circuit | VPR\'s placement, VPR | One pass, VPR | Loop, VPR | One pass, guide | '
              'Loop, guide |')
        print('| --- ' * 6 + '|')
        for c in circuits:
            got = [rows[c] for _, rows in pairs if c in rows]
            cell = lambda key: ', '.join(fmt(r[key], 0) for r in got)
            print(f'| {c} | {fmt(got[0]["vpr_w"], 0)} | {cell("one_vw")} | {cell("loop_vw")} | '
                  f'{cell("one_gw")} | {cell("loop_gw")} |')


if __name__ == '__main__':
    main()
