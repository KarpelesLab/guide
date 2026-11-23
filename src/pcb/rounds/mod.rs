//! The negotiation of cadlab's own router, as a first stage for a run with a budget
//! ([`Rounds::Native`](super::Rounds)).
//!
//! [`whole`](super::whole) routes every net against every other on one grid and asks the
//! [`World`](super::world::World) whether a place is legal. This stage is the other
//! router's negotiation itself, with its own view of the board ([`model`]), made from the
//! [`Design`]: the grid at half a track and its clearance, snapped to the pitch of the rows
//! of fine pitch and aligned with their pads ([`make_grid`]); what the board's copper leaves
//! free of it as maps by class of net ([`grid::Statics`]); the nets as islands of copper to
//! join, the pads that a fill joins being one, in a tree from the first by the distance as
//! the crow flies ([`plan`]); escapes and dog-bones for the rows and arrays of fine pitch
//! before anything is routed ([`fanout`]); the cells where a pad is reached, on its copper at
//! no cost or by a stub ([`engine::Engine::access`]); the search and its prices
//! ([`engine::Engine::search`]); the nets in the order of their length, those near a shared
//! point routed again each round, in batches of nets whose regions do not meet; a
//! connection that may end on any cell of a fill of the island it is to join, near its end;
//! the price of a shared point 0.6 at first and 1.6 times that each round, and half a step
//! of history for each point shared; forty rounds, or twelve with none better; and what is
//! still shared then taken out, the net with the most shared points first, and routed again
//! where nothing is claimed. With a host that fills its zones around the routes
//! ([`RunControl::refill`](super::RunControl)), the nets whose pads the routes cut off their
//! fills are planned again from what the fills still join and negotiated for again, three
//! times at most, and the routes go back where that leaves more open.
//!
//! What comes out is each net's route as a tree ([`Whole`]), for the passes, and with it the
//! places on fills and fixed copper where its routes end.

mod components;
mod engine;
mod fanout;
mod geo;
mod grid;
mod index;
mod model;
mod pours;
mod scoped;
mod work;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use self::engine::{
    Access, Conn, Costs, Engine, FailKind, Island, Mode, NetRoute, Rng, Scratch, Stop, Wire,
};
use self::geo::{BoxF, Nm, P};
use self::index::{Checker, Index, Item};
use self::model::{Copper, ObKind, RouterBoard, What};
use self::work::{Limit, WorkMeter};
use super::control::Run;
use super::detail::{DNode, TermPad, Tree};
use super::router::Job;
use super::whole::{NoWhole, Whole};
use super::{Design, Fill, RouteOptions};
use crate::geom::drc::{Board, Track, Via};
use crate::geom::{LayerId, LayerSet, Point, Rect};
use crate::id::NetId;

/// Largest present-congestion factor of negotiation.
const PRES_MAX: f32 = 1000.0;

/// Negotiation stops when the best round is this many rounds old.
const STAGNATION: usize = 12;

/// Rounds of rip-up and retry after the routes are made legal.
const RETRY_ROUNDS: usize = 6;

/// Retries kept although they only move a failure to another net.
const SIDEWAYS: usize = 4;

/// How many times its first round the negotiation has to have room for in its share of the
/// run, in time and in nodes, to be begun: the rounds after the first are cheaper, and a
/// board's take some thirty of them to end (`tinytapeout-demo` 24 to 40, `corne-cherry` 33,
/// `buspirate5-rev10` 29), in six to eleven times what the first took. One whose share ends
/// before that has routes still all over each other (`glasgow-revD1`, six layers: five
/// rounds in its 20 s, with 1100 points shared), which cost the passes more than they give
/// them: such a board is given up when the first round has said so.
const ROOM: f64 = 8.0;

/// Rounds that their share ends with fewer points shared than one for every this many
/// connections are near their end.
const NEARLY: usize = 4;

/// And how many connections there are to be for each that has no way on the grid whatever
/// the other nets do, a pad that no cell of the grid reaches or that the board's own copper
/// shuts in at this pitch: the balls of an array finer than the grid are such (`cynthion`
/// and `glasgow-revC3`, one connection in twenty), and the passes, which ask the board about
/// each place exactly, then have them all to make on a board that is full.
const NO_WAY: usize = 50;

/// Via cost of [`minimize_vias`] (grid steps; negotiation uses `Costs::via`).
const MIN_VIA_COST: f32 = 40.0;

/// Most extra length (share of the old length, plus `MIN_VIA_SLACK` grid steps per via saved)
/// [`minimize_vias`] accepts.
const MIN_VIA_STRETCH: f64 = 0.1;
const MIN_VIA_SLACK: f64 = 8.0;

/// The most rounds of a negotiation.
const ROUNDS: usize = 40;

/// Fewest connections of a net for partial rip-up in negotiation: such a net, when it is
/// rerouted for being near an overused point and has none of its own, keeps the wires whose
/// box, grown by the clearance reach and [`PARTIAL_MARGIN`], holds no overused point, and
/// routes only the rest. A net in conflict is routed again whole.
const PARTIAL_MIN: usize = 64;

/// Margin (grid pitches) beyond the clearance reach within which an overused point rips a
/// wire of a net routed partially.
const PARTIAL_MARGIN: f64 = 10.0;

/// The share of the stage's time and work in which its rounds end: the rest is for making
/// the routes legal.
const NEGOTIATE: f64 = 0.9;

/// Most times negotiation starts again for nets whose fills the routing cut apart.
const POUR_NEGOTIATIONS: usize = 3;

/// Share of the stage at which a first negotiation that is still going looks at what it
/// cuts off from the pours, and ends there when that is one pad per [`POUR_PAUSE`]
/// connections or more: the pour rounds then get the rest of its share.
const POUR_FIRST: f64 = 0.66;
const POUR_PAUSE: usize = 100;

/// Margin around a net's terminals for its batching region (nm), plus a quarter of its extent.
const REGION_MARGIN: f64 = 1_500_000.0;

/// Largest coarsening of the grid pitch that snaps it to a fine pad pitch (factor).
const GRID_SNAP: f64 = 1.06;

/// The best negotiation state: (overused points, wires per net, failures per net).
type Snapshot = (usize, Vec<Vec<Wire>>, Vec<Vec<Option<FailKind>>>);

/// The search's prices.
fn costs() -> Costs {
    Costs {
        via: 10.0,
        bend45: 0.4,
        bend90: 1.5,
        wrong_way: 1.8,
        diag: 1.05,
        weight: 1.0,
    }
}

/// The smallest via the rules allow, as (diameter, drill): the minimum drill with the minimum
/// annular ring around it.
pub(crate) fn smallest_via(rules: &model::BoardRules) -> (Nm, Nm) {
    let drill = rules.min_drill.max(Nm(1_000));
    (Nm(drill.0 + 2 * rules.min_annular_ring.0), drill)
}

/// Largest extra distance a check adds beyond an item's own extent.
pub(crate) fn reach(rb: &RouterBoard) -> f64 {
    let mut r = rb.rules.copper_to_edge.0 as f64;
    if rb.scoped.on || rb.scoped.hc > 0.0 {
        // Scoped rules: any rule's clearance, and holes keeping the hole clearance.
        let c = rb.max_clear();
        for p in &rb.profiles {
            r = r
                .max(c + p.hw)
                .max(rb.via_out(p, c))
                .max(rb.h2h + 2.0 * p.dr);
        }
        return r.max(c) + 2.0 * index::TOL;
    }
    for p in &rb.profiles {
        r = r.max(p.c + p.hw.max(p.rv)).max(rb.h2h + 2.0 * p.dr);
    }
    r.max(rb.rules.clearance) + 2.0 * index::TOL
}

/// Routes a batch of nets whose regions do not meet: every net is ripped up, all are routed
/// against the same occupancy, on `threads` threads, then committed in batch order. The
/// outcome does not depend on the number of threads. When the budget stops a search, the
/// whole batch is put back as it was.
fn route_batch(
    eng: &mut Engine<'_>,
    routes: &mut [NetRoute],
    batch: &[usize],
    mode: Mode,
    pool: &Mutex<Vec<Scratch>>,
    threads: usize,
) -> Result<(), Stop> {
    let saved: Vec<(Vec<Wire>, Vec<Option<FailKind>>)> = batch
        .iter()
        .map(|&ri| (routes[ri].wires.clone(), routes[ri].failed.clone()))
        .collect();
    for &ri in batch {
        eng.uncommit(&mut routes[ri]);
    }
    let taken: Vec<Mutex<(NetRoute, Result<(), Stop>)>> = batch
        .iter()
        .map(|&ri| Mutex::new((std::mem::take(&mut routes[ri]), Ok(()))))
        .collect();
    // Within a batch every search sees the same work count, whatever the threads do.
    eng.work.begin_batch();
    {
        let e: &Engine<'_> = eng;
        let next = AtomicUsize::new(0);
        let work = || {
            let mut sc = pool
                .lock()
                .expect("scratch pool")
                .pop()
                .unwrap_or_else(|| e.scratch());
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(slot) = taken.get(i) else { break };
                let mut slot = slot.lock().expect("net");
                let (nr, res) = &mut *slot;
                *res = e.route_net(&mut sc, nr, mode);
            }
            pool.lock().expect("scratch pool").push(sc);
        };
        let n = threads.max(1).min(taken.len());
        if n > 1 {
            std::thread::scope(|scope| {
                for _ in 1..n {
                    scope.spawn(work);
                }
                work();
            });
        } else {
            work();
        }
    }
    eng.work.end_batch();
    let taken: Vec<(NetRoute, Result<(), Stop>)> = taken
        .into_iter()
        .map(|m| m.into_inner().expect("net"))
        .collect();
    let stopped = taken.iter().any(|(_, r)| r.is_err());
    for ((&ri, (mut nr, _)), (w, f)) in batch.iter().zip(taken).zip(saved) {
        if stopped {
            nr.wires = w;
            nr.failed = f;
        }
        routes[ri] = nr;
        eng.commit(&mut routes[ri]);
    }
    if stopped { Err(Stop) } else { Ok(()) }
}

