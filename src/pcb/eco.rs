//! Rerouting a routed board after a change, keeping the copper that the change leaves alone:
//! engineering changes, and the routing step of the placement loop.
//!
//! [`reroute`] takes the design as it is after the change, unrouted, the tracks and vias that
//! routed it before, and a [`Change`]: the nets to route again whole, and the areas where
//! something moved. What it keeps of the old copper becomes fixed copper of the design, and
//! [`route`](super::route) routes what is left to connect around it.
//!
//! # What is taken out
//!
//! 1. All the copper of each net of [`Change::nets`], and of nets no longer routed.
//! 2. Copper in an area of [`Change::areas`]: for a net with zones, only the connected piece
//!    of its copper there, a stub to its plane, so that the plane's other stubs stay; for any
//!    other net, all of it, so that no route is left half there.
//! 3. Copper that the design's pads, as they are now, make a violation of, as
//!    [`drc::check`] finds it, taken out the same way, until there
//!    is none.
//! 4. Pieces of a net's copper that touch none of its pads, left over from the above.
//!
//! The nets routed are those that lost copper, and those that what is kept, with the zones
//! filled around it, leaves in pieces; no other net is touched, nor given an escape. Every
//! other net is marked not to route, and the lattice keeps the whole design's pitch, so that
//! nothing changed routes as it was. If that leaves a net
//! unrouted, copper around its pads is taken out too, in a wider area each time, a few times
//! at most, and the best result is kept.

use std::collections::BTreeSet;

use super::router;
use super::{Design, RouteError, RouteOptions, Routed};
use crate::geom::drc::{self, Item, Track, Via, Violation};
use crate::geom::{LayerSet, Rect, Shape};
use crate::id::NetId;

/// What changed on a routed board, for [`reroute`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Change {
    /// Nets whose copper is all taken out, to be routed again: those whose pads moved or
    /// changed, and nets added.
    pub nets: Vec<NetId>,
    /// Areas where something changed, such as a moved part's outline where it was and where
    /// it is: copper that touches one is taken out, for a net with zones only its piece
    /// there, and for any other net, all of it.
    pub areas: Vec<Rect>,
    /// Nets that the routing before left unrouted, as [`Routed::unrouted`] has them: if they
    /// still find no way, the area taken out is not widened around them.
    pub unrouted: Vec<NetId>,
}

impl Change {
    /// A change of these nets and areas, of a routing that left nothing unrouted.
    pub fn new(nets: Vec<NetId>, areas: Vec<Rect>) -> Change {
        Change {
            nets,
            areas,
            unrouted: Vec::new(),
        }
    }

    /// The same change, of a routing that left these nets unrouted.
    pub fn after(mut self, unrouted: &[NetId]) -> Change {
        self.unrouted = unrouted.to_vec();
        self
    }
}

/// What [`reroute`] did.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Rerouted {
    /// The whole board's routing: the copper kept, then what was routed. Its
    /// [`stats`](Routed::stats) count nets and connections over the whole design, and vias
    /// and track over all of it, in [`after_cleanup`](super::Stats::after_cleanup).
    pub routed: Routed,
    /// How many of the old tracks and vias were kept.
    pub kept: (usize, usize),
    /// The nets routed again, in order.
    pub rerouted: Vec<NetId>,
    /// How many times the area taken out had to be widened around nets left unrouted.
    pub widened: usize,
}

/// A track or via of the old routing, as the checks see it.
struct Copper {
    net: NetId,
    shape: Shape,
    layers: LayerSet,
}

fn track_shape(t: &Track, tolerance: i64) -> Option<Shape> {
    match t.mid {
        None => Shape::track(t.start, t.end, t.width).ok(),
        Some(m) => Shape::arc_track(t.start, m, t.end, t.width, tolerance).ok(),
    }
}

fn track_length(t: &Track) -> f64 {
    let (dx, dy) = ((t.end.x - t.start.x) as f64, (t.end.y - t.start.y) as f64);
    (dx * dx + dy * dy).sqrt()
}

/// The vias and the track length, in nanometres, of some routing.
pub fn measure(tracks: &[Track], vias: &[Via]) -> (usize, f64) {
    (vias.len(), tracks.iter().map(track_length).sum())
}

