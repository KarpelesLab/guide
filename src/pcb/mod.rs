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
//!    is drawn up to it, is reached at the point nearest its centre that has. Where the
//!    net's own via fits nowhere near a trapped pad, as between the balls of a BGA of 0.8 mm
//!    pitch, the smaller one of [`RouteOptions::fanout_via`] is tried, if there is one.
//!
//!    **Planes.** The [zones](Design::zones) are filled around the fixed copper, and each
//!    net with zones takes the pads that its largest fill joins, its fills on inner layers
//!    if it has any, as one terminal, which also holds places in that fill near each of the
//!    net's other pads where a via fits. So routing it is a stub from each other pad to the
//!    fill, a track and a via, rather than tracks across the board. Other nets' tracks pay
//!    more for their length inside another net's plane, [`RouteOptions::plane_cost`]. A
//!    zone of a net of four pads or fewer that is 32 pitches of the lattice wide or less
//!    is no plane, unless it [says so](CopperZone::plane): a pour between a few pads is
//!    their connection, and it is copper as it is filled, which other nets' routes keep
//!    clear of.
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
//!    budget. There, in a run with a work budget or a deadline, the passages between two
//!    pads or vias that are only as wide as a
//!    track and its clearances, or less than a pitch more, are *gates*: each has a lattice
//!    of its own through its middle, which routes move onto and along near it as they do
//!    near their own pads, and where two gates cross, as between four balls of an array,
//!    one lattice goes through both. A lattice that starts at a pad's centre has a line
//!    in such a place by chance alone.
//! 4. **Cleanup**: nets with vias are rerouted with dearer vias, kept only if they lose
//!    vias; a run of tracks between two vias that both go back to one layer is replaced
//!    by tracks on that layer where they fit near it; then each run of tracks between
//!    pads, vias and branches is pulled tight, by replacing runs with straight or 45°
//!    shortcuts where those are legal.
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
//!   [`Shape::rings`](crate::geom::Shape::rings) reads a fill back. A host that fills
//!   its zones again around the routes in any case turns [`RouteOptions::fills`] off, and
//!   the result's last fill is not made.
//! - **A smaller via for fan-outs**, [`RouteOptions::fanout_via`]: the smallest the board
//!   may have, for the dog-bones of the pads that a net's own via fits nowhere near.
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
//! yet routed again are as they were before it took them out. Its [`Routed::unrouted`] and
//! the nets and connections of its [`Stats`] are counted on the board it returns, and
//! [`Routed::tried`] has the nets it got to. [`RunControl::progress`] hears of the run as
//! it goes. Every phase looks at the deadline and the flag, the fills before routing
//! among them, which are given up between two zones.
//!
//! A deadline and a budget are shared out, so that a run that routing would spend them on
//! still joins its zones: routing ends when three quarters of the time that is left when
//! it begins are spent, or nineteen twentieths of the work, cleanup has a third of what
//! time is left then, and the joining of zones' pieces the rest, in which it starts no
//! fill that it would not see the end of, by the time the last one took. A run whose
//! routing or cleanup ended so is one that was stopped, and says so. Without zones to
//! join, or with [`RouteOptions::join`] off, routing has that share too.
//!
//! A run with a work budget or a deadline routes for the most connections soonest, which
//! one with neither does not, so that the two differ even if nothing stops the first. In
//! its first pass, a net that finds no way in its corridor waits until every net has had
//! its turn before it is searched for over the board, and waits once more before it takes
//! other nets out: with [`RouteOptions::keep_partial`], a net of more than two terminals
//! keeps what its search over the board reached with nothing moved, and goes on from
//! that. A net
//! with a plane and 32 terminals or more that is in another's way loses only its stubs in
//! the way, not all of them. With neither, nets keep their order, shortest first, and such
//! a net is taken out whole: on crowded boards that ended with a connection more and
//! fewer vias, later.
//!
//! Such a run also plans, before it routes, the ways out of what its first routes would
//! shut in: a net with a pad in a row of fine pitch is necked to the width at which the
//! row's pads are left side by side, by the clearance of the [area](crate::geom::RuleArea)
//! that both tracks are in if one has its own, and is on that pad's lattice, and each ball too deep
//! in an area array to get out on its layer, whose net's own via fits nowhere beside it,
//! gets a dog-bone with [`RouteOptions::fanout_via`]. And a net in
//! the way of another's route is moved aside before it is taken out: cut where the route
//! crosses it and mended around the route, with nothing else moved
//! ([`Stats::shoved`]), or failing that, cut, given the next turn, and mended across
//! other nets if it has to be. A pass that ends with fewer connections than its routes
//! made at their best goes back to those. Once routing is over, the pads that routes cut
//! off the fill that their net was planned on are joined to it again by stubs
//! ([`RouteOptions::pour_cuts`]), and if asked, the ways out of rows of fine pitch are
//! planned a row at a time before anything is routed ([`RouteOptions::row_fanout`]). Two
//! nets whose pads take turns along such a row, with no room to cross on its layer, have
//! their crossing on another layer planned first, as fixed copper
//! ([`RouteOptions::crossings`]). A net whose pads the board itself keeps apart, as on a
//! board in two parts that only the schematic joins, is looked for once on the board with
//! no route on it when a search for it fails, and has no more turns once it has every
//! connection that there is a way for ([`Cause::Apart`]).
//!
//! A host whose fill makes thermal reliefs can have the room of their spokes kept: tracks
//! of other nets then pay to be where a pad's spoke will be
//! ([`RouteOptions::spoke_cost`]).
//!
//! [`Routed::failed`] has the nets left open whose own search found no way: the others
//! were tried and lost their routes to another net's, and a host that routes again on the
//! board as it is returned has room for them.
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
mod cross;
mod detail;
pub mod eco;
mod escape;
mod gates;
mod geometry;
mod global;
mod measure;
mod neck;
mod negotiate;
mod pairs;
pub mod place;
mod planes;
mod pours;
mod report;
mod router;
mod rows;
mod settle;
mod spokes;
pub mod swap;
mod tune;
mod verify;
mod whole;
mod world;
mod zones;

