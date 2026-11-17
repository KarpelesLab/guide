//! Cleanup after routing: fewer vias, then shorter and straighter tracks. Every change is
//! checked against the world as routing is, so cleanup can't make a route illegal.

use super::RouteOptions;
use super::detail::{self, NetCtx, Params, octile};
use super::geometry::{NetCopper, Seg};
use super::router::Job;
use super::world::{Verdict, World};
use crate::geom::{LayerId, Point, Shape};

/// How much dearer vias are when a net is rerouted to lose some.
pub(crate) const VIA_COST_FACTOR: f64 = 10.0;

/// How many nodes the search for a net's route with fewer vias may expand for each of its
/// connections, up to [`CONNECTIONS`] of them, and for each of its first [`FEW`] if that
/// is more: with vias that dear, a search goes everywhere a longer way round might be
/// before it takes one, and a net that needs its vias would cost many times what routing
/// it did to find that out. A net of a few pads is where vias are to be saved, as a bus's
/// nets that went from one layer to the other and back on their way, and its search goes
/// as far as the longest of them needs; a net of many pads, a supply or a ground, has the
/// vias of its pads, and few to lose.
const NODES_PER_CONNECTION: u64 = 20_000;
const NODES_PER_CONNECTION_OF_FEW: u64 = 100_000;
const FEW: u64 = 4;

/// How many connections at most the search for a net's route with fewer vias has
/// [`NODES_PER_CONNECTION`] for.
const CONNECTIONS: u64 = 16;

/// The nodes the search for a route with fewer vias has for a net of `connections`.
fn node_budget(connections: u64) -> u64 {
    let c = connections.max(1);
    (NODES_PER_CONNECTION * c.min(CONNECTIONS)).max(NODES_PER_CONNECTION_OF_FEW * c.min(FEW))
}

/// Cleans up every routed net, and returns the nodes its searches expanded.
pub(crate) fn run(
    world: &mut World<'_>,
    jobs: &[Job],
    copper: &mut [Option<NetCopper>],
    params: Params,
    options: &RouteOptions,
    still: &[bool],
) -> u64 {
    let mut expanded = 0;
    // A net whose routes are as an earlier run left them was cleaned up by that run.
    let still = |j: usize| still.get(j).copied().unwrap_or(false);
    // A run that has to stop leaves the nets not yet cleaned up as they are.
    let stopped = |world: &World<'_>| world.run().is_some_and(super::control::Run::poll);
    // In a run with a budget, the nets with the most vias first, as the time may not be
    // there for all of them, and not a net whose routes are stubs to its plane: each has
    // the via that takes it there, and none to lose.
    let bounded = world.run().is_some_and(super::control::Run::bounded);
    let mut order: Vec<usize> = (0..jobs.len().min(copper.len())).collect();
    if bounded {
        let vias = |j: usize| copper[j].as_ref().map_or(0, |c| c.vias.len());
        order.sort_by_key(|&j| (core::cmp::Reverse(vias(j)), j));
    }
    // And in such a run every net is pulled tight first, which takes no search: the
    // searches for fewer vias then have the time that is left, each net pulled tight again
    // if it was routed anew, where they used to have all of it on a board that a budget
    // cuts, and the tracks of every net were left as rip-up had them.
    if bounded {
        for (j, (job, slot)) in jobs.iter().zip(copper.iter_mut()).enumerate() {
            let Some(c) = slot else { continue };
            if stopped(world) {
                return expanded;
            }
            if !still(j) {
                tidy(world, &job.ctx, c);
            }
        }
    }
    for j in order {
        let job = &jobs[j];
        let Some(c) = &mut copper[j] else { continue };
        if stopped(world) {
            break;
        }
        if !c.vias.is_empty() && !(bounded && job.plane) && !still(j) {
            let before = (c.vias.len(), c.segs.len());
            expanded += fewer_vias(world, job, c, params, options);
            // And the hops that the search for the whole net left, one at a time.
            expanded += fewer_hops(world, job, c, params, options);
            if bounded && before != (c.vias.len(), c.segs.len()) {
                tidy(world, &job.ctx, c);
            }
        }
    }
    if bounded {
        return expanded;
    }
    for (j, (job, slot)) in jobs.iter().zip(copper.iter_mut()).enumerate() {
        let Some(c) = slot else { continue };
        if stopped(world) {
            break;
        }
        if !still(j) {
            tidy(world, &job.ctx, c);
        }
    }
    expanded
}

/// A net's copper without the vias that join nothing, and with its runs pulled tight, in
/// the world.
fn tidy(world: &mut World<'_>, ctx: &NetCtx, c: &mut NetCopper) {
    drop_idle_vias(c, ctx);
    pull_tight(world, ctx, c);
    c.place(world, ctx);
}

