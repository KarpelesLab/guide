//! The whole run: nets from the design, escapes, global routing, detailed routing with
//! rip-up, cleanup, pairs side by side, and meanders.

use std::collections::{BTreeMap, VecDeque};

use super::cleanup;
use super::control::{Phase, Run};
use super::detail::{
    self, Corridor, Failure, History, NetCtx, NetRun, Params, RipUp, StepCache, TermPad,
};
use super::escape;
use super::geometry::NetCopper;
use super::global::{self, GlobalSpace, Probe, TerminalPads};
use super::pairs;
use super::planes;
use super::report::{self, Known};
use super::tune;
use super::world::{Placed, Planes, Verdict, World};
use super::zones::{self, Fill};
use super::{CopperKind, Design, DiffPair, RouteError, RouteOptions, Routed, RunControl, Stats};
use crate::geom::drc::{Board, Track, Via};
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

/// How many times a pair routed first may be moved out of a net's way, and routed again.
const PAIR_RIP_UPS: u32 = 4;

/// How many times its budget a net's search that may cross others' routes has on the finer
/// lattice.
const FINE_RIP_BUDGET: u64 = 4;

/// How many terminals a net has at least for rip-up, in the passes on the finer lattice, to
/// take out only its branches in the way of the net that crosses it, and join it up again
/// from what is left. A net of fewer is taken out whole and routed again from nothing,
/// which finds it another way altogether where it is in the way again and again; a net of
/// very many pads, a supply or a ground routed as tracks, has most of its tree nowhere
/// near what crosses it, and routing all of it again costs many times the nets it makes
/// room for. In the first pass every net is taken out whole: that is cheap there, and has
/// the nets that crossed it routed before it comes back. At 8 terminals, a board of two
/// layers lost its ground net of 24 pads, taken out more often and then with no way back.
const BRANCH_TERMINALS: usize = 32;

/// How many times in a pass a net may lose branches only, before it is taken out whole.
const BRANCH_CUTS: u32 = 8;

/// How much finer the lattice of the second pass is.
const FINE: i64 = 2;

/// How many passes on the finer lattice there are at most, each for the nets the one
/// before left, with every net free to be ripped up again.
const FINE_PASSES: usize = 2;

/// How many pitches from a pad routes may move onto its lattice.
pub(crate) const LATTICE_REACH: i64 = 8;