use core::fmt;

use crate::geom::drc::{Board, Track, Via};
use crate::geom::{GeomError, LayerId, LayerSet, Rules, Shape};
use crate::id::NetId;

pub use crate::geom::drc::MaskOpening;
pub use control::{Phase, Progress, Refill, RefillFn, RunControl, Stopped};
pub use measure::{coupling, net_length};
pub use report::{Blocker, Cause, CopperKind, DesignItem, Endpoint, OpenConnection};
#[cfg(feature = "kicad")]
pub(crate) use router::connectivity;
pub use verify::unconnected;
pub(crate) use zones::starved_reliefs;
pub use zones::{
    CopperZone, Fill, Islands, PadConnection, Spoke, ZoneConnection, Zones, fill_zones,
};

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
                spokes: Vec::new(),
                spare_spokes: Vec::new(),
                min_spokes: 0,
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
    /// 0.25 is a quarter of the box. The default is 0, which makes every zone a plane but
    /// the narrow ones of a net of four pads or fewer, which are pours between them
    /// whatever they cover. A net's zones that cover less, such as small pours round a few of its pads,
    /// are copper of the net as they are filled before routing, which routes of other nets
    /// keep clear of and which joins the pads it touches, and the net is routed as tracks
    /// between the rest. A zone that [says](CopperZone::plane) which it is doesn't count,
    /// either way.
    pub plane_coverage: f64,
    /// The layers that tracks may be routed on, or `None` for every layer of the stack.
    /// Each net may narrow them further, [`NetSpec::layers`]. Vias go through every layer
    /// all the same, and fixed copper is as it is.
    pub layers: Option<LayerSet>,
    /// How many search nodes the whole run may expand, in the unit of [`Stats::work`], or
    /// `None`, the default, for no limit. A run that spends it stops and returns what it has
    /// routed, with [`Routed::stopped`] set: the same result for the same budget, on any
    /// machine. Zone fills, cleanup's shortcuts and meanders cost nothing in this unit. A
    /// design with zones to [join](RouteOptions::join) keeps a twentieth of it for that,
    /// which routing does not spend and joining may not need. A
    /// run with a budget routes [for the most connections soonest](self#stopping-a-run-and-what-is-left-open),
    /// so its result is not that of a run with none, however large the budget.
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
    /// [`Stats::necked`] counts such nets. In a run with a budget or a deadline, the
    /// width is also one at which the pads beside such a pad are left the same way, side
    /// by side: a little narrower again, where one track fits a pad and two do not fit
    /// two.
    pub neck_down: bool,
    /// Whether the result has the zones' [fills](Routed::fills). The default is true. A
    /// host that fills its zones itself, around the routes, sets this to false: the fill
    /// around everything, seconds on a large board, is then not made, the result has no
    /// fills, and its nets and connections are counted on the fill that the joining of
    /// zones' pieces ended on, which is that one but for the islands, pieces that join
    /// nothing, or with no joining, on the fill that routing planned on.
    pub fills: bool,
    /// Whether the nets with [planes](RouteOptions::planes) that the routes leave in pieces
    /// are joined up after routing: the zones are filled around the routes, each piece gets
    /// a stub or tracks to the largest, and the zones are filled again, a few times at
    /// most. The default is true. It takes a fill of the zones or several, seconds each on
    /// a large board, and a run with a deadline keeps a sixth of its time for it. A host
    /// that fills its zones itself and routes again whatever its own fill leaves apart
    /// sets this to false: routing then has the time, and
    /// [`unrouted`](Routed::unrouted) is what the result's fills leave in pieces, or with
    /// no [fills](RouteOptions::fills), what the fill that routing planned on does.
    pub join: bool,
    /// A smaller via for the [escapes](self#how-it-routes) of fine-pitch footprints, such as
    /// the smallest the board's maker drills, or `None`, the default, for none. A pad
    /// trapped in its footprint that its net's own via fits nowhere near, as between the
    /// balls of a BGA of 0.8 mm pitch, gets its dog-bone with this one, if it is smaller
    /// and fits. Every other via of the net is its own, [`NetSpec::via`].
    pub fanout_via: Option<ViaSize>,
    /// Whether a run with a [budget](RouteOptions::work_budget) or a deadline plans the
    /// ways out of rows of fine pitch a row at a time, before anything is routed. The
    /// default is `false`. A row is one in which a via in the way out of a pad is in the way
    /// out of the pads beside it: its pads then get stubs, a straight track out of either
    /// end of each and a via at its end where one is needed and fits, chosen for the row
    /// as a whole so that no stub's via is in another's way. They are routes of their nets,
    /// which the nets go on from, and which rip-up takes out like any other. A run with no
    /// budget plans none. Off by default, as it is not a gain on every board: with it a
    /// crowded board of two layers left fewer connections open and two boards of four
    /// layers more, see `docs/benchmarks.md`.
    pub row_fanout: bool,
    /// Whether a run with a [budget](RouteOptions::work_budget) or a deadline keeps count,
    /// while it routes, of what the routes cut off the fills that its nets are planned on.
    /// The default is `true`. A net is planned on its zones' fill as it is before routing,
    /// given by a host ([`Zones::fills`]) or made here: the pads that the fill joins need
    /// no route. Routes of other nets through a fill on an outer layer leave some of those
    /// pads on a piece of their own, which a fill of the routed board shows, when it is
    /// too late to move anything. With this, the fills are kept as cells of a grid, and
    /// once routing is over what is too near a route for copper of the fill's least width
    /// is taken out of them: a pad that is then no longer on the largest piece of its
    /// fill is joined to it again by a stub that moves nothing, in cleanup's share of the
    /// time. The grid errs on the side of a cut, and a stub. A run with no budget counts
    /// none. A host that fills its zones itself can give its own fill around the routes
    /// to tell them by ([`RunControl::refill`]).
    pub pour_cuts: bool,
    /// Whether a run with a [budget](RouteOptions::work_budget) or a deadline settles what
    /// its passes of rip-up leave open. The default is `true`. Rip-up takes a net out for
    /// another and routes it again later, on a board that has changed by then, and a pass
    /// that goes on may end with fewer connections than it had. With this, the passes have
    /// seven tenths of routing's share of the budget, and in the rest each connection
    /// still open is looked for a route across other nets' routes, with the nets it
    /// crosses cut and joined up again around it at once, each across further nets if it
    /// has to: the chain is kept if the board then has more connections, and undone to
    /// the track if not. A run with no budget settles nothing.
    pub settle: bool,
    /// A seed for another order of the nets, or `None`, the default, for shortest first.
    /// With a seed, each net's length counts for between three quarters and five quarters
    /// of itself in that order, by a factor drawn from the seed and the net's number: the
    /// same seed gives the same result, and another seed a second attempt.
    pub seed: Option<u64>,
    /// Whether a run that [settles](RouteOptions::settle) looks for each connection left
    /// open by a negotiation before its chains. The default is `true`. The connection is
    /// routed across the routes in its way at a price by the length shared, the nets it
    /// runs into are routed again the same way, across it or around it, and so on in
    /// rounds, with the price raised each round and a history kept of where routes were in
    /// each other's way: every net gives way a little, where a chain moves one net at a
    /// time. The routes are kept if a round ends with none in another's way and the board
    /// has more connections for them, and are undone to the track if not.
    pub negotiate: bool,
    /// Whether a run that [negotiates](RouteOptions::negotiate) first has a negotiation
    /// for each place where two pads or more that nothing is joined to are near each
    /// other, after its first pass: the pads of a package of fine pitch that the routes
    /// round them shut in. Every net of at most eight terminals with a pad or a route near
    /// the place takes part, each loses all that it has there in every round that it is
    /// in another's way in, and is routed again across the others at a price that rises by
    /// a quarter a round, so that a fan of tracks beside the pads spreads before it sets;
    /// kept or undone as any negotiation is. The default is `false`: a place costs four
    /// to eight million nodes, which a run of a minute does not have for each of them
    /// beside what else it does, and one of four minutes does.
    pub negotiate_places: bool,
    /// Whether a run with a [budget](RouteOptions::work_budget) or a deadline plans the
    /// crossings of rows of fine pitch before anything is routed. The default is `true`.
    /// Where the pads of two nets take turns along such a row, as those of the two data
    /// lines of a USB receptacle do, only one net can join its pads on their layer, round
    /// a via out of the pad of the other between them. Where that track has no room, as
    /// with another part's pad close in front of the row, the two nets cross on another
    /// layer: the pad between gets its via, the pads beside it their ways past it, the
    /// net's other pad a via of its own and a track on another layer to the first, all
    /// as fixed copper of their nets, like the escapes of an array
    /// ([`Stats::crossings`]). A crossing with room on the row's layer is left to the
    /// routing, and a run with no budget plans none.
    pub crossings: bool,
    /// What a track costs for its length where a thermal relief's spoke of another net's
    /// pad will be, over its length anywhere else, as a fraction of its length. A pad
    /// that a [zone](Design::zones) of its net joins through a thermal relief is joined
    /// by its spokes, and a track of another net beside the pad takes one away: the room
    /// that each spoke needs, from the pad's edge across its gap and into the zone, on
    /// the zone's layers, is marked before anything is routed, and tracks of other nets
    /// keep out of it while they have another way, and go through it when they have
    /// none. A host that fills its zones itself gives the spokes as its fill makes them,
    /// [`Zones::spokes`], and those are then the ones marked. The default is 0, which
    /// marks nothing: a host whose fill makes the reliefs sets it, and 4 is what cadlab
    /// routes its boards with (`docs/benchmarks.md`, "Room for the spokes of thermal
    /// reliefs").
    pub spoke_cost: f64,
    /// Whether every net is routed against every other at once before the passes that
    /// route a net at a time. The default is `false`. On one grid over the board, each
    /// net is routed across the others at a price for every point where its copper would
    /// be too close to theirs, by how many nets are there and by how often the point was
    /// shared before; the nets in each other's way are routed again, round after round at
    /// prices that rise, until none is (PathFinder). What is still shared when the rounds
    /// end is taken out, and each net is then routed once more for fewer vias. The routes
    /// are the nets' copper as the first pass begins, less what the board's rules, asked
    /// exactly, do not allow of them, and the passes route what is left and may move the
    /// rest. In a run with a [budget](RouteOptions::work_budget) or a deadline it has
    /// [`whole_share`](RouteOptions::whole_share) of it at most. Not in a run
    /// [from routes](route_from) that it is given.
    pub whole: bool,
    /// The share of the time that a deadline leaves, and of the work that a budget
    /// leaves, that [`whole`](RouteOptions::whole) may spend. The default is 0.5. A
    /// negotiation that the share ends in with fewer points still shared than a quarter
    /// of the connections has the same share of what is left then, once: it is near its
    /// end, and the passes have little to do after one that ends.
    pub whole_share: f64,
    /// How many threads [`whole`](RouteOptions::whole) routes nets on, side by side where
    /// their pads are apart. The default is 1; the routes are the same for any number.
    pub threads: usize,
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
            fills: true,
            join: true,
            fanout_via: None,
            row_fanout: false,
            pour_cuts: true,
            settle: true,
            negotiate: true,
            negotiate_places: false,
            crossings: true,
            spoke_cost: 0.0,
            seed: None,
            whole: false,
            whole_share: 0.5,
            threads: 1,
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
    /// How many times a net was moved aside for another's route, in a run with a budget:
    /// cut where the route crosses it and mended around the route, with no other net
    /// moved. They are not among the [`rip_ups`](Stats::rip_ups).
    pub shoved: usize,
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
    /// Ways out planned for the pads of rows of fine pitch
    /// ([`RouteOptions::row_fanout`]), before routing.
    pub row_stubs: usize,
    /// The vias planned for the crossings of rows of fine pitch
    /// ([`RouteOptions::crossings`]), before routing.
    pub crossings: usize,
    /// Pieces of fill with pads on them found cut off the fill that their net is planned
    /// on, each time the net was given a turn to join them to it again
    /// ([`RouteOptions::pour_cuts`]).
    pub pour_cuts: usize,
    /// How many times the host gave its own fill of the zones around the routes, to tell
    /// those pieces by ([`RunControl::refill`]).
    pub refills: usize,
    /// The stubs to fills that the host's own fill around the routes made needless, and
    /// that were taken out again ([`RunControl::refill`]).
    pub stubs_pruned: usize,
    /// The chains of moves tried for the connections that rip-up left open
    /// ([`RouteOptions::settle`]), those of them kept, and the connections that these
    /// made.
    pub settled: (usize, usize, usize),
    /// The negotiations tried for the connections that rip-up left open
    /// ([`RouteOptions::negotiate`]), those of them kept, and the connections that these
    /// made.
    pub negotiated: (usize, usize, usize),
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
    /// Of the negotiation for every net at once ([`RouteOptions::whole`]): its rounds, the
    /// points that were still shared after the best of them, the nets taken out for
    /// those, and the nets whose routes the first pass began with.
    pub whole: (usize, usize, usize, usize),
    /// The connections that those routes made, and the vias that routing each net again
    /// for fewer of them saved.
    pub whole_made: (usize, usize),
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
    /// The nets that had a search of their own in the run, in order: a net left open that
    /// is here was looked for a way and not given one, or lost it to another net since,
    /// and one that is not here was not got to before the run was
    /// [stopped](Routed::stopped), or had nothing to route. Nets whose zones' pieces were
    /// only to be joined after routing are not here for that.
    pub tried: Vec<NetId>,
    /// Of the nets left open, in order, those whose own last search found no way, or ran
    /// out of nodes: the others that were [tried](Routed::tried) lost their routes to
    /// another net's and were not got to again before the run was
    /// [stopped](Routed::stopped), and a run on the board as it is returned has room to
    /// give them, where these were looked for and found none. None on the board as it was
    /// when their turn came, that is: in a run that was stopped, other nets were taken
    /// out and put back since, and a run on the board as it is returned may find a way
    /// for some of these too. Only after a run that ended on its own is the list final.
    pub failed: Vec<NetId>,
    /// In [`route_from`], the nets whose routes of the earlier run were taken back, in
    /// order: [`tracks`](Routed::tracks) and [`vias`](Routed::vias) hold what they have
    /// now, in place of what was given for them. What was given for any other net is
    /// not in the result, and stays on the board as the host has it.
    pub taken_back: Vec<NetId>,
    /// The stubs that the host's own fill around the routes made needless and that were
    /// taken out of [`tracks`](Routed::tracks) and [`vias`](Routed::vias) for it
    /// ([`RunControl::refill`], [`Stats::stubs_pruned`]): the tracks and the vias of each.
    /// A host that goes on from this result in another run gives them back with the
    /// routes, as copper of the board or in [`Earlier`]: that run plans a net with a plane
    /// on the plane alone, and would make each of them again on a board that has less
    /// room by then, and take them out again at its end.
    pub spare: (Vec<Track>, Vec<Via>),
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

