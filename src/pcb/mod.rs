//! PCB routing: multi-layer, grid-free routing of a board's nets over the geometry layer,
//! with through vias, keepouts, fixed pre-routes and net classes. Enabled by the `pcb`
//! feature.
//!
//! [`route`] takes a [`Design`], the fixed items of a board with its rules and the nets
//! to route, and returns tracks, vias and zones' fills of the same
//! [`drc`](crate::geom::drc) model, so that [`drc::check`](crate::geom::drc::check) runs on
//! the result as on any board. With the `kicad` feature, [`Design::from_kicad`] takes a
//! board that [`kicad::read`](crate::kicad::read) has read. [`fill_zones`] fills a design's
//! copper zones on their own, as KiCad's zone filler does. [`place`] places a board's
//! components before it is routed, and places some again where routing found trouble.
//! [`eco`] routes a routed board again after a change, keeping the copper the change leaves
//! alone. [`swap`] exchanges the nets of equivalent pins where that shortens them.
//!
//! [`route`] has an example of a design built by hand; the `kicad` and `dsn` modules have
//! examples of routing boards read from files.
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
//!    more for their length inside another net's plane, [`RouteOptions::plane_cost`].
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
//!    the copper it has, and a search that crosses others' routes has four times the
//!    budget.
//! 4. **Cleanup**: nets with vias are rerouted with dearer vias, kept only if they lose
//!    vias; then each run of tracks between pads, vias and branches is pulled tight, by
//!    replacing runs with straight or 45° shortcuts where those are legal.
//! 5. **Differential pairs**, [`Design::pairs`]: each pair's two routes are taken out, and
//!    the pair is routed side by side in their place, at its gap, where it fits: a search
//!    for its centreline, wide enough for both tracks, offset into the two tracks, with
//!    vias side by side and each net's own fan-out near its pads. It is kept if it connects
//!    all that the two routes did, so it can't cost another net its route. Pairs whose
//!    two nets' pads are far apart are left as they are. With [`RouteOptions::pairs_first`],
//!    pairs are routed this way before anything else instead, with a place for a via kept
//!    free of them beside each pad of a net with zones next to their pads, such as a ground
//!    pin beside a pair's pins. They stay where they are in the first pass, as fixed
//!    copper. In the passes on the finer lattice, a net may cross them in rip-up like any
//!    route: the pair is taken out whole, and once the net is placed, routed again side by
//!    side, ripping up other nets in its way but not pairs or that net. A pair that finds
//!    no way goes back where it was, for good, and the net is routed again around it.
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
//! # What a board may say
//!
//! Beyond pads, nets and rules, for a host with a board model of its own. Each is off or
//! empty by default, and routing is then as it is without.
//!
//! - **No via in a pad**, [`RouteOptions::vias_in_pads`]: vias keep out of pads with no
//!   hole, and escapes are dog-bones.
//! - **Zones that the host fills**, [`Zones::fills`]: routing plans on the host's copper
//!   rather than on a fill of its own, which may differ from it.
//!   [`Shape::rings`](crate::geom::Shape::rings) reads a fill back.
//! - **Zones that are not planes**, [`CopperZone::plane`] and
//!   [`RouteOptions::plane_coverage`]: small pours are copper as they are filled, and their
//!   nets are routed as tracks.
//! - **Keepouts that except nets**, [`Keepout::except`](crate::geom::drc::Keepout::except).
//! - **Layers**, [`RouteOptions::layers`] and [`NetSpec::layers`]: where tracks may be.
//! - **Pad stacks**, [`Pad::stack`](crate::geom::drc::Pad::stack): other copper on other
//!   layers, one pad.
//! - **Net ties**, [`Board::ties`](crate::geom::drc::Board::ties): items at which nets may
//!   touch.
//! - **Rules by area**, [`RuleSpec::areas`](crate::geom::RuleSpec::areas): a clearance
//!   inside an area, as between the pads of a part of fine pitch.
//! - **The edge**, [`edges`]: an outline and its cutouts as [`Design::outline`] takes
//!   them.
//!
//! # Stopping a run, and what is left open
//!
//! A run can be bounded in three ways, and returns what it has routed until then, legal as
//! any result, with [`Routed::stopped`] saying why it stopped:
//! [`RouteOptions::work_budget`], a number of search nodes for the whole run, which gives
//! the same result on any machine; and through [`route_with`]'s [`RunControl`], a deadline
//! and a cancel flag, whose results depend on the moment. A stopped run skips what is left
//! of escapes, routing, cleanup, the coupling of pairs, the joining of zones' pieces and
//! meanders, and fills the zones around what it has; nets that rip-up had taken out and not
//! yet routed again are left out. Its [`Routed::unrouted`] and the nets and connections of
//! its [`Stats`] are counted on the board it returns. [`RunControl::progress`] hears of the
//! run as it goes.
//!
//! Whatever the run leaves apart is in [`Routed::open`], connection by connection, in the
//! sense of a ratsnest: the pieces a net's pads are left in, less one, each with a pad on
//! either side, by position in the design's pads, and the best-known [`Cause`]. With
//! [`RouteOptions::keep_partial`], a net that can't be routed whole keeps the tree it could
//! be routed as, so that one pad shut in costs one connection and not all the net's.
//!
//! # Determinism
//!
//! Everything is in a fixed order: nets by length and identifier, steps by direction, and
//! the only hash maps are keyed lookups whose order never matters. Costs are sums of
//! lengths in a fixed order. No thread is used, so there is no thread count to be
//! independent of. [`RouteOptions::seed`] gives another order of nets, the same for the
//! same seed, for a second attempt.

