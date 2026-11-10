//! PCB routing: multi-layer, grid-free routing of a board's nets over the geometry layer,
//! with through vias, keepouts, fixed pre-routes and net classes. Enabled by the `pcb`
//! feature.
//!
//! [`route`] takes a [`Design`], the fixed items of a board with its rules and the nets
//! to route, and returns tracks, vias and zones' fills of the same
//! [`drc`](crate::geom::drc) model, so that [`drc::check`](crate::geom::drc::check) runs on
//! the result as on any board. With the `kicad` feature, [`Design::from_kicad`] takes a
//! board that [`kicad::read`](crate::kicad::read) has read. [`fill_zones`] fills a design's
//! copper zones on their own, as KiCad's zone filler does.
//!
//! # How it routes
//!
//! 0. **Escapes.** Pads closer together than a track and its clearances, such as a BGA's or
//!    an aQFN's, form clusters. A pad of a cluster that a track of its net can't get out
//!    of on its own layers is given a through via first, in the pad or beside it, placed on
//!    a fine grid so as to leave room for the others; one whose only way out is long, and
//!    that no via fits by, is given that way. Both become fixed copper of their nets, which
//!    the rest completes. A pad whose centre has no room for a track's end, as where text
//!    is drawn up to it, is reached at the point nearest its centre that has.
//!
//!    **Planes.** The [zones](Design::zones) are filled around the fixed copper, and each
//!    net with zones takes the pads that its largest fill joins, its fills on inner layers
//!    if it has any, as one terminal, which also holds places in that fill near each of the
//!    net's other pads where a via fits. So routing it is a stub from each other pad to the
//!    fill, a track and a via, rather than tracks across the board. Other nets' tracks pay
//!    more for their length inside another net's plane, [`Options::plane_cost`].
//! 1. **Global routing.** The board is cut into square tiles on each layer, and the
//!    negotiated-congestion [engine](crate::engine) routes every net over them. A resource
//!    is the side between two tiles on a layer, with as many tracks as fit along its free
//!    length as capacity, or a tile's vias. Each net's global route, grown by a margin of
//!    tiles, is its *corridor*.
//! 2. **Detailed routing**, one net at a time, shortest first, inside its corridor. Nodes
//!    are points on layers, generated as the search reaches them: from a node, a track can
//!    go a lattice pitch in any of the eight octilinear directions, a through via can
//!    change layers, and near a pad of the net a track can go straight to its centre.
//!    Every step is checked exactly against the fixed copper and the routes committed so
//!    far, by the same rules as the check, the board's edge and the solder mask's openings.
//!    So whatever is routed is legal by construction, and the check can only confirm it.
//! 3. **Rip-up.** A net that finds no path, in its corridor or then anywhere, searches
//!    again with other nets' routes passable at a price that grows with the history of
//!    conflicts where it crosses them. The nets it crosses are ripped up, it takes its
//!    path, and they route again. A net ripped up too often stays where it is. A net of
//!    more than 16 terminals, such as a supply or ground routed as tracks, that rip-up
//!    can't route whole is searched for a terminal at a time, nearest its first first, and
//!    keeps what it reaches rather than failing whole. Nets left unrouted at the end, or routed in part, get a second pass,
//!    on a lattice of half the pitch, in which a route may also go to a pad's centre on a
//!    layer the pad isn't on, for a via in it; a net routed in part is completed there from
//!    the copper it has.
//! 4. **Cleanup**: nets with vias are rerouted with dearer vias, kept only if they lose
//!    vias; then each run of tracks between pads, vias and branches is pulled tight, by
//!    replacing runs with straight or 45° shortcuts where those are legal.
//! 5. **Differential pairs**, [`Design::pairs`]: each pair's two routes are taken out, and
//!    the pair is routed side by side in their place, at its gap, where it fits: a search
//!    for its centreline, wide enough for both tracks, offset into the two tracks, with
//!    vias side by side and each net's own fan-out near its pads. It is kept if it connects
//!    all that the two routes did, so it can't cost another net its route. Pairs whose
//!    two nets' pads are far apart are left as they are. With [`Options::pairs_first`],
//!    pairs are routed this way before anything else instead, as fixed copper, with a
//!    place for a via kept free of them beside each pad of a net with zones next to their
//!    pads, such as a ground pin beside a pair's pins.
//! 6. **Meanders**, for [`Design::lengths`]: nets shorter than they must be get trombone
//!    meanders on their straight tracks, in free space, each checked exactly as routing
//!    is, and pairs side by side get meanders of both tracks together, which keep their
//!    gap. A net left short where its neighbours' meanders took the room gets it back:
//!    theirs are taken out, it is meandered first, and they again after it.
//! 7. **Zones** are filled again around everything routed, before meanders, and once more
//!    after them if any were added. Before meanders, a net with zones that its
//!    copper and fills leave in pieces, as routes of other nets cut its fills, has each
//!    piece joined to its largest by a stub, from any of the piece's pads, tracks or vias,
//!    or failing that by tracks, and the zones are filled again, a few times at most.
//!
//! `docs/architecture.md` has why it is built this way, with the measurements, and
//! `docs/benchmarks.md` has results on real boards.
//!
//! # Determinism
//!
//! Everything is in a fixed order: nets by length and identifier, steps by direction, and
//! the only hash maps are keyed lookups whose order never matters. Costs are sums of
//! lengths in a fixed order. No thread is used.

