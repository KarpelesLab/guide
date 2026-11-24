//! Cleanup after routing: fewer vias, then shorter and straighter tracks. Every change is
//! checked against the world as routing is, so cleanup can't make a route illegal.

use super::detail::{self, NetCtx, Params, octile};
use super::geometry::{NetCopper, Seg};
use super::router::Job;
use super::world::{Frozen, Verdict, World};
use super::{Design, RouteOptions};
use crate::geom::{LayerId, Point, Rect, Shape};
use core::sync::atomic::{AtomicBool, Ordering};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

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
pub(crate) fn node_budget(connections: u64) -> u64 {
    let c = connections.max(1);
    (NODES_PER_CONNECTION * c.min(CONNECTIONS)).max(NODES_PER_CONNECTION_OF_FEW * c.min(FEW))
}

/// Cleans up every routed net, and returns the nodes its searches expanded.
pub(crate) fn run(
    world: &mut World<'_>,
    design: &Design,
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
    if options.shorten {
        // The nets that an earlier run left as they are too: it looked at each with the
        // others where they were then. But a net of a pair or with a length to keep: the
        // routes that a run was given for those are side by side, or as long as they
        // are to be, and a shorter way is neither.
        let kept: Vec<bool> = jobs
            .iter()
            .map(|job| {
                let net = job.ctx.net;
                (bounded && job.plane)
                    || design
                        .pairs
                        .iter()
                        .any(|p| p.positive == net || p.negative == net)
                    || design.lengths.iter().any(|l| l.nets.contains(&net))
            })
            .collect();
        expanded += shorten(world, design, jobs, copper, params, options, &kept);
        // And every net pulled tight again, a few times over: tracks side by side round
        // a corner are each in the next one's way, and the one on the inside goes first.
        let length = |copper: &[Option<NetCopper>]| -> f64 {
            copper.iter().flatten().map(NetCopper::length).sum()
        };
        for _ in 0..TIDY_SWEEPS {
            let before = length(copper);
            for (j, (job, slot)) in jobs.iter().zip(copper.iter_mut()).enumerate() {
                let Some(c) = slot else { continue };
                if stopped(world) {
                    return expanded;
                }
                if !kept[j] {
                    tidy(world, &job.ctx, c);
                }
            }
            if length(copper) > before * (1.0 - TIDY_AGAIN) {
                break;
            }
        }
        // And each run round the corners of what is beside it, off the lattice, now
        // that the runs are where they stay.
        for (j, (job, slot)) in jobs.iter().zip(copper.iter_mut()).enumerate() {
            let Some(c) = slot else { continue };
            if stopped(world) {
                return expanded;
            }
            if !kept[j] {
                round_corners(world, &job.ctx, c, params.pitch);
                c.place(world, &job.ctx);
            }
        }
    }
    // The nets that are looked at for fewer vias, in their order.
    let order: Vec<usize> = order
        .into_iter()
        .filter(|&j| {
            copper[j]
                .as_ref()
                .is_some_and(|c| !c.vias.is_empty() && !(bounded && jobs[j].plane) && !still(j))
        })
        .collect();
    // On more than one thread, with no budget of nodes to keep, the searches of the nets
    // to come are made ahead on the others ([`Ahead`]).
    let threads = options.threads.max(1).min(order.len());
    let unbudgeted = world.run().is_none_or(|r| r.work_left().is_none());
    let halt = AtomicBool::new(false);
    let ahead = Ahead::default();
    let rules = world.rules();
    std::thread::scope(|scope| {
        if threads > 1 && unbudgeted {
            // As many as the run has threads: this one has the turns, which take no
            // search of their own while the others have them all.
            for _ in 0..threads {
                scope.spawn(|| ahead.work(rules, jobs, params, options, &halt));
            }
        }
        // Whether the searches made ahead are of the world as it is.
        let mut current = false;
        for (at, &j) in order.iter().enumerate() {
            let job = &jobs[j];
            if stopped(world) {
                break;
            }
            if threads > 1 && unbudgeted && !current && world.calm() {
                ahead.begin(
                    world.freeze(),
                    order[at..]
                        .iter()
                        .enumerate()
                        .map(|(k, &j)| (at + k, j, copper[j].clone().unwrap_or_default())),
                );
                current = true;
            }
            let found = if current {
                ahead.take(at, || stopped(world))
            } else {
                None
            };
            let Some(c) = &mut copper[j] else { continue };
            let before = (c.vias.len(), c.segs.len());
            let was = (c.segs.clone(), c.vias.clone(), c.keep.clone());
            expanded += fewer_vias(world, job, c, params, options, found);
            // And the hops that the search for the whole net left, one at a time.
            expanded += fewer_hops(world, job, c, params, options);
            if bounded && before != (c.vias.len(), c.segs.len()) {
                tidy(world, &job.ctx, c);
            }
            // A net with other copper than it had makes another world of this one: the
            // searches made ahead in the world as it was are of no use from here on.
            if current && (was != (c.segs.clone(), c.vias.clone(), c.keep.clone()) || !world.calm())
            {
                ahead.end();
                current = false;
            }
        }
        halt.store(true, Ordering::Relaxed);
        ahead.close();
    });
    if options.shorten {
        // And the end of each run at a pad to where on the pad the run comes to it, now
        // that nothing looks for another way: last, as what pulls a net tight takes it
        // to the pad's centre again.
        for (job, slot) in jobs.iter().zip(copper.iter_mut()) {
            let Some(c) = slot else { continue };
            if stopped(world) {
                return expanded;
            }
            let net = job.ctx.net;
            let held = design
                .pairs
                .iter()
                .any(|p| p.positive == net || p.negative == net)
                || design.lengths.iter().any(|l| l.nets.contains(&net));
            if !held && aim_at_pads(world, &job.ctx, c) {
                c.place(world, &job.ctx);
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

/// How many points along each of the eight directions from a pad's centre a run's end may
/// be aimed at, how many of the run's last points it may leave from, how much less of
/// the run must then be outside the pads, and how much further than half a track's width
/// from the edge of the pad's box the point is, in nanometres.
const AIM_STEPS: i64 = 6;
const AIM_BACK: usize = 4;
const AIM_GAIN: f64 = 20_000.0;
const AIM_MARGIN: i64 = 2_000;

/// Aims the end of each run of a net's tracks at a pad of the net at the pad's copper
/// where the run comes to it, in place of the pad's centre: a run that the searches made
/// goes to the centre, and comes to a pad that is beside its line by a step aside that is
/// outside the pad, where a track straight on ends on the pad's copper as well, the way
/// cadlab's router ends its tracks at the first copper of a pad. The run then goes to a
/// point in the pad's box on one of the eight directions from its centre, and from there
/// to the centre, which is copper on the pad's own; of the ways that the world allows,
/// the one with the least outside the pads of the net, if that is less than the run has.
/// Returns whether any run changed.
pub(crate) fn aim_at_pads(world: &World<'_>, ctx: &NetCtx, c: &mut NetCopper) -> bool {
    let mut changed = false;
    each_run(ctx, c, |l, run| {
        let mut run = run.to_vec();
        for _ in 0..2 {
            if let Some(better) = aim(world, ctx, l, &run) {
                run = better;
                changed = true;
            }
            run.reverse();
        }
        run
    });
    changed
}

/// A run with its last point, the centre of a pad of the net, come to by way of another
/// point of the pad's box, if that leaves less of it outside the pads ([`aim_at_pads`]).
fn aim(world: &World<'_>, ctx: &NetCtx, l: u8, run: &[Point]) -> Option<Vec<Point>> {
    let n = run.len().checked_sub(1).filter(|&n| n >= 1)?;
    let center = run[n];
    let pad = ctx.pads.iter().find(|p| {
        p.center == center && p.layers.contains(LayerId(l)) && p.bbox.min() != p.bbox.max()
    })?;
    // A run from the pad to itself, or one that is all inside it, stays.
    let now = outside(ctx, l, run[0], &run[1..]);
    if now < AIM_GAIN {
        return None;
    }
    // No nearer the box's edge than a track's end stays inside it: a host that ends its
    // tracks where they are on the pad's copper from side to side ends this one there.
    let inset = ctx.widths[usize::from(l)] / 2 + AIM_MARGIN;
    let (lo, hi) = (pad.bbox.min(), pad.bbox.max());
    let (lo, hi) = (
        Point::new(lo.x + inset, lo.y + inset),
        Point::new(hi.x - inset, hi.y - inset),
    );
    if lo.x > center.x || lo.y > center.y || hi.x < center.x || hi.y < center.y {
        return None;
    }
    let mut targets: Vec<Point> = Vec::new();
    for (dx, dy) in [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ] {
        // As far as the box goes that way.
        let room = |d: i64, c: i64, lo: i64, hi: i64| match d {
            1 => hi - c,
            -1 => c - lo,
            _ => i64::MAX,
        };
        let reach = room(dx, center.x, lo.x, hi.x).min(room(dy, center.y, lo.y, hi.y));
        for k in 1..=AIM_STEPS {
            let t = reach * k / AIM_STEPS;
            if t > 0 {
                targets.push(Point::new(center.x + dx * t, center.y + dy * t));
            }
        }
    }
    let first = n.saturating_sub(AIM_BACK);
    // Every way, by what it leaves outside the pads, and then by its length.
    let mut tries: Vec<(f64, f64, usize, Vec<Point>)> = Vec::new();
    for &q in &targets {
        for i in first..n {
            if run[i] == q {
                continue;
            }
            for mut way in ways(run[i], q) {
                way.push(center);
                let mut all = run[..=i].to_vec();
                all.extend(way.iter().copied());
                let out = outside(ctx, l, all[0], &all[1..]);
                if out + AIM_GAIN <= now {
                    let length: f64 = all.windows(2).map(|w| dist(w[0], w[1])).sum();
                    tries.push((out, length, i, way));
                }
            }
        }
    }
    tries.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });
    for (_, _, i, way) in tries.into_iter().take(AIM_TRIES) {
        let mut from = run[i];
        let free = way.iter().all(|&to| {
            let ok = legal(world, ctx, from, to, l);
            from = to;
            ok
        });
        if free {
            let mut all = run[..=i].to_vec();
            all.extend(way);
            return Some(all);
        }
    }
    None
}

/// How many of the ways to a pad are asked of the world, the best first.
const AIM_TRIES: usize = 24;

/// How much less than a run of tracks a way between its two ends must cost at least for
/// the run to be searched for again, in pitches and as a share of what it costs: a run
/// that goes round a pad costs more than the straight line and has no shorter way.
const SHORTER_PITCHES: f64 = 3.0;
const SHORTER_SHARE: f64 = 0.06;

/// How many nodes the search for a shorter run may expand for each pitch of the run's
/// length, at least and at most, and how many pitches round the run it is kept to.
const SHORTER_NODES: f64 = 150.0;
const SHORTER_MIN: u64 = 4_000;
const SHORTER_MAX: u64 = 120_000;
const SHORTER_WINDOW: i64 = 12;

/// How many times at most every net is pulled tight again once the shorter runs are
/// found.
const TIDY_SWEEPS: usize = 4;

/// What share of the nets' length one time through them must take off for another.
const TIDY_AGAIN: f64 = 5e-4;

/// How many times the nets are gone through for shorter runs, and how many runs of one
/// net are searched for in one of them: a run that moves leaves room for another's.
const SHORTER_SWEEPS: usize = 3;

/// The share of cleanup's time, and of the work that a budget leaves, that the search
/// for shorter runs may have.
const SHORTER_PART: f64 = 0.6;
const SHORTER_TRIES: usize = 24;

/// A run of a net's tracks and vias from one pad or branch to the next.
#[derive(Clone, Debug)]
struct Chain {
    segs: Vec<usize>,
    /// The vias along it, its ends apart.
    vias: Vec<Point>,
    ends: [Point; 2],
    length: f64,
}

/// The runs of a net's copper, whose tracks meet at their ends only: from each pad or
/// place where more or fewer than two tracks meet, along tracks and through vias, to the
/// next. Not those with an end that leads nowhere.
fn chains(c: &NetCopper, ctx: &NetCtx) -> Vec<Chain> {
    use std::collections::BTreeMap;
    // A place where tracks are joined: a point on a layer, or on every layer where a via
    // or a pad of more layers is.
    const ALL: u8 = u8::MAX;
    let mut vias = c.vias.clone();
    vias.sort_unstable();
    let pad = |p: Point, l: u8| {
        ctx.pads
            .iter()
            .any(|t| t.center == p && t.layers.contains(LayerId(l)))
    };
    let node = |p: Point, l: u8| {
        if vias.binary_search(&p).is_ok() || pad(p, l) {
            (p, ALL)
        } else {
            (p, l)
        }
    };
    let mut at: BTreeMap<(Point, u8), Vec<usize>> = BTreeMap::new();
    for (i, s) in c.segs.iter().enumerate() {
        at.entry(node(s.a, s.l)).or_default().push(i);
        at.entry(node(s.b, s.l)).or_default().push(i);
    }
    let is_pad = |n: (Point, u8)| n.1 == ALL && ctx.pads.iter().any(|t| t.center == n.0);
    let joint = |n: (Point, u8)| is_pad(n) || at.get(&n).is_none_or(|v| v.len() != 2);
    let mut used = vec![false; c.segs.len()];
    let mut out = Vec::new();
    for (&n, segs) in &at {
        if !joint(n) {
            continue;
        }
        for &first in segs {
            if used[first] {
                continue;
            }
            let mut chain = Chain {
                segs: Vec::new(),
                vias: Vec::new(),
                ends: [n.0, n.0],
                length: 0.0,
            };
            let (mut here, mut seg) = (n, first);
            loop {
                used[seg] = true;
                let s = c.segs[seg];
                chain.segs.push(seg);
                chain.length += s.length();
                let next = if node(s.a, s.l) == here && s.a == here.0 {
                    s.b
                } else {
                    s.a
                };
                here = node(next, s.l);
                if joint(here) {
                    break;
                }
                if here.1 == ALL {
                    chain.vias.push(here.0);
                }
                let Some(&k) = at[&here].iter().find(|&&k| !used[k]) else {
                    break;
                };
                seg = k;
            }
            chain.ends[1] = here.0;
            // An end where nothing else is leads nowhere.
            let open = |n: (Point, u8)| !is_pad(n) && at.get(&n).is_none_or(|v| v.len() < 2);
            if !open(n) && !open(here) && chain.ends[0] != chain.ends[1] {
                out.push(chain);
            }
        }
    }
    out
}

/// What a run costs, as a search counts it: its length, and a via's price for each of
/// its vias.
fn cost(length: f64, vias: usize, options: &RouteOptions) -> f64 {
    length + vias as f64 * 1e6 * options.via_cost * SHORTER_VIA
}

/// How many times what a via costs while routing it costs a run that is searched for
/// again: with nothing to move, a dearer via costs no connection, as it does while nets
/// are routed (`docs/benchmarks.md`), and a run that went under another net for a
/// millimetre saved is one via pair more. Twice, which is 2 mm of track and what a via
/// costs the negotiation at a pitch of 0.2 mm, ten steps: at three times, a net that the
/// searches for fewer vias had taken round a row of pads for 3 mm more, with no via,
/// kept that way, as the same net with a via and 3 mm less cost as much
/// (`buspirate-rs232`: two nets of 16 and 18 mm, and the board's track 3 % over that of
/// cadlab's router with five vias for its ten, where it is 1 % over with eight).
const SHORTER_VIA: f64 = 2.0;

/// How much a run costs more than the straight way between its ends, if that is enough
/// for it to be searched for again.
fn excess(chain: &Chain, pitch: i64, options: &RouteOptions) -> Option<f64> {
    let old = cost(chain.length, chain.vias.len(), options);
    let over = old - octile(chain.ends[0], chain.ends[1]);
    (over >= (SHORTER_PITCHES * pitch as f64).max(SHORTER_SHARE * old)).then_some(over)
}

/// Looks for shorter runs of every net ([`RouteOptions::shorten`]), the nets with the
/// most to gain first, and returns the nodes expanded.
fn shorten(
    world: &mut World<'_>,
    design: &Design,
    jobs: &[Job],
    copper: &mut [Option<NetCopper>],
    params: Params,
    options: &RouteOptions,
    kept: &[bool],
) -> u64 {
    // Of the time and of the work that cleanup has, a share: what is cut off a fill by a
    // run that went another way is joined to it again afterwards.
    let began = std::time::Instant::now();
    let time = world
        .run()
        .and_then(super::control::Run::part_left)
        .map(|left| left.mul_f64(SHORTER_PART));
    let work = world
        .run()
        .and_then(super::control::Run::work_left)
        .map(|left| (left as f64 * SHORTER_PART) as u64);
    let stopped = |world: &World<'_>, expanded: u64| {
        world.run().is_some_and(super::control::Run::poll)
            || time.is_some_and(|t| began.elapsed() >= t)
            || work.is_some_and(|w| expanded >= w)
    };
    let mut expanded = 0;
    // Where runs moved, the last time through the nets.
    let mut last: Option<Vec<Rect>> = None;
    for _ in 0..SHORTER_SWEEPS {
        let mut places = Places {
            near: last.as_deref(),
            moved: Vec::new(),
        };
        // By net, what its runs cost more than straight ways would.
        let mut order: Vec<(f64, usize)> = Vec::new();
        for (j, (job, slot)) in jobs.iter().zip(copper.iter_mut()).enumerate() {
            let Some(c) = slot else { continue };
            if kept[j] {
                continue;
            }
            let mut split = c.detached();
            split.split_at_joints(&job.ctx);
            let over: f64 = chains(&split, &job.ctx)
                .iter()
                .filter_map(|ch| excess(ch, params.pitch, options))
                .sum();
            if over > 0.0 {
                order.push((over, j));
            }
        }
        order.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut gained = false;
        for (_, j) in order {
            if stopped(world, expanded) {
                return expanded;
            }
            let Some(c) = &mut copper[j] else { continue };
            let (more, better) = shorter(world, design, &jobs[j], c, params, options, &mut places);
            expanded += more;
            gained |= better;
        }
        if !gained {
            break;
        }
        // Another time through the nets, for the runs near one that moved: it left room.
        let moved = places.moved;
        last = Some(moved);
    }
    expanded
}

/// Where runs are looked at, and where they moved.
struct Places<'a> {
    /// If any, the boxes that a run must be near one of to be looked at: where runs moved
    /// the last time through the nets.
    near: Option<&'a [Rect]>,
    /// The boxes of the runs that moved, this time.
    moved: Vec<Rect>,
}

