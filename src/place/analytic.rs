//! Analytic placement after SimPL: a quadratic lower bound and a spread upper bound that pull
//! towards each other, then legalization and detailed placement.

use core::sync::atomic::{AtomicBool, Ordering};

use super::anneal::{AnnealOptions, Annealer, PlaceOutcome, PlaceStop};
use super::cost::CostTerm;
use super::legalize::{legalize_units, unit_box};
use super::netlist::{NO_UNIT, Netlist};
use super::problem::rotate;
use super::{Fixed, Layout, PlaceError, PlacementProblem, Point, Rect};
use crate::id::NetId;
use crate::rng::Rng;

/// How [`place_analytic`] runs.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct AnalyticOptions<'c> {
    /// The seed for the starting jitter and for detailed placement. The default is 1.
    pub seed: u64,
    /// How many solves, with bound-to-bound weights from the last, come before spreading
    /// starts. The default is 5.
    pub wirelength_iterations: u32,
    /// The most iterations of solving and spreading. The default is 50.
    pub max_iterations: u32,
    /// Stop when the upper bound's wirelength is within this fraction of the lower bound's.
    /// The default is 0.1, as in SimPL.
    pub stop_gap: f64,
    /// The weight of the pseudo-nets to the upper bound, in the first iteration, against 1 for
    /// a net. The default is 0.01, as in SimPL.
    pub anchor_weight: f64,
    /// What that weight is multiplied by after each iteration. The default is 1.5.
    pub anchor_growth: f64,
    /// The most conjugate-gradient iterations per solve. The default is 200.
    pub cg_iterations: u32,
    /// Stop a solve when the residual is this fraction of the right-hand side. The default is
    /// 1e-6.
    pub cg_tolerance: f64,
    /// Options for detailed placement after legalization, or `None` to skip it. The default is
    /// [`AnnealOptions::detailed`]. A cancel flag here defaults to the one above.
    pub detailed: Option<AnnealOptions<'c>>,
    /// A flag that stops the run once set. Global placement stops at the next iteration and
    /// the rest goes on from there, so the result is still legal. The default is none.
    pub cancel: Option<&'c AtomicBool>,
}

impl Default for AnalyticOptions<'_> {
    fn default() -> Self {
        AnalyticOptions {
            seed: 1,
            wirelength_iterations: 5,
            max_iterations: 50,
            stop_gap: 0.1,
            anchor_weight: 0.01,
            anchor_growth: 1.5,
            cg_iterations: 200,
            cg_tolerance: 1e-6,
            detailed: Some(AnnealOptions::detailed()),
            cancel: None,
        }
    }
}

/// What global placement found, along the way to the result.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct AnalyticReport {
    /// Iterations of solving and spreading.
    pub iterations: u32,
    /// The wirelength of the last lower bound, which is not legal, so it can be below the
    /// optimum.
    pub lower_bound: f64,
    /// The wirelength of the last upper bound, which went to legalization.
    pub upper_bound: f64,
    /// The wirelength after legalization, before detailed placement.
    pub legalized: u64,
}

/// One pin as the solver sees it: on a unit, at an offset from its anchor, or fixed.
#[derive(Clone, Copy, Debug)]
struct SolvePin {
    unit: u32,
    at: [f64; 2],
}

/// A sparse symmetric matrix in compressed rows, with the diagonal kept apart.
struct Matrix {
    diagonal: Vec<f64>,
    row_start: Vec<u32>,
    columns: Vec<u32>,
    values: Vec<f64>,
}

