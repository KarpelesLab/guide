//! The whole run: nets from the design, escapes, global routing, detailed routing with
//! rip-up, cleanup, pairs side by side, and meanders.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::cleanup;
use super::control::{Phase, Run};
use super::cross;
use super::detail::{
    self, Corridor, Failure, History, NetCtx, NetRun, Params, RipUp, StepCache, TermPad,
};
use super::escape;
use super::gates::Gates;
use super::geometry::{NetCopper, Seg};
use super::global::{self, GlobalSpace, Probe, TerminalPads};
use super::neck;
use super::pairs;
use super::planes;
use super::pours::{self, Pours};
use super::report::{self, Known};
use super::rows;
use super::settle::{Routes, Settle};
use super::shut;
use super::spokes;
use super::tune;
use super::uncut;
use super::whole;
use super::world::{Placed, Planes, Verdict, World};
use super::zones::{self, Fill};
use super::{
    CopperKind, Design, DiffPair, Earlier, RouteError, RouteOptions, Routed, RunControl, Stats,
};
use crate::geom::drc::{Board, Track, Via};
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape, SpatialIndex};
use crate::id::NetId;

/// A net to route, and where it may go.
pub(crate) struct Job {
    pub(crate) ctx: NetCtx,
    pub(crate) corridor: Option<Corridor>,
    /// Half the perimeter of its pads' box, which orders the nets.
    span: i64,
    /// Whether its first terminal is a plane of its, which its routes are stubs to.
    pub(crate) plane: bool,
    /// The pieces of fill of that plane, by position among the net's.
    fills: Vec<usize>,
}

/// What a penalty for crossing another net's route costs, in millimetres, and how much a
/// conflict adds to its tile's history.
const RIP_PENALTY: f64 = 10.0;

const HISTORY_STEP: f64 = 0.5;

/// The share of what cleanup has left that routing each net again among the others may
/// have.
const AGAIN_PART: f64 = 0.5;

/// The least time worth routing each net again in: the grid is asked of the board first,
/// which on a board of six layers takes more than this (`glasgow-revD1`), and with less
/// the joining of what the routes cut off their fills has it all.
const AGAIN_LEAST: std::time::Duration = std::time::Duration::from_millis(1000);

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
pub(super) const BRANCH_TERMINALS: usize = 32;

/// How many times in a pass a net may lose branches only, before it is taken out whole.
const BRANCH_CUTS: u32 = 8;

/// A net with a plane may lose stubs only, before it is taken out whole, once in a pass for
/// each this many of its terminals, if that is more than [`BRANCH_CUTS`]: a ground of
/// hundreds of pads taken out whole for one stub in the way is hundreds of connections
/// to make again, and unmade until its turn comes.
const PLANE_CUTS: u32 = 4;

/// How many pads a net has at most, and how many pitches of the lattice a zone of its is
/// wide at most, by the shorter side of its outline's box, for the zone not to be a plane,
/// unless it [says](super::CopperZone::plane) that it is: a pour between two or three pads
/// is their connection, drawn as copper where a track would not do, and not a plane to cut
/// through. Routes of other nets through it would leave its pads apart, and shut in. A
/// wider zone is in the way of too much to be gone around, whatever its net: a ground of
/// four pads under a whole board, or a supply's area under a part.
const POUR_PADS: usize = 4;
const POUR_WIDTH: i64 = 32;

/// How much finer the lattice of the second pass is.
const FINE: i64 = 2;

/// How many passes on the finer lattice there are at most, each for the nets the one
/// before left, with every net free to be ripped up again.
const FINE_PASSES: usize = 2;

/// How many nodes a pass of a run that [settles](super::settle) goes on for at least with
/// no more connections made than it had at its best.
const STALL_NODES: u64 = 2_000_000;

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
pub(crate) fn jobs(
    design: &Design,
    world: &World<'_>,
    pitch: i64,
    planes: &[(NetId, Vec<planes::Piece>)],
    origins: &[Option<Point>],
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
        let mut plane_fills: Vec<usize> = Vec::new();
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
                plane_fills.clone_from(&pieces.fills[m]);
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
                // A net of hundreds of pads is not looked at to the end by a run that has
                // to stop.
                if world.run().is_some_and(Run::poll) {
                    break;
                }
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
        let stubs = span.is_some();
        let span = span.unwrap_or_else(|| {
            let bbox = pads
                .iter()
                .map(|p: &TermPad| Rect::new(p.center, p.center))
                .reduce(|a, b| a.union(&b))
                .unwrap_or_default();
            (bbox.max().x - bbox.min().x) + (bbox.max().y - bbox.min().y)
        });
        let ctx = NetCtx::new(net, widths, via, pads, t, LATTICE_REACH * pitch);
        out.push(Job {
            ctx: match origins.get(i).copied().flatten() {
                Some(origin) => ctx.on_lattice(origin),
                None => ctx,
            },
            corridor: None,
            span,
            plane: stubs,
            fills: if stubs { plane_fills } else { Vec::new() },
        });
    }
    out
}

/// A net's copper as the board's tracks.
pub(crate) fn copper_tracks<'c>(
    ctx: &'c NetCtx,
    c: &'c NetCopper,
) -> impl Iterator<Item = Track> + 'c {
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
pub(crate) fn copper_vias<'c>(
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
    run_from(design, None, options, control)
}

/// How long past a deadline the nets round a pad shut in are laid another way for it.
const SHUT_OVERTIME: std::time::Duration = std::time::Duration::from_millis(1500);

/// As [`run`], going on from the routes of an earlier run if there are any.
pub(crate) fn run_from(
    design: &Design,
    earlier: Option<&Earlier>,
    options: &RouteOptions,
    control: &RunControl<'_>,
) -> Result<Routed, RouteError> {
    report::validate(design)?;
    let mut run = Run::new(control, options.work_budget);
    run.phase(Phase::Prepare);
    run.poll();
    // The run with the rounds of cadlab's own router, if it is asked for and they take the
    // board; or the run with this crate's own, its necks, its escapes and its negotiation,
    // from its start: on the design without the zones that only the others plan on
    // ([`Zones::unfilled_from`](super::Zones)), so that a board given up is routed as one
    // that the others were never asked for.
    let first = if options.rounds == super::Rounds::Native {
        match route_all(design, earlier, options, &run) {
            Ok(outcome) => Some(outcome),
            Err(Halt::Error(e)) => return Err(e),
            Err(Halt::Again) => None,
        }
    } else {
        None
    };
    // A board given up has what a run without those rounds was to have of the time.
    if first.is_none() && options.rounds == super::Rounds::Native && options.given_up_share < 1.0 {
        run.shorten(options.given_up_share);
    }
    let own_options = RouteOptions {
        rounds: super::Rounds::Own,
        ..*options
    };
    let own_design;
    let (design, options, outcome) = match first {
        Some(outcome) => (design, options, outcome),
        None => {
            let design = if design.zones.filled() < design.zones.zones.len() {
                let mut d = design.clone();
                let kept = d.zones.filled();
                d.zones.zones.truncate(kept);
                d.zones.fills.retain(|f| f.zone < kept);
                d.zones.unfilled_from = None;
                own_design = d;
                &own_design
            } else {
                design
            };
            match route_all(design, earlier, &own_options, &run) {
                Ok(mut outcome) => {
                    outcome.0.stats.rounds_given_up = options.rounds == super::Rounds::Native;
                    (design, &own_options, outcome)
                }
                Err(Halt::Error(e)) => return Err(e),
                Err(Halt::Again) => unreachable!("only the rounds asked for give a board up"),
            }
        }
    };
    let (mut routed, mut failed, mut counted, narrow, apart) = outcome;
    if options.tune_lengths && !design.lengths.is_empty() && !run.poll() {
        run.phase(Phase::Lengths);
        tune::run(design, &mut routed, options).map_err(RouteError::Geometry)?;
        // Meanders change what zones fill around, and may cut a plane in pieces, which are
        // joined up again as after routing.
        if routed.stats.meanders > 0.0 && !design.zones.zones.is_empty() {
            run.phase(Phase::Zones);
            // The fill from before the meanders is of a board that is no more.
            counted = None;
            if options.planes && options.join && design.zones.zones.iter().any(|z| z.net.is_some())
            {
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
                let mut unjoined = Vec::new();
                let mut joined = Joined {
                    tracks: &mut routed.tracks,
                    vias: &mut routed.vias,
                    track_kinds: &mut routed.track_kinds,
                    via_kinds: &mut routed.via_kinds,
                    stats: &mut routed.stats,
                    unrouted: &mut routed.unrouted,
                    unjoined: &mut unjoined,
                };
                counted = join_planes(
                    design,
                    &mut world,
                    &mut joined,
                    pitch(design, options),
                    options,
                    &is_plane,
                )?;
                for net in unjoined {
                    failed.entry(net).or_insert(Failure::NoPath);
                }
            }
            if options.fills {
                routed.fills =
                    zones::fill_zones(&with_routes(design, &routed.tracks, &routed.vias), design)
                        .map_err(RouteError::Geometry)?;
            }
        }
    }
    // What the result leaves apart, counted on the board as it is now: with its fills, or
    // if it has none, with those that the joining of zones' pieces ended on, or failing
    // that, those that routing planned on.
    run.phase(Phase::Report);
    let mut board = routed.board(design);
    if routed.fills.is_empty() {
        let fills = counted.iter().flatten();
        board.zones.extend(fills.map(|f| f.to_zone(design)));
    }
    let survey = report::survey(design, &board);
    let stopped = run.stopped().is_some();
    if stopped || (options.planes && !design.zones.zones.is_empty()) {
        let s = &mut routed.stats;
        (s.nets, s.routed, s.connections, s.connected) =
            (survey.nets, survey.done, survey.needed, survey.made);
    }
    if stopped || (!options.join && options.planes && !design.zones.zones.is_empty()) {
        // A run that was stopped may not have got to every net, or to joining the pieces
        // of those with zones, and one that was not to join them has not: what is
        // unrouted is what the board leaves in pieces.
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
            widths: &narrow,
            apart: &apart,
        };
        routed.open = report::explain(design, &world, survey.open, &known);
    }
    routed.failed = failed
        .keys()
        .copied()
        .filter(|n| routed.unrouted.binary_search(n).is_ok())
        .collect();
    routed.apart = apart
        .keys()
        .copied()
        .filter(|n| routed.unrouted.binary_search(n).is_ok())
        .collect();
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

/// The lattice's pitch given that of the nets' own widths and that of the nets that a row
/// of pads made narrower ([`neck::plan`]): the tighter of the two, unless the options set
/// a pitch or the run has a budget of nodes.
fn lattice_pitch(pitch: i64, row_pitch: Option<i64>, options: &RouteOptions) -> i64 {
    match (options.pitch, row_pitch, options.work_budget) {
        (None, Some(row), None) => pitch.min(row),
        _ => pitch,
    }
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
    /// The nets whose pieces no stub and no track joined.
    unjoined: &'a mut Vec<NetId>,
}

/// Fills the zones around everything routed, and joins up the pieces of their nets that
/// the fills leave apart, a few times at most; what can't be joined is unrouted. Islands
/// are kept until the end, as a plane that nothing reaches yet is still one to reach, and
/// they join nothing. Returns the fill it ended on, around all that it added, if it was not
/// stopped in the middle of one.
fn join_planes(
    design: &Design,
    world: &mut World<'_>,
    out: &mut Joined<'_>,
    pitch: i64,
    options: &RouteOptions,
    is_plane: &[bool],
) -> Result<Option<Vec<Fill>>, RouteError> {
    let keeping = keep_islands(design);
    let plane_net = |net: NetId| {
        let mut zones = design.zones.zones.iter().zip(is_plane);
        zones.any(|(z, &plane)| plane && z.net == Some(net))
    };
    // How long the last fill took: a run with a deadline starts none that it would not
    // see the end of.
    // The first is taken to need three times what the fill before routing did, around
    // the copper there was then.
    let mut took: Option<std::time::Duration> =
        world.run().and_then(|r| r.fill_took(None)).map(|t| 3 * t);
    for round in 0..=REPAIRS {
        // A run that has to stop leaves the pieces as they are.
        let mut stopped = world.run().is_some_and(Run::poll);
        if let (Some(run), Some(took)) = (world.run(), took)
            && run.short_of(took)
        {
            run.out_of_time();
            break;
        }
        let board = with_routes(design, out.tracks, out.vias);
        let no_fill = || world.run().is_some_and(Run::no_fill);
        let began = std::time::Instant::now();
        // A fill given up half way joins nothing: the pieces are as they were.
        let Some(all) =
            zones::fill_zones_until(&board, &keeping, &no_fill).map_err(RouteError::Geometry)?
        else {
            break;
        };
        took = Some(began.elapsed());
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
            stopped = stopped || world.run().is_some_and(Run::poll);
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
            // The searches that would have joined them found no way, if they were made.
            if !stopped {
                out.unjoined.extend(left.iter().copied());
            }
            out.unrouted.extend(left);
            out.unrouted.sort_unstable();
            out.unrouted.dedup();
            return Ok(Some(all));
        }
    }
    Ok(None)
}

/// The fills of a net that its stubs and grouping take as connected before routing: its
/// fills on inner layers, its planes, or if it has none, or with `outer`
/// ([`RouteOptions::outer_pours`]), all of them.
fn planned_fills(design: &Design, fills: &[Fill], is_plane: &[bool], outer: bool) -> Vec<Fill> {
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
                && (outer
                    || planes::inner(f.layer, layers)
                    || !fills
                        .iter()
                        .any(|g| net_of(g) == net && planes::inner(g.layer, layers)))
        })
        .cloned()
        .collect()
}

/// Of a host's fills, those that count for the rounds of cadlab's own router: the fills
/// that nets are planned on ([`planned_fills`]), and those of the zones that are no planes.
fn counted_fills(design: &Design, fills: Vec<Fill>, is_plane: &[bool], outer: bool) -> Vec<Fill> {
    let mut out = planned_fills(design, &fills, is_plane, outer);
    out.extend(
        fills
            .into_iter()
            .filter(|f| !is_plane.get(f.zone).copied().unwrap_or(true)),
    );
    out
}

/// The fills that count for the rounds of cadlab's own router as this crate has them
/// ([`Rounds::Native`](super::Rounds)): [`counted_fills`], less, on a board of more than
/// two layers and unless `outer`, the fills on its outer layers that a net with no fill on
/// an inner one is planned on. A net is planned on such a pour by its pads being joined
/// before anything is routed, and the routes of the other nets, which are on that layer,
/// then cut it between them: the pads end joined on the board of the rounds and apart on
/// the board filled again, shut in by the routes that were laid as if they had no need of
/// a way out (`tinytapeout-demo`: the supply's pads on a package of 0.4 mm pitch, joined
/// by a pour under it, four to seven connections open in each order).
fn rounds_fills(design: &Design, fills: Vec<Fill>, is_plane: &[bool], outer: bool) -> Vec<Fill> {
    let layers = design.rules.layer_count();
    let mut out = counted_fills(design, fills, is_plane, outer);
    if layers > 2 && !outer {
        out.retain(|f| {
            planes::inner(f.layer, layers) || !is_plane.get(f.zone).copied().unwrap_or(true)
        });
    }
    out
}

/// By zone, whether its net plans on it as a plane: as the zone [says](super::CopperZone::plane),
/// or if it is no pour, of a net of [`POUR_PADS`] pads or fewer and [`POUR_WIDTH`] pitches
/// wide or less, and its net's fills cover enough of the box of the net's pads,
/// [`RouteOptions::plane_coverage`], on the layer where they cover the most. With no
/// coverage set, nothing is filled to find out.
pub(crate) fn plane_zones(
    design: &Design,
    options: &RouteOptions,
) -> Result<Vec<bool>, RouteError> {
    Ok(plane_zones_until(design, options, &|| false)?.0)
}