mod cleanup;
mod control;
mod detail;
pub mod eco;
mod escape;
mod geometry;
mod global;
mod measure;
mod neck;
mod pairs;
pub mod place;
mod planes;
mod report;
mod router;
pub mod swap;
mod tune;
mod verify;
mod world;
mod zones;

use core::fmt;

use crate::geom::drc::{Board, Track, Via};
use crate::geom::{GeomError, LayerId, LayerSet, Rules, Shape};
use crate::id::NetId;

pub use crate::geom::drc::MaskOpening;
pub use control::{Phase, Progress, RunControl, Stopped};
pub use measure::{coupling, net_length};
pub use report::{Blocker, Cause, CopperKind, DesignItem, Endpoint, OpenConnection};
#[cfg(feature = "kicad")]
pub(crate) use router::connectivity;
pub use verify::unconnected;
pub(crate) use zones::starved_reliefs;
pub use zones::{CopperZone, Fill, Islands, PadConnection, ZoneConnection, Zones, fill_zones};

/// The size of a net's through vias.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViaSize {
    /// The diameter of the copper.
    pub diameter: i64,
    /// The diameter of the hole.
    pub drill: i64,
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
    /// The layers its tracks may be on, of those that [`RouteOptions::layers`] leaves, or
    /// `None` for all of them. Its vias go through every layer all the same, and a pad of
    /// its on none of these layers is reached by a via in the pad, or not at all.
    pub layers: Option<LayerSet>,
}

impl NetSpec {
    /// A net to route, at this track width and with these vias, on any layer.
    pub fn new(track_width: i64, via: Option<ViaSize>) -> NetSpec {
        NetSpec {
            route: true,
            track_width,
            via,
            layers: None,
        }
    }
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
    /// routes keep clear of, or join if they are of the same net, and the openings in the
    /// solder mask, which other nets' copper keeps out of.
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
    /// Copper with no net that the board doesn't hold, such as text, which routes keep
    /// clear of as of any copper. The check doesn't see it.
    pub obstacles: Vec<(Shape, LayerId)>,
    /// Differential pairs, each net in one at most.
    pub pairs: Vec<DiffPair>,
    /// Lengths to route nets to, which meanders make up after routing.
    pub lengths: Vec<LengthConstraint>,
    /// Copper zones, which are filled after routing, and which their nets connect to
    /// rather than routing tracks between all their pads: see [`RouteOptions::planes`].
    pub zones: Zones,
}

