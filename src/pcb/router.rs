//! The whole run: nets from the design, global routing, detailed routing with rip-up, and
//! cleanup.

use std::collections::{BTreeMap, VecDeque};

use super::cleanup;
use super::detail::{self, Corridor, History, NetCtx, Params, RipUp, TermPad};
use super::escape;
use super::geometry::NetCopper;
use super::global::{self, GlobalSpace, Probe, TerminalPads};
use super::world::{Verdict, World};
use super::{Design, Options, RouteError, Routed, Stats};
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

/// How much finer the lattice of the second pass is.
const FINE: i64 = 2;

/// How many pitches from a pad routes may move onto its lattice.
const LATTICE_REACH: i64 = 8;

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
    let mut world = world_of(design)?;
    let mut jobs = jobs(design, &world, pitch);
    jobs.sort_by_key(|j| (j.span, j.ctx.net));

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

    let mut stats = Stats {
        nets: jobs.len(),
        connections: jobs.iter().map(|j| j.ctx.terminals - 1).sum(),
        ..Stats::default()
    };

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

    let params = Params {
        pitch,
        via_cost: options.via_cost,
        layer_bias: options.layer_bias,
        bounds,
        max_expansions: options.max_expansions,
        under_pads: false,
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
            if let Some(corridor) = &job.corridor {
                let rip = RipUp {
                    penalty: None,
                    history: &history,
                };
                match detail::route_net(&world, &job.ctx, params, Some(corridor), Some(rip)) {
                    Ok(t) => {
                        stats.expanded += t.expanded;
                        tree = Some(t);
                    }
                    Err((_, e)) => stats.expanded += e,
                }
            }
            if tree.is_none() {
                let rip = RipUp {
                    penalty: None,
                    history: &history,
                };
                match detail::route_net(&world, &job.ctx, params, None, Some(rip)) {
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
                match detail::route_net(&world, &job.ctx, params, None, Some(rip)) {
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

    // The escapes of nets routed are part of their routes.
    let escaped: Vec<&escape::Escape> = escapes
        .iter()
        .filter(|e| by_net.get(&e.net).is_some_and(|&j| copper[j].is_some()))
        .collect();
    let escape_length: f64 = escaped
        .iter()
        .flat_map(|e| &e.tracks)
        .map(|t| {
            let (dx, dy) = ((t.end.x - t.start.x) as f64, (t.end.y - t.start.y) as f64);
            (dx * dx + dy * dy).sqrt()
        })
        .sum();
    let measure = |copper: &[Option<NetCopper>]| -> (usize, f64) {
        copper.iter().flatten().fold(
            (
                escaped.iter().filter(|e| e.via.is_some()).count(),
                escape_length,
            ),
            |(v, l), c| (v + c.vias.len(), l + c.length()),
        )
    };
    stats.before_cleanup = measure(&copper);
    if options.cleanup {
        stats.expanded += cleanup::run(&mut world, &jobs, &mut copper, params, options);
    }
    stats.after_cleanup = measure(&copper);
    stats.routed = copper.iter().flatten().count();
    stats.connected = (0..jobs.len())
        .filter(|&j| copper[j].is_some())
        .map(|j| jobs[j].ctx.terminals - 1)
        .sum();

    let mut tracks: Vec<Track> = escaped.iter().flat_map(|e| e.tracks.clone()).collect();
    let mut vias: Vec<Via> = escaped.iter().filter_map(|e| e.via.clone()).collect();
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
