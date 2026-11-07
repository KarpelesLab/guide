//! Cleanup after routing: fewer vias, then shorter and straighter tracks. Every change is
//! checked against the world as routing is, so cleanup can't make a route illegal.

use super::Options;
use super::detail::{self, NetCtx, Params, octile};
use super::geometry::{NetCopper, Seg};
use super::router::Job;
use super::world::{Verdict, World};
use crate::geom::{LayerId, Point, Shape};

/// How much dearer vias are when a net is rerouted to lose some.
const VIA_COST_FACTOR: f64 = 10.0;

/// Cleans up every routed net, and returns the nodes its searches expanded.
pub(crate) fn run(
    world: &mut World<'_>,
    jobs: &[Job],
    copper: &mut [Option<NetCopper>],
    params: Params,
    options: &Options,
) -> u64 {
    let mut expanded = 0;
    for (job, slot) in jobs.iter().zip(copper.iter_mut()) {
        let Some(c) = slot else { continue };
        if !c.vias.is_empty() {
            expanded += fewer_vias(world, job, c, params, options);
        }
    }
    for (job, slot) in jobs.iter().zip(copper.iter_mut()) {
        let Some(c) = slot else { continue };
        drop_idle_vias(c, &job.ctx);
        pull_tight(world, &job.ctx, c);
        c.place(world, &job.ctx);
    }
    expanded
}

/// Routes a net again with dearer vias, and keeps the new route if it has fewer vias and is
/// not much longer.
fn fewer_vias(
    world: &mut World<'_>,
    job: &Job,
    c: &mut NetCopper,
    params: Params,
    options: &Options,
) -> u64 {
    c.unplace(world);
    let p = Params {
        via_cost: options.via_cost * VIA_COST_FACTOR,
        ..params
    };
    let mut expanded = 0;
    let mut found = None;
    for corridor in [job.corridor.as_ref(), None] {
        if corridor.is_none() && job.corridor.is_some() && found.is_some() {
            break;
        }
        match detail::route_net(world, &job.ctx, p, corridor, None) {
            Ok(t) => {
                expanded += t.expanded;
                found = Some(t);
                break;
            }
            Err((_, e)) => expanded += e,
        }
    }
    if let Some(t) = found {
        let new = NetCopper::from_tree(&job.ctx, &t);
        // A via is worth a few millimetres of track, at most.
        let limit = c.length() + 1e6 * options.via_cost * VIA_COST_FACTOR / 2.0;
        if new.vias.len() < c.vias.len() && new.length() <= limit {
            *c = new;
        }
    }
    c.place(world, &job.ctx);
    expanded
}

/// Drops vias that join copper on one layer only, or none.
fn drop_idle_vias(c: &mut NetCopper, ctx: &NetCtx) {
    let map = c.incidence();
    c.vias.retain(|&v| {
        let mut layers = ctx
            .pads
            .iter()
            .filter(|p| p.center == v)
            .fold(0u64, |m, p| m | p.layers.0);
        for (l, _) in map.keys().filter(|(_, p)| *p == v) {
            layers |= 1 << l;
        }
        layers.count_ones() > 1
    });
    c.merge(ctx);
}

/// The length of a straight track.
fn dist(a: Point, b: Point) -> f64 {
    Seg { a, b, l: 0 }.length()
}

/// Whether a track of the net may go from `a` to `b` on layer `l`.
fn legal(world: &World<'_>, ctx: &NetCtx, a: Point, b: Point, l: u8) -> bool {
    a == b
        || Shape::track(a, b, ctx.widths[usize::from(l)])
            .is_ok_and(|s| world.track(ctx.net, LayerId(l), &s, None) == Verdict::Free)
}

/// The octilinear ways from `a` to `b`: straight if the two are in line, or else a run at
/// 45° and a straight run, in either order.
fn ways(a: Point, b: Point) -> Vec<Vec<Point>> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    if dx == 0 || dy == 0 || dx.abs() == dy.abs() {
        return vec![vec![b]];
    }
    let d = dx.abs().min(dy.abs());
    let diag = (d * dx.signum(), d * dy.signum());
    let first = Point::new(a.x + diag.0, a.y + diag.1);
    let second = Point::new(b.x - diag.0, b.y - diag.1);
    vec![vec![first, b], vec![second, b]]
}

/// Replaces each run of tracks between pads, vias and branches with shortcuts where they
/// are legal, longest first.
fn pull_tight(world: &World<'_>, ctx: &NetCtx, c: &mut NetCopper) {
    let mut anchors: Vec<Point> = ctx.pads.iter().map(|p| p.center).collect();
    anchors.extend(c.vias.iter().copied());
    anchors.sort_unstable();
    let map = c.incidence();
    let mut used = vec![false; c.segs.len()];
    let mut out: Vec<Seg> = Vec::new();
    let is_joint = |l: u8, p: Point| {
        anchors.binary_search(&p).is_ok() || map.get(&(l, p)).is_none_or(|v| v.len() != 2)
    };
    // Each run starts at a joint, so walk out of every joint.
    let mut starts: Vec<(u8, Point, usize)> = Vec::new();
    for (&(l, p), segs) in &map {
        if is_joint(l, p) {
            for &s in segs {
                starts.push((l, p, s));
            }
        }
    }
    for (l, p, s) in starts {
        if used[s] {
            continue;
        }
        let mut run = vec![p];
        let (mut at, mut seg) = (p, s);
        loop {
            used[seg] = true;
            let t = c.segs[seg];
            let next = if t.a == at { t.b } else { t.a };
            run.push(next);
            at = next;
            if is_joint(l, at) {
                break;
            }
            let Some(&n) = map[&(l, at)].iter().find(|&&k| !used[k]) else {
                break;
            };
            seg = n;
        }
        let tight = tighten(world, ctx, l, &run);
        for w in tight.windows(2) {
            out.push(Seg {
                a: w[0],
                b: w[1],
                l,
            });
        }
    }
    // Runs that close on themselves have no joint; keep them as they are.
    for (k, s) in c.segs.iter().enumerate() {
        if !used[k] {
            out.push(*s);
        }
    }
    c.segs = out;
    c.merge(ctx);
}

/// A run of points made shorter: from each point, the furthest later one that a legal
/// octilinear shortcut reaches.
fn tighten(world: &World<'_>, ctx: &NetCtx, l: u8, run: &[Point]) -> Vec<Point> {
    let mut out = vec![run[0]];
    let mut i = 0;
    while i + 1 < run.len() {
        let mut next = (i + 1, vec![run[i + 1]]);
        for j in (i + 2..run.len()).rev() {
            let old: f64 = run[i..=j].windows(2).map(|w| dist(w[0], w[1])).sum();
            if octile(run[i], run[j]) >= old {
                continue;
            }
            let found = ways(run[i], run[j]).into_iter().find(|way| {
                let mut from = run[i];
                let new: f64 = way
                    .iter()
                    .map(|&to| {
                        let d = dist(from, to);
                        from = to;
                        d
                    })
                    .sum();
                if new >= old {
                    return false;
                }
                let mut from = run[i];
                way.iter().all(|&to| {
                    let ok = legal(world, ctx, from, to, l);
                    from = to;
                    ok
                })
            });
            if let Some(way) = found {
                next = (j, way);
                break;
            }
        }
        out.extend(next.1);
        i = next.0;
    }
    out
}