impl Design {
    /// The same design with no tracks, vias or zones' fills: only pads, keepouts, the edge
    /// and the zones' outlines in [`zones`](Design::zones).
    #[must_use]
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
                layers: None,
            })
            .collect();
        Design {
            board: board.board.clone(),
            rules: board.rules.clone(),
            outline: board.routing.outline.clone(),
            edge_clearance: board.routing.edge_clearance,
            nets,
            obstacles: board.routing.text.clone(),
            pairs,
            lengths: Vec::new(),
            zones: board.routing.zones.clone(),
        }
    }

    /// The design of a Specctra DSN board: every net with a name, at its class's track width
    /// and via, the boundary as the board's edge, which copper keeps the board's clearance
    /// from, and each plane with a net as a copper zone of that net, so that its net connects
    /// to it as Freerouting's do. Enabled by the `dsn` feature.
    ///
    /// The wiring already in the file stays, as fixed copper: take the tracks and vias out
    /// of [`board`](Design::board) to route those nets again. A DSN file has no differential
    /// pairs, lengths, copper text or zone settings, so zones take KiCad's defaults for new
    /// zones, or the board's clearance and track width where those are larger: a clearance of
    /// 0.5 mm, a minimum width of 0.25 mm, and thermal reliefs with a gap of 0.5 mm and spokes
    /// 0.5 mm wide. Those err on the side of fills that reach less than the board's own
    /// zones will, once refilled, so that what routing joins to a zone stays joined.
    #[cfg(feature = "dsn")]
    pub fn from_dsn(board: &crate::dsn::DsnBoard) -> Design {
        /// Clearance, minimum width, thermal gap and spoke width.
        const ZONE_DEFAULTS: (i64, i64, i64, i64) = (500_000, 250_000, 500_000, 500_000);
        let spec = board.rules.spec();
        let nets = board
            .nets
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let net = Some(NetId(i as u32));
                NetSpec {
                    route: !name.is_empty(),
                    track_width: board.rules.track_width(net, LayerId(0)),
                    via: board.rules.via(net).map(|v| ViaSize {
                        diameter: v.diameter(),
                        drill: v.drill,
                    }),
                    layers: None,
                }
            })
            .collect();
        let rings: Vec<&[crate::geom::Point]> = board.boundary.iter().map(Vec::as_slice).collect();
        let outline = edges(&rings);
        let zones = board
            .planes
            .iter()
            .filter(|p| p.net.is_some() && p.outline.len() >= 3)
            .map(|p| CopperZone {
                outline: p.outline.clone(),
                holes: Vec::new(),
                layers: LayerSet::EMPTY.with(p.layer),
                net: p.net,
                priority: 0,
                clearance: spec.clearance.max(ZONE_DEFAULTS.0),
                min_width: spec.track_width.max(ZONE_DEFAULTS.1),
                connection: ZoneConnection::Thermal,
                thermal_gap: ZONE_DEFAULTS.2,
                spoke_width: ZONE_DEFAULTS.3,
                islands: Islands::Remove,
                ..CopperZone::default()
            })
            .collect();
        Design {
            board: board.board.clone(),
            rules: board.rules.clone(),
            outline,
            edge_clearance: spec.clearance,
            nets,
            obstacles: Vec::new(),
            pairs: Vec::new(),
            lengths: Vec::new(),
            zones: Zones {
                zones,
                pads: Vec::new(),
                keepouts: Vec::new(),
                fills: Vec::new(),
            },
        }
    }
}

/// A board's edge as [`Design::outline`] takes it, from its outline and its cutouts as
/// rings of points, in any orientation, the last point joined to the first: each side as a
/// line one unit wide, which copper keeps [`Design::edge_clearance`] from. Sides of no
/// length, and points out of range, are left out.
///
/// ```
/// use guide::geom::Point;
/// use guide::pcb;
///
/// let board = [Point::new(0, 0), Point::new(50_000_000, 0), Point::new(50_000_000, 30_000_000),
///              Point::new(0, 30_000_000)];
/// let slot = [Point::new(10_000_000, 10_000_000), Point::new(20_000_000, 10_000_000),
///             Point::new(20_000_000, 12_000_000), Point::new(10_000_000, 12_000_000)];
/// assert_eq!(pcb::edges(&[&board, &slot]).len(), 8);
/// ```
pub fn edges(rings: &[&[crate::geom::Point]]) -> Vec<Shape> {
    let mut outline = Vec::new();
    for ring in rings {
        for (i, &a) in ring.iter().enumerate() {
            let b = ring[(i + 1) % ring.len()];
            if a != b
                && let Ok(edge) = Shape::track(a, b, 1)
            {
                outline.push(edge);
            }
        }
    }
    outline
}

