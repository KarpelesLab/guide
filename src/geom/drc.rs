//! A design rule check over tracks, vias, pads, zones and keepouts.
//!
//! This is the independent check that PCB routing will be judged by, so it shares no code
//! with any router: only the geometry, the index and the rules.

use std::collections::{BTreeMap, BTreeSet};
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

/// A pad: copper of any shape, the same on each of its layers, maybe with a hole, and
/// maybe with other shapes on other layers, a pad stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pad {
    /// The copper.
    pub shape: Shape,
    /// The layers it has copper on.
    pub layers: LayerSet,
    /// The hole, round or a slot, if it is drilled. A hole goes through every layer of
    /// the stack, whichever layers the pad has copper on.
    pub hole: Option<Shape>,
    /// The net, or `None`.
    pub net: Option<NetId>,
    /// A clearance of the pad's own, which replaces the rules' between it and anything
    /// else. See [`clearance`] for how it combines with the other item's.
    pub clearance: Option<i64>,
    /// Its copper where it is not [`shape`](Pad::shape): other shapes, each on some layers,
    /// as the smaller ring of a through-hole pad on inner layers. Empty for a pad that is
    /// the same on all its layers. All of it is one pad, of one net and one clearance: one
    /// [item](Item::Pad) for the check, which reports it once whichever of its shapes breaks
    /// a rule, and one piece of copper, joined from layer to layer through its hole, for
    /// connectivity and routing, whose routes end at the centre of [`shape`](Pad::shape) on
    /// the layers of every shape that holds that point.
    pub stack: Vec<(LayerSet, Shape)>,
}

impl Pad {
    /// A pad of one shape on its layers, with no hole and no clearance of its own.
    pub fn new(shape: Shape, layers: LayerSet, net: Option<NetId>) -> Pad {
        Pad {
            shape,
            layers,
            hole: None,
            net,
            clearance: None,
            stack: Vec::new(),
        }
    }

    /// Every layer it has copper on: those of its [`shape`](Pad::shape) and of its
    /// [`stack`](Pad::stack).
    pub fn copper_layers(&self) -> LayerSet {
        self.stack
            .iter()
            .fold(self.layers, |all, (layers, _)| LayerSet(all.0 | layers.0))
    }

    /// Its copper, shape by shape, each with its layers: [`shape`](Pad::shape) first.
    pub fn parts(&self) -> impl Iterator<Item = (LayerSet, &Shape)> {
        core::iter::once((self.layers, &self.shape))
            .chain(self.stack.iter().map(|(layers, shape)| (*layers, shape)))
    }
}

/// Copper filling an area of one layer, such as a piece of a zone's fill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zone {
    /// The area.
    pub shape: Shape,
    /// The layer.
    pub layer: LayerId,
    /// The net, or `None`.
    pub net: Option<NetId>,
    /// A clearance of the zone's own, as for [`Pad::clearance`].
    pub clearance: Option<i64>,
}

/// An area that some kinds of copper must stay out of. Zones are not checked against it.
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
    /// The nets whose copper may be in it all the same, such as the nets of the part that
    /// the keepout is drawn under.
    pub except: Vec<NetId>,
}

