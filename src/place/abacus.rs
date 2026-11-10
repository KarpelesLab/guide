//! Legalization in free space by rows, after Abacus (Spindler, Schlichtmann and Johannes,
//! 2008): units in order of their targets, each to the row where it moves least, with the
//! units already in that row shifted along it by as little as keeps them apart.
//!
//! Rows are as tall as the tallest unit, on the pitch lattice, and broken into segments by what
//! is already placed: fixed items, and units limited to a region, which go first, greedily. In a
//! segment, units that would overlap merge into a cluster, which sits where the sum of its
//! members' squared displacements along the row is least, the mean of their targets less their
//! offsets in the cluster, rounded to the lattice and kept inside the segment. A unit goes to the
//! row where its own displacement, along the row and across it, is least.
//!
//! Where space is tight, that leaves the last units far from rows with room, so rows can also be
//! filled in order instead: units sorted across the rows, each row taking its share of their
//! width, which keeps their order across rows as Abacus keeps it along them.
//!
//! Everything is in integers, so results depend only on the problem and the targets.

use super::legalize::{nearest_free_spot, typical_size, unit_box, unit_fits};
use super::netlist::Netlist;
use super::spatial::SpatialIndex;
use super::{ItemId, Layout, Placement, PlacementProblem, Point, Rect};

/// The two ways rows can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rows {
    /// Rows along x, stacked in y.
    Horizontal,
    /// Rows along y, stacked in x.
    Vertical,
}

impl Rows {
    /// Maps a point to row coordinates, along the row first, and back, as it is its own
    /// inverse.
    fn point(self, p: Point) -> Point {
        match self {
            Rows::Horizontal => p,
            Rows::Vertical => Point::new(p.y, p.x),
        }
    }

    fn rect(self, r: &Rect) -> Rect {
        Rect {
            min: self.point(r.min),
            max: self.point(r.max),
        }
    }
}

/// A run of units that abut, from `first` to the segment's last unit or the next cluster's
/// first.
#[derive(Clone, Copy, Debug)]
struct Cluster {
    /// The index of its first unit in the segment's list.
    first: usize,
    /// How many units it holds, which weigh one each.
    count: i64,
    /// The sum over its units of their targets less their offsets in the cluster.
    q: i64,
    /// Its width.
    width: i64,
    /// Where it starts.
    x: i64,
}

/// A stretch of a row with nothing in the way.
#[derive(Clone, Debug)]
struct Segment {
    /// Its first and last coordinates along the row, both on the lattice.
    low: i64,
    high: i64,
    /// The width its units take.
    used: i64,
    /// Its units, in the order they came, with their widths.
    units: Vec<(usize, i64)>,
    clusters: Vec<Cluster>,
}

impl Segment {
    /// Where a cluster of this width whose members want `q / count` sits.
    fn position(&self, q: i64, count: i64, width: i64, pitch: i64) -> i64 {
        let x = (2 * q + count).div_euclid(2 * count);
        let steps = (x - self.low + pitch / 2).div_euclid(pitch);
        (self.low + steps * pitch).clamp(self.low, self.high - width)
    }

    /// Where a unit of `width` that wants to start at `target` would start if added, without
    /// adding it, or `None` if there is no room.
    fn trial(&self, target: i64, width: i64, pitch: i64) -> Option<i64> {
        if self.used + width > self.high - self.low {
            return None;
        }
        let (mut count, mut q, mut w) = (1, target, width);
        let mut k = self.clusters.len();
        loop {
            let x = self.position(q, count, w, pitch);
            match k.checked_sub(1).map(|j| self.clusters[j]) {
                Some(c) if c.x + c.width > x => {
                    q += c.q - count * c.width;
                    count += c.count;
                    w += c.width;
                    k -= 1;
                }
                _ => return Some(x + w - width),
            }
        }
    }

    /// Adds a unit of `width` that wants to start at `target`, which has to fit.
    fn add(&mut self, unit: usize, target: i64, width: i64, pitch: i64) {
        self.used += width;
        self.units.push((unit, width));
        let mut current = Cluster {
            first: self.units.len() - 1,
            count: 1,
            q: target,
            width,
            x: 0,
        };
        loop {
            current.x = self.position(current.q, current.count, current.width, pitch);
            match self.clusters.last() {
                Some(c) if c.x + c.width > current.x => {
                    let c = *c;
                    self.clusters.pop();
                    current = Cluster {
                        first: c.first,
                        count: c.count + current.count,
                        q: c.q + current.q - current.count * c.width,
                        width: c.width + current.width,
                        x: 0,
                    };
                }
                _ => break,
            }
        }
        self.clusters.push(current);
    }

