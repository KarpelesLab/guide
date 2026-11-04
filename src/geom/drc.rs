//! A design rule check over tracks, vias, pads and keepouts.
//!
//! This is the independent check that PCB routing will be judged by, so it shares no code
//! with any router: only the geometry, the index and the rules.

use std::collections::BTreeMap;
use std::fmt;

use super::clearance::Gap;
use super::index::SpatialIndex;
use super::rules::{LayerId, LayerSet, Rules};
use super::shape::{DEFAULT_ARC_TOLERANCE, Shape};
use super::types::{GeomError, MAX_COORD, Point};
use crate::id::NetId;

/// A track: a straight segment, or a circular arc through `mid`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// Where it starts.
    pub start: Point,
    /// Where it ends.
    pub end: Point,
    /// A point between the two on the arc, for an arc track.
    pub mid: Option<Point>,
    /// The width.
    pub width: i64,
    /// The layer.
    pub layer: LayerId,
    /// The net, or `None` for copper connected to nothing.
    pub net: Option<NetId>,
}

/// A via: a drilled, plated hole with round copper on a span of layers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Via {
    /// The centre.
    pub at: Point,
    /// The diameter of the hole.
    pub drill: i64,
    /// The diameter of the copper.
    pub diameter: i64,
    /// The top layer.
    pub from: LayerId,
    /// The bottom layer, `from` itself or below it.
    pub to: LayerId,
    /// The net, or `None`.
    pub net: Option<NetId>,
}

/// A pad: copper of any shape, the same on each of its layers, maybe with a hole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pad {
    /// The copper.
    pub shape: Shape,
    /// The layers it has copper on.
    pub layers: LayerSet,
    /// The hole, round or a slot, if it is drilled.
    pub hole: Option<Shape>,
    /// The net, or `None`.
    pub net: Option<NetId>,
}

/// An area that some kinds of copper must stay out of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keepout {
    /// The area.
    pub shape: Shape,
    /// The layers it applies to.
    pub layers: LayerSet,
    /// Whether tracks must stay out.
    pub tracks: bool,
    /// Whether vias must stay out.
    pub vias: bool,
    /// Whether pads must stay out.
    pub pads: bool,
}

/// Everything a check looks at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    /// The tracks.
    pub tracks: Vec<Track>,
    /// The vias.
    pub vias: Vec<Via>,
    /// The pads.
    pub pads: Vec<Pad>,
    /// The keepouts.
    pub keepouts: Vec<Keepout>,
    /// The tolerance for approximating arc tracks: see [`Shape::arc_track`].
    pub arc_tolerance: i64,
    /// How far a gap may fall short of a clearance, copper's or hole-to-hole, and still
    /// pass. Zero, the default, makes every check exact; [`KICAD_CLEARANCE_SLACK`] is
    /// what KiCad allows. Touching copper is a short whatever this is.
    pub clearance_slack: i64,
}

/// The shortfall KiCad's DRC lets a clearance have, in nanometres: a gap 500 nm short of
/// it passes, and 501 nm short is reported.
///
/// Measured with `kicad-cli pcb drc` from KiCad 10.0.3, on two tracks and on a track and a
/// via at a 0.2 mm clearance, side by side and at 45°. cadlab measured the same with KiCad
/// 10.0.7. KiCad doesn't document it, so it may change.
pub const KICAD_CLEARANCE_SLACK: i64 = 500;

impl Default for Board {
    fn default() -> Board {
        Board {
            tracks: Vec::new(),
            vias: Vec::new(),
            pads: Vec::new(),
            keepouts: Vec::new(),
            arc_tolerance: DEFAULT_ARC_TOLERANCE,
            clearance_slack: 0,
        }
    }
}

/// Something on the board, by its kind and its position in the board's list of them.
///
/// Items order by kind in the order of the variants, then by position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Item {
    /// `Board::tracks[i]`.
    Track(usize),
    /// `Board::vias[i]`.
    Via(usize),
    /// `Board::pads[i]`.
    Pad(usize),
    /// `Board::keepouts[i]`.
    Keepout(usize),
}