/// What [`route`] may do.
///
/// Create this with [`Default`] and set the fields.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct RouteOptions {
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
    /// anything else, rather than last, in place of their nets' routes; only nets left for
    /// the passes on the finer lattice may move them, and they are routed again side by
    /// side.
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
    /// Whether a via may be in a pad that has no hole, or touch one, its own net's included.
    /// The default is true. Without, as for boards whose vias are not filled and capped, a
    /// pad that needs a via gets it beside it, at the end of a short track, a dog-bone: so do
    /// the [escapes](self#how-it-routes) of fine-pitch footprints, and no route goes under a
    /// pad for a via in it.
    pub vias_in_pads: bool,
    /// How much of the box of a net's pads its zones' fills must cover, on the layer where
    /// they cover the most, for the net to plan on them as [planes](RouteOptions::planes):
    /// 0.25 is a quarter of the box. The default is 0, which makes every zone a plane. A
    /// net's zones that cover less, such as small pours round a few of its pads, are copper
    /// of the net as they are filled before routing, which routes of other nets keep clear
    /// of and which joins the pads it touches, and the net is routed as tracks between the
    /// rest. A zone that [says](CopperZone::plane) which it is doesn't count, either way.
    pub plane_coverage: f64,
    /// The layers that tracks may be routed on, or `None` for every layer of the stack.
    /// Each net may narrow them further, [`NetSpec::layers`]. Vias go through every layer
    /// all the same, and fixed copper is as it is.
    pub layers: Option<LayerSet>,
    /// How many search nodes the whole run may expand, in the unit of [`Stats::work`], or
    /// `None`, the default, for no limit. A run that spends it stops and returns what it has
    /// routed, with [`Routed::stopped`] set: the same result for the same budget, on any
    /// machine. Zone fills, cleanup's shortcuts and meanders cost nothing in this unit.
    pub work_budget: Option<u64>,
    /// Whether a net that can't be routed whole keeps the tree it can be routed as: after
    /// everything else has failed for it, it is searched for a terminal at a time and keeps
    /// what it reaches, as nets with zones and nets of many pads do in any case, and the
    /// passes on the finer lattice then complete it from that copper. The default is false:
    /// such a net of 16 terminals or fewer has nothing in the result.
    pub keep_partial: bool,
    /// Whether a net whose tracks are too wide to leave one of its pads is routed narrower:
    /// at the widest width at which a straight track does leave every such pad, past the
    /// pads beside it and everything else that is fixed, and never under the least width
    /// the rules allow, [`Rules::track_width`](crate::geom::Rules::track_width). The net
    /// has that width everywhere. The default is true: [`NetSpec::track_width`] is then
    /// the width a net is routed at where it can be, as a net class's is, and a net between
    /// the pads of a part of fine pitch is routed, narrower, rather than not at all.
    /// [`Stats::necked`] counts such nets.
    pub neck_down: bool,
    /// A seed for another order of the nets, or `None`, the default, for shortest first.
    /// With a seed, each net's length counts for between three quarters and five quarters
    /// of itself in that order, by a factor drawn from the seed and the net's number: the
    /// same seed gives the same result, and another seed a second attempt.
    pub seed: Option<u64>,
}

impl Default for RouteOptions {
    fn default() -> RouteOptions {
        RouteOptions {
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
            vias_in_pads: true,
            plane_coverage: 0.0,
            layers: None,
            work_budget: None,
            keep_partial: false,
            neck_down: true,
            seed: None,
        }
    }
}

