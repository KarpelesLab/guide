//! The whole run: nets from the design, escapes, global routing, detailed routing with
//! rip-up, cleanup, pairs side by side, and meanders.

use std::collections::{BTreeMap, VecDeque};

use super::cleanup;
use super::detail::{self, Corridor, Failure, History, NetCtx, Params, RipUp, StepCache, TermPad};
use super::escape;
use super::geometry::NetCopper;
use super::global::{self, GlobalSpace, Probe, TerminalPads};
use super::pairs;
use super::planes;
use super::tune;
use super::world::{Placed, Planes, Verdict, World};
use super::zones::{self, Fill};
use super::{Design, DiffPair, Options, RouteError, Routed, Stats};
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

/// How many nodes a search from a terminal may expand to find it shut in, before a search
/// over the whole board for it.
const PRECHECK: u64 = 50_000;

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
    let Some(route) = pairs::route_pair(design, world, pair, &jobs[jp], &jobs[jn], params, None)
    else {
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
    let mut routed = route_all(design, options)?;
    if options.tune_lengths && !design.lengths.is_empty() {
        tune::run(design, &mut routed, options).map_err(RouteError::Geometry)?;
        // Meanders change what zones fill around, and may cut a plane in pieces, which are
        // joined up again as after routing.
        if routed.stats.meanders > 0.0 && !design.zones.zones.is_empty() {
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
                let mut joined = Joined {
                    tracks: &mut routed.tracks,
                    vias: &mut routed.vias,
                    stats: &mut routed.stats,
                    unrouted: &mut routed.unrouted,
                };
                join_planes(
                    design,
                    &mut world,
                    &mut joined,
                    pitch(design, options),
                    options,
                )?;
            }
            routed.fills =
                zones::fill_zones(&with_routes(design, &routed.tracks, &routed.vias), design)
                    .map_err(RouteError::Geometry)?;
        }
    }
    if options.planes && !design.zones.zones.is_empty() {
        count_connections(design, &mut routed);
    }
    Ok(routed)
}