/// A broken rule.
///
/// Where two items are involved, `a` is the lesser. Where a pair breaks a rule on several
/// layers, it is reported once, on the first layer from the top, and a short is reported
/// in place of a clearance violation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Violation {
    /// Copper of two different nets touches or overlaps.
    Short {
        /// One item.
        a: Item,
        /// The other.
        b: Item,
        /// Where.
        layer: LayerId,
    },
    /// Copper of two different nets is closer than the clearance between them.
    Clearance {
        /// One item.
        a: Item,
        /// The other.
        b: Item,
        /// Where.
        layer: LayerId,
        /// The clearance.
        required: i64,
        /// The gap, or a lower bound on it if an arc is involved.
        gap: Gap,
    },
    /// A track is narrower than its rules allow.
    TrackWidth {
        /// The track's position in `Board::tracks`.
        track: usize,
        /// The width it should have at least.
        required: i64,
    },
    /// A via's hole is smaller than the minimum drill.
    Drill {
        /// The via's position in `Board::vias`.
        via: usize,
        /// The minimum drill.
        required: i64,
    },
    /// A via's ring of copper around the hole is narrower than allowed.
    AnnularRing {
        /// The via's position in `Board::vias`.
        via: usize,
        /// The narrowest ring allowed.
        required: i64,
    },
    /// Two holes are closer than the hole-to-hole clearance, whatever their nets.
    HoleToHole {
        /// One item with a hole.
        a: Item,
        /// The other.
        b: Item,
        /// The clearance.
        required: i64,
        /// The gap between the holes.
        gap: Gap,
    },
    /// An item touches or enters a keepout it should stay out of.
    Keepout {
        /// The keepout's position in `Board::keepouts`.
        keepout: usize,
        /// The item.
        item: Item,
        /// Where.
        layer: LayerId,
    },
}

impl Violation {
    /// What orders violations: the kind, in the order of the variants, then the items.
    pub(super) fn key(&self) -> (u8, Item, Option<Item>) {
        match *self {
            Violation::Short { a, b, .. } => (0, a, Some(b)),
            Violation::Clearance { a, b, .. } => (1, a, Some(b)),
            Violation::TrackWidth { track, .. } => (2, Item::Track(track), None),
            Violation::Drill { via, .. } => (3, Item::Via(via), None),
            Violation::AnnularRing { via, .. } => (4, Item::Via(via), None),
            Violation::HoleToHole { a, b, .. } => (5, a, Some(b)),
            Violation::Keepout { keepout, item, .. } => (6, Item::Keepout(keepout), Some(item)),
        }
    }
}

/// Why a board could not be checked.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DrcError {
    /// An item's geometry is invalid.
    Geometry {
        /// The item.
        item: Item,
        /// What is wrong.
        error: GeomError,
    },
    /// An item is on a layer that the stack doesn't have, or a via's span runs upward.
    Layer {
        /// The item.
        item: Item,
    },
    /// The board's `clearance_slack` is negative or beyond
    /// [`MAX_COORD`].
    Slack(i64),
}

impl fmt::Display for DrcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DrcError::Geometry { item, error } => write!(f, "{item:?}: {error}"),
            DrcError::Layer { item } => write!(f, "{item:?} is on layers not in the stack"),
            DrcError::Slack(slack) => write!(f, "invalid clearance slack: {slack}"),
        }
    }
}

impl std::error::Error for DrcError {}

/// Copper on the board: its shape, layers and net.
struct Copper {
    item: Item,
    shape: Shape,
    layers: LayerSet,
    net: Option<NetId>,
}

/// Whether two items are of one net, and so can't violate a clearance between them. Copper
/// with no net is connected to nothing, not even other copper with no net.
fn same_net(a: Option<NetId>, b: Option<NetId>) -> bool {
    a.is_some() && a == b
}

/// Checks a board against the rules, and returns every violation, sorted by
/// [kind and items](Violation).
///
/// ```
/// use guide::geom::drc::{self, Board, Item, Track, Violation};
/// use guide::geom::{LayerId, Point, RuleSpec, Rules};
/// use guide::id::NetId;
///
/// let rules = Rules::new(RuleSpec::new(&["F.Cu", "B.Cu"], 200, 200))?;
/// let track = |y, net| Track {
///     start: Point::new(0, y),
///     end: Point::new(10_000, y),
///     mid: None,
///     width: 200,
///     layer: LayerId(0),
///     net: Some(NetId(net)),
/// };
/// let mut board = Board::default();
/// // 400 apart, less half of each width, leaves exactly the clearance.
/// board.tracks = vec![track(0, 1), track(400, 2)];
/// assert!(drc::check(&board, &rules)?.is_empty());
///
/// board.tracks[1].start.y = 399;
/// let found = drc::check(&board, &rules)?;
/// assert!(matches!(
///     found[..],
///     [Violation::Clearance { a: Item::Track(0), b: Item::Track(1), required: 200, .. }]
/// ));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn check(board: &Board, rules: &Rules) -> Result<Vec<Violation>, DrcError> {
    let stack = rules.layers();
    let slack = board.clearance_slack;
    if !(0..=MAX_COORD).contains(&slack) {
        return Err(DrcError::Slack(slack));
    }
    let copper = copper(board, stack)?;
    let mut out = Vec::new();
    single_items(board, rules, &mut out);
    let by_layer = layer_indexes(&copper, rules.layer_count());
    copper_pairs(&copper, &by_layer, rules, slack, &mut out)?;
    holes(board, rules, slack, &mut out)?;
    keepouts(board, &copper, &by_layer, stack, &mut out);
    // Stable, so that equal keys keep the order they were found in, which is fixed too.
    out.sort_by_key(Violation::key);
    Ok(out)
}