/// A point inside a pad, for its routes to end at: the centre of its box if that is in
/// the pad, as for any convex pad, or else the first of a grid of points in the box.
pub(crate) fn pad_center(shape: &Shape) -> Option<Point> {
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

/// The layers a pad has copper on at `center`, a point of its shape: its shape's, and
/// those of the shapes of its stack that hold the point.
pub(crate) fn layers_at(pad: &crate::geom::drc::Pad, center: Point) -> LayerSet {
    if pad.stack.is_empty() {
        return pad.layers;
    }
    let dot = Shape::circle(center, 2);
    pad.stack.iter().fold(pad.layers, |all, (layers, shape)| {
        if dot.as_ref().is_ok_and(|d| shape.touches(d)) {
            LayerSet(all.0 | layers.0)
        } else {
            all
        }
    })
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
pub(crate) fn terminals(design: &Design, net: NetId) -> Vec<Group> {
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
    // The other shapes of pads' stacks, last, each one piece with its pad.
    let mut parent: Vec<usize> = (0..shapes.len()).collect();
    for (k, &i) in pads.iter().enumerate() {
        for (layers, shape) in &b.pads[i].stack {
            parent.push(k);
            shapes.push((shape.clone(), *layers));
        }
    }
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

/// The jobs, in no particular order yet. A net with planned fills in `planes` has the
/// terminal of its main fill first, which also holds places in the fill near its other
/// terminals' pads, where a via or a track's end fits, so that routes from it to them are
/// stubs to the fill.
fn jobs(
    design: &Design,
    world: &World<'_>,
    pitch: i64,
    planes: &[(NetId, Vec<planes::Piece>)],
) -> Vec<Job> {
    let rules = &design.rules;
    let layers = rules.layer_count();
    let mut out = Vec::new();
    for (i, spec) in design.nets.iter().enumerate() {
        let net = NetId(i as u32);
        if !spec.route {
            continue;
        }
        // A run that has to stop routes nothing more: what is left is counted on the board.
        if world.run().is_some_and(Run::poll) {
            break;
        }
        let mut groups = terminals(design, net);
        // The main fill's terminal goes first, empty if the fill reaches no pad.
        let mut plane: Option<Vec<&planes::Piece>> = None;
        if let Some((_, fills)) = planes.iter().find(|(n, _)| *n == net) {
            let pieces = planes::pieces(&design.board, design, net, fills);
            if let Some(m) = pieces.main(fills) {
                match pieces.pads[m]
                    .first()
                    .and_then(|p| groups.iter().position(|(g, _)| g.contains(p)))
                {
                    Some(g) => {
                        let main = groups.remove(g);
                        groups.insert(0, main);
                    }
                    None => groups.insert(0, (Vec::new(), Vec::new())),
                }
                plane = Some(pieces.fills[m].iter().map(|&f| &fills[f]).collect());
            }
        }
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
        for (k, (group, ends)) in groups.iter().enumerate() {
            let before = pads.len();
            for &p in group {
                let pad = &design.board.pads[p];
                if let Some(center) = pad_center(&pad.shape)
                    && pad.layers.0 & rules.layers().0 != 0
                {
                    let on = LayerSet(layers_at(pad, center).0 & rules.layers().0);
                    let center = access_point(world, net, &pad.shape, center, on, &widths);
                    pads.push(TermPad {
                        center,
                        layers: on,
                        bbox: pad.shape.bbox(),
                        terminal: t,
                    });
                }
            }
            if pads.len() > before || (k == 0 && plane.is_some()) {
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
        let via = spec
            .via
            .filter(|_| layers > 1)
            .map(|v| (v.diameter, v.drill));
        let mut span = None;
        if let Some(targets) = &plane {
            let others: Vec<TermPad> = pads.iter().filter(|p| p.terminal != 0).cloned().collect();
            let mut longest = 0;
            for p in &others {
                let sites = planes::sites(
                    world,
                    design,
                    net,
                    core::slice::from_ref(p),
                    targets,
                    pitch,
                    0,
                );
                if let Some(s) = sites.first() {
                    let d = (s.center.x - p.center.x).abs() + (s.center.y - p.center.y).abs();
                    longest = longest.max(d);
                }
                for s in sites {
                    if !pads
                        .iter()
                        .any(|q| q.terminal == 0 && q.center == s.center && q.layers == s.layers)
                    {
                        pads.push(s);
                    }
                }
            }
            if !pads.iter().any(|p| p.terminal == 0) {
                for p in &mut pads {
                    p.terminal -= 1;
                }
                t -= 1;
            } else {
                span = Some(longest);
            }
        }
        if t < 2 {
            continue;
        }
        let span = span.unwrap_or_else(|| {
            let bbox = pads
                .iter()
                .map(|p: &TermPad| Rect::new(p.center, p.center))
                .reduce(|a, b| a.union(&b))
                .unwrap_or_default();
            (bbox.max().x - bbox.min().x) + (bbox.max().y - bbox.min().y)
        });
        out.push(Job {
            ctx: NetCtx::new(net, widths, via, pads, t, LATTICE_REACH * pitch),
            corridor: None,
            span,
        });
    }
    out
}

/// A net's copper as the board's tracks.
fn copper_tracks<'c>(ctx: &'c NetCtx, c: &'c NetCopper) -> impl Iterator<Item = Track> + 'c {
    c.segs.iter().map(move |s| Track {
        start: s.a,
        end: s.b,
        mid: None,
        width: ctx.widths[usize::from(s.l)],
        layer: LayerId(s.l),
        net: Some(ctx.net),
    })
}

/// A net's copper as the board's vias, through every layer.
fn copper_vias<'c>(
    design: &Design,
    ctx: &'c NetCtx,
    c: &'c NetCopper,
) -> impl Iterator<Item = Via> + 'c {
    let bottom = LayerId((design.rules.layer_count() - 1) as u8);
    ctx.via.into_iter().flat_map(move |(diameter, drill)| {
        c.vias.iter().map(move |&at| Via {
            at,
            drill,
            diameter,
            from: LayerId(0),
            to: bottom,
            net: Some(ctx.net),
        })
    })
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
    // Each choice of the pair's ends in turn, until one is routed and leaves the terminals
    // it doesn't end at a way to it.
    for choice in 0..pairs::END_CHOICES {
        let Some(route) = pairs::route_pair(
            design, world, pair, &jobs[jp], &jobs[jn], params, choice, None,
        ) else {
            continue;
        };
        match join_rest(world, &route, [&jobs[jp].ctx, &jobs[jn].ctx], params) {
            Some(rest) => {
                copper[jp] = Some(NetCopper::default());
                copper[jn] = Some(NetCopper::default());
                for (k, c) in rest {
                    copper[[jp, jn][k]] = Some(c);
                }
                return Some(route);
            }
            None => route.undo.undo(world),
        }
    }
    restore(world, copper, old);
    None
}

/// Joins the terminals that a pair's ends leave out to its copper, or to each other, for
/// the positive and the negative net, and returns the copper that does, by which of the
/// two it is for, placed in the world. If either net can't be, nothing is placed.
fn join_rest(
    world: &mut World<'_>,
    route: &pairs::PairRoute,
    nets: [&NetCtx; 2],
    params: Params,
) -> Option<Vec<(usize, NetCopper)>> {
    let mut rest: Vec<(usize, NetCopper)> = Vec::new();
    for (k, ctx) in nets.into_iter().enumerate() {
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
                rest.push((k, c));
            }
            Err(_) => {
                for (_, mut c) in rest {
                    c.unplace(world);
                }
                return None;
            }
        }
    }
    Some(rest)
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

pub(crate) fn run(
    design: &Design,
    options: &RouteOptions,
    control: &RunControl<'_>,
) -> Result<Routed, RouteError> {
    report::validate(design)?;
    let run = Run::new(control, options.work_budget);
    run.phase(Phase::Prepare);
    run.poll();
    let (mut routed, failed) = route_all(design, options, &run)?;
    if options.tune_lengths && !design.lengths.is_empty() && !run.poll() {
        run.phase(Phase::Lengths);
        tune::run(design, &mut routed, options).map_err(RouteError::Geometry)?;
        // Meanders change what zones fill around, and may cut a plane in pieces, which are
        // joined up again as after routing.
        if routed.stats.meanders > 0.0 && !design.zones.zones.is_empty() {
            run.phase(Phase::Zones);
            if options.planes && design.zones.zones.iter().any(|z| z.net.is_some()) {
                let board = with_routes(design, &routed.tracks, &routed.vias);
                let mut world = World::new(
                    &board,
                    &design.rules,
                    &design.outline,
                    design.edge_clearance,
                    &design.obstacles,
                )
                .map_err(RouteError::Geometry)?;
                world.configure(design, options);
                let is_plane = plane_zones(design, options)?;
                world.set_run(&run);
                let mut joined = Joined {
                    tracks: &mut routed.tracks,
                    vias: &mut routed.vias,
                    track_kinds: &mut routed.track_kinds,
                    via_kinds: &mut routed.via_kinds,
                    stats: &mut routed.stats,
                    unrouted: &mut routed.unrouted,
                };
                join_planes(
                    design,
                    &mut world,
                    &mut joined,
                    pitch(design, options),
                    options,
                    &is_plane,
                )?;
            }
            routed.fills =
                zones::fill_zones(&with_routes(design, &routed.tracks, &routed.vias), design)
                    .map_err(RouteError::Geometry)?;
        }
    }
    // What the result leaves apart, counted on the board as it is now.
    run.phase(Phase::Report);
    let board = routed.board(design);
    let survey = report::survey(design, &board);
    let stopped = run.stopped().is_some();
    if stopped || (options.planes && !design.zones.zones.is_empty()) {
        let s = &mut routed.stats;
        (s.nets, s.routed, s.connections, s.connected) =
            (survey.nets, survey.done, survey.needed, survey.made);
    }
    if stopped {
        // A run that was stopped may not have got to every net, or to joining the pieces
        // of those with zones: what is unrouted is what the board leaves in pieces.
        let zoned = |n: &&NetId| design.zones.zones.iter().any(|z| z.net == Some(**n));
        routed.stats.planes_broken = survey.broken.iter().filter(zoned).count();
        routed.unrouted.clone_from(&survey.broken);
    }
    routed.stats.islands = (survey.needed, survey.made);
    run.poll();
    if !survey.open.is_empty() {
        // The world as the result leaves it: every route in it, and none locked.
        let mut world = World::new(
            &design.board,
            &design.rules,
            &design.outline,
            design.edge_clearance,
            &design.obstacles,
        )
        .map_err(RouteError::Geometry)?;
        world.configure(design, options);
        for t in &routed.tracks {
            if let (Some(net), Ok(shape)) = (t.net, Shape::track(t.start, t.end, t.width)) {
                world.add_track(net, t.layer, shape);
            }
        }
        for v in &routed.vias {
            if let Some(net) = v.net {
                world.add_via(net, v.at, v.diameter, v.drill);
            }
        }
        let fine = options.fine_pass && run.stopped().is_none();
        let base = pitch(design, options);
        let known = Known {
            failed: &failed,
            stopped: run.stopped(),
            pitch: if fine { (base / FINE).max(1000) } else { base },
            bounds: bounds_of(design),
        };
        routed.open = report::explain(design, &world, survey.open, &known);
    }
    routed.stats.work = run.work();
    routed.stopped = run.stopped();
    Ok(routed)
}

/// The lattice's pitch: half the tightest pitch of the tracks routed, unless the options
/// set one.
pub(crate) fn pitch(design: &Design, options: &RouteOptions) -> i64 {
    let rules = &design.rules;
    options.pitch.unwrap_or_else(|| {
        (0..design.nets.len())
            .filter(|&i| design.nets[i].route)
            .map(|i| {
                let net = Some(NetId(i as u32));
                design.nets[i]
                    .track_width
                    .max(rules.track_width(net, LayerId(0)))
                    + rules.clearance(net, LayerId(0))
            })
            .min()
            .map_or(100_000, |p| (p / 2).max(1000))
    })
}

/// Where routes may go: the board's edge's box, or failing that, the pads' grown by 5 mm.
fn bounds_of(design: &Design) -> Rect {
    let pad_bounds = design
        .board
        .pads
        .iter()
        .map(|p| p.shape.bbox())
        .reduce(|a, b| a.union(&b));
    design
        .outline
        .iter()
        .map(Shape::bbox)
        .reduce(|a, b| a.union(&b))
        .or(pad_bounds.map(|b| b.expand(5_000_000)))
        .unwrap_or_default()
}

/// A design's board with tracks and vias added.
fn with_routes(design: &Design, tracks: &[Track], vias: &[Via]) -> Board {
    let mut board = design.board.clone();
    board.tracks.extend(tracks.iter().cloned());
    board.vias.extend(vias.iter().cloned());
    board
}

/// What a board with everything routed and filled leaves of a design's nets: the nets
/// that need connecting, of those the nets connected, the connections needed and made, and
/// the nets left in pieces, in order. A net needs connecting if its pads are in pieces that
/// fixed copper doesn't join, and a connection is made for each piece fewer that its copper,
/// routes and fills leave it in.
pub(crate) fn connectivity(
    design: &Design,
    board: &Board,
) -> (usize, usize, usize, usize, Vec<NetId>) {
    let s = report::survey(design, board);
    (s.nets, s.done, s.needed, s.made, s.broken)
}

/// What [`join_planes`] adds to.
struct Joined<'a> {
    tracks: &'a mut Vec<Track>,
    vias: &'a mut Vec<Via>,
    track_kinds: &'a mut Vec<CopperKind>,
    via_kinds: &'a mut Vec<CopperKind>,
    stats: &'a mut Stats,
    unrouted: &'a mut Vec<NetId>,
}

/// Fills the zones around everything routed, and joins up the pieces of their nets that
/// the fills leave apart, a few times at most; what can't be joined is unrouted. Islands
/// are kept until the end, as a plane that nothing reaches yet is still one to reach, and
/// they join nothing.
fn join_planes(
    design: &Design,
    world: &mut World<'_>,
    out: &mut Joined<'_>,
    pitch: i64,
    options: &RouteOptions,
    is_plane: &[bool],
) -> Result<(), RouteError> {
    let keeping = keep_islands(design);
    let plane_net = |net: NetId| {
        let mut zones = design.zones.zones.iter().zip(is_plane);
        zones.any(|(z, &plane)| plane && z.net == Some(net))
    };
    for round in 0..=REPAIRS {
        // A run that has to stop leaves the pieces as they are.
        let stopped = world.run().is_some_and(Run::poll);
        let board = with_routes(design, out.tracks, out.vias);
        let all = zones::fill_zones(&board, &keeping).map_err(RouteError::Geometry)?;
        let mut added = false;
        let mut left = Vec::new();
        for (net, pieces) in planes::pieces_by_net(design, &all) {
            if !design.nets.get(net.index()).is_some_and(|n| n.route)
                || !plane_net(net)
                || planes::pieces(&board, design, net, &pieces).with_pads() < 2
            {
                continue;
            }
            left.push(net);
            if round == REPAIRS || stopped {
                continue;
            }
            let joins = planes::connect(
                design,
                world,
                &board,
                net,
                &pieces,
                pitch,
                options.via_cost,
                true,
            );
            for s in joins {
                out.stats.after_cleanup.0 += s.vias.len();
                out.stats.after_cleanup.1 += s.tracks.iter().map(track_length).sum::<f64>();
                out.track_kinds
                    .extend(s.tracks.iter().map(|_| CopperKind::Stub));
                out.via_kinds
                    .extend(s.vias.iter().map(|_| CopperKind::Stub));
                out.tracks.extend(s.tracks);
                out.vias.extend(s.vias);
                out.stats.stubs += 1;
                added = true;
            }
        }
        // What nothing more joins is left in pieces.
        if !added || round == REPAIRS || stopped {
            out.stats.planes_broken = left.len();
            out.unrouted.extend(left);
            out.unrouted.sort_unstable();
            out.unrouted.dedup();
            break;
        }
    }
    Ok(())
}

/// The fills of a net that its stubs and grouping take as connected before routing: its
/// fills on inner layers, its planes, or if it has none, all of them.
fn planned_fills(design: &Design, fills: &[Fill], is_plane: &[bool]) -> Vec<Fill> {
    let layers = design.rules.layer_count();
    let net_of = |f: &Fill| design.zones.zones.get(f.zone).and_then(|z| z.net);
    let fills: Vec<&Fill> = fills
        .iter()
        .filter(|f| is_plane.get(f.zone).copied().unwrap_or(true))
        .collect();
    fills
        .iter()
        .copied()
        .filter(|f| {
            let net = net_of(f);
            net.is_some()
                && (planes::inner(f.layer, layers)
                    || !fills
                        .iter()
                        .any(|g| net_of(g) == net && planes::inner(g.layer, layers)))
        })
        .cloned()
        .collect()
}

/// By zone, whether its net plans on it as a plane: as the zone [says](super::CopperZone::plane),
/// or if its net's fills cover enough of the box of the net's pads,
/// [`RouteOptions::plane_coverage`], on the layer where they cover the most. With neither
/// set, every zone is, and nothing is filled to find out.
pub(crate) fn plane_zones(
    design: &Design,
    options: &RouteOptions,
) -> Result<Vec<bool>, RouteError> {
    let zones = &design.zones.zones;
    let auto = options.plane_coverage > 0.0;
    if !auto {
        return Ok(zones.iter().map(|z| z.plane.unwrap_or(true)).collect());
    }
    let fills =
        zones::fill_zones(&design.board, &keep_islands(design)).map_err(RouteError::Geometry)?;
    let layers = design.rules.layer_count();
    // By net, twice the area of its fills on each layer, and the box of its pads.
    let mut area: BTreeMap<NetId, Vec<i128>> = BTreeMap::new();
    for f in &fills {
        // A zone that says which it is doesn't count.
        let Some(net) = zones
            .get(f.zone)
            .filter(|z| z.plane.is_none())
            .and_then(|z| z.net)
        else {
            continue;
        };
        if let Ok(a) = crate::geom::Area::of_shape(&f.shape, 0, 1, crate::geom::ArcSide::Inside)
            && let Some(slot) = area
                .entry(net)
                .or_insert_with(|| vec![0; layers])
                .get_mut(usize::from(f.layer.0))
        {
            *slot += a.area2();
        }
    }
    let mut boxes: BTreeMap<NetId, Rect> = BTreeMap::new();
    for p in &design.board.pads {
        if let Some(net) = p.net {
            let b = p.shape.bbox();
            boxes
                .entry(net)
                .and_modify(|r| *r = r.union(&b))
                .or_insert(b);
        }
    }
    let covers = |net: NetId| {
        let most = area
            .get(&net)
            .and_then(|a| a.iter().copied().max())
            .unwrap_or(0);
        match boxes.get(&net).map(Rect::area) {
            Some(b) if b > 0 => most as f64 / 2.0 / b as f64 >= options.plane_coverage,
            // A net with no pad has nothing to connect, and its zones change nothing.
            _ => true,
        }
    };
    Ok(zones
        .iter()
        .map(|z| z.plane.unwrap_or_else(|| z.net.is_none_or(covers)))
        .collect())
}

/// The fills of the zones that are not planes, as the board has them before routing, as
/// fixed copper of their nets: what routes of other nets keep clear of, and what joins the
/// pads it touches.
fn fixed_fills(
    design: &Design,
    is_plane: &[bool],
) -> Result<Vec<crate::geom::drc::Zone>, RouteError> {
    let fills = zones::fill_zones(&design.board, design).map_err(RouteError::Geometry)?;
    let mut out = Vec::new();
    for f in &fills {
        let Some(zone) = design.zones.zones.get(f.zone) else {
            continue;
        };
        if zone.net.is_none() || is_plane.get(f.zone).copied().unwrap_or(true) {
            continue;
        }
        // The zone's own clearance, where it is more than its net's, so that the fill
        // around the routes is the same.
        let clearance =
            Some(zone.clearance).filter(|&c| c > design.rules.clearance(zone.net, f.layer));
        out.push(crate::geom::drc::Zone {
            shape: f.shape.clone(),
            layer: f.layer,
            net: zone.net,
            clearance,
        });
    }
    Ok(out)
}

/// The same design, with zones that keep their islands.
fn keep_islands(design: &Design) -> Design {
    let mut d = design.clone();
    for z in &mut d.zones.zones {
        z.islands = super::Islands::Keep;
    }
    d
}

/// How many times zones are filled again and their nets' pieces joined after routing.
const REPAIRS: usize = 3;

/// Routes everything but meanders. Returns, with the result, how the last search of each
/// net that failed ended, for the nets left open.
fn route_all<'a>(
    design: &'a Design,
    options: &RouteOptions,
    run: &'a Run<'a>,
) -> Result<(Routed, BTreeMap<NetId, Failure>), RouteError> {
    let original = design;
    let rules = &design.rules;
    let world_of = |design: &Design| {
        let mut world = World::new(
            &design.board,
            rules,
            &design.outline,
            design.edge_clearance,
            &design.obstacles,
        )
        .map_err(RouteError::Geometry)?;
        world.configure(design, options);
        world.set_run(run);
        Ok::<_, RouteError>(world)
    };

    let pitch = pitch(design, options);
    // Zones that are not planes are copper as they are filled before anything is routed.
    let planes_on = options.planes && design.zones.zones.iter().any(|z| z.net.is_some());
    let is_plane = if planes_on {
        plane_zones(design, options)?
    } else {
        vec![true; design.zones.zones.len()]
    };
    let with_fixed;
    let design = if is_plane.iter().all(|&p| p) {
        design
    } else {
        let mut d = design.clone();
        d.board.zones.extend(fixed_fills(design, &is_plane)?);
        with_fixed = d;
        &with_fixed
    };
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
    // Pairs first, if asked, side by side on the board as it is before anything else is
    // routed, as copper of their nets that the rest completes, and that only the passes on
    // the finer lattice may move.
    let mut pairs_first = PairsFirst::default();
    let with_pairs;
    let before_pairs = design;
    let design = if options.couple_pairs && options.pairs_first && !design.pairs.is_empty() {
        let mut world = world_of(design)?;
        pairs_first.jobs = jobs(design, &world, pitch, &[]);
        pairs_first.params = Some(Params {
            pitch,
            via_cost: options.via_cost,
            layer_bias: options.layer_bias,
            bounds: bounds_of(design),
            max_expansions: options.max_expansions,
            under_pads: false,
            partial: false,
            stubs: false,
        });
        // The pads of nets with zones near the pairs' ends, which a via takes to their
        // planes, keep a place for it.
        let kept = keep_exits(design, &mut world, pitch, &pairs_first.jobs);
        for pair in &design.pairs {
            let route = if run.poll() {
                None
            } else {
                pairs_first.route(design, &mut world, pair)
            };
            pairs_first.routes.push(route);
        }
        for k in &kept {
            world.remove(k);
        }
        if pairs_first.routes.iter().all(Option::is_none) {
            design
        } else {
            let mut d = design.clone();
            for r in pairs_first.routes.iter().flatten() {
                d.board.tracks.extend(r.tracks.iter().cloned());
                d.board.vias.extend(r.vias.iter().cloned());
            }
            with_pairs = d;
            &with_pairs
        }
    } else {
        design
    };
    // Zones filled around the fixed copper, islands kept, which their nets' terminals
    // take as connected and reach with stubs.
    let planes_on = planes_on && is_plane.iter().any(|&p| p);
    let mut planned: Vec<Fill> = Vec::new();
    if planes_on {
        let fills = zones::fill_zones(&design.board, &keep_islands(design))
            .map_err(RouteError::Geometry)?;
        planned = planned_fills(design, &fills, &is_plane);
    }
    let plane_pieces = planes::pieces_by_net(design, &planned);
    // For grouping pads into terminals only: what the planned fills join is one terminal.
    let grouped;
    let terminal_design = if planned.is_empty() {
        design
    } else {
        let mut d = design.clone();
        d.board
            .zones
            .extend(planned.iter().map(|f| f.to_zone(design)));
        grouped = d;
        &grouped
    };
    let bounds = bounds_of(design);
    let params = Params {
        pitch,
        via_cost: options.via_cost,
        layer_bias: options.layer_bias,
        bounds,
        max_expansions: options.max_expansions,
        under_pads: false,
        partial: false,
        stubs: false,
    };

    // Pairs routed first are routes of their nets in the world, locked until the passes on
    // the finer lattice, where the nets left may move them, and they are routed again.
    let mut world = world_of(before_pairs)?;
    let mut pair_copper: Vec<Option<pairs::Undo>> = Vec::new();
    let mut pair_of: BTreeMap<NetId, usize> = BTreeMap::new();
    for (i, r) in pairs_first.routes.iter().enumerate() {
        pair_copper.push(
            r.as_ref()
                .map(|r| pairs::Undo::place(&mut world, &r.tracks, &r.vias)),
        );
        if r.is_some() {
            for net in [design.pairs[i].positive, design.pairs[i].negative] {
                world.set_locked(net, true);
                pair_of.insert(net, i);
            }
        }
    }
    let mut pair_rips = vec![0u32; pair_copper.len()];
    let pair_nets = pairs_first.nets(design);
    let mut jobs = jobs(terminal_design, &world, pitch, &plane_pieces);
    match options.seed {
        None => jobs.sort_by_key(|j| (j.span, j.ctx.net)),
        Some(seed) => jobs.sort_by_key(|j| (seeded_span(seed, j), j.ctx.net)),
    }
    run.totals(jobs.len(), jobs.iter().map(|j| j.ctx.terminals - 1).sum());
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
    run.phase(Phase::Global);
    if options.global && !run.poll() {
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
        let result = global::route(&space, &nets, 30, run);
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
    if planes_on && options.plane_cost > 0.0 {
        world.set_planes(plane_map(
            design,
            &planned,
            space.grid(),
            options.plane_cost,
        ));
    }

    // Nets with planned fills, whose routes are stubs to them.
    let plane_net = |net: NetId| {
        planned
            .iter()
            .any(|f| design.zones.zones.get(f.zone).and_then(|z| z.net) == Some(net))
    };

    // Detailed routing, with rip-up.
    let mut copper: Vec<Option<NetCopper>> = vec![None; jobs.len()];
    let mut ripped = vec![0u32; jobs.len()];
    // By job, how many times it lost branches only, in this pass.
    let mut cuts = vec![0u32; jobs.len()];
    let mut queue: VecDeque<usize> = (0..jobs.len()).collect();
    let by_net: BTreeMap<NetId, usize> = jobs
        .iter()
        .enumerate()
        .map(|(i, j)| (j.ctx.net, i))
        .collect();
    let mut outside = vec![false; jobs.len()];
    // By job, the terminals its partial route left out, which are those its copper doesn't
    // join to its first, and how many connections it still needs: the pieces its copper
    // leaves its terminals in, less one.
    let mut missing: Vec<Vec<usize>> = vec![Vec::new(); jobs.len()];
    let mut apart: Vec<usize> = vec![0; jobs.len()];
    // By job, for a net routed in part that a later pass completes, the net with its copper
    // so far as its first terminal and the terminals left out as the others.
    let mut completing: Vec<Option<NetCtx>> = vec![None; jobs.len()];
    // By job, how its last search that failed ended, if it is not routed whole since.
    let mut failed: Vec<Option<Failure>> = vec![None; jobs.len()];
    // What the progress hears of: nets with a route, connections made, nets waiting.
    let tell =
        |copper: &[Option<NetCopper>], missing: &[Vec<usize>], apart: &[usize], queued: usize| {
            let (mut nets, mut made) = (0, 0);
            for (j, _) in copper.iter().enumerate().filter(|(_, c)| c.is_some()) {
                nets += usize::from(missing[j].is_empty());
                made += jobs[j].ctx.terminals - 1 - apart[j];
            }
            run.counts(nets, made, queued);
        };
    // A second pass, on a finer lattice, for the nets the first leaves unrouted, or routed in
    // part, with every net that can be ripped up again.
    let mut best: Option<Best> = None;
    'passes: for pass in 0..=FINE_PASSES {
        if run.poll() {
            break;
        }
        let params = if pass == 0 {
            run.phase(Phase::Detail);
            params
        } else {
            let left: Vec<usize> = (0..jobs.len()).filter(|&j| copper[j].is_none()).collect();
            let unfinished: Vec<usize> = (0..jobs.len())
                .filter(|&j| !missing[j].is_empty() && !plane_net(jobs[j].ctx.net))
                .filter(|&j| copper[j].is_some())
                .collect();
            if (left.is_empty() && unfinished.is_empty()) || !options.fine_pass {
                break;
            }
            if pass == 1 {
                stats.fine_pass = left.len() + unfinished.len();
            }
            run.phase(Phase::Fine);
            for &j in &unfinished {
                if let Some(c) = &copper[j] {
                    completing[j] = Some(completion(design, &jobs[j].ctx, c, pitch));
                }
            }
            queue.extend(left);
            queue.extend(unfinished);
            for (j, job) in jobs.iter().enumerate() {
                ripped[j] = 0;
                cuts[j] = 0;
                world.set_locked(job.ctx.net, false);
            }
            // Pairs routed first may be moved, but those that found no way after being
            // moved before.
            for (&net, &i) in &pair_of {
                world.set_locked(net, pair_rips[i] >= PAIR_RIP_UPS);
            }
            Params {
                pitch: (pitch / FINE).max(1000),
                under_pads: options.vias_in_pads,
                ..params
            }
        };
        while let Some(j) = queue.pop_front() {
            tell(&copper, &missing, &apart, queue.len() + 1);
            if run.poll() {
                break 'passes;
            }
            // A net that was cut more than once since its last turn waits more than once,
            // and is whole again after the first.
            if copper[j].is_some() && completing[j].is_none() {
                continue;
            }
            let job = &jobs[j];
            // A net routed in part is completed from its copper, if it still has it.
            let rest = completing[j].take().filter(|_| copper[j].is_some());
            let ctx = rest.as_ref().unwrap_or(&job.ctx);
            // A net of many pads is routed a terminal at a time, as stubs to its plane if it
            // has one.
            let plane = plane_net(job.ctx.net);
            let many = job.ctx.terminals > detail::BUDGET_TERMINALS;
            let params = Params {
                stubs: many,
                ..params
            };
            // The world doesn't change until the net is placed.
            let steps = StepCache::default();
            // One tree through every stage: what the corridor connects is kept by the
            // search over the board, and what that connects by the one that may cross
            // other nets' routes, which then crosses them only for what is left.
            let mut grow = NetRun::new(&world, ctx, &steps, params);
            let mut done = false;
            let mut crossed = false;
            // A search that runs out of nodes in the corridor would over the board too.
            let mut out_of_budget = false;
            // How the last search that failed ended.
            let mut why = None;
            let quiet = RipUp {
                penalty: None,
                history: &history,
            };
            if let Some(corridor) = &job.corridor {
                match grow.stage(params, Some(corridor), Some(quiet)) {
                    Ok(()) => done = true,
                    Err(f) => {
                        out_of_budget = f == Failure::Budget;
                        why = Some(f);
                    }
                }
            }
            if !done && !out_of_budget {
                match grow.stage(params, None, Some(quiet)) {
                    Ok(()) => {
                        outside[j] = job.corridor.is_some();
                        done = true;
                    }
                    Err(f) => why = Some(f),
                }
            }
            // A net with a plane, or of many pads, keeps what it can reach, and a plane's
            // pieces are joined up after routing: one pad shut in doesn't cost it all the
            // others.
            let partial = Params {
                partial: plane || many,
                ..params
            };
            // And in the passes on the finer lattice, a net of more than a few terminals that
            // rip-up can't route whole is searched for a terminal at a time, and keeps what
            // it reaches, too.
            let mut attempts = vec![partial];
            if pass > 0 && !partial.partial && job.ctx.terminals > detail::STUB_TERMINALS {
                attempts.push(Params {
                    partial: true,
                    stubs: true,
                    ..params
                });
            } else if options.keep_partial && !partial.partial && ctx.terminals > 2 {
                // And if asked, any net that can't be routed whole keeps what it reaches.
                attempts.push(Params {
                    partial: true,
                    stubs: true,
                    ..params
                });
            }
            for &partial in &attempts {
                if done || options.rip_ups == 0 {
                    break;
                }
                let rip = RipUp {
                    penalty: Some(RIP_PENALTY),
                    history: &history,
                };
                // On the finer lattice, a search that may cross others' routes has more
                // nodes to go through, among them the pairs', which it may move.
                let partial = Params {
                    max_expansions: if pass > 0 {
                        partial.max_expansions.saturating_mul(FINE_RIP_BUDGET)
                    } else {
                        partial.max_expansions
                    },
                    ..partial
                };
                match grow.stage(partial, None, Some(rip)) {
                    Ok(()) => {
                        outside[j] = job.corridor.is_some();
                        crossed = true;
                        done = true;
                    }
                    Err(f) => why = Some(f),
                }
            }
            let partial = if options.keep_partial && !partial.partial && ctx.terminals > 2 {
                Params {
                    partial: true,
                    stubs: true,
                    ..params
                }
            } else {
                partial
            };
            if !done && partial.partial && options.rip_ups == 0 {
                match grow.stage(partial, None, Some(quiet)) {
                    Ok(()) => done = true,
                    Err(f) => why = Some(f),
                }
            }
            let grown = grow.finish();
            stats.expanded += grown.expanded;
            let tree = done.then_some(grown);
            let mut displaced: Vec<usize> = Vec::new();
            // The nets this one's route runs into, with the copper they had.
            let mut cut: Vec<(usize, NetCopper)> = Vec::new();
            if let Some(t) = tree.as_ref().filter(|_| crossed) {
                let (victims, at) = detail::victims(&world, ctx, t);
                // Taking nets out leaves fewer connections made until they are routed
                // again: the routes as they are now are kept, if none before made more,
                // for a run that is stopped before then.
                let made = made_by(&jobs, &copper, &apart);
                if !victims.is_empty() && best.as_ref().is_none_or(|b| made > b.made) {
                    best = Some(Best {
                        made,
                        copper: copper
                            .iter()
                            .map(|c| c.as_ref().map(NetCopper::detached))
                            .collect(),
                        missing: missing.clone(),
                        apart: apart.clone(),
                    });
                }
                for p in at {
                    if let Some(tile) = history.grid.tile_of(p) {
                        history.cost[tile] += HISTORY_STEP;
                    }
                }
                for v in victims {
                    // A pair in the way is taken out whole, and routed again after.
                    if let Some(&i) = pair_of.get(&v)
                        && let Some(u) = pair_copper[i].take()
                    {
                        u.undo(&mut world);
                        pair_rips[i] += 1;
                        stats.rip_ups += 1;
                        displaced.push(i);
                    }
                    let Some(&k) = by_net.get(&v) else { continue };
                    if let Some(mut c) = copper[k].take() {
                        c.unplace(&mut world);
                        completing[k] = None;
                        failed[k] = None;
                        stats.rip_ups += 1;
                        queue.push_back(k);
                        // A net of many pads loses only its branches in the way, a few
                        // times, before it is taken out whole as any other: a branch
                        // makes room for one net, and the whole of it for all those that
                        // wait, which go before it comes back.
                        if pass > 0
                            && jobs[k].ctx.terminals >= BRANCH_TERMINALS
                            && !plane_net(v)
                            && cuts[k] < BRANCH_CUTS
                        {
                            cuts[k] += 1;
                            cut.push((k, c));
                        } else {
                            ripped[k] += 1;
                            if ripped[k] >= options.rip_ups {
                                world.set_locked(v, true);
                            }
                        }
                    }
                }
            }
            // A search that the run's end cut short says nothing of the net.
            failed[j] = match &tree {
                _ if run.stopped().is_some() => None,
                Some(t) if t.missing.is_empty() => None,
                Some(t) if t.out_of_budget => Some(Failure::Budget),
                Some(_) => Some(Failure::NoPath),
                None => why,
            };
            if let Some(t) = tree {
                let mut c = NetCopper::from_tree(ctx, &t);
                match (&rest, copper[j].take()) {
                    // What completes a net joins the copper it has, and leaves out the
                    // terminals that it does.
                    (Some(_), Some(mut old)) => {
                        old.unplace(&mut world);
                        old.segs.extend(c.segs);
                        for v in c.vias {
                            if !old.vias.contains(&v) {
                                old.vias.push(v);
                            }
                        }
                        old.merge(&job.ctx);
                        c = old;
                        let pieces = c.pieces(&job.ctx);
                        debug_assert_eq!(pieces.apart(), t.missing.len());
                        missing[j] = pieces.missing();
                        apart[j] = pieces.apart();
                    }
                    (_, old) => {
                        if let Some(mut old) = old {
                            old.unplace(&mut world);
                        }
                        missing[j] = t.missing.clone();
                        apart[j] = t.missing.len();
                    }
                }
                c.place(&mut world, &job.ctx);
                copper[j] = Some(c);
            }
            // A net in the way loses only what is in the way: the rest of its copper goes
            // back, in the pieces that leaves it in, and it is joined up again from them
            // when its turn comes, instead of being routed again from nothing.
            for (k, old) in cut {
                let ctx = &jobs[k].ctx;
                let mut kept = old.free_in(&world, ctx, params.pitch);
                kept.prune(ctx);
                if kept.is_empty() {
                    continue;
                }
                kept.place(&mut world, ctx);
                let pieces = kept.pieces(ctx);
                missing[k] = pieces.missing();
                apart[k] = pieces.apart();
                completing[k] = Some(completion(design, ctx, &kept, pitch));
                copper[k] = Some(kept);
            }
            // The pairs this net moved are routed again side by side around it, moving
            // other nets in their way in turn, but not pairs, nor this net. A pair that finds
            // no way goes back where it was, and this net is routed again around it.
            // Routes kept from before a pair moved are of a board that is no more.
            if !displaced.is_empty() {
                best = None;
            }
            for i in displaced {
                let pair = &design.pairs[i];
                let net = job.ctx.net;
                let was = world.locked(net);
                world.set_locked(net, true);
                let open: Vec<NetId> = pair_of
                    .keys()
                    .copied()
                    .filter(|&n| !world.locked(n))
                    .collect();
                for &n in &open {
                    world.set_locked(n, true);
                }
                let mut moved: Vec<NetId> = Vec::new();
                let mut take = |w: &mut World<'_>, nets: &[NetId]| {
                    for &v in nets {
                        let Some(&k) = by_net.get(&v) else { continue };
                        if let Some(mut c) = copper[k].take() {
                            c.unplace(w);
                            moved.push(v);
                        }
                    }
                };
                let route = pairs_first.params.and_then(|p| {
                    let job = |n: NetId| pairs_first.jobs.iter().find(|j| j.ctx.net == n);
                    let rip = pairs::PairRip {
                        rip: RipUp {
                            penalty: Some(RIP_PENALTY),
                            history: &history,
                        },
                        take: &mut take,
                    };
                    pairs::route_pair(
                        design,
                        &mut world,
                        pair,
                        job(pair.positive)?,
                        job(pair.negative)?,
                        p,
                        0,
                        Some(rip),
                    )
                });
                for &n in &open {
                    world.set_locked(n, false);
                }
                world.set_locked(net, was);
                for v in moved {
                    let Some(&k) = by_net.get(&v) else { continue };
                    completing[k] = None;
                    failed[k] = None;
                    ripped[k] += 1;
                    stats.rip_ups += 1;
                    if ripped[k] >= options.rip_ups {
                        world.set_locked(v, true);
                    }
                    queue.push_back(k);
                }
                match route {
                    Some(mut route) => {
                        pair_copper[i] = Some(core::mem::take(&mut route.undo));
                        pairs_first.routes[i] = Some(route);
                    }
                    None => {
                        if let Some(mut c) = copper[j].take() {
                            c.unplace(&mut world);
                            queue.push_back(j);
                        }
                        if let Some(r) = &pairs_first.routes[i] {
                            pair_copper[i] =
                                Some(pairs::Undo::place(&mut world, &r.tracks, &r.vias));
                        }
                        pair_rips[i] = PAIR_RIP_UPS;
                        world.set_locked(pair.positive, true);
                        world.set_locked(pair.negative, true);
                    }
                }
            }
        }
    }
    // A run that was stopped while nets were out goes back to the routes that made the
    // most connections, if what it has now makes fewer.
    if run.stopped().is_some()
        && let Some(b) = best.take()
        && b.made > made_by(&jobs, &copper, &apart)
    {
        for c in copper.iter_mut().flatten() {
            c.unplace(&mut world);
        }
        copper = b.copper;
        for (job, c) in jobs.iter().zip(copper.iter_mut()) {
            if let Some(c) = c {
                c.place(&mut world, &job.ctx);
            }
        }
        missing = b.missing;
        apart = b.apart;
    }
    // A net ripped up and then not routed again is unrouted too.
    // A plane net's pieces are joined after routing, if they can be, so it isn't unrouted
    // yet.
    // A net of many pads left in pieces keeps them, and is unrouted too.
    let mut dropped: Vec<NetId> = (0..jobs.len())
        .filter(|&j| copper[j].is_none())
        .map(|j| jobs[j].ctx.net)
        .filter(|&n| !plane_net(n))
        .collect();
    dropped.sort_unstable();
    let mut unrouted: Vec<NetId> = (0..jobs.len())
        .filter(|&j| copper[j].is_some() && !missing[j].is_empty())
        .map(|j| jobs[j].ctx.net)
        .filter(|&n| !plane_net(n))
        .chain(dropped.iter().copied())
        .collect();
    unrouted.sort_unstable();
    stats.outside_corridor = (0..jobs.len())
        .filter(|&j| outside[j] && copper[j].is_some())
        .count();

    tell(&copper, &missing, &apart, 0);
    // The escapes and pairs of nets routed are part of their routes.
    let kept = |net: Option<NetId>| net.is_some_and(|n| dropped.binary_search(&n).is_err());
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
    let mut track_kinds = vec![CopperKind::Escape; tracks.len()];
    let mut via_kinds = vec![CopperKind::Escape; vias.len()];
    let first_pairs = pairs_first.routes.iter().flatten().count();
    for r in pairs_first.routes.iter().flatten() {
        tracks.extend(r.tracks.iter().filter(|t| kept(t.net)).cloned());
        vias.extend(r.vias.iter().filter(|v| kept(v.net)).cloned());
    }
    track_kinds.resize(tracks.len(), CopperKind::Pair);
    via_kinds.resize(vias.len(), CopperKind::Pair);
    stats.pairs.0 += first_pairs;
    stats.pairs.1 += first_pairs;
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
    // The ends that rip-up left leading nowhere go, before anything is measured.
    for (job, c) in jobs.iter().zip(copper.iter_mut()) {
        if let Some(c) = c {
            c.unplace(&mut world);
            c.prune(&job.ctx);
            c.place(&mut world, &job.ctx);
        }
    }
    stats.before_cleanup = measure(&copper);
    // A run that has to stop keeps the routes as they are.
    if options.cleanup && !run.poll() {
        run.phase(Phase::Cleanup);
        stats.expanded += cleanup::run(&mut world, &jobs, &mut copper, params, options);
    }
    // Pairs side by side, in place of the routes they were given as two nets.
    let mut paired = (0, 0.0);
    if options.couple_pairs && !design.pairs.is_empty() && !run.poll() {
        run.phase(Phase::Pairs);
        for pair in &design.pairs {
            if run.poll() {
                break;
            }
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
                track_kinds.resize(tracks.len(), CopperKind::Pair);
                via_kinds.resize(vias.len(), CopperKind::Pair);
            }
        }
    }
    let (v, l) = measure(&copper);
    stats.after_cleanup = (v + paired.0, l + paired.1);
    stats.routed = (0..jobs.len())
        .filter(|&j| copper[j].is_some() && missing[j].is_empty())
        .count();
    stats.connected = (0..jobs.len())
        .filter(|&j| copper[j].is_some())
        .map(|j| jobs[j].ctx.terminals - 1 - apart[j])
        .sum();
    // Nets that pairs routed first connect whole have no job, but count.
    for &net in &pair_nets {
        let Some(terminals) = pairs_first.terminals(net) else {
            continue;
        };
        if !by_net.contains_key(&net) {
            stats.nets += 1;
            stats.routed += 1;
            stats.connections += terminals - 1;
            stats.connected += terminals - 1;
        }
    }
    for (job, c) in jobs.iter().zip(&copper) {
        let Some(c) = c else { continue };
        tracks.extend(copper_tracks(&job.ctx, c));
        vias.extend(copper_vias(design, &job.ctx, c));
        // The routes of a net with a plane are its stubs to it.
        let kind = if plane_net(job.ctx.net) {
            CopperKind::Stub
        } else {
            CopperKind::Plain
        };
        track_kinds.resize(tracks.len(), kind);
        via_kinds.resize(vias.len(), kind);
    }
    // Zones filled around everything, and the pieces of their nets joined up. Islands are
    // kept until the end, as a plane that nothing reaches yet is still one to reach, and
    // they join nothing.
    if planes_on && !run.poll() {
        run.phase(Phase::Zones);
        let mut joined = Joined {
            tracks: &mut tracks,
            vias: &mut vias,
            track_kinds: &mut track_kinds,
            via_kinds: &mut via_kinds,
            stats: &mut stats,
            unrouted: &mut unrouted,
        };
        join_planes(
            original,
            &mut world,
            &mut joined,
            params.pitch,
            options,
            &is_plane,
        )?;
    }
    let mut fills = Vec::new();
    if !original.zones.zones.is_empty() {
        run.phase(Phase::Zones);
        let board = with_routes(original, &tracks, &vias);
        fills = zones::fill_zones(&board, original).map_err(RouteError::Geometry)?;
    }
    let failed = jobs
        .iter()
        .zip(&failed)
        .filter_map(|(j, f)| f.map(|f| (j.ctx.net, f)))
        .collect();
    let routed = Routed {
        tracks,
        vias,
        fills,
        unrouted,
        stats,
        stopped: None,
        open: Vec::new(),
        track_kinds,
        via_kinds,
    };
    Ok((routed, failed))
}