/// As [`plane_zones`], with the fill it made to find out, if it made one: the zones
/// filled around the board's copper, islands kept. A fill that `stop` gives up says
/// nothing, and the zones are then planes as with no coverage set.
fn plane_zones_until(
    design: &Design,
    options: &RouteOptions,
    stop: &dyn Fn() -> bool,
) -> Result<(Vec<bool>, Option<Vec<Fill>>), RouteError> {
    let zones = &design.zones.zones;
    // A narrow zone of a net of few pads is a pour, not a plane.
    let mut pads = vec![0usize; design.nets.len()];
    for p in &design.board.pads {
        if let Some(slot) = p.net.and_then(|n| pads.get_mut(n.index())) {
            *slot += 1;
        }
    }
    let narrow = POUR_WIDTH * pitch(design, options);
    let pour = |z: &super::CopperZone| {
        let few = z.net.and_then(|n| pads.get(n.index()));
        let (xs, ys) = (z.outline.iter().map(|p| p.x), z.outline.iter().map(|p| p.y));
        let width = xs.clone().max().zip(xs.min()).map(|(hi, lo)| hi - lo);
        let height = ys.clone().max().zip(ys.min()).map(|(hi, lo)| hi - lo);
        few.is_some_and(|&n| n <= POUR_PADS)
            && width.zip(height).is_some_and(|(w, h)| w.min(h) <= narrow)
    };
    let auto = options.plane_coverage > 0.0;
    let plain = || zones.iter().map(|z| z.plane.unwrap_or(!pour(z))).collect();
    if !auto {
        return Ok((plain(), None));
    }
    let Some(fills) = zones::fill_zones_until(&design.board, &keep_islands(design), stop)
        .map_err(RouteError::Geometry)?
    else {
        return Ok((plain(), None));
    };
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
    let planes = zones
        .iter()
        .map(|z| {
            z.plane
                .unwrap_or_else(|| !pour(z) && z.net.is_none_or(covers))
        })
        .collect();
    Ok((planes, Some(fills)))
}