/// How much two routings of a board differ: the tracks and vias of the first that the second
/// doesn't have, exactly, and those of the second the first doesn't have, each as a count and
/// as track length in nanometres: `(removed, added)`, each `(tracks, vias, length)`.
#[allow(clippy::type_complexity)]
pub fn difference(
    before: (&[Track], &[Via]),
    after: (&[Track], &[Via]),
) -> ((usize, usize, f64), (usize, usize, f64)) {
    let key = |t: &Track| {
        (
            t.start.min(t.end),
            t.start.max(t.end),
            t.mid,
            t.width,
            t.layer,
            t.net,
        )
    };
    let one = |a: (&[Track], &[Via]), b: (&[Track], &[Via])| {
        let tracks: BTreeSet<_> = b.0.iter().map(key).collect();
        let vias: BTreeSet<_> = b.1.iter().map(|v| (v.at, v.net, v.diameter)).collect();
        let gone: Vec<&Track> = a.0.iter().filter(|t| !tracks.contains(&key(t))).collect();
        let vias =
            a.1.iter()
                .filter(|v| !vias.contains(&(v.at, v.net, v.diameter)))
                .count();
        (
            gone.len(),
            vias,
            gone.iter().map(|t| track_length(t)).sum::<f64>(),
        )
    };
    (one(before, after), one(after, before))
}

/// The old copper, with what it is taken out by.
struct Old<'a> {
    design: &'a Design,
    tracks: &'a [Track],
    vias: &'a [Via],
    items: Vec<Option<Copper>>,
    /// By item, its piece: the items of its net it is connected to.
    piece: Vec<usize>,
    alive: Vec<bool>,
    planes: BTreeSet<NetId>,
    ripped: BTreeSet<NetId>,
}

