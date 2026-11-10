//! Placing and routing a KiCad board in one loop: placement, pin swaps, routing, and placement
//! again where routing found trouble, rerouting only what moved, while that pays.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use super::place::{footprint_outline, placement_design, replace_pad_nets, replace_placement};
use super::{KicadBoard, KicadError, read, replace_routing_and_fills};
use crate::geom::{Point, Rect};
use crate::id::NetId;
use crate::pcb::eco::{self, Change};
use crate::pcb::place::{
    Feedback, PlaceOptions, Placed, PlacementDesign, place_components, refine_components,
};
use crate::pcb::swap::{PinSwap, SwapGroup, apply_swaps, optimize_swaps, two_pin_groups};
use crate::pcb::{self, Design, Options, RouteError, Routed};
use crate::place::{CongestionGrid, PlaceError};

/// How many times a round gives starved thermal reliefs back their spokes, at most.
const RELIEFS: usize = 2;

/// How [`place_and_route`] places and routes.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FlowOptions {
    /// How parts are placed.
    pub place: PlaceOptions,
    /// How the board is routed.
    pub route: Options,
    /// How many times at most parts are placed again around what routing found. The default
    /// is 6; zero is the one-pass flow, placement then routing.
    pub rounds: usize,
    /// How many rounds in a row may fail to improve the board before the loop stops. The
    /// default is 3.
    pub patience: usize,
    /// What a via counts for against track, in millimetres, in judging a routing. The
    /// default is 3: a via saved is worth 3 mm more track.
    pub via_mm: f64,
    /// How far the parts of a round may go from where they and their nets are, as
    /// [`Feedback::reach`]. The default is 5 mm.
    pub reach: i64,
    /// Groups of pins whose nets may be exchanged, after placement: see
    /// [`pcb::swap`](crate::pcb::swap) and [`swap_groups`]. The default is none.
    pub swaps: Vec<SwapGroup>,
}

impl Default for FlowOptions {
    fn default() -> Self {
        FlowOptions {
            place: PlaceOptions::default(),
            route: Options::default(),
            rounds: 6,
            patience: 3,
            via_mm: 3.0,
            reach: 5_000_000,
            swaps: Vec::new(),
        }
    }
}

/// How a routing came out, as [`place_and_route`] judges it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Score {
    /// Connections needed, and made, over the whole board.
    pub connections: (usize, usize),
    /// Vias.
    pub vias: usize,
    /// Track length, in nanometres.
    pub track: f64,
    /// Thermal reliefs that the zones' fills join to their pads by a single spoke, which
    /// KiCad reports as `starved_thermal`: routes that crowd a pad of a plane's net.
    pub starved: usize,
}

impl Score {
    fn of(design: &Design, routed: &Routed) -> Score {
        let s = &routed.stats;
        let (vias, track) = eco::measure(&routed.tracks, &routed.vias);
        let mut board = design.board.clone();
        board.tracks.extend(routed.tracks.iter().cloned());
        board.vias.extend(routed.vias.iter().cloned());
        Score {
            connections: (s.connections, s.connected),
            vias,
            track,
            starved: pcb::starved_reliefs(&board, design).map_or(0, |s| s.len()),
        }
    }

    /// Whether this is better than `other`: fewer connections missing, or as few and fewer
    /// starved thermal reliefs, or as few of both and less track and vias, at `via_mm` a
    /// via.
    pub fn better(&self, other: &Score, via_mm: f64) -> bool {
        let missing = |s: &Score| s.connections.0.saturating_sub(s.connections.1);
        let cost = |s: &Score| s.track / 1e6 + via_mm * s.vias as f64;
        (missing(self), self.starved) < (missing(other), other.starved)
            || ((missing(self), self.starved) == (missing(other), other.starved)
                && cost(self) < cost(other) - 1e-6)
    }
}

/// One round of the loop.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Round {
    /// The parts that could move, and of those the parts that moved.
    pub window: (usize, usize),
    /// Parts put on the other side.
    pub flipped: usize,
    /// The routing it came to.
    pub score: Score,
    /// Whether it was kept, as better than the best before.
    pub kept: bool,
    /// The nets routed again.
    pub rerouted: usize,
    /// Seconds placing and routing took.
    pub seconds: (f64, f64),
}

/// A pad given another net by a swap, for the schematic to be annotated back from.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PadNetChange {
    /// The footprint, by position in [`KicadBoard::footprints`].
    pub footprint: usize,
    /// The pad, by position in the footprint's [pads](super::Footprint::pads).
    pub pad_index: usize,
    /// The net it has now.
    pub net: NetId,
    /// The footprint's reference.
    pub reference: String,
    /// The pad's number.
    pub pad: String,
    /// The net it had, by name.
    pub from: String,
    /// The net it has now.
    pub to: String,
}

