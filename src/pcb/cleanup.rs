//! Cleanup after routing: fewer vias, then shorter and straighter tracks. Every change is
//! checked against the world as routing is, so cleanup can't make a route illegal.

use super::detail::{self, NetCtx, Params, octile};
use super::geometry::{NetCopper, Seg};
use super::router::Job;
use super::world::{Verdict, World};
use super::{Design, RouteOptions};
use crate::geom::{LayerId, Point, Rect, Shape};

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
/// millimetre saved is one via pair more.
const SHORTER_VIA: f64 = 3.0;

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
                let free = way.iter().all(|&to| {
                    let ok = legal(world, ctx, from, to, l);
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
    use super::{fewer_hops, shorter};
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