mod cleanup;
mod detail;
mod escape;
mod geometry;
mod global;
mod measure;
mod pairs;
mod planes;
mod router;
mod tune;
mod verify;
mod world;
mod zones;

use core::fmt;

use crate::geom::drc::{Board, Track, Via};
use crate::geom::{GeomError, LayerId, Rules, Shape};
use crate::id::NetId;

pub use measure::{coupling, net_length};
pub use verify::unconnected;
pub use zones::{CopperZone, Fill, Islands, PadConnection, ZoneConnection, Zones, fill_zones};

/// The size of a net's through vias.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViaSize {
    /// The diameter of the copper.
    pub diameter: i64,
    /// The diameter of the hole.
    pub drill: i64,
}

/// An opening in the solder mask over an outer layer, which copper of another net must not
/// reach into: KiCad reports an opening with two nets' copper in it as a solder mask
/// bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskOpening {
    /// The shape, before [`margin`](MaskOpening::margin).
    pub shape: Shape,
    /// The outer copper layer it is over.
    pub layer: LayerId,
    /// How far the opening reaches beyond the shape, as a pad's mask expansion does.
    pub margin: i64,
    /// The net of the copper in it, or `None` if there is none or it has several.
    pub net: Option<NetId>,
}

/// How one net is to be routed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetSpec {
    /// Whether to route it at all.
    pub route: bool,
    /// The width of its tracks.
    pub track_width: i64,
    /// Its vias, or `None` to keep it on the layers of its pads.
    pub via: Option<ViaSize>,
}

/// The two nets of a differential pair, routed side by side at a constant gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffPair {
    /// The positive net, such as `D+` or `TX_P`.
    pub positive: NetId,
    /// The negative net.
    pub negative: NetId,
    /// The width of each of the two tracks where they run together. Raised to the rules'
    /// minimum track width if it is less.
    pub width: i64,
    /// The gap between the two tracks, edge to edge. Raised to the clearance the rules
    /// keep between the two nets if it is less.
    pub gap: i64,
}

/// What lengths a net, or a group of nets, must route to: each net's length, the total
/// length of its tracks, and how far the longest and the shortest may differ.
///
/// For the two nets of a [`DiffPair`], a skew within the pair; for a bus or the channels of
/// an interface, a matched group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LengthConstraint {
    /// The nets.
    pub nets: Vec<NetId>,
    /// The shortest each net may be.
    pub min: Option<i64>,
    /// The longest each net may be.
    pub max: Option<i64>,
    /// The length to aim at, if not halfway between [`min`](LengthConstraint::min) and
    /// [`max`](LengthConstraint::max).
    pub target: Option<i64>,
    /// How much longer the longest of the nets may be than the shortest.
    pub max_skew: Option<i64>,
}