impl<'a> Old<'a> {
    fn new(design: &'a Design, tracks: &'a [Track], vias: &'a [Via]) -> Old<'a> {
        let tolerance = design.board.arc_tolerance;
        let stack = design.rules.layers();
        let routable =
            |n: Option<NetId>| n.filter(|n| design.nets.get(n.index()).is_some_and(|s| s.route));
        let mut items: Vec<Option<Copper>> = Vec::new();
        for t in tracks {
            items.push(routable(t.net).and_then(|net| {
                Some(Copper {
                    net,
                    shape: track_shape(t, tolerance)?,
                    layers: LayerSet::EMPTY.with(t.layer),
                })
            }));
        }
        for v in vias {
            items.push(routable(v.net).and_then(|net| {
                Some(Copper {
                    net,
                    shape: Shape::circle(v.at, v.diameter).ok()?,
                    layers: LayerSet(LayerSet::span(v.from, v.to).0 & stack.0),
                })
            }));
        }
        // Pieces, by union of items of a net that touch on a layer.
        let mut parent: Vec<usize> = (0..items.len()).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let mut by_net: std::collections::BTreeMap<NetId, Vec<usize>> = Default::default();
        for (i, c) in items.iter().enumerate() {
            if let Some(c) = c {
                by_net.entry(c.net).or_default().push(i);
            }
        }
        for list in by_net.values() {
            for (k, &i) in list.iter().enumerate() {
                for &j in &list[k + 1..] {
                    let (Some(a), Some(b)) = (&items[i], &items[j]) else {
                        continue;
                    };
                    if a.layers.0 & b.layers.0 != 0
                        && a.shape.bbox().intersects(&b.shape.bbox())
                        && find(&mut parent, i) != find(&mut parent, j)
                        && a.shape.touches(&b.shape)
                    {
                        let (x, y) = (find(&mut parent, i), find(&mut parent, j));
                        parent[x.max(y)] = x.min(y);
                    }
                }
            }
        }
        let piece = (0..items.len()).map(|i| find(&mut parent, i)).collect();
        let alive = items.iter().map(Option::is_some).collect();
        let planes = design.zones.zones.iter().filter_map(|z| z.net).collect();
        Old {
            design,
            tracks,
            vias,
            items,
            piece,
            alive,
            planes,
            ripped: BTreeSet::new(),
        }
    }

    /// Takes out a net's copper, all of it.
    fn rip_net(&mut self, net: NetId) {
        for (i, c) in self.items.iter().enumerate() {
            if c.as_ref().is_some_and(|c| c.net == net) {
                self.alive[i] = false;
            }
        }
        self.ripped.insert(net);
    }

    /// Takes out an item's piece if its net has zones, or else all of its net's copper.
    fn rip(&mut self, i: usize) {
        let Some(net) = self.items[i].as_ref().map(|c| c.net) else {
            return;
        };
        if !self.planes.contains(&net) {
            self.rip_net(net);
            return;
        }
        let p = self.piece[i];
        for j in 0..self.items.len() {
            if self.piece[j] == p {
                self.alive[j] = false;
            }
        }
        self.ripped.insert(net);
    }

    /// Takes out copper touching an area.
    fn rip_area(&mut self, area: &Rect) {
        let Ok(r) = Shape::rect(*area) else { return };
        let hits: Vec<usize> = (0..self.items.len())
            .filter(|&i| {
                self.alive[i]
                    && self.items[i]
                        .as_ref()
                        .is_some_and(|c| c.shape.bbox().intersects(area) && c.shape.touches(&r))
            })
            .collect();
        for i in hits {
            self.rip(i);
        }
    }

    fn kept(&self) -> (Vec<Track>, Vec<Via>) {
        let n = self.tracks.len();
        let tracks = (0..n)
            .filter(|&i| self.alive[i])
            .map(|i| self.tracks[i].clone())
            .collect();
        let vias = (0..self.vias.len())
            .filter(|&i| self.alive[n + i])
            .map(|i| self.vias[i].clone())
            .collect();
        (tracks, vias)
    }

    /// Takes out what the design as it is makes a violation of, until nothing does.
    fn rip_violations(&mut self) -> Result<(), RouteError> {
        for _ in 0..16 {
            let (tracks, vias) = self.kept();
            // By position on the checked board, the item each kept one is.
            let n = self.tracks.len();
            let track_of: Vec<usize> = (0..n).filter(|&i| self.alive[i]).collect();
            let via_of: Vec<usize> = (0..self.vias.len())
                .filter(|&i| self.alive[n + i])
                .map(|i| n + i)
                .collect();
            let mut board = self.design.board.clone();
            let (t0, v0) = (board.tracks.len(), board.vias.len());
            board.tracks.extend(tracks);
            board.vias.extend(vias);
            let found = match drc::check(&board, &self.design.rules) {
                Ok(found) => found,
                Err(drc::DrcError::Geometry { error, .. }) => {
                    return Err(RouteError::Geometry(error));
                }
                // Nothing kept is on a layer the stack doesn't have: it was routed on it.
                Err(_) => return Ok(()),
            };
            let ours = |item: Item| match item {
                Item::Track(t) if t >= t0 => Some(track_of[t - t0]),
                Item::Via(v) if v >= v0 => Some(via_of[v - v0]),
                _ => None,
            };
            let mut hit: BTreeSet<usize> = BTreeSet::new();
            for v in &found {
                let items: Vec<Item> = match *v {
                    Violation::Short { a, b, .. }
                    | Violation::Clearance { a, b, .. }
                    | Violation::HoleToHole { a, b, .. } => vec![a, b],
                    Violation::HoleClearance { hole, item, .. } => vec![hole, item],
                    Violation::Keepout { item, .. } | Violation::MaskBridge { item, .. } => {
                        vec![item]
                    }
                    Violation::TrackWidth { track, .. } => vec![Item::Track(track)],
                    Violation::Drill { via, .. } | Violation::AnnularRing { via, .. } => {
                        vec![Item::Via(via)]
                    }
                };
                hit.extend(items.into_iter().filter_map(ours));
            }
            if hit.is_empty() {
                return Ok(());
            }
            for i in hit {
                self.rip(i);
            }
        }
        Ok(())
    }

    /// Takes out pieces that touch none of their net's pads.
    fn rip_orphans(&mut self) {
        let pads = &self.design.board.pads;
        let mut touching: BTreeSet<usize> = BTreeSet::new();
        for (i, c) in self.items.iter().enumerate() {
            let Some(c) = c.as_ref().filter(|_| self.alive[i]) else {
                continue;
            };
            if touching.contains(&self.piece[i]) {
                continue;
            }
            let b = c.shape.bbox();
            if pads.iter().any(|p| {
                p.net == Some(c.net)
                    && p.layers.0 & c.layers.0 != 0
                    && p.shape.bbox().intersects(&b)
                    && p.shape.touches(&c.shape)
            }) {
                touching.insert(self.piece[i]);
            }
        }
        for i in 0..self.items.len() {
            if self.alive[i] && !touching.contains(&self.piece[i]) {
                self.alive[i] = false;
                if let Some(c) = &self.items[i] {
                    self.ripped.insert(c.net);
                }
            }
        }
    }
}

/// How many times the area taken out is widened around nets left unrouted.
const WIDENINGS: usize = 2;

/// How far around a net left unrouted copper is taken out, the first time, then twice as far.
const WIDEN: i64 = 2_000_000;

/// Reroutes a design after a change, keeping the old copper that the change leaves alone.
/// See the [module](self).
///
/// `design` is the board as it is now, with no routing of its own; `tracks` and `vias` routed
/// it before the change, as [`Routed::tracks`] and [`Routed::vias`] do. The lattice's pitch is
/// the whole design's, as [`route`](super::route) takes it, unless `options` sets one.
///
/// ```
/// use guide::geom::drc::{self, Pad};
/// use guide::geom::{LayerId, LayerSet, Point, Rect, RuleSpec, Rules, Shape};
/// use guide::id::NetId;
/// use guide::pcb::{self, Design, NetSpec, RouteOptions, ViaSize};
/// use guide::pcb::eco::{self, Change};
///
/// let rules = Rules::new(RuleSpec::new(&["F.Cu", "B.Cu"], 200_000, 200_000))?;
/// let pad = |x: i64, y: i64, net| Pad {
///     shape: Shape::rect(Rect::new(Point::new(x - 300_000, y - 300_000),
///                                  Point::new(x + 300_000, y + 300_000))).unwrap(),
///     layers: LayerSet::EMPTY.with(LayerId(0)),
///     hole: None,
///     net: Some(NetId(net)),
///     clearance: None,
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
///     }; 3],
/// };
/// design.board.pads = vec![
///     pad(0, 0, 1), pad(5_000_000, 0, 1),
///     pad(0, 3_000_000, 2), pad(5_000_000, 3_000_000, 2),
/// ];
/// let before = pcb::route(&design, &RouteOptions::default())?;
/// // Net 2's second pad moves: net 1 keeps its copper.
/// design.board.pads[3] = pad(5_000_000, 5_000_000, 2);
/// let change = Change::new(vec![NetId(2)], Vec::new());
/// let after = eco::reroute(&design, &before.tracks, &before.vias, &change, &RouteOptions::default())?;
/// assert!(after.routed.unrouted.is_empty());
/// assert_eq!(after.rerouted, [NetId(2)]);
/// assert!(before.tracks.iter().filter(|t| t.net == Some(NetId(1))).all(|t| after.routed.tracks.contains(t)));
/// assert!(drc::check(&after.routed.board(&design), &design.rules)?.is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn reroute(
    design: &Design,
    tracks: &[Track],
    vias: &[Via],
    change: &Change,
    options: &RouteOptions,
) -> Result<Rerouted, RouteError> {
    let mut options = *options;
    options.pitch = Some(router::pitch(design, &options));
    let mut old = Old::new(design, tracks, vias);
    for &net in &change.nets {
        old.rip_net(net);
    }
    for area in &change.areas {
        old.rip_area(area);
    }
    old.rip_violations()?;
    old.rip_orphans();
    let mut best = route_kept(design, &old, &options)?;
    // A net that was unrouted before is not widened around: if it still finds no way, it is
    // left as it was.
    let ours = |r: &Rerouted| -> Vec<NetId> {
        r.routed
            .unrouted
            .iter()
            .copied()
            .filter(|n| !change.unrouted.contains(n))
            .collect()
    };
    let mut left = ours(&best);
    let mut widened = 0;
    while !left.is_empty() && widened < WIDENINGS {
        widened += 1;
        let reach = WIDEN << (widened - 1);
        for net in left {
            let pads = design
                .board
                .pads
                .iter()
                .filter(|p| p.net == Some(net))
                .map(|p| p.shape.bbox())
                .reduce(|a, b| a.union(&b));
            if let Some(b) = pads {
                old.rip_area(&b.expand(reach));
            }
        }
        old.rip_violations()?;
        old.rip_orphans();
        let attempt = route_kept(design, &old, &options)?;
        left = ours(&attempt);
        let (s, t) = (&attempt.routed.stats, &best.routed.stats);
        if s.connected > t.connected
            || (s.connected == t.connected && s.after_cleanup.0 < t.after_cleanup.0)
        {
            best = Rerouted { widened, ..attempt };
        }
    }
    Ok(best)
}