/// Via minimization: every net with vias is ripped and routed again, without sharing, with
/// vias four times as dear; the new routing is kept when it has fewer vias, no more failed
/// connections and is at most a little longer. Other nets do not move, so the result stays
/// legal. Returns the number of vias saved.
fn minimize_vias(
    eng: &mut Engine<'_>,
    sc: &mut Scratch,
    routes: &mut [NetRoute],
    stop: &dyn Fn() -> bool,
) -> usize {
    let mut saved = 0;
    let base = eng.costs;
    for r in routes.iter_mut() {
        if stop() {
            break;
        }
        let (vias, len) = wire_stats(eng, &r.wires);
        if vias == 0 {
            continue;
        }
        let fails = |r: &NetRoute| r.failed.iter().filter(|f| f.is_some()).count();
        let before = fails(r);
        let old = (r.wires.clone(), r.failed.clone());
        eng.uncommit(r);
        eng.costs = Costs {
            via: MIN_VIA_COST,
            ..base
        };
        let res = eng.route_net(sc, r, Mode::Hard);
        eng.costs = base;
        let (nv, nl) = wire_stats(eng, &r.wires);
        let ok = res.is_ok()
            && fails(r) <= before
            && nv < vias
            && nl
                <= len * (1.0 + MIN_VIA_STRETCH) + MIN_VIA_SLACK * eng.grid.g * (vias - nv) as f64;
        if ok {
            saved += vias - nv;
        } else {
            r.wires = old.0;
            r.failed = old.1;
        }
        eng.commit(r);
    }
    saved
}

/// The routes made legal a wire at a time: while any wire shares a point with another net's,
/// the one that shares the most is taken out (of two that share as many, the longer, which
/// has more other ways), and then each net that lost one is routed again where nothing is
/// claimed, with the wires it kept. cadlab's own router takes the whole net out that shares
/// the most, and that is the net with the most connections, whose other wires were in no
/// one's way and find theirs taken when they are searched for again (`tinytapeout-demo`:
/// eight connections of one net open for two nets taken out). Returns how many wires were
/// taken out, and whether every search ended.
fn legalise_wires(
    eng: &mut Engine<'_>,
    sc: &mut Scratch,
    routes: &mut [NetRoute],
) -> (usize, bool) {
    let mut lost: BTreeSet<usize> = BTreeSet::new();
    let mut taken = 0;
    // The nets with a shared point: only their wires are looked at, and a net is looked at
    // again when a wire is taken out.
    let mut hot: Vec<usize> = (0..routes.len())
        .filter(|&ri| !eng.conflicts(&routes[ri]).is_empty())
        .collect();
    let mut at: Vec<engine::Conflict> = Vec::new();
    while !hot.is_empty() {
        let mut worst: Option<(usize, usize, usize, usize)> = None;
        for &ri in &hot {
            let r = &routes[ri];
            for (wi, w) in r.wires.iter().enumerate() {
                at.clear();
                eng.wire_conflicts(r, w, &mut at);
                at.sort_unstable();
                at.dedup();
                if at.is_empty() {
                    continue;
                }
                let key = (at.len(), w.nodes.len());
                if worst.is_none_or(|b| key > (b.0, b.1)) {
                    worst = Some((key.0, key.1, ri, wi));
                }
            }
        }
        let Some((_, _, ri, wi)) = worst else { break };
        let r = &mut routes[ri];
        eng.uncommit(r);
        r.wires.remove(wi);
        eng.commit(r);
        lost.insert(ri);
        taken += 1;
        hot.retain(|&ri| !eng.conflicts(&routes[ri]).is_empty());
    }
    let mut order: Vec<usize> = lost.into_iter().collect();
    order.sort_by(|a, b| {
        routes[*a]
            .length
            .total_cmp(&routes[*b].length)
            .then(a.cmp(b))
    });
    let mut ended = true;
    for ri in order {
        ended &= route_kept(eng, sc, &mut routes[ri]);
    }
    (taken, ended)
}

/// Routes again, where nothing is claimed, the connections of `r` that its wires do not
/// make, the wires staying as they are. Whether the search ended.
fn route_kept(eng: &mut Engine<'_>, sc: &mut Scratch, r: &mut NetRoute) -> bool {
    eng.uncommit(r);
    r.keep = vec![true; r.wires.len()];
    let res = eng.route_net(sc, r, Mode::Hard);
    r.keep.clear();
    eng.commit(r);
    if res.is_err() {
        for (ci, f) in r.failed.iter_mut().enumerate() {
            if f.is_none() && !r.wires.iter().any(|w| w.conn == ci) {
                *f = Some(FailKind::Budget);
            }
        }
    }
    res.is_ok()
}

/// A connection that found no way where nothing is claimed takes the room of the wires in
/// its way, a wire at a time: those are taken out, it is routed, and their nets are routed
/// again around it with the wires they kept; kept when fewer connections fail, or, a few
/// times, when as many do and the failure moved to another net. Returns how many were kept.
fn retry_wires(
    eng: &mut Engine<'_>,
    sc: &mut Scratch,
    routes: &mut [NetRoute],
    stop: &dyn Fn() -> bool,
) -> usize {
    let mut kept = 0;
    let mut sideways = SIDEWAYS;
    for _ in 0..RETRY_ROUNDS {
        let mut improved = false;
        let failing: Vec<usize> = (0..routes.len())
            .filter(|&ri| routes[ri].failed.contains(&Some(FailKind::Congestion)))
            .collect();
        for fi in failing {
            let cis: Vec<usize> = (0..routes[fi].conns.len())
                .filter(|&ci| routes[fi].failed[ci] == Some(FailKind::Congestion))
                .collect();
            for ci in cis {
                if stop() {
                    return kept;
                }
                if routes[fi].failed[ci] != Some(FailKind::Congestion) {
                    continue;
                }
                let Ok(keys) = eng.blocking_keys(sc, &routes[fi], ci) else {
                    return kept;
                };
                // The wires in the way, by net.
                let mut blockers: Vec<(usize, Vec<usize>)> = Vec::new();
                for (j, r) in routes.iter().enumerate() {
                    if j == fi || !keys.iter().any(|k| r.keys.binary_search(k).is_ok()) {
                        continue;
                    }
                    let wires: Vec<usize> = r
                        .wires
                        .iter()
                        .enumerate()
                        .filter(|(_, w)| {
                            let wk = eng.wire_keys(r.net, w);
                            keys.iter().any(|k| wk.binary_search(k).is_ok())
                        })
                        .map(|(wi, _)| wi)
                        .collect();
                    if !wires.is_empty() {
                        blockers.push((j, wires));
                    }
                }
                if blockers.is_empty() || blockers.len() > 8 {
                    continue;
                }
                let group: Vec<usize> = std::iter::once(fi)
                    .chain(blockers.iter().map(|b| b.0))
                    .collect();
                let fails = |routes: &[NetRoute]| -> usize {
                    group
                        .iter()
                        .map(|&j| routes[j].failed.iter().filter(|f| f.is_some()).count())
                        .sum()
                };
                let before = fails(routes);
                let saved: Vec<(Vec<Wire>, Vec<Option<FailKind>>)> = group
                    .iter()
                    .map(|&j| (routes[j].wires.clone(), routes[j].failed.clone()))
                    .collect();
                for (j, wires) in &blockers {
                    let r = &mut routes[*j];
                    eng.uncommit(r);
                    for &wi in wires.iter().rev() {
                        r.wires.remove(wi);
                    }
                    eng.commit(r);
                }
                let mut ok = true;
                for &j in &group {
                    ok &= route_kept(eng, sc, &mut routes[j]);
                }
                let after = fails(routes);
                let moved = after == before && routes[fi].failed[ci].is_none() && sideways > 0;
                if ok && (after < before || moved) {
                    sideways -= usize::from(moved);
                    improved = true;
                    kept += 1;
                    continue;
                }
                for (&j, (w, f)) in group.iter().zip(saved) {
                    eng.uncommit(&mut routes[j]);
                    routes[j].wires = w;
                    routes[j].failed = f;
                    eng.commit(&mut routes[j]);
                }
                if !ok {
                    return kept;
                }
            }
        }
        if !improved {
            break;
        }
    }
    kept
}