/// The routes of every net at some point of rip-up, and how many connections they made.
struct Best {
    made: usize,
    copper: Vec<Option<NetCopper>>,
    missing: Vec<Vec<usize>>,
    apart: Vec<usize>,
}

/// How many connections the nets' copper makes: for each net with any, its terminals less
/// the pieces it is in.
fn made_by(jobs: &[Job], copper: &[Option<NetCopper>], apart: &[usize]) -> usize {
    (0..jobs.len())
        .filter(|&j| copper[j].is_some())
        .map(|j| jobs[j].ctx.terminals - 1 - apart[j])
        .sum()
}

/// A job's span as a seed orders it: between three quarters and five quarters of itself, by
/// a factor drawn from the seed and the net's number.
fn seeded_span(seed: u64, job: &Job) -> i128 {
    // SplitMix64's finalizer, on the seed and the net.
    let mut z = seed ^ u64::from(job.ctx.net.0).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    i128::from(job.span) * i128::from(768 + z % 513)
}

/// How far from a pad the place for a via that [`keep_exits`] keeps for it may be.
const EXIT_REACH: i64 = 3_000_000;

/// Keeps a way out free of pairs routed first for the pads of nets with zones beside the
/// pairs' pads, which a via takes to their planes, such as a ground pin beside a pair's pins:
/// the nearest place outside the pad where a via of its net fits, and a track to there, as
/// keepouts in `world`. Returns them, to take out again.
fn keep_exits(design: &Design, world: &mut World<'_>, pitch: i64, jobs: &[Job]) -> Vec<Placed> {
    let rules = &design.rules;
    // Closer to a pad of a pair than the pair's width and gap, as the pins of a connector
    // between the pair's, but not the balls of a BGA around them.
    let near: Vec<Rect> = design
        .pairs
        .iter()
        .flat_map(|p| [(p.positive, p.width + p.gap), (p.negative, p.width + p.gap)])
        .filter_map(|(net, reach)| Some((jobs.iter().find(|j| j.ctx.net == net)?, reach)))
        .flat_map(|(j, reach)| j.ctx.pads.iter().map(move |p| p.bbox.expand(reach)))
        .collect();
    let zoned = |net: NetId| design.zones.zones.iter().any(|z| z.net == Some(net));
    let n = EXIT_REACH / pitch.max(1);
    let mut offsets: Vec<(i64, i64, i64)> = Vec::new();
    for i in -n..=n {
        for j in -n..=n {
            if i * i + j * j <= n * n {
                offsets.push((i * i + j * j, i, j));
            }
        }
    }
    offsets.sort_unstable();
    let mut out = Vec::new();
    for pad in &design.board.pads {
        let Some(net) = pad.net.filter(|&n| zoned(n)) else {
            continue;
        };
        if !near.iter().any(|r| r.intersects(&pad.shape.bbox())) {
            continue;
        }
        let Some(spec) = design.nets.get(net.index()).filter(|s| s.route) else {
            continue;
        };
        let Some(via) = spec.via else { continue };
        let (Some(center), Some(layer)) = (
            pad_center(&pad.shape),
            LayerSet(pad.layers.0 & rules.layers().0).iter().next(),
        ) else {
            continue;
        };
        let width = spec.track_width.max(rules.track_width(Some(net), layer));
        let c = rules.clearance(Some(net), layer);
        let inside = |q: Point| Shape::circle(q, 2).is_ok_and(|d| pad.shape.touches(&d));
        let site = offsets.iter().find_map(|&(_, i, j)| {
            let q = Point::new(center.x + i * pitch, center.y + j * pitch);
            let fits = !inside(q)
                && world.via(net, q, via.diameter, via.drill, None) == Verdict::Free
                && Shape::track(center, q, width)
                    .is_ok_and(|t| world.track(net, layer, &t, None) == Verdict::Free);
            fits.then_some(q)
        });
        let Some(q) = site else { continue };
        if let Ok(disk) = Shape::circle(q, via.diameter + 2 * c) {
            out.push(world.add_keepout(rules.layers(), disk));
        }
        if let Ok(neck) = Shape::track(center, q, width + 2 * c) {
            out.push(world.add_keepout(LayerSet::EMPTY.with(layer), neck));
        }
    }
    out
}