/// An opening in the solder mask over an outer copper layer, which leaves the copper in it
/// bare. Copper of two nets bare in one opening is a solder mask bridge, as KiCad reports it:
/// solder could join them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskOpening {
    /// The shape, before [`margin`](MaskOpening::margin).
    pub shape: Shape,
    /// The outer copper layer it is over.
    pub layer: LayerId,
    /// How far the opening reaches beyond the shape, as a pad's mask expansion does. A
    /// negative margin is taken as zero.
    pub margin: i64,
    /// The net of the copper it is for, such as its pad's, or `None` if it is for none, or
    /// already has copper of several nets in it.
    pub net: Option<NetId>,
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
    /// The zones.
    pub zones: Vec<Zone>,
    /// The keepouts.
    pub keepouts: Vec<Keepout>,
    /// The openings in the solder mask.
    pub masks: Vec<MaskOpening>,
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
            zones: Vec::new(),
            keepouts: Vec::new(),
            masks: Vec::new(),
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
    /// `Board::zones[i]`.
    Zone(usize),
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
    /// Copper is closer to a hole than the hole clearance, and is not of the hole's net, or
    /// with no hole clearance, a track or a via overlaps the hole, as KiCad reports too.
    /// A hole with no net, such as a mounting hole's, is of no net at all. A pad's hole is
    /// not reported against copper that its pad is already reported against, by a
    /// [short](Violation::Short) or a [clearance](Violation::Clearance), as KiCad does.
    HoleClearance {
        /// The via or pad with the hole.
        hole: Item,
        /// The copper.
        item: Item,
        /// Where, the first layer from the top if there are several.
        layer: LayerId,
        /// The clearance.
        required: i64,
        /// The gap between the hole and the copper.
        gap: Gap,
    },
    /// An item touches or enters a keepout it should stay out of, and its net is not one
    /// that the keepout [excepts](Keepout::except).
    Keepout {
        /// The keepout's position in `Board::keepouts`.
        keepout: usize,
        /// The item.
        item: Item,
        /// Where.
        layer: LayerId,
    },
    /// Copper of a net other than an opening's in the solder mask is bare in it: it touches
    /// the opening, grown by its margin, on its layer. Copper of no net is of another net
    /// than an opening's. An opening with no net, such as one drawn on the mask, bridges
    /// what is in it if that is copper of a net and copper of another net or of none, and
    /// then each item with a net in it is reported, as KiCad does. A pad's hole where the
    /// pad has no copper, as a plain mounting hole's, counts as copper of the pad's net.
    MaskBridge {
        /// The opening's position in `Board::masks`.
        opening: usize,
        /// The copper.
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
            Violation::HoleClearance { hole, item, .. } => (6, hole, Some(item)),
            Violation::Keepout { keepout, item, .. } => (7, Item::Keepout(keepout), Some(item)),
            Violation::MaskBridge { opening, item, .. } => (8, Item::Keepout(opening), Some(item)),
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
    /// An item's own clearance is negative or beyond [`MAX_COORD`].
    Clearance {
        /// The item.
        item: Item,
        /// The clearance.
        value: i64,
    },
}

impl fmt::Display for DrcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DrcError::Geometry { item, error } => write!(f, "{item:?}: {error}"),
            DrcError::Layer { item } => write!(f, "{item:?} is on layers not in the stack"),
            DrcError::Slack(slack) => write!(f, "invalid clearance slack: {slack}"),
            DrcError::Clearance { item, value } => {
                write!(f, "{item:?} has an invalid clearance of its own: {value}")
            }
        }
    }
}