/// What [`place_and_route`] did.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FlowReport {
    /// The board placed and routed, its zones filled.
    pub text: String,
    /// Where each footprint is.
    pub placed: Vec<Placed>,
    /// The routing, all of it.
    pub routed: Routed,
    /// The pads swaps gave other nets.
    pub swaps: Vec<PadNetChange>,
    /// The one-pass flow's routing, of the first placement.
    pub first: Score,
    /// The board as the one-pass flow left it, placed and routed, its zones filled.
    pub first_text: String,
    /// Where the one-pass flow placed each footprint.
    pub first_placed: Vec<Placed>,
    /// Seconds the first placement and its routing took.
    pub first_seconds: (f64, f64),
    /// The rounds after it.
    pub rounds: Vec<Round>,
}

/// Why [`place_and_route`] failed.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum FlowError {
    /// A board could not be read, as written or placed.
    Read(KicadError),
    /// The parts could not be placed.
    Place(PlaceError),
    /// The board could not be routed.
    Route(RouteError),
}

impl fmt::Display for FlowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlowError::Read(e) => write!(f, "reading: {e}"),
            FlowError::Place(e) => write!(f, "placing: {e}"),
            FlowError::Route(e) => write!(f, "routing: {e}"),
        }
    }
}

impl std::error::Error for FlowError {}

/// The safe automatic swap groups of a board's design, from [`placement_design`]: the two
/// pins of resistors, capacitors, inductors and ferrite beads, as
/// [`two_pin_groups`] finds them, but for polarized capacitors, whose library names say
/// `CP_`, `Polarized`, `Elec` or `Tantal`.
pub fn swap_groups(board: &KicadBoard, design: &PlacementDesign) -> Vec<SwapGroup> {
    two_pin_groups(design)
        .into_iter()
        .filter(|g| {
            board.footprints.get(g.component).is_some_and(|fp| {
                let lib = fp.library.to_ascii_lowercase();
                !["cp_", "polarized", "elec", "tantal"]
                    .iter()
                    .any(|s| lib.contains(s))
            })
        })
        .collect()
}

/// A swap group of a footprint's pads, by reference and pad numbers: the pins of the design
/// from [`placement_design`] that those pads are, those with a net.
pub fn swap_group(board: &KicadBoard, reference: &str, pads: &[&str]) -> Option<SwapGroup> {
    let component = board
        .footprints
        .iter()
        .position(|f| f.reference == reference)?;
    let fp = &board.footprints[component];
    let pins: Vec<usize> = fp
        .pads
        .iter()
        .filter(|p| p.net.is_some())
        .enumerate()
        .filter(|(_, p)| pads.contains(&p.number.as_str()))
        .map(|(k, _)| k)
        .collect();
    (pins.len() >= 2).then_some(SwapGroup { component, pins })
}

/// What swaps of the design from [`placement_design`] are in a board's terms: each pad that
/// changes nets, for [`replace_pad_nets`] to write, with [`PadNetChange::edit`], and for the
/// schematic to be annotated back from.
pub fn pad_changes(board: &KicadBoard, swaps: &[PinSwap]) -> Vec<PadNetChange> {
    let name = |n: NetId| board.nets.get(n.index()).cloned().unwrap_or_default();
    swaps
        .iter()
        .filter_map(|s| {
            let fp = board.footprints.get(s.component)?;
            let (k, pad) = fp
                .pads
                .iter()
                .enumerate()
                .filter(|(_, p)| p.net.is_some())
                .nth(s.pin)?;
            Some(PadNetChange {
                footprint: s.component,
                pad_index: k,
                net: s.to,
                reference: fp.reference.clone(),
                pad: pad.number.clone(),
                from: name(s.from),
                to: name(s.to),
            })
        })
        .collect()
}

impl PadNetChange {
    /// The change as [`replace_pad_nets`] takes it.
    pub fn edit(&self) -> (usize, usize, Option<NetId>) {
        (self.footprint, self.pad_index, Some(self.net))
    }
}

/// A placed board, ready to route: its text, as read, and its design.
struct Stage {
    placed: Vec<Placed>,
    text: String,
    read: KicadBoard,
    design: Design,
}

