//! The whole run: nets from the design, escapes, global routing, detailed routing with
//! rip-up, cleanup, and pairs side by side.

use std::collections::{BTreeMap, VecDeque};

use super::cleanup;
use super::detail::{self, Corridor, Failure, History, NetCtx, Params, RipUp, StepCache, TermPad};
use super::escape;
use super::geometry::NetCopper;
use super::global::{self, GlobalSpace, Probe, TerminalPads};
use super::pairs;
use super::world::{Verdict, World};
use super::{Design, DiffPair, Options, RouteError, Routed, Stats};
use crate::geom::drc::{Track, Via};
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::NetId;

/// A net to route, and where it may go.
pub(crate) struct Job {
    pub(crate) ctx: NetCtx,
    pub(crate) corridor: Option<Corridor>,
    /// Half the perimeter of its pads' box, which orders the nets.
    span: i64,
}

/// What a penalty for crossing another net's route costs, in millimetres, and how much a
/// conflict adds to its tile's history.
const RIP_PENALTY: f64 = 10.0;
const HISTORY_STEP: f64 = 0.5;

/// How many nodes a search from a terminal may expand to find it shut in, before a search
/// over the whole board for it.
const PRECHECK: u64 = 50_000;

/// How much finer the lattice of the second pass is.
const FINE: i64 = 2;

/// How many pitches from a pad routes may move onto its lattice.
pub(crate) const LATTICE_REACH: i64 = 8;

/// A point inside a pad, for its routes to end at: the centre of its box if that is in
/// the pad, as for any convex pad, or else the first of a grid of points in the box.
fn pad_center(shape: &Shape) -> Option<Point> {
    let b = shape.bbox();
    let inside = |p: Point| Shape::circle(p, 2).is_ok_and(|c| shape.touches(&c));
    let c = Point::new(
        b.min().x + (b.max().x - b.min().x) / 2,
        b.min().y + (b.max().y - b.min().y) / 2,
    );
    if inside(c) {
        return Some(c);
    }
    let (w, h) = (b.max().x - b.min().x, b.max().y - b.min().y);
    (1..8)
        .flat_map(|i| (1..8).map(move |j| (i, j)))
        .map(|(i, j)| Point::new(b.min().x + w * i / 8, b.min().y + h * j / 8))
        .find(|&p| inside(p))
}

/// Where routes of `net` end at a pad: its centre, or if fixed copper leaves no room for a
/// track's end there, as with text drawn up to the pad, the point of a grid in the pad
/// nearest the centre that has room, so that routes can still reach it.
fn access_point(
    world: &World<'_>,
    net: NetId,
    shape: &Shape,
    center: Point,
    layers: LayerSet,
    widths: &[i64],
) -> Point {
    let room = |p: Point| {
        layers.iter().any(|l| {
            Shape::circle(p, widths[usize::from(l.0)])
                .is_ok_and(|end| world.track(net, l, &end, None) != Verdict::Blocked)
        })
    };
    if room(center) {
        return center;
    }
    let b = shape.bbox();
    let (w, h) = (b.max().x - b.min().x, b.max().y - b.min().y);
    let inside = |p: Point| Shape::circle(p, 2).is_ok_and(|c| shape.touches(&c));
    let far = |p: Point| {
        let (dx, dy) = ((p.x - center.x) as i128, (p.y - center.y) as i128);
        dx * dx + dy * dy
    };
    (1..8)
        .flat_map(|i| (1..8).map(move |j| (i, j)))
        .map(|(i, j)| Point::new(b.min().x + w * i / 8, b.min().y + h * j / 8))
        .filter(|&p| inside(p) && room(p))
        .min_by_key(|&p| (far(p), p))
        .unwrap_or(center)
}

/// A terminal: its pads, by position in the board's, and the ends of fixed tracks and the
/// vias that connect them, where routes may join it too.
type Group = (Vec<usize>, Vec<(Point, LayerSet)>);