/// Vias and track length (nm) of wires.
fn wire_stats(eng: &Engine<'_>, wires: &[Wire]) -> (usize, f64) {
    let mut vias = 0;
    let mut len = 0.0;
    for w in wires {
        let (segs, v) = eng.wire_geometry(w);
        vias += v.len();
        len += segs.iter().map(|(_, a, b)| a.dist(*b)).sum::<f64>();
    }
    (vias, len)
}

/// By net, the width to route it at if that is narrower than its own: the widest at which
/// each pad of its is left between the other pads of its part, and at which two tracks out
/// of two pads of a row of fine pitch clear each other as the grid has them (`fanout::necks`,
/// cadlab's rule). On the board's pads and edge alone, not its tracks and vias.
pub(crate) fn necks(design: &Design) -> Vec<Option<i64>> {
    let layers = design.rules.layer_count();
    let mut out = vec![None; design.nets.len()];
    if layers == 0 || design.outline.is_empty() {
        return out;
    }
    let slots: Vec<usize> = (0..layers).collect();
    let items = model::copper(&design.board, design.rules.layers());
    let isl: Vec<usize> = (0..items.len()).collect();
    let rb = RouterBoard::build(design, &design.board, &items, &[], &isl, &|_| false);
    let nets: Vec<u32> = (0..design.nets.len())
        .filter(|&i| design.nets[i].route)
        .map(|i| i as u32)
        .collect();
    let mut index = Index::new(rb.bbox.expand(1_000_000.0), 500_000.0, reach(&rb));
    for (i, ob) in rb
        .obstacles
        .iter()
        .enumerate()
        .filter(|(_, o)| o.kind != ObKind::Copper)
    {
        index.insert(Item::Static(i as u32), ob.bbox);
    }
    for (net, w) in fanout::necks(&rb, &index, &slots, &nets) {
        let w = w.round() as i64;
        if w < rb.profile(net).key.width.0 {
            out[net as usize] = Some(w);
        }
    }
    out
}

/// The routing grid: pitch (track width + clearance) / 2 of the finest routed profile,
/// aligned with the routed pads. Fanout is planned on the same grid.
fn make_grid(rb: &RouterBoard, routes: &[NetRoute]) -> grid::Grid {
    let pitch = routes
        .iter()
        .map(|r| {
            let k = rb.profiles[r.prof].key;
            (k.width.0 + k.clear.0) as f64 / 2.0
        })
        .fold(f64::MAX, f64::min);
    let mut pitch = if pitch.is_finite() {
        pitch.round().max(25_000.0)
    } else {
        225_000.0
    };
    // A grid commensurate with the dominant fine pad pitch: every pad of such a row then sits
    // at the same offset from the grid, so their escapes stay a pad pitch apart (with a drifting
    // offset, two neighbors end up one grid line apart somewhere along each row).
    if let Some(snapped) = snap_pitch(rb, routes, pitch) {
        pitch = snapped;
    }
    let (offx, offy) = alignment(rb, routes, pitch);
    grid::Grid::new(rb.bbox, pitch, offx, offy)
}

/// The routed pads of fine-pitch rows: single-layer pads of multi-pin parts with no room for a
/// track of their net between them and their nearest pad of the same part (as escapes see
/// them; two-pad passives are no rows). Per pad: (terminal, center distance to that
/// neighbor, whether the row runs along x).
fn fine_row_pads(rb: &RouterBoard, routes: &[NetRoute]) -> Vec<(usize, f64, bool)> {
    let mut by_comp: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (ti, t) in rb.terminals.iter().enumerate() {
        if let Some((comp, _)) = t.label.split_once('.')
            && t.pad
        {
            by_comp.entry(comp).or_default().push(ti);
        }
    }
    let mut routed = vec![false; rb.terminals.len()];
    for r in routes {
        for isl in &r.islands {
            for &t in &isl.terms {
                routed[t] = true;
            }
        }
    }
    let mut out = Vec::new();
    for terms in by_comp.values().filter(|t| t.len() >= 4) {
        for &t in terms {
            let term = &rb.terminals[t];
            if !routed[t] || term.layers.count_ones() != 1 {
                continue;
            }
            let Some((d, o)) = terms
                .iter()
                .filter(|&&o| o != t)
                .map(|&o| (rb.terminals[o].at.dist(term.at), o))
                .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            else {
                continue;
            };
            let (a, b) = (term.shape.bbox(), rb.terminals[o].shape.bbox());
            let (dx, dy) = (
                (term.at.x - rb.terminals[o].at.x).abs(),
                (term.at.y - rb.terminals[o].at.y).abs(),
            );
            // Gap between the two pads along the line joining them (axis-aligned rows).
            let gap = if dx >= dy {
                dx - (a.max.x - a.min.x) / 2.0 - (b.max.x - b.min.x) / 2.0
            } else {
                dy - (a.max.y - a.min.y) / 2.0 - (b.max.y - b.min.y) / 2.0
            };
            let pr = rb.profile(term.net);
            if gap < 2.0 * pr.hw + 2.0 * rb.c_pad_near(t) {
                out.push((t, d, dx >= dy));
            }
        }
    }
    out
}

/// The grid pitch snapped to a fine pad pitch: `p / 2m` for the pad pitch `p` the most pads
/// share (ties: the smaller) among those for which such a pitch lies in `[g, g × GRID_SNAP]`.
/// Pad pitches are center distances (rounded to µm) from a routed single-layer pad to its
/// nearest pad of the same part, counted when no track of the net passes between the two
/// (fine-pitch rows, [`fine_row_pads`]); at least 8 pads must share one.
fn snap_pitch(rb: &RouterBoard, routes: &[NetRoute], g: f64) -> Option<f64> {
    let mut count: BTreeMap<i64, usize> = BTreeMap::new();
    for (_, d, _) in fine_row_pads(rb, routes) {
        *count.entry((d / 1_000.0).round() as i64).or_default() += 1;
    }
    let snap = |p: f64| {
        let m = (p / (2.0 * g)).floor();
        (m >= 1.0 && p / (2.0 * m) <= g * GRID_SNAP).then(|| (p / (2.0 * m)).round())
    };
    count
        .into_iter()
        .filter(|&(um, n)| n >= 8 && snap(um as f64 * 1_000.0).is_some())
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .and_then(|(um, _)| snap(um as f64 * 1_000.0))
}

/// Grid offset per axis: the one that puts the most pads of the finest rows running along
/// that axis on grid lines, then the most routed pad centers on cell centers. A pad of such a
/// row leaves along its own line; off the grid lines it has to turn onto one within the row,
/// and of a row of turning exits some pads get none. Other pads are reached by a stub from
/// any cell, so they only break ties.
fn alignment(rb: &RouterBoard, routes: &[NetRoute], g: f64) -> (f64, f64) {
    // Offset (µm) → (fine-pitch row pads on its lines, routed pads).
    let mut cx: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
    let mut cy: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
    let key = |v: f64, min: f64| ((v - min).rem_euclid(g) / 1_000.0).round() as i64;
    for r in routes {
        for isl in &r.islands {
            for &t in isl.terms.iter().filter(|&&t| rb.terminals[t].pad) {
                let at = rb.terminals[t].at;
                cx.entry(key(at.x, rb.bbox.min.x)).or_default().1 += 1;
                cy.entry(key(at.y, rb.bbox.min.y)).or_default().1 += 1;
            }
        }
    }
    for (t, d, along_x) in fine_row_pads(rb, routes) {
        let term = &rb.terminals[t];
        let at = term.at;
        // Only rows so fine that a track along the grid line nearest to a pad cannot clear the
        // pad's neighbor when the pad is half a pitch off (the rows that get escapes), and whose
        // pad pitch is a multiple of the grid's, so that one offset puts the whole row on lines.
        let b = term.shape.bbox();
        let size = if along_x {
            b.max.x - b.min.x
        } else {
            b.max.y - b.min.y
        };
        if d - size / 2.0 - rb.profile(term.net).hw - g / 2.0 >= rb.c_pad_near(t) + index::TOL
            || ((d / g).round() * g - d).abs() > 1_000.0
        {
            continue;
        }
        if along_x {
            cx.entry(key(at.x, rb.bbox.min.x)).or_default().0 += 1;
        } else {
            cy.entry(key(at.y, rb.bbox.min.y)).or_default().0 += 1;
        }
    }
    let pick = |m: &BTreeMap<i64, (usize, usize)>| -> f64 {
        m.iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(k, _)| *k as f64 * 1_000.0)
            .unwrap_or(0.0)
    };
    (pick(&cx).min(g - 1.0), pick(&cy).min(g - 1.0))
}