fn copper(board: &Board, stack: LayerSet) -> Result<Vec<Copper>, DrcError> {
    let mut copper = Vec::new();
    for (i, t) in board.tracks.iter().enumerate() {
        let item = Item::Track(i);
        let shape = match t.mid {
            None => Shape::track(t.start, t.end, t.width),
            Some(mid) => Shape::arc_track(t.start, mid, t.end, t.width, board.arc_tolerance),
        }
        .map_err(geometry_error(item))?;
        let layers = LayerSet::EMPTY.with(t.layer);
        if !stack.contains(t.layer) {
            return Err(DrcError::Layer { item });
        }
        copper.push(Copper {
            item,
            shape,
            layers,
            net: t.net,
        });
    }
    for (i, v) in board.vias.iter().enumerate() {
        let item = Item::Via(i);
        let shape = Shape::circle(v.at, v.diameter).map_err(geometry_error(item))?;
        // The hole is only measured if there is a hole-to-hole rule, but its size is
        // compared with the copper's in any case, so it has to be in range.
        Shape::circle(v.at, v.drill).map_err(geometry_error(item))?;
        if v.from > v.to || !stack.contains(v.to) {
            return Err(DrcError::Layer { item });
        }
        copper.push(Copper {
            item,
            shape,
            layers: LayerSet::span(v.from, v.to),
            net: v.net,
        });
    }
    for (i, p) in board.pads.iter().enumerate() {
        let item = Item::Pad(i);
        if !p.layers.is_subset(stack) {
            return Err(DrcError::Layer { item });
        }
        copper.push(Copper {
            item,
            shape: p.shape.clone(),
            layers: p.layers,
            net: p.net,
        });
    }
    for (i, k) in board.keepouts.iter().enumerate() {
        if !k.layers.is_subset(stack) {
            return Err(DrcError::Layer {
                item: Item::Keepout(i),
            });
        }
    }
    Ok(copper)
}

/// Rules about one item at a time.
fn single_items(board: &Board, rules: &Rules, out: &mut Vec<Violation>) {
    for (i, t) in board.tracks.iter().enumerate() {
        let required = rules.track_width(t.net, t.layer);
        if t.width < required {
            out.push(Violation::TrackWidth { track: i, required });
        }
    }
    let spec = rules.spec();
    for (i, v) in board.vias.iter().enumerate() {
        if v.drill < spec.min_drill {
            out.push(Violation::Drill {
                via: i,
                required: spec.min_drill,
            });
        }
        // The ring is `(diameter - drill) / 2`, compared doubled so as to stay in integers.
        // Both are within 2^40, so none of this overflows.
        if v.diameter - v.drill < 2 * spec.min_annular_ring || v.diameter <= v.drill {
            out.push(Violation::AnnularRing {
                via: i,
                required: spec.min_annular_ring,
            });
        }
    }
}

/// For each layer, the positions in `copper` of what is on it, and an index of their boxes
/// with the `k`th position in slot `k`.
fn layer_indexes(copper: &[Copper], layers: usize) -> Vec<(Vec<usize>, SpatialIndex)> {
    (0..layers)
        .map(|l| {
            let on: Vec<usize> = (0..copper.len())
                .filter(|&i| copper[i].layers.contains(LayerId(l as u8)))
                .collect();
            let index = SpatialIndex::bulk_load(on.iter().map(|&i| copper[i].shape.bbox()));
            (on, index)
        })
        .collect()
}

fn geometry_error(item: Item) -> impl FnOnce(GeomError) -> DrcError {
    move |error| DrcError::Geometry { item, error }
}