/// Pairs routed first: the jobs of their nets and what they were routed with, and by pair
/// in the design's order, its route.
#[derive(Default)]
struct PairsFirst {
    jobs: Vec<Job>,
    params: Option<Params>,
    routes: Vec<Option<pairs::PairRoute>>,
}

impl PairsFirst {
    /// Routes a pair side by side into `world`, if it can.
    fn route(
        &self,
        design: &Design,
        world: &mut World<'_>,
        pair: &DiffPair,
    ) -> Option<pairs::PairRoute> {
        let job = |net: NetId| self.jobs.iter().find(|j| j.ctx.net == net);
        let (p, n) = (job(pair.positive)?, job(pair.negative)?);
        let params = self.params?;
        (0..pairs::END_CHOICES)
            .find_map(|choice| pairs::route_pair(design, world, pair, p, n, params, choice, None))
    }

    /// The nets of the pairs routed.
    fn nets(&self, design: &Design) -> Vec<NetId> {
        let mut out: Vec<NetId> = design
            .pairs
            .iter()
            .zip(&self.routes)
            .filter(|(_, r)| r.is_some())
            .flat_map(|(p, _)| [p.positive, p.negative])
            .collect();
        out.sort_unstable();
        out
    }

    /// How many terminals a net of a pair had to connect.
    fn terminals(&self, net: NetId) -> Option<usize> {
        self.jobs
            .iter()
            .find(|j| j.ctx.net == net)
            .map(|j| j.ctx.terminals)
    }
}