/// Routes the design around the copper `old` keeps.
fn route_kept(
    design: &Design,
    old: &Old<'_>,
    options: &RouteOptions,
) -> Result<Rerouted, RouteError> {
    let (kept_tracks, kept_vias) = old.kept();
    let mut fixed = design.clone();
    fixed.board.tracks.extend(kept_tracks.iter().cloned());
    fixed.board.vias.extend(kept_vias.iter().cloned());
    // What the kept copper and the zones, filled around it, leave apart.
    let mut filled = fixed.board.clone();
    if !fixed.zones.zones.is_empty() {
        let fills = super::fill_zones(&fixed.board, &fixed).map_err(RouteError::Geometry)?;
        filled.zones.extend(fills.iter().map(|f| f.to_zone(&fixed)));
    }
    let (_, _, _, _, open) = router::connectivity(design, &filled);
    for (i, spec) in fixed.nets.iter_mut().enumerate() {
        let net = NetId(i as u32);
        spec.route = spec.route && (old.ripped.contains(&net) || open.binary_search(&net).is_ok());
    }
    let routed = super::route(&fixed, options)?;
    let mut all = Routed {
        tracks: kept_tracks.clone(),
        vias: kept_vias.clone(),
        fills: routed.fills,
        unrouted: Vec::new(),
        stats: routed.stats,
    };
    all.tracks.extend(routed.tracks);
    all.vias.extend(routed.vias);
    let board = all.board(design);
    let (nets, done, needed, made, broken) = router::connectivity(design, &board);
    let s = &mut all.stats;
    (s.nets, s.routed, s.connections, s.connected) = (nets, done, needed, made);
    s.after_cleanup = measure(&all.tracks, &all.vias);
    all.unrouted = broken;
    let mut rerouted: Vec<NetId> = fixed
        .nets
        .iter()
        .enumerate()
        .filter(|(i, s)| s.route && !old.planes.contains(&NetId(*i as u32)))
        .map(|(i, _)| NetId(i as u32))
        .chain(old.ripped.iter().copied())
        .collect();
    rerouted.sort_unstable();
    rerouted.dedup();
    Ok(Rerouted {
        routed: all,
        kept: (kept_tracks.len(), kept_vias.len()),
        rerouted,
        widened: 0,
    })
}

