//! Plane-aware routing: nets with copper zones connect to them, rather than across the board.
//!
//! Before anything else is routed, the zones are filled around the fixed copper, islands
//! kept. A zone net's pads that its fills don't reach get a *stub*: a short track, and a via
//! if the fill is on another layer, to the largest piece of its fills, its *main* piece,
//! searched for on the lattice of detailed routing, to the nearest places where a via, or a
//! track's end, fits in the fill. Fills on inner layers are planes, which the rest of
//! routing doesn't cut through without paying for it, and a net with a plane connects to it
//! alone. A net with zones on outer layers only, which routes may cut in pieces, takes them
//! as they are, and relies on what comes after routing.
//!
//! After routing, the zones are filled again around everything, and every zone net is
//! checked: its pads, routes and fills must be one piece. A piece left out is joined to the
//! main one by a stub from any of its pads, tracks or vias, or failing that by tracks to the
//! main piece's, and the zones are filled again, a few times at most.

use super::Design;
use super::detail::{self, NetCtx, Params, StepCache, TermPad};
use super::geometry::NetCopper;
use super::router::{LATTICE_REACH, pad_center};
use super::world::{Verdict, World};
use super::zones::Fill;
use crate::geom::drc::{Board, Track, Via};
use crate::geom::{ArcSide, Area, LayerId, LayerSet, Point, Rect, Shape, SpatialIndex};
use crate::id::NetId;

/// How far from where it leaves a stub may reach.
const STUB_REACH: i64 = 4_000_000;

/// How many places in the fill a stub's search aims for around each place it may leave
/// from, nearest first.
const STUB_SITES: usize = 24;

/// How many of a piece's places to leave from are aimed around.
const STUB_SOURCES: usize = 6;

/// How many nodes a stub's search may expand.
const STUB_BUDGET: u64 = 60_000;

/// How many nodes a search for tracks between pieces may expand.
const JOIN_BUDGET: u64 = 400_000;

/// Copper added to connect a net to its zones.
#[derive(Clone, Debug, Default)]
pub(crate) struct Stub {
    pub(crate) tracks: Vec<Track>,
    pub(crate) vias: Vec<Via>,
}

/// A piece of fill of a net, ready for point queries.
pub(crate) struct Piece {
    pub(crate) shape: Shape,
    pub(crate) area: Area,
    pub(crate) layer: LayerId,
    pub(crate) bbox: Rect,
    pub(crate) size: i128,
}

/// The pieces of fill of each net, by net.
pub(crate) fn pieces_by_net(design: &Design, fills: &[Fill]) -> Vec<(NetId, Vec<Piece>)> {
    let mut out: Vec<(NetId, Vec<Piece>)> = Vec::new();
    for f in fills {
        let Some(net) = design.zones.zones.get(f.zone).and_then(|z| z.net) else {
            continue;
        };
        let Ok(area) = Area::of_shape(&f.shape, 0, 1, ArcSide::Inside) else {
            continue;
        };
        let piece = Piece {
            bbox: f.shape.bbox(),
            size: area.area2(),
            shape: f.shape.clone(),
            area,
            layer: f.layer,
        };
        match out.iter_mut().find(|(n, _)| *n == net) {
            Some((_, list)) => list.push(piece),
            None => out.push((net, vec![piece])),
        }
    }
    out.sort_by_key(|(n, _)| *n);
    out
}

/// Whether a layer is inside the stack, where zones are planes.
pub(crate) fn inner(layer: LayerId, layers: usize) -> bool {
    layer.0 > 0 && usize::from(layer.0) + 1 < layers
}

/// A net's copper in pieces: which pads and fills each piece of it holds.
pub(crate) struct Pieces {
    /// By piece, its pads, by position in the board's.
    pub(crate) pads: Vec<Vec<usize>>,
    /// By piece, its fills, by position in the list given.
    pub(crate) fills: Vec<Vec<usize>>,
    /// By piece, the ends of its tracks and its vias, with their layers, where routes can
    /// join it too.
    pub(crate) points: Vec<Vec<(Point, LayerSet)>>,
}