/// Looks for a shorter way for each run of one net, the one with the most to gain first.
/// Returns the nodes expanded, and whether any run changed.
fn shorter(
    world: &mut World<'_>,
    design: &Design,
    job: &Job,
    c: &mut NetCopper,
    params: Params,
    options: &RouteOptions,
    places: &mut Places<'_>,
) -> (u64, bool) {
    let (near, moved) = (places.near, &mut places.moved);
    let ctx = &job.ctx;
    let apart = c.pieces(ctx).apart();
    let reach = SHORTER_WINDOW * params.pitch;
    let around = |split: &NetCopper, ch: &Chain| {
        let mut r = Rect::new(ch.ends[0], ch.ends[0]);
        for &i in &ch.segs {
            let s = split.segs[i];
            r = r.union(&Rect::new(s.a, s.a)).union(&Rect::new(s.b, s.b));
        }
        r.expand(reach)
    };
    let mut expanded = 0;
    let mut better = false;
    c.unplace(world);
    let mut tried: Vec<[Point; 2]> = Vec::new();
    // The ends of the tracks that the searches made.
    let mut fresh: Vec<(u8, Point)> = Vec::new();
    for _ in 0..SHORTER_TRIES {
        if world.run().is_some_and(super::control::Run::poll) {
            break;
        }
        let mut split = c.detached();
        split.split_at_joints(ctx);
        let Some((_, chain)) = chains(&split, ctx)
            .into_iter()
            .filter(|ch| !tried.contains(&ch.ends))
            // After the first time through the nets, where a run has moved since.
            .filter(|ch| {
                near.is_none_or(|near| {
                    let r = around(&split, ch);
                    near.iter().any(|m| m.intersects(&r))
                })
            })
            .filter(|ch| {
                // Not a run that a search has just made.
                !ch.segs.iter().all(|&i| {
                    let s = split.segs[i];
                    fresh.binary_search(&(s.l, s.a)).is_ok()
                        && fresh.binary_search(&(s.l, s.b)).is_ok()
                })
            })
            .filter_map(|ch| excess(&ch, params.pitch, options).map(|e| (e, ch)))
            .max_by(|a, b| a.0.total_cmp(&b.0))
        else {
            break;
        };
        tried.push(chain.ends);
        tried.push([chain.ends[1], chain.ends[0]]);
        let old = cost(chain.length, chain.vias.len(), options);
        // The net without the run.
        let mut less = split.detached();
        let mut k = 0;
        less.segs.retain(|_| {
            k += 1;
            !chain.segs.contains(&(k - 1))
        });
        less.vias.retain(|v| !chain.vias.contains(v));
        let pieces = less.pieces(ctx);
        if pieces.apart() <= apart {
            // A run that the net is joined without: a way round that closed on itself.
            moved.push(around(&split, &chain));
            less.prune(ctx);
            *c = less;
            better = true;
            continue;
        }
        if pieces.apart() != apart + 1 || apart != 0 {
            continue;
        }
        let between = super::router::completion(design, ctx, &less, params.pitch);
        let window = around(&split, &chain);
        let b = params.bounds;
        let nodes = (chain.length / params.pitch as f64 * SHORTER_NODES) as u64;
        let p = Params {
            via_cost: params.via_cost * SHORTER_VIA,
            max_expansions: nodes
                .clamp(SHORTER_MIN, SHORTER_MAX)
                .min(params.max_expansions),
            bounds: Rect::new(
                Point::new(window.min().x.max(b.min().x), window.min().y.max(b.min().y)),
                Point::new(window.max().x.min(b.max().x), window.max().y.min(b.max().y)),
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
        let Some(new) = found else { continue };
        let gain = old - cost(new.length(), new.vias.len(), options);
        if gain < (params.pitch as f64).max(SHORTER_SHARE / 2.0 * old) {
            continue;
        }
        let mut whole = less;
        fresh.extend(new.segs.iter().flat_map(|s| [(s.l, s.a), (s.l, s.b)]));
        fresh.sort_unstable();
        fresh.dedup();
        whole.segs.extend(new.segs);
        for v in new.vias {
            if !whole.vias.contains(&v) {
                whole.vias.push(v);
            }
        }
        whole.prune(ctx);
        if whole.pieces(ctx).apart() > apart {
            continue;
        }
        *c = whole;
        better = true;
        moved.push(window);
    }
    if better {
        drop_idle_vias(c, ctx);
        pull_tight(world, ctx, c);
    }
    c.place(world, ctx);
    (expanded, better)
}

/// A net's copper, which is not in the world, without the vias that join nothing and with
/// its runs pulled tight.
pub(crate) fn tighten_net(world: &World<'_>, ctx: &NetCtx, c: &mut NetCopper) {
    drop_idle_vias(c, ctx);
    pull_tight(world, ctx, c);
}

/// A net's copper without the vias that join nothing, and with its runs pulled tight, in
/// the world.
pub(crate) fn tidy(world: &mut World<'_>, ctx: &NetCtx, c: &mut NetCopper) {
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
    ahead: Option<Again>,
) -> u64 {
    c.unplace(world);
    let (found, expanded) = match ahead {
        // Looked for already, in a world as this one is: the run is charged here.
        Some((found, expanded)) => {
            if let Some(run) = world.run() {
                run.charge(expanded);
            }
            (found, expanded)
        }
        None => route_again(world, job, params, options),
    };
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

/// The searches for fewer vias of the nets to come, made on other threads while this one
/// has the turn of the net before them.
///
/// A net's search for a route with fewer vias asks the world without the net's own
/// copper, and few of them end with another route for their net (3 to 8 in a hundred on
/// the corpus): the world most often is, at a net's turn, what it was at the turn of the
/// net before, but for copper taken out and laid back where it was. So the world is
/// [frozen](World::freeze) as it is, each thread makes a world of its own of that, and
/// takes the nets to come one at a time: the net's copper out, the search, the copper
/// back. A net's turn then has the search that was made for it, and goes on as it would
/// with its own: what a net is routed as, and whether that is kept, is the same on any
/// number of threads. Once a net does end with other copper, what was looked for ahead is
/// of a world that is no more, and is left; the world is frozen again as it then is.
///
/// Not in a run with a budget of nodes, where a search is cut by the nodes that the
/// searches before it took.
#[derive(Default)]
struct Ahead {
    state: Mutex<AheadState>,
    wake: Condvar,
}

#[derive(Default)]
struct AheadState {
    /// Which freezing of the world the searches are of, counted from one.
    epoch: u64,
    frozen: Option<Arc<Frozen>>,
    /// The nets to look at, the next first: the place in the order, the job, its copper.
    queue: VecDeque<(usize, usize, NetCopper)>,
    /// The places whose search is under way.
    under_way: Vec<usize>,
    /// By place, what its search came to.
    found: Vec<(usize, Again)>,
    closed: bool,
}

impl Ahead {
    fn lock(&self) -> std::sync::MutexGuard<'_, AheadState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Begins the searches of `nets` in the world as `frozen` has it.
    fn begin(&self, frozen: Frozen, nets: impl Iterator<Item = (usize, usize, NetCopper)>) {
        let mut s = self.lock();
        s.epoch += 1;
        s.frozen = Some(Arc::new(frozen));
        s.queue = nets.collect();
        s.under_way.clear();
        s.found.clear();
        drop(s);
        self.wake.notify_all();
    }

    /// Leaves the searches begun: the world is no longer the one they are of.
    fn end(&self) {
        let mut s = self.lock();
        s.epoch += 1;
        s.frozen = None;
        s.queue.clear();
        s.under_way.clear();
        s.found.clear();
    }

    /// No more searches.
    fn close(&self) {
        self.lock().closed = true;
        self.wake.notify_all();
    }

    /// What the search of the net at place `at` came to, waited for while it is under
    /// way or to come. `None` if it is neither, and once `stopped` says that the run has
    /// to stop: the caller then makes the search, which a run that has to stop ends.
    fn take(&self, at: usize, stopped: impl Fn() -> bool) -> Option<Again> {
        let mut s = self.lock();
        loop {
            if let Some(k) = s.found.iter().position(|f| f.0 == at) {
                return Some(s.found.swap_remove(k).1);
            }
            if !s.under_way.contains(&at) && !s.queue.iter().any(|q| q.0 == at) {
                return None;
            }
            s = self
                .wake
                .wait_timeout(s, std::time::Duration::from_millis(5))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
            if stopped() {
                return None;
            }
        }
    }

    /// A thread's work: the searches of the nets in the queue, each in a world of the
    /// thread's own, until there are no more to come.
    fn work(
        &self,
        rules: &crate::geom::Rules,
        jobs: &[Job],
        params: Params,
        options: &RouteOptions,
        halt: &AtomicBool,
    ) {
        let mut mine: Option<(u64, World<'_>)> = None;
        loop {
            let (epoch, frozen, (at, j, mut c)) = {
                let mut s = self.lock();
                loop {
                    if s.closed {
                        return;
                    }
                    if let (Some(frozen), Some(next)) = (s.frozen.clone(), s.queue.pop_front()) {
                        s.under_way.push(next.0);
                        break (s.epoch, frozen, next);
                    }
                    s = self
                        .wake
                        .wait(s)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            };
            if mine.as_ref().is_none_or(|m| m.0 != epoch) {
                mine = Some((epoch, World::thaw(rules, &frozen, halt)));
            }
            let Some((_, world)) = mine.as_mut() else {
                return;
            };
            c.unplace(world);
            let found = route_again(world, &jobs[j], params, options);
            c.place(world, &jobs[j].ctx);
            let mut s = self.lock();
            if s.epoch == epoch && !halt.load(Ordering::Relaxed) {
                s.under_way.retain(|&k| k != at);
                s.found.push((at, found));
            }
            drop(s);
            self.wake.notify_all();
        }
    }
}

/// What the search of a net for a route with fewer vias comes to: the route, if it found
/// one, and the nodes it expanded.
type Again = (Option<detail::Tree>, u64);

/// A net's route with dearer vias, in a world that its copper is not in, and the nodes
/// that the search for it expanded.
fn route_again(world: &World<'_>, job: &Job, params: Params, options: &RouteOptions) -> Again {
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
    (found, expanded)
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
    each_run(ctx, c, |l, run| tighten(world, ctx, l, run));
}

/// How many points of a run are looked at together for a way round the corners of what
/// is beside them, how many pitches from the run a corner may be, how many corners are
/// looked at, the nearest first, and how much shorter the way has to be, in nanometres.
const ROUND_STRETCH: usize = 24;
const ROUND_CORRIDOR: i64 = 4;
const ROUND_CORNERS: usize = 240;
const ROUND_GAIN: f64 = 1_000.0;

/// Lays each run of tracks between pads, vias and branches round the corners of what is
/// in its way, off the lattice: a run that the searches made keeps to the lattice's lines,
/// and passes a pad or another net's track a part of a pitch further off than the rules
/// ask, by a way that many steps longer. The corners are the world's
/// ([`World::corners`]), a run's ends stay where they are, each stretch of it is the
/// shortest way through the corners near it by tracks in the eight directions that the
/// world allows, and is laid only if it is shorter: what cadlab's router does once its
/// routes are final. Returns whether any run changed.
pub(crate) fn round_corners(
    world: &World<'_>,
    ctx: &NetCtx,
    c: &mut NetCopper,
    pitch: i64,
) -> bool {
    let before = c.length();
    each_run(ctx, c, |l, run| {
        let round = round_run(world, ctx, l, run, pitch);
        if round.len() == run.len() && round == run {
            round
        } else {
            tighten(world, ctx, l, &round)
        }
    });
    c.length() + ROUND_GAIN <= before
}

/// A run with each stretch of it laid round the corners beside it, where that is shorter.
fn round_run(world: &World<'_>, ctx: &NetCtx, l: u8, run: &[Point], pitch: i64) -> Vec<Point> {
    let mut pts = run.to_vec();
    let mut i = 0;
    while i + 1 < pts.len() {
        if world.run().is_some_and(super::control::Run::poll) {
            break;
        }
        let j = (i + ROUND_STRETCH).min(pts.len() - 1);
        if j - i >= 2
            && let Some(mid) = stretch(world, ctx, l, &pts[i..=j], pitch)
        {
            let n = mid.len();
            pts.splice(i + 1..j, mid);
            i = (i + n + 1).min(pts.len() - 1);
            continue;
        }
        i = if j + 1 == pts.len() || j == i + 1 {
            j
        } else {
            j - 1
        };
    }
    pts
}

/// A shorter way between the two ends of `pts` through the corners near it, without the
/// ends, if there is one.
fn stretch(
    world: &World<'_>,
    ctx: &NetCtx,
    l: u8,
    pts: &[Point],
    pitch: i64,
) -> Option<Vec<Point>> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let cur: f64 = pts.windows(2).map(|w| dist(w[0], w[1])).sum();
    if cur - octile(a, b) < ROUND_GAIN {
        return None;
    }
    let corridor = ROUND_CORRIDOR * pitch;
    let mut area = Rect::new(a, a);
    for &p in pts {
        area = area.union(&Rect::new(p, p));
    }
    let area = area.expand(corridor);
    let mut found = Vec::new();
    world.corners(
        ctx.net,
        LayerId(l),
        ctx.widths[usize::from(l)],
        &area,
        &mut found,
    );
    // How far a point is from the run, squared.
    let off = |q: Point| -> f64 {
        pts.windows(2)
            .map(|w| {
                let (ux, uy) = ((w[1].x - w[0].x) as f64, (w[1].y - w[0].y) as f64);
                let (vx, vy) = ((q.x - w[0].x) as f64, (q.y - w[0].y) as f64);
                let len2 = ux * ux + uy * uy;
                let t = if len2 > 0.0 {
                    ((vx * ux + vy * uy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (dx, dy) = (vx - t * ux, vy - t * uy);
                dx * dx + dy * dy
            })
            .fold(f64::MAX, f64::min)
    };
    let mut cand: Vec<(f64, Point)> = found
        .into_iter()
        // A corner helps only if a way through it is shorter than the stretch.
        .filter(|&v| octile(a, v) + octile(v, b) < cur - ROUND_GAIN)
        .map(|v| (off(v), v))
        .filter(|&(d2, _)| d2 <= (corridor as f64) * (corridor as f64))
        .collect();
    cand.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
    cand.dedup_by(|x, y| x.1 == y.1);
    cand.truncate(ROUND_CORNERS);
    let mut nodes = vec![a, b];
    nodes.extend(cand.into_iter().map(|c| c.1).filter(|&v| v != a && v != b));
    // A way from one place to another that the world allows: its turn, if it has one.
    let edge = |u: Point, v: Point| -> Option<Vec<Point>> {
        ways(u, v).into_iter().find_map(|way| {
            let mut from = u;
            way.iter()
                .all(|&to| {
                    let ok = legal(world, ctx, from, to, l);
                    from = to;
                    ok
                })
                .then(|| way[..way.len() - 1].to_vec())
        })
    };
    let n = nodes.len();
    let mut g = vec![f64::MAX; n];
    let mut parent: Vec<Option<(usize, Vec<Point>)>> = vec![None; n];
    let mut closed = vec![false; n];
    let mut heap: BinaryHeap<Reverse<(u64, usize)>> = BinaryHeap::new();
    let bound = cur - ROUND_GAIN;
    g[0] = 0.0;
    heap.push(Reverse((octile(a, b).to_bits(), 0)));
    let mut checks = 0usize;
    while let Some(Reverse((_, u))) = heap.pop() {
        if closed[u] {
            continue;
        }
        closed[u] = true;
        if u == 1 {
            break;
        }
        for v in 1..n {
            if closed[v] || v == u {
                continue;
            }
            let est = g[u] + octile(nodes[u], nodes[v]);
            if est >= g[v] || est + octile(nodes[v], b) >= bound {
                continue;
            }
            checks += 1;
            if checks > 4 * ROUND_CORNERS * 8 {
                return None;
            }
            if let Some(mid) = edge(nodes[u], nodes[v]) {
                g[v] = est;
                parent[v] = Some((u, mid));
                heap.push(Reverse(((est + octile(nodes[v], b)).to_bits(), v)));
            }
        }
    }
    if g[1] >= bound {
        return None;
    }
    // The points between the two ends, from the far one back.
    let mut out: Vec<Point> = Vec::new();
    let mut v = 1;
    while let Some((u, mid)) = parent[v].take() {
        out.extend(mid.into_iter().rev());
        if u != 0 {
            out.push(nodes[u]);
        }
        v = u;
        if v == 0 {
            break;
        }
    }
    out.reverse();
    Some(out)
}

/// Each run of a net's tracks between pads, vias and branches, on its layer, replaced by
/// what `f` makes of its points.
fn each_run(ctx: &NetCtx, c: &mut NetCopper, mut f: impl FnMut(u8, &[Point]) -> Vec<Point>) {
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
        let tight = f(l, &run);
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

/// How much of the way from `from` through `way` on layer `l` is outside the boxes of the
/// net's pads on that layer. A part inside two boxes counts for neither twice over: the
/// boxes of a net's pads that overlap are few, and this only tells two ways apart.
fn outside(ctx: &NetCtx, l: u8, from: Point, way: &[Point]) -> f64 {
    let mut total = 0.0;
    let mut a = from;
    for &b in way {
        let length = dist(a, b);
        let mut inside = 0.0;
        for pad in ctx.pads.iter().filter(|p| p.layers.contains(LayerId(l))) {
            inside += within(&pad.bbox, a, b) * length;
        }
        total += (length - inside).max(0.0);
        a = b;
    }
    total
}

/// The share of the straight line from `a` to `b` that is inside `rect`.
fn within(rect: &Rect, a: Point, b: Point) -> f64 {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let d = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let edges = [
        (-d.0, (a.x - rect.min().x) as f64),
        (d.0, (rect.max().x - a.x) as f64),
        (-d.1, (a.y - rect.min().y) as f64),
        (d.1, (rect.max().y - a.y) as f64),
    ];
    for (p, q) in edges {
        if p == 0.0 {
            if q < 0.0 {
                return 0.0;
            }
            continue;
        }
        let t = q / p;
        if p < 0.0 {
            t0 = t0.max(t);
        } else {
            t1 = t1.min(t);
        }
    }
    (t1 - t0).max(0.0)
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
            let mut found: Option<(f64, Vec<Point>)> = None;
            for way in ways(run[i], run[j]) {
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
                    continue;
                }
                let mut from = run[i];
                // And no nearer a pad of another net in the room of a spoke of its relief
                // than the run is: pulled straight, a run that went round the end of a
                // spoke goes across the gap that the spoke bridges, and the pad is short
                // of a spoke for a millimetre of track (`glasgow-revD1`, 12 pads of 20
                // that the board had short of two spokes, `docs/benchmarks.md`).
                let free = way.iter().all(|&to| {
                    let ok = legal(world, ctx, from, to, l)
                        && !world.nearer_a_spoke(
                            ctx.net,
                            LayerId(l),
                            ctx.widths[usize::from(l)],
                            &run[i..=j],
                            (from, to),
                        );
                    from = to;
                    ok
                });
                if !free {
                    continue;
                }
                // Of two ways, which are as long as each other, the one with less of its
                // length outside the pads of the net: what is inside a pad is copper on
                // copper, and a host leaves it out.
                let out = outside(ctx, l, run[i], &way);
                if found.as_ref().is_none_or(|(least, _)| out + 1.0 < *least) {
                    found = Some((out, way));
                }
            }
            if let Some((_, way)) = found {
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
    use super::{aim_at_pads, fewer_hops, outside, shorter};
    use crate::geom::Point;

    #[test]
    fn a_run_that_comes_to_a_pad_beside_its_middle_ends_on_the_pad_where_it_comes() {
        // Two pads, the second 0.15 mm above the line of the first: the run goes along
        // that line and steps up to the second pad's middle before it is at the pad.
        let pads = vec![smd(0, 0, 1), smd(10 * MM, 150_000, 1)];
        let d = design(1, pads, 2);
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
        let seg = |a: (i64, i64), b: (i64, i64)| Seg {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            l: 0,
        };
        let mut c = NetCopper::default();
        c.segs = vec![
            seg((0, 0), (9_400_000, 0)),
            seg((9_400_000, 0), (9_550_000, 150_000)),
            seg((9_550_000, 150_000), (10 * MM, 150_000)),
        ];
        let out =
            |c: &NetCopper| -> f64 { c.segs.iter().map(|s| outside(ctx, 0, s.a, &[s.b])).sum() };
        let before = out(&c);
        assert!(aim_at_pads(&world, ctx, &mut c));
        // Straight on to the pad, less the step aside that was outside it, and still
        // joined at the pad's middle.
        assert!(
            out(&c) < before - 40_000.0,
            "{} for {before}: {c:?}",
            out(&c)
        );
        assert_eq!(c.pieces(ctx).apart(), 0, "{c:?}");
        assert!(c.pieces(ctx).missing().is_empty(), "{c:?}");
        let middle = Point::new(10 * MM, 150_000);
        assert!(c.segs.iter().any(|s| s.a == middle || s.b == middle));
        // What is aimed stays as it is.
        let again = c.segs.clone();
        assert!(!aim_at_pads(&world, ctx, &mut c));
        assert_eq!(c.segs, again);
    }

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

    #[test]
    fn a_run_is_pulled_no_nearer_a_pad_across_the_gap_of_its_spoke() {
        // Two pads of a net 4 mm apart, one above the other, and a run between them that
        // goes a millimetre to the left on its way. A pad of another net 1.2 mm to the
        // right of the straight line, with a spoke of its relief to the left that ends
        // 0.2 mm from that line: the straight track is clear of the pad, and is in the
        // room of the spoke.
        let pad = |x: i64, y: i64, net: u32| crate::geom::drc::Pad {
            shape: crate::geom::Shape::rect(crate::geom::Rect::new(
                Point::new(x - 300_000, y - 300_000),
                Point::new(x + 300_000, y + 300_000),
            ))
            .unwrap(),
            layers: crate::geom::LayerSet::EMPTY.with(crate::geom::LayerId(0)),
            hole: None,
            net: Some(crate::id::NetId(net)),
            clearance: None,
            stack: Vec::new(),
        };
        let (a, b) = (Point::new(0, -2 * MM), Point::new(0, 2 * MM));
        let other = Point::new(1_200_000, 0);
        let pads = vec![pad(a.x, a.y, 1), pad(b.x, b.y, 1), pad(other.x, other.y, 2)];
        let options = RouteOptions::default();
        let run = |spoke: bool| {
            let mut d = design(3, pads.clone(), 2);
            if spoke {
                d.zones.spokes.push(crate::pcb::Spoke {
                    pad: 2,
                    layer: crate::geom::LayerId(0),
                    from: other,
                    to: Point::new(200_000, 0),
                    width: 250_000,
                });
            }
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
            let job = jobs
                .iter()
                .find(|j| j.ctx.net == crate::id::NetId(1))
                .unwrap();
            let (p, q) = (Point::new(-MM, -MM), Point::new(-MM, MM));
            let mut c = NetCopper::default();
            c.segs = vec![
                Seg { a, b: p, l: 0 },
                Seg { a: p, b: q, l: 0 },
                Seg { a: q, b, l: 0 },
            ];
            super::pull_tight(&world, &job.ctx, &mut c);
            c
        };
        // With no relief there, the run is pulled straight.
        let straight = run(false);
        assert_eq!(straight.segs.len(), 1, "{straight:?}");
        assert!((straight.length() - 4e6).abs() < 1.0, "{straight:?}");
        // With the spoke, it stays as far from the pad as it was: 2.2 mm.
        let kept = run(true);
        let near = |s: &Seg| {
            let (px, py) = (other.x as f64, other.y as f64);
            let (ax, ay, bx, by) = (s.a.x as f64, s.a.y as f64, s.b.x as f64, s.b.y as f64);
            let (dx, dy) = (bx - ax, by - ay);
            let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
        };
        assert!(kept.segs.iter().all(|s| near(s) > 2_190_000.0), "{kept:?}");
        assert!(kept.length() > 4.5e6, "{kept:?}");
    }

    #[test]
    fn of_two_ways_as_long_the_one_with_less_outside_the_pads_is_laid() {
        // A pad 2 mm wide and one beside it, 3.2 mm along and 0.6 mm down: straight and
        // then at 45 degrees, the track is inside the wide pad for a millimetre; at 45
        // degrees first, it leaves the pad at once. Both are as long.
        let pad = |x0: i64, y0: i64, x1: i64, y1: i64| crate::geom::drc::Pad {
            shape: crate::geom::Shape::rect(crate::geom::Rect::new(
                Point::new(x0, y0),
                Point::new(x1, y1),
            ))
            .unwrap(),
            layers: crate::geom::LayerSet::EMPTY.with(crate::geom::LayerId(0)),
            hole: None,
            net: Some(crate::id::NetId(1)),
            clearance: None,
            stack: Vec::new(),
        };
        let pads = vec![
            pad(-MM, -300_000, MM, 300_000),
            pad(2_700_000, -1_200_000, 3_700_000, 0),
        ];
        let d = design(2, pads, 2);
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
        assert_eq!(jobs.len(), 1);
        let ctx = &jobs[0].ctx;
        let (a, b) = (Point::new(0, 0), Point::new(3_200_000, -600_000));
        assert!(ctx.pads.iter().any(|p| p.center == a) && ctx.pads.iter().any(|p| p.center == b));
        let corner = Point::new(0, -600_000);
        let mut c = NetCopper::default();
        c.segs = vec![Seg { a, b: corner, l: 0 }, Seg { a: corner, b, l: 0 }];
        super::pull_tight(&world, ctx, &mut c);
        let turn = Point::new(2_600_000, 0);
        assert!(
            c.segs
                .iter()
                .any(|s| (s.a, s.b) == (a, turn) || (s.a, s.b) == (turn, a)),
            "{c:?}"
        );
        assert_eq!(c.segs.len(), 2, "{c:?}");
        // What is outside the two boxes: 1.6 mm straight and a tenth of a millimetre
        // across, for 2.1 mm and three tenths across the other way.
        let straight_first = super::outside(ctx, 0, a, &[turn, b]);
        let across_first = super::outside(ctx, 0, a, &[Point::new(600_000, -600_000), b]);
        assert!(
            (straight_first - 1_741_421.0).abs() < 2.0,
            "{straight_first}"
        );
        assert!((across_first - 2_524_264.0).abs() < 2.0, "{across_first}");
        // The same from the other end.
        let mut c = NetCopper::default();
        c.segs = vec![
            Seg {
                a: b,
                b: corner,
                l: 0,
            },
            Seg {
                a: corner,
                b: a,
                l: 0,
            },
        ];
        super::pull_tight(&world, ctx, &mut c);
        assert!(
            c.segs
                .iter()
                .any(|s| (s.a, s.b) == (a, turn) || (s.a, s.b) == (turn, a)),
            "{c:?}"
        );
    }

    #[test]
    fn a_run_is_laid_round_the_corners_of_what_is_beside_it() {
        // A net of two pads 10 mm apart on the top layer with a pad of another net
        // halfway, and its route round that pad along a line a millimetre off: 0.4 mm
        // further than a track 0.2 mm wide has to stay from a pad 0.6 mm wide at a
        // clearance of 0.2 mm.
        let pads = vec![smd(0, 0, 1), smd(10 * MM, 0, 1), smd(5 * MM, 0, 2)];
        let d = design(2, pads, 3);
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
        let seg = |a: (i64, i64), b: (i64, i64)| Seg {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            l: 0,
        };
        let mut c = NetCopper::default();
        c.segs = vec![
            seg((0, 0), (3 * MM, 0)),
            seg((3 * MM, 0), (4 * MM, MM)),
            seg((4 * MM, MM), (6 * MM, MM)),
            seg((6 * MM, MM), (7 * MM, 0)),
            seg((7 * MM, 0), (10 * MM, 0)),
        ];
        c.place(&mut world, ctx);
        // Pulled tight on the points it has, it stays as it is: no shortcut is legal.
        let before = c.length();
        super::pull_tight(&world, ctx, &mut c);
        assert!((c.length() - before).abs() < 1.0);
        assert!(super::round_corners(&world, ctx, &mut c, pitch));
        c.place(&mut world, ctx);
        // Shorter by what the line was further off than it had to be, on a diagonal
        // either side: some 0.3 mm.
        assert!(
            c.length() < before - 300_000.0,
            "{} for {before}",
            c.length()
        );
        assert_eq!(c.pieces(ctx).apart(), 0);
        // Every track is one that the world allows, in one of the eight directions, and
        // the nearest of them is as far from the pad as the rules ask and no further
        // than a micrometre more.
        let mut nearest = i64::MAX;
        for s in &c.segs {
            let (dx, dy) = (s.b.x - s.a.x, s.b.y - s.a.y);
            assert!(dx == 0 || dy == 0 || dx.abs() == dy.abs(), "{s:?}");
            assert!(super::legal(&world, ctx, s.a, s.b, 0), "{s:?}");
            if s.a.y == s.b.y && s.a.x.min(s.b.x) <= 5 * MM && s.a.x.max(s.b.x) >= 5 * MM {
                nearest = nearest.min(s.a.y.abs());
            }
        }
        assert!((600_000..=601_000).contains(&nearest), "{nearest}: {c:?}");
        // A second time finds nothing shorter.
        assert!(!super::round_corners(&world, ctx, &mut c, pitch));
    }

    #[test]
    fn a_way_round_is_made_shorter_where_there_is_a_shorter_one() {
        // Three nets of two pads on the top layer, 10 mm apart, each routed the long way
        // round: the first round nothing, the second round a track of a fourth net that
        // goes on for 10 mm either way, and the third with a straight track as well.
        let pads = vec![
            smd(0, 0, 1),
            smd(10 * MM, 0, 1),
            smd(0, 40 * MM, 2),
            smd(10 * MM, 40 * MM, 2),
            smd(0, 80 * MM, 3),
            smd(10 * MM, 80 * MM, 3),
            smd(5 * MM, 30 * MM, 4),
            smd(5 * MM, 50 * MM, 4),
        ];
        let d = design(2, pads, 5);
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
        assert_eq!(jobs.len(), 4);
        let seg = |a: (i64, i64), b: (i64, i64)| Seg {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            l: 0,
        };
        let round = |y: i64, by: i64| {
            let mut c = NetCopper::default();
            c.segs = vec![
                seg((0, y), (0, y + by)),
                seg((0, y + by), (10 * MM, y + by)),
                seg((10 * MM, y + by), (10 * MM, y)),
            ];
            c
        };
        let mut twice = round(80 * MM, 5 * MM);
        twice.segs.push(seg((0, 80 * MM), (10 * MM, 80 * MM)));
        let mut wall = NetCopper::default();
        wall.segs = vec![seg((5 * MM, 30 * MM), (5 * MM, 50 * MM))];
        let mut copper = [round(0, 5 * MM), round(40 * MM, -12 * MM), twice, wall];
        for (job, c) in jobs.iter().zip(copper.iter_mut()) {
            c.place(&mut world, &job.ctx);
        }
        let params = Params {
            pitch,
            via_cost: options.via_cost,
            layer_bias: options.layer_bias,
            bounds: crate::geom::Rect::new(
                Point::new(-10 * MM, -20 * MM),
                Point::new(20 * MM, 100 * MM),
            ),
            max_expansions: options.max_expansions,
            under_pads: false,
            partial: false,
            stubs: false,
            forest: false,
        };
        let [free, under, looped, _] = &mut copper;
        let mut places = super::Places {
            near: None,
            moved: Vec::new(),
        };
        // Straight across, where nothing is in the way.
        let (_, better) = shorter(
            &mut world,
            &d,
            &jobs[0],
            free,
            params,
            &options,
            &mut places,
        );
        assert!(better);
        assert!(free.vias.is_empty(), "{free:?}");
        assert_eq!(free.length(), (10 * MM) as f64, "{free:?}");
        assert_eq!(free.pieces(&jobs[0].ctx).apart(), 0);
        // Under the track in the way, which costs two vias and less than 24 mm of track.
        let (_, better) = shorter(
            &mut world,
            &d,
            &jobs[1],
            under,
            params,
            &options,
            &mut places,
        );
        assert!(better);
        assert_eq!(under.vias.len(), 2, "{under:?}");
        assert!(under.length() < (12 * MM) as f64, "{under:?}");
        assert_eq!(under.pieces(&jobs[1].ctx).apart(), 0);
        // The way round that the net is joined without goes, with no search.
        let (nodes, better) = shorter(
            &mut world,
            &d,
            &jobs[2],
            looped,
            params,
            &options,
            &mut places,
        );
        assert!(better);
        assert_eq!(nodes, 0);
        assert_eq!(looped.length(), (10 * MM) as f64, "{looped:?}");
        assert_eq!(places.moved.len(), 3);
        // And what is as short as it gets stays as it is.
        let (_, better) = shorter(
            &mut world,
            &d,
            &jobs[0],
            free,
            params,
            &options,
            &mut places,
        );
        assert!(!better);
        // Nor is a run looked at that is near nothing that moved.
        let far = [crate::geom::Rect::new(
            Point::new(500 * MM, 0),
            Point::new(501 * MM, MM),
        )];
        places.near = Some(&far);
        let mut again = round(0, 5 * MM);
        free.unplace(&mut world);
        again.place(&mut world, &jobs[0].ctx);
        let (nodes, better) = shorter(
            &mut world,
            &d,
            &jobs[0],
            &mut again,
            params,
            &options,
            &mut places,
        );
        assert!(!better && nodes == 0);
    }

    #[test]
    fn a_net_with_a_length_to_keep_is_left_as_long_as_it_is() {
        // Two nets of two pads, each routed the long way round with nothing in its way.
        // The second has a length to keep.
        let pads = vec![
            smd(0, 0, 1),
            smd(10 * MM, 0, 1),
            smd(0, 40 * MM, 2),
            smd(10 * MM, 40 * MM, 2),
        ];
        let mut d = design(2, pads, 3);
        d.lengths = vec![crate::pcb::LengthConstraint {
            nets: vec![crate::id::NetId(2)],
            min: Some(20 * MM),
            max: None,
            target: None,
            max_skew: None,
        }];
        let options = RouteOptions {
            shorten: true,
            ..RouteOptions::default()
        };
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
        assert_eq!(jobs.len(), 2);
        let seg = |a: (i64, i64), b: (i64, i64)| Seg {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            l: 0,
        };
        let round = |y: i64| {
            let mut c = NetCopper::default();
            c.segs = vec![
                seg((0, y), (0, y + 5 * MM)),
                seg((0, y + 5 * MM), (10 * MM, y + 5 * MM)),
                seg((10 * MM, y + 5 * MM), (10 * MM, y)),
            ];
            c
        };
        let mut copper = vec![Some(round(0)), Some(round(40 * MM))];
        for (job, c) in jobs.iter().zip(copper.iter_mut()) {
            c.as_mut().unwrap().place(&mut world, &job.ctx);
        }
        let params = Params {
            pitch,
            via_cost: options.via_cost,
            layer_bias: options.layer_bias,
            bounds: crate::geom::Rect::new(
                Point::new(-10 * MM, -20 * MM),
                Point::new(20 * MM, 60 * MM),
            ),
            max_expansions: options.max_expansions,
            under_pads: false,
            partial: false,
            stubs: false,
            forest: false,
        };
        // As routes that an earlier run left: looked at for a shorter way all the same.
        super::run(
            &mut world,
            &d,
            &jobs,
            &mut copper,
            params,
            &options,
            &[true, true],
        );
        let by_net = |n: u32| {
            let j = jobs
                .iter()
                .position(|j| j.ctx.net == crate::id::NetId(n))
                .unwrap();
            copper[j].as_ref().unwrap().length()
        };
        assert_eq!(by_net(1), (10 * MM) as f64);
        assert_eq!(by_net(2), (20 * MM) as f64);
    }
}