/// Clearances and shorts between copper of different nets.
fn copper_pairs(
    copper: &[Copper],
    by_layer: &[(Vec<usize>, SpatialIndex)],
    rules: &Rules,
    slack: i64,
    out: &mut Vec<Violation>,
) -> Result<(), DrcError> {
    // By pair, the first finding, which a short on a later layer replaces.
    let mut found: BTreeMap<(usize, usize), Violation> = BTreeMap::new();
    let reach = rules.max_clearance();
    let mut near = Vec::new();
    for (l, (on, index)) in by_layer.iter().enumerate() {
        let layer = LayerId(l as u8);
        for (k, &i) in on.iter().enumerate() {
            let a = &copper[i];
            index.query(&a.shape.bbox().expand(reach), &mut near);
            for id in &near {
                let j = on[id.slot() as usize];
                // Each pair once, from the lesser side. `copper` is in item order.
                if (id.slot() as usize) <= k || same_net(a.net, copper[j].net) {
                    continue;
                }
                if matches!(found.get(&(i, j)), Some(Violation::Short { .. })) {
                    continue;
                }
                let b = &copper[j];
                let required = rules.clearance_between(a.net, b.net, layer);
                // What the gap has to reach, which is all that a short needs checking
                // under: touching copper is closer than any positive clearance.
                let limit = (required - slack).max(0);
                let violated = limit > 0
                    && a.shape
                        .violates(&b.shape, limit)
                        .map_err(geometry_error(a.item))?;
                if !violated && limit > 0 {
                    continue;
                }
                if a.shape.touches(&b.shape) {
                    found.insert(
                        (i, j),
                        Violation::Short {
                            a: a.item,
                            b: b.item,
                            layer,
                        },
                    );
                } else if violated && !found.contains_key(&(i, j)) {
                    found.insert(
                        (i, j),
                        Violation::Clearance {
                            a: a.item,
                            b: b.item,
                            layer,
                            required,
                            gap: a.shape.distance(&b.shape),
                        },
                    );
                }
            }
        }
    }
    out.extend(found.into_values());
    Ok(())
}

/// Holes closer to each other than the hole-to-hole clearance.
fn holes(
    board: &Board,
    rules: &Rules,
    slack: i64,
    out: &mut Vec<Violation>,
) -> Result<(), DrcError> {
    let required = rules.spec().hole_to_hole;
    let limit = required - slack;
    if limit <= 0 {
        return Ok(());
    }
    let mut holes: Vec<(Item, Shape)> = Vec::new();
    for (i, v) in board.vias.iter().enumerate() {
        let item = Item::Via(i);
        holes.push((
            item,
            Shape::circle(v.at, v.drill).map_err(geometry_error(item))?,
        ));
    }
    for (i, p) in board.pads.iter().enumerate() {
        if let Some(hole) = &p.hole {
            holes.push((Item::Pad(i), hole.clone()));
        }
    }
    let index = SpatialIndex::bulk_load(holes.iter().map(|(_, s)| s.bbox()));
    let mut near = Vec::new();
    for (i, (a, hole)) in holes.iter().enumerate() {
        index.query(&hole.bbox().expand(limit), &mut near);
        for id in &near {
            let j = id.slot() as usize;
            let (b, other) = &holes[j];
            if j > i && hole.violates(other, limit).map_err(geometry_error(*a))? {
                out.push(Violation::HoleToHole {
                    a: *a,
                    b: *b,
                    required,
                    gap: hole.distance(other),
                });
            }
        }
    }
    Ok(())
}

/// Items in keepouts that forbid them.
fn keepouts(
    board: &Board,
    copper: &[Copper],
    by_layer: &[(Vec<usize>, SpatialIndex)],
    stack: LayerSet,
    out: &mut Vec<Violation>,
) {
    let mut near = Vec::new();
    for (k, keepout) in board.keepouts.iter().enumerate() {
        // By item, the first layer it is found on.
        let mut found: BTreeMap<Item, LayerId> = BTreeMap::new();
        for layer in keepout.layers.iter().filter(|&l| stack.contains(l)) {
            let (on, index) = &by_layer[usize::from(layer.0)];
            index.query(&keepout.shape.bbox(), &mut near);
            for id in &near {
                let c = &copper[on[id.slot() as usize]];
                let forbidden = match c.item {
                    Item::Track(_) => keepout.tracks,
                    Item::Via(_) => keepout.vias,
                    Item::Pad(_) => keepout.pads,
                    Item::Keepout(_) => false,
                };
                if forbidden && !found.contains_key(&c.item) && c.shape.touches(&keepout.shape) {
                    found.insert(c.item, layer);
                }
            }
        }
        out.extend(found.into_iter().map(|(item, layer)| Violation::Keepout {
            keepout: k,
            item,
            layer,
        }));
    }
}