impl std::error::Error for DrcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DrcError::Geometry { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// Copper on the board: its shape, layers, net and own clearance.
struct Copper {
    item: Item,
    shape: Shape,
    layers: LayerSet,
    net: Option<NetId>,
    own: Option<i64>,
}

/// The clearance between two pieces of copper of different nets on `layer`, given their
/// nets and their own clearances, if they have any.
///
/// If neither has one of its own, this is [`Rules::clearance_between`] the nets. If either
/// has, it is the larger of their own clearances, and at least
/// [`min_clearance`](super::RuleSpec::min_clearance), whatever the nets' rules say: a
/// pad's own clearance can be smaller than its class's. This is how KiCad treats the
/// clearances that pads, footprints and zones override.
///
/// Hole clearances work the same way, with
/// [`hole_clearance`](super::RuleSpec::hole_clearance) for both the rule and the floor.
///
/// ```
/// use guide::geom::drc::clearance;
/// use guide::geom::{LayerId, RuleSpec, Rules};
/// use guide::id::NetId;
///
/// let mut spec = RuleSpec::new(&["F.Cu"], 100, 200);
/// spec.min_clearance = 150;
/// let rules = Rules::new(spec)?;
/// let (a, b, top) = (Some(NetId(1)), Some(NetId(2)), LayerId(0));
/// assert_eq!(clearance(&rules, (a, None), (b, None), top), 200);
/// assert_eq!(clearance(&rules, (a, Some(100)), (b, None), top), 150);
/// assert_eq!(clearance(&rules, (a, Some(100)), (b, Some(300)), top), 300);
/// # Ok::<(), guide::geom::RuleError>(())
/// ```
pub fn clearance(
    rules: &Rules,
    a: (Option<NetId>, Option<i64>),
    b: (Option<NetId>, Option<i64>),
    layer: LayerId,
) -> i64 {
    match (a.1, b.1) {
        (None, None) => rules.clearance_between(a.0, b.0, layer),
        (x, y) => x
            .unwrap_or(0)
            .max(y.unwrap_or(0))
            .max(rules.spec().min_clearance),
    }
}

/// The hole clearance between a hole and copper, given their own clearances: see
/// [`clearance`].
fn hole_clearance_between(rules: &Rules, a: Option<i64>, b: Option<i64>) -> i64 {
    let floor = rules.spec().hole_clearance;
    match (a, b) {
        (None, None) => floor,
        (x, y) => x.unwrap_or(0).max(y.unwrap_or(0)).max(floor),
    }
}

/// Whether two items are of one net, and so can't violate a clearance between them. Copper
/// with no net is connected to nothing, not even other copper with no net.
fn same_net(a: Option<NetId>, b: Option<NetId>) -> bool {
    a.is_some() && a == b
}

/// Checks a board against the rules, and returns every violation, sorted by
/// [kind and items](Violation).
///
/// # Errors
///
/// [`DrcError`] for an item whose geometry is invalid, that is on a layer the rules' stack
/// doesn't have, or whose own clearance is out of range, or for a board whose
/// `clearance_slack` is: violations are what is found on a board that can be checked.
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
    let close = copper_pairs(&copper, &by_layer, rules, slack, &mut out)?;
    let holes = holes(board, stack)?;
    hole_to_hole(&holes, rules, slack, &mut out)?;
    hole_clearances(&holes, &copper, &by_layer, rules, slack, &close, &mut out)?;
    keepouts(board, &copper, &by_layer, stack, &mut out);
    mask_bridges(board, &copper, &by_layer, &mut out)?;
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
            own: None,
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
            own: None,
        });
    }
    for (i, p) in board.pads.iter().enumerate() {
        let item = Item::Pad(i);
        if !p.copper_layers().is_subset(stack) {
            return Err(DrcError::Layer { item });
        }
        for (layers, shape) in p.parts() {
            copper.push(Copper {
                item,
                shape: shape.clone(),
                layers,
                net: p.net,
                own: own_clearance(item, p.clearance)?,
            });
        }
    }
    for (i, z) in board.zones.iter().enumerate() {
        let item = Item::Zone(i);
        if !stack.contains(z.layer) {
            return Err(DrcError::Layer { item });
        }
        copper.push(Copper {
            item,
            shape: z.shape.clone(),
            layers: LayerSet::EMPTY.with(z.layer),
            net: z.net,
            own: own_clearance(item, z.clearance)?,
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

fn own_clearance(item: Item, value: Option<i64>) -> Result<Option<i64>, DrcError> {
    match value {
        Some(v) if !(0..=MAX_COORD).contains(&v) => Err(DrcError::Clearance { item, value: v }),
        _ => Ok(value),
    }
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
) -> Result<BTreeSet<(Item, Item)>, DrcError> {
    // By pair, the first finding, which a short on a later layer replaces.
    let mut found: BTreeMap<(usize, usize), Violation> = BTreeMap::new();
    // By position, the first position of its item, which a pad's shapes share.
    let mut first: Vec<usize> = (0..copper.len()).collect();
    for i in 1..copper.len() {
        if copper[i].item == copper[i - 1].item {
            first[i] = first[i - 1];
        }
    }
    let reach = copper
        .iter()
        .filter_map(|c| c.own)
        .fold(rules.max_clearance(), i64::max);
    let mut near = Vec::new();
    for (l, (on, index)) in by_layer.iter().enumerate() {
        let layer = LayerId(l as u8);
        for (k, &i) in on.iter().enumerate() {
            let a = &copper[i];
            index.query(&a.shape.bbox().expand(reach), &mut near);
            for id in &near {
                let j = on[id.slot() as usize];
                // Each pair once, from the lesser side. `copper` is in item order.
                if (id.slot() as usize) <= k
                    || same_net(a.net, copper[j].net)
                    || first[i] == first[j]
                {
                    continue;
                }
                let pair = (first[i], first[j]);
                if matches!(found.get(&pair), Some(Violation::Short { .. })) {
                    continue;
                }
                let b = &copper[j];
                let required = clearance(rules, (a.net, a.own), (b.net, b.own), layer);
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
                        pair,
                        Violation::Short {
                            a: a.item,
                            b: b.item,
                            layer,
                        },
                    );
                } else if violated && !found.contains_key(&pair) {
                    found.insert(
                        pair,
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
    let close = found
        .keys()
        .map(|&(i, j)| (copper[i].item, copper[j].item))
        .collect();
    out.extend(found.into_values());
    Ok(close)
}

/// A drilled hole: what has it, its shape, the layers it goes through, and the net and own
/// clearance of what has it.
struct Hole {
    item: Item,
    shape: Shape,
    layers: LayerSet,
    net: Option<NetId>,
    own: Option<i64>,
}

/// Every hole on the board, vias' first, then pads'.
fn holes(board: &Board, stack: LayerSet) -> Result<Vec<Hole>, DrcError> {
    let mut holes = Vec::new();
    for (i, v) in board.vias.iter().enumerate() {
        let item = Item::Via(i);
        holes.push(Hole {
            item,
            shape: Shape::circle(v.at, v.drill).map_err(geometry_error(item))?,
            layers: LayerSet::span(v.from, v.to),
            net: v.net,
            own: None,
        });
    }
    for (i, p) in board.pads.iter().enumerate() {
        if let Some(hole) = &p.hole {
            let item = Item::Pad(i);
            holes.push(Hole {
                item,
                shape: hole.clone(),
                layers: stack,
                net: p.net,
                own: own_clearance(item, p.clearance)?,
            });
        }
    }
    Ok(holes)
}

/// Holes closer to each other than the hole-to-hole clearance.
fn hole_to_hole(
    holes: &[Hole],
    rules: &Rules,
    slack: i64,
    out: &mut Vec<Violation>,
) -> Result<(), DrcError> {
    let required = rules.spec().hole_to_hole;
    let limit = required - slack;
    if limit <= 0 {
        return Ok(());
    }
    let index = SpatialIndex::bulk_load(holes.iter().map(|h| h.shape.bbox()));
    let mut near = Vec::new();
    for (i, a) in holes.iter().enumerate() {
        index.query(&a.shape.bbox().expand(limit), &mut near);
        for id in &near {
            let j = id.slot() as usize;
            let b = &holes[j];
            if j > i
                && a.shape
                    .violates(&b.shape, limit)
                    .map_err(geometry_error(a.item))?
            {
                out.push(Violation::HoleToHole {
                    a: a.item,
                    b: b.item,
                    required,
                    gap: a.shape.distance(&b.shape),
                });
            }
        }
    }
    Ok(())
}

/// Copper of other nets closer to holes than the hole clearance, leaving out pairs whose
/// copper is `close` already.
fn hole_clearances(
    holes: &[Hole],
    copper: &[Copper],
    by_layer: &[(Vec<usize>, SpatialIndex)],
    rules: &Rules,
    slack: i64,
    close: &BTreeSet<(Item, Item)>,
    out: &mut Vec<Violation>,
) -> Result<(), DrcError> {
    let reach = holes
        .iter()
        .filter_map(|h| h.own)
        .chain(copper.iter().filter_map(|c| c.own))
        .fold(rules.spec().hole_clearance, i64::max)
        .max(0);
    let mut near = Vec::new();
    for hole in holes {
        // By item, the first finding, from the top layer down.
        let mut found: BTreeMap<Item, Violation> = BTreeMap::new();
        for layer in hole.layers.iter() {
            let Some((on, index)) = by_layer.get(usize::from(layer.0)) else {
                continue;
            };
            index.query(&hole.shape.bbox().expand(reach), &mut near);
            for id in &near {
                let c = &copper[on[id.slot() as usize]];
                let pair = (hole.item.min(c.item), hole.item.max(c.item));
                if c.item == hole.item
                    || same_net(hole.net, c.net)
                    || found.contains_key(&c.item)
                    || close.contains(&pair)
                {
                    continue;
                }
                let required = hole_clearance_between(rules, hole.own, c.own);
                let limit = required - slack;
                // With no clearance left to keep, a track or a via still may not cover the
                // hole. KiCad reports that, and not a zone's fill over a hole.
                let violated = if limit > 0 {
                    hole.shape
                        .violates(&c.shape, limit)
                        .map_err(geometry_error(hole.item))?
                } else {
                    matches!(c.item, Item::Track(_) | Item::Via(_))
                        && hole.shape.touches(&c.shape)
                        && hole.shape.distance(&c.shape).is_overlap()
                };
                if violated {
                    found.insert(
                        c.item,
                        Violation::HoleClearance {
                            hole: hole.item,
                            item: c.item,
                            layer,
                            required,
                            gap: hole.shape.distance(&c.shape),
                        },
                    );
                }
            }
        }
        out.extend(found.into_values());
    }
    Ok(())
}

/// Whether copper is bare in an opening of the solder mask: it touches the opening, grown
/// by its margin.
fn bare_in(shape: &Shape, item: Item, opening: &Shape, margin: i64) -> Result<bool, DrcError> {
    Ok(shape.touches(opening)
        || (margin > 0
            && shape
                .violates(opening, margin)
                .map_err(geometry_error(item))?))
}

/// Copper bare in openings of the solder mask that are another net's.
fn mask_bridges(
    board: &Board,
    copper: &[Copper],
    by_layer: &[(Vec<usize>, SpatialIndex)],
    out: &mut Vec<Violation>,
) -> Result<(), DrcError> {
    // Pads' holes, which are bare in an opening where their pad has no copper, as a plain
    // hole of a mounting pad's is, as KiCad has it.
    let holed: Vec<(usize, &Shape)> = board
        .pads
        .iter()
        .enumerate()
        .filter_map(|(i, p)| Some((i, p.hole.as_ref()?)))
        .collect();
    let holes = SpatialIndex::bulk_load(holed.iter().map(|(_, h)| h.bbox()));
    let mut near = Vec::new();
    for (k, m) in board.masks.iter().enumerate() {
        let Some((on, index)) = by_layer.get(usize::from(m.layer.0)) else {
            continue;
        };
        let margin = m.margin.clamp(0, MAX_COORD);
        let reach = m.shape.bbox().expand(margin);
        index.query(&reach, &mut near);
        let mut bare: Vec<usize> = near.iter().map(|id| on[id.slot() as usize]).collect();
        // In item order, as `copper` is.
        bare.sort_unstable();
        // What is bare in it, by item and net.
        let mut inside: Vec<(Item, Option<NetId>)> = Vec::new();
        for i in bare {
            let c = &copper[i];
            if bare_in(&c.shape, c.item, &m.shape, margin)? && !inside.contains(&(c.item, c.net)) {
                inside.push((c.item, c.net));
            }
        }
        holes.query(&reach, &mut near);
        let mut found: Vec<usize> = near.iter().map(|id| id.slot() as usize).collect();
        found.sort_unstable();
        for h in found {
            let (i, hole) = holed[h];
            let pad = &board.pads[i];
            if !pad.copper_layers().contains(m.layer)
                && bare_in(hole, Item::Pad(i), &m.shape, margin)?
            {
                inside.push((Item::Pad(i), pad.net));
            }
        }
        let bridged: Vec<Item> = match m.net {
            Some(net) => inside
                .iter()
                .filter(|c| c.1 != Some(net))
                .map(|c| c.0)
                .collect(),
            // An opening of no net bridges what is in it, if it has copper of a net and of
            // another, or of none.
            None => {
                let with_net = inside.iter().filter(|c| c.1.is_some());
                let first = with_net.clone().next().and_then(|c| c.1);
                if first.is_some() && inside.iter().any(|c| c.1 != first) {
                    with_net.map(|c| c.0).collect()
                } else {
                    Vec::new()
                }
            }
        };
        out.extend(bridged.into_iter().map(|item| Violation::MaskBridge {
            opening: k,
            item,
            layer: m.layer,
        }));
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
                    Item::Zone(_) | Item::Keepout(_) => false,
                };
                let excepted = c.net.is_some_and(|n| keepout.except.contains(&n));
                if forbidden
                    && !excepted
                    && !found.contains_key(&c.item)
                    && c.shape.touches(&keepout.shape)
                {
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