/// The pads of each net, grouped into terminals: pads that fixed copper of the net already
/// connects are one terminal.
fn terminals(design: &Design, net: NetId) -> Vec<Group> {
    let pads: Vec<usize> = (0..design.board.pads.len())
        .filter(|&i| design.board.pads[i].net == Some(net))
        .collect();
    let b = &design.board;
    let mut shapes: Vec<(Shape, LayerSet)> = pads
        .iter()
        .map(|&i| (b.pads[i].shape.clone(), b.pads[i].layers))
        .collect();
    // By shape past the pads, the points where routes may join it.
    let mut ends: Vec<Vec<(Point, LayerSet)>> = Vec::new();
    for t in b.tracks.iter().filter(|t| t.net == Some(net)) {
        let shape = match t.mid {
            None => Shape::track(t.start, t.end, t.width),
            Some(m) => Shape::arc_track(t.start, m, t.end, t.width, b.arc_tolerance),
        };
        if let Ok(s) = shape {
            let on = LayerSet::EMPTY.with(t.layer);
            shapes.push((s, on));
            ends.push(vec![(t.start, on), (t.end, on)]);
        }
    }
    for v in b.vias.iter().filter(|v| v.net == Some(net)) {
        if let Ok(s) = Shape::circle(v.at, v.diameter) {
            let span = LayerSet(LayerSet::span(v.from, v.to).0 & design.rules.layers().0);
            shapes.push((s, span));
            ends.push(vec![(v.at, span)]);
        }
    }
    for z in b.zones.iter().filter(|z| z.net == Some(net)) {
        shapes.push((z.shape.clone(), LayerSet::EMPTY.with(z.layer)));
    }
    let mut parent: Vec<usize> = (0..shapes.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    {
        for i in 0..shapes.len() {
            for j in i + 1..shapes.len() {
                let (a, b) = (&shapes[i], &shapes[j]);
                if a.1.0 & b.1.0 != 0
                    && a.0.bbox().intersects(&b.0.bbox())
                    && find(&mut parent, i) != find(&mut parent, j)
                    && a.0.touches(&b.0)
                {
                    let (x, y) = (find(&mut parent, i), find(&mut parent, j));
                    parent[x.max(y)] = x.min(y);
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Group> = BTreeMap::new();
    for (k, &pad) in pads.iter().enumerate() {
        let root = find(&mut parent, k);
        groups.entry(root).or_default().0.push(pad);
    }
    for (k, points) in ends.into_iter().enumerate() {
        let root = find(&mut parent, pads.len() + k);
        if let Some(group) = groups.get_mut(&root) {
            group.1.extend(points);
        }
    }
    groups.into_values().collect()
}

/// The jobs, in no particular order yet.
fn jobs(design: &Design, world: &World<'_>, pitch: i64) -> Vec<Job> {
    let rules = &design.rules;
    let layers = rules.layer_count();
    let mut out = Vec::new();
    for (i, spec) in design.nets.iter().enumerate() {
        let net = NetId(i as u32);
        if !spec.route {
            continue;
        }
        let groups = terminals(design, net);
        if groups.len() < 2 {
            continue;
        }
        let widths: Vec<i64> = (0..layers)
            .map(|l| {
                spec.track_width
                    .max(rules.track_width(Some(net), LayerId(l as u8)))
            })
            .collect();
        let mut pads = Vec::new();
        let mut t = 0;
        for (group, ends) in &groups {
            let before = pads.len();
            for &p in group {
                let pad = &design.board.pads[p];
                if let Some(center) = pad_center(&pad.shape)
                    && pad.layers.0 & rules.layers().0 != 0
                {
                    let on = LayerSet(pad.layers.0 & rules.layers().0);
                    let center = access_point(world, net, &pad.shape, center, on, &widths);
                    pads.push(TermPad {
                        center,
                        layers: on,
                        bbox: pad.shape.bbox(),
                        terminal: t,
                    });
                }
            }
            if pads.len() > before {
                for &(at, layers) in ends {
                    pads.push(TermPad {
                        center: at,
                        layers,
                        bbox: Rect::new(at, at),
                        terminal: t,
                    });
                }
                t += 1;
            }
        }
        if t < 2 {
            continue;
        }
        let via = spec
            .via
            .filter(|_| layers > 1)
            .map(|v| (v.diameter, v.drill));
        let bbox = pads
            .iter()
            .map(|p: &TermPad| Rect::new(p.center, p.center))
            .reduce(|a, b| a.union(&b))
            .unwrap_or_default();
        let span = (bbox.max().x - bbox.min().x) + (bbox.max().y - bbox.min().y);
        out.push(Job {
            ctx: NetCtx::new(net, widths, via, pads, t, LATTICE_REACH * pitch),
            corridor: None,
            span,
        });
    }
    out
}

/// The length of a straight track.
fn track_length(t: &Track) -> f64 {
    let (dx, dy) = ((t.end.x - t.start.x) as f64, (t.end.y - t.start.y) as f64);
    (dx * dx + dy * dy).sqrt()
}

/// Routes a pair whose two nets are routed already side by side, in place of their routes,
/// if it can, with any terminals that the pair's ends leave out joined to it. Otherwise the
/// two routes stay as they were.
fn couple_in_place(
    design: &Design,
    world: &mut World<'_>,
    pair: &DiffPair,
    jobs: &[Job],
    by_net: &BTreeMap<NetId, usize>,
    copper: &mut [Option<NetCopper>],
    params: Params,
) -> Option<pairs::PairRoute> {
    let (&jp, &jn) = (by_net.get(&pair.positive)?, by_net.get(&pair.negative)?);
    if jp == jn || copper[jp].is_none() || copper[jn].is_none() {
        return None;
    }
    let old = [copper[jp].take()?, copper[jn].take()?];
    let restore = |world: &mut World<'_>, copper: &mut [Option<NetCopper>], old: [NetCopper; 2]| {
        for (j, mut c) in [jp, jn].into_iter().zip(old) {
            c.place(world, &jobs[j].ctx);
            copper[j] = Some(c);
        }
    };
    let mut old = old;
    for c in &mut old {
        c.unplace(world);
    }
    let Some(route) = pairs::route_pair(design, world, pair, &jobs[jp], &jobs[jn], params) else {
        restore(world, copper, old);
        return None;
    };
    // The terminals the pair's ends leave out, joined to its copper or to each other.
    let mut rest: Vec<(usize, NetCopper)> = Vec::new();
    for (k, j) in [jp, jn].into_iter().enumerate() {
        let ctx = &jobs[j].ctx;
        let joined = route.terminals[k];
        if ctx.terminals <= 2 {
            continue;
        }
        let mut number: Vec<usize> = vec![0; ctx.terminals];
        let mut next = 1;
        for (t, n) in number.iter_mut().enumerate() {
            if !joined.contains(&t) {
                *n = next;
                next += 1;
            }
        }
        let mut pads: Vec<TermPad> = ctx
            .pads
            .iter()
            .map(|p| TermPad {
                terminal: number[p.terminal],
                ..p.clone()
            })
            .collect();
        for t in route.tracks.iter().filter(|t| t.net == Some(ctx.net)) {
            for at in [t.start, t.end] {
                pads.push(TermPad {
                    center: at,
                    layers: LayerSet::EMPTY.with(t.layer),
                    bbox: Rect::new(at, at),
                    terminal: 0,
                });
            }
        }
        let others = NetCtx::new(
            ctx.net,
            ctx.widths.clone(),
            ctx.via,
            pads,
            next,
            LATTICE_REACH * params.pitch,
        );
        let steps = StepCache::default();
        match detail::route_net(world, &others, &steps, params, None, None) {
            Ok(tree) => {
                let mut c = NetCopper::from_tree(&others, &tree);
                c.place(world, &others);
                rest.push((j, c));
            }
            Err(_) => {
                for (_, mut c) in rest {
                    c.unplace(world);
                }
                route.undo.undo(world);
                restore(world, copper, old);
                return None;
            }
        }
    }
    copper[jp] = Some(NetCopper::default());
    copper[jn] = Some(NetCopper::default());
    for (j, c) in rest {
        copper[j] = Some(c);
    }
    Some(route)
}

/// The most common value, the smallest among equals.
fn most_common(values: impl Iterator<Item = i64>) -> Option<i64> {
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for v in values {
        *counts.entry(v).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(v, _)| v)
}

pub(crate) fn run(design: &Design, options: &Options) -> Result<Routed, RouteError> {
    route_all(design, options)
}

/// Routes everything but meanders.
fn route_all(design: &Design, options: &Options) -> Result<Routed, RouteError> {
    let rules = &design.rules;
    let world_of = |design: &Design| {
        World::new(
            &design.board,
            rules,
            &design.outline,
            design.edge_clearance,
            &design.masks,
            &design.obstacles,
        )
        .map_err(RouteError::Geometry)
    };

    // The lattice: half the tightest pitch of tracks routed.
    let routed_nets: Vec<usize> = (0..design.nets.len())
        .filter(|&i| design.nets[i].route)
        .collect();
    let pitch = options.pitch.unwrap_or_else(|| {
        routed_nets
            .iter()
            .map(|&i| {
                let net = Some(NetId(i as u32));
                design.nets[i]
                    .track_width
                    .max(rules.track_width(net, LayerId(0)))
                    + rules.clearance(net, LayerId(0))
            })
            .min()
            .map_or(100_000, |p| (p / 2).max(1000))
    });
    // Escapes from fine-pitch footprints, which become fixed copper of their nets.
    let escapes = {
        let mut world = world_of(design)?;
        escape::plan(design, &mut world, pitch)
    };
    let with_escapes;
    let design = if escapes.is_empty() {
        design
    } else {
        let mut d = design.clone();
        for e in &escapes {
            d.board.tracks.extend(e.tracks.iter().cloned());
            d.board.vias.extend(e.via.clone());
        }
        with_escapes = d;
        &with_escapes
    };
    let pad_bounds = design
        .board
        .pads
        .iter()
        .map(|p| p.shape.bbox())
        .reduce(|a, b| a.union(&b));
    let bounds = design
        .outline
        .iter()
        .map(Shape::bbox)
        .reduce(|a, b| a.union(&b))
        .or(pad_bounds.map(|b| b.expand(5_000_000)))
        .unwrap_or_default();
    let params = Params {
        pitch,
        via_cost: options.via_cost,
        layer_bias: options.layer_bias,
        bounds,
        max_expansions: options.max_expansions,
        under_pads: false,
        precheck: 0,
    };

    let mut world = world_of(design)?;
    let mut jobs = jobs(design, &world, pitch);
    jobs.sort_by_key(|j| (j.span, j.ctx.net));
    let mut stats = Stats {
        nets: jobs.len(),
        connections: jobs.iter().map(|j| j.ctx.terminals - 1).sum(),
        ..Stats::default()
    };
    let has_job = |net: NetId| jobs.iter().any(|j| j.ctx.net == net);
    stats.pairs.0 = design
        .pairs
        .iter()
        .filter(|p| has_job(p.positive) && has_job(p.negative))
        .count();

    // Global routing.
    let tile = options.tile.unwrap_or(16 * pitch).max(pitch);
    let common_width = most_common(jobs.iter().map(|j| j.ctx.widths[0])).unwrap_or(pitch);
    let common_via = most_common(jobs.iter().filter_map(|j| j.ctx.via.map(|v| v.0)))
        .and_then(|d| jobs.iter().find_map(|j| j.ctx.via.filter(|v| v.0 == d)));
    let seeds: Vec<Point> = jobs
        .iter()
        .flat_map(|j| j.ctx.pads.iter().map(|p| p.center))
        .collect();
    let probe = Probe {
        width: common_width,
        clearance: rules.spec().clearance,
        via: common_via,
        via_cost: options.via_cost,
    };
    let space = GlobalSpace::new(&world, bounds, tile, probe, &seeds);
    if options.global {
        let nets: Vec<(NetId, Vec<TerminalPads>)> = jobs
            .iter()
            .map(|j| {
                let terms = (0..j.ctx.terminals)
                    .map(|t| {
                        j.ctx
                            .pads
                            .iter()
                            .filter(|p| p.terminal == t)
                            .map(|p| (p.center, p.layers.iter().map(|l| l.0).collect()))
                            .collect()
                    })
                    .collect();
                (j.ctx.net, terms)
            })
            .collect();
        let result = global::route(&space, &nets, 30);
        stats.global_overused = result.overused;
        let margin = options.corridor_margin as usize;
        for (job, route) in jobs.iter_mut().zip(result.routes) {
            job.corridor = route
                .filter(|r| !r.is_empty())
                .map(|r| space.corridor(&r, margin));
        }
    }
    let mut history = History {
        grid: space.grid(),
        cost: vec![0.0; space.grid().cells.len()],
    };

    // Detailed routing, with rip-up.
    let mut copper: Vec<Option<NetCopper>> = vec![None; jobs.len()];
    let mut ripped = vec![0u32; jobs.len()];
    let mut queue: VecDeque<usize> = (0..jobs.len()).collect();
    let by_net: BTreeMap<NetId, usize> = jobs
        .iter()
        .enumerate()
        .map(|(i, j)| (j.ctx.net, i))
        .collect();
    let mut outside = vec![false; jobs.len()];
    // A second pass, on a finer lattice, for the nets the first leaves unrouted, with every
    // net that can be ripped up again.
    for pass in 0..2 {
        let params = if pass == 0 {
            params
        } else {
            let left: Vec<usize> = (0..jobs.len()).filter(|&j| copper[j].is_none()).collect();
            if left.is_empty() || !options.fine_pass {
                break;
            }
            stats.fine_pass = left.len();
            queue.extend(left);
            for (j, job) in jobs.iter().enumerate() {
                ripped[j] = 0;
                world.set_locked(job.ctx.net, false);
            }
            Params {
                pitch: (pitch / FINE).max(1000),
                under_pads: true,
                ..params
            }
        };
        while let Some(j) = queue.pop_front() {
            let job = &jobs[j];
            let mut tree = None;
            // The world doesn't change until the net is placed.
            let steps = StepCache::default();
            // A search that runs out of nodes in the corridor would over the board too.
            let mut out_of_budget = false;
            if let Some(corridor) = &job.corridor {
                let rip = RipUp {
                    penalty: None,
                    history: &history,
                };
                match detail::route_net(&world, &job.ctx, &steps, params, Some(corridor), Some(rip))
                {
                    Ok(t) => {
                        stats.expanded += t.expanded;
                        tree = Some(t);
                    }
                    Err((f, e)) => {
                        stats.expanded += e;
                        out_of_budget = f == Failure::Budget;
                    }
                }
            }
            if tree.is_none() && !out_of_budget {
                let rip = RipUp {
                    penalty: None,
                    history: &history,
                };
                let checked = Params {
                    precheck: PRECHECK,
                    ..params
                };
                match detail::route_net(&world, &job.ctx, &steps, checked, None, Some(rip)) {
                    Ok(t) => {
                        stats.expanded += t.expanded;
                        outside[j] = job.corridor.is_some();
                        tree = Some(t);
                    }
                    Err((_, e)) => stats.expanded += e,
                }
            }
            if tree.is_none() && options.rip_ups > 0 {
                let rip = RipUp {
                    penalty: Some(RIP_PENALTY),
                    history: &history,
                };
                match detail::route_net(&world, &job.ctx, &steps, params, None, Some(rip)) {
                    Ok(t) => {
                        stats.expanded += t.expanded;
                        let (victims, at) = detail::victims(&world, &job.ctx, &t);
                        for p in at {
                            if let Some(tile) = history.grid.tile_of(p) {
                                history.cost[tile] += HISTORY_STEP;
                            }
                        }
                        for v in victims {
                            let Some(&k) = by_net.get(&v) else { continue };
                            if let Some(mut c) = copper[k].take() {
                                c.unplace(&mut world);
                                ripped[k] += 1;
                                stats.rip_ups += 1;
                                if ripped[k] >= options.rip_ups {
                                    world.set_locked(v, true);
                                }
                                queue.push_back(k);
                            }
                        }
                        outside[j] = job.corridor.is_some();
                        tree = Some(t);
                    }
                    Err((_, e)) => stats.expanded += e,
                }
            }
            if let Some(t) = tree {
                let mut c = NetCopper::from_tree(&job.ctx, &t);
                c.place(&mut world, &job.ctx);
                copper[j] = Some(c);
            }
        }
    }
    // A net ripped up and then not routed again is unrouted too.
    let mut unrouted: Vec<NetId> = (0..jobs.len())
        .filter(|&j| copper[j].is_none())
        .map(|j| jobs[j].ctx.net)
        .collect();
    unrouted.sort_unstable();
    stats.outside_corridor = (0..jobs.len())
        .filter(|&j| outside[j] && copper[j].is_some())
        .count();

    // The escapes and pairs of nets routed are part of their routes.
    let kept = |net: Option<NetId>| net.is_some_and(|n| unrouted.binary_search(&n).is_err());
    let mut tracks: Vec<Track> = escapes
        .iter()
        .filter(|e| kept(Some(e.net)))
        .flat_map(|e| e.tracks.iter().cloned())
        .collect();
    let mut vias: Vec<Via> = escapes
        .iter()
        .filter(|e| kept(Some(e.net)))
        .filter_map(|e| e.via.clone())
        .collect();
    let fixed_length: f64 = tracks.iter().map(track_length).sum();
    let fixed_vias = vias.len();
    let measure = |copper: &[Option<NetCopper>]| -> (usize, f64) {
        copper
            .iter()
            .flatten()
            .fold((fixed_vias, fixed_length), |(v, l), c| {
                (v + c.vias.len(), l + c.length())
            })
    };
    stats.before_cleanup = measure(&copper);
    if options.cleanup {
        stats.expanded += cleanup::run(&mut world, &jobs, &mut copper, params, options);
    }
    // Pairs side by side, in place of the routes they were given as two nets.
    let mut paired = (0, 0.0);
    if options.couple_pairs {
        for pair in &design.pairs {
            if let Some(route) = couple_in_place(
                design,
                &mut world,
                pair,
                &jobs,
                &by_net,
                &mut copper,
                params,
            ) {
                stats.pairs.1 += 1;
                paired.0 += route.vias.len();
                paired.1 += route.tracks.iter().map(track_length).sum::<f64>();
                tracks.extend(route.tracks);
                vias.extend(route.vias);
            }
        }
    }
    let (v, l) = measure(&copper);
    stats.after_cleanup = (v + paired.0, l + paired.1);
    stats.routed = copper.iter().flatten().count();
    stats.connected = (0..jobs.len())
        .filter(|&j| copper[j].is_some())
        .map(|j| jobs[j].ctx.terminals - 1)
        .sum();
    let top = LayerId(0);
    let bottom = LayerId((rules.layer_count() - 1) as u8);
    for (job, c) in jobs.iter().zip(&copper) {
        let Some(c) = c else { continue };
        let net = Some(job.ctx.net);
        for s in &c.segs {
            tracks.push(Track {
                start: s.a,
                end: s.b,
                mid: None,
                width: job.ctx.widths[usize::from(s.l)],
                layer: LayerId(s.l),
                net,
            });
        }
        if let Some((diameter, drill)) = job.ctx.via {
            for &at in &c.vias {
                vias.push(Via {
                    at,
                    drill,
                    diameter,
                    from: top,
                    to: bottom,
                    net,
                });
            }
        }
    }
    Ok(Routed {
        tracks,
        vias,
        unrouted,
        stats,
    })
}