fn stage(
    pcb: &str,
    project: Option<&str>,
    board: &KicadBoard,
    placed: Vec<Placed>,
    pads: &[(usize, usize, Option<NetId>)],
) -> Result<Stage, FlowError> {
    let text = replace_placement(pcb, board, &placed);
    let text = if pads.is_empty() {
        text
    } else {
        replace_pad_nets(&text, board, pads)
    };
    let read = read(&text, project).map_err(FlowError::Read)?;
    let mut design = Design::from_kicad(&read);
    design.board.tracks.clear();
    design.board.vias.clear();
    Ok(Stage {
        placed,
        text,
        read,
        design,
    })
}

/// By net, its vias and track length, in nanometres.
fn per_net(routed: &Routed) -> BTreeMap<NetId, (usize, f64)> {
    let mut out: BTreeMap<NetId, (usize, f64)> = BTreeMap::new();
    for t in &routed.tracks {
        if let Some(n) = t.net {
            out.entry(n).or_default().1 += eco::measure(core::slice::from_ref(t), &[]).1;
        }
    }
    for v in &routed.vias {
        if let Some(n) = v.net {
            out.entry(n).or_default().0 += 1;
        }
    }
    out
}

/// A map of how much of the board routing used: each track's length spread along it, and a
/// via as `via_mm` of track, against half of what the layers carry, as placement's RUDY map.
fn used(design: &Design, pd: &PlacementDesign, routed: &Routed, via_mm: f64) -> CongestionGrid {
    let bounds = design
        .outline
        .iter()
        .map(crate::geom::Shape::bbox)
        .chain(design.board.pads.iter().map(|p| p.shape.bbox()))
        .reduce(|a, b| a.union(&b))
        .unwrap_or_default()
        .expand(2_000_000);
    let bin = 1_000_000;
    let pitch = pd.track_pitch.max(1) as f64;
    let capacity = pd.layers.max(1) as f64 * (bin as f64 / pitch) * bin as f64 * 0.5;
    let area = crate::place::Rect::new(
        crate::place::Point::new(bounds.min().x, bounds.min().y),
        crate::place::Point::new(bounds.max().x, bounds.max().y),
    );
    let mut grid = CongestionGrid::new(area, bin, capacity);
    let step = 250_000;
    for t in &routed.tracks {
        let length = eco::measure(core::slice::from_ref(t), &[]).1;
        let n = ((length / step as f64) as i64).max(1);
        for k in 0..n {
            let p = Point::new(
                t.start.x + (t.end.x - t.start.x) * (2 * k + 1) / (2 * n),
                t.start.y + (t.end.y - t.start.y) * (2 * k + 1) / (2 * n),
            );
            grid.add_demand_at(crate::place::Point::new(p.x, p.y), length / n as f64);
        }
    }
    for v in &routed.vias {
        grid.add_demand_at(crate::place::Point::new(v.at.x, v.at.y), via_mm * 1e6);
    }
    grid
}

/// The half-perimeter of a net's pads' centres.
fn pad_spans(design: &Design) -> BTreeMap<NetId, Rect> {
    let mut out: BTreeMap<NetId, Rect> = BTreeMap::new();
    for p in &design.board.pads {
        if let Some(n) = p.net {
            let b = p.shape.bbox();
            let c = Point::new((b.min().x + b.max().x) / 2, (b.min().y + b.max().y) / 2);
            out.entry(n)
                .and_modify(|r| *r = r.union(&Rect::new(c, c)))
                .or_insert(Rect::new(c, c));
        }
    }
    out
}

