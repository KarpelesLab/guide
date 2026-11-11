//! Pattern routing: a first route for every net on the plane, quickly, before negotiation.
//!
//! Each net's pins are joined by a rectilinear Steiner tree, by the iterated 1-Steiner
//! heuristic of [`rsmt`](super::rsmt), or a minimum spanning tree, by Prim's algorithm on the
//! Manhattan distance. Each edge of that tree is laid as whichever of its two L shapes costs
//! less against what earlier nets use, with edges this net already has free, so that the Ls of
//! a net overlap where they can; or, if asked, as a Z shape, with two turns, where that costs
//! less still. Nets go shortest first. Edges that would be over capacity cost much more than
//! any detour of an L, and nearly full ones a little more, so that this spreads nets before
//! the engine negotiates.

use super::planar::PlanarSpace;
use super::rsmt;

/// What one unit over an edge's capacity costs, against 1 for an edge's length.
const OVER: f64 = 20.0;
/// What a full edge costs on top of its length, rising with the square of its use.
const FULL: f64 = 2.0;

/// The edges of each net's route, as pairs of neighbouring cells, by position in `nets`.
///
/// Each net is its pins' cells, distinct, the driver's first.
pub(crate) fn pattern_routes(
    plane: &PlanarSpace,
    nets: &[Vec<usize>],
    bend: f64,
    steiner: bool,
    zs: bool,
) -> Vec<Vec<(usize, usize)>> {
    let edges = plane.horizontal_edges() + plane.cols() * plane.rows().saturating_sub(1);
    let mut usage = vec![0u32; edges];
    // Which net last took an edge, plus one, so that a net's own edges are free to it.
    let mut owner = vec![0u32; edges];
    let mut order: Vec<usize> = (0..nets.len()).collect();
    let hpwl = |cells: &[usize]| -> usize {
        let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0, usize::MAX, 0);
        for &c in cells {
            let (x, y) = plane.cell_xy(c);
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
        }
        (x1 - x0) + (y1 - y0)
    };
    order.sort_by_key(|&n| (hpwl(&nets[n]), n));
    let mut out = vec![Vec::new(); nets.len()];
    let mut path = Vec::new();
    let mut best = Vec::new();
    for &n in &order {
        let stamp = n as u32 + 1;
        let mut taken: Vec<(usize, usize)> = Vec::new();
        let cost = |e: usize, owner: &[u32], usage: &[u32]| -> f64 {
            if owner[e] == stamp {
                return 0.0;
            }
            let cap = f64::from(plane.edge_capacity(e));
            let after = f64::from(usage[e] + 1);
            if after > cap {
                1.0 + FULL + OVER * (after - cap).min(1.0)
            } else {
                let r = after / cap;
                1.0 + FULL * r * r
            }
        };
        let links: Vec<(usize, usize)> = if steiner {
            let xy: Vec<(u32, u32)> = nets[n]
                .iter()
                .map(|&c| {
                    let (x, y) = plane.cell_xy(c);
                    (x as u32, y as u32)
                })
                .collect();
            let (points, edges) = rsmt::steiner_tree(&xy);
            let cell = |i: usize| plane.cell_at(points[i].0 as usize, points[i].1 as usize);
            edges.into_iter().map(|(a, b)| (cell(a), cell(b))).collect()
        } else {
            spanning_tree(plane, &nets[n])
        };
        for (a, b) in links {
            let (ax, ay) = plane.cell_xy(a);
            let (bx, by) = plane.cell_xy(b);
            let mut best_cost = f64::INFINITY;
            // Along the row first, then along the column; and the other way round. Then, if
            // asked, the Z shapes: along the row to a column between, along it, and on along
            // the row; and the same turned round.
            let mut shapes: Vec<[usize; 2]> = vec![
                [plane.cell_at(bx, ay), plane.cell_at(bx, ay)],
                [plane.cell_at(ax, by), plane.cell_at(ax, by)],
            ];
            if ax == bx || ay == by {
                shapes.truncate(1);
            } else if zs {
                for xm in ax.min(bx) + 1..ax.max(bx) {
                    shapes.push([plane.cell_at(xm, ay), plane.cell_at(xm, by)]);
                }
                for ym in ay.min(by) + 1..ay.max(by) {
                    shapes.push([plane.cell_at(ax, ym), plane.cell_at(bx, ym)]);
                }
            }
            for (k, [c1, c2]) in shapes.into_iter().enumerate() {
                path.clear();
                walk(plane, a, c1, &mut path);
                walk(plane, c1, c2, &mut path);
                walk(plane, c2, b, &mut path);
                let mut total: f64 = path
                    .iter()
                    .map(|&(c, d)| cost(plane.edge(c, d).unwrap_or(0), &owner, &usage))
                    .sum();
                if ax != bx && ay != by {
                    total += if k < 2 { bend } else { 2.0 * bend };
                }
                if total < best_cost {
                    best_cost = total;
                    core::mem::swap(&mut best, &mut path);
                }
            }
            for &(c, d) in &best {
                let Some(e) = plane.edge(c, d) else { continue };
                if owner[e] != stamp {
                    owner[e] = stamp;
                    usage[e] += 1;
                    taken.push((c, d));
                }
            }
        }
        out[n] = taken;
    }
    out
}