/// The routes of an earlier run of a design, for [`route_from`] to go on from.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Earlier {
    /// Its tracks, each with its net.
    pub tracks: Vec<Track>,
    /// Its vias, each with its net.
    pub vias: Vec<Via>,
}

impl Earlier {
    /// These tracks and vias, as a run returned them in [`Routed::tracks`] and
    /// [`Routed::vias`], or what a host kept of them.
    pub fn new(tracks: Vec<Track>, vias: Vec<Via>) -> Earlier {
        Earlier { tracks, vias }
    }
}

/// Routes a design as [`route_with`] does, going on from the routes of an earlier run: the
/// tracks and vias of `earlier` are routes of their nets from the start, which this run may
/// move for another net as it moves its own, where the same copper among
/// [`Board::tracks`](crate::geom::drc::Board) is fixed and can only be gone round.
///
/// A net that the routes given join whole is not routed again unless another net moves
/// it, and one that they leave in pieces is completed from them. What the run returns
/// holds the routes it took back as they are then, moved or not, so that it stands in
/// place of them, not beside them. The design is the one that the earlier run had, or one
/// changed since: what of the routes given is no longer legal on it is left out, and their
/// nets are routed again there. With no routes it is [`route_with`].
///
/// This is for a host that fills the zones itself between two runs, or checks the first
/// run's routes by its own rules: the second run has the new fill and what the host kept,
/// and can still move a route for a pad that the fill no longer reaches. It is of use in
/// a run with a deadline or a budget, where nets are moved aside and mended; a run with
/// neither routes the nets left as [`route_with`] does.
///
/// Routes are taken back for a net that is routed ([`NetSpec::route`]) and has no zone,
/// if its tracks are straight, each as wide as the net is routed on its layer, and its
/// vias are of the net's size ([`NetSpec::via`]) through every layer, as a run returns
/// them. The copper of any other net stays where it is, as copper of the board:
/// a net with a zone has stubs to its fill, which go where the fill is, and a net with
/// nothing to route has no route to move. [`Routed::taken_back`] lists the nets whose
/// routes were taken back; what was given for the others is not in the result.
///
/// # Errors
///
/// As [`route`].
pub fn route_from(
    design: &Design,
    earlier: &Earlier,
    options: &RouteOptions,
    control: &RunControl<'_>,
) -> Result<Routed, RouteError> {
    router::run_from(design, Some(earlier), options, control)
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
mod whole_tests;
#[cfg(test)]
mod zone_tests;