/// Nets to route with their islands and connections: those of `wanted`.
fn plan(rb: &RouterBoard, wanted: &BTreeSet<u32>) -> Vec<NetRoute> {
    // Terminals by net, then island.
    let mut by_net: BTreeMap<u32, BTreeMap<usize, Vec<usize>>> = BTreeMap::new();
    for (ti, t) in rb.terminals.iter().enumerate() {
        by_net
            .entry(t.net)
            .or_default()
            .entry(t.island)
            .or_default()
            .push(ti);
    }
    let mut routes = Vec::new();
    for (net, isls) in by_net {
        if !wanted.contains(&net) {
            continue;
        }
        routes.extend(plan_net(rb, net, isls));
    }
    // Short nets first: they have the least freedom.
    routes.sort_by(|a, b| a.length.total_cmp(&b.length).then(a.net.cmp(&b.net)));
    routes
}

/// The islands and connections of one net: `isls` are its terminals by copper island.
fn plan_net(rb: &RouterBoard, net: u32, isls: BTreeMap<usize, Vec<usize>>) -> Option<NetRoute> {
    let via = |t: usize| matches!(rb.terminals[t].what, What::Via(_));
    // Islands with a pad or via (track-only copper is not a ratsnest endpoint).
    let islands: Vec<Vec<usize>> = isls
        .into_values()
        .filter(|ts| ts.iter().any(|&t| rb.terminals[t].pad || via(t)))
        .collect();
    if islands.len() < 2 {
        return None;
    }
    // Ratsnest anchors: the pads of an island (its vias when it has none).
    let anchors: Vec<Vec<usize>> = islands
        .iter()
        .map(|isl| {
            let pads: Vec<usize> = isl
                .iter()
                .copied()
                .filter(|&t| rb.terminals[t].pad)
                .collect();
            if !pads.is_empty() {
                return pads;
            }
            isl.iter().copied().filter(|&t| via(t)).collect()
        })
        .collect();
    let mut conns = Vec::new();
    // Prim over islands, edge = closest anchor pair (as the ratsnest).
    let n = islands.len();
    let mut in_tree = vec![false; n];
    in_tree[0] = true;
    for _ in 1..n {
        let mut best: Option<(f64, usize, usize, usize, usize)> = None;
        for gi in (0..n).filter(|&i| in_tree[i]) {
            for hj in (0..n).filter(|&j| !in_tree[j]) {
                for &a in &anchors[gi] {
                    for &b in &anchors[hj] {
                        let d = rb.terminals[a].at.dist(rb.terminals[b].at).round();
                        if best.is_none_or(|x| d < x.0) {
                            best = Some((d, gi, hj, a, b));
                        }
                    }
                }
            }
        }
        let Some((_, gi, hj, a, b)) = best else { break };
        in_tree[hj] = true;
        conns.push(Conn {
            a: gi,
            b: hj,
            ta: a,
            tb: b,
            from_at: rb.terminals[a].at,
            to_at: rb.terminals[b].at,
        });
    }
    if conns.is_empty() {
        return None;
    }
    let length = conns.iter().map(|c| c.from_at.dist(c.to_at)).sum();
    let mut region = BoxF::EMPTY;
    for &t in islands.iter().flatten() {
        region = region.union(rb.terminals[t].shape.bbox());
    }
    let side = (region.max.x - region.min.x).max(region.max.y - region.min.y);
    let region = region.expand(REGION_MARGIN + 0.25 * side);
    Some(NetRoute {
        net,
        prof: rb.net_profile[net as usize],
        islands: islands
            .into_iter()
            .map(|terms| Island {
                terms,
                access: vec![],
                pours: vec![],
            })
            .collect(),
        failed: vec![None; conns.len()],
        conns,
        wires: vec![],
        keys: vec![],
        bbox: BoxF::EMPTY,
        length,
        region,
        dyn_ids: vec![],
        keep: vec![],
    })
}

/// Gives every island of `routes` the fills that are part of it (`term_island`: the copper
/// island of each terminal).
fn assign_pours(eng: &Engine<'_>, routes: &mut [NetRoute], term_island: &[usize]) {
    let mut by_island: BTreeMap<(u32, usize), Vec<u32>> = BTreeMap::new();
    for (i, f) in eng.pours.iter().enumerate() {
        by_island
            .entry((f.net, f.island))
            .or_default()
            .push(i as u32);
    }
    for r in routes {
        for island in &mut r.islands {
            island.pours = island
                .terms
                .first()
                .and_then(|&t| by_island.get(&(r.net, term_island[t])))
                .cloned()
                .unwrap_or_default();
        }
    }
}

/// The fills of a board's zones as the rounds are given them.
#[derive(Clone, Copy)]
pub(crate) struct Pours<'a> {
    /// The fills that count: those that the nets are planned on, and those of the zones
    /// that are no planes, which the run took as copper before routing.
    pub(crate) fills: &'a [Fill],
    /// The zones of the board from here on are those fills taken as copper.
    pub(crate) own_zones: usize,
    /// By zone, whether its net plans on it as a plane.
    pub(crate) is_plane: &'a [bool],
    /// Of a host's fills around the routes, those that count, by the same rule.
    pub(crate) counted: &'a dyn Fn(Vec<Fill>) -> Vec<Fill>,
    /// The net whose pad has a spoke of its thermal relief at a place of a layer, as its
    /// number plus one, or zero, and what a track of another net costs there
    /// ([`World::spoke_toll`](super::world::World::spoke_toll)).
    pub(crate) spoke: &'a dyn Fn(u8, Point) -> (u32, f64),
}

/// A planned fanout or escape: the copper to add for it.
pub(crate) struct FanItem {
    pub(crate) net: NetId,
    pub(crate) tracks: Vec<Track>,
    pub(crate) via: Option<Via>,
}

/// Plans dog-bone fanouts and escapes for the nets of `routes`, on the board `rb` without
/// them.
fn fanout_items(rb: &RouterBoard, routes: &[NetRoute], slots: &[usize]) -> Vec<FanItem> {
    if routes.is_empty() {
        return Vec::new();
    }
    let mut index = Index::new(rb.bbox.expand(1_000_000.0), 500_000.0, reach(rb));
    for (i, ob) in rb.obstacles.iter().enumerate() {
        index.insert(Item::Static(i as u32), ob.bbox);
    }
    let mut bones = fanout::plan(rb, routes, slots, &mut index, None);
    let grid = make_grid(rb, routes);
    let cross = fanout::crossovers(rb, routes, slots, &grid, &mut index);
    bones.extend(cross.bones.iter().cloned());
    // Escapes end on cells from which the router can move outwards (static maps).
    let mut active = vec![false; rb.profiles.len()];
    for r in routes {
        active[r.prof] = true;
    }
    let st = grid::Statics::build(rb, &grid, slots, &active);
    let open = |net: u32, layer: usize, (x, y): (i32, i32), (dx, dy): (i32, i32)| {
        let Some(s) = slots.iter().position(|&l| l == layer) else {
            return false;
        };
        let map = &st.track[rb.net_profile[net as usize]][s];
        [
            (2 * x, 2 * y),
            (2 * x + dx, 2 * y + dy),
            (2 * x + 2 * dx, 2 * y + 2 * dy),
        ]
        .into_iter()
        .all(|(i, j)| {
            i >= 0 && j >= 0 && i < grid.dw() && j < grid.dh() && st.ok(map[grid.didx(i, j)], net)
        })
    };
    let mut missed = Vec::new();
    let mut escapes = fanout::escape(
        rb,
        routes,
        slots,
        &grid,
        &mut index,
        None,
        &open,
        &cross,
        &[],
        &mut missed,
    );
    escapes.splice(0..0, cross.exits);
    let last = LayerId((rb.layers.max(1) - 1) as u8);
    let track = |net: u32, layer: usize, a: P, b: P| -> Option<Track> {
        let (start, end) = (a.to_point(), b.to_point());
        (start != end).then(|| Track {
            start,
            end,
            mid: None,
            width: rb.profile(net).key.width.0,
            layer: LayerId(layer as u8),
            net: Some(NetId(net)),
        })
    };
    let mut out: Vec<FanItem> = bones
        .into_iter()
        .map(|b| {
            let key = rb.profile(b.net).key;
            let (diameter, drill) = b.small.unwrap_or((key.via_dia, key.via_drill));
            let via = Via {
                at: b.via.to_point(),
                drill: drill.0,
                diameter: diameter.0,
                from: LayerId(0),
                to: last,
                net: Some(NetId(b.net)),
            };
            FanItem {
                net: NetId(b.net),
                tracks: track(b.net, b.layer, b.pad, b.via).into_iter().collect(),
                via: Some(via),
            }
        })
        .collect();
    for e in escapes {
        let tracks = e
            .pts
            .windows(2)
            .filter_map(|w| track(e.net, e.layer, w[0], w[1]))
            .collect();
        out.push(FanItem {
            net: NetId(e.net),
            tracks,
            via: None,
        });
    }
    out
}