/// The fills of the zones that are not planes, as the board has them before routing, as
/// fixed copper of their nets: what routes of other nets keep clear of, and what joins the
/// pads it touches.
fn fixed_fills(
    design: &Design,
    is_plane: &[bool],
    stop: &dyn Fn() -> bool,
) -> Result<Vec<crate::geom::drc::Zone>, RouteError> {
    let fills = zones::fill_zones_until(&design.board, design, stop)
        .map_err(RouteError::Geometry)?
        .unwrap_or_default();
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

/// How many times over the planned fills are looked at once routing is over, in a run
/// with a budget, for what the routes cut off them and the stubs of the time before left
/// so.
const POUR_ROUNDS: usize = 3;

/// The share of a work budget that routing leaves for those stubs.
const POUR_WORK: f64 = 0.03;

/// How many times its price a via costs such a stub on a board of two layers: there the
/// fill that a pad was cut off is on the pad's own layer, and a track along that layer to
/// copper of its net takes it back as a via to the other side does. At the price of the
/// passes the via won wherever the track was longer than 1 mm (`lumenpnp-ringlight`'s
/// ground: two vias and 2.9 mm of track in four orders of four, where cadlab's router has
/// one via and 8.5 mm; with this, one and 4.4 mm). On more layers a net's plane is inside
/// the board, and a via is the way to it.
const POUR_VIA_TWO: f64 = 8.0;

/// What a negotiation for every net that ended with points still shared is begun again
/// as, in order: the pitch (2, see [`WHOLE_FINER`]) and which other order of the nets.
const WHOLE_ORDERS: [(i64, u64); 3] = [(2, 1), (2, 2), (2, 3)];

/// What a negotiation at the first pitch that ended with points still shared is begun
/// again as, in order: what the pitch is divided by, in halves (2 is the pitch), and
/// which other order of the nets (0 is the one given).
const WHOLE_FINER: [(i64, u64); 9] = [
    (3, 0),
    (4, 0),
    (2, 1),
    (3, 1),
    (4, 1),
    (6, 0),
    (2, 2),
    (3, 2),
    (4, 2),
];
/// And how many points at most may still be shared for the finer grids, or one for each
/// net if that is more: a board with more has another trouble than its grid.
const FINER_SHARED: usize = 100;

/// How many points a negotiation for every net may end with still shared and not be begun
/// again on another grid or in another order: one point is one wire to take out, which the
/// passes lay again, and the tries after such a negotiation found none that ended with
/// nothing shared where they were made (`nrfmicro`, two orders of eight: nine tries more in
/// 7 to 8 s of a run of 22 s, and the board that the run without them ends with, to the via
/// and the tenth of a millimetre; `docs/benchmarks.md`).
const WHOLE_FEW: usize = 1;

/// Whether a negotiation for every net that ended on its own (`ended`) with `shared` points
/// still shared is begun again ([`WHOLE_ORDERS`], [`WHOLE_FINER`]): not one that its rounds
/// ended at their most (`capped`), nor one with no more shared than [`WHOLE_FEW`].
fn tried_again(ended: bool, shared: usize, capped: bool) -> bool {
    ended && !capped && shared > WHOLE_FEW
}

/// What [`route_all`] returns: the result, how the last search of each net that failed
/// ended, for the nets left open, the fill that the joining of zones' pieces ended on,
/// if it was made, by net the width it was routed at where that is narrower than its
/// own, and by net how many of its connections the board with no route on it has no way
/// for.
/// Why [`route_all`] has no outcome: an error, or the rounds of cadlab's own router gave the
/// board up after their first round ([`Rounds::Native`](super::Rounds)), and the run is to
/// begin again with this crate's own.
enum Halt {
    Error(RouteError),
    Again,
}

impl From<RouteError> for Halt {
    fn from(e: RouteError) -> Halt {
        Halt::Error(e)
    }
}

type Outcome = (
    Routed,
    BTreeMap<NetId, Failure>,
    Option<Vec<Fill>>,
    Vec<Option<i64>>,
    BTreeMap<NetId, usize>,
);

/// By net, the tracks and vias of an earlier run that are taken back as its route, and the
/// tracks and vias that stay as copper of the board.
type TakenBack = (
    BTreeMap<NetId, (Vec<Seg>, Vec<Point>)>,
    (Vec<Track>, Vec<Via>),
);

/// The routes of an earlier run, by net: those that this run can have as routes of its
/// own, and the others. A net's are taken back if the net is routed and has pads to join,
/// its tracks are straight and as wide as the net is routed on their layers, and its vias
/// are the net's, through every layer.
fn taken_back(design: &Design, earlier: &Earlier) -> TakenBack {
    let rules = &design.rules;
    let layers = rules.layer_count();
    let bottom = LayerId(layers.saturating_sub(1) as u8);
    // By net, whether it is routed and has pads to join.
    let mut routable: BTreeMap<NetId, bool> = BTreeMap::new();
    for n in earlier
        .tracks
        .iter()
        .map(|t| t.net)
        .chain(earlier.vias.iter().map(|v| v.net))
        .flatten()
    {
        routable.entry(n).or_insert_with(|| {
            design.nets.get(n.index()).is_some_and(|s| s.route) && terminals(design, n).len() >= 2
        });
    }
    let spec = |net: Option<NetId>| {
        net.filter(|n| routable.get(n).copied().unwrap_or(false))
            .and_then(|n| design.nets.get(n.index()).map(|s| (n, s)))
    };
    let track_fits = |t: &Track| {
        spec(t.net).is_some_and(|(n, s)| {
            t.mid.is_none()
                && usize::from(t.layer.0) < layers
                && rules.layers().contains(t.layer)
                && t.width == s.track_width.max(rules.track_width(Some(n), t.layer))
        })
    };
    let via_fits = |v: &Via| {
        spec(v.net).is_some_and(|(_, s)| {
            layers > 1
                && v.from == LayerId(0)
                && v.to == bottom
                && s.via
                    .is_some_and(|size| size.diameter == v.diameter && size.drill == v.drill)
        })
    };
    // A net with one piece that does not fit keeps all of it where it is.
    let mut fixed: BTreeSet<Option<NetId>> = BTreeSet::new();
    fixed.extend(
        earlier
            .tracks
            .iter()
            .filter(|t| !track_fits(t))
            .map(|t| t.net),
    );
    fixed.extend(earlier.vias.iter().filter(|v| !via_fits(v)).map(|v| v.net));
    let mut routes: BTreeMap<NetId, (Vec<Seg>, Vec<Point>)> = BTreeMap::new();
    let mut kept: (Vec<Track>, Vec<Via>) = Default::default();
    for t in &earlier.tracks {
        match t.net.filter(|_| !fixed.contains(&t.net)) {
            Some(n) => routes.entry(n).or_default().0.push(Seg {
                a: t.start,
                b: t.end,
                l: t.layer.0,
            }),
            None => kept.0.push(t.clone()),
        }
    }
    for v in &earlier.vias {
        match v.net.filter(|_| !fixed.contains(&v.net)) {
            Some(n) => {
                let vias = &mut routes.entry(n).or_default().1;
                if !vias.contains(&v.at) {
                    vias.push(v.at);
                }
            }
            None => kept.1.push(v.clone()),
        }
    }
    (routes, kept)
}

/// The routes of a net that a run was given, as tracks and vias of its width and its via.
fn route_copper(
    design: &Design,
    net: NetId,
    segs: &[Seg],
    vias: &[Point],
) -> (Vec<Track>, Vec<Via>) {
    let rules = &design.rules;
    let spec = &design.nets[net.index()];
    let tracks: Vec<Track> = segs
        .iter()
        .map(|s| Track {
            start: s.a,
            end: s.b,
            mid: None,
            width: spec
                .track_width
                .max(rules.track_width(Some(net), LayerId(s.l))),
            layer: LayerId(s.l),
            net: Some(net),
        })
        .collect();
    let bottom = LayerId((rules.layer_count() - 1) as u8);
    let vias: Vec<Via> = spec
        .via
        .into_iter()
        .flat_map(|size| {
            vias.iter().map(move |&at| Via {
                at,
                drill: size.drill,
                diameter: size.diameter,
                from: LayerId(0),
                to: bottom,
                net: Some(net),
            })
        })
        .collect();
    (tracks, vias)
}

/// Routes everything but meanders.
fn route_all<'a>(
    design: &'a Design,
    earlier: Option<&Earlier>,
    options: &RouteOptions,
    run: &'a Run<'a>,
) -> Result<Outcome, Halt> {
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
    // Nets too wide to leave their pads are routed narrower, from here on.
    let (narrow, row_pitch) = if options.neck_down && !run.poll() {
        if options.rounds == super::Rounds::Native {
            // By the rule of the rounds that route them, in every run of a board that has
            // them in its first: the routes of one run are laid again in the next.
            let narrow = super::rounds::necks(design);
            let row = narrow
                .iter()
                .enumerate()
                .filter_map(|(i, w)| {
                    let clearance = rules.clearance(Some(NetId(i as u32)), LayerId(0));
                    w.map(|w| (w + clearance) / 2)
                })
                .min();
            (narrow, row)
        } else {
            let world = world_of(design)?;
            // Beside their neighbours too in a run with a budget.
            neck::plan(design, &world, run.bounded())
        }
    } else {
        (Vec::new(), None)
    };
    // The lattice's pitch is that of the tracks as they are routed, where the rows of a
    // package made its nets narrower: at the pitch of the wider tracks, two of the
    // narrower ones side by side are two steps apart with room to spare, which a board
    // that needed them narrower has not got (`tinytapeout-demo`, 53 nets at 0.185 mm:
    // 296 vias and 5140 mm over four orders on the lattice of 0.2 mm, 268.5 and 4961 mm
    // on that of 0.1925 mm; `docs/benchmarks.md`).
    // Not in a run with a budget of nodes, where a finer lattice is more nodes for the
    // same way: `corne-cherry`, 0.195 mm for 0.22 mm, had 110 connections open over three
    // orders at 40 million nodes for 86.
    let pitch = lattice_pitch(pitch, row_pitch, options);
    let necked_nets = narrow.iter().flatten().count();
    let with_necks;
    let design = if necked_nets == 0 {
        design
    } else {
        let mut d = design.clone();
        for (spec, width) in d.nets.iter_mut().zip(&narrow) {
            if let Some(w) = *width {
                spec.track_width = w;
            }
        }
        with_necks = d;
        &with_necks
    };
    // Zones that are not planes are copper as they are filled before anything is routed.
    let planes_on = options.planes && design.zones.zones.iter().any(|z| z.net.is_some());
    // A run that has to stop gives up the fill it is in: nothing is planned on it.
    let stop = || run.poll();
    let (is_plane, mut filled) = if planes_on {
        plane_zones_until(design, options, &stop)?
    } else {
        (vec![true; design.zones.zones.len()], None)
    };
    // The fill made to tell the planes is the one that nets are planned on, for as long
    // as nothing is added to the board.
    let unchanged = design;
    // The routes of an earlier run are routes of their nets from the start, which this run
    // may move, but those that can't be: they are copper of the board, as the host's own,
    // and the fill made to tell the planes is not of that board.
    let (mut taken_back, kept_fixed) =
        earlier.map_or_else(Default::default, |e| taken_back(design, e));
    let with_earlier;
    let design = if kept_fixed.0.is_empty() && kept_fixed.1.is_empty() {
        design
    } else {
        let mut d = design.clone();
        // They were on the host's board when it made its fills, as the rest was.
        if let Some(clear) = d.zones.fills_clear_of.as_mut()
            && clear[0] == d.board.tracks.len()
            && clear[1] == d.board.vias.len()
        {
            clear[0] += kept_fixed.0.len();
            clear[1] += kept_fixed.1.len();
        }
        d.board.tracks.extend(kept_fixed.0);
        d.board.vias.extend(kept_fixed.1);
        with_earlier = d;
        &with_earlier
    };
    let with_fixed;
    let design = if is_plane.iter().all(|&p| p) {
        design
    } else {
        let mut d = design.clone();
        let fixed = fixed_fills(design, &is_plane, &stop)?;
        // The host's fills are clear of each other, and what is cut of one is less of it.
        if let Some(clear) = d.zones.fills_clear_of.as_mut()
            && clear[2] == d.board.zones.len()
        {
            clear[2] += fixed.len();
        }
        d.board.zones.extend(fixed);
        with_fixed = d;
        &with_fixed
    };
    // Escapes from fine-pitch footprints, which become fixed copper of their nets.
    let mut crossings = 0;
    let escapes = {
        // In a run that moves no route ([`RouteOptions::rip_ups`] at none), planned on
        // the board with the routes that the run was given where they are: a pad that its
        // net's route leaves is out already, and no escape goes where another net's route
        // is. Planned without them, each pad has the escape again that a first run
        // planned for it, and where that run gave the place to another net since, the
        // other net's route does not fit beside the escape and is cut there as the run
        // begins. A run that moves routes then routes that net again, round the escape,
        // which is how a ball deep in an array that a first run left shut in comes out
        // on a full board; a run that does not leaves it open (`cynthion`: a supply's
        // via between four balls, where the ball beside it has its dog-bone planned, and
        // the supply's two pads open after a run that only looks for shorter ways, in
        // every run of it: `docs/benchmarks.md`).
        let with_routes;
        let design = if taken_back.is_empty() || options.rip_ups > 0 {
            design
        } else {
            let mut d = design.clone();
            for (&net, (segs, vias)) in &taken_back {
                let (tracks, vias) = route_copper(design, net, segs, vias);
                d.board.tracks.extend(tracks);
                d.board.vias.extend(vias);
            }
            with_routes = d;
            &with_routes
        };
        let mut world = world_of(design)?;
        // The rounds of cadlab's own router plan their own ways out, on their own grid.
        let own_fanout = options.whole
            && options.rounds == super::Rounds::Native
            && options.rounds_fanout
            && run.bounded()
            && taken_back.is_empty();
        // In a run with a budget, for the balls deep in an area array too.
        let mut escapes: Vec<escape::Escape> = if own_fanout {
            // The ways out are planned on every fill that a net of this crate's own rounds
            // is planned on: a pad in a pour of its net on its own layer has no way out
            // of its own planned for it (with the fills as the rounds count them,
            // `tinytapeout-demo` ends with 306 vias for 293).
            let fills = counted_fills(
                design,
                design.zones.fills.clone(),
                &is_plane,
                options.outer_pours,
            );
            let counted = |fills: Vec<Fill>| fills;
            let pours = super::rounds::Pours {
                fills: &fills,
                own_zones: original.board.zones.len(),
                is_plane: &is_plane,
                counted: &counted,
                spoke: &|_, _| (0, 0.0),
            };
            super::rounds::fanout(design, &pours)
                .into_iter()
                .map(|f| escape::Escape {
                    net: f.net,
                    tracks: f.tracks,
                    via: f.via,
                })
                .collect()
        } else {
            escape::plan(design, &mut world, pitch, options.fanout_via, run.bounded())
        };
        // And in a run with a budget, the crossings of the rows of fine pitch, around the
        // escapes: fixed copper of their nets as those are, as a route that rip-up may
        // take out is gone before the nets beside it have settled.
        if run.bounded() && options.crossings && !own_fanout && !run.poll() {
            let jobs = jobs(design, &world, pitch, &[], &[]);
            let bottom = LayerId((rules.layer_count() - 1) as u8);
            for s in cross::plan(design, &mut world, &jobs) {
                let ctx = &jobs[s.job].ctx;
                let net = Some(ctx.net);
                escapes.push(escape::Escape {
                    net: ctx.net,
                    tracks: vec![Track {
                        start: s.seg.a,
                        end: s.seg.b,
                        mid: None,
                        width: ctx.widths[usize::from(s.seg.l)],
                        layer: LayerId(s.seg.l),
                        net,
                    }],
                    via: s.via.zip(ctx.via).map(|(at, (diameter, drill))| Via {
                        at,
                        drill,
                        diameter,
                        from: LayerId(0),
                        to: bottom,
                        net,
                    }),
                });
                crossings += usize::from(s.via.is_some());
            }
        }
        escapes
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
    // What the look at the board made that the rounds have again, if they are begun.
    let look_kept: std::cell::Cell<Option<super::rounds::Kept>> = std::cell::Cell::new(None);
    // The rounds of cadlab's own router, where they are asked for, are given up here, before
    // anything of the run is planned for them but their own ways out: on a board that they
    // have no room for in their share, or whose pads their grid does not reach, the run
    // begins again as one of this crate's own, and is that run from then on.
    if options.whole
        && options.rounds == super::Rounds::Native
        && taken_back.is_empty()
        && !run.poll()
    {
        let fills = rounds_fills(
            design,
            design.zones.fills.clone(),
            &is_plane,
            options.outer_pours,
        );
        let counted = |fills: Vec<Fill>| fills;
        let pours = super::rounds::Pours {
            fills: &fills,
            own_zones: original.board.zones.len(),
            is_plane: &is_plane,
            counted: &counted,
            spoke: &|_, _| (0, 0.0),
        };
        let (look, kept) = super::rounds::look_kept(design, &pours).unzip();
        look_kept.set(kept.flatten());
        let share = options.whole_share.clamp(0.0, 1.0);
        let work = run.work_left().map(|w| w as f64 * share);
        let time = run.left().map(|t| t.as_secs_f64() * share);
        if look.is_none_or(|l| l.gives_up(work, time, options.threads)) {
            return Err(Halt::Again);
        }
    }
    // Pairs first, if asked, side by side on the board as it is before anything else is
    // routed, as copper of their nets that the rest completes, and that only the passes on
    // the finer lattice may move.
    let mut pairs_first = PairsFirst::default();
    let with_pairs;
    let before_pairs = design;
    let design = if options.couple_pairs && options.pairs_first && !design.pairs.is_empty() {
        let mut world = world_of(design)?;
        pairs_first.jobs = jobs(design, &world, pitch, &[], &[]);
        pairs_first.params = Some(Params {
            pitch,
            via_cost: options.via_cost,
            layer_bias: options.layer_bias,
            bounds: bounds_of(design),
            max_expansions: options.max_expansions,
            under_pads: false,
            partial: false,
            stubs: false,
            forest: false,
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
    // The whole of that fill, which a result with no fill of its own is counted on.
    let mut first_fills: Vec<Fill> = Vec::new();
    if planes_on {
        let fills = match filled.take().filter(|_| core::ptr::eq(unchanged, design)) {
            Some(fills) => fills,
            None => {
                let began = std::time::Instant::now();
                let fills = zones::fill_zones_until(&design.board, &keep_islands(design), &stop)
                    .map_err(RouteError::Geometry)?
                    .unwrap_or_default();
                run.fill_took(Some(began.elapsed()));
                fills
            }
        };
        planned = planned_fills(design, &fills, &is_plane, options.outer_pours);
        first_fills = fills;
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
        forest: false,
    };

    // Pairs routed first are routes of their nets in the world, locked until the passes on
    // the finer lattice, where the nets left may move them, and they are routed again.
    let mut world = world_of(before_pairs)?;
    // The passages only as wide as a track, between the pads and the escapes' vias, for
    // the passes on the finer lattice of a run with a budget: one with none is as it was,
    // where on a board whose pairs are routed first they cost a net its route.
    if run.bounded() {
        world.set_gates(Gates::plan(before_pairs, pitch).finer_than(pitch));
    }
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
    // In a run with a budget, a net with a pad in a row of fine pitch is on that pad's
    // lattice.
    let origins = if run.bounded() {
        row_origins(design)
    } else {
        Vec::new()
    };
    let mut jobs = jobs(terminal_design, &world, pitch, &plane_pieces, &origins);
    match options.seed {
        None => jobs.sort_by_key(|j| (j.span, j.ctx.net)),
        Some(seed) => jobs.sort_by_key(|j| (seeded_span(seed, j), j.ctx.net)),
    }
    run.totals(jobs.len(), jobs.iter().map(|j| j.ctx.terminals - 1).sum());
    let mut stats = Stats {
        nets: jobs.len(),
        connections: jobs.iter().map(|j| j.ctx.terminals - 1).sum(),
        necked: necked_nets,
        crossings,
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

    // Where the spokes of pads in zones of their nets will be, which other nets' tracks pay
    // to be in.
    if options.spoke_cost > 0.0
        && !(design.zones.zones.is_empty() && design.zones.spokes.is_empty())
        && !run.poll()
    {
        world.set_spokes(spokes::Spokes::plan(
            design,
            bounds,
            pitch,
            common_width / 2 + rules.spec().clearance,
            options.spoke_cost,
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
    // By job, the net that cut it last, if it is to be mended.
    let mut cut_by: Vec<Option<NetId>> = vec![None; jobs.len()];
    // By job, whether it has waited for the others in the first pass, after a search in its
    // corridor that ended so.
    let mut waited = vec![Wait::No; jobs.len()];
    // By job, whether its copper is what it reached with nothing moved, in the turn after
    // it waited, and its next turn is the first in which it may cross other nets' routes:
    // if that finds no way, it keeps what it has.
    let mut held = vec![false; jobs.len()];
    // Whether the run has a budget or a deadline, and so routes for the most connections
    // soonest.
    let hurried = run.bounded();
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
    // And how its last search that failed ended, whatever came after.
    let mut last_failure: Vec<Option<Failure>> = vec![None; jobs.len()];
    // By job, once a search for it has failed in a run with a budget, how many of its
    // connections have no way on the board with no route on it: what fixed copper, keepouts
    // and the edge keep apart on every layer stays apart whatever is moved, and a net that
    // has all the rest has no more turns, in the passes or in the settling.
    let mut floor: Vec<Option<usize>> = vec![None; jobs.len()];
    let mut bare: Option<World<'_>> = None;
    // The finest lattice that the passes search on.
    let finest = if options.fine_pass {
        Params {
            pitch: (pitch / FINE).max(1000),
            under_pads: options.vias_in_pads,
            ..params
        }
    } else {
        params
    };
    // What the progress hears of: nets with a route, connections made, nets waiting.
    let tell = |jobs: &[Job],
                copper: &[Option<NetCopper>],
                missing: &[Vec<usize>],
                apart: &[usize],
                queued: usize| {
        let (mut nets, mut made) = (0, 0);
        for (j, _) in copper.iter().enumerate().filter(|(_, c)| c.is_some()) {
            nets += usize::from(missing[j].is_empty());
            made += jobs[j].ctx.terminals - 1 - apart[j];
        }
        run.counts(nets, made, queued);
    };
    // A second pass, on a finer lattice, for the nets the first leaves unrouted, or routed in
    // part, with every net that can be ripped up again.
    // By job, whether it has had a turn.
    let mut tried = vec![false; jobs.len()];
    let mut best: Option<Best> = None;
    // In a run with a budget, the ways out of the rows of fine pitch, planned a row at a
    // time, are copper of their nets from the start, which each net's first search goes on
    // from, as it does when it completes a net routed in part. By job, whether it has
    // such stubs and has not had that search.
    let mut fresh = vec![false; jobs.len()];
    // In a run with a budget, the fills that nets are planned on are kept as cells, to
    // tell what the routes cut off them once routing is over.
    // What an earlier run routed is where each net was left: a net that it joins whole has
    // no turn until another moves it, and one in pieces is completed from them. What of it
    // is not legal on the board as it is now, beside the nets taken back before it, goes.
    let mut given: Vec<NetId> = Vec::new();
    // By job, the routes it began with, in order, to tell at the end if they were moved.
    let mut as_given: Vec<Option<(Vec<Seg>, Vec<Point>)>> = vec![None; jobs.len()];
    for j in 0..jobs.len() {
        let net = jobs[j].ctx.net;
        let Some((segs, vias)) = taken_back.remove(&net) else {
            continue;
        };
        given.push(net);
        // The stubs of a net with a plane end where its fill was: a place that a piece of
        // the fill still holds is one of the terminal that the piece is of, the plane's
        // or a pad's that the piece reaches, as the places looked for beside the pads are.
        if jobs[j].plane
            && let Some((_, fills)) = plane_pieces.iter().find(|(n, _)| *n == net)
        {
            let pieces = planes::pieces(&terminal_design.board, terminal_design, net, fills);
            let ctx = &jobs[j].ctx;
            let mut of_fill: Vec<Option<usize>> = vec![None; fills.len()];
            for &f in &jobs[j].fills {
                of_fill[f] = Some(0);
            }
            for (pads, theirs) in pieces.pads.iter().zip(&pieces.fills) {
                let terminal = pads.iter().find_map(|&p| {
                    let bbox = terminal_design.board.pads[p].shape.bbox();
                    ctx.pads.iter().find(|t| t.bbox == bbox).map(|t| t.terminal)
                });
                for &f in theirs {
                    of_fill[f] = of_fill[f].or(terminal);
                }
            }
            let held = |q: Point, on: Option<u8>| {
                fills.iter().zip(&of_fill).find_map(|(f, &terminal)| {
                    let terminal = terminal?;
                    (on.is_none_or(|l| f.layer == LayerId(l))
                        && f.bbox.intersects(&Rect::new(q, q))
                        && f.holds(q))
                    .then(|| TermPad {
                        center: q,
                        layers: LayerSet::EMPTY.with(f.layer),
                        bbox: Rect::new(q, q),
                        terminal,
                    })
                })
            };
            let same = |p: &TermPad, s: &TermPad| {
                p.terminal == s.terminal && p.center == s.center && p.layers == s.layers
            };
            let mut sites: Vec<TermPad> = Vec::new();
            let ends = segs
                .iter()
                .flat_map(|s| [(s.a, Some(s.l)), (s.b, Some(s.l))]);
            for (q, on) in vias.iter().map(|&v| (v, None)).chain(ends) {
                if let Some(site) = held(q, on)
                    && !sites.iter().any(|p| same(p, &site))
                    && !ctx.pads.iter().any(|p| same(p, &site))
                {
                    sites.push(site);
                }
            }
            if !sites.is_empty() {
                jobs[j].ctx = pours::with_sites(&jobs[j].ctx, &sites, pitch);
            }
        }
        let ctx = &jobs[j].ctx;
        let mut theirs = NetCopper::default();
        theirs.segs = segs;
        theirs.vias = vias;
        theirs.merge(ctx);
        let mut c = theirs.free_in(&world, ctx, pitch);
        if ctx.terminals < BRANCH_TERMINALS {
            c.drop_floating(ctx);
        } else {
            c.prune(ctx);
        }
        if c.is_empty() {
            continue;
        }
        c.place(&mut world, ctx);
        let pieces = c.pieces(ctx);
        missing[j] = pieces.missing();
        apart[j] = pieces.apart();
        as_given[j] = Some({
            let (mut segs, mut vias) = (c.segs.clone(), c.vias.clone());
            segs.sort_unstable();
            vias.sort_unstable();
            (segs, vias)
        });
        if apart[j] > 0 {
            completing[j] = Some(completion(design, ctx, &c, pitch));
            // It has its turns as a net routed for the first time has them: with nothing
            // moved until every net has had one, and across other nets' routes after.
            fresh[j] = true;
        }
        copper[j] = Some(c);
    }
    // What the run has added to the design as copper of its own until here, which a host
    // that fills its zones around the routes is given with them: the routes of an earlier
    // run that stay where they are, the escapes, and the pairs routed first.
    let mut own_fixed: (Vec<Track>, Vec<Via>) = (
        design.board.tracks[original.board.tracks.len()..].to_vec(),
        design.board.vias[original.board.vias.len()..].to_vec(),
    );
    // A net with nothing to route has no route to take back: what was given for it stays
    // where it is, in the way of the others as copper of the board is.
    let mut back_copper: Vec<(Vec<Track>, Vec<Via>)> = Vec::new();
    for (net, (segs, vias)) in core::mem::take(&mut taken_back) {
        let (tracks, vias) = route_copper(design, net, &segs, &vias);
        let _ = pairs::Undo::place(&mut world, &tracks, &vias);
        own_fixed.0.extend(tracks.iter().cloned());
        own_fixed.1.extend(vias.iter().cloned());
        back_copper.push((tracks, vias));
    }
    let own_fixed = own_fixed;
    // A world for each thread of its own: the board as this one has it, the pairs
    // routed first locked in it, and no run.
    let make_with = |pairs_first: &PairsFirst| {
        let mut w = World::new(
            &before_pairs.board,
            rules,
            &before_pairs.outline,
            before_pairs.edge_clearance,
            &before_pairs.obstacles,
        )
        .ok()?;
        w.configure(before_pairs, options);
        for (i, r) in pairs_first.routes.iter().enumerate() {
            if let Some(r) = r {
                let _ = pairs::Undo::place(&mut w, &r.tracks, &r.vias);
                for net in [design.pairs[i].positive, design.pairs[i].negative] {
                    w.set_locked(net, true);
                }
            }
        }
        Some(w)
    };
    let pours = (hurried && options.pour_cuts && !planned.is_empty() && !run.poll())
        .then(|| Pours::plan(design, &world, &planned, &jobs, bounds, pitch));
    // Every net against every other at once, on one grid: what that ends with is each
    // net's copper as the passes begin, less what the world does not allow of it.
    let mut negotiated_whole = false;
    // By job, whether its copper began as what a negotiation that ended left it with.
    let mut from_whole = vec![false; jobs.len()];
    if options.whole && given.is_empty() && !run.poll() {
        run.phase(Phase::Whole);
        let was = run.cut();
        let share = options.whole_share.clamp(0.0, 1.0);
        run.part(share, share);
        let make = || make_with(&pairs_first);
        let native = options.rounds == super::Rounds::Native;
        let counted = |fills: Vec<Fill>| -> Vec<Fill> {
            if native {
                rounds_fills(design, fills, &is_plane, options.outer_pours)
            } else {
                counted_fills(design, fills, &is_plane, options.outer_pours)
            }
        };
        let attempt = |options: &RouteOptions, coarse: bool, finer: i64| {
            if native {
                let fills = counted(first_fills.clone());
                let pours = super::rounds::Pours {
                    fills: &fills,
                    own_zones: original.board.zones.len(),
                    is_plane: &is_plane,
                    counted: &counted,
                    spoke: &|l, at| world.spoke_toll(l, at),
                };
                return super::rounds::route(
                    design,
                    &jobs,
                    &pours,
                    (&own_fixed.0, &own_fixed.1),
                    (share, was, look_kept.take()),
                    options,
                    run,
                );
            }
            whole::route(
                design,
                &world,
                &make,
                &jobs,
                pours.as_ref(),
                (&own_fixed.0, &own_fixed.1),
                options,
                run,
                (share, was),
                (pitch, coarse, finer),
                bounds,
            )
        };
        let began = (std::time::Instant::now(), run.work());
        // On a grid of twice the pitch on a board of more than two layers, and on one of
        // two where the first round says that the share has no room for it at the first:
        // with two layers a channel has to hold every track that fits it, and with more
        // the rounds that the coarser grid has the time for count for more.
        let mut coarse = rules.layer_count() > 2 && !native;
        // The rounds of cadlab's own router, where they are asked for and the look at the
        // board before its escapes did not give them up.
        let first = attempt(options, coarse, 2);
        let mut whole = match first {
            Err(whole::NoWhole::NoRoom) if !coarse && !run.poll() => {
                coarse = true;
                run.part(share, share);
                attempt(options, true, 2).ok()
            }
            other => other.ok(),
        };
        // A negotiation that ended on its own with points still shared, in a small part
        // of its share, is begun again, and the one that ends with the fewest shared is
        // kept: its rounds stop where the prices have every net fixed where it is.
        //
        // At twice the pitch, with the nets moved in their order another way: another
        // order has other nets there.
        //
        // At the first pitch, where few enough are shared for the board to be nearly
        // there, on finer grids as well, each in the order given and then in others. The
        // pitch, half a track and its clearance, holds the ways that lie on its lines;
        // between parts placed for routing by hand the passages are often narrower than a
        // step and off those lines, and the rounds then go on for ever with the same few
        // nets in each other's way (`nrfmicro`: 45 to 100 points shared from the tenth
        // round on at 0.2 mm, in every order). Which grid ends with nothing shared is
        // not to be told beforehand, so there are several, as cadlab's router has it for
        // its whole run, and for the same board. A finer grid has as many times the cells
        // as the square of what it is finer by, and is begun only with room for that.
        let first = (began.0.elapsed(), run.work() - began.1);
        let finer_on = !coarse
            && whole
                .as_ref()
                .is_some_and(|w| w.shared <= FINER_SHARED.max(jobs.len()));
        let tries: &[(i64, u64)] = if finer_on {
            &WHOLE_FINER
        } else {
            &WHOLE_ORDERS
        };
        let (mut longest, mut most) = first;
        for (again, &(finer, order)) in (1u32..).zip(tries) {
            if native
                || whole
                    .as_ref()
                    .is_none_or(|w| !tried_again(w.ended, w.shared, w.capped))
            {
                break;
            }
            // Each has cost what the ones before did, at most, for there to be room; where
            // some are on finer grids, what the costliest of them did, and a finer grid
            // its cells more than the first.
            let (took, worked) = if finer_on {
                let cells = (finer * finer) as f64 / 4.0;
                (
                    longest.max(first.0.mul_f64(cells)),
                    most.max((first.1 as f64 * cells) as u64),
                )
            } else {
                (
                    began.0.elapsed() / again,
                    (run.work() - began.1) / u64::from(again),
                )
            };
            if run.poll() {
                break;
            }
            // With twice that in what is left; on the finer grids, where one that ends
            // costs less than those that did not, with as much, and one that has no room
            // leaves the next its turn.
            let times = if finer_on { 1 } else { 2 };
            if run.part_left().is_some_and(|left| left < took * times)
                || run.limits(u64::MAX).max_expansions < worked.saturating_mul(u64::from(times))
            {
                if finer_on {
                    continue;
                }
                break;
            }
            let other = RouteOptions {
                seed: if order == 0 {
                    options.seed
                } else {
                    Some(
                        options
                            .seed
                            .unwrap_or(0)
                            .wrapping_add(order.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
                    )
                },
                ..*options
            };
            let at = (std::time::Instant::now(), run.work());
            let next = attempt(&other, coarse, finer);
            longest = longest.max(at.0.elapsed());
            most = most.max(run.work() - at.1);
            if let Ok(next) = next
                && whole
                    .as_ref()
                    .is_none_or(|w| (next.shared, next.taken_out) < (w.shared, w.taken_out))
            {
                whole = Some(next);
            }
        }
        run.resume(was);
        if let Some(w) = whole {
            negotiated_whole = true;
            stats.whole = (w.rounds, w.shared, w.taken_out, 0);
            stats.whole_made.1 = w.vias_saved;
            stats.pour_cuts += w.pour_cuts;
            stats.refills += w.refills;
            stats.pours_back += w.pours_back;
            stats.whole_finer = w.finer;
            for (k, sites) in &w.sites {
                jobs[*k].ctx = pours::with_sites(&jobs[*k].ctx, sites, pitch);
            }
            // The rounds of cadlab's own router count no spoke's room: the last spoke of a pad
            // is kept from here on, and the net whose route of those rounds stands in the
            // room of it begins the passes without that route.
            if native {
                world.arm_spokes();
            }
            for (j, tree) in w.trees.iter().enumerate() {
                let Some(tree) = tree else { continue };
                let ctx = &jobs[j].ctx;
                let mut c = NetCopper::from_tree(ctx, tree).free_in(&world, ctx, w.pitch);
                if ctx.terminals < BRANCH_TERMINALS {
                    c.drop_floating(ctx);
                } else {
                    c.prune(ctx);
                }
                if c.is_empty() {
                    continue;
                }
                c.place(&mut world, ctx);
                let pieces = c.pieces(ctx);
                missing[j] = pieces.missing();
                apart[j] = pieces.apart();
                stats.whole.3 += 1;
                stats.whole_made.0 += ctx.terminals - 1 - apart[j];
                if apart[j] > 0 {
                    completing[j] = Some(completion(design, ctx, &c, pitch));
                    fresh[j] = true;
                }
                from_whole[j] = w.ended;
                tried[j] = true;
                copper[j] = Some(c);
            }
        }
    }
    if hurried && options.row_fanout && !negotiated_whole && !run.poll() {
        let mut stubs: Vec<Option<NetCopper>> = vec![None; jobs.len()];
        for s in rows::plan(design, &mut world, pitch, &jobs, options.via_cost) {
            let c = stubs[s.job].get_or_insert_with(NetCopper::default);
            c.segs.push(s.seg);
            c.vias.extend(s.via);
            // A stub's end is kept for as long as its net has it, whatever else of the
            // net is cut away.
            c.keep.push(s.seg.b);
            stats.row_stubs += 1;
        }
        for (j, c) in stubs.into_iter().enumerate() {
            let Some(mut c) = c else { continue };
            // A net that has its routes of an earlier run goes on from those.
            if copper[j].is_some() {
                continue;
            }
            let ctx = &jobs[j].ctx;
            c.place(&mut world, ctx);
            let pieces = c.pieces(ctx);
            missing[j] = pieces.missing();
            apart[j] = pieces.apart();
            completing[j] = Some(completion(design, ctx, &c, pitch));
            copper[j] = Some(c);
            fresh[j] = true;
        }
    }
    // A run with a deadline or a budget keeps a share of it for what comes after routing.
    let shares = Shares::of(options, planes_on);
    // The work kept for the joining of zones' pieces, of the budget as it is now.
    let join_work = run
        .work_left()
        .map_or(0, |left| (left as f64 * shares.join_work) as u64);
    let pour_work = if pours.is_some() {
        1.0 - POUR_WORK
    } else {
        1.0
    };
    run.part(shares.routing.0, shares.routing.1 * pour_work);
    // In a run with a budget, a pass that has stopped gaining ends, and what the passes
    // leave open is settled in the time and the work that this leaves of routing's.
    let settles = hurried && options.rip_ups > 0 && options.settle;
    // By job, whether a negotiation for it was given up, and is not tried again.
    let mut spent = vec![0u8; jobs.len()];
    // Whether the pass before, on the finer lattice, took no net out and moved none aside.
    let mut idle_pass = false;
    // From here on a pad's last spoke is kept as the routes move ([`World::keep_spokes`]):
    // what a negotiation ended with and what an earlier run left are in the world by now.
    world.arm_spokes();
    'passes: for pass in 0..=FINE_PASSES {
        if run.poll() {
            break;
        }
        // Another pass on the finer lattice has every net free to be taken out again: after
        // one that took none out, it has the same to say ([`said_all`]). `sweep-v2.2`'s
        // three connections that no router makes had a pass of 13 s and one of 6 s for
        // them, and have the first.
        if idle_pass {
            break;
        }
        let pass_began = (stats.rip_ups, stats.shoved);
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
        // The most connections that the routes made in this pass, and the nodes that the
        // run had expanded when the pass began and when they first made as many.
        let pass_from = stats.expanded;
        let mut peak = (made_by(&jobs, &copper, &apart), pass_from);
        // And how many nets have had a turn since.
        let mut turns = 0usize;
        // How many nets have had their turn when each has had one: all of them in the
        // first pass, and in those on the finer lattice the few that it is for.
        let all_turns = if pass == 0 { jobs.len() } else { queue.len() };
        while let Some(j) = queue.pop_front() {
            tell(&jobs, &copper, &missing, &apart, queue.len() + 1);
            if run.poll() {
                break 'passes;
            }
            // A pass of a run that settles what is left ends once it has gone on for as
            // long again as it took to make the most connections it has made: rip-up
            // that moves nets for each other from then on makes few more, and the
            // settling has the time.
            if settles {
                let made = made_by(&jobs, &copper, &apart);
                if made > peak.0 {
                    peak = (made, stats.expanded);
                    turns = 0;
                }
                turns += 1;
                // The first pass makes most of the connections, and is given as long
                // again as that took; the passes on the finer lattice are for the few left.
                let patience = if pass == 0 {
                    STALL_NODES.max(peak.1 - pass_from)
                } else {
                    STALL_NODES
                };
                if turns > all_turns && stats.expanded - peak.1 > patience {
                    queue.clear();
                    break;
                }
            }
            if stuck(
                floor[j],
                jobs[j].ctx.terminals,
                copper[j].is_some(),
                apart[j],
            ) {
                continue;
            }
            // A net that was cut more than once since its last turn waits more than once,
            // and is whole again after the first.
            if copper[j].is_some() && completing[j].is_none() {
                continue;
            }
            let job = &jobs[j];
            tried[j] = true;
            let was_held = core::mem::take(&mut held[j]);
            // A net routed in part is completed from its copper, if it still has it.
            let mut rest = completing[j].take().filter(|_| copper[j].is_some());
            // A net with nothing but its stubs is routed as any net is for the first time.
            let first = fresh[j] && rest.is_some();
            let ctx = rest.as_ref().unwrap_or(&job.ctx);
            // A net of many pads is routed a terminal at a time, as stubs to its plane if it
            // has one.
            let plane = plane_net(job.ctx.net);
            let many = job.ctx.terminals > detail::BUDGET_TERMINALS;
            let params = Params {
                stubs: many,
                forest: hurried,
                ..params
            };
            // A net that is mended does not cross the net that cut it: the two would take
            // each other's place in turn. It goes around, or is taken out whole.
            let cutter = cut_by[j].take().filter(|_| rest.is_some());
            let mending = hurried && rest.is_some() && !first;
            let cutter_was = cutter.map(|n| world.locked(n));
            if let Some(n) = cutter {
                world.set_locked(n, true);
            }
            let world_ref = &world;
            // The world doesn't change until the net is placed.
            let steps = StepCache::default();
            // One tree through every stage: what the corridor connects is kept by the
            // search over the board, and what that connects by the one that may cross
            // other nets' routes, which then crosses them only for what is left.
            let mut grow = NetRun::new(world_ref, ctx, &steps, params);
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
            // In the first pass of a run with a budget, a net that finds no way in its
            // corridor waits for every net to have had its turn before it is searched for
            // over the board, and takes others out: those searches cost many times the
            // first, and a run that is stopped has then routed what there was room for,
            // rather than moved a few nets for each other while most waited. A net that
            // keeps what it reaches is not made to wait: what it reached would be searched
            // for again. A run with no budget keeps the nets in their order, shortest
            // first, which on a crowded board ends with fewer vias, if later.
            // When it comes back it is searched for over the board with nothing moved, and
            // if that finds no way it waits once more before it crosses other nets'
            // routes: a search that finds none with nothing moved ends soon where the net
            // is shut in, and one that crosses routes costs up to its limit of nodes, so
            // that a hundred nets waited behind a few such searches with nothing, on a
            // board of six layers. A net of more than two terminals that keeps what it
            // reaches has what that search reached as its copper, and goes on from it.
            let may_wait = hurried
                && pass == 0
                && options.rip_ups > 0
                && !plane
                && !many
                && (rest.is_none() || first);
            let back = may_wait && matches!(waited[j], Wait::NoPath | Wait::OutOfBudget);
            // Whether this turn is the one in which it may cross them.
            let cross = may_wait && waited[j] == Wait::Cross;
            // Whether what it reaches this turn, with nothing moved, is kept for it to go
            // on from.
            let mut reached = false;
            if cross {
                out_of_budget = true;
                waited[j] = Wait::Done;
                why = Some(Failure::NoPath);
            } else if back {
                out_of_budget = waited[j] == Wait::OutOfBudget;
                why = Some(if out_of_budget {
                    Failure::Budget
                } else {
                    Failure::NoPath
                });
                if !out_of_budget {
                    match grow.stage(params, None, Some(quiet)) {
                        Ok(()) => {
                            outside[j] = job.corridor.is_some();
                            done = true;
                        }
                        Err(f) => why = Some(f),
                    }
                }
                if !done && options.keep_partial && ctx.terminals > 2 {
                    let part = Params {
                        partial: true,
                        stubs: true,
                        // What the net is left with here waits, as copper, for its
                        // turn to cross: pads joined among themselves that the first
                        // reaches none of are then in the way of the nets still to come,
                        // on a board with room for them (tinytapeout-demo, 91 open over
                        // six orders for 59).
                        forest: false,
                        ..params
                    };
                    if grow.stage(part, None, Some(quiet)).is_ok()
                        && grow.apart() + 1 < ctx.terminals
                    {
                        done = true;
                        reached = true;
                    }
                }
                if !done && !queue.is_empty() {
                    waited[j] = Wait::Cross;
                    stats.expanded += grow.finish().expanded;
                    completing[j] = rest.take();
                    queue.push_back(j);
                    continue;
                }
                waited[j] = Wait::Done;
                out_of_budget = true;
            } else if mending {
                // A net that is mended is looked for a way around what cut it, near where
                // it was: its corridor may have none, and the board over would be searched
                // whole before the routes in its way are crossed.
                let near = Params {
                    max_expansions: MEND_NODES,
                    ..params
                };
                match grow.stage(near, None, Some(quiet)) {
                    Ok(()) => done = true,
                    Err(f) => {
                        out_of_budget = true;
                        why = Some(f);
                    }
                }
            } else if let Some(corridor) = &job.corridor {
                match grow.stage(params, Some(corridor), Some(quiet)) {
                    Ok(()) => done = true,
                    Err(f) => {
                        out_of_budget = f == Failure::Budget;
                        why = Some(f);
                    }
                }
                if !done && may_wait && waited[j] == Wait::No && !queue.is_empty() {
                    waited[j] = if out_of_budget {
                        Wait::OutOfBudget
                    } else {
                        Wait::NoPath
                    };
                    stats.expanded += grow.finish().expanded;
                    completing[j] = rest.take();
                    queue.push_back(j);
                    continue;
                }
            }
            fresh[j] = false;
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
            // Whether a search that may cross the routes of every net, none of them locked,
            // found no way for the net whole, on the finest lattice of the run: the board
            // has none, wherever the others go.
            let all_free =
                rest.is_none() && params.pitch == finest.pitch && !world_ref.any_locked();
            let mut no_way = false;
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
                    Err(f) => {
                        why = Some(f);
                        no_way |= shut_in(all_free, partial.partial, ctx.terminals, f);
                    }
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
            if let (Some(n), Some(was)) = (cutter, cutter_was) {
                world.set_locked(n, was);
            }
            // A net that could not be mended is taken out whole, and routed again from
            // nothing when its turn comes: its pieces join nothing, and are in the way.
            // But one that a negotiation for every net ended with in pieces: those join
            // its pads, among nets that were all routed round them, and routed again from
            // nothing it has less room than it had (`sweep-v2.2`, 13 open over three
            // orders for 9). One that its share cut short is as any other.
            if tree.is_none()
                && rest.is_some()
                && !was_held
                && !from_whole[j]
                && job.ctx.terminals < BRANCH_TERMINALS
                && !plane
                && mends(job.ctx.terminals, plane, hurried, 0)
                && !run.ending()
                && let Some(mut c) = copper[j].take()
            {
                c.unplace(&mut world);
                missing[j].clear();
                apart[j] = 0;
                ripped[j] += 1;
                queue.push_back(j);
                continue;
            }
            let mut displaced: Vec<usize> = Vec::new();
            // The nets this one's route runs into, with the copper they had.
            let mut cut: Vec<(usize, NetCopper)> = Vec::new();
            if let Some(t) = tree.as_ref().filter(|_| crossed) {
                let (mut victims, at) = detail::victims(&world, ctx, t);
                // In a run with a budget, the nets in the way are first moved aside: each
                // cut where the route crosses it and mended around the route, with nothing
                // else moved. If all of them can be, no connection is lost for the one
                // made, and nothing has to be routed again.
                if hurried && !victims.is_empty() && victims.len() <= SHOVED {
                    let whole = |v: &NetId| {
                        !pair_of.contains_key(v)
                            && by_net.get(v).is_some_and(|&k| {
                                copper[k].is_some()
                                    && apart[k] == 0
                                    && missing[k].is_empty()
                                    && completing[k].is_none()
                            })
                    };
                    if victims.iter().all(whole) {
                        let aside: Vec<usize> = victims.iter().map(|v| by_net[v]).collect();
                        let mut route = NetCopper::from_tree(ctx, t);
                        let mut shove = Shove {
                            design,
                            jobs: &jobs,
                            plane: &plane_net,
                            params,
                            pitch,
                            history: &history,
                            expanded: 0,
                        };
                        let moved =
                            shove.all(&mut world, &mut copper, &aside, &mut route, &job.ctx);
                        stats.expanded += shove.expanded;
                        if moved {
                            stats.shoved += aside.len();
                            victims.clear();
                        }
                    }
                }
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
                        // A net of many pads loses only its branches in the way, a few
                        // times, before it is taken out whole as any other: a branch
                        // makes room for one net, and the whole of it for all those that
                        // wait, which go before it comes back.
                        let terminals = jobs[k].ctx.terminals;
                        let mended = mends(terminals, plane_net(v), hurried, cuts[k]);
                        if mended {
                            // A net to mend goes next: the room it is mended in is beside
                            // the route that cut it, and is not left to others first.
                            queue.push_front(k);
                        } else {
                            queue.push_back(k);
                        }
                        if mended {
                            cut_by[k] = Some(job.ctx.net);
                        }
                        if mended || loses_branches(pass, terminals, plane_net(v), hurried, cuts[k])
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
                _ if run.ending() => None,
                Some(t) if t.missing.is_empty() => None,
                Some(t) if t.out_of_budget => Some(Failure::Budget),
                Some(_) => Some(Failure::NoPath),
                None => why,
            };
            last_failure[j] = failed[j].or(last_failure[j]);
            // The first time a net finds no way, it is looked for one on the board with no
            // route on it. Not a net with a plane: its terminals there are places on a fill
            // that the routes of the others have not cut yet.
            if hurried && no_way && !plane {
                floor[j] = Some(1);
            }
            if hurried && failed[j].is_some() && floor[j].is_none() && !plane {
                if bare.is_none() {
                    let mut world = world_of(before_pairs)?;
                    world.set_gates(Gates::plan(before_pairs, pitch).finer_than(pitch));
                    bare = Some(world);
                }
                if let Some(bare) = &bare {
                    let (apart, expanded) = apart_on(bare, &job.ctx, params, finest);
                    floor[j] = Some(apart);
                    stats.expanded += expanded;
                }
            }
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
                        let mut pieces = c.pieces(&job.ctx);
                        // A net mended whole loses the ends that its cuts left.
                        if pieces.apart() == 0 && job.ctx.terminals < BRANCH_TERMINALS && !plane {
                            // With the stubs it did without.
                            c.keep.clear();
                            c.prune(&job.ctx);
                            pieces = c.pieces(&job.ctx);
                        }
                        missing[j] = pieces.missing();
                        apart[j] = pieces.apart();
                    }
                    (_, old) => {
                        if let Some(mut old) = old {
                            old.unplace(&mut world);
                        }
                        // What the route leaves out may be joined among itself.
                        let pieces = c.pieces(&job.ctx);
                        missing[j] = pieces.missing();
                        apart[j] = pieces.apart();
                    }
                }
                c.place(&mut world, &job.ctx);
                // What a net reached with nothing moved is what it goes on from, when its
                // turn to cross other nets' routes comes.
                if reached {
                    completing[j] = Some(completion(design, &job.ctx, &c, pitch));
                    held[j] = true;
                    queue.push_back(j);
                }
                copper[j] = Some(c);
            }
            // A net in the way loses only what is in the way: the rest of its copper goes
            // back, in the pieces that leaves it in, and it is joined up again from them
            // when its turn comes, instead of being routed again from nothing.
            for (k, old) in cut {
                let ctx = &jobs[k].ctx;
                let mut kept = old.free_in(&world, ctx, params.pitch);
                if ctx.terminals < BRANCH_TERMINALS && !plane_net(ctx.net) {
                    // A net of few pads is mended from its ends at the cut: cut back to
                    // its joints, one of two pads would have nothing left.
                    kept.drop_floating(ctx);
                } else {
                    kept.prune(ctx);
                }
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
        // A pass of a run with a budget that ends with fewer connections than its routes
        // made at their best goes back to those: more work never leaves fewer.
        if hurried
            && !run.ending()
            && best
                .as_ref()
                .is_some_and(|b| b.made > made_by(&jobs, &copper, &apart))
            && let Some(b) = best.take()
        {
            b.restore(&mut world, &jobs, &mut copper, &mut missing, &mut apart);
            for (j, slot) in completing.iter_mut().enumerate() {
                *slot = None;
                // What these routes leave open was looked for since, and its search is
                // what it is left with.
                if copper[j].is_none() || !missing[j].is_empty() {
                    failed[j] = failed[j].or(last_failure[j]);
                }
            }
        }
        idle_pass = said_all(pass, pass_began, (stats.rip_ups, stats.shoved));
        // What the passes leave open is made a connection at a time, each kept only if the
        // board has more for it.
        if settles && !run.ending() && !run.poll() {
            run.phase(Phase::Settle);
            let mut settle = Settle {
                design,
                by_net: &by_net,
                plane: &plane_net,
                params,
                pitch,
                history: &history,
                fixed: &pair_nets,
                pours: pours.as_ref(),
                expanded: 0,
                blocked: None,
                tried: 0,
                kept: 0,
                made: 0,
                chains: pass == FINE_PASSES || !options.fine_pass,
                negotiate: options.negotiate,
                negotiated: (0, 0, 0),
                places: pass == 0 && options.negotiate_places,
                spent: &mut spent,
                skip: (0..jobs.len())
                    .map(|j| {
                        stuck(
                            floor[j],
                            jobs[j].ctx.terminals,
                            copper[j].is_some(),
                            apart[j],
                        )
                    })
                    .collect(),
            };
            let mut routes = Routes {
                copper: &mut copper,
                missing: &mut missing,
                apart: &mut apart,
            };
            settle.all(&mut world, &jobs, &mut routes);
            stats.expanded += settle.expanded;
            stats.settled.0 += settle.tried;
            stats.settled.1 += settle.kept;
            stats.settled.2 += settle.made;
            stats.negotiated.0 += settle.negotiated.0;
            stats.negotiated.1 += settle.negotiated.1;
            stats.negotiated.2 += settle.negotiated.2;
            if settle.kept + settle.negotiated.1 > 0 {
                // The routes kept from before are of a board that is no more, and so is
                // what the nets were to go on from.
                best = None;
                for j in 0..jobs.len() {
                    completing[j] = None;
                    cut_by[j] = None;
                    held[j] = false;
                }
            }
            for j in 0..jobs.len() {
                if copper[j].is_some() && missing[j].is_empty() {
                    failed[j] = None;
                }
            }
        }
    }
    // A run that was stopped while nets were out goes back to the routes that made the
    // most connections, if what it has now makes fewer.
    if run.ending()
        && let Some(b) = best.take()
        && b.made > made_by(&jobs, &copper, &apart)
    {
        b.restore(&mut world, &jobs, &mut copper, &mut missing, &mut apart);
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

    tell(&jobs, &copper, &missing, &apart, 0);
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
    // And the stubs of rows that nothing was routed on from.
    for (job, c) in jobs.iter().zip(copper.iter_mut()) {
        if let Some(c) = c {
            c.unplace(&mut world);
            c.keep.clear();
            c.prune(&job.ctx);
            c.place(&mut world, &job.ctx);
        }
    }
    stats.before_cleanup = measure(&copper);
    // A run that has to stop keeps the routes as they are.
    // Cleanup has the work that routing left, but what was kept for the joining.
    let cleanup_work = match run.work_left() {
        Some(left) if left > 0 => left.saturating_sub(join_work) as f64 / left as f64,
        _ => 1.0,
    };
    run.part(shares.cleanup, cleanup_work);
    // What the routes cut off the planned fills is joined to them again, by stubs that
    // move nothing: now that nothing moves any more, a piece that is cut off stays so.
    // And once more after a cleanup that looked for shorter ways: a run of tracks that
    // goes another way may cut a fill where the old one did not.
    // By job, whether the nets round its pads shut in were tried and none gave way.
    let mut shut_tried = vec![false; jobs.len()];
    for cleaned in [false, true] {
        if cleaned && !(options.cleanup && options.shorten) {
            break;
        }
        // A net that this run has not moved is as the earlier run's cleanup left it.
        let still: Vec<bool> = (0..jobs.len())
            .map(|j| match (&as_given[j], &copper[j]) {
                (Some((segs, vias)), Some(c)) => {
                    let mut now = c.segs.clone();
                    now.sort_unstable();
                    let mut at = c.vias.clone();
                    at.sort_unstable();
                    *segs == now && *vias == at
                }
                _ => false,
            })
            .collect();
        // Each net again where the others are, by the negotiation's search: for fewer
        // vias and shorter ways, before the searches of cleanup, which cost many times
        // as much a net.
        //
        // Before what the routes cut off the fills is joined to them again: a net laid
        // another way cuts them elsewhere, and the stubs are for the routes as they stay.
        // Not with less than [`AGAIN_LEAST`] to do it in, which is the joining's then.
        //
        // In a run with a budget of nodes it comes after that joining, as it did: the
        // joining makes connections where this makes none, and the work that this takes
        // first is work that the joining ends without (`corne-cherry` at 40 million nodes:
        // 48 connections open with this first, 38 with half of the work for it, 36 after).
        let again_first = again_before_joining(run.work_left());
        for step in 0..2 {
            if (step == 0) == again_first {
                if !cleaned
                    && options.cleanup
                    && (options.whole || options.shorten)
                    && hurried
                    && !run.poll()
                    && run
                        .part_left()
                        .is_none_or(|left| left.mul_f64(AGAIN_PART) >= AGAIN_LEAST)
                    && let Some(mut base) = make_with(&pairs_first)
                {
                    run.phase(Phase::Cleanup);
                    for (tracks, vias) in &back_copper {
                        let _ = pairs::Undo::place(&mut base, tracks, vias);
                    }
                    let may: Vec<bool> = jobs
                        .iter()
                        .map(|job| {
                            let net = job.ctx.net;
                            !design
                                .pairs
                                .iter()
                                .any(|p| p.positive == net || p.negative == net)
                                && !design.lengths.iter().any(|l| l.nets.contains(&net))
                        })
                        .collect();
                    let until = run
                        .part_left()
                        .map(|left| std::time::Instant::now() + left.mul_f64(AGAIN_PART));
                    // On the negotiation's grid, twice the pitch on more than two layers, where
                    // the routes are its own; at the pitch in a run for shorter ways, whose routes
                    // are where they stay, and a way that only the finer grid has is one to take.
                    let coarse = rules.layer_count() > 2 && !options.shorten;
                    let again = whole::again(
                        design,
                        &base,
                        &mut world,
                        &jobs,
                        &mut copper,
                        &may,
                        options,
                        run,
                        (pitch, coarse),
                        bounds,
                        until,
                    );
                    stats.again.0 += again.tried;
                    stats.again.1 += again.kept;
                    stats.again.2 += again.vias;
                }
                continue;
            }
            if let Some(pours) = &pours {
                for round in 0..POUR_ROUNDS {
                    // The first round also once the deadline is past: what the routes cut
                    // off the fills stays cut off once nothing moves, and a run that its
                    // deadline ended in the passes, or in the round itself, left those pads
                    // so and counted them joined (`must_rejoin`, `Run::overtime`).
                    let overtime = must_rejoin(round, cleaned) && run.overtime();
                    if !overtime && run.poll() {
                        break;
                    }
                    let mut joined = false;
                    // What a pad shut in is tried for past a deadline, at most.
                    let mut shut_until: Option<std::time::Instant> = None;
                    let mut shut_ins: Vec<(usize, usize, Params)> = Vec::new();
                    // The nets that may be laid another way for one: whole, and of no pair
                    // and no group of lengths.
                    let may_move: Vec<bool> = jobs
                        .iter()
                        .enumerate()
                        .map(|(j, job)| {
                            let net = job.ctx.net;
                            missing[j].is_empty()
                                && !design
                                    .pairs
                                    .iter()
                                    .any(|p| p.positive == net || p.negative == net)
                                && !design.lengths.iter().any(|l| l.nets.contains(&net))
                        })
                        .collect();
                    // By the host's own fill around the routes as they are, if it gives one, on
                    // a board with pours that routes cut.
                    let real = pours
                        .outer(design)
                        .then(|| {
                            pours.refill(
                                run,
                                design,
                                &jobs,
                                &copper,
                                (&own_fixed.0, &own_fixed.1),
                                false,
                            )
                        })
                        .flatten();
                    stats.refills += usize::from(real.is_some());
                    for k in pours.jobs().collect::<Vec<usize>>() {
                        if if overtime {
                            run.overtime_ending()
                        } else {
                            run.poll()
                        } {
                            break;
                        }
                        let Some(c) = copper[k].as_ref() else {
                            continue;
                        };
                        let Some(again) = pours.rejoin(
                            &world,
                            design,
                            &jobs,
                            &copper,
                            k,
                            pitch,
                            real.as_ref(),
                            &[],
                        ) else {
                            continue;
                        };
                        let ctx = completion(design, &again.ctx, c, pitch);
                        jobs[k].ctx = pours::with_sites(&jobs[k].ctx, &again.sites, pitch);
                        stats.pour_cuts += again.cut;
                        let steps = StepCache::default();
                        let stubs = Params {
                            partial: true,
                            stubs: true,
                            forest: true,
                            via_cost: if rules.layer_count() <= 2 {
                                params.via_cost * POUR_VIA_TWO
                            } else {
                                params.via_cost
                            },
                            ..params
                        };
                        let found = detail::route_net(&world, &ctx, &steps, stubs, None, None);
                        // How many pieces the search left cut off: pads shut in.
                        let shut_in = match found {
                            Ok(tree) => {
                                stats.expanded += tree.expanded;
                                let more = NetCopper::from_tree(&ctx, &tree);
                                if !more.is_empty()
                                    && let Some(mut old) = copper[k].take()
                                {
                                    old.unplace(&mut world);
                                    old.segs.extend(more.segs);
                                    for v in more.vias {
                                        if !old.vias.contains(&v) {
                                            old.vias.push(v);
                                        }
                                    }
                                    old.merge(&jobs[k].ctx);
                                    old.place(&mut world, &jobs[k].ctx);
                                    copper[k] = Some(old);
                                    joined = true;
                                }
                                tree.missing.len()
                            }
                            Err((_, expanded)) => {
                                stats.expanded += expanded;
                                ctx.terminals
                            }
                        };
                        if shut_in != 0 && !shut_tried[k] {
                            shut_ins.push((k, shut_in, stubs));
                        }
                    }
                    // The pads that the stubs left shut in, once every net has had its
                    // stubs; in a run with a budget of work, in a round whose stubs joined
                    // nothing, which would be the last: what the tries take is then what no
                    // stub of this round or the next would have had.
                    if run.work_left().is_some() && joined {
                        shut_ins.clear();
                    }
                    for (k, shut_in, stubs) in shut_ins {
                        if overtime && shut_until.is_none() {
                            shut_until = Some(
                                std::time::Instant::now()
                                    + run.left().unwrap_or_default()
                                    + SHUT_OVERTIME,
                            );
                        }
                        let stop = || {
                            shut_until.is_some_and(|t| std::time::Instant::now() >= t)
                                || if overtime {
                                    run.overtime_ending()
                                } else {
                                    run.poll()
                                }
                        };
                        let nets: Vec<&NetCtx> = jobs.iter().map(|j| &j.ctx).collect();
                        let open = Routes {
                            copper: &mut copper,
                            missing: &mut missing,
                            apart: &mut apart,
                        }
                        .open_all(&nets);
                        drop(nets);
                        let mut mender = shut::Mender {
                            settle: Settle {
                                design,
                                by_net: &by_net,
                                plane: &plane_net,
                                params,
                                pitch,
                                history: &history,
                                fixed: &pair_nets,
                                pours: None,
                                expanded: 0,
                                blocked: None,
                                tried: 0,
                                kept: 0,
                                made: 0,
                                chains: true,
                                negotiate: false,
                                negotiated: (0, 0, 0),
                                places: false,
                                spent: &mut spent,
                                skip: Vec::new(),
                            },
                            missing: &mut missing,
                            apart: &mut apart,
                            open,
                        };
                        let opened = shut::open(
                            &mut world,
                            design,
                            &mut jobs,
                            &mut copper,
                            pours,
                            real.as_ref(),
                            (k, shut_in),
                            &may_move,
                            (stubs, params),
                            &mut mender,
                            &stop,
                        );
                        stats.expanded += opened.expanded;
                        joined |= opened.kept;
                        // A net whose tries were all undone has no more of them.
                        shut_tried[k] = opened.tried > 0 && !opened.kept;
                    }
                    run.overtime_over();
                    if !joined {
                        break;
                    }
                }
            }
        }
        if cleaned || !options.cleanup || run.poll() {
            continue;
        }
        run.phase(Phase::Cleanup);
        // The stairs laid straight at its end, if asked for, where nothing is laid after
        // it that a track moved into free room could be in the way of: the stubs that
        // join the pieces of planes, pairs side by side and meanders.
        let straight = options.straighten
            && !(planes_on && options.join)
            && !(options.couple_pairs && !design.pairs.is_empty())
            && !(options.tune_lengths && !design.lengths.is_empty());
        stats.expanded += cleanup::run(
            &mut world,
            design,
            &jobs,
            &mut copper,
            params,
            options,
            (&still, straight),
        );
    }
    // A pour left whole, where the route that cut a pad off it has another way: in a run
    // for shorter ways, whose routes are where they stay.
    if let Some(pours) = &pours
        && options.cleanup
        && options.shorten
        && pours.outer(design)
        && !run.poll()
    {
        run.phase(Phase::Cleanup);
        let may: Vec<bool> = jobs
            .iter()
            .map(|job| {
                let net = job.ctx.net;
                !design
                    .pairs
                    .iter()
                    .any(|p| p.positive == net || p.negative == net)
                    && !design.lengths.iter().any(|l| l.nets.contains(&net))
            })
            .collect();
        let done = uncut::run(
            &mut world,
            design,
            &jobs,
            &mut copper,
            pours,
            params,
            options,
            &may,
        );
        stats.expanded += done.expanded;
        stats.uncut = (done.tried, done.kept, done.vias);
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
    // The stubs that the host's own fill around the routes as they stay makes needless,
    // for a host that gives it: a net with a plane has one for every pad off its plane,
    // whatever pour joins the pad.
    // Whatever stopped the run but its caller: nothing is searched for here, and the host
    // says itself whether it has the time for a fill.
    let mut host_fills: Option<Vec<Fill>> = None;
    let mut spare: (Vec<Track>, Vec<Via>) = Default::default();
    if let Some(pours) = &pours
        && !run.cancelled()
        && let Some(real) = pours.refill(
            run,
            design,
            &jobs,
            &copper,
            (&own_fixed.0, &own_fixed.1),
            true,
        )
    {
        stats.refills += 1;
        for k in pours.jobs().collect::<Vec<usize>>() {
            let Some(c) = copper[k].as_ref() else {
                continue;
            };
            let Some((mut less, gone)) = pours.without_needless(design, &jobs, k, c, &real) else {
                continue;
            };
            let vias = gone.vias.len();
            spare.0.extend(copper_tracks(&jobs[k].ctx, &gone));
            spare.1.extend(copper_vias(design, &jobs[k].ctx, &gone));
            if let Some(mut old) = copper[k].take() {
                old.unplace(&mut world);
            }
            less.place(&mut world, &jobs[k].ctx);
            copper[k] = Some(less);
            stats.stubs_pruned += vias;
        }
        host_fills = Some(real.fills);
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
    let mut unjoined = Vec::new();
    let mut counted = None;
    run.part(1.0, 1.0);
    if planes_on && options.join && !run.poll() {
        run.phase(Phase::Zones);
        let mut joined = Joined {
            tracks: &mut tracks,
            vias: &mut vias,
            track_kinds: &mut track_kinds,
            via_kinds: &mut via_kinds,
            stats: &mut stats,
            unrouted: &mut unrouted,
            unjoined: &mut unjoined,
        };
        counted = join_planes(
            original,
            &mut world,
            &mut joined,
            params.pitch,
            options,
            &is_plane,
        )?;
    }
    // A run that ended its routing or its cleanup early for the rest is one that was
    // stopped.
    run.settle();
    let mut fills = Vec::new();
    if !original.zones.zones.is_empty() && options.fills && !run.no_fill() {
        run.phase(Phase::Zones);
        let board = with_routes(original, &tracks, &vias);
        fills = zones::fill_zones_until(&board, original, &|| run.no_fill())
            .map_err(RouteError::Geometry)?
            .unwrap_or_default();
    }
    let mut failed: BTreeMap<NetId, Failure> = jobs
        .iter()
        .zip(&failed)
        .filter_map(|(j, f)| f.map(|f| (j.ctx.net, f)))
        .collect();
    // A net with zones that nothing joined up again was searched for too, to no end.
    for net in unjoined {
        failed.entry(net).or_insert(Failure::NoPath);
    }
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
        tried: {
            let mut nets: Vec<NetId> = jobs
                .iter()
                .zip(&tried)
                .filter(|(_, t)| **t)
                .map(|(j, _)| j.ctx.net)
                .collect();
            nets.sort_unstable();
            nets
        },
        failed: Vec::new(),
        apart: Vec::new(),
        taken_back: {
            given.sort_unstable();
            given
        },
        spare,
    };
    // Counted on the host's fill around the routes, if it gave one once they were where
    // they stay.
    let counted = counted
        .or(host_fills)
        .or(Some(first_fills).filter(|f| !f.is_empty()));
    let floors: BTreeMap<NetId, usize> = jobs
        .iter()
        .zip(&floor)
        .filter_map(|(j, f)| f.filter(|&f| f > 0).map(|f| (j.ctx.net, f)))
        .collect();
    Ok((routed, failed, counted, narrow, floors))
}

/// Whether a round of joining what the routes cut off the fills to them again is made
/// also in a run that its deadline has ended ([`Run::overtime`]): the first of those before
/// cleanup. What a cleanup that looked for shorter ways cuts off is a run's with the time
/// for it.
fn must_rejoin(round: usize, cleaned: bool) -> bool {
    round == 0 && !cleaned
}

/// How many nodes the search for a net on the board with no route on it may expand.
const APART_NODES: u64 = 4_000_000;

/// How many of a net's connections have no way on `bare`, a world with the board's own
/// copper and no route: the pieces that its terminals are left in there, less one, by a
/// search that keeps what it reaches and joins the rest among itself, on the lattice of
/// `params`. If that leaves any, the finest lattice that the run searches on has the say,
/// where it has one within [`APART_NODES`]: a way that only it has, past a pad that is off
/// the lattice, is near the pad, and found long before the half of a board that a search
/// to the other half goes over. Zero if the first search ran out of nodes, which says
/// nothing. With the nodes expanded.
fn apart_on(bare: &World<'_>, ctx: &NetCtx, params: Params, finest: Params) -> (usize, u64) {
    let look = |params: Params| {
        let p = Params {
            partial: true,
            stubs: ctx.terminals > detail::BUDGET_TERMINALS,
            forest: true,
            max_expansions: APART_NODES,
            ..params
        };
        let steps = StepCache::default();
        // A net of two terminals is joined or it is not: the search for it whole says
        // which, where the one that keeps what it reaches goes on from the other
        // terminal with a share of the nodes, and says nothing if that runs out, with
        // the first shut in after a few hundred.
        if ctx.terminals == 2 {
            let whole = Params {
                partial: false,
                forest: false,
                ..p
            };
            return match detail::route_net(bare, ctx, &steps, whole, None, None) {
                Ok(t) => (Some(0), t.expanded),
                Err((Failure::NoPath, expanded)) => (Some(1), expanded),
                Err((Failure::Budget, expanded)) => (None, expanded),
            };
        }
        match detail::route_net(bare, ctx, &steps, p, None, None) {
            Ok(t) if !t.out_of_budget => (
                Some(NetCopper::from_tree(ctx, &t).pieces(ctx).apart()),
                t.expanded,
            ),
            Ok(t) => (None, t.expanded),
            Err((_, expanded)) => (None, expanded),
        }
    };
    let (apart, expanded) = look(params);
    let apart = apart.unwrap_or(0);
    if apart == 0 || finest.pitch == params.pitch {
        return (apart, expanded);
    }
    let (finer, more) = look(finest);
    // A net of two terminals with no way on the lattice of `params` is apart only if the
    // finest lattice says so too: its one connection is all it has.
    if ctx.terminals == 2 {
        return (finer.unwrap_or(0), expanded + more);
    }
    (finer.map_or(apart, |f| f.min(apart)), expanded + more)
}

/// Whether a pass has said all that the passes after it would: one on the finer lattice,
/// `pass` past the first, that took no net out and moved none aside, by how many were
/// taken out and moved aside in the run when it began and when it ended. The next has
/// every net free to be taken out again and nothing else that this one had not, so where
/// none was taken out, none was kept from it, and what this pass found no way for has
/// none in the next, with whatever this one made in the way as well.
fn said_all(pass: usize, began: (usize, usize), ended: (usize, usize)) -> bool {
    pass > 0 && began == ended
}

/// Whether each net is routed again where the others are before what the routes cut off
/// their fills is joined to them again, by the work that the run's budget leaves, if it
/// has one: with a budget of nodes, after. The joining makes connections, and this makes
/// none; of a time that is running out each has its half, and of nodes this took what
/// the joining then ended without.
fn again_before_joining(work_left: Option<u64>) -> bool {
    work_left.is_none()
}

/// Whether a search that failed as `why` has said that a net has no way on the board,
/// whatever the routes of the others: one for the net whole, of two terminals, on the
/// finest lattice of the run, that might cross the routes of every net, `all_free`, and
/// went everywhere it could. What it could
/// not cross is the board's own, and stays. The passes after it have nothing to look for
/// ([`stuck`]): cynthion's eight nets at its net ties, pads that no track leaves at the
/// board's clearance, had a search of the whole board in each pass on the finer lattice,
/// 9 s each for one of them, and the run no time for what comes after routing.
fn shut_in(all_free: bool, partial: bool, terminals: usize, why: Failure) -> bool {
    all_free && !partial && terminals == 2 && why == Failure::NoPath
}

/// Whether a net has no more to gain: `floor` of its connections have no way on the board
/// with no route on it, and it has all the others, or there are no others.
fn stuck(floor: Option<usize>, terminals: usize, routed: bool, apart: usize) -> bool {
    floor.is_some_and(|f| f > 0 && ((routed && apart <= f) || f + 1 >= terminals))
}

/// Whether a net in the way of another loses only its branches in the way, rather than
/// being taken out whole: a net of [`BRANCH_TERMINALS`] or more, cut `cuts` times in this
/// pass so far. Without a plane, in the passes on the finer lattice, [`BRANCH_CUTS`] times.
/// With one, only in a run with a budget, `hurried`, and then in any pass: it is stubs to
/// its plane, each on its own, and may lose them a time for each [`PLANE_CUTS`] of its
/// terminals, if that is more. A run with no budget takes a net with a plane out whole,
/// which finds its stubs other places altogether, and on a crowded board that made one
/// more connection.
fn loses_branches(pass: usize, terminals: usize, plane: bool, hurried: bool, cuts: u32) -> bool {
    let most = match (plane, hurried) {
        (true, true) => BRANCH_CUTS.max(terminals as u32 / PLANE_CUTS),
        (true, false) => 0,
        (false, _) if pass > 0 => BRANCH_CUTS,
        (false, _) => 0,
    };
    terminals >= BRANCH_TERMINALS && cuts < most
}

/// How many nets at most a route moves aside at once.
const SHOVED: usize = 8;

/// The moving aside of the nets in a route's way.
struct Shove<'a> {
    design: &'a Design,
    jobs: &'a [Job],
    plane: &'a dyn Fn(NetId) -> bool,
    params: Params,
    /// The pitch of the first pass, which the lattices of pads go on by.
    pitch: i64,
    history: &'a History,
    /// The nodes its searches expanded.
    expanded: u64,
}

impl Shove<'_> {
    /// Moves the nets of the jobs `aside`, each routed whole, out of the way of `route`,
    /// a route of the net `of` that is not in the world: with the route in place, each is
    /// cut where the route is in its way and mended around it, crossing nothing. If all
    /// of them are mended, they have their new copper, in the world, and the route is not
    /// in it. If one can't be, everything is as it was.
    fn all(
        &mut self,
        world: &mut World<'_>,
        copper: &mut [Option<NetCopper>],
        aside: &[usize],
        route: &mut NetCopper,
        of: &NetCtx,
    ) -> bool {
        let mut old: Vec<(usize, NetCopper)> = Vec::new();
        for &k in aside {
            if let Some(mut c) = copper[k].take() {
                c.unplace(world);
                old.push((k, c));
            }
        }
        route.place(world, of);
        let mut mended: Vec<(usize, NetCopper)> = Vec::new();
        for (k, c) in &old {
            match self.mend(world, &self.jobs[*k].ctx, c) {
                Some(c) => mended.push((*k, c)),
                None => break,
            }
        }
        route.unplace(world);
        if mended.len() == old.len() {
            for (k, c) in mended {
                copper[k] = Some(c);
            }
            return true;
        }
        for (_, mut c) in mended {
            c.unplace(world);
        }
        for (k, mut c) in old {
            c.place(world, &self.jobs[k].ctx);
            copper[k] = Some(c);
        }
        false
    }

    /// The copper of a net that was routed whole as `old`, which is not in the world,
    /// mended around what is in the world now: what of it is still free there, joined up
    /// again by a search that crosses nothing, of [`MEND_NODES`] nodes at most. It is
    /// in the world if it is returned.
    fn mend(&mut self, world: &mut World<'_>, ctx: &NetCtx, old: &NetCopper) -> Option<NetCopper> {
        let mut kept = old.free_in(world, ctx, self.params.pitch);
        if ctx.terminals < BRANCH_TERMINALS && !(self.plane)(ctx.net) {
            // A net of few pads is mended from its ends at the cut: cut back to its
            // joints, one of two pads would have nothing left.
            kept.drop_floating(ctx);
        } else {
            kept.prune(ctx);
        }
        kept.place(world, ctx);
        if kept.pieces(ctx).apart() > 0 {
            let rest = completion(self.design, ctx, &kept, self.pitch);
            let near = Params {
                max_expansions: MEND_NODES,
                partial: false,
                stubs: false,
                ..self.params
            };
            let quiet = RipUp {
                penalty: None,
                history: self.history,
            };
            let steps = StepCache::default();
            let mut grow = NetRun::new(world, &rest, &steps, near);
            let joined = grow.stage(near, None, Some(quiet)).is_ok();
            let tree = grow.finish();
            self.expanded += tree.expanded;
            kept.unplace(world);
            if !joined {
                return None;
            }
            let more = NetCopper::from_tree(&rest, &tree);
            kept.segs.extend(more.segs);
            for v in more.vias {
                if !kept.vias.contains(&v) {
                    kept.vias.push(v);
                }
            }
            kept.merge(ctx);
            if kept.pieces(ctx).apart() > 0 {
                return None;
            }
        } else {
            kept.unplace(world);
        }
        // The ends that the cut left go.
        kept.prune(ctx);
        kept.place(world, ctx);
        Some(kept)
    }
}

/// How many times in a pass a net of few pads is cut and mended before it is taken out
/// whole.
const MENDS: u32 = 8;

/// How many nodes the search for a way to mend a net, around what cut it, may expand
/// before the routes in its way may be crossed.
const MEND_NODES: u64 = 20_000;

/// Whether a net in the way of another is cut where it is in the way and mended from the
/// ends that leaves, rather than taken out whole and routed again from nothing: a net with
/// no plane and fewer than [`BRANCH_TERMINALS`] terminals, in a run with a budget, `hurried`,
/// cut `cuts` times in this pass so far. Routed again from nothing, a net is searched for over
/// its whole length for a few lattice pitches of it that were in the way, and comes back
/// by another way altogether, in the way of other nets somewhere else.
fn mends(terminals: usize, plane: bool, hurried: bool, cuts: u32) -> bool {
    let most = if plane {
        BRANCH_CUTS.max(terminals as u32 / PLANE_CUTS)
    } else {
        MENDS
    };
    hurried && cuts < most
}

/// Whether a net has been made to wait in the first pass, and how the search in its
/// corridor ended that it waited after.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wait {
    No,
    NoPath,
    OutOfBudget,
    /// It has waited twice: its search over the board, which moved nothing, found no way
    /// either, and it waits for its turn to cross other nets' routes.
    Cross,
    /// It has, and had its turn since.
    Done,
}

/// The routes of every net at some point of rip-up, and how many connections they made.
struct Best {
    made: usize,
    copper: Vec<Option<NetCopper>>,
    missing: Vec<Vec<usize>>,
    apart: Vec<usize>,
}

impl Best {
    /// Puts these routes back in place of those there are.
    fn restore(
        self,
        world: &mut World<'_>,
        jobs: &[Job],
        copper: &mut Vec<Option<NetCopper>>,
        missing: &mut Vec<Vec<usize>>,
        apart: &mut Vec<usize>,
    ) {
        for c in copper.iter_mut().flatten() {
            c.unplace(world);
        }
        *copper = self.copper;
        for (job, c) in jobs.iter().zip(copper.iter_mut()) {
            if let Some(c) = c {
                c.place(world, &job.ctx);
            }
        }
        *missing = self.missing;
        *apart = self.apart;
    }
}

/// Cleanup's share of the time that a deadline leaves when routing begins: with zones to
/// join after it or after the rounds of cadlab's router, and after this crate's own rounds
/// with none to join.
const CLEANUP_SHARE_JOINING: f64 = 0.08;
const CLEANUP_SHARE: f64 = 0.25;

/// The shares of the time that a deadline leaves, and of the work that a budget leaves,
/// that the parts of a run may spend: routing, of what there is when it begins, and then
/// cleanup with the pairs, of what routing left. The rest is for joining zones' pieces, which makes the connections of the nets with planes
/// that routing cut, and takes a fill of the zones or several.
struct Shares {
    routing: (f64, f64),
    /// Cleanup's share of the time.
    cleanup: f64,
    /// The share of the work, as it is when routing begins, that the joining has.
    join_work: f64,
}

impl Shares {
    fn of(options: &RouteOptions, planes: bool) -> Shares {
        // Fills cost time and no work, so the shares of work are smaller.
        let join = if planes && options.join {
            (0.17, 0.05)
        } else {
            (0.0, 0.0)
        };
        // Cleanup makes no connection, and has no work kept for it: it has what work
        // routing leaves of its own share, which is all of it where routing ends early.
        //
        // Of the time a quarter, where no zones are to be joined after it and the rounds
        // are this crate's own: with a twelfth the passes of a board that its deadline
        // cuts took what cleanup begins with, each net again where the others are and
        // every net pulled tight, and ended with the connections they had
        // (`glasgow-revD1` at 60 s, four orders: 12, 17, 7 and 12 open with either, 1130
        // vias for 1146; `docs/benchmarks.md`). With zones to join the joining has its
        // share as it had, and cleanup the twelfth; and after the rounds of cadlab's
        // router, which have had their own pass for fewer vias and leave the passes a
        // few nets to settle in what is left of the run (`tinytapeout-demo`: 3
        // connections open in two runs of twelve with the quarter, in none of 32 with
        // the twelfth).
        let cleanup = match (
            options.cleanup,
            join.0 > 0.0 || options.rounds == super::Rounds::Native,
        ) {
            (false, _) => 0.0,
            (true, true) => CLEANUP_SHARE_JOINING,
            (true, false) => CLEANUP_SHARE,
        };
        // Of the time left when the part begins: cleanup's is of what routing left.
        let of_rest = |c: f64, j: f64| if j > 0.0 { c / (c + j) } else { 1.0 };
        Shares {
            routing: (1.0 - join.0 - cleanup, 1.0 - join.1),
            cleanup: of_rest(cleanup, join.0),
            join_work: join.1,
        }
    }
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
pub(super) fn completion(design: &Design, ctx: &NetCtx, copper: &NetCopper, pitch: i64) -> NetCtx {
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
    // A via of what completes the net keeps clear of those the copper has, hole to hole.
    let reach = ctx
        .via
        .map_or(0, |(_, drill)| drill + design.rules.spec().hole_to_hole);
    NetCtx::new(
        ctx.net,
        ctx.widths.clone(),
        ctx.via,
        pads,
        terminals,
        LATTICE_REACH * pitch,
    )
    .on_lattice(ctx.lattice())
    .with_vias(&copper.vias, reach)
}

/// By net, the centre of its pad with the least room beside it, if no track of the net
/// passes between that pad and the pad of another net next to it: the pad of a row of
/// fine pitch. A net's own lattice starts there, rather than at its first pad. Out of such
/// a row the tracks of pads side by side have no room to move sideways onto another
/// lattice, each beside the next, within the few pitches that a pad's lattice goes on
/// for: on the lattice of its pad in the row, a track goes straight on for as long as it
/// has to, and turns where the next one lets it.
fn row_origins(design: &Design) -> Vec<Option<Point>> {
    let rules = &design.rules;
    let stack = rules.layers();
    let pads = &design.board.pads;
    let copper: Vec<usize> = (0..pads.len())
        .filter(|&i| pads[i].layers.0 & stack.0 != 0)
        .collect();
    let index = SpatialIndex::bulk_load(copper.iter().map(|&i| pads[i].shape.bbox()));
    // By net, the least gap beside one of its pads, and that pad.
    let mut best: Vec<Option<(i64, usize)>> = vec![None; design.nets.len()];
    let mut near = Vec::new();
    for &i in &copper {
        let pad = &pads[i];
        let Some(net) = pad.net.filter(|n| n.index() < design.nets.len()) else {
            continue;
        };
        let spec = &design.nets[net.index()];
        if !spec.route || pad.hole.is_some() {
            continue;
        }
        let layer = LayerSet(pad.layers.0 & stack.0)
            .iter()
            .next()
            .unwrap_or(LayerId(0));
        let passage = spec.track_width.max(rules.track_width(Some(net), layer))
            + 2 * rules.clearance(Some(net), layer);
        let b = pad.shape.bbox();
        index.query(&b.expand(passage), &mut near);
        let mut gap: Option<i64> = None;
        for id in &near {
            let q = &pads[copper[id.slot() as usize]];
            if q.net == pad.net || q.layers.0 & pad.layers.0 & stack.0 == 0 {
                continue;
            }
            let o = q.shape.bbox();
            let dx = (o.min().x - b.max().x).max(b.min().x - o.max().x);
            let dy = (o.min().y - b.max().y).max(b.min().y - o.max().y);
            let d = dx.max(dy).max(0);
            if d < passage {
                gap = Some(gap.map_or(d, |g| g.min(d)));
            }
        }
        if let Some(g) = gap
            && best[net.index()].is_none_or(|(h, _)| g < h)
        {
            best[net.index()] = Some((g, i));
        }
    }
    best.into_iter()
        .map(|b| b.and_then(|(_, i)| pad_center(&pads[i].shape)))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::super::tests::{MM, design, smd};
    use super::{BRANCH_CUTS, BRANCH_TERMINALS, loses_branches, row_origins, said_all};
    use crate::geom::drc::Pad;
    use crate::geom::{Point, Rect, Shape};

    #[test]
    fn the_rounds_of_cadlab_s_router_plan_no_net_on_a_pour_of_an_outer_layer() {
        use super::super::{CopperZone, Fill};
        use super::{counted_fills, rounds_fills};
        use crate::geom::{LayerId, LayerSet};
        use crate::id::NetId;
        let square =
            || Shape::rect(Rect::new(Point::new(0, 0), Point::new(5 * MM, 5 * MM))).unwrap();
        // Net 1 has a zone on the top layer alone; net 2 one on the top and one inside.
        let zones = [(1, 0u8), (2, 1), (2, 0)];
        let with = |layers: usize| {
            let mut d = design(layers, vec![smd(MM, MM, 1), smd(2 * MM, 2 * MM, 2)], 3);
            for (net, layer) in zones {
                d.zones.zones.push(CopperZone {
                    layers: LayerSet::EMPTY.with(LayerId(layer)),
                    net: Some(NetId(net)),
                    ..CopperZone::default()
                });
            }
            d
        };
        let fills = || -> Vec<Fill> {
            zones
                .iter()
                .enumerate()
                .map(|(zone, &(_, layer))| Fill {
                    zone,
                    layer: LayerId(layer),
                    shape: square(),
                })
                .collect()
        };
        let which = |fills: Vec<Fill>| fills.iter().map(|f| f.zone).collect::<Vec<_>>();
        let planes = [true; 3];
        let d = with(4);
        // As planes, a net with a fill inside is planned on that, and one with none on what
        // it has; these rounds plan the second on none.
        assert_eq!(which(counted_fills(&d, fills(), &planes, false)), [0, 1]);
        assert_eq!(which(rounds_fills(&d, fills(), &planes, false)), [1]);
        // Asked for every fill, they have every fill.
        assert_eq!(which(rounds_fills(&d, fills(), &planes, true)), [0, 1, 2]);
        // A zone that is no plane is copper of its net as it is filled, wherever it is.
        let pour = [false, true, true];
        assert_eq!(which(rounds_fills(&d, fills(), &pour, false)), [1, 0]);
        // On two layers every fill is on an outer layer, and the nets are planned on them.
        let two = with(2);
        assert_eq!(
            which(rounds_fills(&two, fills(), &planes, false)),
            which(counted_fills(&two, fills(), &planes, false))
        );
    }

    #[test]
    fn cleanup_has_a_quarter_of_the_time_with_no_zones_to_join() {
        let mut options = crate::pcb::RouteOptions {
            join: false,
            ..Default::default()
        };
        let none = super::Shares::of(&options, true);
        assert_eq!(
            (none.routing, none.cleanup, none.join_work),
            ((0.75, 1.0), 1.0, 0.0)
        );
        // After the rounds of cadlab's router, the twelfth.
        options.rounds = crate::pcb::Rounds::Native;
        assert_eq!(super::Shares::of(&options, true).routing, (0.92, 1.0));
        options.rounds = crate::pcb::Rounds::Own;
        options.join = true;
        let joining = super::Shares::of(&options, true);
        assert!(
            (joining.routing.0 - 0.75).abs() < 1e-9,
            "{:?}",
            joining.routing
        );
        assert!(
            (joining.cleanup - 0.08 / 0.25).abs() < 1e-9,
            "{}",
            joining.cleanup
        );
        // The work is shared as it was: cleanup has none of its own.
        assert_eq!((joining.routing.1, joining.join_work), (0.95, 0.05));
        options.cleanup = false;
        assert_eq!(super::Shares::of(&options, false).routing, (1.0, 1.0));
    }

    #[test]
    fn a_net_of_two_pads_with_no_way_across_every_route_has_no_more_turns() {
        use super::super::detail::Failure;
        use super::{shut_in, stuck};
        // The search for the net whole that may cross every route, on the finest lattice
        // of the run, and went everywhere: the net has none.
        assert!(shut_in(true, false, 2, Failure::NoPath));
        // Not one that ran out of nodes, nor one that kept what it reached, nor one for a
        // net of more pads, which may still join some of them, nor one with a net locked
        // or on a lattice that a finer one follows.
        assert!(!shut_in(true, false, 2, Failure::Budget));
        assert!(!shut_in(true, true, 2, Failure::NoPath));
        assert!(!shut_in(true, false, 3, Failure::NoPath));
        assert!(!shut_in(false, false, 2, Failure::NoPath));
        // Such a net is at its floor: one connection, and none to be had.
        assert!(stuck(Some(1), 2, false, 0));
        assert!(!stuck(None, 2, false, 0));
    }

    #[test]
    fn a_search_across_routes_looks_from_its_other_end_before_it_has_the_whole_board() {
        use super::super::detail::{
            Corridor, Failure, History, NetCtx, Params, RipUp, StepCache, TermPad, route_net,
        };
        use super::super::world::World;
        use crate::geom::drc::Keepout;
        use crate::geom::{LayerId, LayerSet};
        use crate::id::NetId;
        // A net of two pads 100 mm apart on a board of one layer, 400 mm by 300 mm: the
        // first is walled in by keepouts, which nothing crosses, and the second by a
        // route of another net, which a search may cross. The look from the second finds
        // it shut in with nothing crossed, so the search that may cross is from it, and
        // went over the three million places of the board to say that the first pad is
        // not to be reached. It has 200 000 nodes for that now, and then the first pad
        // has a look of its own, which ends at its walls.
        let mut d = design(1, vec![smd(0, 0, 1), smd(100 * MM, 0, 1)], 3);
        let wall = |a: Point, b: Point| Keepout {
            shape: Shape::rect(Rect::new(a, b)).unwrap(),
            layers: LayerSet::first(1),
            tracks: true,
            vias: true,
            pads: false,
            except: Vec::new(),
        };
        d.board.keepouts.extend([
            wall(Point::new(-MM, -MM), Point::new(MM, -MM / 2)),
            wall(Point::new(-MM, MM / 2), Point::new(MM, MM)),
            wall(Point::new(-MM, -MM), Point::new(-MM / 2, MM)),
            wall(Point::new(MM / 2, -MM), Point::new(MM, MM)),
        ]);
        let bounds = Rect::new(
            Point::new(-150 * MM, -150 * MM),
            Point::new(250 * MM, 150 * MM),
        );
        let mut world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )
        .unwrap();
        let (x, r) = (100 * MM, MM);
        let ring = [
            Point::new(x - r, -r),
            Point::new(x + r, -r),
            Point::new(x + r, r),
            Point::new(x - r, r),
        ];
        for k in 0..4 {
            let side = Shape::track(ring[k], ring[(k + 1) % 4], 200_000).unwrap();
            world.add_track(NetId(2), LayerId(0), side);
        }
        let pad = |x: i64, terminal: usize| TermPad {
            center: Point::new(x, 0),
            layers: LayerSet::EMPTY.with(LayerId(0)),
            bbox: Rect::new(Point::new(x, 0), Point::new(x, 0)),
            terminal,
        };
        let ctx = NetCtx::new(
            NetId(1),
            vec![200_000],
            None,
            vec![pad(0, 0), pad(x, 1)],
            2,
            8 * 200_000,
        );
        let params = Params {
            pitch: 200_000,
            via_cost: 3.0,
            layer_bias: 0.0,
            bounds,
            max_expansions: 16_000_000,
            under_pads: false,
            partial: false,
            stubs: false,
            forest: false,
        };
        let history = History {
            grid: Corridor {
                origin: bounds.min(),
                tile: 400 * MM,
                cols: 1,
                rows: 1,
                cells: vec![0],
            },
            cost: vec![0.0],
        };
        let rip = RipUp {
            penalty: Some(10.0),
            history: &history,
        };
        let cache = StepCache::default();
        match route_net(&world, &ctx, &cache, params, None, Some(rip)) {
            Err((Failure::NoPath, expanded)) => assert!(expanded < 500_000, "{expanded}"),
            other => panic!("{:?}", other.map(|t| t.expanded)),
        }
    }

    #[test]
    fn a_negotiation_that_ended_with_a_point_shared_is_not_begun_again() {
        use super::{WHOLE_FEW, tried_again};
        // With points shared, it is: on another grid, in another order.
        assert!(tried_again(true, 2, false));
        assert!(tried_again(true, 60, false));
        // With none, or with the one that is a wire for the passes to lay again, it is not.
        assert!(!tried_again(true, 0, false));
        assert!(!tried_again(true, WHOLE_FEW, false));
        // Nor one that its share ended, or its rounds at their most.
        assert!(!tried_again(false, 60, false));
        assert!(!tried_again(true, 60, true));
    }

    #[test]
    fn a_run_with_a_budget_of_nodes_joins_its_fills_again_before_it_routes_each_net_again() {
        use super::again_before_joining;
        assert!(again_before_joining(None));
        assert!(!again_before_joining(Some(40_000_000)));
        assert!(!again_before_joining(Some(0)));
    }

    #[test]
    fn a_pass_on_the_finer_lattice_that_moved_no_net_is_the_last() {
        // The first pass is never the last for that: the lattice of the next is finer.
        assert!(!said_all(0, (3, 1), (3, 1)));
        // One on the finer lattice that took no net out and moved none aside is.
        assert!(said_all(1, (3, 1), (3, 1)));
        assert!(said_all(2, (0, 0), (0, 0)));
        // And one that took a net out, or moved one aside, is not: the next has the nets
        // that this one could take out no more free again.
        assert!(!said_all(1, (3, 1), (4, 1)));
        assert!(!said_all(1, (3, 1), (3, 2)));
    }

    #[test]
    fn a_net_s_lattice_starts_at_its_pad_in_a_row_of_fine_pitch() {
        // Four nets, each from a pad on its own to a pad 0.2 mm wide in a row at 0.4 mm,
        // where no track passes between two pads, and a fifth between two pads apart.
        let mut pads: Vec<Pad> = (0..4)
            .map(|k| smd(k * 2 * MM, 5 * MM, k as u32 + 1))
            .collect();
        for k in 0..4 {
            let x = k * 400_000;
            pads.push(Pad {
                shape: Shape::rect(Rect::new(
                    Point::new(x - 100_000, -400_000),
                    Point::new(x + 100_000, 400_000),
                ))
                .unwrap(),
                ..smd(x, 0, k as u32 + 1)
            });
        }
        pads.push(smd(10 * MM, 10 * MM, 5));
        pads.push(smd(12 * MM, 10 * MM, 5));
        let origins = row_origins(&design(1, pads, 6));
        for k in 0..4 {
            assert_eq!(origins[k + 1], Some(Point::new(k as i64 * 400_000, 0)));
        }
        assert_eq!(origins[5], None);
        assert_eq!(origins[0], None);
    }

    #[test]
    fn what_completes_a_net_puts_no_via_beside_one_it_has() {
        use super::super::detail::{DNode, DetailSpace, NetCtx, Params, StepCache, TermPad};
        use super::super::geometry::{NetCopper, Seg};
        use super::super::world::World;
        use super::completion;
        use crate::geom::{LayerId, LayerSet};
        use crate::id::NetId;
        use crate::space::RoutingSpace;
        // A net with a track from its first pad to a via, and a pad still to join. The
        // copper is not in the world while a negotiation looks for the rest, and what is
        // found is added to it: a via a pitch from the one it has would be a via on a via.
        let d = design(2, vec![smd(0, 0, 1), smd(4 * MM, 3 * MM, 1)], 2);
        let world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )
        .unwrap();
        let pad = |x: i64, y: i64, terminal: usize| TermPad {
            center: Point::new(x, y),
            layers: LayerSet::EMPTY.with(LayerId(0)),
            bbox: Rect::new(Point::new(x, y), Point::new(x, y)),
            terminal,
        };
        let via = (600_000, 300_000);
        let whole = NetCtx::new(
            NetId(1),
            vec![200_000; 2],
            Some(via),
            vec![pad(0, 0, 0), pad(4 * MM, 3 * MM, 1)],
            2,
            8 * 200_000,
        );
        let mut copper = NetCopper::default();
        copper.segs.push(Seg {
            a: Point::new(0, 0),
            b: Point::new(2 * MM, 0),
            l: 0,
        });
        copper.vias.push(Point::new(2 * MM, 0));
        let params = Params {
            pitch: 200_000,
            via_cost: 3.0,
            layer_bias: 0.0,
            bounds: Rect::new(Point::new(-MM, -MM), Point::new(6 * MM, 6 * MM)),
            max_expansions: 100_000,
            under_pads: false,
            partial: false,
            stubs: false,
            forest: false,
        };
        let changes_layer = |ctx: &NetCtx, x: i64| {
            let cache = StepCache::default();
            let space = DetailSpace::new(&world, ctx, &cache, params, None, None);
            let from = DNode { x, y: 0, l: 0 };
            let mut steps = Vec::new();
            space.expand(NetId(1), from, &mut steps);
            steps.iter().any(|s| s.to.l != 0)
        };
        let rest = completion(&d, &whole, &copper, 200_000);
        // Hole to hole is 0.25 mm in these rules, and the holes are 0.3 mm.
        assert!(!changes_layer(&rest, 2 * MM + 200_000));
        assert!(!changes_layer(&rest, 2 * MM - 400_000));
        assert!(changes_layer(&rest, 3 * MM));
        // The net with no copper yet has a via wherever the board has room.
        assert!(changes_layer(&whole, 2 * MM + 200_000));
    }

    #[test]
    fn a_run_with_a_budget_pulls_every_net_tight_before_it_looks_for_fewer_vias() {
        use super::super::cleanup;
        use super::super::control::Run;
        use super::super::detail::Params;
        use super::super::geometry::{NetCopper, Seg};
        use super::super::world::World;
        use super::super::{RouteOptions, RunControl, Stopped};
        // Two nets of two pads on the top layer. The first goes under and back up by two
        // vias for nothing, and the second round by a way that is longer than it has to
        // be, as rip-up leaves routes.
        let pads = vec![
            smd(0, 0, 1),
            smd(4 * MM, 0, 1),
            smd(0, 3 * MM, 2),
            smd(4 * MM, 3 * MM, 2),
        ];
        let d = design(2, pads, 3);
        let options = RouteOptions::default();
        // A budget of one node: the first search for fewer vias ends the run.
        let control = RunControl::default();
        let run = Run::new(&control, Some(1));
        let mut world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )
        .unwrap();
        world.configure(&d, &options);
        world.set_run(&run);
        let pitch = super::pitch(&d, &options);
        let jobs = super::jobs(&d, &world, pitch, &[], &[]);
        assert_eq!(jobs.len(), 2);
        let seg = |a: (i64, i64), b: (i64, i64), l: u8| Seg {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            l,
        };
        let mut under = NetCopper::default();
        under.segs = vec![
            seg((0, 0), (MM, 0), 0),
            seg((MM, 0), (3 * MM, 0), 1),
            seg((3 * MM, 0), (4 * MM, 0), 0),
        ];
        under.vias = vec![Point::new(MM, 0), Point::new(3 * MM, 0)];
        let mut round = NetCopper::default();
        round.segs = vec![
            seg((0, 3 * MM), (MM, 4 * MM), 0),
            seg((MM, 4 * MM), (3 * MM, 4 * MM), 0),
            seg((3 * MM, 4 * MM), (4 * MM, 3 * MM), 0),
        ];
        let mut copper = vec![Some(under), Some(round)];
        for (job, c) in jobs.iter().zip(copper.iter_mut()) {
            c.as_mut().unwrap().place(&mut world, &job.ctx);
        }
        let params = Params {
            pitch,
            via_cost: options.via_cost,
            layer_bias: options.layer_bias,
            bounds: super::bounds_of(&d),
            max_expansions: options.max_expansions,
            under_pads: false,
            partial: false,
            stubs: false,
            forest: false,
        };
        cleanup::run(
            &mut world,
            &d,
            &jobs,
            &mut copper,
            params,
            &options,
            (&[], false),
        );
        // The search for the first net's route with no vias spent the budget, and the
        // second net is straight all the same: it was pulled tight before.
        assert_eq!(run.stopped(), Some(Stopped::WorkBudget));
        let round = copper[1].as_ref().unwrap();
        assert_eq!(round.segs, [seg((0, 3 * MM), (4 * MM, 3 * MM), 0)]);
    }

    #[test]
    fn the_searches_for_fewer_vias_made_ahead_change_no_route() {
        use super::super::cleanup;
        use super::super::control::Run;
        use super::super::detail::Params;
        use super::super::geometry::{NetCopper, Seg};
        use super::super::world::World;
        use super::super::{RouteOptions, RunControl};
        // Six nets of two pads on the top layer, one above the other. Every second one
        // goes under and back up by two vias for nothing, and loses them; the others have
        // a via at an end of a track that goes nowhere, which a search leaves as it is.
        // So the world is another one three times while the searches of the nets after
        // are under way on the other threads.
        let nets = 6;
        let pads: Vec<_> = (0..nets)
            .flat_map(|k| {
                let y = k * 2 * MM;
                [smd(0, y, k as u32 + 1), smd(4 * MM, y, k as u32 + 1)]
            })
            .collect();
        let d = design(2, pads, nets as usize + 1);
        let seg = |a: (i64, i64), b: (i64, i64), l: u8| Seg {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            l,
        };
        let cleaned = |threads: usize| {
            let options = RouteOptions {
                threads,
                ..RouteOptions::default()
            };
            let control = RunControl::default();
            let run = Run::new(&control, None);
            let mut world = World::new(
                &d.board,
                &d.rules,
                &d.outline,
                d.edge_clearance,
                &d.obstacles,
            )
            .unwrap();
            world.configure(&d, &options);
            world.set_run(&run);
            let pitch = super::pitch(&d, &options);
            let jobs = super::jobs(&d, &world, pitch, &[], &[]);
            assert_eq!(jobs.len(), nets as usize);
            let mut copper: Vec<Option<NetCopper>> = jobs
                .iter()
                .map(|job| {
                    let y = job.ctx.pads[0].center.y;
                    let mut c = NetCopper::default();
                    if (y / (2 * MM)) % 2 == 0 {
                        c.segs = vec![
                            seg((0, y), (MM, y), 0),
                            seg((MM, y), (3 * MM, y), 1),
                            seg((3 * MM, y), (4 * MM, y), 0),
                        ];
                        c.vias = vec![Point::new(MM, y), Point::new(3 * MM, y)];
                    } else {
                        c.segs = vec![seg((0, y), (4 * MM, y), 0)];
                        c.vias = vec![Point::new(2 * MM, y)];
                    }
                    Some(c)
                })
                .collect();
            for (job, c) in jobs.iter().zip(copper.iter_mut()) {
                c.as_mut().unwrap().place(&mut world, &job.ctx);
            }
            let params = Params {
                pitch,
                via_cost: options.via_cost,
                layer_bias: options.layer_bias,
                bounds: super::bounds_of(&d),
                max_expansions: options.max_expansions,
                under_pads: false,
                partial: false,
                stubs: false,
                forest: false,
            };
            let expanded = cleanup::run(
                &mut world,
                &d,
                &jobs,
                &mut copper,
                params,
                &options,
                (&[], false),
            );
            let routes: Vec<(Vec<Seg>, Vec<Point>)> = copper
                .into_iter()
                .flatten()
                .map(|c| (c.segs, c.vias))
                .collect();
            (routes, expanded, run.work())
        };
        let one = cleaned(1);
        // The nets that went under are straight on the top layer.
        assert!(one.0.iter().step_by(2).all(|(_, vias)| vias.is_empty()));
        assert!(one.1 > 0);
        for threads in [2, 3, 8] {
            assert_eq!(cleaned(threads), one, "on {threads} threads");
        }
    }

    #[test]
    fn a_net_with_a_plane_loses_only_the_stubs_in_the_way_in_a_run_with_a_budget() {
        // A ground of 584 pads, as stubs to its plane: in the first pass too, and far more
        // often than a net without a plane, before it is taken out whole.
        assert!(loses_branches(0, 584, true, true, 0));
        assert!(loses_branches(0, 584, true, true, 100));
        assert!(!loses_branches(0, 584, true, true, 146));
        assert!(loses_branches(1, 584, true, true, 0));
        // A small one is taken out whole, as it is quick to route again.
        assert!(!loses_branches(0, BRANCH_TERMINALS - 1, true, true, 0));
        assert!(loses_branches(
            0,
            BRANCH_TERMINALS,
            true,
            true,
            BRANCH_CUTS - 1
        ));
        assert!(!loses_branches(
            0,
            BRANCH_TERMINALS,
            true,
            true,
            BRANCH_CUTS
        ));
        // With no budget it is taken out whole, in any pass.
        assert!(!loses_branches(0, 584, true, false, 0));
        assert!(!loses_branches(1, 584, true, false, 0));
        // Without a plane, only on the finer lattice, a few times, budget or none.
        for hurried in [false, true] {
            assert!(!loses_branches(0, 584, false, hurried, 0));
            assert!(loses_branches(1, 584, false, hurried, BRANCH_CUTS - 1));
            assert!(!loses_branches(1, 584, false, hurried, BRANCH_CUTS));
        }
    }
    #[test]
    fn the_lattice_has_the_pitch_of_the_tracks_that_a_row_made_narrower() {
        use super::super::RouteOptions;
        use super::lattice_pitch;
        let plain = RouteOptions::default();
        // Tracks of 0.185 mm with a clearance of 0.2 mm, where the widths before were
        // 0.2 mm at least: two of them side by side are two steps of 0.1925 mm apart.
        assert_eq!(lattice_pitch(200_000, Some(192_500), &plain), 192_500);
        // No row made a net narrower, or the row's tracks are no tighter: as it was.
        assert_eq!(lattice_pitch(200_000, None, &plain), 200_000);
        assert_eq!(lattice_pitch(176_600, Some(193_700), &plain), 176_600);
        // A pitch that the options set stays, and so does that of a run with a budget of
        // nodes, which a finer lattice costs more of for the same way.
        let set = RouteOptions {
            pitch: Some(250_000),
            ..RouteOptions::default()
        };
        assert_eq!(lattice_pitch(250_000, Some(192_500), &set), 250_000);
        let nodes = RouteOptions {
            work_budget: Some(40_000_000),
            ..RouteOptions::default()
        };
        assert_eq!(lattice_pitch(200_000, Some(192_500), &nodes), 200_000);
    }
}