/// A board to route: what is fixed on it, its rules, its edge and its nets.
#[derive(Clone, Debug)]
pub struct Design {
    /// The fixed items: pads, keepouts, and any tracks, vias and zones already there, which
    /// routes keep clear of, or join if they are of the same net.
    pub board: Board,
    /// The rules.
    pub rules: Rules,
    /// The board's edge, which copper keeps [`edge_clearance`](Design::edge_clearance)
    /// from. Routes stay on the side of it where their pads are.
    pub outline: Vec<Shape>,
    /// The clearance between copper and the edge.
    pub edge_clearance: i64,
    /// By [`NetId`], how to route each net. Nets past the end are not routed.
    pub nets: Vec<NetSpec>,
    /// Openings in the solder mask, which other nets' copper keeps out of.
    pub masks: Vec<MaskOpening>,
    /// Copper with no net that the board doesn't hold, such as text, which routes keep
    /// clear of as of any copper. The check doesn't see it.
    pub obstacles: Vec<(Shape, LayerId)>,
    /// Differential pairs, each net in one at most.
    pub pairs: Vec<DiffPair>,
    /// Lengths to route nets to, which meanders make up after routing.
    pub lengths: Vec<LengthConstraint>,
    /// Copper zones, which are filled after routing, and which their nets connect to
    /// rather than routing tracks between all their pads: see [`Options::planes`].
    pub zones: Zones,
}

impl Design {
    /// The same design with no tracks, vias or zones' fills: only pads, keepouts, the edge
    /// and the zones' outlines in [`zones`](Design::zones).
    pub fn without_routing(&self) -> Design {
        let mut design = self.clone();
        design.board.tracks.clear();
        design.board.vias.clear();
        design.board.zones.clear();
        design
    }

    /// The design of a KiCad board: every net with a name, at its class's track width and
    /// via size, and its differential pairs, as KiCad finds them by their names, at their
    /// class's width and gap. Enabled by the `kicad` feature.
    ///
    /// Lengths come from a board's custom rules, which
    /// [`kicad::length_rules`](crate::kicad::length_rules) reads, into
    /// [`lengths`](Design::lengths).
    #[cfg(feature = "kicad")]
    pub fn from_kicad(board: &crate::kicad::KicadBoard) -> Design {
        let mut pairs = board.routing.pairs.clone();
        // Pairs that the rules give a gap of their own, if the names didn't.
        for &(a, b, gap) in &board.rules.spec().pairs {
            if !pairs.iter().any(|p| [p.positive, p.negative].contains(&a))
                && !pairs.iter().any(|p| [p.positive, p.negative].contains(&b))
            {
                let width = board
                    .routing
                    .nets
                    .get(a.index())
                    .map_or(0, |n| n.track_width);
                pairs.push(DiffPair {
                    positive: a,
                    negative: b,
                    width,
                    gap,
                });
            }
        }
        let nets = board
            .routing
            .nets
            .iter()
            .zip(&board.nets)
            .map(|(n, name)| NetSpec {
                route: !name.is_empty(),
                track_width: n.track_width,
                via: (n.via_diameter > n.via_drill && n.via_drill > 0).then_some(ViaSize {
                    diameter: n.via_diameter,
                    drill: n.via_drill,
                }),
            })
            .collect();
        Design {
            board: board.board.clone(),
            rules: board.rules.clone(),
            outline: board.routing.outline.clone(),
            edge_clearance: board.routing.edge_clearance,
            nets,
            masks: board.routing.masks.clone(),
            obstacles: board.routing.text.clone(),
            pairs,
            lengths: Vec::new(),
            zones: board.routing.zones.clone(),
        }
    }
}

