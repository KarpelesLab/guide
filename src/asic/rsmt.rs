//! Rectilinear Steiner trees for pattern routing, by the iterated 1-Steiner heuristic.
//!
//! A net's pins are joined by a minimum spanning tree by Manhattan distance, and then points
//! of the Hanan grid, where the lines through the pins cross, are added while one makes the
//! spanning tree shorter, as in Kahng and Robins's batched 1-Steiner: every candidate's gain
//! is worked out against the tree, and the candidates are then taken best first, each only if
//! it still gains against the tree as it is by then. Steiner points left with two neighbours
//! or fewer are dropped, as they gain nothing. A net of three pins gets the exact tree, through
//! the median of its pins.
//!
//! A candidate's gain needs the spanning tree of the points with it added, which only differs
//! from the tree without it by edges to the candidate. Under the Manhattan distance, a point's
//! neighbours in a minimum spanning tree can be taken one to an octant around it, the nearest
//! there, so that this is the old tree's edges and eight new ones, by Kruskal's algorithm.

/// Nets with more pins than this keep their spanning tree, as the heuristic takes time of the
/// cube of the pins for each candidate round.
pub(crate) const MAX_STEINER_PINS: usize = 64;

/// How many rounds of candidates at most.
const MAX_ROUNDS: usize = 4;

type Point = (u32, u32);

/// A tree over `pins` and Steiner points: the points, `pins` first in their order and Steiner
/// points after, and the edges between them as indices.
pub(crate) fn steiner_tree(pins: &[Point]) -> (Vec<Point>, Vec<(usize, usize)>) {
    let mut points = pins.to_vec();
    let n = pins.len();
    if n <= 2 {
        let edges = if n == 2 { vec![(0, 1)] } else { Vec::new() };
        return (points, edges);
    }
    if n == 3 {
        let mut xs = [pins[0].0, pins[1].0, pins[2].0];
        let mut ys = [pins[0].1, pins[1].1, pins[2].1];
        xs.sort_unstable();
        ys.sort_unstable();
        let median = (xs[1], ys[1]);
        if let Some(at) = pins.iter().position(|&p| p == median) {
            let edges = (0..3).filter(|&i| i != at).map(|i| (at, i)).collect();
            return (points, edges);
        }
        points.push(median);
        return (points, vec![(3, 0), (3, 1), (3, 2)]);
    }
    if n > MAX_STEINER_PINS {
        let edges = spanning_tree(&points);
        return (points, edges);
    }

    let mut xs: Vec<u32> = pins.iter().map(|p| p.0).collect();
    let mut ys: Vec<u32> = pins.iter().map(|p| p.1).collect();
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    let mut sorted_pins: Vec<Point> = pins.to_vec();
    sorted_pins.sort_unstable();

    let mut tree = sorted_edges(&points, spanning_tree(&points));
    let mut length = tree_length(&tree);
    let mut scratch = Scratch::default();
    for _ in 0..MAX_ROUNDS {
        // Every candidate's gain against the tree as it is.
        let mut gains: Vec<(u64, Point)> = Vec::new();
        for &x in &xs {
            for &y in &ys {
                let c = (x, y);
                if sorted_pins.binary_search(&c).is_ok() || points[n..].contains(&c) {
                    continue;
                }
                let with = length_with(&points, &tree, c, &mut scratch);
                if with < length {
                    gains.push((length - with, c));
                }
            }
        }
        if gains.is_empty() {
            break;
        }
        gains.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut added = false;
        for &(_, c) in &gains {
            let with = length_with(&points, &tree, c, &mut scratch);
            if with < length {
                points.push(c);
                tree = std::mem::take(&mut scratch.kept);
                length = with;
                added = true;
            }
        }
        // Steiner points of degree two or less gain nothing.
        let mut degree = vec![0u32; points.len()];
        for e in &tree {
            degree[e.1] += 1;
            degree[e.2] += 1;
        }
        let before = points.len();
        let mut k = n;
        let mut keep = n;
        while k < points.len() {
            if degree[k] > 2 {
                points[keep] = points[k];
                keep += 1;
            }
            k += 1;
        }
        points.truncate(keep);
        if points.len() != before {
            tree = sorted_edges(&points, spanning_tree(&points));
            length = tree_length(&tree);
        }
        if !added {
            break;
        }
    }
    let edges = tree.into_iter().map(|(_, a, b)| (a, b)).collect();
    (points, edges)
}

/// The Manhattan distance.
#[inline]
fn distance(a: Point, b: Point) -> u64 {
    u64::from(a.0.abs_diff(b.0)) + u64::from(a.1.abs_diff(b.1))
}

/// A minimum spanning tree by Prim's algorithm from the first point, as edges from a point
/// already joined to the next. Ties go to the earlier point.
pub(crate) fn spanning_tree(points: &[Point]) -> Vec<(usize, usize)> {
    let k = points.len();
    if k < 2 {
        return Vec::new();
    }
    let mut joined = vec![false; k];
    let mut nearest: Vec<(u64, usize)> = (0..k)
        .map(|j| (distance(points[0], points[j]), 0))
        .collect();
    joined[0] = true;
    let mut out = Vec::with_capacity(k - 1);
    for _ in 1..k {
        let mut pick = usize::MAX;
        for j in 0..k {
            if !joined[j] && (pick == usize::MAX || nearest[j].0 < nearest[pick].0) {
                pick = j;
            }
        }
        joined[pick] = true;
        out.push((nearest[pick].1, pick));
        for j in 0..k {
            if !joined[j] {
                let d = distance(points[pick], points[j]);
                if d < nearest[j].0 {
                    nearest[j] = (d, pick);
                }
            }
        }
    }
    out
}