    /// Each unit and where it starts.
    fn starts(&self) -> impl Iterator<Item = (usize, i64)> + '_ {
        self.clusters.iter().enumerate().flat_map(move |(k, c)| {
            let end = self
                .clusters
                .get(k + 1)
                .map_or(self.units.len(), |n| n.first);
            let mut x = c.x;
            self.units[c.first..end].iter().map(move |&(unit, width)| {
                let at = x;
                x += width;
                (unit, at)
            })
        })
    }
}

/// Legalizes units towards anchor targets by rows running one way, or returns `None` if some
/// unit finds no room.
///
/// Each unit goes to the row where it moves least, or, `balanced`, rows are filled in order of
/// the units' targets across them, each to its share of the room, which keeps the order of
/// units across rows when space is tight.
pub(crate) fn legalize_rows(
    problem: &PlacementProblem,
    netlist: &Netlist,
    anchors: &[Point],
    rotations: &[i32],
    rows: Rows,
    balanced: bool,
) -> Option<Placement> {
    let Layout::FreeSpace { area, pitch } = *problem.layout() else {
        return None;
    };
    let mut placement = Placement::new(problem);
    let mut index = SpatialIndex::new(area, problem.items().len(), typical_size(problem));
    let mut obstacles = Vec::new();
    for (i, item) in problem.items().iter().enumerate() {
        if item.fixed.is_some() {
            let id = ItemId(i as u32);
            let outline = problem.outline_at(id, placement.origins[i], placement.rotations[i]);
            index.insert(id.0, outline);
            obstacles.push(outline);
        }
    }
    let rotation_of = |u: usize, item: ItemId| {
        if netlist.units[u].is_macro {
            problem.item(item).rotations[0]
        } else {
            rotations[u]
        }
    };
    // Units limited to a region go first, greedily, smallest region first.
    let room = |u: usize| {
        netlist.units[u]
            .members
            .iter()
            .filter_map(|&(item, _)| problem.item(item).region)
            .map(|region| problem.regions()[region.index()].area())
            .min()
    };
    let mut limited: Vec<(u64, usize)> = (0..netlist.units.len())
        .filter_map(|u| room(u).map(|r| (r, u)))
        .collect();
    limited.sort_unstable();
    let place = |placement: &mut Placement, index: &mut SpatialIndex, u: usize, at: Point| {
        let unit = &netlist.units[u];
        if !unit_fits(problem, unit, rotations[u], at, &area, index) {
            return None;
        }
        let mut outlines = Vec::with_capacity(unit.members.len());
        for &(item, offset) in &unit.members {
            let rotation = rotation_of(u, item);
            let origin = at.offset(offset);
            placement.set_origin(item, origin, rotation);
            let outline = problem.outline_at(item, origin, rotation);
            index.insert(item.0, outline);
            outlines.push(outline);
        }
        Some(outlines)
    };
    for &(_, u) in &limited {
        let unit = &netlist.units[u];
        let at = nearest_free_spot(problem, unit, rotations[u], anchors[u], area, pitch, &index)?;
        obstacles.extend(place(&mut placement, &mut index, u, at)?);
    }

    // The rest go by rows, in row coordinates.
    let free: Vec<usize> = (0..netlist.units.len())
        .filter(|&u| room(u).is_none())
        .collect();
    if free.is_empty() {
        return Some(placement);
    }
    let lattice = |v: i64| v.div_euclid(pitch) * pitch;
    let row_area = rows.rect(&area);
    let origin = row_area.min;
    let boxes: Vec<Rect> = free
        .iter()
        .map(|&u| rows.rect(&unit_box(problem, &netlist.units[u], rotations[u])))
        .collect();
    let widths: Vec<i64> = boxes
        .iter()
        .map(|b| (b.width() as i64 + pitch - 1).div_euclid(pitch) * pitch)
        .collect();
    let tallest = boxes.iter().map(|b| b.height() as i64).max().unwrap_or(0);
    let height = ((tallest + pitch - 1).div_euclid(pitch) * pitch).max(pitch);
    let row_count = (row_area.height() as i64 / height) as usize;
    if row_count == 0 {
        return None;
    }
    let obstacles: Vec<Rect> = obstacles.iter().map(|o| rows.rect(o)).collect();
    let mut segments: Vec<Vec<Segment>> = (0..row_count)
        .map(|r| {
            let (y0, y1) = (
                origin.y + r as i64 * height,
                origin.y + (r as i64 + 1) * height,
            );
            let mut blocked: Vec<(i64, i64)> = obstacles
                .iter()
                .filter(|o| o.min.y < y1 && o.max.y > y0 && o.width() > 0 && o.height() > 0)
                .map(|o| (o.min.x, o.max.x))
                .collect();
            blocked.sort_unstable();
            let mut out = Vec::new();
            let mut start = row_area.min.x;
            let mut push = |low: i64, high: i64| {
                // On the lattice, inside what is free.
                let low = origin.x + lattice(low - origin.x + pitch - 1);
                let high = origin.x + lattice(high - origin.x);
                if high > low {
                    out.push(Segment {
                        low,
                        high,
                        used: 0,
                        units: Vec::new(),
                        clusters: Vec::new(),
                    });
                }
            };
            for (a, b) in blocked {
                if a > start {
                    push(start, a.min(row_area.max.x));
                }
                start = start.max(b);
            }
            if start < row_area.max.x {
                push(start, row_area.max.x);
            }
            out
        })
        .collect();

    // Where each unit's box wants to start, along and across the rows.
    let targets: Vec<Point> = free
        .iter()
        .zip(&boxes)
        .map(|(&u, b)| rows.point(anchors[u]).offset(b.min))
        .collect();
    // Balanced, each unit's row is fixed first: units in order across the rows fill them in
    // turn, each row up to its share of the units' width.
    let forced: Option<Vec<usize>> = if balanced {
        let capacity: Vec<i64> = segments
            .iter()
            .map(|row| row.iter().map(|s| s.high - s.low).sum())
            .collect();
        let total_capacity: i64 = capacity.iter().sum();
        let total_width: i64 = widths.iter().sum();
        if total_width > total_capacity || total_capacity == 0 {
            return None;
        }
        let mut by_row: Vec<usize> = (0..free.len()).collect();
        by_row.sort_by_key(|&k| (targets[k].y, targets[k].x, k));
        let mut row_of = vec![0; free.len()];
        let (mut r, mut used) = (0, 0i64);
        for k in by_row {
            let w = widths[k];
            // Move on once this row has its share, `capacity * total_width / total_capacity`,
            // compared without dividing, or has no room.
            while r + 1 < row_count
                && ((2 * used + w) * total_capacity > 2 * capacity[r] * total_width
                    || used + w > capacity[r])
            {
                r += 1;
                used = 0;
            }
            row_of[k] = r;
            used += w;
        }
        Some(row_of)
    } else {
        None
    };
    let mut order: Vec<usize> = (0..free.len()).collect();
    order.sort_by_key(|&k| (targets[k].x, targets[k].y, k));
    let mut across = vec![0i64; free.len()];
    for &k in &order {
        if let Some(row_of) = &forced {
            let r = row_of[k];
            let box_height = boxes[k].height() as i64;
            let y0 = origin.y + r as i64 * height;
            let steps = (targets[k].y - y0 + pitch / 2).div_euclid(pitch);
            let y = (y0 + steps * pitch).clamp(y0, y0 + lattice(height - box_height));
            let mut best: Option<(u64, usize)> = None;
            for (s, segment) in segments[r].iter().enumerate() {
                if let Some(x) = segment.trial(targets[k].x, widths[k], pitch) {
                    let cost = x.abs_diff(targets[k].x);
                    if best.is_none_or(|(c, _)| cost < c) {
                        best = Some((cost, s));
                    }
                }
            }
            let (_, s) = best?;
            segments[r][s].add(k, targets[k].x, widths[k], pitch);
            across[k] = y;
            continue;
        }
        let target = targets[k];
        let box_height = boxes[k].height() as i64;
        let y_in = |r: usize| {
            let y0 = origin.y + r as i64 * height;
            let steps = (target.y - y0 + pitch / 2).div_euclid(pitch);
            (y0 + steps * pitch).clamp(y0, y0 + lattice(height - box_height))
        };
        let nearest = ((target.y - origin.y + height / 2).div_euclid(height))
            .clamp(0, row_count as i64 - 1) as usize;
        let mut best: Option<(u64, usize, usize, i64)> = None;
        let consider = |r: usize, best: &mut Option<(u64, usize, usize, i64)>| {
            let y = y_in(r);
            let dy = y.abs_diff(target.y);
            if best.is_some_and(|(cost, ..)| dy >= cost) {
                return false;
            }
            for (s, segment) in segments[r].iter().enumerate() {
                if let Some(x) = segment.trial(target.x, widths[k], pitch) {
                    let cost = dy + x.abs_diff(target.x);
                    if best.is_none_or(|(c, br, bs, _)| (cost, r, s) < (c, br, bs)) {
                        *best = Some((cost, r, s, y));
                    }
                }
            }
            true
        };
        consider(nearest, &mut best);
        let (mut down, mut up) = (nearest.checked_sub(1), nearest + 1);
        loop {
            let mut going = false;
            if let Some(r) = down {
                if consider(r, &mut best) || best.is_none() {
                    going = true;
                    down = r.checked_sub(1);
                } else {
                    down = None;
                }
            }
            if up < row_count {
                if consider(up, &mut best) || best.is_none() {
                    going = true;
                    up += 1;
                } else {
                    up = row_count;
                }
            }
            if !going {
                break;
            }
        }
        let (_, r, s, y) = best?;
        segments[r][s].add(k, target.x, widths[k], pitch);
        across[k] = y;
    }

    for row in &segments {
        for segment in row {
            for (k, x) in segment.starts() {
                let u = free[k];
                let at = rows.point(Point::new(x, across[k]).minus(boxes[k].min));
                place(&mut placement, &mut index, u, at)?;
            }
        }
    }
    Some(placement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::{Item, Pin, check, synth};
    use crate::rng::Rng;

    const WAYS: [(Rows, bool); 4] = [
        (Rows::Horizontal, false),
        (Rows::Vertical, false),
        (Rows::Horizontal, true),
        (Rows::Vertical, true),
    ];

    #[test]
    fn a_full_area_is_filled_exactly() {
        let generated = synth::free_grid(6, 5, 10, 5, 0, 3);
        let problem = &generated.problem;
        let netlist = Netlist::new(problem);
        let rotations = vec![0; netlist.units.len()];
        // Targets at the reference, nudged off the lattice.
        let anchors: Vec<Point> = netlist
            .units
            .iter()
            .map(|unit| {
                let at = generated.reference.origin(unit.members[0].0);
                Point::new(at.x + 2, at.y - 3)
            })
            .collect();
        for (rows, balanced) in WAYS {
            let placement = legalize_rows(problem, &netlist, &anchors, &rotations, rows, balanced)
                .expect("an exact fit");
            assert!(check(problem, &placement).is_empty());
            assert_eq!(problem.wirelength(&placement), generated.optimum);
        }
    }

    #[test]
    fn rows_go_around_fixed_items_regions_and_macros() {
        for seed in 0..200 {
            let mut rng = Rng::new(seed);
            let area = Rect::with_size(0, 0, 40, 30);
            let pitch = 1 + rng.below(3) as i64;
            let mut problem = PlacementProblem::new(Layout::FreeSpace { area, pitch });
            let region = problem.add_region(Rect::with_size(0, 0, 20, 30));
            let mut items = Vec::new();
            for k in 0..12 {
                let (w, h) = (1 + rng.below(6) as i64, 1 + rng.below(6) as i64);
                let mut item = Item::new().size(w, h);
                match k {
                    0 => item = item.fixed_at(Point::new(17, 3), 0),
                    1 => item = item.fixed_at(Point::new(30, 20), 90_000),
                    2 | 3 => item = item.region(region),
                    _ if rng.below(2) == 0 => item = item.rotations(&[0, 90_000]),
                    _ => {}
                }
                items.push(problem.add_item(item));
            }
            problem.add_macro(&[(items[4], Point::new(0, 0)), (items[5], Point::new(6, 0))]);
            for _ in 0..12 {
                let a = items[rng.below(12) as usize];
                let b = items[rng.below(12) as usize];
                problem.add_net(1, &[Pin::new(a), Pin::new(b)]);
            }
            let netlist = Netlist::new(&problem);
            let anchors: Vec<Point> = netlist
                .units
                .iter()
                .map(|_| Point::new(rng.below(45) as i64 - 2, rng.below(35) as i64 - 2))
                .collect();
            let rotations: Vec<i32> = netlist
                .units
                .iter()
                .map(|unit| {
                    let allowed = problem.item(unit.members[0].0).allowed_rotations();
                    allowed[rng.below(allowed.len() as u64) as usize]
                })
                .collect();
            let mut found = 0;
            for (rows, balanced) in WAYS {
                let first = legalize_rows(&problem, &netlist, &anchors, &rotations, rows, balanced);
                let again = legalize_rows(&problem, &netlist, &anchors, &rotations, rows, balanced);
                assert_eq!(first, again);
                if let Some(placement) = first {
                    found += 1;
                    let violations = check(&problem, &placement);
                    assert!(violations.is_empty(), "seed {seed}: {violations:?}");
                }
            }
            assert!(found > 0, "seed {seed}");
        }
    }
}