/// What a routing says to place again: the parts on the nets that cost most, beyond their
/// half-perimeter, by their vias and detours, or that failed; their nets weighed more; and
/// those whose connections are mostly on the other side, to try there.
fn feedback(
    pd: &PlacementDesign,
    stage: &Stage,
    routed: &Routed,
    options: &FlowOptions,
    share: f64,
) -> Feedback {
    let planes: BTreeSet<NetId> = pd.planes.iter().map(|&(n, _)| n).collect();
    let spans = pad_spans(&stage.design);
    let nets = per_net(routed);
    let failed: BTreeSet<NetId> = routed.unrouted.iter().copied().collect();
    // By net, what it costs beyond its half-perimeter, in millimetres.
    let mut excess: BTreeMap<NetId, f64> = BTreeMap::new();
    for (&net, r) in &spans {
        if planes.contains(&net) {
            continue;
        }
        let hpwl = ((r.max().x - r.min().x) + (r.max().y - r.min().y)) as f64 / 1e6;
        let (vias, length) = nets.get(&net).copied().unwrap_or_default();
        let mut e = options.via_mm * vias as f64 + (length / 1e6 - hpwl).max(0.0);
        if failed.contains(&net) {
            e += 10.0 * options.via_mm + hpwl;
        }
        if e > 0.05 {
            excess.insert(net, e);
        }
    }
    let comps = &pd.components;
    let movable = |c: usize| !comps[c].fixed && !comps[c].through;
    let mut badness: Vec<(f64, usize)> = comps
        .iter()
        .enumerate()
        .filter(|&(c, _)| movable(c))
        .map(|(c, comp)| {
            let mut seen: Vec<NetId> = comp.pins.iter().map(|p| p.net).collect();
            seen.sort_unstable();
            seen.dedup();
            let b: f64 = seen.iter().filter_map(|n| excess.get(n)).sum();
            (b, c)
        })
        .filter(|&(b, _)| b > 0.0)
        .collect();
    badness.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let count = comps
        .iter()
        .enumerate()
        .filter(|&(c, _)| movable(c))
        .count();
    let cap = ((count as f64 * share).ceil() as usize).max(1);
    let mut window: Vec<usize> = badness.iter().take(cap).map(|&(_, c)| c).collect();
    window.sort_unstable();
    // Sides: a part goes where most of its connections are, by its nets of a few pins.
    let side_of = |c: usize| stage.placed.get(c).map_or(comps[c].side, |p| p.side);
    let mut by_net: BTreeMap<NetId, Vec<usize>> = BTreeMap::new();
    for (c, comp) in comps.iter().enumerate() {
        for p in &comp.pins {
            if !planes.contains(&p.net) {
                by_net.entry(p.net).or_default().push(c);
            }
        }
    }
    let mut flips = Vec::new();
    for &c in &window {
        let mine = side_of(c);
        let mut hold = 0i64;
        for p in &comps[c].pins {
            let Some(others) = by_net.get(&p.net).filter(|o| o.len() <= 12) else {
                continue;
            };
            for &d in others.iter().filter(|&&d| d != c) {
                if comps[d].through {
                    continue;
                }
                hold += if side_of(d) == mine { 1 } else { -1 };
            }
        }
        if hold < 0 {
            flips.push(c);
        }
    }
    let mut weights: Vec<(NetId, u32)> = excess
        .iter()
        .map(|(&n, &e)| {
            let r = spans.get(&n).map_or(1.0, |r| {
                (((r.max().x - r.min().x) + (r.max().y - r.min().y)) as f64 / 1e6).max(1.0)
            });
            (n, (100.0 + 100.0 * (e / r).min(3.0)) as u32)
        })
        .collect();
    weights.sort_unstable();
    Feedback {
        window,
        weights,
        congestion: Some(used(&stage.design, pd, routed, options.via_mm)),
        flips,
        reach: options.reach,
    }
}