/// Routes a net again with dearer vias, and keeps the new route if it has fewer vias and is
/// not much longer.
fn fewer_vias(
    world: &mut World<'_>,
    job: &Job,
    c: &mut NetCopper,
    params: Params,
    options: &RouteOptions,
) -> u64 {
    c.unplace(world);
    let p = Params {
        via_cost: options.via_cost * VIA_COST_FACTOR,
        max_expansions: params
            .max_expansions
            .min(node_budget(job.ctx.terminals as u64 - 1)),
        ..params
    };
    let mut expanded = 0;
    let mut found = None;
    // In the corridor first, if there is one, and then anywhere.
    let attempts = [job.corridor.as_ref(), None];
    let attempts = if job.corridor.is_some() {
        &attempts[..]
    } else {
        &attempts[1..]
    };
    let steps = detail::StepCache::default();
    for &corridor in attempts {
        match detail::route_net(world, &job.ctx, &steps, p, corridor, None) {
            Ok(t) => {
                expanded += t.expanded;
                found = Some(t);
                break;
            }
            Err((f, e)) => {
                expanded += e;
                // A search that runs out of nodes in the corridor would over the board too.
                if f == detail::Failure::Budget {
                    break;
                }
            }
        }
    }
    if let Some(t) = found {
        let new = NetCopper::from_tree(&job.ctx, &t);
        // Each via saved may cost up to half its price in the search in track, in
        // millimetres, as the costs are.
        let saved = c.vias.len().saturating_sub(new.vias.len());
        let limit = c.length() + saved as f64 * 1e6 * options.via_cost * VIA_COST_FACTOR / 2.0;
        if saved > 0 && new.length() <= limit {
            *c = new;
        }
    }
    c.place(world, &job.ctx);
    expanded
}

/// How many nodes the search for a run in place of a hop may expand, how many pitches
/// round the hop's two vias it is kept to, and how many hops of a net are tried.
const HOP_NODES: u64 = 20_000;
const HOP_WINDOW: i64 = 16;
const HOPS: usize = 64;

