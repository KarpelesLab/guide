//! Rows of fine pitch: the ways out of a row's pads, planned for the whole row at once.
//!
//! A row is *fine* if a via in the way out of one of its pads is in the way out of the
//! pads beside it: on a 0.4 mm QFN with vias of 0.6 mm, a via at the end of a pad closes
//! the ways of both its neighbours. A search for one net at a time puts each via where it
//! is nearest, and whichever pad of such a row is routed shuts in another. No price on
//! the ways out settles that, as two tracks side by side out of two pads leave neither a
//! place for a via between them: what a row needs is that pads side by side do not leave
//! it on the same side with a via, and that is a choice for the whole row.
//!
//! So each pad of a fine row has a few ways out to choose from: none, a straight track
//! out of either end of the pad, or such a track with a via at its end, at one of a few
//! depths, each where the world has room for it. Each has a cost, from where the pad's
//! net has to go: the length of the stub, a via's cost, and how far the stub's end is
//! from the nearest other terminal of the net, further if that is behind the row for a
//! track with no via, which then has to go round the part. Two ways of pads near each
//! other clash if the via of one is in the other's way, or too close to its via, and two
//! plain tracks side by side on one side, which leave no place for a via, cost a little.
//! The ways of a row are then chosen together, at the least cost with no clash, exactly,
//! as the clashes only reach a few pads along the row.
//!
//! What is chosen is copper of the pads' nets that is in the world before the first net is
//! searched for, as routes that their nets go on from, and as nothing more: a net whose
//! search does without its stub loses it, and a net in the way of another is cut or
//! taken out, stubs and all, like any other.

use std::collections::BTreeMap;

use super::Design;
use super::control::Run;
use super::escape;
use super::geometry::Seg;
use super::router::{Job, pad_center};
use super::world::{Placed, Verdict, World};
use crate::geom::{LayerId, LayerSet, Point, Shape, SpatialIndex};
use crate::id::NetId;

/// The fewest pads of a row.
const MIN_ROW: usize = 4;

/// How far two pads' sizes and places may differ for them to be of one row.
const TOL: i64 = 2_000;

/// How many pads along the row a way out may clash with, either way.
const WINDOW: usize = 3;

/// What a pad with no way out planned costs: what crossing a route does.
const SHUT: i64 = 10_000_000;

/// What two plain tracks side by side on one side of a row cost.
const CROWD: i64 = 3_000_000;

/// How near another terminal of its net is, on its layer, for a pad to need no way out.
const NEAR: i64 = 1_500_000;

/// A stub planned for a pad: a track from its centre, and a via at the track's end if it
/// has one. Copper of job `job`.
#[derive(Clone, Debug)]
pub(crate) struct Stub {
    pub(crate) job: usize,
    pub(crate) seg: Seg,
    pub(crate) via: Option<Point>,
}

/// The ways a row may run: along x, along y, and along either diagonal. By way, how far
/// along and how far across it a point is, in units that are the same for both, and the
/// two sides of a row, as steps of one unit each way.
const WAYS: usize = 4;

fn along(way: usize, p: Point) -> i64 {
    match way {
        0 => p.x,
        1 => p.y,
        2 => p.x + p.y,
        _ => p.x - p.y,
    }
}

fn across(way: usize, p: Point) -> i64 {
    match way {
        0 => p.y,
        1 => p.x,
        2 => p.x - p.y,
        _ => p.x + p.y,
    }
}

/// The length of `d` units of a way: a unit of a diagonal is half a diagonal step.
fn length(way: usize, d: i64) -> i64 {
    if way < 2 { d } else { d * 7071 / 10_000 }
}

fn sides(way: usize) -> [(i64, i64); 2] {
    match way {
        0 => [(0, 1), (0, -1)],
        1 => [(1, 0), (-1, 0)],
        2 => [(1, -1), (-1, 1)],
        _ => [(1, 1), (-1, -1)],
    }
}