/// Edges with their lengths, shortest first.
fn sorted_edges(points: &[Point], edges: Vec<(usize, usize)>) -> Vec<(u64, usize, usize)> {
    let mut out: Vec<(u64, usize, usize)> = edges
        .into_iter()
        .map(|(a, b)| (distance(points[a], points[b]), a, b))
        .collect();
    out.sort_unstable();
    out
}

fn tree_length(tree: &[(u64, usize, usize)]) -> u64 {
    tree.iter().map(|e| e.0).sum()
}

#[derive(Default)]
struct Scratch {
    parent: Vec<usize>,
    new_edges: Vec<(u64, usize, usize)>,
    /// The tree found by the last [`length_with`], its edges shortest first.
    kept: Vec<(u64, usize, usize)>,
}

fn find(parent: &mut [usize], mut a: usize) -> usize {
    while parent[a] != a {
        parent[a] = parent[parent[a]];
        a = parent[a];
    }
    a
}

/// The length of a minimum spanning tree of `points` and `c`, given one of `points`, `tree`,
/// whose edges are shortest first. The tree found is left in `scratch.kept`, with `c` as point
/// `points.len()`.
fn length_with(
    points: &[Point],
    tree: &[(u64, usize, usize)],
    c: Point,
    scratch: &mut Scratch,
) -> u64 {
    let m = points.len();
    // The nearest point in each octant around `c`.
    let mut best: [(u64, usize); 8] = [(u64::MAX, usize::MAX); 8];
    for (i, &p) in points.iter().enumerate() {
        let (dx, dy) = (
            i64::from(p.0) - i64::from(c.0),
            i64::from(p.1) - i64::from(c.1),
        );
        let quadrant = match (dx >= 0, dy >= 0) {
            (true, true) => 0,
            (false, true) => 2,
            (false, false) => 4,
            (true, false) => 6,
        };
        let octant = quadrant + usize::from(dx.abs() >= dy.abs());
        let d = distance(p, c);
        if d < best[octant].0 {
            best[octant] = (d, i);
        }
    }
    scratch.new_edges.clear();
    scratch.new_edges.extend(
        best.iter()
            .filter(|b| b.1 != usize::MAX)
            .map(|&(d, i)| (d, i, m)),
    );
    scratch.new_edges.sort_unstable();
    scratch.parent.clear();
    scratch.parent.extend(0..=m);
    scratch.kept.clear();
    let mut total = 0;
    let (mut i, mut j) = (0, 0);
    while scratch.kept.len() < m {
        let take_new = match (tree.get(i), scratch.new_edges.get(j)) {
            (Some(a), Some(b)) => b < a,
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (None, None) => break,
        };
        let e = if take_new {
            j += 1;
            scratch.new_edges[j - 1]
        } else {
            i += 1;
            tree[i - 1]
        };
        let (ra, rb) = (
            find(&mut scratch.parent, e.1),
            find(&mut scratch.parent, e.2),
        );
        if ra != rb {
            scratch.parent[ra] = rb;
            scratch.kept.push(e);
            total += e.0;
        }
    }
    // Merged in order, so the tree found is shortest first too.
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn length(points: &[Point], edges: &[(usize, usize)]) -> u64 {
        edges
            .iter()
            .map(|&(a, b)| distance(points[a], points[b]))
            .sum()
    }

    fn connected(points: &[Point], edges: &[(usize, usize)]) -> bool {
        let mut parent: Vec<usize> = (0..points.len()).collect();
        for &(a, b) in edges {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            parent[ra] = rb;
        }
        let root = find(&mut parent, 0);
        (0..points.len()).all(|i| find(&mut parent, i) == root)
    }

    #[test]
    fn three_pins_meet_at_their_median() {
        let pins = [(0, 0), (4, 2), (2, 5)];
        let (points, edges) = steiner_tree(&pins);
        assert_eq!(points.len(), 4);
        assert_eq!(points[3], (2, 2));
        assert_eq!(length(&points, &edges), 9);
    }

    #[test]
    fn a_cross_of_four_pins_takes_its_centre() {
        let pins = [(0, 5), (10, 5), (5, 0), (5, 10)];
        let (points, edges) = steiner_tree(&pins);
        // The spanning tree is 30 long; through the centre, 20.
        assert_eq!(length(&points, &edges), 20);
        assert!(points.contains(&(5, 5)));
        assert!(connected(&points, &edges));
        assert_eq!(edges.len(), points.len() - 1);
    }

    #[test]
    fn steiner_trees_are_never_longer_than_spanning_trees() {
        let mut rng = crate::rng::Rng::new(7);
        for n in 4..20 {
            for _ in 0..20 {
                let mut pins: Vec<Point> = Vec::new();
                while pins.len() < n {
                    let p = (rng.below(30) as u32, rng.below(30) as u32);
                    if !pins.contains(&p) {
                        pins.push(p);
                    }
                }
                let mst = length(&pins, &spanning_tree(&pins));
                let (points, edges) = steiner_tree(&pins);
                assert!(connected(&points, &edges));
                assert_eq!(edges.len(), points.len() - 1);
                assert_eq!(&points[..n], &pins[..]);
                assert!(length(&points, &edges) <= mst);
            }
        }
    }
}