/// Places a KiCad board's movable footprints, swaps pins, routes it, and then, round after
/// round while it pays, places again the parts that routing found most trouble with, and
/// reroutes only what that moved.
///
/// 1. [`place_components`] places the board, from [`placement_design`].
/// 2. With [`FlowOptions::swaps`], [`optimize_swaps`] exchanges pins' nets within their
///    groups, and the pads get their new nets, in the text too.
/// 3. [`pcb::route`] routes it: the one-pass flow, [`FlowReport::first`].
/// 4. Each round, the nets that cost most beyond their half-perimeter, in vias and detours,
///    and those left unrouted, choose the parts to place again, a share of the movable ones
///    that shrinks each round; their nets weigh more, routing's use of the board prices
///    congestion, and a part whose connections are mostly on the other side goes there.
///    [`refine_components`] places them, near where they were, around everything else, and
///    [`eco::reroute`] routes again what moved, keeping the rest. The result is kept if it
///    is [better](Score::better). The loop stops after [`FlowOptions::rounds`] rounds, or
///    [`FlowOptions::patience`] in a row that keep nothing.
///
/// # Errors
///
/// If a board can't be read, placed, or routed.
pub fn place_and_route(
    pcb: &str,
    project: Option<&str>,
    options: &FlowOptions,
) -> Result<FlowReport, FlowError> {
    let board = read(pcb, project).map_err(FlowError::Read)?;
    let mut pd = placement_design(&board);
    let start = Instant::now();
    let report = place_components(&pd, &options.place).map_err(FlowError::Place)?;
    let spacing = report.spacing;
    let swaps = if options.swaps.is_empty() {
        Vec::new()
    } else {
        optimize_swaps(&pd, &report.placed, &options.swaps)
    };
    apply_swaps(&mut pd, &swaps);
    let changes = pad_changes(&board, &swaps);
    let pads: Vec<(usize, usize, Option<NetId>)> = changes.iter().map(PadNetChange::edit).collect();
    let place_seconds = start.elapsed().as_secs_f64();
    let first_placed = report.placed.clone();
    let mut best = stage(pcb, project, &board, report.placed, &pads)?;
    let start = Instant::now();
    let mut routed = pcb::route(&best.design, &options.route).map_err(FlowError::Route)?;
    // Counted over the whole board, as the loop's are.
    let all = routed.board(&best.design);
    let (nets, done, needed, made, _) = pcb::connectivity(&best.design, &all);
    (
        routed.stats.nets,
        routed.stats.routed,
        routed.stats.connections,
        routed.stats.connected,
    ) = (nets, done, needed, made);
    let route_seconds = start.elapsed().as_secs_f64();
    let first = Score::of(&best.design, &routed);
    let first_text = replace_routing_and_fills(
        &best.text,
        &routed.tracks,
        &routed.vias,
        &routed.fills,
        &best.read,
    );
    let mut score = first;
    let mut rounds = Vec::new();
    let mut misses = 0;
    let mut share = 0.4;
    for round in 0..options.rounds {
        if misses >= options.patience {
            break;
        }
        let start = Instant::now();
        let fb = feedback(&pd, &best, &routed, options, share);
        if fb.window.is_empty() {
            break;
        }
        let mut place = options.place.clone();
        place.seed = options.place.seed.wrapping_add(round as u64 + 1);
        let refined = match refine_components(&pd, &best.placed, spacing, &fb, &place) {
            Ok(r) => Ok(r),
            Err(PlaceError::NoRoom(_)) if !fb.flips.is_empty() => {
                let mut fb = fb.clone();
                fb.flips.clear();
                refine_components(&pd, &best.placed, spacing, &fb, &place)
            }
            Err(e) => Err(e),
        };
        let refined = match refined {
            Ok(r) => r,
            Err(PlaceError::NoRoom(_)) => {
                misses += 1;
                share /= 2.0;
                continue;
            }
            Err(e) => return Err(FlowError::Place(e)),
        };
        let moved: Vec<usize> = (0..pd.components.len())
            .filter(|&c| refined.placed[c] != best.placed[c])
            .collect();
        let flipped = moved
            .iter()
            .filter(|&&c| refined.placed[c].side != best.placed[c].side)
            .count();
        if moved.is_empty() {
            misses += 1;
            share /= 2.0;
            continue;
        }
        let next = stage(pcb, project, &board, refined.placed, &pads)?;
        let place_seconds = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let planes: BTreeSet<NetId> = pd.planes.iter().map(|&(n, _)| n).collect();
        let mut nets: Vec<NetId> = moved
            .iter()
            .flat_map(|&c| pd.components[c].pins.iter().map(|p| p.net))
            .filter(|n| !planes.contains(n))
            .collect();
        nets.sort_unstable();
        nets.dedup();
        let areas: Vec<Rect> = moved
            .iter()
            .flat_map(|&c| {
                [
                    footprint_outline(&best.read, c),
                    footprint_outline(&next.read, c),
                ]
            })
            .flatten()
            .collect();
        let change = Change::new(nets, areas).after(&routed.unrouted);
        let mut rerouted = eco::reroute(
            &next.design,
            &routed.tracks,
            &routed.vias,
            &change,
            &options.route,
        )
        .map_err(FlowError::Route)?;
        let mut s = Score::of(&next.design, &rerouted.routed);
        // Reliefs that routing starved of spokes get them back, if that pays.
        for _ in 0..RELIEFS {
            if s.starved == 0 {
                break;
            }
            let Some(r) = eco::relieve(
                &next.design,
                &rerouted.routed.tracks,
                &rerouted.routed.vias,
                &options.route,
            )
            .map_err(FlowError::Route)?
            else {
                break;
            };
            let t = Score::of(&next.design, &r.routed);
            if !t.better(&s, options.via_mm) {
                break;
            }
            s = t;
            rerouted.routed = r.routed;
            rerouted.rerouted.extend(r.rerouted);
        }
        let kept = s.better(&score, options.via_mm);
        rounds.push(Round {
            window: (fb.window.len(), moved.len()),
            flipped,
            score: s,
            kept,
            rerouted: rerouted.rerouted.len(),
            seconds: (place_seconds, start.elapsed().as_secs_f64()),
        });
        if kept {
            score = s;
            best = next;
            routed = rerouted.routed;
            misses = 0;
        } else {
            misses += 1;
            share /= 2.0;
        }
    }
    let text = replace_routing_and_fills(
        &best.text,
        &routed.tracks,
        &routed.vias,
        &routed.fills,
        &best.read,
    );
    Ok(FlowReport {
        text,
        placed: best.placed,
        routed,
        swaps: changes,
        first,
        first_text,
        first_placed,
        first_seconds: (place_seconds, route_seconds),
        rounds,
    })
}