/// The lattice's pitch: half the tightest pitch of the tracks routed, unless the options
/// set one.
pub(crate) fn pitch(design: &Design, options: &Options) -> i64 {
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

/// Counts nets and connections again, through zones' fills: a net needs connecting if its
/// pads are in pieces that fixed copper doesn't join, and a connection is made for each
/// piece fewer that its copper, routes and fills leave it in.
fn count_connections(design: &Design, routed: &mut Routed) {
    let board = routed.board(design);
    let (nets, done, needed, made, _) = connectivity(design, &board);
    let s = &mut routed.stats;
    (s.nets, s.routed, s.connections, s.connected) = (nets, done, needed, made);
}

/// What [`count_connections`] counts, on a board with everything routed and filled: the nets
/// that need connecting, of those the nets connected, the connections needed and made, and the
/// nets left in pieces, in order.
pub(crate) fn connectivity(
    design: &Design,
    board: &Board,
) -> (usize, usize, usize, usize, Vec<NetId>) {
    let (mut nets, mut done, mut needed, mut made) = (0, 0, 0, 0);
    let mut broken = Vec::new();
    for (i, spec) in design.nets.iter().enumerate() {
        let net = NetId(i as u32);
        if !spec.route {
            continue;
        }
        let groups = terminals(design, net)
            .into_iter()
            .filter(|(pads, _)| {
                pads.iter()
                    .any(|&p| design.board.pads[p].layers.0 & design.rules.layers().0 != 0)
            })
            .count();
        if groups < 2 {
            continue;
        }
        let left = planes::pieces(board, design, net, &[])
            .pads
            .len()
            .clamp(1, groups);
        nets += 1;
        needed += groups - 1;
        made += groups - left;
        if left == 1 {
            done += 1;
        } else {
            broken.push(net);
        }
    }
    (nets, done, needed, made, broken)
}

/// What [`join_planes`] adds to.
struct Joined<'a> {
    tracks: &'a mut Vec<Track>,
    vias: &'a mut Vec<Via>,
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
    options: &Options,
) -> Result<(), RouteError> {
    let keeping = keep_islands(design);
    for round in 0..=REPAIRS {
        let board = with_routes(design, out.tracks, out.vias);
        let all = zones::fill_zones(&board, &keeping).map_err(RouteError::Geometry)?;
        let mut added = false;
        let mut left = Vec::new();
        for (net, pieces) in planes::pieces_by_net(design, &all) {
            if !design.nets.get(net.index()).is_some_and(|n| n.route)
                || planes::pieces(&board, design, net, &pieces).with_pads() < 2
            {
                continue;
            }
            left.push(net);
            if round == REPAIRS {
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
                out.tracks.extend(s.tracks);
                out.vias.extend(s.vias);
                out.stats.stubs += 1;
                added = true;
            }
        }
        // What nothing more joins is left in pieces.
        if !added || round == REPAIRS {
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
fn planned_fills(design: &Design, fills: &[Fill]) -> Vec<Fill> {
    let layers = design.rules.layer_count();
    let net_of = |f: &Fill| design.zones.zones.get(f.zone).and_then(|z| z.net);
    fills
        .iter()
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

/// Routes everything but meanders.
fn route_all(design: &Design, options: &Options) -> Result<Routed, RouteError> {
    let original = design;
    let rules = &design.rules;
    let world_of = |design: &Design| {
        World::new(
            &design.board,
            rules,
            &design.outline,
            design.edge_clearance,
            &design.obstacles,
        )
        .map_err(RouteError::Geometry)
    };

    let pitch = pitch(design, options);
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
            precheck: 0,
            partial: false,
            stubs: false,
        });
        // The pads of nets with zones near the pairs' ends, which a via takes to their
        // planes, keep a place for it.
        let kept = keep_exits(design, &mut world, pitch, &pairs_first.jobs);
        for pair in &design.pairs {
            let route = pairs_first.route(design, &mut world, pair);
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
    let planes_on = options.planes && design.zones.zones.iter().any(|z| z.net.is_some());
    let mut planned: Vec<Fill> = Vec::new();
    if planes_on {
        let fills = zones::fill_zones(&design.board, &keep_islands(design))
            .map_err(RouteError::Geometry)?;
        planned = planned_fills(design, &fills);
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
        precheck: 0,
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
    let mut queue: VecDeque<usize> = (0..jobs.len()).collect();
    let by_net: BTreeMap<NetId, usize> = jobs
        .iter()
        .enumerate()
        .map(|(i, j)| (j.ctx.net, i))
        .collect();
    let mut outside = vec![false; jobs.len()];
    // By job, the terminals its partial route left out.
    let mut missing: Vec<Vec<usize>> = vec![Vec::new(); jobs.len()];
    // By job, for a net routed in part that a later pass completes, the net with its copper
    // so far as its first terminal and the terminals left out as the others.
    let mut completing: Vec<Option<NetCtx>> = vec![None; jobs.len()];
    // A second pass, on a finer lattice, for the nets the first leaves unrouted, or routed in
    // part, with every net that can be ripped up again.
    for pass in 0..=FINE_PASSES {
        let params = if pass == 0 {
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
            for &j in &unfinished {
                if let Some(c) = &copper[j] {
                    completing[j] = Some(completion(design, &jobs[j].ctx, c, &missing[j], pitch));
                }
            }
            queue.extend(left);
            queue.extend(unfinished);
            for (j, job) in jobs.iter().enumerate() {
                ripped[j] = 0;
                world.set_locked(job.ctx.net, false);
            }
            // Pairs routed first may be moved, but those that found no way after being
            // moved before.
            for (&net, &i) in &pair_of {
                world.set_locked(net, pair_rips[i] >= PAIR_RIP_UPS);
            }
            Params {
                pitch: (pitch / FINE).max(1000),
                under_pads: true,
                ..params
            }
        };
        while let Some(j) = queue.pop_front() {
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
                match detail::route_net(&world, ctx, &steps, params, Some(corridor), Some(rip)) {
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
                match detail::route_net(&world, ctx, &steps, checked, None, Some(rip)) {
                    Ok(t) => {
                        stats.expanded += t.expanded;
                        outside[j] = job.corridor.is_some();
                        tree = Some(t);
                    }
                    Err((_, e)) => stats.expanded += e,
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
            }
            let mut displaced: Vec<usize> = Vec::new();
            for &partial in &attempts {
                if tree.is_some() || options.rip_ups == 0 {
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
                match detail::route_net(&world, ctx, &steps, partial, None, Some(rip)) {
                    Ok(t) => {
                        stats.expanded += t.expanded;
                        let (victims, at) = detail::victims(&world, ctx, &t);
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
            if tree.is_none() && partial.partial && options.rip_ups == 0 {
                let rip = RipUp {
                    penalty: None,
                    history: &history,
                };
                match detail::route_net(&world, ctx, &steps, partial, None, Some(rip)) {
                    Ok(t) => {
                        stats.expanded += t.expanded;
                        tree = Some(t);
                    }
                    Err((_, e)) => stats.expanded += e,
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
                        missing[j] = t.missing.iter().map(|&k| missing[j][k - 1]).collect();
                    }
                    (_, old) => {
                        if let Some(mut old) = old {
                            old.unplace(&mut world);
                        }
                        missing[j] = t.missing.clone();
                    }
                }
                c.place(&mut world, &job.ctx);
                copper[j] = Some(c);
            }
            // The pairs this net moved are routed again side by side around it, moving
            // other nets in their way in turn, but not pairs, nor this net. A pair that finds
            // no way goes back where it was, and this net is routed again around it.
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
    let first_pairs = pairs_first.routes.iter().flatten().count();
    for r in pairs_first.routes.iter().flatten() {
        tracks.extend(r.tracks.iter().filter(|t| kept(t.net)).cloned());
        vias.extend(r.vias.iter().filter(|v| kept(v.net)).cloned());
    }
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
    stats.routed = (0..jobs.len())
        .filter(|&j| copper[j].is_some() && missing[j].is_empty())
        .count();
    stats.connected = (0..jobs.len())
        .filter(|&j| copper[j].is_some())
        .map(|j| jobs[j].ctx.terminals - 1 - missing[j].len())
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
    }
    // Zones filled around everything, and the pieces of their nets joined up. Islands are
    // kept until the end, as a plane that nothing reaches yet is still one to reach, and
    // they join nothing.
    if planes_on {
        let mut joined = Joined {
            tracks: &mut tracks,
            vias: &mut vias,
            stats: &mut stats,
            unrouted: &mut unrouted,
        };
        join_planes(original, &mut world, &mut joined, params.pitch, options)?;
    }
    let mut fills = Vec::new();
    if !original.zones.zones.is_empty() {
        let board = with_routes(original, &tracks, &vias);
        fills = zones::fill_zones(&board, original).map_err(RouteError::Geometry)?;
    }
    Ok(Routed {
        tracks,
        vias,
        fills,
        unrouted,
        stats,
    })
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
        pairs::route_pair(design, world, pair, p, n, self.params?, None)
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

/// A net routed in part, `missing` the terminals it left out, as a net to complete: its
/// copper and the terminals it reached are its first terminal, and those it left out the
/// others, in order.
fn completion(
    design: &Design,
    ctx: &NetCtx,
    copper: &NetCopper,
    missing: &[usize],
    pitch: i64,
) -> NetCtx {
    let mut pads: Vec<TermPad> = ctx
        .pads
        .iter()
        .filter(|p| !missing.contains(&p.terminal))
        .map(|p| TermPad {
            terminal: 0,
            ..p.clone()
        })
        .collect();
    let point = |at: Point, layers: LayerSet| TermPad {
        center: at,
        layers,
        bbox: Rect::new(at, at),
        terminal: 0,
    };
    for s in &copper.segs {
        let on = LayerSet::EMPTY.with(LayerId(s.l));
        pads.push(point(s.a, on));
        pads.push(point(s.b, on));
    }
    for &v in &copper.vias {
        pads.push(point(v, design.rules.layers()));
    }
    for (k, &t) in missing.iter().enumerate() {
        pads.extend(
            ctx.pads
                .iter()
                .filter(|p| p.terminal == t)
                .map(|p| TermPad {
                    terminal: k + 1,
                    ..p.clone()
                }),
        );
    }
    NetCtx::new(
        ctx.net,
        ctx.widths.clone(),
        ctx.via,
        pads,
        missing.len() + 1,
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