/// The ways out of the rows and arrays of fine pitch for the nets of `design` that are
/// routed, as cadlab's own router plans them before it routes: a dog-bone for a ball inside
/// an array, a short track to a via between the balls, with the smallest via of the rules
/// where the net's own does not fit, and for a pad of a row whose track cannot leave it along
/// a line of the grid, an escape to a cell from which it can.
pub(crate) fn fanout(design: &Design, pours: &Pours<'_>) -> Vec<FanItem> {
    let Pours {
        fills,
        own_zones,
        is_plane,
        ..
    } = *pours;
    let layers = design.rules.layer_count();
    if layers == 0 || design.outline.is_empty() {
        return Vec::new();
    }
    let slots: Vec<usize> = (0..layers).collect();
    let wanted: BTreeSet<u32> = (0..design.nets.len())
        .filter(|&i| design.nets[i].route)
        .map(|i| i as u32)
        .collect();
    let (base, fills) = base_of(design, fills, own_zones);
    let first = scene(design, base, &fills, is_plane);
    let routes = plan(&first.rb, &wanted);
    fanout_items(&first.rb, &routes, &slots)
}

/// The board of `design` without the fills that it took as copper (its zones from
/// `own_zones` on, which are among the fills that count), and the fills that count, each with
/// its net.
fn base_of(design: &Design, fills: &[Fill], own_zones: usize) -> (Board, Vec<(Fill, NetId)>) {
    let mut base = design.board.clone();
    base.zones.truncate(own_zones.min(base.zones.len()));
    (base, with_nets(design, fills))
}

/// The board with zone fills, as the router has it.
struct Scene {
    board: Board,
    items: Vec<Copper>,
    rb: RouterBoard,
}

/// The fills of `fills` whose zones have a net, with it.
fn with_nets(design: &Design, fills: &[Fill]) -> Vec<(Fill, NetId)> {
    fills
        .iter()
        .filter_map(|f| Some((f.clone(), design.zones.zones.get(f.zone)?.net?)))
        .collect()
}

fn refs(fills: &[(Fill, NetId)]) -> Vec<(&Fill, NetId)> {
    fills.iter().map(|(f, n)| (f, *n)).collect()
}

fn scene(design: &Design, board: Board, fills: &[(Fill, NetId)], is_plane: &[bool]) -> Scene {
    let items = model::copper(&board, design.rules.layers());
    let fills = refs(fills);
    let isl = model::islands(&items, &fills);
    // The fills of the zones that are no planes are copper to the other nets.
    let wall = |f: &Fill| !is_plane.get(f.zone).copied().unwrap_or(true);
    let rb = RouterBoard::build(design, &board, &items, &fills, &isl, &wall);
    Scene { board, items, rb }
}

/// The routes as tracks and vias of the board.
fn copper_of(eng: &Engine<'_>, routes: &[NetRoute]) -> (Vec<Track>, Vec<Via>) {
    let rb = eng.rb;
    let last = LayerId((rb.layers.max(1) - 1) as u8);
    let (mut tracks, mut vias) = (Vec::new(), Vec::new());
    for r in routes {
        let key = rb.profile(r.net).key;
        let net = Some(NetId(r.net));
        for w in &r.wires {
            let (segs, at) = eng.wire_geometry(w);
            for (s, a, b) in segs {
                let (start, end) = (a.to_point(), b.to_point());
                if start != end {
                    tracks.push(Track {
                        start,
                        end,
                        mid: None,
                        width: key.width.0,
                        layer: LayerId(eng.slots[s] as u8),
                        net,
                    });
                }
            }
            for v in at {
                vias.push(Via {
                    at: v.to_point(),
                    drill: key.via_drill.0,
                    diameter: key.via_dia.0,
                    from: LayerId(0),
                    to: last,
                    net,
                });
            }
        }
    }
    (tracks, vias)
}

/// The board as the routing would leave it, with the pours filled again around the new copper.
struct PourTruth {
    /// The fills as the host makes them around the routes, with their nets.
    fills: Vec<(Fill, NetId)>,
    /// By item of the board and then by fill, its island with those fills and without the
    /// routes: what the fills alone still join.
    isl: Vec<usize>,
    /// Per route: one of its connections is routed and yet open (a track that ends on a piece
    /// of a pour that other nets cut off).
    dangling: Vec<bool>,
    /// Connections open on that board, of the nets routed.
    open: usize,
}

/// Fills the pours for the routing as it stands ([`PourTruth`]), by the host's fill.
/// `None` if the host gives none.
#[allow(clippy::too_many_arguments)]
fn pour_truth(
    design: &Design,
    sc: &Scene,
    counted: &dyn Fn(Vec<Fill>) -> Vec<Fill>,
    before: (&[Track], &[Via]),
    eng: &Engine<'_>,
    routes: &[NetRoute],
    wanted: &BTreeSet<u32>,
    run: &Run<'_>,
) -> Option<PourTruth> {
    let (tracks, vias) = copper_of(eng, routes);
    let (mut all_t, mut all_v) = (before.0.to_vec(), before.1.to_vec());
    all_t.extend(tracks.iter().cloned());
    all_v.extend(vias.iter().cloned());
    let fills: Vec<Fill> = counted(run.refill(&all_t, &all_v, false)?);
    let fills = with_nets(design, &fills);
    let nets = refs(&fills);
    // With the routes: what is open, and the routes that end on a piece cut off.
    let mut board = sc.board.clone();
    board.tracks.extend(tracks);
    board.vias.extend(vias);
    let items = model::copper(&board, design.rules.layers());
    let isl = model::islands(&items, &nets);
    let key = |w: What| match w {
        What::Pad(p, first) => (0u8, p, first),
        What::Track(t) => (1, t, false),
        What::Via(v) => (2, v, false),
        What::Drawing(z) => (3, z, false),
    };
    let at: BTreeMap<(u8, usize, bool), usize> = items
        .iter()
        .enumerate()
        .map(|(i, it)| (key(it.what), i))
        .collect();
    let island_of = |t: usize| at.get(&key(sc.rb.terminals[t].what)).map(|&i| isl[i]);
    let mut pieces: BTreeSet<(u32, usize)> = BTreeSet::new();
    let mut nets_seen: BTreeSet<u32> = BTreeSet::new();
    for (t, term) in sc.rb.terminals.iter().enumerate() {
        if wanted.contains(&term.net)
            && (term.pad || matches!(term.what, What::Via(_)))
            && let Some(k) = island_of(t)
        {
            pieces.insert((term.net, k));
            nets_seen.insert(term.net);
        }
    }
    let open = pieces.len() - nets_seen.len();
    let dangling = routes
        .iter()
        .map(|r| {
            r.conns
                .iter()
                .zip(&r.failed)
                .any(|(c, f)| f.is_none() && island_of(c.ta) != island_of(c.tb))
        })
        .collect();
    // Without them: what the fills alone join.
    let isl = model::islands(&sc.items, &nets);
    Some(PourTruth {
        fills,
        isl,
        dangling,
        open,
    })
}

/// The copper island of every terminal on the board of `truth`: what the fills alone join.
fn truth_islands(rb: &RouterBoard, truth: &PourTruth) -> Vec<usize> {
    rb.terminals.iter().map(|t| truth.isl[t.item]).collect()
}

/// How many more pieces the pads of the nets with fills are in with the islands `now` than with
/// `before`: the pads (or groups of pads) the routing cut off from their pours.
fn cut_off(rb: &RouterBoard, before: &[usize], now: &[usize]) -> usize {
    let nets: BTreeSet<u32> = rb.fills.iter().map(|f| f.net).collect();
    let pieces = |isl: &[usize]| -> usize {
        let set: BTreeSet<(u32, usize)> = rb
            .terminals
            .iter()
            .enumerate()
            .filter(|(_, t)| t.pad && nets.contains(&t.net))
            .map(|(i, t)| (t.net, isl[i]))
            .collect();
        set.len()
    };
    pieces(now).saturating_sub(pieces(before))
}