/// What a run did, for measuring it.
///
/// Nets and connections are the router's own count of what it set out to do, unless the
/// design has zones that nets connect to, or the run was [stopped](Routed::stopped): they
/// are then counted on the board that is returned, as [`islands`](Stats::islands) always is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Stats {
    /// Nets that needed routing: those with pads not yet connected.
    pub nets: usize,
    /// Of those, the nets routed.
    pub routed: usize,
    /// Connections needed, the number of terminals less one for each net. A terminal is a
    /// group of pads that fixed copper, or a zone's planned fill, joins already; see
    /// [`islands`](Stats::islands) for the count that leaves fills out.
    pub connections: usize,
    /// Of those, the connections made, by routed nets.
    pub connected: usize,
    /// Nets routed outside their corridor, as theirs had no path.
    pub outside_corridor: usize,
    /// How many times a net was ripped up.
    pub rip_ups: usize,
    /// How many nets the first pass left unrouted, for the second.
    pub fine_pass: usize,
    /// How many nets are routed narrower than their own width, so as to leave their pads:
    /// see [`RouteOptions::neck_down`].
    pub necked: usize,
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
    /// Work spent: the nodes expanded by every search of the run, those of pairs, stubs
    /// and joins included, which [`expanded`](Stats::expanded) leaves out. The unit of
    /// [`RouteOptions::work_budget`].
    pub work: u64,
    /// Connections as a host counts them, and of those, the connections made: for each net
    /// routed, the pieces that the design's own copper leaves its pads in (its tracks, vias
    /// and filled zones in [`Design::board`], not the fills planned for
    /// [`Design::zones`]), less one, and how many fewer pieces the result leaves, through
    /// its tracks, vias and fills. What is not made is listed in [`Routed::open`].
    pub islands: (usize, usize),
}

/// The result of [`route`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Routed {
    /// The tracks added, all straight.
    pub tracks: Vec<Track>,
    /// The through vias added.
    pub vias: Vec<Via>,
    /// The zones' fills, around everything else. Those of a zone that the host
    /// [filled](Zones::fills) are the host's, less what the routes need.
    pub fills: Vec<Fill>,
    /// The nets that could not be routed, in order. Nothing of theirs is in the result, but
    /// for a net with zones, which keeps what joins it to them, though they leave it in
    /// pieces, a net of many pads, which keeps the pieces it was routed in, and with
    /// [`RouteOptions::keep_partial`], any net. [`open`](Routed::open) has what each of them
    /// is short of. In a run that was [stopped](Routed::stopped), these are the nets that
    /// the board returned leaves in pieces.
    pub unrouted: Vec<NetId>,
    /// Measurements.
    pub stats: Stats,
    /// Why the run stopped before it was done, if it did: what is here is then what was
    /// routed until then.
    pub stopped: Option<Stopped>,
    /// The connections left open, by net and then by the position of the pad they end at.
    pub open: Vec<OpenConnection>,
    /// What each of [`tracks`](Routed::tracks) is, in the same order.
    pub track_kinds: Vec<CopperKind>,
    /// What each of [`vias`](Routed::vias) is, in the same order.
    pub via_kinds: Vec<CopperKind>,
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
    /// A zone's outline, or a shape the router derived, has invalid geometry.
    Geometry(GeomError),
    /// A track or a via of the design has invalid geometry.
    Item {
        /// Which item.
        item: DesignItem,
        /// What is wrong with it.
        error: GeomError,
    },
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::Geometry(e) => write!(f, "invalid geometry: {e}"),
            RouteError::Item { item, error } => write!(f, "invalid geometry of {item}: {error}"),
        }
    }
}

impl std::error::Error for RouteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RouteError::Geometry(e) | RouteError::Item { error: e, .. } => Some(e),
        }
    }
}