/// What [`route`] may do.
///
/// Create this with [`Default`] and set the fields.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Options {
    /// The pitch of the lattice that tracks move on, or `None` for half the smallest track
    /// width plus clearance of the nets routed.
    pub pitch: Option<i64>,
    /// The size of a global routing tile, or `None` for 16 pitches.
    pub tile: Option<i64>,
    /// Whether to route globally first and keep each net to its corridor. The default is
    /// true. Without, each net is searched for over the whole board.
    pub global: bool,
    /// How many tiles a corridor reaches beyond its net's global route. The default is 1.
    pub corridor_margin: u32,
    /// The extra cost of a track on a layer that global routing did not choose for its
    /// tile, as a fraction of its length. The default is 0, as any more lost nets on the
    /// curated boards: see `docs/benchmarks.md`.
    pub layer_bias: f64,
    /// What a via costs, in millimetres of track. The default is 1.
    pub via_cost: f64,
    /// How many times a net can be ripped up for others. The default is 4; zero turns
    /// rip-up off.
    pub rip_ups: u32,
    /// How many nodes one net's search may expand. The default is 2 million.
    pub max_expansions: u64,
    /// Whether nets left unrouted get a second pass, on a lattice of half the pitch, in
    /// which any net may be ripped up again. The default is true.
    pub fine_pass: bool,
    /// Whether to clean up after routing. The default is true.
    pub cleanup: bool,
    /// Whether to route [differential pairs](Design::pairs) side by side. The default is
    /// true. Without, their nets are routed as any others.
    pub couple_pairs: bool,
    /// Whether to route [differential pairs](Design::pairs) side by side first, before
    /// anything else, as fixed copper, rather than last, in place of their nets' routes.
    /// The default is false: first, pairs take room that other nets need, and last, a
    /// pair that finds no room through what is routed stays as two nets. Boards whose pairs
    /// matter most, such as DRAM's clocks and strobes, route them first: see
    /// `docs/benchmarks.md`.
    pub pairs_first: bool,
    /// Whether to add meanders for [lengths](Design::lengths). The default is true.
    pub tune_lengths: bool,
    /// How far a meander may reach out from the track it is added to. The default is 3 mm.
    pub max_amplitude: i64,
    /// Whether nets with [zones](Design::zones) connect to them, rather than routing tracks
    /// between all their pads. The default is true. Without, zones are still filled after
    /// routing, around everything routed.
    pub planes: bool,
    /// What a track costs for its length inside another net's plane, a zone on an inner
    /// layer, over its length anywhere else, as a fraction of its length. The default is 1,
    /// twice the cost, so that routes cut planes in pieces only where they must.
    pub plane_cost: f64,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            pitch: None,
            tile: None,
            global: true,
            corridor_margin: 1,
            layer_bias: 0.0,
            via_cost: 1.0,
            rip_ups: 4,
            max_expansions: 2_000_000,
            fine_pass: true,
            cleanup: true,
            couple_pairs: true,
            pairs_first: false,
            tune_lengths: true,
            max_amplitude: 3_000_000,
            planes: true,
            plane_cost: 1.0,
        }
    }
}

/// What a run did, for measuring it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Stats {
    /// Nets that needed routing: those with pads not yet connected.
    pub nets: usize,
    /// Of those, the nets routed.
    pub routed: usize,
    /// Connections needed, the number of terminals less one for each net.
    pub connections: usize,
    /// Of those, the connections made, by routed nets.
    pub connected: usize,
    /// Nets routed outside their corridor, as theirs had no path.
    pub outside_corridor: usize,
    /// How many times a net was ripped up.
    pub rip_ups: usize,
    /// How many nets the first pass left unrouted, for the second.
    pub fine_pass: usize,
    /// Nodes expanded by detailed searches, cleanup included.
    pub expanded: u64,
    /// Resources that global routing left overused.
    pub global_overused: usize,
    /// Vias and track length, in nanometres, before cleanup.
    pub before_cleanup: (usize, f64),
    /// Vias and track length after.
    pub after_cleanup: (usize, f64),
    /// Differential pairs with pads to connect, and of those, the pairs routed side by
    /// side.
    pub pairs: (usize, usize),
    /// Length constraints, and of those, the constraints met after meanders.
    pub lengths: (usize, usize),
    /// The track that meanders added, in nanometres.
    pub meanders: f64,
    /// Stubs added to connect nets to their zones, before routing and after.
    pub stubs: usize,
    /// Nets whose copper and zones are still in pieces, after the last attempt to join them.
    pub planes_broken: usize,
}