/// Pours are filled again around new copper, so what a fill joined when the run was planned
/// may be cut apart by the routing: pads in a piece of a pour that tracks of other nets cut off,
/// or a track that ends on such a piece. Every net with fills whose pads they no longer join
/// as planned, or with a connection that is routed and yet open, is planned again from what
/// the fills still join (`truth`; its routing is dropped), to be routed onto the new fills
/// where they are near (`pours`). Returns the nets planned again (indices into `routes`),
/// shortest first, for negotiation to route, and the plans they replace.
#[allow(clippy::too_many_arguments)]
fn replan_pours(
    sc: &Scene,
    wanted: &BTreeSet<u32>,
    slots: &[usize],
    eng: &mut Engine<'_>,
    ck: &Checker<'_>,
    routes: &mut Vec<NetRoute>,
    term_island: &mut [usize],
    truth: &PourTruth,
) -> (Vec<usize>, Vec<(usize, NetRoute)>) {
    let rb = &sc.rb;
    term_island.copy_from_slice(&truth_islands(rb, truth));
    let fills = model::fills_of(&refs(&truth.fills), &truth.isl, sc.items.len());
    let mut nets: BTreeSet<u32> = rb.fills.iter().chain(&fills).map(|f| f.net).collect();
    nets.retain(|n| wanted.contains(n));
    eng.pours = pours::build(&fills, rb, &eng.grid, slots);
    let mut by_net: BTreeMap<u32, BTreeMap<usize, Vec<usize>>> = BTreeMap::new();
    for (ti, t) in rb.terminals.iter().enumerate() {
        if nets.contains(&t.net) {
            by_net
                .entry(t.net)
                .or_default()
                .entry(term_island[ti])
                .or_default()
                .push(ti);
        }
    }
    let mut changed: Vec<usize> = Vec::new();
    let mut replaced: Vec<(usize, NetRoute)> = Vec::new();
    for (net, isls) in by_net {
        let old = routes.iter().position(|r| r.net == net);
        let Some(mut nr) = plan_net(rb, net, isls) else {
            continue;
        };
        if eng.act[nr.prof] == usize::MAX {
            continue; // no occupancy maps for its rules: reported as cut off
        }
        // Planned again when the fills no longer join an island of the plan (fills joining
        // more than planned leave it valid).
        if let Some(ri) = old
            && !truth.dangling[ri]
            && routes[ri].islands.iter().all(|i| {
                i.terms
                    .iter()
                    .all(|&t| term_island[t] == term_island[i.terms[0]])
            })
        {
            continue;
        }
        for island in &mut nr.islands {
            for &t in &island.terms {
                island.access.extend(eng.access(t, ck));
            }
        }
        match old {
            Some(ri) => {
                eng.uncommit(&mut routes[ri]);
                replaced.push((ri, std::mem::replace(&mut routes[ri], nr)));
                changed.push(ri);
            }
            None => {
                routes.push(nr);
                changed.push(routes.len() - 1);
            }
        }
    }
    assign_pours(eng, routes, term_island);
    changed.sort_by(|a, b| {
        routes[*a]
            .length
            .total_cmp(&routes[*b].length)
            .then(a.cmp(b))
    });
    (changed, replaced)
}

/// The routing before nets were planned again for their pours ([`pour_round`]), to go back to
/// when routing them leaves more connections open than there were.
struct PourSnapshot {
    /// Connections open on the board then.
    open: usize,
    /// Wires and failures of every net.
    state: Vec<(Vec<Wire>, Vec<Option<FailKind>>)>,
    /// The plans that were replaced.
    replaced: Vec<(usize, NetRoute)>,
    term_island: Vec<usize>,
    pours: Vec<pours::Pour>,
}

/// What a step of the pour rounds did.
enum PourStep {
    /// The routes went back to those before the nets were planned again: no more rounds.
    Back,
    /// These nets were planned again, for negotiation (none: the rounds are over).
    Planned(Vec<usize>),
}

/// One step of the pour rounds, on a legal routing. When nets were planned again before
/// (`pending`), the board is filled for what negotiation made of them: with more connections
/// open than before, everything goes back to the routing before that plan. Otherwise, when
/// `may_plan`, the nets the routing cut off from their pours are planned again
/// ([`replan_pours`]) and returned for negotiation.
#[allow(clippy::too_many_arguments)]
fn pour_round(
    design: &Design,
    sc: &Scene,
    counted: &dyn Fn(Vec<Fill>) -> Vec<Fill>,
    before: (&[Track], &[Via]),
    wanted: &BTreeSet<u32>,
    slots: &[usize],
    eng: &mut Engine<'_>,
    ck: &Checker<'_>,
    routes: &mut Vec<NetRoute>,
    term_island: &mut Vec<usize>,
    pending: &mut Option<PourSnapshot>,
    may_plan: bool,
    run: &Run<'_>,
    refills: &mut usize,
) -> PourStep {
    if pending.is_none() && !may_plan {
        return PourStep::Planned(vec![]);
    }
    let truth = pour_truth(design, sc, counted, before, eng, routes, wanted, run);
    *refills += usize::from(truth.is_some());
    let fails = |state: &mut dyn Iterator<Item = &Vec<Option<FailKind>>>| -> usize {
        state
            .map(|f| f.iter().filter(|f| f.is_some()).count())
            .sum()
    };
    // More open on the board filled again; with no fill to tell by (a host that gives none
    // any more as the run ends), more connections that found no way.
    let worse = |snap: &PourSnapshot| match &truth {
        Some(t) => t.open > snap.open,
        None => {
            fails(&mut routes.iter().map(|r| &r.failed))
                > fails(&mut snap.state.iter().map(|s| &s.1))
        }
    };
    if let Some(snap) = pending.take()
        && worse(&snap)
    {
        for r in routes.iter_mut() {
            eng.uncommit(r);
        }
        routes.truncate(snap.state.len());
        for (ri, old) in snap.replaced {
            routes[ri] = old;
        }
        for (r, (w, f)) in routes.iter_mut().zip(snap.state) {
            r.wires = w;
            r.failed = f;
            eng.commit(r);
        }
        *term_island = snap.term_island;
        eng.pours = snap.pours;
        // The plan gave every island the fills of its own board, the nets it left alone
        // included: they are named by their place among the fills, which are the ones before
        // again.
        assign_pours(eng, routes, term_island);
        return PourStep::Back;
    }
    let Some(truth) = truth.filter(|_| may_plan) else {
        return PourStep::Planned(vec![]);
    };
    let state = routes
        .iter()
        .map(|r| (r.wires.clone(), r.failed.clone()))
        .collect();
    let (old_islands, old_pours) = (term_island.clone(), eng.pours.clone());
    let (changed, replaced) = replan_pours(sc, wanted, slots, eng, ck, routes, term_island, &truth);
    if !changed.is_empty() {
        *pending = Some(PourSnapshot {
            open: truth.open,
            state,
            replaced,
            term_island: old_islands,
            pours: old_pours,
        });
    }
    PourStep::Planned(changed)
}