/// Routes a design. See the [module documentation](self).
///
/// Nets that can't be routed are not errors: they are left in [`Routed::unrouted`].
///
/// # Errors
///
/// [`RouteError::Item`] if a track or a via of the design has invalid geometry, such as a
/// coordinate beyond [`MAX_COORD`](crate::geom::MAX_COORD) or a width of zero, with which
/// item it is; [`RouteError::Geometry`] if a zone's outline, or a shape the router derives
/// from the design, has.
///
/// ```
/// use guide::geom::drc::{self, Pad};
/// use guide::geom::{LayerId, LayerSet, Point, Rect, RuleSpec, Rules, Shape};
/// use guide::id::NetId;
/// use guide::pcb::{self, Design, NetSpec, RouteOptions, ViaSize};
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
///     stack: Vec::new(),
/// };
/// let mut design = Design {
///     board: drc::Board::default(),
///     rules,
///     outline: Vec::new(),
///     edge_clearance: 0,
///     obstacles: Vec::new(),
///     pairs: Vec::new(),
///     lengths: Vec::new(),
///     zones: Default::default(),
///     nets: vec![NetSpec {
///         route: true,
///         track_width: 200_000,
///         via: Some(ViaSize { diameter: 600_000, drill: 300_000 }),
///         layers: None,
///     }; 3],
/// };
/// // Two nets that cross: one of them takes vias.
/// design.board.pads = vec![
///     pad(0, 5_000_000, 1), pad(10_000_000, 5_000_000, 1),
///     pad(5_000_000, 0, 2), pad(5_000_000, 10_000_000, 2),
/// ];
/// let routed = pcb::route(&design, &RouteOptions::default())?;
/// assert!(routed.unrouted.is_empty());
/// let board = routed.board(&design);
/// assert!(drc::check(&board, &design.rules)?.is_empty());
/// assert!(pcb::unconnected(&board, &design.rules).is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn route(design: &Design, options: &RouteOptions) -> Result<Routed, RouteError> {
    router::run(design, options, &RunControl::default())
}

/// Routes a design as [`route`] does, under a caller's control: a flag and a deadline that
/// stop it, and a callback that hears of its progress. See [`RunControl`].
///
/// A run that is stopped is not an error: it returns what it has routed until then, legal
/// as any result, with [`Routed::stopped`] saying why and [`Routed::open`] what is left.
/// Left to finish, it returns exactly what [`route`] does.
///
/// # Errors
///
/// As [`route`].
///
/// ```
/// use std::cell::Cell;
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use guide::geom::drc::{self, Pad};
/// use guide::geom::{LayerId, LayerSet, Point, RuleSpec, Rules, Shape};
/// use guide::id::NetId;
/// use guide::pcb::{self, Design, NetSpec, RouteOptions, RunControl, Stopped};
///
/// let rules = Rules::new(RuleSpec::new(&["F.Cu"], 200_000, 200_000))?;
/// let top = LayerSet::EMPTY.with(LayerId(0));
/// let pad = |x: i64, net| {
///     Pad::new(Shape::circle(Point::new(x, 0), 600_000).unwrap(), top, Some(NetId(net)))
/// };
/// let mut design = Design {
///     board: drc::Board::default(),
///     rules,
///     outline: Vec::new(),
///     edge_clearance: 0,
///     obstacles: Vec::new(),
///     pairs: Vec::new(),
///     lengths: Vec::new(),
///     zones: Default::default(),
///     nets: vec![NetSpec::new(200_000, None); 2],
/// };
/// design.board.pads = vec![pad(0, 1), pad(5_000_000, 1)];
///
/// // A flag set before the run starts: nothing is routed, and the result says so.
/// let cancel = AtomicBool::new(true);
/// let calls = Cell::new(0);
/// let heard = |_: &pcb::Progress| calls.set(calls.get() + 1);
/// let mut control = RunControl::default();
/// control.cancel = Some(&cancel);
/// control.progress = Some(&heard);
/// let routed = pcb::route_with(&design, &RouteOptions::default(), &control)?;
/// assert_eq!(routed.stopped, Some(Stopped::Cancelled));
/// assert!(routed.tracks.is_empty());
/// assert_eq!((routed.open.len(), routed.open[0].from.pad, routed.open[0].to.pad), (1, Some(0), Some(1)));
/// assert!(calls.get() > 0);
///
/// // Left alone, it routes as `route` does.
/// cancel.store(false, Ordering::Relaxed);
/// let routed = pcb::route_with(&design, &RouteOptions::default(), &control)?;
/// assert_eq!(routed.stopped, None);
/// assert!(routed.open.is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn route_with(
    design: &Design,
    options: &RouteOptions,
    control: &RunControl<'_>,
) -> Result<Routed, RouteError> {
    router::run(design, options, control)
}

#[cfg(test)]
mod control_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod pair_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod zone_tests;