/// The result of [`route`].
#[derive(Clone, Debug)]
pub struct Routed {
    /// The tracks added, all straight.
    pub tracks: Vec<Track>,
    /// The through vias added.
    pub vias: Vec<Via>,
    /// The zones' fills, around everything else.
    pub fills: Vec<Fill>,
    /// The nets that could not be routed, in order. Nothing of theirs is in the result, but
    /// for a net with zones, which keeps what joins it to them, though they leave it in
    /// pieces, and a net of many pads, which keeps the pieces it was routed in.
    pub unrouted: Vec<NetId>,
    /// Measurements.
    pub stats: Stats,
}

impl Routed {
    /// The design's board with these tracks, vias and fills added, ready for
    /// [`drc::check`](crate::geom::drc::check).
    pub fn board(&self, design: &Design) -> Board {
        let mut board = design.board.clone();
        board.tracks.extend(self.tracks.iter().cloned());
        board.vias.extend(self.vias.iter().cloned());
        board
            .zones
            .extend(self.fills.iter().map(|f| f.to_zone(design)));
        board
    }
}

/// Why a design could not be routed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RouteError {
    /// An item's geometry is invalid.
    Geometry(GeomError),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::Geometry(e) => write!(f, "invalid geometry: {e}"),
        }
    }
}

impl std::error::Error for RouteError {}

/// Routes a design. See the [module documentation](self).
///
/// ```
/// use guide::geom::drc::{self, Pad};
/// use guide::geom::{LayerId, LayerSet, Point, Rect, RuleSpec, Rules, Shape};
/// use guide::id::NetId;
/// use guide::pcb::{self, Design, NetSpec, Options, ViaSize};
///
/// let rules = Rules::new(RuleSpec::new(&["F.Cu", "B.Cu"], 200_000, 200_000))?;
/// let top = LayerSet::EMPTY.with(LayerId(0));
/// let pad = |x: i64, y: i64, net| Pad {
///     shape: Shape::rect(Rect::new(Point::new(x - 300_000, y - 300_000),
///                                  Point::new(x + 300_000, y + 300_000))).unwrap(),
///     layers: top,
///     hole: None,
///     net: Some(NetId(net)),
///     clearance: None,
/// };
/// let mut design = Design {
///     board: drc::Board::default(),
///     rules,
///     outline: Vec::new(),
///     edge_clearance: 0,
///     masks: Vec::new(),
///     obstacles: Vec::new(),
///     pairs: Vec::new(),
///     lengths: Vec::new(),
///     zones: Default::default(),
///     nets: vec![NetSpec {
///         route: true,
///         track_width: 200_000,
///         via: Some(ViaSize { diameter: 600_000, drill: 300_000 }),
///     }; 3],
/// };
/// // Two nets that cross: one of them takes vias.
/// design.board.pads = vec![
///     pad(0, 5_000_000, 1), pad(10_000_000, 5_000_000, 1),
///     pad(5_000_000, 0, 2), pad(5_000_000, 10_000_000, 2),
/// ];
/// let routed = pcb::route(&design, &Options::default())?;
/// assert!(routed.unrouted.is_empty());
/// let board = routed.board(&design);
/// assert!(drc::check(&board, &design.rules)?.is_empty());
/// assert!(pcb::unconnected(&board, &design.rules).is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn route(design: &Design, options: &Options) -> Result<Routed, RouteError> {
    router::run(design, options)
}

#[cfg(test)]
mod pair_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod zone_tests;