/// Takes out a net's hops: a run of tracks on one layer between two vias that both go
/// back to the layer the net came on, as a route has where it went under or over another
/// net's that has moved since, or that it had room to go round. Each is replaced by
/// tracks on the layer of its two ends, if a search between the two vias, kept near them
/// and with no via to place, finds a way that is not much longer; nothing else of the
/// net moves, so what it joins stays joined. A search for the whole net with dearer vias
/// finds this too where it ends, and a net with a dozen vias is one where it does not.
/// Returns the nodes expanded.
fn fewer_hops(
    world: &mut World<'_>,
    job: &Job,
    c: &mut NetCopper,
    params: Params,
    options: &RouteOptions,
) -> u64 {
    let ctx = &job.ctx;
    let mut expanded = 0;
    if c.vias.len() < 2 {
        return 0;
    }
    c.unplace(world);
    let mut tried: Vec<(Point, Point)> = Vec::new();
    let mut tries = 0;
    'hops: while tries < HOPS {
        let map = c.incidence();
        let at_pad = |p: Point| ctx.pads.iter().any(|t| t.center == p);
        // By via, its two tracks' layers, if it joins one track on each of two layers
        // and nothing else.
        let sides = |v: Point| -> Option<[(u8, usize); 2]> {
            if at_pad(v) {
                return None;
            }
            let mut on = map
                .iter()
                .filter(|((_, p), _)| *p == v)
                .map(|((l, _), segs)| (*l, segs));
            let (a, b) = (on.next()?, on.next()?);
            (on.next().is_none() && a.1.len() == 1 && b.1.len() == 1)
                .then(|| [(a.0, a.1[0]), (b.0, b.1[0])])
        };
        for &from in &c.vias {
            let Some(ends) = sides(from) else { continue };
            for (k, &(across, first)) in ends.iter().enumerate() {
                let home = ends[1 - k].0;
                // Along the run on `across` to the next via.
                let mut run = vec![first];
                let mut at = {
                    let s = c.segs[first];
                    if s.a == from { s.b } else { s.a }
                };
                let mut length = c.segs[first].length();
                let to = loop {
                    if c.vias.contains(&at) {
                        break Some(at);
                    }
                    let next = map
                        .get(&(across, at))
                        .filter(|v| v.len() == 2 && !at_pad(at))
                        .and_then(|v| v.iter().copied().find(|s| !run.contains(s)));
                    let Some(next) = next else { break None };
                    let s = c.segs[next];
                    at = if s.a == at { s.b } else { s.a };
                    length += s.length();
                    run.push(next);
                };
                let Some(to) = to.filter(|&to| to != from && !tried.contains(&(from, to))) else {
                    continue;
                };
                let back = sides(to).is_some_and(|e| {
                    e.iter().any(|&(l, _)| l == across) && e.iter().any(|&(l, _)| l == home)
                });
                if !back {
                    continue;
                }
                tried.push((from, to));
                tried.push((to, from));
                tries += 1;
                if world.run().is_some_and(super::control::Run::poll) {
                    break 'hops;
                }
                // The two vias as the two pads of a net with no via, on the layer they
                // both go back to.
                let on = crate::geom::LayerSet::EMPTY.with(LayerId(home));
                let end = |at: Point, terminal: usize| detail::TermPad {
                    center: at,
                    layers: on,
                    bbox: crate::geom::Rect::new(at, at),
                    terminal,
                };
                let between = NetCtx::new(
                    ctx.net,
                    ctx.widths.clone(),
                    None,
                    vec![end(from, 0), end(to, 1)],
                    2,
                    super::router::LATTICE_REACH * params.pitch,
                )
                .on_lattice(ctx.lattice());
                let near = crate::geom::Rect::new(from, from)
                    .union(&crate::geom::Rect::new(to, to))
                    .expand(HOP_WINDOW * params.pitch);
                let b = params.bounds;
                let p = Params {
                    max_expansions: HOP_NODES,
                    bounds: crate::geom::Rect::new(
                        Point::new(near.min().x.max(b.min().x), near.min().y.max(b.min().y)),
                        Point::new(near.max().x.min(b.max().x), near.max().y.min(b.max().y)),
                    ),
                    partial: false,
                    stubs: false,
                    forest: false,
                    ..params
                };
                let steps = detail::StepCache::default();
                let found = match detail::route_net(world, &between, &steps, p, None, None) {
                    Ok(t) => {
                        expanded += t.expanded;
                        Some(NetCopper::from_tree(&between, &t))
                    }
                    Err((_, e)) => {
                        expanded += e;
                        None
                    }
                };
                let Some(new) = found.filter(|n| n.vias.is_empty()) else {
                    continue;
                };
                // Two vias saved may cost what one does in the search for fewer, in track.
                let limit = length + 1e6 * options.via_cost * VIA_COST_FACTOR;
                if new.length() > limit {
                    continue;
                }
                let mut k = 0;
                c.segs.retain(|_| {
                    k += 1;
                    !run.contains(&(k - 1))
                });
                c.vias.retain(|&v| v != from && v != to);
                c.segs.extend(new.segs);
                c.merge(ctx);
                continue 'hops;
            }
        }
        break;
    }
    c.place(world, ctx);
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
pub(crate) fn pull_tight(world: &World<'_>, ctx: &NetCtx, c: &mut NetCopper) {
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

#[cfg(test)]
mod tests {
    use super::super::detail::Params;
    use super::super::geometry::{NetCopper, Seg};
    use super::super::tests::{MM, design, smd};
    use super::super::world::World;
    use super::super::{RouteOptions, router};
    use super::fewer_hops;
    use crate::geom::Point;

    #[test]
    fn a_hop_under_nothing_is_taken_out_and_one_under_a_track_is_kept() {
        // Two nets of two pads on the top layer, each under and back up by two vias: the
        // first for nothing, the second under a track of a third net that goes on for
        // 20 mm either way.
        let pads = vec![
            smd(0, 0, 1),
            smd(4 * MM, 0, 1),
            smd(0, 40 * MM, 2),
            smd(4 * MM, 40 * MM, 2),
            smd(2 * MM, 20 * MM, 3),
            smd(2 * MM, 60 * MM, 3),
        ];
        let d = design(2, pads, 4);
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
        assert_eq!(jobs.len(), 3);
        let seg = |a: (i64, i64), b: (i64, i64), l: u8| Seg {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            l,
        };
        let hop = |y: i64| {
            let mut c = NetCopper::default();
            c.segs = vec![
                seg((0, y), (MM, y), 0),
                seg((MM, y), (3 * MM, y), 1),
                seg((3 * MM, y), (4 * MM, y), 0),
            ];
            c.vias = vec![Point::new(MM, y), Point::new(3 * MM, y)];
            c
        };
        let mut wall = NetCopper::default();
        wall.segs = vec![seg((2 * MM, 20 * MM), (2 * MM, 60 * MM), 0)];
        let mut copper = [hop(0), hop(40 * MM), wall];
        for (job, c) in jobs.iter().zip(copper.iter_mut()) {
            c.place(&mut world, &job.ctx);
        }
        let params = Params {
            pitch,
            via_cost: options.via_cost,
            layer_bias: options.layer_bias,
            bounds: crate::geom::Rect::new(
                Point::new(-10 * MM, -10 * MM),
                Point::new(14 * MM, 70 * MM),
            ),
            max_expansions: options.max_expansions,
            under_pads: false,
            partial: false,
            stubs: false,
            forest: false,
        };
        let [free, under, _] = &mut copper;
        fewer_hops(&mut world, &jobs[0], free, params, &options);
        assert!(free.vias.is_empty(), "{free:?}");
        assert!(free.segs.iter().all(|s| s.l == 0), "{free:?}");
        assert_eq!(free.pieces(&jobs[0].ctx).apart(), 0);
        fewer_hops(&mut world, &jobs[1], under, params, &options);
        assert_eq!(under.vias.len(), 2, "{under:?}");
        assert_eq!(under.pieces(&jobs[1].ctx).apart(), 0);
    }
}