/// The thermal reliefs that the zones' fills join to their pads by a single spoke, routed as
/// `tracks` and `vias`: each as the pad, by position in the board's pads, and the layer. KiCad
/// reports them as `starved_thermal`, wanting two spokes at least.
pub fn starved_reliefs(
    design: &Design,
    tracks: &[Track],
    vias: &[Via],
) -> Result<Vec<(usize, crate::geom::LayerId)>, RouteError> {
    let mut board = design.board.clone();
    board.tracks.extend(tracks.iter().cloned());
    board.vias.extend(vias.iter().cloned());
    super::starved_reliefs(&board, design).map_err(RouteError::Geometry)
}

/// Gives back their spokes to thermal reliefs that routing starved, as
/// [`starved_reliefs`] finds them: for each, of the four places beside its pad where spokes
/// go, the one that other nets' copper crosses, but crosses least, is kept free of copper, as
/// an [obstacle](Design::obstacles), for a second spoke; the nets whose copper is there are
/// routed again as by [`reroute`], and the zones are filled again without the obstacles.
/// Nets that this leaves unrouted are routed again without them. Returns `None` if no relief
/// is starved, or none has a place for a spoke that rerouting would free.
pub fn relieve(
    design: &Design,
    tracks: &[Track],
    vias: &[Via],
    options: &RouteOptions,
) -> Result<Option<Rerouted>, RouteError> {
    let starved = starved_reliefs(design, tracks, vias)?;
    if starved.is_empty() {
        return Ok(None);
    }
    let tolerance = design.board.arc_tolerance;
    let others: Vec<(Option<NetId>, crate::geom::LayerId, Shape)> = tracks
        .iter()
        .filter_map(|t| Some((t.net, t.layer, track_shape(t, tolerance)?)))
        .collect();
    let via_shapes: Vec<(Option<NetId>, LayerSet, Shape)> = vias
        .iter()
        .filter_map(|v| {
            Some((
                v.net,
                LayerSet::span(v.from, v.to),
                Shape::circle(v.at, v.diameter).ok()?,
            ))
        })
        .collect();
    let mut fixed = design.clone();
    let mut nets: Vec<NetId> = Vec::new();
    for &(p, layer) in &starved {
        let pad = &design.board.pads[p];
        let own = design.zones.pads.get(p).copied().unwrap_or_default();
        let zone = design
            .zones
            .zones
            .iter()
            .find(|z| z.net == pad.net && z.layers.contains(layer));
        let gap = own.thermal_gap.or(zone.map(|z| z.thermal_gap)).unwrap_or(0);
        let width = own.spoke_width.or(zone.map(|z| z.spoke_width)).unwrap_or(0);
        let min_width = zone.map_or(0, |z| z.min_width);
        if width <= 0 {
            continue;
        }
        let b = pad.shape.bbox();
        let c = crate::geom::Point::new(
            b.min().x + (b.max().x - b.min().x) / 2,
            b.min().y + (b.max().y - b.min().y) / 2,
        );
        let reach = gap + 2 * min_width + width;
        let half = width / 2 + 1;
        let point = crate::geom::Point::new;
        let places = [
            Rect::new(
                point(b.max().x, c.y - half),
                point(b.max().x + reach, c.y + half),
            ),
            Rect::new(
                point(c.x - half, b.max().y),
                point(c.x + half, b.max().y + reach),
            ),
            Rect::new(
                point(b.min().x - reach, c.y - half),
                point(b.min().x, c.y + half),
            ),
            Rect::new(
                point(c.x - half, b.min().y - reach),
                point(c.x + half, b.min().y),
            ),
        ];
        // The nets other than the pad's whose copper a place touches, grown by the
        // clearance.
        let crossing = |r: &Rect| -> Vec<NetId> {
            let clearance = design.rules.max_clearance();
            let mut out: Vec<NetId> = Vec::new();
            let Ok(s) = Shape::rect(r.expand(clearance)) else {
                return out;
            };
            for (net, l, shape) in &others {
                if *l == layer && *net != pad.net && shape.touches(&s) {
                    out.extend(*net);
                }
            }
            for (net, ls, shape) in &via_shapes {
                if ls.contains(layer) && *net != pad.net && shape.touches(&s) {
                    out.extend(*net);
                }
            }
            out.sort_unstable();
            out.dedup();
            out
        };
        // The relief has a spoke already, likely where nothing crosses: one more, where
        // routes cross least.
        let Some((place, crossed)) = places
            .iter()
            .map(|r| (r, crossing(r)))
            .filter(|(_, n)| !n.is_empty())
            .min_by_key(|(_, n)| n.len())
        else {
            continue;
        };
        if let Ok(s) = Shape::rect(*place) {
            fixed.obstacles.push((s, layer));
        }
        nets.extend(crossed);
    }
    nets.sort_unstable();
    nets.dedup();
    if nets.is_empty() {
        return Ok(None);
    }
    let mut out = reroute(
        &fixed,
        tracks,
        vias,
        &Change::new(nets, Vec::new()),
        options,
    )?;
    // Filled again without the places kept free, which aren't copper.
    let mut board = design.board.clone();
    board.tracks.extend(out.routed.tracks.iter().cloned());
    board.vias.extend(out.routed.vias.iter().cloned());
    out.routed.fills = super::fill_zones(&board, design).map_err(RouteError::Geometry)?;
    let all = out.routed.board(design);
    let (n, done, needed, made, broken) = router::connectivity(design, &all);
    let s = &mut out.routed.stats;
    (s.nets, s.routed, s.connections, s.connected) = (n, done, needed, made);
    out.routed.unrouted = broken;
    // What the places kept free left unrouted is routed again where they were.
    if !out.routed.unrouted.is_empty() {
        let change = Change::new(out.routed.unrouted.clone(), Vec::new());
        let again = reroute(
            design,
            &out.routed.tracks,
            &out.routed.vias,
            &change,
            options,
        )?;
        if again.routed.stats.connected > out.routed.stats.connected {
            let mut rerouted = out.rerouted;
            rerouted.extend(again.rerouted.iter().copied());
            rerouted.sort_unstable();
            rerouted.dedup();
            out = Rerouted { rerouted, ..again };
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::drc::Pad;
    use crate::geom::{LayerId, Point, RuleSpec, Rules};
    use crate::pcb::{NetSpec, ViaSize};

    fn design() -> Design {
        let rules = Rules::new(RuleSpec::new(&["F.Cu", "B.Cu"], 200_000, 200_000)).unwrap();
        let pad = |x: i64, y: i64, net| Pad {
            shape: Shape::rect(Rect::new(
                Point::new(x - 300_000, y - 300_000),
                Point::new(x + 300_000, y + 300_000),
            ))
            .unwrap(),
            layers: LayerSet::EMPTY.with(LayerId(0)),
            hole: None,
            net: Some(NetId(net)),
            clearance: None,
        };
        let mut d = Design {
            board: drc::Board::default(),
            rules,
            outline: Vec::new(),
            edge_clearance: 0,
            obstacles: Vec::new(),
            pairs: Vec::new(),
            lengths: Vec::new(),
            zones: Default::default(),
            nets: vec![
                NetSpec {
                    route: true,
                    track_width: 200_000,
                    via: Some(ViaSize {
                        diameter: 600_000,
                        drill: 300_000,
                    }),
                };
                5
            ],
        };
        for k in 1..5 {
            let y = k as i64 * 2_000_000;
            d.board.pads.push(pad(0, y, k));
            d.board.pads.push(pad(8_000_000, y + 1_000_000, k));
        }
        d
    }

    #[test]
    fn nothing_changed_keeps_everything() {
        let d = design();
        let before = super::super::route(&d, &RouteOptions::default()).unwrap();
        let after = reroute(
            &d,
            &before.tracks,
            &before.vias,
            &Change::default(),
            &RouteOptions::default(),
        )
        .unwrap();
        assert!(after.rerouted.is_empty(), "{:?}", after.rerouted);
        assert_eq!(after.routed.tracks, before.tracks);
        assert_eq!(after.routed.vias, before.vias);
        assert_eq!(after.routed.stats.connected, before.stats.connected);
    }

    #[test]
    fn a_moved_pad_reroutes_its_net_and_whatever_it_lands_on() {
        let mut d = design();
        let before = super::super::route(&d, &RouteOptions::default()).unwrap();
        // Net 1's far pad moves onto where net 2 was routed.
        let pad = d.board.pads[1].clone();
        let to = Point::new(4_000_000, 4_500_000);
        d.board.pads[1].shape = Shape::rect(Rect::new(
            Point::new(to.x - 300_000, to.y - 300_000),
            Point::new(to.x + 300_000, to.y + 300_000),
        ))
        .unwrap();
        let change = Change::new(
            vec![NetId(1)],
            vec![pad.shape.bbox(), d.board.pads[1].shape.bbox()],
        );
        let after = reroute(
            &d,
            &before.tracks,
            &before.vias,
            &change,
            &RouteOptions::default(),
        )
        .unwrap();
        assert!(after.routed.unrouted.is_empty());
        assert!(after.rerouted.contains(&NetId(1)));
        assert!(!after.rerouted.contains(&NetId(4)));
        let board = after.routed.board(&d);
        assert!(drc::check(&board, &d.rules).unwrap().is_empty());
        assert!(super::super::unconnected(&board, &d.rules).is_empty());
        // Net 4 is far away and kept as it was.
        for t in before.tracks.iter().filter(|t| t.net == Some(NetId(4))) {
            assert!(after.routed.tracks.contains(t));
        }
    }
}