/// A row: its pads in order along it, the way it runs, the distance from a pad to the
/// next, the layer, and how far a pad goes on from its centre across the row.
#[derive(Clone, Debug)]
struct Row {
    pads: Vec<usize>,
    way: usize,
    pitch: i64,
    layer: LayerId,
    half: i64,
}

/// How far a shape goes on from `center` in the direction of `side`, to a micrometre.
fn extent(shape: &Shape, center: Point, (sx, sy): (i64, i64)) -> i64 {
    let inside = |d: i64| {
        Shape::circle(Point::new(center.x + sx * d, center.y + sy * d), 2)
            .is_ok_and(|c| shape.touches(&c))
    };
    let b = shape.bbox();
    let (mut lo, mut hi) = (0, (b.max().x - b.min().x).max(b.max().y - b.min().y) + 1);
    while hi - lo > 1000 {
        let mid = lo + (hi - lo) / 2;
        if inside(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    if sx != 0 && sy != 0 {
        hi * 14_142 / 10_000
    } else {
        hi
    }
}

/// The rows of pads with no hole on one layer, of one size, in line at one pitch, along
/// x, y or a diagonal, that are not part of an area array: its pads are in rows more than
/// one way.
fn rows(design: &Design, reach: i64) -> Vec<Row> {
    let stack = design.rules.layers();
    let pads = &design.board.pads;
    // Pad, centre, half sizes, layer.
    let cands: Vec<(usize, Point, i64, i64, LayerId)> = (0..pads.len())
        .filter_map(|i| {
            let p = &pads[i];
            let on = LayerSet(p.copper_layers().0 & stack.0);
            let mut layers = on.iter();
            let layer = layers.next()?;
            if p.hole.is_some() || layers.next().is_some() {
                return None;
            }
            let b = p.shape.bbox();
            let c = pad_center(&p.shape)?;
            let (w, h) = (b.max().x - b.min().x, b.max().y - b.min().y);
            let mid = Point::new(b.min().x + w / 2, b.min().y + h / 2);
            (c == mid).then_some((i, c, w / 2, h / 2, layer))
        })
        .collect();
    let index = SpatialIndex::bulk_load(cands.iter().map(|c| pads[c.0].shape.bbox()));
    let mut near = Vec::new();
    // By way, the runs of pads in line, as positions in `cands`, with their pitch.
    let mut runs: [Vec<(Vec<usize>, i64)>; WAYS] = Default::default();
    for (way, runs) in runs.iter_mut().enumerate() {
        // Rounding leaves the pads of a part turned by 45° a little out of line.
        let tol = if way < 2 { TOL } else { 2 * TOL };
        // The next pad along, and how far it is.
        let mut next: Vec<Option<(usize, i64)>> = vec![None; cands.len()];
        let mut has_prev = vec![false; cands.len()];
        for (k, &(i, at, hx, hy, layer)) in cands.iter().enumerate() {
            index.query(&pads[i].shape.bbox().expand(reach), &mut near);
            let mut best: Option<(i64, usize)> = None;
            for id in &near {
                let m = id.slot() as usize;
                let (_, other, ox, oy, on) = cands[m];
                let d = length(way, along(way, other) - along(way, at));
                if m == k
                    || on != layer
                    || d <= 0
                    || d > reach
                    || (across(way, other) - across(way, at)).abs() > tol
                    || (ox - hx).abs() > TOL
                    || (oy - hy).abs() > TOL
                {
                    continue;
                }
                if best.is_none_or(|b| (d, m) < b) {
                    best = Some((d, m));
                }
            }
            if let Some((d, m)) = best {
                next[k] = Some((m, d));
                has_prev[m] = true;
            }
        }
        for head in (0..cands.len()).filter(|&k| !has_prev[k]) {
            let mut run = vec![head];
            let mut pitch = 0;
            let mut k = head;
            while let Some((m, d)) = next[k] {
                if run.len() > 1 && (d - pitch).abs() > tol {
                    runs.push((core::mem::replace(&mut run, vec![k]), pitch));
                }
                pitch = d;
                run.push(m);
                k = m;
            }
            runs.push((run, pitch));
        }
        runs.retain(|(r, _)| r.len() >= MIN_ROW);
    }
    // The pads in rows more than one way are those of an area array.
    let mut ways = vec![0u8; cands.len()];
    for (way, runs) in runs.iter().enumerate() {
        for (run, _) in runs {
            for &k in run {
                ways[k] |= 1 << way;
            }
        }
    }
    let mut out = Vec::new();
    for (way, runs) in runs.iter().enumerate() {
        for (run, pitch) in runs {
            for part in run.split(|&k| ways[k].count_ones() > 1) {
                if part.len() < MIN_ROW {
                    continue;
                }
                let (i, center, _, _, layer) = cands[part[0]];
                out.push(Row {
                    pads: part.iter().map(|&k| cands[k].0).collect(),
                    way,
                    pitch: *pitch,
                    layer,
                    half: extent(&pads[i].shape, center, sides(way)[0]),
                });
            }
        }
    }
    out.sort_by_key(|r| (r.pads[0], r.way));
    out
}

/// What a way out is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    None,
    Plain,
    Via,
}

/// A way out of a pad.
#[derive(Clone, Debug)]
struct Way {
    kind: Kind,
    /// Which side of the row: 0 or 1.
    side: u8,
    /// The end of its stub, where its via is if it has one.
    to: Point,
    /// The track that its net has in the way of others, further than the stub for a plain
    /// track, which goes on from it.
    lane: Option<Shape>,
    cost: i64,
}

/// A pad of a row as its ways out are chosen: its net, job, centre, and the ways.
struct Slot {
    net: Option<NetId>,
    job: usize,
    center: Point,
    via: (i64, i64),
    ways: Vec<Way>,
}

/// The distance from `a` to `b` along the eight directions.
fn octile(a: Point, b: Point) -> i64 {
    let (dx, dy) = ((a.x - b.x).abs(), (a.y - b.y).abs());
    let (lo, hi) = (dx.min(dy), dx.max(dy));
    hi - lo + lo * 1414 / 1000
}

/// Whether two ways out of two pads can't both be taken.
fn clash(design: &Design, layer: LayerId, a: (&Slot, &Way), b: (&Slot, &Way)) -> bool {
    let (Some(na), Some(nb)) = (a.0.net, b.0.net) else {
        return false;
    };
    if na == nb {
        return false;
    }
    let rules = &design.rules;
    let spec = rules.spec();
    let gap = rules.clearance_between(Some(na), Some(nb), layer);
    let hit = |x: &Shape, y: &Shape, clearance: i64| x.violates(y, clearance).unwrap_or(true);
    let via = |s: &Slot, w: &Way| {
        (w.kind == Kind::Via)
            .then(|| {
                Shape::circle(w.to, s.via.0)
                    .ok()
                    .zip(Shape::circle(w.to, s.via.1).ok())
            })
            .flatten()
    };
    let (va, vb) = (via(a.0, a.1), via(b.0, b.1));
    if let (Some((ca, ha)), Some((cb, hb))) = (&va, &vb)
        && (hit(ca, cb, gap)
            || (spec.hole_to_hole > 0 && hit(ha, hb, spec.hole_to_hole))
            || hit(ha, cb, spec.hole_clearance)
            || hit(ca, hb, spec.hole_clearance))
    {
        return true;
    }
    let crossed = |via: &Option<(Shape, Shape)>, lane: &Option<Shape>| match (via, lane) {
        (Some((c, h)), Some(t)) => hit(t, c, gap) || hit(t, h, spec.hole_clearance),
        _ => false,
    };
    crossed(&va, &b.1.lane) || crossed(&vb, &a.1.lane)
}

/// Plans the ways out of the rows of fine pitch of a design, against `world`, which is
/// left as it was. `pitch` is detailed routing's lattice, and `via_cost` a via's cost in
/// millimetres of track.
pub(crate) fn plan(
    design: &Design,
    world: &mut World<'_>,
    pitch: i64,
    jobs: &[Job],
    via_cost: f64,
) -> Vec<Stub> {
    let rules = &design.rules;
    if rules.layer_count() < 2 || pitch <= 0 {
        return Vec::new();
    }
    let pads = &design.board.pads;
    let via_cost = (via_cost * 1e6) as i64;
    // The most that a via and a track side by side take, over the nets routed.
    let need = |job: &Job, layer: LayerId| {
        job.ctx.via.map(|(d, _)| {
            d / 2
                + rules.clearance(Some(job.ctx.net), layer)
                + job.ctx.widths[usize::from(layer.0)] / 2
        })
    };
    let Some(reach) = jobs
        .iter()
        .filter_map(|j| need(j, LayerId(0)))
        .max()
        .map(|n| 2 * n)
    else {
        return Vec::new();
    };
    let rows = rows(design, reach);
    if rows.is_empty() {
        return Vec::new();
    }
    // By pad's box, the pad, for the pads of rows; and by pad, its job and where routes
    // end at it.
    let mut by_box: BTreeMap<(Point, Point), usize> = BTreeMap::new();
    for r in &rows {
        for &p in &r.pads {
            let b = pads[p].shape.bbox();
            by_box.insert((b.min(), b.max()), p);
        }
    }
    let mut job_of: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    for (j, job) in jobs.iter().enumerate() {
        for (k, t) in job.ctx.pads.iter().enumerate() {
            if let Some(&p) = by_box.get(&(t.bbox.min(), t.bbox.max()))
                && pads[p].net == Some(job.ctx.net)
                && pad_center(&pads[p].shape) == Some(t.center)
            {
                job_of.entry(p).or_insert((j, k));
            }
        }
    }
    let mut placed: Vec<Placed> = Vec::new();
    let mut out: Vec<Stub> = Vec::new();
    for row in &rows {
        if world.run().is_some_and(Run::poll) {
            break;
        }
        let layer = row.layer;
        let l = usize::from(layer.0);
        // Fine: a via of any of its nets in a pad's way out is in the way of the pad
        // beside it.
        let needs: Vec<i64> = row
            .pads
            .iter()
            .filter_map(|p| job_of.get(p))
            .filter_map(|&(j, _)| need(&jobs[j], layer))
            .collect();
        if needs.len() < 2 || needs.iter().any(|&n| row.pitch >= n) {
            continue;
        }
        let ends = (
            pad_center(&pads[row.pads[0]].shape),
            pad_center(&pads[row.pads[row.pads.len() - 1]].shape),
        );
        let (Some(first), Some(last)) = ends else {
            continue;
        };
        let span = octile(first, last);
        let sides = sides(row.way);
        // What a step of a stub is long, for one that is a unit either way: a diagonal's
        // is longer.
        let diagonal = row.way >= 2;
        let mut slots: Vec<Slot> = Vec::new();
        for &p in &row.pads {
            let none = Way {
                kind: Kind::None,
                side: 0,
                to: Point::new(0, 0),
                lane: None,
                cost: 0,
            };
            let Some(center) = pad_center(&pads[p].shape) else {
                continue;
            };
            let mut slot = Slot {
                net: pads[p].net,
                job: usize::MAX,
                center,
                via: (0, 0),
                ways: vec![none],
            };
            let Some(&(j, k)) = job_of.get(&p).filter(|_| !escape::joined(design, p)) else {
                slots.push(slot);
                continue;
            };
            let ctx = &jobs[j].ctx;
            let Some((d, drill)) = ctx.via else {
                slots.push(slot);
                continue;
            };
            (slot.job, slot.via) = (j, (d, drill));
            let net = ctx.net;
            let width = ctx.widths[l];
            let terminal = ctx.pads[k].terminal;
            let targets: Vec<(Point, LayerSet)> = ctx
                .pads
                .iter()
                .filter(|q| q.terminal != terminal)
                .map(|q| (q.center, q.layers))
                .collect();
            if targets.is_empty() {
                slots.push(slot);
                continue;
            }
            // How far `from` is from the nearest other terminal, on `layers`, and with
            // `ahead`, a side of the row, further for those behind the row.
            let rest = |from: Point, layers: LayerSet, ahead: Option<(i64, i64)>| {
                targets
                    .iter()
                    .map(|&(t, on)| {
                        let behind = ahead.is_some_and(|(sx, sy)| {
                            sx * (t.x - center.x) + sy * (t.y - center.y) < 0
                        });
                        octile(from, t)
                            + if on.0 & layers.0 == 0 { via_cost } else { 0 }
                            + if behind { span } else { 0 }
                    })
                    .min()
                    .unwrap_or(0)
            };
            let own = LayerSet::EMPTY.with(layer);
            let direct = rest(center, own, None);
            slot.ways[0].cost = if direct <= NEAR {
                direct
            } else {
                direct + SHUT
            };
            let clearance = rules.clearance(Some(net), layer);
            // Depths are looked at in steps much finer than the lattice: the room for a
            // via between a row and what is under its part is a few tens of micrometres
            // long, and a stub's end is a place of its own that routes go on from.
            let unit = (pitch / 8).max(1000);
            // The length of a step: a unit each way for a row along a diagonal.
            let step = if diagonal {
                unit * 14_142 / 10_000
            } else {
                unit
            };
            // The first step at which a via's centre is past the pad, and how many steps
            // deeper a via clear of one two pads along is.
            let first = (row.half + step - 1) / step;
            let far = d + clearance;
            let stagger = {
                let side = 2 * row.pitch;
                let sq = (far as i128 * far as i128 - side as i128 * side as i128).max(0);
                ((sq as f64).sqrt() as i64 + step - 1) / step
            };
            let last = first + (3 * far) / step + 2;
            let free = |world: &World<'_>, to: Point| {
                Shape::track(center, to, width)
                    .is_ok_and(|s| world.track(net, layer, &s, None) == Verdict::Free)
            };
            for (side, &(sx, sy)) in sides.iter().enumerate() {
                let at = |m: i64| Point::new(center.x + sx * m * unit, center.y + sy * m * unit);
                // The vias, at up to three depths.
                let mut depths: Vec<i64> = Vec::new();
                let mut m = first;
                while m <= last && depths.len() < if stagger > 0 { 3 } else { 1 } {
                    if world.via(net, at(m), d, drill, None) == Verdict::Free {
                        if !free(world, at(m)) {
                            break;
                        }
                        depths.push(m);
                        m += stagger.max(1);
                    } else {
                        m += 1;
                    }
                }
                for &m in &depths {
                    slot.ways.push(Way {
                        kind: Kind::Via,
                        side: side as u8,
                        to: at(m),
                        lane: Shape::track(center, at(m), width).ok(),
                        cost: m * step + via_cost + rest(at(m), rules.layers(), None),
                    });
                }
                // A plain track: past the pad by what a via beside it takes, on a side
                // where it has room to go on, which under a part with an exposed pad it
                // has not.
                let m = first + (d / 2 + clearance + step - 1) / step;
                if free(world, at(last + 1)) {
                    slot.ways.push(Way {
                        kind: Kind::Plain,
                        side: side as u8,
                        to: at(m),
                        lane: Shape::track(center, at(last + 1), width).ok(),
                        cost: m * step + rest(at(m), own, Some((sx, sy))),
                    });
                }
            }
            slots.push(slot);
        }
        let chosen = choose(design, layer, &slots);
        for (slot, &w) in slots.iter().zip(&chosen) {
            let way = &slot.ways[w];
            if way.kind == Kind::None {
                continue;
            }
            let ctx = &jobs[slot.job].ctx;
            let seg = Seg {
                a: slot.center,
                b: way.to,
                l: layer.0,
            };
            if let Ok(shape) = Shape::track(seg.a, seg.b, ctx.widths[l]) {
                placed.push(world.add_track(ctx.net, layer, shape));
            }
            let via = (way.kind == Kind::Via).then_some(way.to);
            if via.is_some() {
                placed.push(world.add_via(ctx.net, way.to, slot.via.0, slot.via.1));
            }
            out.push(Stub {
                job: slot.job,
                seg,
                via,
            });
        }
    }
    for p in &placed {
        world.remove(p);
    }
    out
}