/// Negotiates for every net of `jobs` at once, as cadlab's own router does, and returns what
/// each ends with.
///
/// `design` is the board with nothing of the run on it but the copper `fixed`, which is at
/// the end of its tracks and vias: its ways out of the rows and arrays of fine pitch
/// ([`fanout`]) among it. The fills of the zones that are no planes ([`Pours`]) are fills of
/// their nets as the others are, which a connection may end on, and copper to every other
/// net, as they are to the passes. `share` is the share of the run that the stage has, and
/// `was` what ended a part of the run before it, for a stage that is near its end when its
/// share is spent to have as much again, once.
///
/// # Errors
///
/// [`NoWhole::Not`] if there is nothing to route, or the run ended before a round did.
#[allow(clippy::too_many_lines)]
pub(crate) fn route(
    design: &Design,
    jobs: &[Job],
    pours: &Pours<'_>,
    fixed: (&[Track], &[Via]),
    (share, was): (f64, Option<super::Stopped>),
    options: &RouteOptions,
    run: &Run<'_>,
) -> Result<Whole, NoWhole> {
    let Pours {
        fills,
        own_zones,
        is_plane,
        counted,
        spoke,
    } = *pours;
    let layers = design.rules.layer_count();
    if jobs.is_empty() || layers == 0 || design.outline.is_empty() {
        return Err(NoWhole::Not);
    }
    let start = Instant::now();
    let slots: Vec<usize> = (0..layers).collect();
    let wanted: BTreeSet<u32> = jobs.iter().map(|j| j.ctx.net.0).collect();
    let (base, fills) = base_of(design, fills, own_zones);
    let fills = &fills[..];
    // The budget: what the part of the run under way has, in time and in work.
    let work_limit = run.limits(u64::MAX).max_expansions;
    let meter = if work_limit == u64::MAX {
        WorkMeter::default()
    } else {
        WorkMeter::limited(work_limit)
    };
    let wall = run.part_left();
    let mut stage_end = Limit::share(wall, start, &meter, 0, 1.0);
    let mut negotiate_limit = Limit::share(wall, start, &meter, 0, NEGOTIATE);
    let pour_first = Limit::share(wall, start, &meter, 0, POUR_FIRST);
    let flag = run.flag();
    let phase_end = Mutex::new(negotiate_limit.clone());
    let stop = || flag.load(Ordering::Relaxed) || phase_end.lock().expect("deadline").passed();
    let charge = |meter: &WorkMeter| {
        run.charge(meter.used());
    };

    // The fills are those of the board with the run's own copper on it, its ways out of the
    // rows and arrays of fine pitch, where the host fills its zones itself: a pour that
    // reached no pad, and so was no copper at all, is joined to the balls of its net by
    // their vias.
    let mut refills = 0;
    let around_fan: Option<Vec<(Fill, NetId)>> = if !(fixed.0.is_empty() && fixed.1.is_empty())
        && !design.zones.zones.is_empty()
        && run.refills()
    {
        run.refill(fixed.0, fixed.1, false).map(|fills| {
            refills += 1;
            with_nets(design, &counted(fills))
        })
    } else {
        None
    };
    if run.poll() {
        return Err(NoWhole::Not);
    }
    let fills = around_fan.as_deref().unwrap_or(fills);
    let sc = scene(design, base, fills, is_plane);
    let mut routes = plan(&sc.rb, &wanted);
    if routes.is_empty() {
        return Err(NoWhole::Not);
    }
    let rb = &sc.rb;
    // What the host's fill is asked around besides the routes: the run's own copper before
    // this stage.
    let before: (Vec<Track>, Vec<Via>) = (fixed.0.to_vec(), fixed.1.to_vec());

    // Grid.
    let mut active = vec![false; rb.profiles.len()];
    for r in &routes {
        active[r.prof] = true;
    }
    let grid = make_grid(rb, &routes);
    let pitch = grid.g;
    let mut eng = Engine::new(rb, grid, slots.clone(), &active, costs(), &stop);
    eng.work = meter.clone();
    // The rooms of the spokes of pads in pours, at a price to every other net.
    {
        let mut any = false;
        let mut by_slot = Vec::with_capacity(slots.len());
        for &l in &slots {
            let mut cells = Vec::with_capacity(eng.grid.cells());
            for y in 0..eng.grid.h {
                for x in 0..eng.grid.w {
                    let (n, cost) = spoke(l as u8, eng.grid.cell(x, y).to_point());
                    any |= n != 0;
                    eng.spoke_cost = cost as f32;
                    cells.push(n);
                }
            }
            by_slot.push(cells);
        }
        if any {
            eng.spoke = by_slot;
        }
    }
    let mut scratch = eng.scratch();
    let pool: Mutex<Vec<Scratch>> = Mutex::new(Vec::new());
    // Fills: each island's connections may end on the fills that are part of it.
    let mut term_island: Vec<usize> = rb.terminals.iter().map(|t| t.island).collect();
    eng.pours = pours::build(&rb.fills, rb, &eng.grid, &slots);
    assign_pours(&eng, &mut routes, &term_island);
    let poured = !rb.fills.is_empty() && run.refills();

    // Static index for access stubs and exact checks.
    let reach = reach(rb);
    let mut index = Index::new(
        rb.bbox.expand(pitch * 4.0),
        (pitch * 8.0).max(500_000.0),
        reach,
    );
    for (i, ob) in rb.obstacles.iter().enumerate() {
        index.insert(Item::Static(i as u32), ob.bbox);
    }
    {
        let ck = Checker { rb, index: &index };
        let mut cache: BTreeMap<usize, Vec<Access>> = BTreeMap::new();
        for r in &mut routes {
            for island in &mut r.islands {
                for &t in &island.terms {
                    let acc = cache.entry(t).or_insert_with(|| eng.access(t, &ck));
                    island.access.extend(acc.iter().cloned());
                }
            }
        }
    }
    if run.poll() || stop() {
        charge(&meter);
        return Err(NoWhole::Not);
    }
    let total_conns: usize = routes.iter().map(|r| r.conns.len()).sum();
    let threads = options.threads.max(1);
    // The connections that have no way on this grid whatever the other nets do, told before
    // anything is routed: with too many of them the rounds are given up here, and the first
    // round's time is not spent to find it out.
    let no_way: usize = routes
        .iter()
        .map(|r| (0..r.conns.len()).filter(|&ci| eng.no_way(r, ci)).count())
        .sum();
    if no_way * NO_WAY > total_conns {
        charge(&meter);
        return Err(NoWhole::NoRoom);
    }

    // ---- negotiated congestion ----
    let mut rng = Rng(options.seed.unwrap_or(0) ^ 0x5EED_CAD1_AB00_0000);
    let mut pres = 0.6f32;
    let hist_fac = 0.5f32;
    let mut order: Vec<usize> = (0..routes.len()).collect();
    let mut reroute: Vec<usize> = order.clone();
    let mut pour_rounds = 0;
    let pours_ok = poured && options.rounds_pours;
    let mut pending: Option<PourSnapshot> = None;
    let mut mid_check = false;
    let mut iterations = 0;
    let mut pours_back = 0;
    let mut pour_cuts = 0;
    let mut shared_at_end = 0;
    let mut taken_out;
    let mut ended = true;
    let mut capped = false;
    let mut any_round = false;
    let mut extended = false;
    let mut vias_saved = 0;
    loop {
        let mut best: Option<Snapshot> = None;
        let mut best_it = 0;
        // Whether this round's negotiation ended on the budget, or early for the pour rounds.
        let mut cut = false;
        let mut pause = false;
        for it in 0..ROUNDS {
            iterations += 1;
            for batch in engine::batches(&reroute, &routes) {
                if run.poll()
                    || route_batch(
                        &mut eng,
                        &mut routes,
                        &batch,
                        Mode::Negotiate(pres),
                        &pool,
                        threads,
                    )
                    .is_err()
                {
                    cut = true;
                    for &ri in &batch {
                        let r = &mut routes[ri];
                        for (ci, f) in r.failed.iter_mut().enumerate() {
                            if f.is_none() && !r.wires.iter().any(|w| w.conn == ci) {
                                *f = Some(FailKind::Budget);
                            }
                        }
                    }
                    break;
                }
            }
            // Overuse.
            let mut points: Vec<(engine::Conflict, usize)> = Vec::new();
            let mut net_conf = vec![0usize; routes.len()];
            for (ri, r) in routes.iter().enumerate() {
                let c = eng.conflicts(r);
                net_conf[ri] = c.len();
                points.extend(c.into_iter().map(|c| (c, ri)));
            }
            let total: usize = net_conf.iter().sum();
            any_round |= !cut;
            // The first round says what the rounds take, and what of the board this grid
            // reaches: with less than that in the stage's share, or with too many connections
            // that have no way on the grid whatever the other nets do, the rounds are given up
            // here, for a negotiation that has room.
            if iterations == 1 && !cut {
                let took = start.elapsed().as_secs_f64();
                let nodes = meter.used() as f64;
                let no_way: usize = routes
                    .iter()
                    .map(|r| {
                        r.failed
                            .iter()
                            .filter(|f| matches!(f, Some(FailKind::Static | FailKind::NoAccess)))
                            .count()
                    })
                    .sum();
                let short = wall.is_some_and(|w| ROOM * took > w.as_secs_f64())
                    || meter.limit().is_some_and(|l| ROOM * nodes > l as f64);
                if short || no_way * NO_WAY > total_conns {
                    charge(&meter);
                    return Err(NoWhole::NoRoom);
                }
            }
            if best.as_ref().is_none_or(|b| total < b.0) {
                best = Some((
                    total,
                    routes.iter().map(|r| r.wires.clone()).collect(),
                    routes.iter().map(|r| r.failed.clone()).collect(),
                ));
                best_it = it;
            }
            // Pours, on a board whose first negotiation is still going late in its share of the
            // budget: when the routing as it stands cuts enough pads off their fills, negotiation
            // ends here, so that the pour rounds get the rest of its share; else it goes on.
            if pours_ok && !mid_check && pour_rounds == 0 && total > 0 && pour_first.passed() {
                mid_check = true;
                if let Some(truth) = pour_truth(
                    design,
                    &sc,
                    counted,
                    (&before.0, &before.1),
                    &eng,
                    &routes,
                    &wanted,
                    run,
                ) {
                    refills += 1;
                    let n = cut_off(rb, &term_island, &truth_islands(rb, &truth));
                    pause = n > 0 && n * POUR_PAUSE >= total_conns;
                }
            }
            let stale = it - best_it >= STAGNATION;
            // Rounds that their share ends with few points still shared are near their end:
            // they have as much again of what the run has left, once.
            if (cut || stop())
                && total > 0
                && !extended
                && it > 0
                && meter.limit().is_none()
                && run.stopped().is_none()
                && total * NEARLY < total_conns
            {
                extended = true;
                run.extend(share, share, was);
                if !run.ending() {
                    let now = Instant::now();
                    let left = run.part_left();
                    stage_end = Limit::share(left, now, &meter, 0, 1.0);
                    negotiate_limit = Limit::share(left, now, &meter, 0, NEGOTIATE);
                    *phase_end.lock().expect("deadline") = negotiate_limit.clone();
                    cut = false;
                }
            }
            if total == 0 || cut || pause || stop() || stale {
                cut |= total > 0 && stop();
                capped = false;
                break;
            }
            capped = it + 1 == ROUNDS;
            for (c, _) in &points {
                eng.add_history(*c, hist_fac);
            }
            // Capped: beyond it, sharing is already prohibitive and a higher factor only makes
            // the searches of the nets still in conflict visit ever more of the board.
            pres = (pres * 1.6).min(PRES_MAX);
            // Reroute nets with overuse and nets whose routes are near an overused point.
            let mut set: BTreeSet<usize> = BTreeSet::new();
            for (c, ri) in &points {
                set.insert(*ri);
                let at = eng.conflict_pos(*c);
                for (rj, r) in routes.iter().enumerate() {
                    if r.bbox.expand(reach).dist(at) == 0.0 {
                        set.insert(rj);
                    }
                }
            }
            reroute = order.iter().copied().filter(|r| set.contains(r)).collect();
            // Nets with many connections rerouted only for being near the overuse (none of their
            // own) keep the wires clear of it (partial rip-up).
            let hot: Vec<P> = points.iter().map(|(c, _)| eng.conflict_pos(*c)).collect();
            for &ri in &reroute {
                let r = &mut routes[ri];
                if r.conns.len() < PARTIAL_MIN || net_conf[ri] > 0 {
                    continue;
                }
                r.keep = r
                    .wires
                    .iter()
                    .map(|w| {
                        let mut bb = BoxF::EMPTY;
                        for &q in &w.nodes {
                            let (_, x, y) = eng.unpack(q);
                            bb.add(eng.grid.cell(x, y));
                        }
                        let bb = bb.expand(reach + PARTIAL_MARGIN * pitch);
                        !w.nodes.is_empty() && !hot.iter().any(|&h| bb.dist(h) == 0.0)
                    })
                    .collect();
            }
            // Perturb the order a little (seeded) to break cycles.
            for i in (1..reroute.len()).rev() {
                if rng.next().is_multiple_of(4) {
                    let j = (rng.next() % (i as u64 + 1)) as usize;
                    reroute.swap(i, j);
                }
            }
            order.sort_by_key(|&r| std::cmp::Reverse(net_conf[r]));
            order.sort_by(|a, b| routes[*a].length.total_cmp(&routes[*b].length));
        }
        if run.cancelled() {
            charge(&meter);
            return Err(NoWhole::Not);
        }
        // Partial rip-up only applies to negotiation (a cut-short iteration leaves masks behind).
        for r in routes.iter_mut() {
            r.keep.clear();
        }
        ended &= !cut;
        // Restore the best iteration.
        if let Some((bt, wires, failed)) = best {
            let cur: usize = routes.iter().map(|r| eng.conflicts(r).len()).sum();
            if bt < cur {
                for r in routes.iter_mut() {
                    eng.uncommit(r);
                }
                for ((r, w), f) in routes.iter_mut().zip(wires).zip(failed) {
                    r.wires = w;
                    r.failed = f;
                    eng.commit(r);
                }
            }
            shared_at_end = bt;
        }
        // ---- legalization: rip what still conflicts, reroute without sharing ----
        *phase_end.lock().expect("deadline") = stage_end.clone();
        let (taken, all) = legalise_wires(&mut eng, &mut scratch, &mut routes);
        taken_out = taken;
        ended &= all;
        // Pours: the routing is legal now, so it is what the board would get. Nets whose fills
        // it cut apart are planned again from what the fills still join and negotiate again, the
        // other nets making room for them.
        if pours_ok {
            let may = pour_rounds < POUR_NEGOTIATIONS && !run.poll() && !negotiate_limit.passed();
            let step = {
                let ck = Checker { rb, index: &index };
                pour_round(
                    design,
                    &sc,
                    counted,
                    (&before.0, &before.1),
                    &wanted,
                    &slots,
                    &mut eng,
                    &ck,
                    &mut routes,
                    &mut term_island,
                    &mut pending,
                    may,
                    run,
                    &mut refills,
                )
            };
            match step {
                PourStep::Back => {
                    pours_back += 1;
                }
                PourStep::Planned(changed) => {
                    if !changed.is_empty() {
                        pour_rounds += 1;
                        pour_cuts += changed.len();
                        *phase_end.lock().expect("deadline") = negotiate_limit.clone();
                        order = (0..routes.len()).collect();
                        order.sort_by(|a, b| routes[*a].length.total_cmp(&routes[*b].length));
                        reroute = changed;
                        continue;
                    }
                }
            }
        }
        // ---- rip-up and retry: a connection that still fails takes the room of the wires in
        // its way, whose nets are routed again around it; kept only when fewer fail overall ----
        retry_wires(&mut eng, &mut scratch, &mut routes, &|| {
            run.poll() || stop()
        });
        // ---- fewer vias: each net again where the others are, with vias four times as dear ----
        if !(run.poll() || stop()) {
            vias_saved = minimize_vias(&mut eng, &mut scratch, &mut routes, &|| {
                run.poll() || stop()
            });
        }
        break;
    }
    charge(&meter);
    if !any_round {
        return Err(NoWhole::Not);
    }

    // ---- what the passes are given: each net's route as a tree, and the places where its
    // routes end that are no pad's centre ----
    let by_net: BTreeMap<u32, usize> = jobs
        .iter()
        .enumerate()
        .map(|(j, job)| (job.ctx.net.0, j))
        .collect();
    let mut trees: Vec<Option<Tree>> = vec![None; jobs.len()];
    let mut sites: Vec<(usize, Vec<TermPad>)> = Vec::new();
    let node = |p: Point, l: usize| DNode {
        x: p.x,
        y: p.y,
        l: l as u8,
    };
    for r in &routes {
        let Some(&j) = by_net.get(&r.net) else {
            continue;
        };
        let ctx = &jobs[j].ctx;
        let tree = trees[j].get_or_insert_with(Tree::default);
        let mut found: Vec<TermPad> = Vec::new();
        let mut earlier: BTreeSet<u32> = BTreeSet::new();
        // The terminal of the job that an island of the route is: that of one of its pads.
        let terminal_of = |island: usize| -> Option<usize> {
            r.islands[island]
                .terms
                .iter()
                .find_map(|&t| match rb.terminals[t].what {
                    What::Pad(p, _) => {
                        let bbox = sc.board.pads[p].shape.bbox();
                        ctx.pads
                            .iter()
                            .find(|tp| tp.bbox == bbox)
                            .map(|tp| tp.terminal)
                    }
                    _ => None,
                })
        };
        for w in &r.wires {
            let (segs, vias) = eng.wire_geometry(w);
            for &(s, a, b) in &segs {
                tree.edges.push((
                    node(a.to_point(), eng.slots[s]),
                    node(b.to_point(), eng.slots[s]),
                ));
            }
            for v in vias {
                for l in 1..layers {
                    tree.edges
                        .push((node(v.to_point(), l - 1), node(v.to_point(), l)));
                }
            }
            // Its two ends: at a pad's centre by a track from where it ends on the pad, or a
            // place of the island's terminal.
            let c = &r.conns[w.conn];
            let ends = [
                (w.nodes.first(), w.start.as_ref(), c.a),
                (w.nodes.last(), w.end.as_ref(), c.b),
            ];
            for (n, stub, island) in ends {
                let Some(&n) = n else { continue };
                let (s, _, _) = eng.unpack(n);
                let layer = LayerId(eng.slots[s] as u8);
                let at = stub
                    .and_then(|st| st.last().copied())
                    .unwrap_or_else(|| eng.node_pos(n))
                    .to_point();
                if ctx
                    .pads
                    .iter()
                    .any(|p| p.center == at && p.layers.contains(layer))
                {
                    continue;
                }
                // An end on a wire of the net laid before is joined there.
                if stub.is_none() && earlier.contains(&n) {
                    continue;
                }
                // A place of the terminal whose copper the end is on, a pad's or fixed copper
                // of the net away from its ends; on a fill, of the plane where the net has
                // one, else of the island's terminal.
                let on = r.islands[island].terms.iter().copied().find(|&t| {
                    let term = &rb.terminals[t];
                    term.layers >> eng.slots[s] & 1 == 1
                        && (term.shape.contains(P::of(at))
                            || term.shape.dist_point(P::of(at)) < 1_000.0)
                });
                let of_pad = on.and_then(|t| match rb.terminals[t].what {
                    What::Pad(p, _) => {
                        let bbox = sc.board.pads[p].shape.bbox();
                        ctx.pads
                            .iter()
                            .find(|tp| tp.bbox == bbox)
                            .map(|tp| tp.terminal)
                    }
                    _ => None,
                });
                let terminal = match of_pad {
                    Some(t) => Some(t),
                    None if on.is_none() && jobs[j].plane => Some(0),
                    None => terminal_of(island),
                };
                if let Some(terminal) = terminal
                    && !found
                        .iter()
                        .any(|p| p.center == at && p.layers.contains(layer))
                {
                    found.push(TermPad {
                        center: at,
                        layers: LayerSet::EMPTY.with(layer),
                        bbox: Rect::new(at, at),
                        terminal,
                    });
                }
            }
            earlier.extend(w.nodes.iter().copied());
        }
        if !found.is_empty() {
            sites.push((j, found));
        }
    }
    Ok(Whole {
        trees,
        pitch: pitch.round() as i64,
        rounds: iterations,
        shared: shared_at_end,
        taken_out,
        vias_saved,
        sites,
        pour_cuts,
        refills,
        pours_back,
        finer: (0, 0, 0),
        ended,
        capped,
    })
}