/// A net routed in part as a net to complete: each of the pieces its copper leaves it in
/// is a terminal, the first terminal's the first, with the pads of the terminals in it and
/// the ends of its tracks and its vias as the places where routes may join it.
fn completion(design: &Design, ctx: &NetCtx, copper: &NetCopper, pitch: i64) -> NetCtx {
    let pieces = copper.pieces(ctx);
    // The pieces with a terminal are numbered first, and are the terminals to join.
    let terminals = pieces.terminals.iter().copied().max().map_or(1, |m| m + 1);
    let mut pads: Vec<TermPad> = ctx
        .pads
        .iter()
        .map(|p| TermPad {
            terminal: pieces.terminals[p.terminal],
            ..p.clone()
        })
        .collect();
    let point = |at: Point, layers: LayerSet, terminal: usize| TermPad {
        center: at,
        layers,
        bbox: Rect::new(at, at),
        terminal,
    };
    for (s, &piece) in copper.segs.iter().zip(&pieces.segs) {
        if piece < terminals {
            let on = LayerSet::EMPTY.with(LayerId(s.l));
            pads.push(point(s.a, on, piece));
            pads.push(point(s.b, on, piece));
        }
    }
    for (&v, &piece) in copper.vias.iter().zip(&pieces.vias) {
        if piece < terminals {
            pads.push(point(v, design.rules.layers(), piece));
        }
    }
    NetCtx::new(
        ctx.net,
        ctx.widths.clone(),
        ctx.via,
        pads,
        terminals,
        LATTICE_REACH * pitch,
    )
}

/// Where planes are: by layer and tile of `grid`, the net whose planned fill on an inner
/// layer holds the tile's centre.
fn plane_map(design: &Design, fills: &[Fill], grid: Corridor, cost: f64) -> Planes {
    let layers = design.rules.layer_count();
    let cells = grid.cols * grid.rows;
    let mut nets = vec![0u32; layers * cells];
    for f in fills.iter().filter(|f| planes::inner(f.layer, layers)) {
        let Some(net) = design.zones.zones.get(f.zone).and_then(|z| z.net) else {
            continue;
        };
        let Ok(area) = crate::geom::Area::of_shape(&f.shape, 0, 1, crate::geom::ArcSide::Inside)
        else {
            continue;
        };
        let b = f.shape.bbox();
        for row in 0..grid.rows {
            for col in 0..grid.cols {
                let c = Point::new(
                    grid.origin.x + col as i64 * grid.tile + grid.tile / 2,
                    grid.origin.y + row as i64 * grid.tile + grid.tile / 2,
                );
                if b.intersects(&Rect::new(c, c)) && area.contains_point(c) {
                    nets[usize::from(f.layer.0) * cells + row * grid.cols + col] = net.0 + 1;
                }
            }
        }
    }
    Planes { grid, nets, cost }
}