/// The way out of each pad of a row, at the least cost in all with no two that clash:
/// exact, by the ways of the last [`WINDOW`] pads.
fn choose(design: &Design, layer: LayerId, slots: &[Slot]) -> Vec<usize> {
    // By the ways of the last few pads: the least cost so far, and where it came from in
    // `trail`, which has by step the way taken and the step before.
    let mut trail: Vec<(u8, usize)> = Vec::new();
    let mut states: BTreeMap<Vec<u8>, (i64, usize)> = BTreeMap::new();
    states.insert(Vec::new(), (0, usize::MAX));
    for (i, slot) in slots.iter().enumerate() {
        let mut next: BTreeMap<Vec<u8>, (i64, usize)> = BTreeMap::new();
        for (state, &(cost, from)) in &states {
            'ways: for (w, way) in slot.ways.iter().enumerate() {
                let mut cost = cost + way.cost;
                for (back, &v) in state.iter().rev().enumerate() {
                    let other = &slots[i - 1 - back];
                    let theirs = &other.ways[usize::from(v)];
                    if clash(design, layer, (slot, way), (other, theirs)) {
                        continue 'ways;
                    }
                    if back == 0
                        && way.kind == Kind::Plain
                        && theirs.kind == Kind::Plain
                        && way.side == theirs.side
                        && slot.net != other.net
                    {
                        cost += CROWD;
                    }
                }
                let mut key = state.clone();
                key.push(w as u8);
                if key.len() > WINDOW {
                    key.remove(0);
                }
                if next.get(&key).is_none_or(|&(c, _)| cost < c) {
                    trail.push((w as u8, from));
                    next.insert(key, (cost, trail.len() - 1));
                }
            }
        }
        states = next;
    }
    let mut chosen = vec![0; slots.len()];
    let Some(&(_, mut at)) = states.values().min_by_key(|&&(c, _)| c) else {
        return chosen;
    };
    for slot in chosen.iter_mut().rev() {
        let (w, from) = trail[at];
        *slot = usize::from(w);
        at = from;
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::super::control::Run;
    use super::super::router;
    use super::super::tests::{MM, design, route_checked, smd, through};
    use super::super::world::World;
    use super::super::{Design, RouteOptions, RunControl};
    use super::{Stub, plan};
    use crate::geom::drc::{self, Pad, Track, Via};
    use crate::geom::{LayerId, Point, Rect, Shape};

    /// A pad of 0.2 by 0.875 mm, upright, on the top layer.
    fn lead(x: i64, y: i64, net: u32) -> Pad {
        Pad {
            shape: Shape::rect(Rect::new(
                Point::new(x - 100_000, y - 437_500),
                Point::new(x + 100_000, y + 437_500),
            ))
            .unwrap(),
            ..smd(x, y, net)
        }
    }

    /// One side of a QFN: `n` leads at `step` along x, with the exposed pad of the part
    /// 1.025 mm below their ends, each lead of a net with a pad 10 mm above the row.
    fn side(n: i64, step: i64) -> Design {
        let mut pads = Vec::new();
        for i in 0..n {
            pads.push(lead(i * step, 0, i as u32 + 1));
            pads.push(through(i * 2 * MM - 4 * MM, 10 * MM, i as u32 + 1, 2));
        }
        pads.push(Pad {
            shape: Shape::rect(Rect::new(
                Point::new(-MM, -4_662_500),
                Point::new(n * step + MM, -1_462_500),
            ))
            .unwrap(),
            ..smd(0, 0, 0)
        });
        design(2, pads, n as usize + 1)
    }

    fn planned(d: &Design) -> Vec<Stub> {
        let options = RouteOptions {
            vias_in_pads: false,
            ..RouteOptions::default()
        };
        let control = RunControl::default();
        let run = Run::new(&control, Some(1_000_000));
        let mut world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )
        .unwrap();
        world.configure(d, &options);
        world.set_run(&run);
        let pitch = router::pitch(d, &options);
        let jobs = router::jobs(d, &world, pitch, &[], &[]);
        plan(d, &mut world, pitch, &jobs, options.via_cost)
    }

    #[test]
    fn the_pads_of_a_fine_row_leave_it_by_turns_on_either_side() {
        let d = side(10, 400_000);
        let stubs = planned(&d);
        // Every lead has a way out.
        assert_eq!(stubs.len(), 10, "{stubs:?}");
        // No two leads side by side have a via on the same side of the row, and those
        // under the part are between the leads and its exposed pad.
        let mut by_x: Vec<(i64, Option<Point>)> =
            stubs.iter().map(|s| (s.seg.a.x, s.via)).collect();
        by_x.sort_unstable();
        for w in by_x.windows(2) {
            if let (Some(a), Some(b)) = (w[0].1, w[1].1) {
                assert!((a.y > 0) != (b.y > 0), "{a:?} {b:?}");
            }
        }
        let under = stubs
            .iter()
            .filter(|s| s.via.is_some_and(|v| v.y < 0))
            .count();
        assert!(under >= 4, "{under} of {stubs:?}");
        for v in stubs.iter().filter_map(|s| s.via) {
            assert!(
                v.y > 437_500 || (v.y < -437_500 && v.y > -1_462_500),
                "{v:?}"
            );
        }
        // They break no rule, with each other or the pads.
        let mut board = d.board.clone();
        for s in &stubs {
            let net = d.board.pads.iter().find(|p| {
                p.shape.bbox().min().x < s.seg.a.x
                    && p.shape.bbox().max().x > s.seg.a.x
                    && p.hole.is_none()
                    && p.net.is_some_and(|n| n.index() > 0)
            });
            let net = net.and_then(|p| p.net);
            board.tracks.push(Track {
                start: s.seg.a,
                end: s.seg.b,
                mid: None,
                width: 200_000,
                layer: LayerId(s.seg.l),
                net,
            });
            board.vias.extend(s.via.map(|at| Via {
                at,
                drill: 300_000,
                diameter: 600_000,
                from: LayerId(0),
                to: LayerId(1),
                net,
            }));
        }
        let broken = drc::check(&board, &d.rules).unwrap();
        assert!(broken.is_empty(), "{broken:?}");
        // A row that a via fits beside the tracks of has none.
        assert!(planned(&side(10, 1_270_000)).is_empty());
    }

    #[test]
    fn a_row_turned_by_45_degrees_is_a_row_too() {
        // Ten leads of 0.2 by 0.875 mm at 0.403 mm along a diagonal, as those of a QFN that
        // is turned by 45°, each of a net with a pad far off on one side.
        let mut pads = Vec::new();
        for i in 0..10i64 {
            let c = Point::new(i * 285_000, i * 285_000);
            let (l, w) = (309_359, 70_711);
            let corner = |a: i64, b: i64| Point::new(c.x + a * l + b * w, c.y - a * l + b * w);
            let ring = [corner(1, 1), corner(1, -1), corner(-1, -1), corner(-1, 1)];
            pads.push(Pad {
                shape: Shape::polygon(&ring, &[]).unwrap(),
                ..smd(c.x, c.y, i as u32 + 1)
            });
            pads.push(through(10 * MM + i * 2 * MM, -10 * MM, i as u32 + 1, 2));
        }
        let d = design(2, pads, 11);
        let stubs = planned(&d);
        assert_eq!(stubs.len(), 10, "{stubs:?}");
        // Each stub leaves its lead across the row, along the other diagonal.
        for s in &stubs {
            let (dx, dy) = (s.seg.b.x - s.seg.a.x, s.seg.b.y - s.seg.a.y);
            assert!(dx != 0 && dx == -dy, "{s:?}");
        }
        assert!(stubs.iter().any(|s| s.via.is_some()), "{stubs:?}");
        let mut board = d.board.clone();
        for (i, s) in stubs.iter().enumerate() {
            let net = d.board.pads[2 * i].net;
            assert_eq!(Some(s.seg.a), super::pad_center(&d.board.pads[2 * i].shape));
            board.tracks.push(Track {
                start: s.seg.a,
                end: s.seg.b,
                mid: None,
                width: 200_000,
                layer: LayerId(s.seg.l),
                net,
            });
            board.vias.extend(s.via.map(|at| Via {
                at,
                drill: 300_000,
                diameter: 600_000,
                from: LayerId(0),
                to: LayerId(1),
                net,
            }));
        }
        let broken = drc::check(&board, &d.rules).unwrap();
        assert!(broken.is_empty(), "{broken:?}");
    }

    #[test]
    fn a_stub_s_end_is_kept_until_its_net_does_without_it() {
        use super::super::geometry::NetCopper;
        let d = side(10, 400_000);
        let options = RouteOptions::default();
        let mut world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )
        .unwrap();
        world.configure(&d, &options);
        let pitch = router::pitch(&d, &options);
        let jobs = router::jobs(&d, &world, pitch, &[], &[]);
        let ctx = &jobs[0].ctx;
        // A stub out of the first lead, with a via at its end, and a track that leads
        // nowhere, as a cut leaves one.
        let seg = |a: Point, b: Point| super::Seg { a, b, l: 0 };
        let lead = Point::new(0, 0);
        let end = Point::new(0, -850_000);
        let mut c = NetCopper::default();
        c.segs = vec![
            seg(lead, end),
            seg(lead, Point::new(0, MM)),
            seg(Point::new(0, MM), Point::new(MM, 2 * MM)),
        ];
        c.vias = vec![end];
        c.keep = vec![end];
        // What a cut left goes, and the stub stays, with its via.
        c.prune(ctx);
        assert_eq!(c.segs, [seg(end, lead)]);
        assert_eq!(c.vias, [end]);
        // It goes once it is not kept any more: when its net is routed whole without
        // it, or the run ends.
        c.keep.clear();
        c.prune(ctx);
        assert!(c.is_empty(), "{c:?}");
    }

    #[test]
    fn a_run_with_a_budget_routes_a_fine_row_from_its_stubs() {
        let d = side(10, 400_000);
        let options = RouteOptions {
            vias_in_pads: false,
            work_budget: Some(50_000_000),
            ..RouteOptions::default()
        };
        let routed = route_checked(&d, &options);
        assert!(routed.unrouted.is_empty(), "{:?}", routed.unrouted);
        assert_eq!(routed.stopped, None);
        assert!(routed.stats.row_stubs >= 10, "{:?}", routed.stats);
        // Without the plan it has none, and with no budget neither.
        let without = RouteOptions {
            row_fanout: false,
            ..options
        };
        assert_eq!(route_checked(&d, &without).stats.row_stubs, 0);
        let unbounded = RouteOptions {
            work_budget: None,
            ..options
        };
        assert_eq!(route_checked(&d, &unbounded).stats.row_stubs, 0);
    }
}