impl Pieces {
    /// How many pieces hold pads.
    pub(crate) fn with_pads(&self) -> usize {
        self.pads.iter().filter(|p| !p.is_empty()).count()
    }

    /// The piece with the most fill, the main one, if any has fill.
    pub(crate) fn main(&self, fills: &[Piece]) -> Option<usize> {
        (0..self.pads.len())
            .filter(|&k| !self.fills[k].is_empty())
            .max_by_key(|&k| {
                let size: i128 = self.fills[k].iter().map(|&f| fills[f].size).sum();
                (size, core::cmp::Reverse(k))
            })
    }
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// How a net's pads, its copper on `board` and its `fills` connect: the pieces that hold
/// pads, in the order of their first pad, then those of fills alone.
pub(crate) fn pieces(board: &Board, design: &Design, net: NetId, fills: &[Piece]) -> Pieces {
    let stack = design.rules.layers();
    let mut items: Vec<(Shape, LayerSet)> = Vec::new();
    let pads: Vec<usize> = (0..board.pads.len())
        .filter(|&i| board.pads[i].net == Some(net) && board.pads[i].layers.0 & stack.0 != 0)
        .collect();
    for &p in &pads {
        items.push((board.pads[p].shape.clone(), board.pads[p].layers));
    }
    let first_fill = items.len();
    for f in fills {
        items.push((f.shape.clone(), LayerSet::EMPTY.with(f.layer)));
    }
    let first_end = items.len();
    // By item past the fills, where routes can join it.
    let mut ends: Vec<Vec<(Point, LayerSet)>> = Vec::new();
    for t in board.tracks.iter().filter(|t| t.net == Some(net)) {
        let shape = match t.mid {
            None => Shape::track(t.start, t.end, t.width),
            Some(m) => Shape::arc_track(t.start, m, t.end, t.width, board.arc_tolerance),
        };
        if let Ok(s) = shape {
            let on = LayerSet::EMPTY.with(t.layer);
            items.push((s, on));
            ends.push(vec![(t.start, on), (t.end, on)]);
        }
    }
    for v in board.vias.iter().filter(|v| v.net == Some(net)) {
        if let Ok(s) = Shape::circle(v.at, v.diameter) {
            let span = LayerSet(LayerSet::span(v.from, v.to).0 & stack.0);
            items.push((s, span));
            ends.push(vec![(v.at, span)]);
        }
    }
    for z in board.zones.iter().filter(|z| z.net == Some(net)) {
        items.push((z.shape.clone(), LayerSet::EMPTY.with(z.layer)));
        ends.push(Vec::new());
    }
    let mut parent: Vec<usize> = (0..items.len()).collect();
    let index = SpatialIndex::bulk_load(items.iter().map(|(s, _)| s.bbox()));
    let mut near = Vec::new();
    for i in 0..items.len() {
        index.query(&items[i].0.bbox(), &mut near);
        for id in &near {
            let j = id.slot() as usize;
            if j <= i || items[i].1.0 & items[j].1.0 & stack.0 == 0 {
                continue;
            }
            let (a, b) = (find(&mut parent, i), find(&mut parent, j));
            if a != b && items[i].0.touches(&items[j].0) {
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut roots: Vec<usize> = Vec::new();
    let mut out = Pieces {
        pads: Vec::new(),
        fills: Vec::new(),
        points: Vec::new(),
    };
    let piece_of = |r: usize, roots: &mut Vec<usize>, out: &mut Pieces| match roots
        .iter()
        .position(|&x| x == r)
    {
        Some(at) => at,
        None => {
            roots.push(r);
            out.pads.push(Vec::new());
            out.fills.push(Vec::new());
            out.points.push(Vec::new());
            roots.len() - 1
        }
    };
    for (k, &p) in pads.iter().enumerate() {
        let r = find(&mut parent, k);
        let at = piece_of(r, &mut roots, &mut out);
        out.pads[at].push(p);
    }
    // Fills that no pad reaches are pieces too, with no pads.
    for f in 0..fills.len() {
        let r = find(&mut parent, first_fill + f);
        let at = piece_of(r, &mut roots, &mut out);
        out.fills[at].push(f);
    }
    for (k, points) in ends.into_iter().enumerate() {
        let r = find(&mut parent, first_end + k);
        if let Some(at) = roots.iter().position(|&x| x == r) {
            out.points[at].extend(points);
        }
    }
    out
}

/// Where routes may join a piece of a net: its pads' centres, its tracks' ends and its
/// vias, as terminal `terminal`.
fn sources(
    board: &Board,
    design: &Design,
    pieces: &Pieces,
    k: usize,
    terminal: usize,
) -> Vec<TermPad> {
    let stack = design.rules.layers();
    let mut out = Vec::new();
    for &p in &pieces.pads[k] {
        let pad = &board.pads[p];
        if let Some(center) = pad_center(&pad.shape) {
            out.push(TermPad {
                center,
                layers: LayerSet(pad.layers.0 & stack.0),
                bbox: pad.shape.bbox(),
                terminal,
            });
        }
    }
    for &(at, layers) in &pieces.points[k] {
        if layers != LayerSet::EMPTY {
            out.push(TermPad {
                center: at,
                layers,
                bbox: Rect::new(at, at),
                terminal,
            });
        }
    }
    out
}

/// A net's track width on each layer, and its via's copper and hole, if it has vias.
type Parts = (Vec<i64>, Option<(i64, i64)>);

/// The track widths and via of a net, as routing gives them.
fn net_ctx_parts(design: &Design, net: NetId) -> Option<Parts> {
    let spec = design.nets.get(net.index())?;
    if !spec.route {
        return None;
    }
    let rules = &design.rules;
    let widths = (0..rules.layer_count())
        .map(|l| {
            spec.track_width
                .max(rules.track_width(Some(net), LayerId(l as u8)))
        })
        .collect();
    let via = spec
        .via
        .filter(|_| rules.layer_count() > 1)
        .map(|v| (v.diameter, v.drill));
    Some((widths, via))
}

/// Places in `targets`, the pieces of fill to reach, near each of the first few of `from`,
/// where a stub from it can end: a via, or a track's end on a layer of its own. As
/// terminal `terminal`, nearest first.
pub(crate) fn sites(
    world: &World<'_>,
    design: &Design,
    net: NetId,
    from: &[TermPad],
    targets: &[&Piece],
    pitch: i64,
    terminal: usize,
) -> Vec<TermPad> {
    let Some((widths, via)) = net_ctx_parts(design, net) else {
        return Vec::new();
    };
    let n = STUB_REACH / pitch.max(1);
    let mut offsets: Vec<(i64, i64, i64)> = Vec::new();
    for i in -n..=n {
        for j in -n..=n {
            let d = i * i + j * j;
            if d <= n * n {
                offsets.push((d, i, j));
            }
        }
    }
    offsets.sort_unstable();
    let mut sites: Vec<TermPad> = Vec::new();
    for source in from.iter().take(STUB_SOURCES) {
        let (center, layers) = (source.center, source.layers);
        let mut found = 0;
        for &(_, i, j) in &offsets {
            let q = Point::new(center.x + i * pitch, center.y + j * pitch);
            let r = Rect::new(q, q);
            let Some(t) = targets
                .iter()
                .find(|t| t.bbox.intersects(&r) && t.area.contains_point(q))
            else {
                continue;
            };
            if sites.iter().any(|s| s.center == q) {
                continue;
            }
            let ok = if layers.contains(t.layer) {
                Shape::circle(q, widths[usize::from(t.layer.0)])
                    .is_ok_and(|end| world.track(net, t.layer, &end, None) == Verdict::Free)
            } else {
                via.is_some_and(|(d, drill)| world.via(net, q, d, drill, None) == Verdict::Free)
            };
            if ok {
                sites.push(TermPad {
                    center: q,
                    layers: LayerSet::EMPTY.with(t.layer),
                    bbox: r,
                    terminal,
                });
                found += 1;
                if found >= STUB_SITES {
                    break;
                }
            }
        }
    }
    sites
}

/// A stub from `from`, the places where a piece of `net` may be left from, to one of
/// `targets`, the pieces of fill to reach, if one is found: its tracks and vias, placed in
/// the world.
fn stub(
    world: &mut World<'_>,
    design: &Design,
    net: NetId,
    from: &[TermPad],
    targets: &[&Piece],
    pitch: i64,
    via_cost: f64,
) -> Option<Stub> {
    let sites = sites(world, design, net, from, targets, pitch, 1);
    if sites.is_empty() {
        return None;
    }
    let bounds = from
        .iter()
        .take(STUB_SOURCES)
        .map(|p| p.bbox)
        .reduce(|a, b| a.union(&b))?
        .expand(STUB_REACH + 2 * pitch);
    let mut pads: Vec<TermPad> = from
        .iter()
        .map(|p| TermPad {
            terminal: 0,
            ..p.clone()
        })
        .collect();
    pads.extend(sites);
    search(
        world,
        design,
        net,
        pads,
        bounds,
        STUB_BUDGET,
        pitch,
        via_cost,
    )
}

/// Routes `pads`' two terminals together inside `bounds`, and places the copper.
#[allow(clippy::too_many_arguments)]
fn search(
    world: &mut World<'_>,
    design: &Design,
    net: NetId,
    pads: Vec<TermPad>,
    bounds: Rect,
    budget: u64,
    pitch: i64,
    via_cost: f64,
) -> Option<Stub> {
    let (widths, via) = net_ctx_parts(design, net)?;
    let ctx = NetCtx::new(net, widths, via, pads, 2, LATTICE_REACH * pitch);
    let params = Params {
        pitch,
        via_cost,
        layer_bias: 0.0,
        bounds,
        max_expansions: budget,
        under_pads: false,
        precheck: 0,
        partial: false,
        stubs: false,
    };
    let tree = detail::route_net(world, &ctx, &StepCache::default(), params, None, None).ok()?;
    let mut copper = NetCopper::from_tree(&ctx, &tree);
    copper.place(world, &ctx);
    let last = LayerId((design.rules.layer_count() - 1) as u8);
    let net = Some(ctx.net);
    Some(Stub {
        tracks: copper
            .segs
            .iter()
            .map(|s| Track {
                start: s.a,
                end: s.b,
                mid: None,
                width: ctx.widths[usize::from(s.l)],
                layer: LayerId(s.l),
                net,
            })
            .collect(),
        vias: ctx
            .via
            .map(|(diameter, drill)| {
                copper
                    .vias
                    .iter()
                    .map(|&at| Via {
                        at,
                        drill,
                        diameter,
                        from: LayerId(0),
                        to: last,
                        net,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Joins the pieces of a net's copper to its main piece: by a stub from any of a piece's
/// pads, tracks and vias, or with `tracks`, failing that, by tracks to the main piece's.
#[allow(clippy::too_many_arguments)]
pub(crate) fn connect(
    design: &Design,
    world: &mut World<'_>,
    board: &Board,
    net: NetId,
    fills: &[Piece],
    pitch: i64,
    via_cost: f64,
    tracks: bool,
) -> Vec<Stub> {
    let groups = pieces(board, design, net, fills);
    let Some(main) = groups.main(fills) else {
        return Vec::new();
    };
    let targets: Vec<&Piece> = groups.fills[main].iter().map(|&f| &fills[f]).collect();
    let mut out = Vec::new();
    for k in 0..groups.pads.len() {
        if k == main || groups.pads[k].is_empty() {
            continue;
        }
        let from = sources(board, design, &groups, k, 0);
        if let Some(s) = stub(world, design, net, &from, &targets, pitch, via_cost) {
            out.push(s);
            continue;
        }
        let to = sources(board, design, &groups, main, 1);
        if !tracks || to.is_empty() {
            continue;
        }
        let mut pads = from;
        pads.extend(to);
        let Some(bounds) = design
            .outline
            .iter()
            .map(Shape::bbox)
            .chain(pads.iter().map(|p| p.bbox.expand(STUB_REACH)))
            .reduce(|a, b| a.union(&b))
        else {
            continue;
        };
        if let Some(s) = search(
            world,
            design,
            net,
            pads,
            bounds,
            JOIN_BUDGET,
            pitch,
            via_cost,
        ) {
            out.push(s);
        }
    }
    out
}