impl Matrix {
    fn multiply(&self, x: &[f64], out: &mut [f64]) {
        for (row, o) in out.iter_mut().enumerate() {
            let mut sum = self.diagonal[row] * x[row];
            for k in self.row_start[row] as usize..self.row_start[row + 1] as usize {
                sum += self.values[k] * x[self.columns[k] as usize];
            }
            *o = sum;
        }
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut sum = 0.0;
    for (x, y) in a.iter().zip(b) {
        sum += x * y;
    }
    sum
}

/// Solves `matrix * x = rhs` by preconditioned conjugate gradients, from the `x` given.
fn conjugate_gradient(
    matrix: &Matrix,
    rhs: &[f64],
    x: &mut [f64],
    iterations: u32,
    tolerance: f64,
) {
    let n = rhs.len();
    let mut r = vec![0.0; n];
    matrix.multiply(x, &mut r);
    for i in 0..n {
        r[i] = rhs[i] - r[i];
    }
    let target = tolerance * dot(rhs, rhs).sqrt();
    let precondition = |r: &[f64], z: &mut [f64]| {
        for i in 0..n {
            z[i] = r[i] / matrix.diagonal[i];
        }
    };
    let mut z = vec![0.0; n];
    precondition(&r, &mut z);
    let mut p = z.clone();
    let mut rz = dot(&r, &z);
    let mut ap = vec![0.0; n];
    for _ in 0..iterations {
        if dot(&r, &r).sqrt() <= target {
            break;
        }
        matrix.multiply(&p, &mut ap);
        let pap = dot(&p, &ap);
        if pap <= 0.0 {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        precondition(&r, &mut z);
        let next = dot(&r, &z);
        let beta = next / rz;
        rz = next;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
    }
}

/// The problem as the solver sees it.
struct Model<'p> {
    problem: &'p PlacementProblem,
    netlist: Netlist,
    pins: Vec<SolvePin>,
    nets: Vec<(u32, u32, f64)>,
    /// The smallest distance a bound-to-bound weight divides by.
    epsilon: f64,
    rotations: Vec<i32>,
}

impl<'p> Model<'p> {
    fn new(problem: &'p PlacementProblem, netlist: Netlist) -> Model<'p> {
        let mut offsets = vec![[0.0; 2]; problem.items().len()];
        for unit in &netlist.units {
            for &(item, offset) in &unit.members {
                offsets[item.index()] = [offset.x as f64, offset.y as f64];
            }
        }
        let start = super::Placement::new(problem);
        let rotations: Vec<i32> = netlist
            .units
            .iter()
            .map(|unit| problem.item(unit.members[0].0).rotations[0])
            .collect();
        let mut pins = Vec::new();
        let mut nets = Vec::new();
        for net in 0..problem.net_count() {
            let net = NetId(net as u32);
            let from = pins.len() as u32;
            for &pin in problem.net_pins(net) {
                let item = pin.item.index();
                let unit = netlist.unit_of[item];
                if unit == NO_UNIT {
                    let at = problem.pin_position(&start, pin);
                    pins.push(SolvePin {
                        unit,
                        at: [at.x as f64, at.y as f64],
                    });
                } else {
                    let r = problem.item(pin.item).rotations[0];
                    let o = rotate(pin.offset, r);
                    pins.push(SolvePin {
                        unit,
                        at: [offsets[item][0] + o.x as f64, offsets[item][1] + o.y as f64],
                    });
                }
            }
            let to = pins.len() as u32;
            if to - from >= 2 {
                nets.push((from, to, f64::from(problem.net_weight(net))));
            }
        }
        let epsilon = match problem.layout() {
            Layout::FreeSpace { pitch, .. } => *pitch as f64,
            Layout::Sites(_) => netlist
                .grids
                .iter()
                .map(|g| {
                    let span = (g.xs[g.xs.len() - 1] - g.xs[0]).max(g.ys[g.ys.len() - 1] - g.ys[0]);
                    span as f64 / (g.xs.len().max(g.ys.len()).max(2) - 1) as f64
                })
                .fold(f64::INFINITY, f64::min)
                .clamp(1e-3, f64::MAX),
        };
        Model {
            problem,
            netlist,
            pins,
            nets,
            epsilon,
            rotations,
        }
    }

    fn position(&self, pin: &SolvePin, anchors: &[[f64; 2]], axis: usize) -> f64 {
        if pin.unit == NO_UNIT {
            pin.at[axis]
        } else {
            anchors[pin.unit as usize][axis] + pin.at[axis]
        }
    }

    /// The half-perimeter wirelength with units' anchors at `anchors`.
    fn wirelength(&self, anchors: &[[f64; 2]]) -> f64 {
        let mut total = 0.0;
        for &(from, to, weight) in &self.nets {
            let pins = &self.pins[from as usize..to as usize];
            let mut span = 0.0;
            for axis in 0..2 {
                let mut low = f64::INFINITY;
                let mut high = f64::NEG_INFINITY;
                for pin in pins {
                    let v = self.position(pin, anchors, axis);
                    low = low.min(v);
                    high = high.max(v);
                }
                span += high - low;
            }
            total += weight * span;
        }
        total
    }

    /// Solves one axis with bound-to-bound weights from `anchors`, and pseudo-nets to
    /// `targets` of weight `alpha` if given, starting from and writing to `anchors`.
    fn solve(
        &self,
        axis: usize,
        anchors: &mut [[f64; 2]],
        targets: Option<(&[[f64; 2]], f64)>,
        options: &AnalyticOptions<'_>,
    ) {
        let n = self.netlist.units.len();
        let mut diagonal = vec![0.0; n];
        let mut rhs = vec![0.0; n];
        let mut entries: Vec<(u32, u32, f64)> = Vec::new();
        let eps = self.epsilon;
        let mut connect = |a: &SolvePin, b: &SolvePin, xa: f64, xb: f64, w: f64| {
            let w = w / (xa - xb).abs().max(eps);
            match (a.unit == NO_UNIT, b.unit == NO_UNIT) {
                (true, true) => {}
                (false, true) => {
                    diagonal[a.unit as usize] += w;
                    rhs[a.unit as usize] += w * (xb - a.at[axis]);
                }
                (true, false) => {
                    diagonal[b.unit as usize] += w;
                    rhs[b.unit as usize] += w * (xa - b.at[axis]);
                }
                (false, false) => {
                    if a.unit == b.unit {
                        return;
                    }
                    diagonal[a.unit as usize] += w;
                    diagonal[b.unit as usize] += w;
                    entries.push((a.unit, b.unit, -w));
                    entries.push((b.unit, a.unit, -w));
                    rhs[a.unit as usize] += w * (b.at[axis] - a.at[axis]);
                    rhs[b.unit as usize] += w * (a.at[axis] - b.at[axis]);
                }
            }
        };
        for &(from, to, weight) in &self.nets {
            let pins = &self.pins[from as usize..to as usize];
            let count = pins.len();
            let mut low = 0;
            let mut high = 0;
            let mut values = Vec::with_capacity(count);
            for (k, pin) in pins.iter().enumerate() {
                let v = self.position(pin, anchors, axis);
                values.push(v);
                if v < values[low] {
                    low = k;
                }
                if v > values[high] {
                    high = k;
                }
            }
            if low == high {
                high = if low == 0 { 1 } else { 0 };
            }
            let w = weight * 2.0 / (count - 1) as f64;
            connect(&pins[low], &pins[high], values[low], values[high], w);
            for k in 0..count {
                if k == low || k == high {
                    continue;
                }
                connect(&pins[k], &pins[low], values[k], values[low], w);
                connect(&pins[k], &pins[high], values[k], values[high], w);
            }
        }
        // Pseudo-nets to the upper bound, or a faint pull to where each unit is, so that a
        // unit with no path to a fixed pin doesn't make the system singular.
        let mut largest = 0.0f64;
        for &d in &diagonal {
            largest = largest.max(d);
        }
        let faint = 1e-6 * largest.max(1.0);
        for u in 0..n {
            let (target, w) = match targets {
                Some((targets, alpha)) => {
                    let t = targets[u][axis];
                    (t, alpha / (anchors[u][axis] - t).abs().max(eps))
                }
                None => (anchors[u][axis], faint),
            };
            let w = if diagonal[u] == 0.0 { w.max(faint) } else { w };
            diagonal[u] += w;
            rhs[u] += w * target;
        }
        entries.sort_by_key(|&(r, c, _)| (r, c));
        let mut row_start = vec![0u32; n + 1];
        let mut columns = Vec::with_capacity(entries.len());
        let mut values: Vec<f64> = Vec::with_capacity(entries.len());
        let mut last: Option<(u32, u32)> = None;
        for &(r, c, v) in &entries {
            if last == Some((r, c)) {
                if let Some(value) = values.last_mut() {
                    *value += v;
                }
            } else {
                columns.push(c);
                values.push(v);
                row_start[r as usize + 1] = columns.len() as u32;
                last = Some((r, c));
            }
        }
        for r in 0..n {
            row_start[r + 1] = row_start[r + 1].max(row_start[r]);
        }
        let matrix = Matrix {
            diagonal,
            row_start,
            columns,
            values,
        };
        let mut x: Vec<f64> = anchors.iter().map(|a| a[axis]).collect();
        conjugate_gradient(
            &matrix,
            &rhs,
            &mut x,
            options.cg_iterations,
            options.cg_tolerance,
        );
        for (a, v) in anchors.iter_mut().zip(x) {
            a[axis] = v;
        }
    }

    fn solve_both(
        &self,
        anchors: &mut [[f64; 2]],
        targets: Option<(&[[f64; 2]], f64)>,
        options: &AnalyticOptions<'_>,
    ) {
        self.solve(0, anchors, targets, options);
        self.solve(1, anchors, targets, options);
    }

    /// The upper bound: `anchors` spread out.
    fn spread(&self, anchors: &[[f64; 2]]) -> Vec<[f64; 2]> {
        let mut out = anchors.to_vec();
        let problem = self.problem;
        match problem.layout() {
            Layout::Sites(sites) => {
                let mut fixed = vec![false; sites.len()];
                for item in problem.items() {
                    if let Some(Fixed::Site(site)) = item.fixed {
                        fixed[site.index()] = true;
                    }
                }
                for &kind in &self.netlist.kinds {
                    let mut units: Vec<Body> = Vec::new();
                    for (u, unit) in self.netlist.units.iter().enumerate() {
                        let (first, offset) = unit.members[0];
                        if problem.item(first).kind != kind {
                            continue;
                        }
                        units.push(Body {
                            unit: u as u32,
                            at: [
                                anchors[u][0] + offset.x as f64,
                                anchors[u][1] + offset.y as f64,
                            ],
                            size: unit.members.len() as f64,
                        });
                    }
                    let mut free: Vec<[f64; 2]> = sites
                        .iter()
                        .enumerate()
                        .filter(|&(s, site)| site.kind == kind && !fixed[s])
                        .map(|(_, site)| [site.at.x as f64, site.at.y as f64])
                        .collect();
                    bisect_sites(&mut units, &mut free, &mut out);
                    for body in &units {
                        let offset = self.netlist.units[body.unit as usize].members[0].1;
                        let o = &mut out[body.unit as usize];
                        o[0] -= offset.x as f64;
                        o[1] -= offset.y as f64;
                    }
                }
            }
            Layout::FreeSpace { area, .. } => {
                let mut obstacles = Vec::new();
                let placement = super::Placement::new(problem);
                for (i, item) in problem.items().iter().enumerate() {
                    if item.fixed.is_some() {
                        let id = super::ItemId(i as u32);
                        obstacles.push(problem.outline_at(
                            id,
                            placement.origins[i],
                            placement.rotations[i],
                        ));
                    }
                }
                let mut units: Vec<Body> = Vec::new();
                let mut boxes = Vec::new();
                for (u, unit) in self.netlist.units.iter().enumerate() {
                    let bounds = unit_box(problem, unit, self.rotations[u]);
                    let centre = [
                        anchors[u][0] + (bounds.min.x + bounds.max.x) as f64 / 2.0,
                        anchors[u][1] + (bounds.min.y + bounds.max.y) as f64 / 2.0,
                    ];
                    units.push(Body {
                        unit: u as u32,
                        at: centre,
                        size: bounds.width() as f64 * bounds.height() as f64,
                    });
                    boxes.push(bounds);
                }
                let region = [
                    [area.min.x as f64, area.min.y as f64],
                    [area.max.x as f64, area.max.y as f64],
                ];
                let mut centres = vec![[0.0; 2]; self.netlist.units.len()];
                bisect_area(&mut units, region, &obstacles, &boxes, &mut centres);
                for (u, bounds) in boxes.iter().enumerate() {
                    out[u] = [
                        centres[u][0] - (bounds.min.x + bounds.max.x) as f64 / 2.0,
                        centres[u][1] - (bounds.min.y + bounds.max.y) as f64 / 2.0,
                    ];
                }
            }
        }
        out
    }
}

/// A unit being spread: its position, of its first member on sites or its centre in free
/// space, and how much room it takes.
#[derive(Clone, Copy, Debug)]
struct Body {
    unit: u32,
    at: [f64; 2],
    size: f64,
}

fn sort_bodies(bodies: &mut [Body], axis: usize) {
    bodies.sort_by(|a, b| {
        a.at[axis]
            .total_cmp(&b.at[axis])
            .then(a.at[1 - axis].total_cmp(&b.at[1 - axis]))
            .then(a.unit.cmp(&b.unit))
    });
}

/// How many of `bodies`, taken in order, go to the side with `share` of the room.
fn split_point(bodies: &[Body], share: f64) -> usize {
    let total: f64 = bodies.iter().fold(0.0, |s, b| s + b.size);
    let want = total * share;
    let mut sum = 0.0;
    let mut k = 0;
    while k < bodies.len() {
        let next = sum + bodies[k].size;
        // Stop where the cumulative size is nearest to what the side should hold.
        if next > want && next - want > want - sum {
            break;
        }
        sum = next;
        k += 1;
    }
    k
}

/// Spreads bodies over free sites by recursive bisection, writing each body's target.
fn bisect_sites(bodies: &mut [Body], sites: &mut [[f64; 2]], out: &mut [[f64; 2]]) {
    if bodies.is_empty() {
        return;
    }
    if sites.is_empty() {
        for b in bodies.iter() {
            out[b.unit as usize] = b.at;
        }
        return;
    }
    if bodies.len() == 1 || sites.len() == 1 {
        for b in bodies.iter() {
            let mut best = sites[0];
            let mut best_d = f64::INFINITY;
            for s in sites.iter() {
                let d = (s[0] - b.at[0]).abs() + (s[1] - b.at[1]).abs();
                if d < best_d {
                    best_d = d;
                    best = *s;
                }
            }
            out[b.unit as usize] = best;
        }
        return;
    }
    let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for s in sites.iter() {
        for a in 0..2 {
            low[a] = low[a].min(s[a]);
            high[a] = high[a].max(s[a]);
        }
    }
    // Cut across the longer side, between two distinct coordinates, as near the middle of the
    // sites as can be, so that both parts stay rectangles of whole columns or rows.
    let longer = usize::from(high[1] - low[1] > high[0] - low[0]);
    let mut cut = None;
    for axis in [longer, 1 - longer] {
        sites.sort_by(|a, b| {
            a[axis]
                .total_cmp(&b[axis])
                .then(a[1 - axis].total_cmp(&b[1 - axis]))
        });
        let half = sites.len() / 2;
        let mut best: Option<usize> = None;
        for k in 1..sites.len() {
            if sites[k - 1][axis] < sites[k][axis]
                && best.is_none_or(|b| k.abs_diff(half) < b.abs_diff(half))
            {
                best = Some(k);
            }
        }
        if let Some(k) = best {
            cut = Some((axis, k));
            break;
        }
    }
    let Some((axis, middle)) = cut else {
        // Every site is at the same point.
        for b in bodies.iter() {
            out[b.unit as usize] = sites[0];
        }
        return;
    };
    sort_bodies(bodies, axis);
    let k = split_point(bodies, middle as f64 / sites.len() as f64);
    let (left_sites, right_sites) = sites.split_at_mut(middle);
    let (left, right) = bodies.split_at_mut(k);
    bisect_sites(left, left_sites, out);
    bisect_sites(right, right_sites, out);
}

/// The room in `region` that obstacles leave.
fn room(region: [[f64; 2]; 2], obstacles: &[Rect]) -> f64 {
    let mut free = (region[1][0] - region[0][0]) * (region[1][1] - region[0][1]);
    for o in obstacles {
        let w = (region[1][0].min(o.max.x as f64) - region[0][0].max(o.min.x as f64)).max(0.0);
        let h = (region[1][1].min(o.max.y as f64) - region[0][1].max(o.min.y as f64)).max(0.0);
        free -= w * h;
    }
    free.max(0.0)
}

/// Spreads bodies over free space by recursive bisection, writing each body's centre.
fn bisect_area(
    bodies: &mut [Body],
    region: [[f64; 2]; 2],
    obstacles: &[Rect],
    boxes: &[Rect],
    out: &mut [[f64; 2]],
) {
    if bodies.is_empty() {
        return;
    }
    let width = region[1][0] - region[0][0];
    let height = region[1][1] - region[0][1];
    if bodies.len() == 1 || (width <= 1.0 && height <= 1.0) {
        for b in bodies.iter() {
            let bounds = boxes[b.unit as usize];
            let half = [bounds.width() as f64 / 2.0, bounds.height() as f64 / 2.0];
            let mut c = b.at;
            for a in 0..2 {
                let (lo, hi) = (region[0][a] + half[a], region[1][a] - half[a]);
                c[a] = if lo <= hi {
                    c[a].clamp(lo, hi)
                } else {
                    (region[0][a] + region[1][a]) / 2.0
                };
            }
            out[b.unit as usize] = c;
        }
        return;
    }
    let axis = usize::from(height > width);
    let cut = (region[0][axis] + region[1][axis]) / 2.0;
    let mut left = region;
    left[1][axis] = cut;
    let mut right = region;
    right[0][axis] = cut;
    let (room_left, room_right) = (room(left, obstacles), room(right, obstacles));
    let share = if room_left + room_right > 0.0 {
        room_left / (room_left + room_right)
    } else {
        0.5
    };
    sort_bodies(bodies, axis);
    let k = split_point(bodies, share);
    let (l, r) = bodies.split_at_mut(k);
    bisect_area(l, left, obstacles, boxes, out);
    bisect_area(r, right, obstacles, boxes, out);
}

/// Places by analytic global placement after SimPL, then legalization and detailed placement.
///
/// # Global placement
///
/// Each net is modelled as springs by the bound-to-bound model (Spindler, Schlichtmann and
/// Johannes, 2008): the pins at the two ends of the net's span, in each direction, are joined
/// to each other and to every other pin, with weights `2 / ((pins - 1) * distance)` from the
/// positions of the previous solve. Its quadratic cost then equals the half-perimeter at those
/// positions. Minimizing it is a sparse, symmetric, positive definite linear system per axis,
/// solved by conjugate gradients with a Jacobi preconditioner, written here with every sum in a
/// fixed order.
///
/// The solution, the **lower bound**, has items piled on each other. **Spreading** makes the
/// upper bound from it: recursive bisection, cutting the sites (of each kind separately) or
/// the free area in two, alternately across its longer side, and dividing the items, in order
/// of position, in proportion to the room on each side, until each part holds one. On sites
/// the item then takes the part's site nearest to it, so the upper bound is nearly legal; in
/// free space it stays where it was, pulled inside its part.
///
/// Each further solve adds a pseudo-net from every item to its place in the upper bound,
/// with a weight that grows each iteration, so that the two bounds converge. The loop stops
/// when the upper bound is within [`stop_gap`](AnalyticOptions::stop_gap) of the lower, or
/// after [`max_iterations`](AnalyticOptions::max_iterations).
///
/// # Then
///
/// The last upper bound goes to [`legalize`](super::legalize), and the legal result to the
/// [`Annealer`] with [`detailed`](AnnealOptions::detailed) options, at a low temperature over
/// short distances.
///
/// Spreading ignores regions; legalization enforces them.
///
/// # Errors
///
/// [`PlaceError::Problem`] if the problem is malformed, and [`PlaceError::NoRoom`] if
/// legalization finds no room for an item.
///
/// # Examples
///
/// ```
/// use guide::place::{AnalyticOptions, Wirelength, check, place_analytic, synth};
///
/// let generated = synth::grid_with_pads(6, 6, 3);
/// let (outcome, report) = place_analytic(&generated.problem, &Wirelength, AnalyticOptions::default())?;
/// assert!(check(&generated.problem, &outcome.placement).is_empty());
/// assert!(outcome.wirelength <= report.legalized);
/// assert!(outcome.wirelength >= generated.optimum);
/// # Ok::<(), guide::place::PlaceError>(())
/// ```
pub fn place_analytic<C: CostTerm>(
    problem: &PlacementProblem,
    cost: C,
    options: AnalyticOptions<'_>,
) -> Result<(PlaceOutcome, AnalyticReport), PlaceError> {
    problem.validate()?;
    let model = Model::new(problem, Netlist::new(problem));
    let units = model.netlist.units.len();
    let extent = problem.extent();
    let mut rng = Rng::new(options.seed ^ 0xA5A5_5A5A_C3C3_3C3C);
    let centre = [
        (extent.min.x + extent.max.x) as f64 / 2.0,
        (extent.min.y + extent.max.y) as f64 / 2.0,
    ];
    let jitter = [
        extent.width() as f64 / 100.0 + 1e-3,
        extent.height() as f64 / 100.0 + 1e-3,
    ];
    let mut anchors: Vec<[f64; 2]> = (0..units)
        .map(|_| {
            [
                centre[0] + (rng.unit() - 0.5) * jitter[0],
                centre[1] + (rng.unit() - 0.5) * jitter[1],
            ]
        })
        .collect();
    let cancelled = || {
        options
            .cancel
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    };

    for _ in 0..options.wirelength_iterations {
        if cancelled() {
            break;
        }
        model.solve_both(&mut anchors, None, &options);
    }
    let mut upper = model.spread(&anchors);
    let mut iterations = 0;
    let mut alpha = options.anchor_weight;
    let mut lower_wl = model.wirelength(&anchors);
    let mut upper_wl = model.wirelength(&upper);
    while iterations < options.max_iterations && !cancelled() {
        if upper_wl <= lower_wl * (1.0 + options.stop_gap) {
            break;
        }
        iterations += 1;
        model.solve_both(&mut anchors, Some((&upper, alpha)), &options);
        upper = model.spread(&anchors);
        lower_wl = model.wirelength(&anchors);
        upper_wl = model.wirelength(&upper);
        alpha *= options.anchor_growth;
    }

    let targets: Vec<Point> = upper
        .iter()
        .map(|a| Point::new(a[0].round() as i64, a[1].round() as i64))
        .collect();
    let legal = legalize_units(problem, &model.netlist, &targets, &model.rotations)?;
    let legalized = problem.wirelength(&legal);
    let report = AnalyticReport {
        iterations,
        lower_bound: lower_wl,
        upper_bound: upper_wl,
        legalized,
    };
    let outcome = match options.detailed {
        Some(mut detailed) => {
            if detailed.cancel.is_none() {
                detailed.cancel = options.cancel;
            }
            Annealer::with_netlist(problem, cost, legal, detailed, model.netlist).run()
        }
        None => {
            let annealer = Annealer::with_netlist(
                problem,
                cost,
                legal,
                AnnealOptions::default(),
                model.netlist,
            );
            let mut outcome = annealer.outcome();
            outcome.stop = PlaceStop::Converged;
            outcome
        }
    };
    Ok((outcome, report))
}