/// Appends the steps of the straight run from `a` to `b`, which share a row or a column.
fn walk(plane: &PlanarSpace, a: usize, b: usize, out: &mut Vec<(usize, usize)>) {
    let (ax, ay) = plane.cell_xy(a);
    let (bx, by) = plane.cell_xy(b);
    let (mut x, mut y) = (ax, ay);
    while x != bx {
        let nx = if bx > x { x + 1 } else { x - 1 };
        out.push((plane.cell_at(x, y), plane.cell_at(nx, y)));
        x = nx;
    }
    while y != by {
        let ny = if by > y { y + 1 } else { y - 1 };
        out.push((plane.cell_at(x, y), plane.cell_at(x, ny)));
        y = ny;
    }
}

/// A minimum spanning tree of the cells by Manhattan distance, as edges from a cell already
/// joined to the next, grown from the first, by Prim's algorithm. Ties go to the earlier cell.
fn spanning_tree(plane: &PlanarSpace, cells: &[usize]) -> Vec<(usize, usize)> {
    let k = cells.len();
    let xy: Vec<(usize, usize)> = cells.iter().map(|&c| plane.cell_xy(c)).collect();
    let dist = |i: usize, j: usize| xy[i].0.abs_diff(xy[j].0) + xy[i].1.abs_diff(xy[j].1);
    let mut joined = vec![false; k];
    let mut nearest: Vec<(usize, usize)> = (0..k).map(|j| (dist(0, j), 0)).collect();
    joined[0] = true;
    let mut out = Vec::with_capacity(k.saturating_sub(1));
    for _ in 1..k {
        let mut pick = usize::MAX;
        for j in 0..k {
            if !joined[j] && (pick == usize::MAX || nearest[j].0 < nearest[pick].0) {
                pick = j;
            }
        }
        joined[pick] = true;
        out.push((cells[nearest[pick].1], cells[pick]));
        for j in 0..k {
            if !joined[j] {
                let d = dist(pick, j);
                if d < nearest[j].0 {
                    nearest[j] = (d, pick);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asic::format::read_design;

    /// A plane of 6 by 5 cells, with `tracks` on every edge but the given horizontal ones,
    /// which have one.
    fn plane(tracks: u32, narrow: &[(u32, u32)]) -> PlanarSpace {
        let mut text = format!(
            "grid 6 5 2\nvertical capacity 0 {c}\nhorizontal capacity {c} 0\n\
             minimum width 1 1\nminimum spacing 1 1\nvia spacing 1 1\n0 0 10 10\nnum net 0\n{}\n",
            narrow.len(),
            c = 2 * tracks
        );
        for &(x, y) in narrow {
            text.push_str(&format!("{x} {y} 1 {} {y} 1 2\n", x + 1));
        }
        let design = read_design(text.as_bytes()).unwrap();
        PlanarSpace::new(&design.capacities(), |_, c| c / 2)
    }

    fn length_and_cells(plane: &PlanarSpace, links: &[(usize, usize)]) -> (usize, Vec<usize>) {
        let mut cells: Vec<usize> = links.iter().flat_map(|&(a, b)| [a, b]).collect();
        cells.sort_unstable();
        cells.dedup();
        for &(a, b) in links {
            assert!(plane.edge(a, b).is_some(), "{a} and {b} are not neighbours");
        }
        (links.len(), cells)
    }

    #[test]
    fn a_net_along_a_row_and_a_column_is_as_short_as_it_can_be() {
        let p = plane(4, &[]);
        let net = vec![p.cell_at(0, 0), p.cell_at(4, 0), p.cell_at(4, 3)];
        let routes = pattern_routes(&p, core::slice::from_ref(&net), 1.0, false, false);
        let (length, cells) = length_and_cells(&p, &routes[0]);
        assert_eq!(length, 7);
        assert!(net.iter().all(|c| cells.contains(c)));
    }

    #[test]
    fn an_l_takes_the_corner_with_room() {
        // Row 0 from x = 1 to 2 has one track, which the first net takes.
        let p = plane(4, &[(1, 0)]);
        let nets = vec![
            vec![p.cell_at(1, 0), p.cell_at(2, 0)],
            vec![p.cell_at(0, 0), p.cell_at(4, 2)],
        ];
        let routes = pattern_routes(&p, &nets, 1.0, false, false);
        assert_eq!(routes[0], [(p.cell_at(1, 0), p.cell_at(2, 0))]);
        // The second net goes up first and along row 2, not along row 0.
        let (length, cells) = length_and_cells(&p, &routes[1]);
        assert_eq!(length, 6);
        assert!(cells.contains(&p.cell_at(0, 2)));
        assert!(!cells.contains(&p.cell_at(2, 0)));
    }

    #[test]
    fn a_z_shape_goes_round_where_both_ls_are_full() {
        // Row 0 from x = 1 to 2 and row 2 from x = 2 to 3 have one track each, which the
        // first two nets take, and both Ls of the third would cross one of them.
        let p = plane(4, &[(1, 0), (2, 2)]);
        let nets = vec![
            vec![p.cell_at(1, 0), p.cell_at(2, 0)],
            vec![p.cell_at(2, 2), p.cell_at(3, 2)],
            vec![p.cell_at(0, 0), p.cell_at(4, 2)],
        ];
        let routes = pattern_routes(&p, &nets, 1.0, false, true);
        let (length, cells) = length_and_cells(&p, &routes[2]);
        assert_eq!(length, 6);
        // Up a row, along it, and up again.
        assert!(cells.contains(&p.cell_at(2, 1)));
        assert!(!cells.contains(&p.cell_at(2, 0)) && !cells.contains(&p.cell_at(2, 2)));
        // Without Z shapes, it takes an L over a full edge.
        let routes = pattern_routes(&p, &nets, 1.0, false, false);
        let (length, cells) = length_and_cells(&p, &routes[2]);
        assert_eq!(length, 6);
        assert!(!cells.contains(&p.cell_at(2, 1)));
    }

    #[test]
    fn the_spanning_tree_joins_each_pin_to_the_nearest_already_joined() {
        let p = plane(4, &[]);
        let cells = [
            p.cell_at(0, 0),
            p.cell_at(5, 4),
            p.cell_at(1, 0),
            p.cell_at(5, 3),
        ];
        let tree = spanning_tree(&p, &cells);
        assert_eq!(
            tree,
            [
                (p.cell_at(0, 0), p.cell_at(1, 0)),
                (p.cell_at(1, 0), p.cell_at(5, 3)),
                (p.cell_at(5, 3), p.cell_at(5, 4)),
            ]
        );
    }
}
