//! Routing every net against every other at once, on one grid, before the passes that route
//! a net at a time.
//!
//! The passes of [`router`](super::router) route a net, and take others out for it: what
//! they leave open on a crowded board are nets taken out that found no way back past
//! neighbours that had settled by then. Here every net is routed across every other at a
//! price, as PathFinder does it (McMurchie and Ebeling, 1995): a route claims the points
//! of a grid where a track's centre or a via's of another net would be too close to it, a
//! search pays for each claimed point it enters, by how many nets claim it now and by how
//! often it was claimed by more than one before, and all nets in each other's way are
//! routed again, at prices that rise, until no point is shared. What is still shared then
//! is taken out, and routed again where nothing is claimed.
//!
//! The nets of a round are routed a few at a time, on as many threads as there are, each
//! against what all the others claim before any of the few and with its own claims not
//! counted, and then have their turns one after another: a net gives up its claims and
//! claims what it was routed as, if that is in no claim that it was not in when it was
//! routed, and is routed again at its turn if it is. Two nets of the few whose routes
//! keep clear of each other are as they would be routed one after the other, but for
//! what the first gave up; two that do not are routed one after the other. The few are
//! a number of nets and no more, so the routes are the same on any number of threads.
//!
//! The grid is only where congestion is priced. Whether the board's own copper lets a
//! track or a via be somewhere is asked of the [`World`], exactly, once for each place
//! that a search reaches, and what the negotiation ends with is given to the router as
//! copper of its nets, which keeps of it what the world says is legal beside the rest and
//! goes on from there.

use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};

use super::control::Run;
use super::detail::{DNode, NetCtx, TermPad, Tree};
use super::geometry::NetCopper;
use super::pours::Pours;
use super::router::{Job, pad_center};
use super::spokes::{HELD, HELD_TIMES};
use super::world::{Class, Verdict, World};
use super::{Design, RouteOptions};
use crate::geom::drc::{Track, Via};
use crate::geom::{EntryId, LayerId, LayerSet, Point, Rect, Shape, SpatialIndex};
use crate::id::NetId;

/// The eight directions, counter-clockwise from east.
const DIRS: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];
/// No direction: where a search starts, and after a via.
const NONE: u8 = 8;

/// What a step costs, in steps along a layer's own direction.
const VIA: f32 = 10.0;
const BEND45: f32 = 0.4;
const BEND90: f32 = 1.5;
const WRONG_WAY: f32 = 1.8;
const DIAG: f32 = 1.05;
/// What a via costs a net with a plane at twice the pitch, as a share of what it costs the
/// others: half, which is ten steps of the first pitch, as it is on a board of two layers
/// and in cadlab's router. At twice the pitch a via costs the others twenty steps of the
/// first, which keeps their vias down; a net with a plane then joined two of its pads by a
/// track of up to that length where each could go to the plane by a via where it is
/// (`glasgow-revC3`'s nine nets with zones: 674 mm of track for cadlab's router's 481, and
/// 91 vias for 120).
const PLANE_VIA: f32 = 0.5;
/// What a via costs a net with a plane at the first pitch, in steps: what a via costs when
/// the nets are routed again for fewer of them ([`FEWER_VIA`]), from the first round on. A
/// board of two layers has its pours on the layers its pads are on, and a pad that the
/// routes cut off its pour has two ways back: a track on its own layer to copper of its
/// net, or a via to the pour on the other side. At ten steps the via won wherever the
/// track was longer than 2 mm, and the pass for fewer vias, which has all of a net with
/// seventy pads to route again at once and keep or not, took none of them back
/// (`corne-cherry`'s two grounds: 130 to 157 vias over four orders, for the 65 to 71 of
/// cadlab's router, whose own pass takes a net a connection at a time). At forty they have
/// 92 to 111, and the board 427 to 459 vias for 459 to 519 (`docs/benchmarks.md`).
const PLANE_VIA_FIRST: f32 = 40.0;

/// The price of a claimed point at first, what it is multiplied by after each round, and
/// the most it is: past that, sharing is out of the question anyway, and a search only
/// goes further for it. At the first pitch the most is thirty times a step, at which a
/// net goes thirty steps round a point rather than share it, and what is still shared is
/// the history's to price, which rises for as long as a point is shared: at a thousand,
/// `corne-cherry` ended a minute with 566 vias and 8269 mm of track over four orders
/// where it has 506 and 7590, for as many connections. At twice the pitch it stays at a
/// thousand, as VPR has it and cadlab's router: `glasgow-revC3` had 33 to 36 connections
/// open over four orders at thirty where it has 22 to 27 (`docs/benchmarks.md`).
const PRES: f32 = 0.6;
const PRES_STEP: f32 = 1.6;
const PRES_MAX: f32 = 1000.0;
const PRES_MAX_FIRST: f32 = 30.0;
/// What a round adds to the price of a point that was shared in it.
const HISTORY: f32 = 0.5;
/// How many rounds there are at most, and how many may pass since the best.
const ROUNDS: usize = 100;
/// And at twice the pitch, where the price of a shared point goes on rising to a
/// thousand steps as it does in cadlab's router: as many as that router has. By the
/// fortieth the price has been at its most for twenty, and the rounds after it move the
/// same few nets from one shared point to the next while the vias and the track of all
/// of them rise (`tinytapeout-demo`: 44 points shared after the fortieth and 16 to 28
/// after some 150, with 349 to 388 vias for 394 to 458). What is still shared is taken
/// out and routed again by the passes, which have the time that the rounds leave: at 150
/// million nodes over six orders the board has 305 vias and 5037 mm at forty rounds for
/// 311.5 and 5412 mm at a hundred (`docs/benchmarks.md`).
const ROUNDS_COARSE: usize = 40;
const STALE: usize = 12;
/// How many times the first round a negotiation has to have room for in its share to be
/// worth beginning: the first round is each net once, with nothing priced yet, and a
/// board whose share ends before a few times that has routes that are still all over each
/// other, which cost the passes more than they give them. Such a board is given up as
/// soon as the first round says so, and the passes have the time.
const WHOLE_ROUNDS: f64 = 3.0;
/// And on the grid of twice the pitch: by the time, once, and by the work, thirty times.
/// The first round's time is mostly that of asking the board about each place for the
/// first time, which the later ones do not pay, and its nodes are a fraction of theirs,
/// which go round what is shared. `glasgow-revD1`, six layers and 1939 connections, is
/// the board these are set by: its first round is 2 million nodes and some 10 s on three
/// threads, five rounds are 60 million and 20 s, and its minute ends with 74 to 84
/// connections open for 125 to 135 with no negotiation; with 20 million nodes for it, two
/// rounds, the board has 599 open over three orders for 441 (`docs/benchmarks.md`).
const COARSE_ROUNDS: f64 = 1.0;
const COARSE_WORK: f64 = 30.0;
/// The share of the nets' length that the first round has routed when it says so.
const SAMPLE: f64 = 0.15;

/// A negotiation that its share ends in with few points still shared has as much again
/// of what the run has left as its share was of the whole, once: with fewer than one for
/// every this many connections, it is near its end, and the passes have little to do
/// after one that ends.
const NEARLY: usize = 4;

/// How many times the pads that the routes cut off their fills are told, and their nets
/// planned again with them, for the nets round them to make room.
const POUR_ROUNDS: usize = 3;
/// How many more rounds there may be for each of those times.
const POUR_MORE: usize = 40;
/// With the routes made legal before the pours are filled
/// ([`RouteOptions::legal_pours`]): the share of its part at which a negotiation that is
/// still going looks at what its routes cut off the pours, once, and ends its rounds
/// there for the rounds that plan those again if that is a piece of fill for every this
/// many connections or more. cadlab's router looks at 0.55 of its budget, where its
/// rounds end at 0.75.
const POUR_FIRST: f64 = 0.73;
const POUR_PAUSE: usize = 100;

/// What a via costs when the nets are routed again for fewer of them, and what a net may
/// be longer for each one less: a tenth, and eight steps for each.
const FEWER_VIA: f32 = 40.0;
const FEWER_STRETCH: f64 = 0.10;
const FEWER_SLACK: f64 = 8.0;

/// How many times what its first round took a negotiation that its share ended has for
/// routing again the nets it takes out and for fewer vias: the second is each net once
/// with nothing shared, which costs up to twice what the first round did.
const TAIL: f64 = 2.5;
/// And the most of what the run has left then that it has for them.
const TAIL_SHARE: f64 = 0.15;

/// The side of the blocks of cells that the world is asked about together.
const BLOCK: i32 = 8;

/// How many classes of halo and of rules a board may have for it to be negotiated whole.
const CLASSES: usize = 4;
const GROUPS: usize = 32;

/// How far from a pad's centre the cells are that a route may end on, in cells, and how
/// many of them a pad has at most on a layer.
const ACCESS_REACH: i32 = 2;
const ACCESS: usize = 12;

/// How far out of a pad that no cell beside it serves a way out is looked for along its
/// axis, in cells, and how many are kept for each way and line.
const WAY_OUT: i32 = 16;
const WAYS_OUT: usize = 3;

/// How many nodes nearest its targets a long search late in a negotiation has settled
/// backwards, once it has expanded twice as many, and the price from which it does.
const NEAR: usize = 5000;
const NEAR_PRES: f32 = 5.0;

/// How many nodes are looked at backwards from the targets of a search, over what the
/// board's own copper allows, to tell that they are walled in before the search goes over
/// the whole board to find that out.
const POCKET: usize = 20_000;

/// How many nets of a round are routed at once, against the claims as they are before
/// any of them: on whatever number of threads, so that the routes do not depend on it.
/// With more of them the threads have fewer waits for the longest search, and more nets
/// are routed without a word of those just before them.
const WAVE: usize = 8;

/// How far round the box of its two ends a search looks late in a negotiation, from the
/// price of [`NEAR_PRES`] on: this many cells at least, and this many hundredths of the
/// box's longer side. A search that has to enter a claimed point looks first at every
/// place that costs less than entering it, and at a price of a hundred steps and more
/// that is the board: on `glasgow-revD1`, six layers, the rounds from the ninth on cost
/// 9 to 12 million nodes each for some ninety nets and 2 to 3 million within the box,
/// with fewer points shared for the same number of nodes (`docs/benchmarks.md`). With
/// no way in the box, the whole board is looked at as before. At twice the pitch only:
/// at the first, on two layers, the price stops at thirty steps and a search goes no
/// further than that pays for, and the ways round that a board of two layers needs are
/// long ones (`corne-cherry` had 17 connections open over three orders at 150 million
/// nodes with it for 8).
const WINDOW: i32 = 16;
const WINDOW_SHARE: i32 = 50;

/// How many nodes at least a search may expand where nothing claimed is entered, once a
/// negotiation is over, and how many times what its net took the last time it was routed
/// across the claims: a net with no way left looks at every place that the board's own
/// copper leaves it before it says so, which on `glasgow-revD1` is 600 thousand nodes
/// for one net, and there are a hundred such.
const HARD_NODES: u64 = 100_000;
const HARD_TIMES: u64 = 8;

/// A terminal with more places than this is a plane's: a connection uses those near its
/// other end.
const MANY: usize = 64;
/// How far from the other end those are, and at least this many cells.
const PLANE_REACH: i64 = 4_000_000;
const PLANE_REACH_CELLS: i64 = 16;

/// The grid: cells a pitch apart, and the points halfway between them, where claims are
/// kept.
#[derive(Clone, Copy, Debug)]
struct Grid {
    ox: i64,
    oy: i64,
    g: i64,
    w: i32,
    h: i32,
}

impl Grid {
    fn cells(&self) -> usize {
        self.w as usize * self.h as usize
    }
    fn dw(&self) -> i32 {
        2 * self.w - 1
    }
    fn dh(&self) -> i32 {
        2 * self.h - 1
    }
    fn dpoints(&self) -> usize {
        self.dw() as usize * self.dh() as usize
    }
    fn cidx(&self, x: i32, y: i32) -> usize {
        y as usize * self.w as usize + x as usize
    }
    fn didx(&self, i: i32, j: i32) -> usize {
        j as usize * self.dw() as usize + i as usize
    }
    fn cell(&self, x: i32, y: i32) -> Point {
        Point::new(
            self.ox + i64::from(x) * self.g,
            self.oy + i64::from(y) * self.g,
        )
    }
    fn dpt(&self, i: i32, j: i32) -> (f64, f64) {
        (
            self.ox as f64 + f64::from(i) * self.g as f64 / 2.0,
            self.oy as f64 + f64::from(j) * self.g as f64 / 2.0,
        )
    }
    /// The cells in a box, if any.
    fn crange(&self, min: (f64, f64), max: (f64, f64)) -> Option<(i32, i32, i32, i32)> {
        let g = self.g as f64;
        let x0 = (((min.0 - self.ox as f64) / g).ceil() as i64).max(0);
        let y0 = (((min.1 - self.oy as f64) / g).ceil() as i64).max(0);
        let x1 = (((max.0 - self.ox as f64) / g).floor() as i64).min(i64::from(self.w) - 1);
        let y1 = (((max.1 - self.oy as f64) / g).floor() as i64).min(i64::from(self.h) - 1);
        (x0 <= x1 && y0 <= y1).then_some((x0 as i32, y0 as i32, x1 as i32, y1 as i32))
    }
    /// The points of the double grid in a box, if any.
    fn drange(&self, min: (f64, f64), max: (f64, f64)) -> Option<(i32, i32, i32, i32)> {
        let g = self.g as f64 / 2.0;
        let x0 = (((min.0 - self.ox as f64) / g).ceil() as i64).max(0);
        let y0 = (((min.1 - self.oy as f64) / g).ceil() as i64).max(0);
        let x1 = (((max.0 - self.ox as f64) / g).floor() as i64).min(i64::from(self.dw()) - 1);
        let y1 = (((max.1 - self.oy as f64) / g).floor() as i64).min(i64::from(self.dh()) - 1);
        (x0 <= x1 && y0 <= y1).then_some((x0 as i32, y0 as i32, x1 as i32, y1 as i32))
    }
}

/// What a net's copper keeps other nets' away by: its width, its clearance and its via.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Halo {
    width: i64,
    clearance: i64,
    via: Option<(i64, i64)>,
}

/// The nets that the board's own copper is judged the same for, and what it says of them,
/// asked of the world a block of cells at a time as searches reach them.
struct Group {
    net: NetId,
    alone: bool,
    widths: Vec<i64>,
    via: Option<(i64, i64)>,
    /// By layer and cell, two bits for each direction: 0 free, 1 blocked, 2 only for the
    /// net in `owners`.
    moves: Vec<AtomicU16>,
    /// By layer and cell, the same for a track's end there.
    points: Vec<AtomicU8>,
    /// By cell, for a via: 0 not asked yet, then 1 free, 2 blocked, 3 only for a net.
    vias: Vec<AtomicU8>,
    /// By layer and block, whether it was asked about.
    filled: Vec<AtomicU8>,
    /// By layer and block, once it was asked about, the nets that its places are only
    /// for, by their keys in order: nothing is locked to look one up.
    owners: Vec<OnceLock<Owners>>,
    /// By cell, the net that a via there is only for.
    via_owner: Vec<AtomicU32>,
}

/// The nets that the places of a block are only for, by their keys in order.
type Owners = Box<[(u64, u32)]>;

/// A price that the threads read while one of them raises it between two rounds.
#[inline]
fn price(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

#[inline]
fn count(a: &AtomicU16) -> u16 {
    a.load(Ordering::Relaxed)
}

const FREE: u16 = 0;
const BLOCKED: u16 = 1;
const ONLY: u16 = 2;

/// A cell that a route of a net may end on at a pad.
#[derive(Clone, Copy, Debug)]
struct Access {
    node: u32,
    /// The way from the pad to the cell, where the cell is not on the pad's centre.
    stub: Option<Stub>,
    cost: f32,
}

/// The way from a pad to a cell: the pad's centre, and where the track turns between the
/// two, if it does.
type Stub = (Point, Option<Point>);

/// A path between two parts of a net.
#[derive(Clone, Debug)]
struct Wire {
    nodes: Vec<u32>,
    start: Option<Stub>,
    end: Option<Stub>,
    /// The terminals that it begins and ends at, where it does not begin or end on
    /// another wire of its net.
    from: Option<usize>,
    to: Option<usize>,
}

/// A net as it is negotiated for.
#[derive(Clone)]
struct WNet {
    job: usize,
    net: NetId,
    halo: usize,
    group: usize,
    /// By terminal, where routes may end.
    terms: Vec<Vec<Access>>,
    /// By terminal, the box of its pads' centres.
    boxes: Vec<Rect>,
    /// The terminals to join, two by two: a spanning tree, nearest first.
    conns: Vec<(usize, usize)>,
    /// By connection, whether it has no way even across the other nets.
    dead: Vec<bool>,
    /// By connection, whether it was looked at for an end that is walled in.
    looked: Vec<bool>,
    wires: Vec<Wire>,
    keys: Vec<u64>,
    /// The cells its wires are within.
    bbox: (i32, i32, i32, i32),
    /// The length of its connections as the crow flies, which orders the nets.
    length: i64,
    /// What a via costs it in the rounds, in steps.
    via: f32,
}

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// What a search knows of a node, in one place in memory: a search looks at the nodes
/// round the one it is at, and each of them is one line of the cache where they were
/// four, by what was kept.
#[derive(Clone, Copy)]
#[repr(C)]
struct Slot {
    /// The search that the rest is of, by its stamp.
    seen: u32,
    cost: f32,
    parent: u32,
    dir: u8,
    /// The search that the node is a target of, by its stamp for targets.
    target: u16,
}

/// A search's own memory.
struct Scratch {
    slots: Vec<Slot>,
    stamp: u32,
    target_stamp: u16,
    heap: Ranked,
    /// By cell, whether a via of the net that is being routed would be too near another
    /// of its own wires' vias, hole to hole: by the stamp of the net's routing.
    own_near: Vec<u32>,
    own_stamp: u32,
    near: Vec<EntryId>,
    /// What the nodes nearest a search's targets cost to them, by stamp.
    lb_cost: Vec<f32>,
    lb_seen: Vec<u32>,
    lb_stamp: u32,
    lb_heap: Heap,
    /// By point of the double grid on each layer, and by cell, whether the net that is
    /// being routed claims it itself, for a net routed with its claims still in place.
    own_t: Vec<u8>,
    own_v: Vec<u8>,
}

/// A binary heap of costs and nodes, the least first, and of equal costs the least node.
#[derive(Default)]
struct Heap {
    items: Vec<(f32, u32)>,
}

impl Heap {
    fn clear(&mut self) {
        self.items.clear();
    }
    fn less(a: (f32, u32), b: (f32, u32)) -> bool {
        a.0 < b.0 || (a.0 == b.0 && a.1 < b.1)
    }
    fn push(&mut self, cost: f32, node: u32) {
        let mut i = self.items.len();
        self.items.push((cost, node));
        while i > 0 {
            let p = (i - 1) / 2;
            if !Heap::less(self.items[i], self.items[p]) {
                break;
            }
            self.items.swap(i, p);
            i = p;
        }
    }
    fn pop(&mut self) -> Option<(f32, u32)> {
        let n = self.items.len();
        if n == 0 {
            return None;
        }
        self.items.swap(0, n - 1);
        let top = self.items.pop();
        let n = n - 1;
        let mut i = 0;
        loop {
            let (l, r) = (2 * i + 1, 2 * i + 2);
            let mut m = i;
            if l < n && Heap::less(self.items[l], self.items[m]) {
                m = l;
            }
            if r < n && Heap::less(self.items[r], self.items[m]) {
                m = r;
            }
            if m == i {
                break;
            }
            self.items.swap(i, m);
            i = m;
        }
        top
    }
}

/// How many parts of a step the costs of a search are told apart by, where the order of
/// its nodes is told: of two nodes that the way through them costs the same to within
/// one, the one nearer the targets is looked at first. On a grid where a step costs the
/// same along a line as its bound says, every node between two ends that one bend
/// reaches is on a way that costs what the best way does, short of the bend still to
/// come, which no bound counts: by their costs alone, a search looks at all of them
/// before it reaches its target, and at a line of them with this.
const PARTS: f32 = 16.0;

/// The place of a node in the order of a search: the cost of the way through it first,
/// then how far it is from the targets.
#[inline]
fn rank(f: f32, h: f32) -> u64 {
    (u64::from((f * PARTS) as u32) << 32) | u64::from((h * PARTS) as u32)
}

/// A heap of ranks and nodes, the least first, and of equal ranks the least node: each
/// item above four others, so that a pop goes down half as many levels as with two, and
/// the four it chooses among are next to each other. A rank and its node are one number,
/// the rank above the node, which two of them are compared as at once.
#[derive(Default)]
struct Ranked {
    items: Vec<u128>,
}

impl Ranked {
    fn clear(&mut self) {
        self.items.clear();
    }
    #[inline]
    fn push(&mut self, rank: u64, node: u32) {
        let item = (u128::from(rank) << 32) | u128::from(node);
        let mut i = self.items.len();
        self.items.push(item);
        let items = self.items.as_mut_slice();
        while i > 0 {
            let p = (i - 1) / 4;
            let up = items[p];
            if item >= up {
                break;
            }
            items[i] = up;
            i = p;
        }
        items[i] = item;
    }
    #[inline]
    fn pop(&mut self) -> Option<(u64, u32)> {
        let last = self.items.pop()?;
        let items = self.items.as_mut_slice();
        let n = items.len();
        let Some(&top) = items.first() else {
            return Some(((last >> 32) as u64, last as u32));
        };
        // The last item goes down from the top to where no item under it is less.
        let mut i = 0;
        loop {
            let first = 4 * i + 1;
            let (m, least) = if first + 4 <= n {
                let four: &[u128; 4] = items[first..first + 4]
                    .try_into()
                    .unwrap_or(&[u128::MAX; 4]);
                let (a, x) = if four[1] < four[0] {
                    (1, four[1])
                } else {
                    (0, four[0])
                };
                let (b, y) = if four[3] < four[2] {
                    (3, four[3])
                } else {
                    (2, four[2])
                };
                if y < x {
                    (first + b, y)
                } else {
                    (first + a, x)
                }
            } else if first < n {
                let mut m = first;
                let mut least = items[first];
                for (k, &it) in items[first + 1..n].iter().enumerate() {
                    if it < least {
                        least = it;
                        m = first + 1 + k;
                    }
                }
                (m, least)
            } else {
                break;
            };
            if last <= least {
                break;
            }
            items[i] = least;
            i = m;
        }
        items[i] = last;
        Some(((top >> 32) as u64, top as u32))
    }
}

/// How a search treats claimed points.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// They cost this much more for each net that claims them.
    Share(f32),
    /// They are not entered.
    Hard,
}

/// A search was stopped: the run has to end, or the part of it that this is.
struct Stop;

/// A shared point: on a layer's double grid, or a via's cell.
type Conflict = (Option<u8>, u32);

/// How many steps a turn from one direction to another is, of eighths of a circle.
fn turn(a: u8, b: u8) -> u8 {
    let d = a.abs_diff(b);
    d.min(8 - d)
}

fn dist2_point_seg(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (ux, uy) = (b.0 - a.0, b.1 - a.1);
    let (vx, vy) = (p.0 - a.0, p.1 - a.1);
    let len = ux * ux + uy * uy;
    let t = if len > 0.0 {
        ((ux * vx + uy * vy) / len).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (dx, dy) = (vx - t * ux, vy - t * uy);
    dx * dx + dy * dy
}

fn fp(p: Point) -> (f64, f64) {
    (p.x as f64, p.y as f64)
}

/// What stops a search: the run's flag, which the thread that the run is on sets as it
/// looks at the clock, and a number of nodes.
#[derive(Clone, Copy)]
struct Ctl<'a, 'r> {
    run: Option<&'a Run<'r>>,
    flag: &'a AtomicBool,
    limit: u64,
}

/// The whole of a negotiation: the grid and what is claimed on it.
struct Board {
    grid: Grid,
    layers: usize,
    halos: Vec<Halo>,
    groups: Vec<Group>,
    /// By class of halo, then layer and point of the double grid, how many nets claim it.
    occ_t: Vec<Vec<AtomicU16>>,
    /// By class of halo and cell, how many nets claim it against a via.
    occ_v: Vec<Vec<AtomicU16>>,
    hist_t: Vec<AtomicU32>,
    hist_v: Vec<AtomicU32>,
    /// By layer and cell, the net that a spoke's room there is of, plus one.
    spoke: Vec<u32>,
    spoke_cost: f32,
    /// By layer, the direction its tracks run along: 0 across, 1 up, 2 either.
    along: Vec<u8>,
    hole_clearance: i64,
    hole_to_hole: i64,
    /// What a via costs more on a grid finer than the pitch when the nets are routed again
    /// for fewer of them: as many steps more as the steps are shorter, so that one costs
    /// the same length of track. In the rounds a via costs its ten steps on any grid:
    /// dearer there, `nrfmicro` ended fewer of its negotiations with nothing shared (5
    /// connections open over twelve orders for 2).
    via_scale: f32,
    /// Whether a search that is kept round its two ends and finds no way there is over:
    /// it is not made again over the whole board.
    near_only: bool,
}

fn code(class: Class, key: u64, owners: &mut Vec<(u64, u32)>) -> u16 {
    match class {
        Class::Free => FREE,
        Class::Blocked => BLOCKED,
        Class::Only(n) => {
            owners.push((key, n.0));
            ONLY
        }
    }
}

fn owner_key(kind: u64, at: usize, dir: usize) -> u64 {
    kind << 60 | (at as u64) << 3 | dir as u64
}

impl Board {
    fn node(&self, layer: usize, x: i32, y: i32) -> u32 {
        (layer * self.grid.cells() + self.grid.cidx(x, y)) as u32
    }

    #[inline]
    fn unpack(&self, n: u32) -> (usize, i32, i32) {
        // A node is numbered in 32 bits, and so are the cells: divided as such.
        let (cells, w) = (self.grid.cells() as u32, self.grid.w as u32);
        let (l, c) = (n / cells, n % cells);
        (l as usize, (c % w) as i32, (c / w) as i32)
    }

    fn node_point(&self, n: u32) -> Point {
        let (_, x, y) = self.unpack(n);
        self.grid.cell(x, y)
    }

    fn key(&self, halo: usize, map: usize, idx: usize) -> u64 {
        (((halo * (self.layers + 1) + map) as u64) << 32) | idx as u64
    }

    fn blocks(&self) -> (i32, usize) {
        let w = (self.grid.w + BLOCK - 1) / BLOCK;
        (w, w as usize * ((self.grid.h + BLOCK - 1) / BLOCK) as usize)
    }

    // ---- what the board's own copper allows ----

    /// Asks the world about a block of cells on a layer, for a group.
    fn fill(
        &self,
        world: &World<'_>,
        gi: usize,
        layer: usize,
        bx: i32,
        by: i32,
        near: &mut Vec<EntryId>,
    ) {
        let group = &self.groups[gi];
        let grid = &self.grid;
        let cells = grid.cells();
        let (x0, y0) = (bx * BLOCK, by * BLOCK);
        let (x1, y1) = ((x0 + BLOCK).min(grid.w), (y0 + BLOCK).min(grid.h));
        let width = group.widths[layer];
        let rect = Rect::new(grid.cell(x0 - 1, y0 - 1), grid.cell(x1, y1))
            .expand(width / 2 + 1 + world.reach());
        let l = LayerId(layer as u8);
        world.near(l, &rect, near);
        let class = |shape: &Shape| -> Class {
            if group.alone {
                match world.track_among(group.net, l, shape, near) {
                    Verdict::Free => Class::Free,
                    _ => Class::Blocked,
                }
            } else {
                world.track_class(group.net, l, shape, near)
            }
        };
        let mut owners: Vec<(u64, u32)> = Vec::new();
        // With nothing near, every place in the block is as its first is.
        let empty = near.is_empty();
        let probe = if empty {
            Shape::circle(grid.cell(x0, y0), width)
                .map(|s| class(&s))
                .unwrap_or(Class::Blocked)
        } else {
            Class::Free
        };
        for y in y0..y1 {
            for x in x0..x1 {
                let at = layer * cells + grid.cidx(x, y);
                let p = grid.cell(x, y);
                // A disc that holds every step from the cell: free, they all are.
                let open = !empty
                    && Shape::circle(p, width + 3 * grid.g).is_ok_and(|s| class(&s) == Class::Free);
                let point = if empty {
                    probe
                } else if open {
                    Class::Free
                } else {
                    Shape::circle(p, width)
                        .map(|s| class(&s))
                        .unwrap_or(Class::Blocked)
                };
                let c = code(point, owner_key(1, at, 0), &mut owners);
                group.points[at].store(c as u8, Ordering::Relaxed);
                let mut bits = 0u16;
                for (d, &(dx, dy)) in DIRS.iter().enumerate() {
                    let (nx, ny) = (x + dx, y + dy);
                    // The step back from a cell of the block that was asked about before
                    // this one is the same track.
                    if nx >= x0 && nx < x1 && ny >= y0 && (ny < y || (ny == y && nx < x)) {
                        let from = layer * cells + grid.cidx(nx, ny);
                        let back = (d + 4) % 8;
                        let c = (group.moves[from].load(Ordering::Relaxed) >> (2 * back)) & 3;
                        if c == ONLY {
                            let key = owner_key(0, from, back);
                            if let Some(&(_, n)) = owners.iter().rev().find(|o| o.0 == key) {
                                owners.push((owner_key(0, at, d), n));
                            }
                        }
                        bits |= c << (2 * d);
                        continue;
                    }
                    let c = if nx < 0 || ny < 0 || nx >= grid.w || ny >= grid.h {
                        Class::Blocked
                    } else if empty {
                        probe
                    } else if open {
                        Class::Free
                    } else {
                        Shape::track(p, grid.cell(nx, ny), width)
                            .map(|s| class(&s))
                            .unwrap_or(Class::Blocked)
                    };
                    bits |= code(c, owner_key(0, at, d), &mut owners) << (2 * d);
                }
                group.moves[at].store(bits, Ordering::Relaxed);
            }
        }
        let (blocks_w, blocks) = self.blocks();
        let block = layer * blocks + (by * blocks_w + bx) as usize;
        // Two threads that ask about a block at once find the same.
        owners.sort_unstable();
        let _ = group.owners[block].set(owners.into_boxed_slice());
        group.filled[block].store(1, Ordering::Release);
    }

    /// Makes sure the block of a cell on a layer was asked about.
    #[inline]
    fn ensure(
        &self,
        world: &World<'_>,
        gi: usize,
        layer: usize,
        x: i32,
        y: i32,
        near: &mut Vec<EntryId>,
    ) {
        let (blocks_w, blocks) = self.blocks();
        let (bx, by) = (x / BLOCK, y / BLOCK);
        if self.groups[gi].filled[layer * blocks + (by * blocks_w + bx) as usize]
            .load(Ordering::Acquire)
            == 0
        {
            self.fill(world, gi, layer, bx, by, near);
        }
    }

    /// Whether a place on a layer that is only for a net is for `net`: by the key of the
    /// place, in a block that was asked about.
    fn owned(&self, gi: usize, key: u64, net: NetId) -> bool {
        let at = ((key & ((1 << 60) - 1)) >> 3) as usize;
        let cells = self.grid.cells();
        let (l, c) = (at / cells, at % cells);
        let (x, y) = (
            (c % self.grid.w as usize) as i32,
            (c / self.grid.w as usize) as i32,
        );
        let (blocks_w, blocks) = self.blocks();
        let block = l * blocks + ((y / BLOCK) * blocks_w + x / BLOCK) as usize;
        self.groups[gi].owners[block].get().is_some_and(|o| {
            o.binary_search_by_key(&key, |e| e.0)
                .is_ok_and(|k| o[k].1 == net.0)
        })
    }

    /// Whether a track's end of `net` may be at a node.
    fn point_ok(
        &self,
        world: &World<'_>,
        gi: usize,
        net: NetId,
        n: u32,
        near: &mut Vec<EntryId>,
    ) -> bool {
        let (l, x, y) = self.unpack(n);
        self.point_ok_at(world, gi, net, n, (l, x, y), near)
    }

    /// [`point_ok`](Board::point_ok) for a node whose layer and cell are at hand.
    #[inline]
    fn point_ok_at(
        &self,
        world: &World<'_>,
        gi: usize,
        net: NetId,
        n: u32,
        (l, x, y): (usize, i32, i32),
        near: &mut Vec<EntryId>,
    ) -> bool {
        self.ensure(world, gi, l, x, y, near);
        match u16::from(self.groups[gi].points[n as usize].load(Ordering::Relaxed)) {
            FREE => true,
            ONLY => self.owned(gi, owner_key(1, n as usize, 0), net),
            _ => false,
        }
    }

    /// Whether a via of `net` may be at a cell.
    fn via_ok(&self, world: &World<'_>, gi: usize, net: NetId, x: i32, y: i32) -> bool {
        let group = &self.groups[gi];
        let Some((d, drill)) = group.via else {
            return false;
        };
        let cell = self.grid.cidx(x, y);
        let mut c = group.vias[cell].load(Ordering::Acquire);
        if c == 0 {
            let at = self.grid.cell(x, y);
            let class = if group.alone {
                match world.via(group.net, at, d, drill, None) {
                    Verdict::Free => Class::Free,
                    _ => Class::Blocked,
                }
            } else {
                world.via_class(group.net, at, d, drill)
            };
            c = match class {
                Class::Free => 1,
                Class::Blocked => 2,
                Class::Only(n) => {
                    group.via_owner[cell].store(n.0, Ordering::Release);
                    3
                }
            };
            group.vias[cell].store(c, Ordering::Release);
        }
        match c {
            1 => true,
            3 => group.via_owner[cell].load(Ordering::Acquire) == net.0,
            _ => false,
        }
    }

    // ---- claims ----

    /// How far a track's centre of class `b` keeps from a track's of class `a`.
    fn track_track(&self, a: &Halo, b: &Halo) -> f64 {
        (a.width / 2 + b.width / 2 + a.clearance.max(b.clearance)) as f64
    }

    /// How far a via's centre of class `v` keeps from a track's centre of class `t`.
    fn track_via(&self, t: &Halo, v: &Halo) -> Option<f64> {
        let (d, drill) = v.via?;
        let copper = d / 2 + t.clearance.max(v.clearance);
        let hole = drill / 2 + self.hole_clearance;
        Some((t.width / 2 + copper.max(hole)) as f64)
    }

    fn via_via(&self, a: &Halo, b: &Halo) -> Option<f64> {
        let ((da, ha), (db, hb)) = (a.via?, b.via?);
        let copper = da / 2 + db / 2 + a.clearance.max(b.clearance);
        let holes = ha / 2 + hb / 2 + self.hole_to_hole;
        let mixed = (da / 2 + hb / 2).max(db / 2 + ha / 2) + self.hole_clearance;
        Some(copper.max(holes).max(mixed) as f64)
    }

    /// How much further copper that is off the grid keeps a track's centre than `r`,
    /// the distance that the rules ask: a track on the grid is looked at where its points
    /// are, half a step apart along a diagonal at most, and passes nearer than two of
    /// them in a row are to what lies between them.
    fn between(&self, r: f64) -> f64 {
        let s = self.grid.g as f64 * core::f64::consts::SQRT_2 / 2.0;
        (r * r + s * s / 4.0).sqrt() - r
    }

    /// Whether a point is a cell of the grid.
    fn on_grid(&self, p: Point) -> bool {
        (p.x - self.grid.ox).rem_euclid(self.grid.g) == 0
            && (p.y - self.grid.oy).rem_euclid(self.grid.g) == 0
    }

    /// The keys that copper of a net claims, wherever it is: tracks between cells of the
    /// grid along its eight directions and vias on its cells as wires claim them, and
    /// the rest by as much more as keeps a track on the grid clear of it between two of
    /// the grid's points.
    fn copper_keys(&self, halo: usize, c: &NetCopper) -> Vec<u64> {
        let mut keys = Vec::new();
        for s in &c.segs {
            let l = usize::from(s.l);
            if l >= self.layers {
                continue;
            }
            let (dx, dy) = (s.b.x - s.a.x, s.b.y - s.a.y);
            let on = self.on_grid(s.a)
                && self.on_grid(s.b)
                && (dx == 0 || dy == 0 || dx.abs() == dy.abs());
            self.claim_seg(halo, l, fp(s.a), fp(s.b), !on, &mut keys);
        }
        for &v in &c.vias {
            self.claim_via(halo, fp(v), !self.on_grid(v), &mut keys);
        }
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// Adds a net's claims to those of the others, or takes them away again.
    fn occupy(&self, keys: &[u64], add: bool) {
        for &k in keys {
            let (hi, map, idx) = self.unkey(k);
            let at = if map == self.layers {
                &self.occ_v[hi][idx]
            } else {
                &self.occ_t[hi][idx]
            };
            let n = count(at);
            at.store(
                if add {
                    n.saturating_add(1)
                } else {
                    n.saturating_sub(1)
                },
                Ordering::Relaxed,
            );
        }
    }

    /// The keys that a piece of track of a net claims.
    fn claim_seg(
        &self,
        halo: usize,
        layer: usize,
        a: (f64, f64),
        b: (f64, f64),
        off: bool,
        out: &mut Vec<u64>,
    ) {
        let mine = self.halos[halo];
        let (min, max) = ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1)));
        let dn = self.grid.dpoints();
        for (hi, theirs) in self.halos.iter().enumerate() {
            let r = self.track_track(&mine, theirs) - 1.0;
            let r = if off { r + self.between(r) } else { r };
            if let Some((i0, j0, i1, j1)) = self
                .grid
                .drange((min.0 - r, min.1 - r), (max.0 + r, max.1 + r))
            {
                for j in j0..=j1 {
                    for i in i0..=i1 {
                        if dist2_point_seg(self.grid.dpt(i, j), a, b) < r * r {
                            out.push(self.key(hi, layer, layer * dn + self.grid.didx(i, j)));
                        }
                    }
                }
            }
            let Some(rv) = self.track_via(&mine, theirs) else {
                continue;
            };
            let rv = rv - 1.0;
            if let Some((x0, y0, x1, y1)) = self
                .grid
                .crange((min.0 - rv, min.1 - rv), (max.0 + rv, max.1 + rv))
            {
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        if dist2_point_seg(fp(self.grid.cell(x, y)), a, b) < rv * rv {
                            out.push(self.key(hi, self.layers, self.grid.cidx(x, y)));
                        }
                    }
                }
            }
        }
    }

    /// The keys that a via of a net claims.
    fn claim_via(&self, halo: usize, at: (f64, f64), off: bool, out: &mut Vec<u64>) {
        let mine = self.halos[halo];
        let dn = self.grid.dpoints();
        for (hi, theirs) in self.halos.iter().enumerate() {
            if let Some(r) = self.track_via(theirs, &mine) {
                let r = r - 1.0;
                let r = if off { r + self.between(r) } else { r };
                if let Some((i0, j0, i1, j1)) =
                    self.grid.drange((at.0 - r, at.1 - r), (at.0 + r, at.1 + r))
                {
                    for j in j0..=j1 {
                        for i in i0..=i1 {
                            let p = self.grid.dpt(i, j);
                            let (dx, dy) = (p.0 - at.0, p.1 - at.1);
                            if dx * dx + dy * dy < r * r {
                                let idx = self.grid.didx(i, j);
                                for l in 0..self.layers {
                                    out.push(self.key(hi, l, l * dn + idx));
                                }
                            }
                        }
                    }
                }
            }
            if let Some(r) = self.via_via(&mine, theirs) {
                let r = r - 1.0;
                if let Some((x0, y0, x1, y1)) =
                    self.grid.crange((at.0 - r, at.1 - r), (at.0 + r, at.1 + r))
                {
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            let p = fp(self.grid.cell(x, y));
                            let (dx, dy) = (p.0 - at.0, p.1 - at.1);
                            if dx * dx + dy * dy < r * r {
                                out.push(self.key(hi, self.layers, self.grid.cidx(x, y)));
                            }
                        }
                    }
                }
            }
        }
    }

    /// A wire's straight runs, by layer, and its vias. The ways between its ends and
    /// their pads are among the runs: off the grid, they claim what they are near on it
    /// as the others do, and what that misses between two of its points is found when
    /// the routes are given to the world.
    fn geometry(&self, w: &Wire) -> (Vec<(usize, Point, Point)>, Vec<Point>) {
        let mut segs = Vec::new();
        let mut vias = Vec::new();
        let (Some(&first), Some(&last)) = (w.nodes.first(), w.nodes.last()) else {
            return (segs, vias);
        };
        let cells = self.grid.cells();
        if let Some((c, bend)) = w.start {
            let l = first as usize / cells;
            let end = self.node_point(first);
            match bend {
                Some(q) => {
                    segs.push((l, c, q));
                    segs.push((l, q, end));
                }
                None => segs.push((l, c, end)),
            }
        }
        let mut k = 0;
        while k + 1 < w.nodes.len() {
            let (a, b) = (w.nodes[k], w.nodes[k + 1]);
            let (la, lb) = (a as usize / cells, b as usize / cells);
            if la != lb {
                vias.push(self.node_point(a));
                k += 1;
                continue;
            }
            // A run goes on for as long as its direction does, eight steps at most, so
            // that the box of a run on a diagonal is not looked at whole for nothing.
            let (pa, pb) = (self.node_point(a), self.node_point(b));
            let d = (pb.x - pa.x, pb.y - pa.y);
            let mut e = k + 1;
            while e + 1 < w.nodes.len() && e - k < 8 {
                let (c, n) = (w.nodes[e], w.nodes[e + 1]);
                if n as usize / cells != la {
                    break;
                }
                let (pc, pn) = (self.node_point(c), self.node_point(n));
                if (pn.x - pc.x, pn.y - pc.y) != d {
                    break;
                }
                e += 1;
            }
            segs.push((la, pa, self.node_point(w.nodes[e])));
            k = e;
        }
        if let Some((c, bend)) = w.end {
            let l = last as usize / cells;
            let from = self.node_point(last);
            match bend {
                Some(q) => {
                    segs.push((l, from, q));
                    segs.push((l, q, c));
                }
                None => segs.push((l, from, c)),
            }
        }
        (segs, vias)
    }

    /// Claims what a net's wires keep others away from.
    fn commit(&self, net: &mut WNet) {
        let mut keys = Vec::new();
        let mut bbox = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for w in &net.wires {
            let (segs, vias) = self.geometry(w);
            for (l, a, b) in segs {
                self.claim_seg(net.halo, l, fp(a), fp(b), false, &mut keys);
            }
            for v in vias {
                self.claim_via(net.halo, fp(v), false, &mut keys);
            }
            if w.nodes.len() == 1 {
                let (l, _, _) = self.unpack(w.nodes[0]);
                let p = fp(self.node_point(w.nodes[0]));
                self.claim_seg(net.halo, l, p, p, false, &mut keys);
            }
            for &n in &w.nodes {
                let (_, x, y) = self.unpack(n);
                bbox = (bbox.0.min(x), bbox.1.min(y), bbox.2.max(x), bbox.3.max(y));
            }
        }
        keys.sort_unstable();
        keys.dedup();
        for &k in &keys {
            let (m, idx) = ((k >> 32) as usize, (k & 0xFFFF_FFFF) as usize);
            let (hi, map) = (m / (self.layers + 1), m % (self.layers + 1));
            let at = if map == self.layers {
                &self.occ_v[hi][idx]
            } else {
                &self.occ_t[hi][idx]
            };
            at.store(count(at).saturating_add(1), Ordering::Relaxed);
        }
        net.keys = keys;
        net.bbox = bbox;
    }

    /// Gives up a net's claims.
    fn uncommit(&self, net: &mut WNet) {
        for &k in &net.keys {
            let (m, idx) = ((k >> 32) as usize, (k & 0xFFFF_FFFF) as usize);
            let (hi, map) = (m / (self.layers + 1), m % (self.layers + 1));
            let at = if map == self.layers {
                &self.occ_v[hi][idx]
            } else {
                &self.occ_t[hi][idx]
            };
            at.store(count(at).saturating_sub(1), Ordering::Relaxed);
        }
        net.keys.clear();
    }

    /// The points where a net's wires are in what another net claims.
    fn conflicts(&self, net: &WNet) -> Vec<Conflict> {
        let mut out = Vec::new();
        for w in &net.wires {
            self.wire_conflicts(net, w, &mut out);
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The points where one wire of a net is in what another net claims, added to `out`.
    fn wire_conflicts(&self, net: &WNet, w: &Wire, out: &mut Vec<Conflict>) {
        let cells = self.grid.cells();
        let dn = self.grid.dpoints();
        let occ = &self.occ_t[net.halo];
        {
            for (k, &n) in w.nodes.iter().enumerate() {
                let (l, x, y) = self.unpack(n);
                if k == 0 {
                    let idx = self.grid.didx(2 * x, 2 * y);
                    if count(&occ[l * dn + idx]) > 1 {
                        out.push((Some(l as u8), idx as u32));
                    }
                    continue;
                }
                let p = w.nodes[k - 1];
                if p as usize / cells != l {
                    let idx = self.grid.cidx(x, y);
                    if count(&self.occ_v[net.halo][idx]) > 1 {
                        out.push((None, idx as u32));
                    }
                    continue;
                }
                let (_, px, py) = self.unpack(p);
                for (i, j) in [(px + x, py + y), (2 * x, 2 * y)] {
                    let idx = self.grid.didx(i, j);
                    if count(&occ[l * dn + idx]) > 1 {
                        out.push((Some(l as u8), idx as u32));
                    }
                }
            }
        }
    }

    /// The cell nearest a shared point.
    fn conflict_cell(&self, c: Conflict) -> (i32, i32) {
        match c.0 {
            Some(_) => {
                let dw = self.grid.dw() as u32;
                (((c.1 % dw) / 2) as i32, ((c.1 / dw) / 2) as i32)
            }
            None => (
                (c.1 % self.grid.w as u32) as i32,
                (c.1 / self.grid.w as u32) as i32,
            ),
        }
    }

    fn add_history(&self, c: Conflict, amount: f32) {
        let at = match c.0 {
            Some(l) => &self.hist_t[usize::from(l) * self.grid.dpoints() + c.1 as usize],
            None => &self.hist_v[c.1 as usize],
        };
        at.store((price(at) + amount).to_bits(), Ordering::Relaxed);
    }

    // ---- searching ----

    fn scratch(&self) -> Scratch {
        let n = self.layers * self.grid.cells();
        Scratch {
            slots: vec![
                Slot {
                    seen: 0,
                    cost: 0.0,
                    parent: 0,
                    dir: NONE,
                    target: 0,
                };
                n
            ],
            stamp: 0,
            target_stamp: 0,
            heap: Ranked::default(),
            own_near: vec![0; self.grid.cells()],
            own_stamp: 0,
            near: Vec::new(),
            lb_cost: vec![0.0; n],
            lb_seen: vec![0; n],
            lb_stamp: 0,
            lb_heap: Heap::default(),
            own_t: vec![0; self.layers * self.grid.dpoints()],
            own_v: vec![0; self.grid.cells()],
        }
    }

    /// Whether the targets of a search are walled in by the board's own copper, with no
    /// source among them: everything that a target can be reached from, whatever other
    /// nets claim, is looked at backwards from the targets, [`POCKET`] nodes at most. A
    /// search for what has no way would look at everything that the sources reach before
    /// it said so, which on a board of several layers is most of it.
    ///
    /// The answer does not depend on the order the nodes are looked at in: it is yes
    /// where what reaches the targets is [`POCKET`] nodes at most with no source among
    /// them. So the nodes nearest a source are looked at first, and where there is a way
    /// it is most often found in about as many nodes as it is long.
    fn pocket(
        &self,
        world: &World<'_>,
        sc: &mut Scratch,
        net: &WNet,
        sources: &[(u32, f32)],
        targets: &[u32],
    ) -> bool {
        sc.lb_stamp = sc.lb_stamp.wrapping_add(1);
        if sc.lb_stamp == 0 {
            sc.lb_seen.iter_mut().for_each(|v| *v = 0);
            sc.lb_stamp = 1;
        }
        let stamp = sc.lb_stamp;
        let (gi, id) = (net.group, net.net);
        let grid = self.grid;
        let cells = grid.cells();
        let gw = grid.w as usize;
        let has_via = self.groups[gi].via.is_some() && self.layers > 1;
        let Some(&first) = targets.first() else {
            return true;
        };
        // The source nearest the first target is what the nodes are ordered by.
        let (_, tx, ty) = self.unpack(first);
        let mut aim = (0usize, tx, ty);
        let mut best = i32::MAX;
        // The sources are marked in the same map, by a cost that no node has.
        for &(n, _) in sources {
            sc.lb_seen[n as usize] = stamp;
            sc.lb_cost[n as usize] = -1.0;
            let (l, x, y) = self.unpack(n);
            let d = (x - tx).abs().max((y - ty).abs());
            if d < best {
                best = d;
                aim = (l, x, y);
            }
        }
        let far = |l: usize, x: i32, y: i32| -> f32 {
            (2 * (x - aim.1).abs().max((y - aim.2).abs()) + i32::from(l != aim.0)) as f32
        };
        sc.lb_heap.clear();
        for &t in targets {
            let i = t as usize;
            if sc.lb_seen[i] == stamp {
                if sc.lb_cost[i] < 0.0 {
                    return false;
                }
                continue;
            }
            sc.lb_seen[i] = stamp;
            sc.lb_cost[i] = 0.0;
            let (l, x, y) = self.unpack(t);
            sc.lb_heap.push(far(l, x, y), t);
        }
        let mut seen = 0usize;
        while let Some((_, v)) = sc.lb_heap.pop() {
            seen += 1;
            if seen > POCKET {
                return false;
            }
            let vi = v as usize;
            let (l, c) = (vi / cells, vi % cells);
            let (x, y) = ((c % gw) as i32, (c / gw) as i32);
            self.ensure(world, gi, l, x, y, &mut sc.near);
            let bits = self.groups[gi].moves[vi].load(Ordering::Relaxed);
            for (nd, &(dx, dy)) in DIRS.iter().enumerate() {
                match (bits >> (2 * nd)) & 3 {
                    FREE => {}
                    ONLY => {
                        if !self.owned(gi, owner_key(0, vi, nd), id) {
                            continue;
                        }
                    }
                    _ => continue,
                }
                let ui = l * cells + grid.cidx(x + dx, y + dy);
                if sc.lb_seen[ui] == stamp {
                    if sc.lb_cost[ui] < 0.0 {
                        return false;
                    }
                    continue;
                }
                sc.lb_seen[ui] = stamp;
                sc.lb_cost[ui] = 0.0;
                sc.lb_heap.push(far(l, x + dx, y + dy), ui as u32);
            }
            if has_via && self.via_ok(world, gi, id, x, y) {
                for l2 in (0..self.layers).filter(|&l2| l2 != l) {
                    let ui = l2 * cells + c;
                    if sc.lb_seen[ui] == stamp {
                        if sc.lb_cost[ui] < 0.0 {
                            return false;
                        }
                        continue;
                    }
                    sc.lb_seen[ui] = stamp;
                    sc.lb_cost[ui] = 0.0;
                    sc.lb_heap.push(far(l2, x, y), ui as u32);
                }
            }
        }
        true
    }

    /// What a step costs before anything is shared, by layer and direction.
    fn base_costs(&self) -> Vec<[f32; 8]> {
        (0..self.layers)
            .map(|l| {
                DIRS.map(|(dx, dy)| {
                    if dx != 0 && dy != 0 {
                        core::f32::consts::SQRT_2 * DIAG
                    } else if (self.along[l] == 0 && dy != 0) || (self.along[l] == 1 && dx != 0) {
                        WRONG_WAY
                    } else {
                        1.0
                    }
                })
            })
            .collect()
    }

    /// What the nodes nearest the targets of a search cost to them, at the search's prices
    /// less its bends: settled backwards from the targets, `limit` of them at most. The
    /// costs are left in the scratch, by stamp, and what is returned is the cost of the
    /// last node settled, which every node not settled costs at least.
    #[allow(clippy::too_many_arguments)]
    fn near_costs(
        &self,
        world: &World<'_>,
        sc: &mut Scratch,
        net: &WNet,
        targets: &[u32],
        pres: f32,
        via_cost: f32,
        limit: usize,
    ) -> f32 {
        sc.lb_stamp = sc.lb_stamp.wrapping_add(1);
        if sc.lb_stamp == 0 {
            sc.lb_seen.iter_mut().for_each(|v| *v = 0);
            sc.lb_stamp = 1;
        }
        let stamp = sc.lb_stamp;
        let (gi, hi, id) = (net.group, net.halo, net.net);
        let grid = self.grid;
        let cells = grid.cells();
        let dn = grid.dpoints();
        let base = self.base_costs();
        let has_via = self.groups[gi].via.is_some() && self.layers > 1;
        sc.lb_heap.clear();
        for &t in targets {
            let i = t as usize;
            if sc.lb_seen[i] != stamp {
                sc.lb_seen[i] = stamp;
                sc.lb_cost[i] = 0.0;
                sc.lb_heap.push(0.0, t);
            }
        }
        let mut settled = 0usize;
        while let Some((d, v)) = sc.lb_heap.pop() {
            let vi = v as usize;
            if d > sc.lb_cost[vi] {
                continue;
            }
            settled += 1;
            if settled >= limit {
                return d;
            }
            let (l, x, y) = self.unpack(v);
            self.ensure(world, gi, l, x, y, &mut sc.near);
            let bits = self.groups[gi].moves[vi].load(Ordering::Relaxed);
            let occ_l = &self.occ_t[hi][l * dn..(l + 1) * dn];
            let own_l = &sc.own_t[l * dn..(l + 1) * dn];
            let hist_l = &self.hist_t[l * dn..(l + 1) * dn];
            let kq = grid.didx(2 * x, 2 * y);
            // Steps from u to v along the layer: u is a step back from v.
            for (nd, &(dx, dy)) in DIRS.iter().enumerate() {
                let back = (nd + 4) % 8;
                match (bits >> (2 * back)) & 3 {
                    FREE => {}
                    ONLY => {
                        if !self.owned(gi, owner_key(0, vi, back), id) {
                            continue;
                        }
                    }
                    _ => continue,
                }
                let km = grid.didx(2 * x - dx, 2 * y - dy);
                let mut occ = count(&occ_l[km]) + count(&occ_l[kq]);
                if occ != 0 {
                    occ = occ.saturating_sub(u16::from(own_l[km]) + u16::from(own_l[kq]));
                }
                let occ = f32::from(occ);
                let c = d
                    + (base[l][nd] + price(&hist_l[km]) + price(&hist_l[kq])) * (1.0 + pres * occ);
                let ui = l * cells + grid.cidx(x - dx, y - dy);
                if sc.lb_seen[ui] == stamp && sc.lb_cost[ui] <= c {
                    continue;
                }
                sc.lb_seen[ui] = stamp;
                sc.lb_cost[ui] = c;
                sc.lb_heap.push(c, ui as u32);
            }
            if has_via && self.via_ok(world, gi, id, x, y) {
                let cell = grid.cidx(x, y);
                let occ = f32::from(
                    count(&self.occ_v[hi][cell]).saturating_sub(u16::from(sc.own_v[cell])),
                );
                let c = d + (via_cost + price(&self.hist_v[cell])) * (1.0 + pres * occ);
                for l2 in (0..self.layers).filter(|&l2| l2 != l) {
                    let ui = l2 * cells + cell;
                    if sc.lb_seen[ui] == stamp && sc.lb_cost[ui] <= c {
                        continue;
                    }
                    sc.lb_seen[ui] = stamp;
                    sc.lb_cost[ui] = c;
                    sc.lb_heap.push(c, ui as u32);
                }
            }
        }
        f32::INFINITY
    }

    /// How near a via of a group's nets may not be to another of the same net, hole to
    /// hole: in cells and squared, as the largest that is nearer than that, or less than
    /// nought where none is. The squares of whole numbers are exact, and so is what is
    /// asked of them one by one: whether so many cells are less than the distance.
    fn own_reach(&self, gi: usize) -> i64 {
        let own_min = self.groups[gi]
            .via
            .map_or(0.0, |(_, drill)| (drill + self.hole_to_hole) as f64 - 1.0);
        let g = self.grid.g as f64;
        let near = |s: i64| (s as f64).sqrt() * g < own_min;
        let (mut lo, mut hi) = (-1i64, 1i64 << 40);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if near(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Marks the cells that a via of the net's own wires at a cell leaves no other via of
    /// the net: those within `reach` of it, squared, but its own.
    fn own_via(&self, sc: &mut Scratch, reach: i64, x: i32, y: i32) {
        if reach < 1 {
            return;
        }
        let r = reach.isqrt() as i32;
        let (w, h) = (self.grid.w, self.grid.h);
        for cy in (y - r).max(0)..=(y + r).min(h - 1) {
            for cx in (x - r).max(0)..=(x + r).min(w - 1) {
                let (dx, dy) = (i64::from(cx - x), i64::from(cy - y));
                if (cx, cy) != (x, y) && dx * dx + dy * dy <= reach {
                    sc.own_near[self.grid.cidx(cx, cy)] = sc.own_stamp;
                }
            }
        }
    }

    /// The cheapest path of a net from one of `sources`, each at its cost, to one of
    /// `targets`, and how many nodes it took.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    fn search(
        &self,
        world: &World<'_>,
        sc: &mut Scratch,
        net: &WNet,
        sources: &[(u32, f32)],
        targets: &[u32],
        mode: Mode,
        via_cost: f32,
        ctl: &Ctl<'_, '_>,
        within: (i32, i32, i32, i32),
    ) -> (Result<Option<Vec<u32>>, Stop>, u64) {
        if sources.is_empty() || targets.is_empty() {
            return (Ok(None), 0);
        }
        sc.stamp = sc.stamp.wrapping_add(1);
        if sc.stamp == 0 {
            sc.slots.iter_mut().for_each(|v| v.seen = 0);
            sc.stamp = 1;
        }
        sc.target_stamp = sc.target_stamp.wrapping_add(1);
        if sc.target_stamp == 0 {
            sc.slots.iter_mut().for_each(|v| v.target = 0);
            sc.target_stamp = 1;
        }
        let stamp = sc.stamp;
        let target = sc.target_stamp;
        let ns = self.layers;
        let grid = self.grid;
        let cells = grid.cells();
        let dn = grid.dpoints();
        let gw = grid.w;
        let dw = grid.dw() as isize;
        let (gi, hi, id) = (net.group, net.halo, net.net);
        let own = id.0 + 1;
        // The box of the targets, and of those on each layer.
        let mut tb = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        let mut by_layer = vec![tb; ns];
        for &t in targets {
            sc.slots[t as usize].target = target;
            let (l, x, y) = self.unpack(t);
            tb = (tb.0.min(x), tb.1.min(y), tb.2.max(x), tb.3.max(y));
            let b = &mut by_layer[l];
            *b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
        }
        let pres = match mode {
            Mode::Share(p) => p,
            Mode::Hard => 0.0,
        };
        let hard = mode == Mode::Hard;
        // What is left to a target at least costs: on the node's own layer, the way to the
        // targets there, and through a via, that and the way to any target. With two
        // layers the way alone.
        let layered = ns > 2;
        let diag = if layered {
            core::f32::consts::SQRT_2 * DIAG
        } else {
            core::f32::consts::SQRT_2
        };
        let has_via = self.groups[gi].via.is_some() && ns > 1;
        let via_bound = if layered && has_via {
            via_cost
        } else {
            f32::INFINITY
        };
        if !layered {
            by_layer.iter_mut().for_each(|b| *b = tb);
        }
        let octile = |b: (i32, i32, i32, i32), x: i32, y: i32| -> f32 {
            let dx = (b.0 - x).max(x - b.2).max(0) as f32;
            let dy = (b.1 - y).max(y - b.3).max(0) as f32;
            let (a, c) = if dx > dy { (dx, dy) } else { (dy, dx) };
            a + (diag - 1.0) * c
        };
        // The costs settled near the targets raise it, once there are any.
        let bound = |sc: &Scratch, floor: f32, n: usize, l: usize, x: i32, y: i32| -> f32 {
            let mut h = via_bound + octile(tb, x, y);
            let own = by_layer[l];
            if own.0 <= own.2 {
                h = h.min(octile(own, x, y));
            }
            if floor > 0.0 {
                let d = if sc.lb_seen[n] == sc.lb_stamp {
                    sc.lb_cost[n].min(floor)
                } else {
                    floor
                };
                h.max(d)
            } else {
                h
            }
        };
        let mut floor = 0.0f32;
        sc.heap.clear();
        for &(n, c0) in sources {
            let (l, x, y) = self.unpack(n);
            let k = l * dn + grid.didx(2 * x, 2 * y);
            let occ = f32::from(count(&self.occ_t[hi][k]).saturating_sub(u16::from(sc.own_t[k])));
            if hard && occ > 0.0 {
                continue;
            }
            let c = c0 + price(&self.hist_t[k]) + pres * occ;
            let i = n as usize;
            let slot = &mut sc.slots[i];
            if slot.seen == stamp && slot.cost <= c {
                continue;
            }
            slot.seen = stamp;
            slot.cost = c;
            slot.parent = u32::MAX;
            slot.dir = NONE;
            let h = bound(sc, floor, i, l, x, y);
            sc.heap.push(rank(c + h, h), n);
        }
        let mid_off: [isize; 8] = DIRS.map(|(dx, dy)| dy as isize * dw + dx as isize);
        let cell_off: [isize; 8] = DIRS.map(|(dx, dy)| dy as isize * gw as isize + dx as isize);
        let base_cost = self.base_costs();
        let bend_cost = [0.0, BEND45, BEND90];
        let tolls = !self.spoke.is_empty();
        let mut pops = 0u64;
        let mut found = None;
        let mut stopped = false;
        while let Some((f, u)) = sc.heap.pop() {
            let ui = u as usize;
            let (l, x, y) = self.unpack(u);
            let Slot {
                cost: gu,
                dir: du,
                target: at_target,
                ..
            } = sc.slots[ui];
            // Left behind by a cheaper way to the node since.
            let h = bound(sc, floor, ui, l, x, y);
            if f >> 32 > rank(gu + h, h) >> 32 {
                continue;
            }
            if at_target == target {
                found = Some(u);
                break;
            }
            pops += 1;
            if pops & 1023 == 0
                && (ctl.run.is_some_and(Run::poll) || ctl.flag.load(Ordering::Relaxed))
            {
                stopped = true;
                break;
            }
            if pops > ctl.limit {
                stopped = true;
                break;
            }
            // A long search late in a negotiation: what the nodes round its targets cost
            // to them is a cost every path pays, which the distance does not see.
            if pops == 2 * NEAR as u64 && pres >= NEAR_PRES && !hard {
                floor = self.near_costs(world, sc, net, targets, pres, via_cost, NEAR);
                pops += NEAR as u64;
            }
            self.ensure(world, gi, l, x, y, &mut sc.near);
            let bits = self.groups[gi].moves[ui].load(Ordering::Relaxed);
            let occ_l = &self.occ_t[hi][l * dn..(l + 1) * dn];
            let hist_l = &self.hist_t[l * dn..(l + 1) * dn];
            let k0 = grid.didx(2 * x, 2 * y) as isize;
            for nd in 0..8usize {
                let t = if du == NONE { 0 } else { turn(du, nd as u8) };
                if t >= 3 {
                    continue;
                }
                match (bits >> (2 * nd)) & 3 {
                    FREE => {}
                    ONLY => {
                        if !self.owned(gi, owner_key(0, ui, nd), id) {
                            continue;
                        }
                    }
                    _ => continue,
                }
                let (dx, dy) = DIRS[nd];
                let (nx, ny) = (x + dx, y + dy);
                if nx < within.0 || ny < within.1 || nx > within.2 || ny > within.3 {
                    continue;
                }
                let km = (k0 + mid_off[nd]) as usize;
                let kq = (k0 + 2 * mid_off[nd]) as usize;
                let mut occ = count(&occ_l[km]) + count(&occ_l[kq]);
                if occ != 0 {
                    occ = occ.saturating_sub(
                        u16::from(sc.own_t[l * dn + km]) + u16::from(sc.own_t[l * dn + kq]),
                    );
                }
                let occ = f32::from(occ);
                if hard && occ > 0.0 {
                    continue;
                }
                let vi = (ui as isize + cell_off[nd]) as usize;
                let mut base = base_cost[l][nd];
                if tolls
                    && let Some(&p) = self.spoke.get(vi)
                    && p != 0
                    && p & !HELD != own
                {
                    base *= 1.0
                        + self.spoke_cost
                            * if p & HELD != 0 {
                                HELD_TIMES as f32
                            } else {
                                1.0
                            };
                }
                let hist = price(&hist_l[km]) + price(&hist_l[kq]);
                let c = gu + (base + hist) * (1.0 + pres * occ) + bend_cost[t as usize];
                let slot = &mut sc.slots[vi];
                if slot.seen == stamp && slot.cost <= c {
                    continue;
                }
                slot.seen = stamp;
                slot.cost = c;
                slot.parent = u;
                slot.dir = nd as u8;
                let h = bound(sc, floor, vi, l, nx, ny);
                sc.heap.push(rank(c + h, h), vi as u32);
            }
            if has_via {
                let cell = grid.cidx(x, y);
                let occ = f32::from(
                    count(&self.occ_v[hi][cell]).saturating_sub(u16::from(sc.own_v[cell])),
                );
                if hard && occ > 0.0 {
                    continue;
                }
                if !self.via_ok(world, gi, id, x, y) {
                    continue;
                }
                // Hole to hole from the vias of the net's own wires.
                if sc.own_near[cell] == sc.own_stamp {
                    continue;
                }
                let c0 = gu + (via_cost + price(&self.hist_v[cell])) * (1.0 + pres * occ);
                for l2 in 0..ns {
                    if l2 == l {
                        continue;
                    }
                    let v = (l2 * cells + cell) as u32;
                    let vi = v as usize;
                    let slot = sc.slots[vi];
                    if slot.seen == stamp && slot.cost <= c0 {
                        continue;
                    }
                    if !self.point_ok_at(world, gi, id, v, (l2, x, y), &mut sc.near) {
                        continue;
                    }
                    let slot = &mut sc.slots[vi];
                    slot.seen = stamp;
                    slot.cost = c0;
                    slot.parent = u;
                    slot.dir = NONE;
                    let h = bound(sc, floor, vi, l2, x, y);
                    sc.heap.push(rank(c0 + h, h), v);
                }
            }
        }
        if stopped {
            return (Err(Stop), pops);
        }
        let path = found.map(|u| {
            let mut path = vec![u];
            let mut c = u;
            while sc.slots[c as usize].parent != u32::MAX {
                c = sc.slots[c as usize].parent;
                path.push(c);
            }
            path.reverse();
            path
        });
        (Ok(path), pops)
    }

    /// The sources or the targets that a group of terminals gives a search: its tree,
    /// and where routes may end at its pads, those of a plane near `other` only.
    fn ends(
        &self,
        net: &WNet,
        members: &[usize],
        tree: &[u32],
        other: Rect,
        mode: Mode,
    ) -> Vec<(u32, f32)> {
        let mut out: Vec<(u32, f32)> = tree.iter().map(|&n| (n, 0.0)).collect();
        let dn = self.grid.dpoints();
        let reach = PLANE_REACH.max(PLANE_REACH_CELLS * self.grid.g);
        let near = other.expand(reach);
        for &t in members {
            let acc = &net.terms[t];
            let before = out.len();
            for a in acc {
                if acc.len() > MANY {
                    let p = self.node_point(a.node);
                    if !near.intersects(&Rect::new(p, p)) {
                        continue;
                    }
                }
                if mode == Mode::Hard {
                    let (l, x, y) = self.unpack(a.node);
                    if count(&self.occ_t[net.halo][l * dn + self.grid.didx(2 * x, 2 * y)]) > 0 {
                        continue;
                    }
                }
                out.push((a.node, a.cost));
            }
            if acc.len() > MANY && out.len() == before && mode != Mode::Hard {
                out.extend(acc.iter().map(|a| (a.node, a.cost)));
            }
        }
        out
    }

    /// Routes a net: each of its connections from what the ones before joined. Returns
    /// the nodes that took, too.
    #[allow(clippy::too_many_arguments)]
    fn route_net(
        &self,
        world: &World<'_>,
        sc: &mut Scratch,
        net: &mut WNet,
        mode: Mode,
        via_cost: f32,
        ctl: &Ctl<'_, '_>,
        keep: Option<&[bool]>,
        margin: Option<(i32, i32)>,
    ) -> (Result<(), Stop>, u64) {
        let old = core::mem::take(&mut net.wires);
        sc.own_stamp = sc.own_stamp.wrapping_add(1);
        if sc.own_stamp == 0 {
            sc.own_near.iter_mut().for_each(|v| *v = 0);
            sc.own_stamp = 1;
        }
        let reach = self.own_reach(net.group);
        let n = net.terms.len();
        let mut parent: Vec<usize> = (0..n).collect();
        let mut members: Vec<Vec<usize>> = (0..n).map(|t| vec![t]).collect();
        let mut tree: Vec<Vec<u32>> = vec![Vec::new(); n];
        let mut boxes = net.boxes.clone();
        let cells = self.grid.cells() as u32;
        let mut expanded = 0u64;
        // The wires kept are what the net has before any search: the terminals that they
        // join are joined, and their nodes are where the others may begin and end.
        if let Some(keep) = keep.filter(|k| k.len() == old.len()) {
            for (terms, w) in kept_wires(n, old, keep) {
                let r = find(&mut parent, terms[0]);
                for &t in &terms[1..] {
                    let (ra, rb) = (find(&mut parent, r), find(&mut parent, t));
                    if ra != rb {
                        let (mb, tb) = (
                            core::mem::take(&mut members[rb]),
                            core::mem::take(&mut tree[rb]),
                        );
                        members[ra].extend(mb);
                        tree[ra].extend(tb);
                        boxes[ra] = boxes[ra].union(&boxes[rb]);
                        parent[rb] = ra;
                    }
                }
                let r = find(&mut parent, r);
                tree[r].extend(w.nodes.iter().copied());
                for q in w.nodes.windows(2) {
                    if q[0] / cells != q[1] / cells {
                        let (_, x, y) = self.unpack(q[0]);
                        self.own_via(sc, reach, x, y);
                    }
                }
                net.wires.push(w);
            }
        }
        for ci in 0..net.conns.len() {
            if net.dead[ci] {
                continue;
            }
            let (a, b) = net.conns[ci];
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra == rb {
                continue;
            }
            let sources = self.ends(net, &members[ra], &tree[ra], boxes[rb], mode);
            let targets: Vec<u32> = self
                .ends(net, &members[rb], &tree[rb], boxes[ra], mode)
                .into_iter()
                .map(|(n, _)| n)
                .collect();
            // A connection is looked at once for whether one end is walled in.
            if !net.looked[ci] {
                net.looked[ci] = true;
                if self.pocket(world, sc, net, &sources, &targets) {
                    net.dead[ci] = true;
                    continue;
                }
            }
            let left = Ctl {
                limit: ctl.limit.saturating_sub(expanded),
                ..*ctl
            };
            // Within a box round the two ends, if there is a way in it: a search that has
            // to share goes on until it has looked at every place that costs less than
            // sharing does, which late in a negotiation is the whole board.
            let all = (0, 0, self.grid.w - 1, self.grid.h - 1);
            let within = margin.map_or(all, |(least, share)| {
                let mut b = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
                for n in sources.iter().map(|s| s.0).chain(targets.iter().copied()) {
                    let (_, x, y) = self.unpack(n);
                    b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
                }
                let m = least.max((b.2 - b.0).max(b.3 - b.1) * share / 100);
                (
                    (b.0 - m).max(0),
                    (b.1 - m).max(0),
                    (b.2 + m).min(all.2),
                    (b.3 + m).min(all.3),
                )
            });
            let (mut found, pops) = self.search(
                world, sc, net, &sources, &targets, mode, via_cost, &left, within,
            );
            expanded += pops;
            if matches!(found, Ok(None)) && within != all && !self.near_only {
                let left = Ctl {
                    limit: ctl.limit.saturating_sub(expanded),
                    ..*ctl
                };
                let (again, pops) = self.search(
                    world, sc, net, &sources, &targets, mode, via_cost, &left, all,
                );
                expanded += pops;
                found = again;
            }
            let path = match found {
                Err(stop) => return (Err(stop), expanded),
                Ok(None) => {
                    if matches!(mode, Mode::Share(_)) {
                        net.dead[ci] = true;
                    }
                    continue;
                }
                Ok(Some(path)) => path,
            };
            // Where a path ends: at a pad whose centre the cell is on, or else on the
            // net's own wires, or else at a pad by the way from the cell to it.
            let stub =
                |members: &[usize], tree: &[u32], node: u32| -> (Option<usize>, Option<Stub>) {
                    let at = |with: bool| {
                        members
                            .iter()
                            .flat_map(|&t| net.terms[t].iter().map(move |acc| (t, acc)))
                            .filter(|(_, acc)| acc.node == node && acc.stub.is_some() == with)
                            .min_by(|x, y| x.1.cost.total_cmp(&y.1.cost))
                    };
                    if let Some((t, _)) = at(false) {
                        (Some(t), None)
                    } else if tree.contains(&node) {
                        (None, None)
                    } else {
                        at(true).map_or((None, None), |(t, acc)| (Some(t), acc.stub))
                    }
                };
            let last = *path.last().unwrap_or(&path[0]);
            let (from, start) = stub(&members[ra], &tree[ra], path[0]);
            let (to, end) = stub(&members[rb], &tree[rb], last);
            for w in path.windows(2) {
                if w[0] / cells != w[1] / cells {
                    let (_, x, y) = self.unpack(w[0]);
                    self.own_via(sc, reach, x, y);
                }
            }
            let (mb, tb) = (
                core::mem::take(&mut members[rb]),
                core::mem::take(&mut tree[rb]),
            );
            members[ra].extend(mb);
            tree[ra].extend(tb);
            tree[ra].extend(path.iter().copied());
            boxes[ra] = boxes[ra].union(&boxes[rb]);
            parent[rb] = ra;
            net.wires.push(Wire {
                nodes: path,
                start,
                end,
                from,
                to,
            });
        }
        (Ok(()), expanded)
    }

    /// The class of halo, the map and the place in it that a key is of.
    fn unkey(&self, k: u64) -> (usize, usize, usize) {
        let (m, idx) = ((k >> 32) as usize, (k & 0xFFFF_FFFF) as usize);
        (m / (self.layers + 1), m % (self.layers + 1), idx)
    }

    /// Of a net's wires, whether each is in no other net's way.
    fn clear_wires(&self, net: &WNet) -> Vec<bool> {
        let mut c = Vec::new();
        net.wires
            .iter()
            .map(|w| {
                c.clear();
                self.wire_conflicts(net, w, &mut c);
                c.is_empty()
            })
            .collect()
    }

    /// Routes the net of a task with its claims still in place, as
    /// [`route_net`](Board::route_net) routes one that has given them up: its searches
    /// do not count what it claims itself. Returns it as it then is, as it was if it was
    /// stopped, with the points where its wires are in what another net claims.
    fn route_task(
        &self,
        world: &World<'_>,
        sc: &mut Scratch,
        task: Task,
        ctl: &Ctl<'_, '_>,
    ) -> Ahead {
        let mut net = task.net;
        let old = net.wires.clone();
        let own = |sc: &mut Scratch, net: &WNet, to: u8| {
            for &k in &net.keys {
                let (hi, map, idx) = self.unkey(k);
                if hi == net.halo {
                    if map == self.layers {
                        sc.own_v[idx] = to;
                    } else {
                        sc.own_t[idx] = to;
                    }
                }
            }
        };
        own(sc, &net, 1);
        let via = net.via;
        let (res, nodes) = self.route_net(
            world,
            sc,
            &mut net,
            task.mode,
            via,
            ctl,
            task.keep.as_deref(),
            task.margin,
        );
        if res.is_err() {
            net.wires = old;
        }
        // What its wires are in, by the claims as they are without its own.
        let mut conflicts = Vec::new();
        {
            let cells = self.grid.cells();
            let dn = self.grid.dpoints();
            let occ = &self.occ_t[net.halo];
            let others_t = |k: usize| count(&occ[k]) > u16::from(sc.own_t[k]);
            for w in &net.wires {
                for (k, &n) in w.nodes.iter().enumerate() {
                    let (l, x, y) = self.unpack(n);
                    if k == 0 {
                        let idx = self.grid.didx(2 * x, 2 * y);
                        if others_t(l * dn + idx) {
                            conflicts.push((Some(l as u8), idx as u32));
                        }
                        continue;
                    }
                    let p = w.nodes[k - 1];
                    if p as usize / cells != l {
                        let idx = self.grid.cidx(x, y);
                        if count(&self.occ_v[net.halo][idx]) > u16::from(sc.own_v[idx]) {
                            conflicts.push((None, idx as u32));
                        }
                        continue;
                    }
                    let (_, px, py) = self.unpack(p);
                    for (i, j) in [(px + x, py + y), (2 * x, 2 * y)] {
                        let idx = self.grid.didx(i, j);
                        if others_t(l * dn + idx) {
                            conflicts.push((Some(l as u8), idx as u32));
                        }
                    }
                }
            }
            conflicts.sort_unstable();
            conflicts.dedup();
        }
        own(sc, &net, 0);
        Ahead {
            net,
            conflicts,
            nodes,
            stopped: res.is_err(),
        }
    }

    /// How many vias a net's wires have, and how long they are, in nanometres.
    fn measure(&self, net: &WNet) -> (usize, f64) {
        let (mut vias, mut len) = (0, 0.0);
        for w in &net.wires {
            let (segs, v) = self.geometry(w);
            vias += v.len();
            for (_, a, b) in segs {
                let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
                len += (dx * dx + dy * dy).sqrt();
            }
        }
        (vias, len)
    }

    /// Where a route of a net may end at a pad on a layer: the cells beside its centre
    /// that a straight track joins to it, nearest first, and for a pad with none, as one
    /// in a row of fine pitch that is off the grid's lines, ways out along its axis that
    /// turn onto a line once they are clear of its neighbours.
    #[allow(clippy::too_many_arguments)]
    fn access(
        &self,
        world: &World<'_>,
        net: &WNet,
        center: Point,
        bbox: Rect,
        l: LayerId,
        width: i64,
        near: &mut Vec<EntryId>,
    ) -> Vec<Access> {
        let grid = self.grid;
        let li = usize::from(l.0);
        let free = |a: Point, b: Point| {
            a == b
                || Shape::track(a, b, width)
                    .is_ok_and(|s| world.track(net.net, l, &s, None) == Verdict::Free)
        };
        let far = |a: Point, b: Point| ((a.x - b.x) as f64).hypot((a.y - b.y) as f64);
        let (cx, cy) = (
            ((center.x - grid.ox) as f64 / grid.g as f64).round() as i64,
            ((center.y - grid.oy) as f64 / grid.g as f64).round() as i64,
        );
        let inside =
            |x: i64, y: i64| x >= 0 && y >= 0 && x < i64::from(grid.w) && y < i64::from(grid.h);
        let mut found: Vec<(i64, u32)> = Vec::new();
        for dy in -ACCESS_REACH..=ACCESS_REACH {
            for dx in -ACCESS_REACH..=ACCESS_REACH {
                let (x, y) = (cx + i64::from(dx), cy + i64::from(dy));
                if !inside(x, y) {
                    continue;
                }
                let p = grid.cell(x as i32, y as i32);
                let d2 = (p.x - center.x).pow(2) + (p.y - center.y).pow(2);
                found.push((d2, self.node(li, x as i32, y as i32)));
            }
        }
        found.sort_unstable();
        let mut out = Vec::new();
        for (_, node) in found {
            if out.len() >= ACCESS {
                break;
            }
            let p = self.node_point(node);
            if self.point_ok(world, net.group, net.net, node, near) && free(p, center) {
                out.push(Access {
                    node,
                    stub: (p != center).then_some((center, None)),
                    cost: (far(p, center) / grid.g as f64) as f32,
                });
            }
        }
        if !out.is_empty() {
            return out;
        }
        // Ways out: along the pad's long axis, either way, to a cell of one of the two
        // lines beside the axis, by a turn of 45 degrees that ends on the cell.
        let (bw, bh) = (bbox.max().x - bbox.min().x, bbox.max().y - bbox.min().y);
        let axes: &[bool] = if bh > bw {
            &[true]
        } else if bw > bh {
            &[false]
        } else {
            &[true, false]
        };
        for &up in axes {
            // Along the axis and across it, the centre and the grid's origin.
            let (ca, cp, oa, op) = if up {
                (center.y, center.x, grid.oy, grid.ox)
            } else {
                (center.x, center.y, grid.ox, grid.oy)
            };
            let below = (cp - op).div_euclid(grid.g);
            for line in [below, below + 1] {
                let lp = op + line * grid.g;
                let side = (lp - cp).abs();
                for s in [1i64, -1] {
                    // The first row along the axis that the turn fits before.
                    let first = if s > 0 {
                        (ca + side - oa + grid.g - 1).div_euclid(grid.g)
                    } else {
                        (ca - side - oa).div_euclid(grid.g)
                    };
                    let mut kept = 0;
                    for k in 0..i64::from(WAY_OUT) {
                        let row = first + s * k;
                        let (x, y) = if up { (line, row) } else { (row, line) };
                        if !inside(x, y) {
                            break;
                        }
                        let node = self.node(li, x as i32, y as i32);
                        let cell = self.node_point(node);
                        let along = oa + row * grid.g - s * side;
                        let bend = if up {
                            Point::new(center.x, along)
                        } else {
                            Point::new(along, center.y)
                        };
                        if !self.point_ok(world, net.group, net.net, node, near)
                            || !free(center, bend)
                            || !free(bend, cell)
                        {
                            continue;
                        }
                        out.push(Access {
                            node,
                            stub: Some((center, (bend != center && bend != cell).then_some(bend))),
                            cost: ((far(center, bend) + far(bend, cell)) / grid.g as f64) as f32,
                        });
                        kept += 1;
                        if kept >= WAYS_OUT {
                            break;
                        }
                    }
                }
            }
        }
        out
    }
}

fn find(p: &mut [usize], mut i: usize) -> usize {
    while p[i] != i {
        p[i] = p[p[i]];
        i = p[i];
    }
    i
}

/// Whether every point of `now` is one of `then`, both in order.
fn none_new(now: &[Conflict], then: &[Conflict]) -> bool {
    let mut k = 0;
    now.iter().all(|c| {
        while k < then.len() && then[k] < *c {
            k += 1;
        }
        k < then.len() && then[k] == *c
    })
}

/// A net to route.
struct Task {
    /// Its turn in the round.
    at: usize,
    net: WNet,
    /// Its wires to route it from, if it keeps any.
    keep: Option<Vec<bool>>,
    mode: Mode,
    /// How far round the ends of a connection its searches look, if not everywhere.
    margin: Option<(i32, i32)>,
    /// The nodes that its searches may expand.
    limit: u64,
}

/// A net as it was routed.
struct Ahead {
    net: WNet,
    /// The points where its wires were in what another net claimed then.
    conflicts: Vec<Conflict>,
    nodes: u64,
    stopped: bool,
}

/// Of a net's wires, those marked to be kept that still hold together: a wire that began
/// or ended on a wire that is not kept goes with it, and so do wires that reach no
/// terminal. With each, the terminals that it and the wires it is joined to reach.
fn kept_wires(terminals: usize, old: Vec<Wire>, keep: &[bool]) -> Vec<(Vec<usize>, Wire)> {
    let mut alive: Vec<bool> = keep.to_vec();
    // By end of a wire with no terminal, the wire it is on.
    let mut on: Vec<[Option<usize>; 2]> = vec![[None; 2]; old.len()];
    loop {
        let mut nodes: Vec<(u32, usize)> = Vec::new();
        for (i, w) in old.iter().enumerate() {
            if alive[i] {
                nodes.extend(w.nodes.iter().map(|&n| (n, i)));
            }
        }
        nodes.sort_unstable();
        let mut changed = false;
        for (i, w) in old.iter().enumerate() {
            if !alive[i] {
                continue;
            }
            let ends = [(w.from, w.nodes.first()), (w.to, w.nodes.last())];
            for (e, (terminal, node)) in ends.into_iter().enumerate() {
                on[i][e] = None;
                if terminal.is_some() {
                    continue;
                }
                let Some(&node) = node else {
                    alive[i] = false;
                    changed = true;
                    break;
                };
                let k = nodes.partition_point(|&(n, _)| n < node);
                let other = nodes[k..]
                    .iter()
                    .take_while(|&&(n, _)| n == node)
                    .find(|&&(_, j)| j != i);
                match other {
                    Some(&(_, j)) => on[i][e] = Some(j),
                    None => {
                        alive[i] = false;
                        changed = true;
                        break;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    // Wires and terminals in pieces: terminals first, then wires.
    let mut parent: Vec<usize> = (0..terminals + old.len()).collect();
    for (i, w) in old.iter().enumerate() {
        if !alive[i] {
            continue;
        }
        for (e, terminal) in [w.from, w.to].into_iter().enumerate() {
            let other = match (terminal, on[i][e]) {
                (Some(t), _) => t,
                (None, Some(j)) => terminals + j,
                (None, None) => continue,
            };
            let (a, b) = (find(&mut parent, terminals + i), find(&mut parent, other));
            if a != b {
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut by_piece: Vec<Vec<usize>> = vec![Vec::new(); terminals + old.len()];
    for t in 0..terminals {
        let r = find(&mut parent, t);
        by_piece[r].push(t);
    }
    let mut out = Vec::new();
    for (i, w) in old.into_iter().enumerate() {
        if !alive[i] {
            continue;
        }
        let r = find(&mut parent, terminals + i);
        if !by_piece[r].is_empty() {
            out.push((by_piece[r].clone(), w));
        }
    }
    out
}

/// Why there is no negotiation for every net.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoWhole {
    /// Its first round said that the share has no room for it on this grid.
    NoRoom,
    /// The board is none for it, or the run ended.
    Not,
}

/// What a whole-board negotiation ends with.
pub(crate) struct Whole {
    /// By job, the route found for its net, if any.
    pub(crate) trees: Vec<Option<Tree>>,
    /// The pitch of the grid.
    pub(crate) pitch: i64,
    /// How many rounds there were, how many points were shared after the best of them, and
    /// how many nets were taken out for it.
    pub(crate) rounds: usize,
    pub(crate) shared: usize,
    pub(crate) taken_out: usize,
    /// How many vias routing the nets again for fewer of them saved.
    pub(crate) vias_saved: usize,
    /// By job, the places in its fill that a net with pads cut off it was given, for its
    /// routes to end at, and how many pieces of fill were told to be cut off in all.
    pub(crate) sites: Vec<(usize, Vec<TermPad>)>,
    pub(crate) pour_cuts: usize,
    /// How many times the host gave its own fill for that.
    pub(crate) refills: usize,
    /// How many times the routes went back to what they were before nets were planned
    /// again for their pours, for having more connections open after it.
    pub(crate) pours_back: usize,
    /// The rounds on the grid of the pitch for the connections that a negotiation at twice
    /// the pitch left open: how many there were, how many connections were open on the
    /// grid before them, and how many after.
    pub(crate) finer: (usize, usize, usize),
    /// Whether the rounds ended on their own, and not with their share.
    pub(crate) ended: bool,
    /// Whether they ended at the most rounds that there are, with points still shared and
    /// fewer of them than twelve rounds before.
    pub(crate) capped: bool,
}

/// The pads of fine pitch: those of routed nets with no hole and another net's pad so near
/// that no track of theirs passes between. For each, its centre, how far the centre of
/// the nearest such pad is, and whether the two are side by side along x.
fn fine_pads(design: &Design) -> Vec<(Point, i64, bool)> {
    let rules = &design.rules;
    let stack = rules.layers();
    let pads = &design.board.pads;
    let copper: Vec<usize> = (0..pads.len())
        .filter(|&i| pads[i].layers.0 & stack.0 != 0)
        .collect();
    let index = SpatialIndex::bulk_load(copper.iter().map(|&i| pads[i].shape.bbox()));
    let mut near = Vec::new();
    let mut out = Vec::new();
    for &i in &copper {
        let pad = &pads[i];
        let Some(net) = pad.net.filter(|n| n.index() < design.nets.len()) else {
            continue;
        };
        let spec = &design.nets[net.index()];
        if !spec.route || pad.hole.is_some() {
            continue;
        }
        let Some(center) = pad_center(&pad.shape) else {
            continue;
        };
        let layer = LayerSet(pad.layers.0 & stack.0)
            .iter()
            .next()
            .unwrap_or(LayerId(0));
        let passage = spec.track_width.max(rules.track_width(Some(net), layer))
            + 2 * rules.clearance(Some(net), layer);
        let b = pad.shape.bbox();
        index.query(&b.expand(passage), &mut near);
        let mut best: Option<(i64, i64, bool)> = None;
        for id in &near {
            let q = &pads[copper[id.slot() as usize]];
            if q.net == pad.net || q.layers.0 & pad.layers.0 & stack.0 == 0 {
                continue;
            }
            let o = q.shape.bbox();
            let dx = (o.min().x - b.max().x).max(b.min().x - o.max().x);
            let dy = (o.min().y - b.max().y).max(b.min().y - o.max().y);
            if dx.max(dy).max(0) >= passage {
                continue;
            }
            let Some(c) = pad_center(&q.shape) else {
                continue;
            };
            let (cx, cy) = ((c.x - center.x).abs(), (c.y - center.y).abs());
            let d = ((cx as f64).hypot(cy as f64)).round() as i64;
            if best.is_none_or(|(e, _, _)| d < e) {
                best = Some((d, cx, cx >= cy));
            }
        }
        if let Some((d, _, along_x)) = best {
            out.push((center, d, along_x));
        }
    }
    out
}

/// The grid: its pitch, taken to a divisor of the pitch that the most pads of fine pitch
/// have if that is within 6 % above it, and its origin, which puts the most of those pads on
/// its lines, and then the most pads on its cells.
fn make_grid(design: &Design, jobs: &[Job], pitch: i64, bounds: Rect) -> Grid {
    let fine = fine_pads(design);
    let mut count: std::collections::BTreeMap<i64, usize> = std::collections::BTreeMap::new();
    for &(_, d, _) in &fine {
        *count.entry((d + 500) / 1000).or_default() += 1;
    }
    let snap = |um: i64| -> Option<i64> {
        let p = um * 1000;
        let m = p / (2 * pitch);
        (m >= 1 && p / (2 * m) <= pitch + pitch * 6 / 100).then(|| p / (2 * m))
    };
    let g = count
        .iter()
        .filter(|&(&um, &n)| n >= 8 && snap(um).is_some())
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .and_then(|(&um, _)| snap(um))
        .unwrap_or(pitch)
        .max(1000);
    let (minx, miny) = (bounds.min().x, bounds.min().y);
    // By offset, the pads of fine rows on its lines and all pads.
    let mut cx: std::collections::BTreeMap<i64, (usize, usize)> = std::collections::BTreeMap::new();
    let mut cy = cx.clone();
    let key = |v: i64, min: i64| (v - min).rem_euclid(g);
    for job in jobs {
        for p in &job.ctx.pads {
            cx.entry(key(p.center.x, minx)).or_default().1 += 1;
            cy.entry(key(p.center.y, miny)).or_default().1 += 1;
        }
    }
    for &(at, d, along_x) in &fine {
        // Rows whose pitch is a multiple of the grid's, so that one offset serves them.
        if ((d + g / 2) / g * g - d).abs() > 1000 {
            continue;
        }
        if along_x {
            cx.entry(key(at.x, minx)).or_default().0 += 1;
        } else {
            cy.entry(key(at.y, miny)).or_default().0 += 1;
        }
    }
    let pick = |m: &std::collections::BTreeMap<i64, (usize, usize)>| -> i64 {
        m.iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map_or(0, |(&k, _)| k)
    };
    let (ox, oy) = (minx + pick(&cx), miny + pick(&cy));
    let w = ((bounds.max().x - ox).max(0) / g + 1).min(i64::from(i32::MAX / 4)) as i32;
    let h = ((bounds.max().y - oy).max(0) / g + 1).min(i64::from(i32::MAX / 4)) as i32;
    Grid { ox, oy, g, w, h }
}

/// The terminals of a net to join, as a spanning tree grown from the first, the nearest
/// next: by the distance between the centres of their nearest pads.
fn spanning(ctx: &NetCtx) -> (Vec<(usize, usize)>, i64) {
    let n = ctx.terminals;
    let mut by_term: Vec<Vec<Point>> = vec![Vec::new(); n];
    for p in &ctx.pads {
        by_term[p.terminal].push(p.center);
    }
    let far = |a: usize, b: usize| -> i64 {
        let mut best = i64::MAX;
        for p in &by_term[a] {
            for q in &by_term[b] {
                best = best.min((p.x - q.x).abs() + (p.y - q.y).abs());
            }
        }
        best
    };
    let mut done = vec![false; n];
    let mut near: Vec<(i64, usize)> = vec![(i64::MAX, 0); n];
    let mut conns = Vec::new();
    let mut length = 0i64;
    let mut last = 0;
    done[0] = true;
    for _ in 1..n {
        for t in 0..n {
            if !done[t] {
                let d = far(last, t);
                if d < near[t].0 {
                    near[t] = (d, last);
                }
            }
        }
        let Some(t) = (0..n).filter(|&t| !done[t]).min_by_key(|&t| (near[t].0, t)) else {
            break;
        };
        done[t] = true;
        if near[t].0 < i64::MAX {
            conns.push((near[t].1, t));
            length = length.saturating_add(near[t].0);
        }
        last = t;
    }
    (conns, length)
}

/// Gives a net the terminals of `ctx`, the net planned again with the pads that are cut
/// off its fill: where its routes may end, its connections, and the terminals that its
/// wires end at now.
fn plan_again(board: &Board, world: &World<'_>, net: &mut WNet, ctx: &NetCtx, sc: &mut Scratch) {
    let layers = board.layers;
    net.terms = vec![Vec::new(); ctx.terminals];
    net.boxes = vec![Rect::default(); ctx.terminals];
    let mut any = vec![false; ctx.terminals];
    for pad in &ctx.pads {
        let r = Rect::new(pad.center, pad.center);
        net.boxes[pad.terminal] = if any[pad.terminal] {
            net.boxes[pad.terminal].union(&r)
        } else {
            r
        };
        any[pad.terminal] = true;
        for l in pad.layers.iter() {
            let li = usize::from(l.0);
            if li >= layers {
                continue;
            }
            let found = board.access(
                world,
                net,
                pad.center,
                pad.bbox,
                l,
                ctx.widths[li],
                &mut sc.near,
            );
            net.terms[pad.terminal].extend(found);
        }
    }
    let (conns, length) = spanning(ctx);
    net.dead = vec![false; conns.len()];
    net.looked = vec![false; conns.len()];
    net.conns = conns;
    net.length = length;
    // Its wires end at the terminals that their pads are of now.
    let cells = board.grid.cells();
    let at = |p: Point, l: usize| {
        ctx.pads
            .iter()
            .find(|q| q.center == p && q.layers.contains(LayerId(l as u8)))
            .map(|q| q.terminal)
    };
    for w in &mut net.wires {
        if let (Some(_), Some(&n)) = (w.from, w.nodes.first()) {
            let p = w.start.map_or_else(|| board.node_point(n), |s| s.0);
            w.from = at(p, n as usize / cells);
        }
        if let (Some(_), Some(&n)) = (w.to, w.nodes.last()) {
            let p = w.end.map_or_else(|| board.node_point(n), |s| s.0);
            w.to = at(p, n as usize / cells);
        }
    }
}

/// The nodes that a search of a net may expand that routes it where nothing is claimed,
/// by those that it took the last time it was routed, and those that the run has left.
fn hard_nodes(run: &Run<'_>, took: u64) -> (u64, u64) {
    let most = HARD_NODES.max(HARD_TIMES.saturating_mul(took));
    let left = run.limits(u64::MAX).max_expansions;
    (left.min(most), left)
}

/// Makes the routes legal on the grid: what is still shared is taken out, the net with
/// the most first, and routed again where nothing is claimed, the shortest net first. A
/// net with no way there keeps what it had that is in no other's way. Returns how many
/// nets were taken out.
fn legalise(
    board: &Board,
    world: &World<'_>,
    nets: &mut [WNet],
    sc: &mut Scratch,
    run: &Run<'_>,
    taken: &[u64],
) -> usize {
    let mut out: Vec<usize> = Vec::new();
    loop {
        let conf: Vec<usize> = nets.iter().map(|n| board.conflicts(n).len()).collect();
        let Some(worst) = (0..nets.len())
            .filter(|&i| conf[i] > 0)
            .max_by_key(|&i| (conf[i], nets[i].length, core::cmp::Reverse(i)))
        else {
            break;
        };
        // Its wires that are in another's way go, and those that hung on them.
        let keep: Vec<bool> = nets[worst]
            .wires
            .iter()
            .map(|w| {
                let mut c = Vec::new();
                board.wire_conflicts(&nets[worst], w, &mut c);
                c.is_empty()
            })
            .collect();
        board.uncommit(&mut nets[worst]);
        let old = core::mem::take(&mut nets[worst].wires);
        nets[worst].wires = kept_wires(nets[worst].terms.len(), old, &keep)
            .into_iter()
            .map(|(_, w)| w)
            .collect();
        board.commit(&mut nets[worst]);
        if !out.contains(&worst) {
            out.push(worst);
        }
    }
    let taken_out = out.len();
    out.sort_by_key(|&i| (nets[i].length, i));
    for i in out {
        if run.ending() {
            break;
        }
        let before = nets[i].wires.clone();
        let all = vec![true; before.len()];
        board.uncommit(&mut nets[i]);
        let (limit, left) = hard_nodes(run, taken[i]);
        let ctl = Ctl {
            run: Some(run),
            flag: run.flag(),
            limit,
        };
        let via = nets[i].via;
        let (res, pops) = board.route_net(
            world,
            sc,
            &mut nets[i],
            Mode::Hard,
            via,
            &ctl,
            Some(&all),
            None,
        );
        if res.is_err() {
            nets[i].wires = before;
        }
        board.commit(&mut nets[i]);
        run.charge(pops);
        // Stopped by its own number of nodes, the net is left to the passes.
        if run.poll() || (res.is_err() && pops >= left) {
            break;
        }
    }
    taken_out
}

/// How many connections the nets have open with their routes as they are and the pours
/// filled around them, and by job with a fill, the net planned again if pads are cut off
/// it: what the host's fill `real` joins is joined, and a route that ends on a piece of
/// fill that is cut off joins nothing but that piece.
#[allow(clippy::too_many_arguments)]
pub(crate) fn open_on_fills(
    world: &World<'_>,
    design: &Design,
    jobs: &[Job],
    pours: &Pours,
    copper: &[Option<NetCopper>],
    real: Option<&super::pours::Refilled>,
    sites: &[(usize, Vec<TermPad>)],
    pitch: i64,
) -> (Vec<usize>, Vec<(usize, super::pours::Rejoin)>) {
    let planned: Vec<usize> = pours.jobs().collect();
    let empty = NetCopper::default();
    let mut open = Vec::with_capacity(jobs.len());
    let mut plans = Vec::new();
    for (k, job) in jobs.iter().enumerate() {
        let c = copper.get(k).and_then(Option::as_ref).unwrap_or(&empty);
        let given = sites
            .iter()
            .find(|(j, _)| *j == k)
            .map_or(&[][..], |(_, s)| s.as_slice());
        let again = planned
            .contains(&k)
            .then(|| pours.rejoin(world, design, jobs, copper, k, pitch, real, given))
            .flatten();
        let with_given;
        let ctx = match &again {
            Some(a) => &a.ctx,
            None if given.is_empty() => &job.ctx,
            None => {
                with_given = super::pours::with_sites(&job.ctx, given, pitch);
                &with_given
            }
        };
        let mut parts = c.pieces(ctx).terminals;
        parts.sort_unstable();
        parts.dedup();
        open.push(parts.len().saturating_sub(1));
        if let Some(a) = again {
            plans.push((k, a));
        }
    }
    (open, plans)
}

/// Fewer vias: each net with any again, with vias dear, kept if it has fewer and is
/// little longer. After a negotiation that its share ended, the nets with the most
/// first: the time may end before all of them have had their turn, and those are
/// the ones with the most to lose.
fn fewer_vias(
    board: &Board,
    world: &World<'_>,
    nets: &mut [WNet],
    sc: &mut Scratch,
    run: &Run<'_>,
    taken: &[u64],
    cut: bool,
) -> usize {
    let grid = board.grid;
    let mut vias_saved = 0;
    let mut by_vias: Vec<(usize, usize)> = nets
        .iter()
        .enumerate()
        .map(|(i, n)| (board.measure(n).0, i))
        .filter(|&(v, _)| v > 0)
        .collect();
    if cut {
        by_vias.sort_by_key(|&(v, i)| (core::cmp::Reverse(v), i));
    }
    for (_, i) in by_vias {
        if run.ending() {
            break;
        }
        let net = &mut nets[i];
        let (vias, len) = board.measure(net);
        let old = net.wires.clone();
        board.uncommit(net);
        let ctl = Ctl {
            run: Some(run),
            flag: run.flag(),
            limit: hard_nodes(run, taken[i]).0,
        };
        let (res, pops) = board.route_net(
            world,
            sc,
            net,
            Mode::Hard,
            FEWER_VIA * board.via_scale,
            &ctl,
            None,
            None,
        );
        let (nv, nl) = board.measure(net);
        let ok = res.is_ok()
            && net.wires.len() >= old.len()
            && nv < vias
            && nl <= len * (1.0 + FEWER_STRETCH) + FEWER_SLACK * grid.g as f64 * (vias - nv) as f64;
        if ok {
            vias_saved += vias - nv;
        } else {
            net.wires = old;
        }
        board.commit(net);
        run.charge(pops);
        if run.poll() {
            break;
        }
    }
    vias_saved
}

/// How many rounds there are at most on the grid of the pitch for the connections that a
/// negotiation at twice the pitch left open, how many of them may pass since the best,
/// and the price of a claimed point in the first of them.
const FINER_ROUNDS: usize = 16;
const FINER_STALE: usize = 5;
const FINER_PRES: f32 = 8.0;
/// The share of what the run has left, in time and in work, that those rounds have at
/// most.
const FINER_SHARE: f64 = 0.1;

/// What the rounds on a grid twice as fine ended with.
struct Finer {
    board: Board,
    nets: Vec<WNet>,
    /// How many rounds there were on it, how many connections the routes left open on
    /// the grid before the first, and how many after the last.
    rounds: usize,
    before: usize,
    after: usize,
}

/// How many connections a net's wires leave open on the grid.
fn open_on_grid(net: &WNet) -> usize {
    let n = net.terms.len();
    let mut parent: Vec<usize> = (0..n).collect();
    let all = vec![true; net.wires.len()];
    for (terms, _) in kept_wires(n, net.wires.clone(), &all) {
        for &t in &terms[1..] {
            let (a, b) = (find(&mut parent, terms[0]), find(&mut parent, t));
            if a != b {
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    (0..n)
        .filter(|&t| find(&mut parent, t) == t)
        .count()
        .saturating_sub(1)
}

/// A wire of a grid on the grid twice as fine that has its origin: each of its nodes is
/// one of that grid, and between two of a layer there is the one halfway.
fn refined(from: &Board, to: &Board, w: &Wire) -> Wire {
    let mut nodes = Vec::with_capacity(w.nodes.len() * 2);
    let mut last: Option<(usize, i32, i32)> = None;
    for &n in &w.nodes {
        let (l, x, y) = from.unpack(n);
        if let Some((pl, px, py)) = last
            && pl == l
        {
            nodes.push(to.node(l, px + x, py + y));
        }
        nodes.push(to.node(l, 2 * x, 2 * y));
        last = Some((l, x, y));
    }
    Wire {
        nodes,
        start: w.start,
        end: w.end,
        from: w.from,
        to: w.to,
    }
}

/// Completes what a negotiation at twice the pitch left open by rounds on the grid of
/// the pitch, for the nets concerned alone.
///
/// The routes are legal by now: what was still shared was taken out and routed again
/// where nothing is claimed, and a net with no way there has a connection open. Left to
/// the passes, such a net is completed by taking others out for it, and each net taken
/// out comes back with more vias. Here the nets stay as their routes left them, on a
/// grid with a cell between every two of theirs. The nets with a connection open are
/// routed across the others at a price, between the lines of the grid they were on if
/// there is room, and through another net's claims if not; the nets whose claims they
/// are in are routed again in the next round, with what is in no one's way kept, at a
/// price that rises, until nothing is shared: the rounds of the negotiation, for a few
/// nets, with every other net where it is. What is still shared when they end is taken
/// out as before.
///
/// `plans` has, by net, what it was planned again as, if it was; `until` and `nodes`
/// are what the rounds may take. `None` if the grid is not one to halve, or the run
/// ended while it was made.
#[allow(clippy::too_many_arguments)]
fn finer_rounds(
    design: &Design,
    world: &World<'_>,
    jobs: &[Job],
    coarse: &Board,
    old: &[WNet],
    plans: &[Option<NetCtx>],
    run: &Run<'_>,
    (twice, seed): (bool, u64),
    (until, nodes): (Option<std::time::Instant>, u64),
    taken: &mut [u64],
) -> Option<Finer> {
    let g = coarse.grid;
    if g.g % 2 != 0 || g.g / 2 < 1000 {
        return None;
    }
    let grid = Grid {
        ox: g.ox,
        oy: g.oy,
        g: g.g / 2,
        w: g.dw(),
        h: g.dh(),
    };
    let (mut board, mut nets, mut sc) =
        setup_on(design, world, world, jobs, grid, (twice, 2), run).ok()?;
    // A via costs what it did, in steps half as long.
    board.via_scale = 2.0;
    for (i, net) in nets.iter_mut().enumerate() {
        net.via *= 2.0;
        if let Some(ctx) = &plans[i] {
            plan_again(&board, world, net, ctx, &mut sc);
        }
        net.wires = old[i]
            .wires
            .iter()
            .map(|w| refined(coarse, &board, w))
            .collect();
        board.commit(net);
    }
    if run.poll() {
        return None;
    }
    let before: usize = nets.iter().map(open_on_grid).sum();
    let mut order: Vec<usize> = (0..nets.len()).collect();
    order.sort_by_key(|&i| (nets[i].length, nets[i].net));
    let mut rng = Rng(seed ^ 0xF1BE_5EED_CAD1_AB00);
    let mut pres = FINER_PRES;
    let (most, stale) = (FINER_ROUNDS, FINER_STALE);
    // The routes after the round with the fewest points shared, and the fewest open.
    type Round = ((usize, usize), Vec<Vec<Wire>>);
    let mut best: Option<Round> = None;
    let mut best_at = 0;
    let mut rounds = 0;
    let began = run.work();
    let flag = run.flag();
    let over = |run: &Run<'_>| {
        run.work() - began >= nodes || until.is_some_and(|t| std::time::Instant::now() >= t)
    };
    for it in 0..=most {
        let mut points: Vec<(Conflict, usize)> = Vec::new();
        for (i, net) in nets.iter().enumerate() {
            points.extend(board.conflicts(net).into_iter().map(|c| (c, i)));
        }
        let total = points.len();
        let open: Vec<usize> = (0..nets.len())
            .filter(|&i| open_on_grid(&nets[i]) > 0)
            .collect();
        let score = (total, open.len());
        if it > 0 && best.as_ref().is_none_or(|b| score < b.0) {
            best = Some((score, nets.iter().map(|n| n.wires.clone()).collect()));
            best_at = it;
        }
        if (it > 0 && total == 0) || it == most || (it > 0 && it - best_at >= stale) || over(run) {
            break;
        }
        for (c, _) in &points {
            board.add_history(*c, HISTORY);
        }
        // The nets with something shared, and those with a connection open: the grid of
        // twice the pitch had no way for it, and this one may.
        let mut set: BTreeSet<usize> = points.iter().map(|&(_, i)| i).collect();
        set.extend(open);
        let mut reroute: Vec<usize> = order.iter().copied().filter(|i| set.contains(i)).collect();
        for i in (1..reroute.len()).rev() {
            if rng.next().is_multiple_of(4) {
                let j = (rng.next() % (i as u64 + 1)) as usize;
                reroute.swap(i, j);
            }
        }
        rounds = it + 1;
        let mode = Mode::Share(pres);
        let margin = (pres >= NEAR_PRES).then_some((2 * WINDOW, WINDOW_SHARE));
        let mut stopped = false;
        for i in reroute {
            let limit = run
                .limits(u64::MAX)
                .max_expansions
                .min(nodes.saturating_sub(run.work() - began));
            let ctl = Ctl {
                run: Some(run),
                flag,
                limit,
            };
            let task = Task {
                at: 0,
                keep: Some(board.clear_wires(&nets[i])),
                net: nets[i].clone(),
                mode,
                margin,
                limit,
            };
            let routed = board.route_task(world, &mut sc, task, &ctl);
            run.charge(routed.nodes.min(limit));
            if routed.stopped || run.poll() || over(run) {
                stopped = true;
                break;
            }
            taken[i] = routed.nodes;
            board.uncommit(&mut nets[i]);
            nets[i] = routed.net;
            board.commit(&mut nets[i]);
        }
        if stopped {
            break;
        }
        pres = (pres * PRES_STEP).min(PRES_MAX);
    }
    // Back to the best round, and what is still shared there taken out.
    let (_, wires) = best?;
    for net in &mut nets {
        board.uncommit(net);
    }
    for (net, w) in nets.iter_mut().zip(wires) {
        net.wires = w;
        board.commit(net);
    }
    if run.stopped().is_some() {
        return None;
    }
    legalise(&board, world, &mut nets, &mut sc, run, taken);
    let after: usize = nets.iter().map(open_on_grid).sum();
    Some(Finer {
        board,
        nets,
        rounds,
        before,
        after,
    })
}

/// How many rounds a negotiation has at most: at twice the pitch, forty, as cadlab's
/// router has; at the first, a hundred; and forty more each time the pads that the
/// routes cut off their fills were told.
fn most_rounds(coarse: bool, pour_rounds: usize) -> usize {
    (if coarse { ROUNDS_COARSE } else { ROUNDS }) + POUR_MORE * pour_rounds
}

/// The pitch of a negotiation's grid, given the lattice's.
fn grid_pitch(
    design: &Design,
    jobs: &[Job],
    options: &RouteOptions,
    (pitch, coarse, finer): (i64, bool, i64),
) -> i64 {
    let rules = &design.rules;
    // The pitch is half the tightest pitch of the tracks as they are routed: those of a
    // net routed narrower are its narrow ones.
    let pitch = if options.pitch.is_some() {
        pitch
    } else {
        jobs.iter()
            .map(|j| {
                (j.ctx.widths.iter().copied().min().unwrap_or(pitch)
                    + rules.clearance(Some(j.ctx.net), LayerId(0)))
                    / 2
            })
            .min()
            .map_or(pitch, |p| p.max(1000))
    };
    // Twice that on more than two layers, and where a board of two has no room for the
    // negotiation at it: a track's width and its clearance, at which two tracks side by
    // side are still a step apart.
    let pitch = if coarse { pitch * 2 } else { pitch };
    // Or finer, by halves of `finer`: a board whose negotiation does not end at the pitch
    // has passages between its parts that are off the grid's lines, and narrower than a
    // step.
    if finer > 2 {
        (pitch * 2 / finer).max(1000)
    } else {
        pitch
    }
}

/// The grid of a negotiation at `pitch` with nothing claimed on it, the nets as they are
/// before any is routed, each with the places where its routes may end at its pads, and
/// a search's memory. `world` is the board that the grid's places are asked of, with no
/// route of the nets on it; `tolls` is the world that says where the rooms of spokes
/// are, which may be the same.
fn setup(
    design: &Design,
    world: &World<'_>,
    tolls: &World<'_>,
    jobs: &[Job],
    (pitch, coarse, finer): (i64, bool, i64),
    bounds: Rect,
    run: &Run<'_>,
) -> Result<(Board, Vec<WNet>, Scratch), NoWhole> {
    let grid = make_grid(design, jobs, pitch, bounds);
    setup_on(design, world, tolls, jobs, grid, (coarse, finer), run)
}

/// [`setup`] on a grid that is given.
#[allow(clippy::too_many_lines)]
fn setup_on(
    design: &Design,
    world: &World<'_>,
    tolls: &World<'_>,
    jobs: &[Job],
    grid: Grid,
    (coarse, finer): (bool, i64),
    run: &Run<'_>,
) -> Result<(Board, Vec<WNet>, Scratch), NoWhole> {
    let rules = &design.rules;
    let layers = rules.layer_count();
    let cells = grid.cells();
    if cells == 0 || (layers as u64) * (cells as u64) >= u64::from(u32::MAX) / 8 {
        return Err(NoWhole::Not);
    }
    // Classes of halo and groups of rules.
    let mut halos: Vec<Halo> = Vec::new();
    #[allow(clippy::type_complexity)]
    let mut group_keys: Vec<(Vec<i64>, Vec<i64>, Option<(i64, i64)>, u64, Option<NetId>)> =
        Vec::new();
    let mut nets: Vec<WNet> = Vec::new();
    for (j, job) in jobs.iter().enumerate() {
        let ctx = &job.ctx;
        let clearances: Vec<i64> = (0..layers)
            .map(|l| rules.clearance(Some(ctx.net), LayerId(l as u8)))
            .collect();
        let halo = Halo {
            width: ctx.widths.iter().copied().max().unwrap_or(0),
            clearance: clearances.iter().copied().max().unwrap_or(0),
            via: ctx.via,
        };
        let hi = halos.iter().position(|h| *h == halo).unwrap_or_else(|| {
            halos.push(halo);
            halos.len() - 1
        });
        let alone = world.alone(ctx.net);
        let key = (
            ctx.widths.clone(),
            clearances,
            ctx.via,
            world.layer_mask(ctx.net),
            alone.then_some(ctx.net),
        );
        let gi = group_keys
            .iter()
            .position(|k| *k == key)
            .unwrap_or_else(|| {
                group_keys.push(key);
                group_keys.len() - 1
            });
        let (conns, length) = spanning(ctx);
        let mut boxes = vec![Rect::default(); ctx.terminals];
        let mut any = vec![false; ctx.terminals];
        for p in &ctx.pads {
            let r = Rect::new(p.center, p.center);
            boxes[p.terminal] = if any[p.terminal] {
                boxes[p.terminal].union(&r)
            } else {
                r
            };
            any[p.terminal] = true;
        }
        nets.push(WNet {
            job: j,
            net: ctx.net,
            halo: hi,
            group: gi,
            terms: vec![Vec::new(); ctx.terminals],
            boxes,
            dead: vec![false; conns.len()],
            looked: vec![false; conns.len()],
            conns,
            wires: Vec::new(),
            keys: Vec::new(),
            bbox: (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
            length,
            via: match (job.plane, coarse) {
                (true, true) => VIA * PLANE_VIA,
                (true, false) => PLANE_VIA_FIRST,
                (false, _) => VIA,
            },
        });
    }
    if group_keys.len() > GROUPS {
        return Err(NoWhole::Not);
    }
    // More classes of halo than there are maps for are taken together, two at a time:
    // the two whose nets lose the least room by being kept apart as the wider of them
    // with the larger clearance and via is.
    {
        let reach = |h: &Halo| h.width / 2 + h.clearance + h.via.map_or(0, |v| v.0 / 8);
        let join = |a: &Halo, b: &Halo| Halo {
            width: a.width.max(b.width),
            clearance: a.clearance.max(b.clearance),
            via: match (a.via, b.via) {
                (Some(x), Some(y)) => Some((x.0.max(y.0), x.1.max(y.1))),
                (x, y) => x.or(y),
            },
        };
        // By class, the classes of the nets that it stands for, each with its connections.
        let mut members: Vec<Vec<(Halo, i64)>> = halos
            .iter()
            .enumerate()
            .map(|(hi, h)| {
                let conns: usize = nets
                    .iter()
                    .filter(|n| n.halo == hi)
                    .map(|n| n.conns.len())
                    .sum();
                vec![(*h, conns as i64)]
            })
            .collect();
        let mut of: Vec<usize> = (0..halos.len()).collect();
        while halos.len() > CLASSES {
            let mut best: Option<(i64, usize, usize)> = None;
            for a in 0..halos.len() {
                for b in a + 1..halos.len() {
                    let m = join(&halos[a], &halos[b]);
                    let loss: i64 = members[a]
                        .iter()
                        .chain(&members[b])
                        .map(|(h, n)| n * (reach(&m) - reach(h)))
                        .sum();
                    if best.is_none_or(|(l, _, _)| loss < l) {
                        best = Some((loss, a, b));
                    }
                }
            }
            let Some((_, a, b)) = best else { break };
            halos[a] = join(&halos[a], &halos[b]);
            let moved = members.remove(b);
            members[a].extend(moved);
            halos.remove(b);
            for o in &mut of {
                if *o == b {
                    *o = a;
                } else if *o > b {
                    *o -= 1;
                }
            }
        }
        for net in &mut nets {
            net.halo = of[net.halo];
        }
    }
    let blocks = ((grid.w + BLOCK - 1) / BLOCK) as usize * ((grid.h + BLOCK - 1) / BLOCK) as usize;
    let groups: Vec<Group> = group_keys
        .into_iter()
        .zip(0..)
        .map(|((widths, _, via, _, alone), gi)| Group {
            net: nets
                .iter()
                .find(|n| n.group == gi)
                .map_or(NetId(0), |n| n.net),
            alone: alone.is_some(),
            widths,
            via,
            moves: (0..layers * cells).map(|_| AtomicU16::new(0)).collect(),
            points: (0..layers * cells).map(|_| AtomicU8::new(0)).collect(),
            vias: (0..cells).map(|_| AtomicU8::new(0)).collect(),
            filled: (0..layers * blocks).map(|_| AtomicU8::new(0)).collect(),
            owners: (0..layers * blocks).map(|_| OnceLock::new()).collect(),
            via_owner: (0..cells).map(|_| AtomicU32::new(0)).collect(),
        })
        .collect();
    let dn = grid.dpoints();
    let mut board = Board {
        grid,
        layers,
        occ_t: halos
            .iter()
            .map(|_| (0..layers * dn).map(|_| AtomicU16::new(0)).collect())
            .collect(),
        occ_v: halos
            .iter()
            .map(|_| (0..cells).map(|_| AtomicU16::new(0)).collect())
            .collect(),
        halos,
        groups,
        hist_t: (0..layers * dn).map(|_| AtomicU32::new(0)).collect(),
        hist_v: (0..cells).map(|_| AtomicU32::new(0)).collect(),
        spoke: Vec::new(),
        spoke_cost: 0.0,
        along: (0..layers)
            .map(|l| if layers < 2 { 2 } else { (l % 2) as u8 })
            .collect(),
        hole_clearance: rules.spec().hole_clearance,
        hole_to_hole: rules.spec().hole_to_hole,
        via_scale: if finer > 2 { finer as f32 / 2.0 } else { 1.0 },
        near_only: false,
    };
    // Where the rooms of spokes are. A plane of another net costs a track nothing here,
    // as in cadlab's router: at twice its length inside one, the nets of a board with
    // planes had half the board's layers at half price for a via, and went under each
    // other where they had room to go round (`tinytapeout-demo`, four layers, at 150
    // million nodes over four orders: 324 vias and 5133 mm with 5 connections open, for
    // 299 and 5071 mm with none; `docs/benchmarks.md`). The passes that route a net at a
    // time, and move others for it, pay for a plane as they did.
    {
        let mut spoke = Vec::new();
        let mut any_spoke = false;
        for l in 0..layers {
            for y in 0..grid.h {
                for x in 0..grid.w {
                    let (s, cost) = tolls.spoke_toll(l as u8, grid.cell(x, y));
                    any_spoke |= s != 0;
                    board.spoke_cost = cost as f32;
                    spoke.push(s);
                }
            }
            if run.poll() {
                return Err(NoWhole::Not);
            }
        }
        if any_spoke {
            board.spoke = spoke;
        }
    }
    let mut sc = board.scratch();
    // Where routes may end at the pads.
    for net in &mut nets {
        let ctx = &jobs[net.job].ctx;
        for pad in &ctx.pads {
            for l in pad.layers.iter() {
                let li = usize::from(l.0);
                if li >= layers {
                    continue;
                }
                let found = board.access(
                    world,
                    net,
                    pad.center,
                    pad.bbox,
                    l,
                    ctx.widths[li],
                    &mut sc.near,
                );
                net.terms[pad.terminal].extend(found);
            }
        }
        if run.poll() {
            return Err(NoWhole::Not);
        }
    }
    Ok((board, nets, sc))
}

/// Negotiates for every net of `jobs` at once, and returns what each ends with.
///
/// `make_world` makes a world as `world` is for a thread of its own to ask, with no run:
/// `threads` of them route the nets of a round a few at a time ([`WAVE`]), each against
/// the claims as they are before any of the few, and the few then have their turns in
/// their order, so that what comes out is the same for any number of threads.
///
/// `None` if the board is not one for it: too many classes of nets, too large a grid, or
/// nothing to route.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub(crate) fn route<'r>(
    design: &Design,
    world: &World<'r>,
    make_world: &(dyn Fn() -> Option<World<'r>> + Sync),
    jobs: &[Job],
    pours: Option<&Pours>,
    fixed: (&[Track], &[Via]),
    options: &RouteOptions,
    run: &Run<'r>,
    (share, was): (f64, Option<super::Stopped>),
    (pitch, coarse, finer): (i64, bool, i64),
    bounds: Rect,
) -> Result<Whole, NoWhole> {
    let rules = &design.rules;
    let layers = rules.layer_count();
    if jobs.is_empty() || layers == 0 {
        return Err(NoWhole::Not);
    }
    let pitch = grid_pitch(design, jobs, options, (pitch, coarse, finer));
    let (board, mut nets, mut sc) = setup(
        design,
        world,
        world,
        jobs,
        (pitch, coarse, finer),
        bounds,
        run,
    )?;
    // The nets in order, the shortest first.
    let mut order: Vec<usize> = (0..nets.len()).collect();
    order.sort_by_key(|&i| (nets[i].length, nets[i].net));
    let mut rng = Rng(options.seed.unwrap_or(0) ^ 0x5EED_CAD1_AB00_0000);
    let mut pres = PRES;
    // The first round takes the nets in strides through that order, so that the first of
    // them say what all of them take.
    let mut reroute: Vec<usize> = {
        let n = order.len();
        let gcd = |mut a: usize, mut b: usize| {
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a
        };
        let mut stride = (n * 5 / 8).max(1);
        while gcd(stride, n) != 1 {
            stride -= 1;
        }
        (0..n).map(|k| order[k * stride % n]).collect()
    };
    type Snapshot = (usize, Vec<Vec<Wire>>);
    let mut best: Option<Snapshot> = None;
    let mut best_at = 0;
    let mut rounds = 0;
    let mut cut = false;
    let mut given_up = false;
    let mut capped = false;
    let mut pour_rounds = 0;
    let mut pour_cuts = 0;
    let mut refills = 0;
    // With the routes made legal before the pours are filled: whether that goes on, the
    // legal routes that the nets were last planned again from, with the connections that
    // were open on that board, the nets taken out to make the routes legal that the
    // rounds ended with, the points shared when routes were last made legal, and how
    // often the routes went back.
    let legal = options.legal_pours;
    let mut pours_on = true;
    type Sites = Vec<(usize, Vec<TermPad>)>;
    let mut pending: Option<(usize, Vec<WNet>, Sites)> = None;
    let mut early_out = 0;
    let mut early_shared: Option<usize> = None;
    let mut pours_back = 0;
    // Whether a negotiation still going late in its share has looked at its pours: one
    // that plans no net on more pours than before has nothing more cut off for it.
    let mut mid_checked = !options.outer_pours;
    let mut sites: Vec<(usize, Vec<TermPad>)> = Vec::new();
    // By net, what it was planned again as, if it was.
    let mut planned_as: Vec<Option<NetCtx>> = vec![None; nets.len()];
    // What the first round took, in time and in nodes, once it has ended.
    let mut first_round: Option<(std::time::Duration, u64)> = None;
    let threads = options.threads.max(1);
    let board = board;
    // The nets to route, the next first, and whether there are no more to come.
    let queue: Mutex<(VecDeque<Task>, bool)> = Mutex::new((VecDeque::new(), false));
    let wake = std::sync::Condvar::new();
    let flag = run.flag();
    let (done_tx, done_rx) = mpsc::channel::<(usize, Ahead)>();
    // By net, the nodes that it took the last time it was routed.
    let mut taken: Vec<u64> = vec![0; nets.len()];
    std::thread::scope(|scope| {
        for _ in 1..threads {
            let done_tx = done_tx.clone();
            let (board, queue, wake) = (&board, &queue, &wake);
            scope.spawn(move || {
                let Some(world) = make_world() else { return };
                let mut sc = board.scratch();
                loop {
                    let task = {
                        let mut q = queue
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        loop {
                            if let Some(t) = q.0.pop_front() {
                                break Some(t);
                            }
                            if q.1 {
                                break None;
                            }
                            q = wake
                                .wait(q)
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                    };
                    let Some(task) = task else {
                        break;
                    };
                    let ctl = Ctl {
                        run: None,
                        flag,
                        limit: task.limit,
                    };
                    let at = task.at;
                    let routed = board.route_task(&world, &mut sc, task, &ctl);
                    if done_tx.send((at, routed)).is_err() {
                        break;
                    }
                }
            });
        }
        let began = (std::time::Instant::now(), run.work());
        let total_length: i64 = nets.iter().map(|n| n.length).sum();
        let mut sampled = false;
        let mut length_done = 0i64;
        let connections: usize = nets.iter().map(|n| n.conns.len()).sum();
        let mut extended = false;
        let mut it = 0;
        let mut round_began = began;
        // Each time the pads cut off are told, the rounds have as many more for them.
        while it < most_rounds(coarse, pour_rounds) {
            it += 1;
            let it = it - 1;
            rounds = it + 1;
            // The nets of the round, a few at a time. Those few are routed at once, each
            // against the claims of all the others as they are before any of the few,
            // from its wires that are in no other net's way; then they have their turns
            // one after another: a net gives up its claims and claims what it was routed
            // as, if that is in no claim that it was not in when it was routed, and else
            // it is routed again there and then, against the claims as they are. So a net
            // is never in another's way without having counted it, and the routes are the
            // same on any number of threads: what a net is routed as depends on the
            // claims alone, and those change between the few and not while they are
            // routed.
            let mode = Mode::Share(pres);
            let partial = it > 0;
            // Late in a negotiation at twice the pitch, where the price of a shared
            // point goes on rising, a search looks round its two ends first.
            let margin = (coarse && pres >= NEAR_PRES).then_some((WINDOW, WINDOW_SHARE));
            let n = reroute.len();
            let mut head = 0;
            'turns: while head < n {
                let end = (head + WAVE).min(n);
                let budget = run.limits(u64::MAX).max_expansions;
                {
                    let mut q = queue
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    for at in head..end {
                        let net = &nets[reroute[at]];
                        q.0.push_back(Task {
                            at,
                            keep: partial.then(|| board.clear_wires(net)),
                            net: net.clone(),
                            mode,
                            margin,
                            limit: budget,
                        });
                    }
                }
                if end - head > 1 {
                    wake.notify_all();
                }
                let mut routed: Vec<Option<Ahead>> = (head..end).map(|_| None).collect();
                let mut left = end - head;
                while left > 0 {
                    let task = queue
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0
                        .pop_front();
                    if let Some(task) = task {
                        let ctl = Ctl {
                            run: Some(run),
                            flag,
                            limit: task.limit,
                        };
                        let at = task.at;
                        routed[at - head] = Some(board.route_task(world, &mut sc, task, &ctl));
                        left -= 1;
                        continue;
                    }
                    match done_rx.recv_timeout(std::time::Duration::from_millis(5)) {
                        Ok((at, r)) => {
                            routed[at - head] = Some(r);
                            left -= 1;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            run.poll();
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                // Their turns.
                let first = head;
                let mut stopped = false;
                for r in routed {
                    let Some(r) = r else {
                        stopped = true;
                        break;
                    };
                    let i = reroute[head];
                    let mut spent = r.nodes;
                    taken[i] = r.nodes;
                    stopped = r.stopped || spent > budget;
                    if !stopped {
                        let before = nets[i].clone();
                        board.uncommit(&mut nets[i]);
                        nets[i] = r.net;
                        board.commit(&mut nets[i]);
                        if head > first && !none_new(&board.conflicts(&nets[i]), &r.conflicts) {
                            board.uncommit(&mut nets[i]);
                            nets[i] = before;
                            board.commit(&mut nets[i]);
                            let limit = budget - spent;
                            let ctl = Ctl {
                                run: Some(run),
                                flag,
                                limit,
                            };
                            let task = Task {
                                at: head,
                                keep: partial.then(|| board.clear_wires(&nets[i])),
                                net: nets[i].clone(),
                                mode,
                                margin,
                                limit,
                            };
                            let again = board.route_task(world, &mut sc, task, &ctl);
                            spent += again.nodes;
                            stopped = again.stopped || spent > budget;
                            if !stopped {
                                board.uncommit(&mut nets[i]);
                                nets[i] = again.net;
                                board.commit(&mut nets[i]);
                            }
                        }
                    }
                    run.charge(spent.min(budget));
                    if stopped || run.poll() {
                        stopped = true;
                        break;
                    }
                    head += 1;
                    // The first round, once it has routed a share of the nets' length,
                    // says what the whole negotiation takes: with less than that left, it
                    // is given up here.
                    if it == 0 && !sampled {
                        length_done += nets[i].length;
                        if length_done as f64 >= SAMPLE * total_length as f64 && length_done > 0 {
                            sampled = true;
                            let share = length_done as f64 / total_length as f64;
                            let nodes = (run.work() - began.1) as f64;
                            let (factor, work_factor) = if coarse {
                                (COARSE_ROUNDS, COARSE_WORK)
                            } else {
                                (WHOLE_ROUNDS, WHOLE_ROUNDS)
                            };
                            let needed = work_factor * nodes / share;
                            let short_of_work = run.limits(u64::MAX).max_expansions != u64::MAX
                                && needed > run.limits(u64::MAX).max_expansions as f64 + nodes;
                            let took = began.0.elapsed().as_secs_f64();
                            let short_of_time = run.part_left().is_some_and(|left| {
                                factor * took / share > left.as_secs_f64() + took
                            });
                            if short_of_work || short_of_time {
                                given_up = true;
                                cut = true;
                                break 'turns;
                            }
                        }
                    }
                }
                if stopped {
                    // A net that was stopped is as it was. Near its end, the
                    // negotiation goes on once.
                    let shared: usize = nets.iter().map(|n| board.conflicts(n).len()).sum();
                    if !extended
                        && it > 0
                        && run.stopped().is_none()
                        && shared * NEARLY < connections
                    {
                        extended = true;
                        run.extend(share, share, was);
                        if !run.ending() {
                            continue;
                        }
                    }
                    cut = true;
                    break;
                }
            }
            if given_up {
                break;
            }
            if it == 0 && !cut {
                first_round = Some((round_began.0.elapsed(), run.work() - round_began.1));
            }
            round_began = (std::time::Instant::now(), run.work());
            // What is shared.
            let board = &board;
            let mut points: Vec<(Conflict, usize)> = Vec::new();
            let mut net_conf = vec![0usize; nets.len()];
            for (i, net) in nets.iter().enumerate() {
                let c = board.conflicts(net);
                net_conf[i] = c.len();
                points.extend(c.into_iter().map(|c| (c, i)));
            }
            let total: usize = net_conf.iter().sum();
            if !cut && best.as_ref().is_none_or(|b| total < b.0) {
                best = Some((total, nets.iter().map(|n| n.wires.clone()).collect()));
                best_at = it;
            }
            // With the routes made legal before the pours are filled, the rounds that
            // end at their most have their pours looked at as well.
            let at_most =
                legal && pours_on && pours.is_some() && it + 1 >= most_rounds(coarse, pour_rounds);
            // And those of a negotiation that is still at its first rounds late in its
            // share, if the routes as they are cut enough pads off: the rounds for them
            // then have the rest of the share.
            let mut pause = false;
            if legal
                && pours_on
                && !mid_checked
                && !cut
                && total > 0
                && pour_rounds == 0
                && !at_most
                && it - best_at < STALE
                && let Some(pours) = pours
            {
                let by_time = run.part_left().map(|left| {
                    let took = began.0.elapsed().as_secs_f64();
                    took / (took + left.as_secs_f64()).max(1e-9)
                });
                let left = run.limits(u64::MAX).max_expansions;
                let by_work = (left != u64::MAX).then(|| {
                    let worked = (run.work() - began.1) as f64;
                    worked / (worked + left as f64).max(1.0)
                });
                let used = by_time.into_iter().chain(by_work).fold(0.0, f64::max);
                if used >= POUR_FIRST {
                    mid_checked = true;
                    let copper: Vec<Option<NetCopper>> = nets
                        .iter()
                        .map(|n| {
                            (!n.wires.is_empty())
                                .then(|| NetCopper::from_tree(&jobs[n.job].ctx, &tree_of(board, n)))
                        })
                        .collect();
                    let off: usize = pours
                        .jobs()
                        .filter_map(|k| {
                            pours.rejoin(world, design, jobs, &copper, k, pitch, None, &[])
                        })
                        .map(|again| again.cut)
                        .sum();
                    pause = off > 0 && off * POUR_PAUSE >= connections;
                }
            }
            if total == 0 || cut || it - best_at >= STALE || at_most || pause {
                // The pads that the routes as they are cut off their fills: their nets
                // are planned again with them, and the rounds go on from the prices as
                // they are, for the nets round them to make room.
                let mut changed: Vec<usize> = Vec::new();
                if legal {
                    // In cadlab's router's order: the routes of the best round, made
                    // legal, are what the board would get, and the pours are filled
                    // around those. With more connections open than on the legal routes
                    // that the nets were last planned again from, the routes go back
                    // to those, and the pours are left to the passes.
                    if let Some(pours) = pours.filter(|_| {
                        !cut && pours_on && (pending.is_some() || pour_rounds < POUR_ROUNDS)
                    }) {
                        if let Some((least, wires)) = best.take() {
                            early_shared = Some(least);
                            if least < total {
                                for net in &mut nets {
                                    board.uncommit(net);
                                }
                                for (net, w) in nets.iter_mut().zip(wires) {
                                    net.wires = w;
                                    board.commit(net);
                                }
                            }
                        }
                        // The routes of the best round as they are: the rounds go on from
                        // those, and a net that making them legal took out, and found no
                        // way for where nothing is claimed, is in them as it was.
                        let shared_routes: Vec<Vec<Wire>> =
                            nets.iter().map(|n| n.wires.clone()).collect();
                        early_out = legalise(board, world, &mut nets, &mut sc, run, &taken);
                        let copper: Vec<Option<NetCopper>> = nets
                            .iter()
                            .map(|n| {
                                (!n.wires.is_empty()).then(|| {
                                    NetCopper::from_tree(&jobs[n.job].ctx, &tree_of(board, n))
                                })
                            })
                            .collect();
                        let real = (!run.poll() && pours.outer(design))
                            .then(|| pours.refill(run, design, jobs, &copper, fixed, false))
                            .flatten();
                        refills += usize::from(real.is_some());
                        let (opens, plans) = open_on_fills(
                            world,
                            design,
                            jobs,
                            pours,
                            &copper,
                            real.as_ref(),
                            &sites,
                            pitch,
                        );
                        let open: usize = opens.iter().sum();
                        if let Some((before, old, old_sites)) = pending.take()
                            && open > before
                        {
                            for net in &mut nets {
                                board.uncommit(net);
                            }
                            nets = old;
                            for net in &mut nets {
                                net.keys.clear();
                                board.commit(net);
                            }
                            sites = old_sites;
                            pours_on = false;
                            pours_back += 1;
                        } else if pour_rounds < POUR_ROUNDS && !plans.is_empty() && !run.poll() {
                            pending = Some((open, nets.clone(), sites.clone()));
                            pour_rounds += 1;
                            early_out = 0;
                            for net in &mut nets {
                                board.uncommit(net);
                            }
                            for (net, w) in nets.iter_mut().zip(shared_routes) {
                                net.wires = w;
                                board.commit(net);
                            }
                            for (k, again) in plans {
                                pour_cuts += again.cut;
                                plan_again(board, world, &mut nets[k], &again.ctx, &mut sc);
                                planned_as[k] = Some(again.ctx.clone());
                                match sites.iter_mut().find(|(j, _)| *j == k) {
                                    Some((_, s)) => s.extend(again.sites),
                                    None => sites.push((k, again.sites)),
                                }
                                changed.push(k);
                            }
                        }
                    }
                } else if let Some(pours) = pours.filter(|_| !cut && pour_rounds < POUR_ROUNDS) {
                    pour_rounds += 1;
                    let copper: Vec<Option<NetCopper>> = nets
                        .iter()
                        .map(|n| {
                            (!n.wires.is_empty())
                                .then(|| NetCopper::from_tree(&jobs[n.job].ctx, &tree_of(board, n)))
                        })
                        .collect();
                    // By the host's own fill around the routes as they are, if it gives
                    // one.
                    let real = pours
                        .outer(design)
                        .then(|| pours.refill(run, design, jobs, &copper, fixed, false))
                        .flatten();
                    refills += usize::from(real.is_some());
                    for k in pours.jobs() {
                        if run.poll() {
                            break;
                        }
                        // With the places in its fill that the rounds before gave it.
                        let given = sites
                            .iter()
                            .find(|(j, _)| *j == k)
                            .map_or(&[][..], |(_, s)| s.as_slice());
                        let Some(again) = pours.rejoin(
                            world,
                            design,
                            jobs,
                            &copper,
                            k,
                            pitch,
                            real.as_ref(),
                            given,
                        ) else {
                            continue;
                        };
                        pour_cuts += again.cut;
                        plan_again(board, world, &mut nets[k], &again.ctx, &mut sc);
                        planned_as[k] = Some(again.ctx.clone());
                        match sites.iter_mut().find(|(j, _)| *j == k) {
                            Some((_, s)) => s.extend(again.sites),
                            None => sites.push((k, again.sites)),
                        }
                        changed.push(k);
                    }
                }
                if changed.is_empty() || run.poll() {
                    break;
                }
                reroute = order
                    .iter()
                    .copied()
                    .filter(|i| changed.contains(i))
                    .collect();
                best = None;
                best_at = it;
                continue;
            }
            let mut hot: Vec<(i32, i32)> = points
                .iter()
                .map(|(c, _)| board.conflict_cell(*c))
                .collect();
            for (c, _) in &points {
                board.add_history(*c, HISTORY);
            }
            // On a finer grid than the pitch the price rises as at twice the pitch: it is
            // begun only where the rounds at the pitch, with their price stopped at thirty
            // steps, did not end.
            let most = if coarse || finer > 2 {
                PRES_MAX
            } else {
                PRES_MAX_FIRST
            };
            pres = (pres * PRES_STEP).min(most);
            // The nets with something shared, and those whose routes are near it.
            let mut set: BTreeSet<usize> = BTreeSet::new();
            hot.sort_unstable();
            hot.dedup();
            for (_, i) in &points {
                set.insert(*i);
            }
            let _ = &hot;
            reroute = order.iter().copied().filter(|i| set.contains(i)).collect();
            // The order is moved a little, by the seed, so that it does not go round.
            for i in (1..reroute.len()).rev() {
                if rng.next().is_multiple_of(4) {
                    let j = (rng.next() % (i as u64 + 1)) as usize;
                    reroute.swap(i, j);
                }
            }
            order.sort_by_key(|&i| core::cmp::Reverse(net_conf[i]));
            order.sort_by_key(|&i| nets[i].length);
        }
        // Whether the rounds ended at the most that there are.
        capped = !cut && !given_up && it >= most_rounds(coarse, pour_rounds);
        // The threads have nothing more to do.
        queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .1 = true;
        wake.notify_all();
    });
    drop(done_tx);
    if given_up {
        return Err(NoWhole::NoRoom);
    }
    // Back to the best round.
    let mut shared = early_shared.unwrap_or(0);
    if let Some((total, wires)) = best {
        shared = total;
        let now: usize = nets.iter().map(|n| board.conflicts(n).len()).sum();
        if total < now || cut {
            for net in &mut nets {
                board.uncommit(net);
            }
            for (net, w) in nets.iter_mut().zip(wires) {
                net.wires = w;
                board.commit(net);
            }
        }
    }
    // At twice the pitch, a negotiation that its share ended has what follows all the
    // same, within a few times what its first round took: the nets taken out routed
    // again where nothing is claimed, and each net once more for fewer vias. Left to the
    // passes, the first is done by searches that cost three times as much a node, and
    // the second not at all: the routes of a negotiation that was cut were the ones with
    // the most vias. Not at the first pitch, where a round costs five times as much:
    // `corne-cherry`, cut a third of the way through by 40 million nodes, had 95
    // connections open over three orders with it for 79.
    if cut
        && coarse
        && let Some((time, nodes)) = first_round
    {
        let time = run.left().map_or(time.mul_f64(TAIL), |left| {
            time.mul_f64(TAIL).min(left.mul_f64(TAIL_SHARE))
        });
        let nodes = run
            .work_left()
            .map_or((nodes as f64 * TAIL) as u64, |left| {
                ((nodes as f64 * TAIL) as u64).min((left as f64 * TAIL_SHARE) as u64)
            });
        run.allow(time, nodes, was);
    }
    let taken_out = early_out + legalise(&board, world, &mut nets, &mut sc, run, &taken);
    let vias_saved = fewer_vias(&board, world, &mut nets, &mut sc, run, &taken, cut);
    // With connections left open at twice the pitch, rounds on the grid of the pitch for
    // their nets and the nets in their way, kept if fewer are open after them.
    let mut finer_stats = (0, 0, 0);
    let connections: usize = nets.iter().map(|n| n.conns.len()).sum();
    let (board, nets) = if options.whole_finer
        && coarse
        && finer <= 2
        && (!cut || shared * NEARLY < connections)
        && run.stopped().is_none()
        && nets.iter().any(|n| open_on_grid(n) > 0)
    {
        let least = FINER_SHARE;
        let time = run.left().map(|left| left.mul_f64(least));
        let work = run
            .work_left()
            .map_or(u64::MAX, |left| (left as f64 * least) as u64);
        // A negotiation that its share ended has that much more for them.
        if run.ending() {
            run.allow(time.unwrap_or(std::time::Duration::MAX), work, was);
        }
        let until = time.map(|t| std::time::Instant::now() + t);
        let seed = options.seed.unwrap_or(0);
        match finer_rounds(
            design,
            world,
            jobs,
            &board,
            &nets,
            &planned_as,
            run,
            (coarse, seed),
            (until, work),
            &mut taken,
        ) {
            Some(f) => {
                finer_stats = (f.rounds, f.before, f.after);
                if f.after < f.before {
                    (f.board, f.nets)
                } else {
                    (board, nets)
                }
            }
            None => (board, nets),
        }
    } else {
        (board, nets)
    };
    let grid = board.grid;
    // What each net ends with, as a tree.
    let mut trees: Vec<Option<Tree>> = vec![None; jobs.len()];
    for net in &nets {
        if !net.wires.is_empty() {
            trees[net.job] = Some(tree_of(&board, net));
        }
    }
    Ok(Whole {
        trees,
        pitch: grid.g,
        rounds,
        shared,
        taken_out,
        vias_saved,
        sites,
        pour_cuts,
        refills,
        pours_back,
        finer: finer_stats,
        ended: !cut,
        capped,
    })
}

/// How many nodes a search may expand that routes a net again among the routes as they
/// are: for each step of the net's length, at least and at most. A net with no better way
/// looks at every place it can get to before it says so.
const AGAIN_PER_STEP: u64 = 200;
const AGAIN_LEAST: u64 = 20_000;
const AGAIN_NODES: u64 = 400_000;
/// How many times the nets are gone through: once for fewer vias alone, and then for
/// shorter ways as well, for as long as a time through them gains.
const AGAIN_SWEEPS: usize = 3;
/// How much shorter a net routed again with as many vias has to be to be kept: this
/// share of its length, and a step at least.
const AGAIN_SHORTER: f64 = 0.02;

/// What routing the nets again among the routes as they are ended with.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Again {
    /// The nets looked at, and those of them that were kept as routed again.
    pub(crate) tried: usize,
    pub(crate) kept: usize,
    /// The vias that those have fewer, and the track, in nanometres.
    pub(crate) vias: usize,
    pub(crate) shorter: f64,
    /// The nodes that the searches expanded.
    pub(crate) expanded: u64,
}

/// Routes each net again where the others are, once every net has its route and none is
/// in another's way, and keeps what has fewer vias and is little longer, or as many and is
/// shorter: what cadlab's router does after its own negotiation, by the negotiation's
/// search, which is one over a grid whose every place was asked of the board once.
///
/// The routes are no longer the negotiation's: the passes made some, cleanup pulled them
/// tight, and an earlier run may have left them. So each net claims on the grid what its
/// copper keeps others away from wherever it lies ([`Board::copper_keys`]), a net gives
/// up its claims while it is routed again where nothing is claimed, and what the search
/// finds is the net's only if `world`, which has every other net's copper as it is, says
/// that all of it is free: the grid errs on the side of room for copper that is off it,
/// and a route that it allows and the world does not is not kept.
///
/// `base` is the board without the routes, which the grid is asked of; `may` says by job
/// whether its net may be routed again. A net that is not joined whole, one with copper
/// to keep, and one whose routes are stubs to its plane are left as they are.
#[allow(clippy::too_many_arguments)]
pub(crate) fn again(
    design: &Design,
    base: &World<'_>,
    world: &mut World<'_>,
    jobs: &[Job],
    copper: &mut [Option<NetCopper>],
    may: &[bool],
    options: &RouteOptions,
    run: &Run<'_>,
    (pitch, coarse): (i64, bool),
    bounds: Rect,
    until: Option<std::time::Instant>,
) -> Again {
    let mut out = Again::default();
    if jobs.is_empty() || design.rules.layer_count() == 0 {
        return out;
    }
    let pitch = grid_pitch(design, jobs, options, (pitch, coarse, 2));
    let Ok((mut board, mut nets, mut sc)) =
        setup(design, base, world, jobs, (pitch, coarse, 2), bounds, run)
    else {
        return out;
    };
    // No layer has a direction of its own here: that is for the nets to find room among
    // each other, and with every route where it is, a step across it is a step, and a
    // way on one layer that is no longer than one through a via is the better of the two.
    board.along = vec![2; board.layers];
    // And a net's better way is near the one it has.
    board.near_only = true;
    let late = |run: &Run<'_>| {
        run.poll() || run.ending() || until.is_some_and(|t| std::time::Instant::now() >= t)
    };
    // What every net claims as it is.
    let mut keys: Vec<Vec<u64>> = nets
        .iter()
        .map(|n| {
            copper[n.job]
                .as_ref()
                .map_or_else(Vec::new, |c| board.copper_keys(n.halo, c))
        })
        .collect();
    for k in &keys {
        board.occupy(k, true);
    }
    let whole = |ctx: &NetCtx, c: &NetCopper| {
        let pieces = c.pieces(ctx);
        pieces.apart() == 0 && pieces.missing().is_empty()
    };
    let per_via = 1e6 * options.via_cost * super::cleanup::VIA_COST_FACTOR / 2.0;
    let eligible: Vec<bool> = nets
        .iter()
        .map(|n| {
            let job = &jobs[n.job];
            may.get(n.job).copied().unwrap_or(false)
                && !job.plane
                && copper[n.job]
                    .as_ref()
                    .is_some_and(|c| c.keep.is_empty() && !c.is_empty() && whole(&job.ctx, c))
        })
        .collect();
    for sweep in 0..AGAIN_SWEEPS {
        // The nets with the most vias first, and then the longest for what they join.
        let mut order: Vec<usize> = (0..nets.len()).filter(|&i| eligible[i]).collect();
        let measure = |i: usize| {
            let c = copper[nets[i].job].as_ref();
            (
                c.map_or(0, |c| c.vias.len()),
                c.map_or(0.0, NetCopper::length) - nets[i].length as f64,
            )
        };
        order.sort_by(|&a, &b| {
            let (ma, mb) = (measure(a), measure(b));
            mb.0.cmp(&ma.0).then(mb.1.total_cmp(&ma.1)).then(a.cmp(&b))
        });
        let mut gained = false;
        for i in order {
            if late(run) {
                return out;
            }
            let j = nets[i].job;
            let ctx = &jobs[j].ctx;
            let Some(mut old) = copper[j].take() else {
                continue;
            };
            let (ov, ol) = (old.vias.len(), old.length());
            // The first time through the nets is for fewer vias alone: a net laid a
            // shorter way first may be in the way that another has to lose a via by. A
            // net with no via is looked at once for a shorter way, the second time.
            if ov == 0 && (sweep != 1 || ol <= nets[i].length as f64 * (1.0 + AGAIN_SHORTER)) {
                copper[j] = Some(old);
                continue;
            }
            out.tried += 1;
            board.occupy(&keys[i], false);
            old.unplace(world);
            let mut found: Option<NetCopper> = None;
            // With vias dear, which finds the fewest and the shortest way with those;
            // and where that has fewer and is too long for them, at their own price.
            for via in [FEWER_VIA, nets[i].via] {
                // As many nodes as its length has steps, a few hundred times over: a net
                // with no better way looks at every place it can get to before it says so.
                let steps = (ol / board.grid.g as f64) as u64;
                let most = (steps * AGAIN_PER_STEP).clamp(AGAIN_LEAST, AGAIN_NODES);
                let limit = run.limits(most).max_expansions;
                let ctl = Ctl {
                    run: Some(run),
                    flag: run.flag(),
                    limit,
                };
                nets[i].wires.clear();
                let (res, pops) = board.route_net(
                    base,
                    &mut sc,
                    &mut nets[i],
                    Mode::Hard,
                    via,
                    &ctl,
                    None,
                    Some((WINDOW, WINDOW_SHARE)),
                );
                run.charge(pops);
                out.expanded += pops;
                if res.is_err() || nets[i].wires.is_empty() {
                    break;
                }
                let mut c = NetCopper::from_tree(ctx, &tree_of(&board, &nets[i]));
                c.prune(ctx);
                if !c.all_free(world, ctx) {
                    continue;
                }
                super::cleanup::tighten_net(world, ctx, &mut c);
                if !whole(ctx, &c) {
                    continue;
                }
                let (nv, nl) = (c.vias.len(), c.length());
                // A via less may cost what it does when cleanup routes a net again for
                // fewer: half of what a via costs that search, in track.
                let fewer = nv < ov && nl <= ol + (ov - nv) as f64 * per_via;
                let shorter = sweep > 0
                    && nv == ov
                    && nl < ol - (ol * AGAIN_SHORTER).max(board.grid.g as f64);
                if fewer || shorter {
                    out.vias += ov - nv;
                    out.shorter += ol - nl;
                    found = Some(c);
                    break;
                }
                if nv >= ov {
                    break;
                }
            }
            nets[i].wires.clear();
            let mut now = match found {
                Some(c) => {
                    out.kept += 1;
                    gained = true;
                    keys[i] = board.copper_keys(nets[i].halo, &c);
                    c
                }
                None => old,
            };
            now.place(world, ctx);
            copper[j] = Some(now);
            board.occupy(&keys[i], true);
        }
        if !gained && sweep > 0 {
            break;
        }
    }
    out
}

/// A net's wires as a tree of steps.
fn tree_of(board: &Board, net: &WNet) -> Tree {
    let mut tree = Tree::default();
    let at = |p: Point, l: u8| DNode { x: p.x, y: p.y, l };
    for w in &net.wires {
        let (segs, _) = board.geometry(w);
        for (l, a, b) in segs {
            tree.edges.push((at(a, l as u8), at(b, l as u8)));
        }
        for pair in w.nodes.windows(2) {
            let (la, _, _) = board.unpack(pair[0]);
            let (lb, _, _) = board.unpack(pair[1]);
            if la != lb {
                let p = board.node_point(pair[0]);
                tree.edges.push((at(p, la as u8), at(p, lb as u8)));
            }
        }
    }
    tree
}

#[cfg(test)]
mod tests {
    use super::super::control::Run;
    use super::super::geometry::{NetCopper, Seg};
    use super::super::tests::{MM, design, smd};
    use super::super::world::World;
    use super::super::{Design, RouteOptions, RunControl, router};
    use super::{
        Board, Conflict, Ctl, FEWER_VIA, Grid, Mode, PLANE_VIA, PLANE_VIA_FIRST, POUR_MORE, Ranked,
        VIA, Wire, again, count, finer_rounds, fp, most_rounds, none_new, open_on_grid, rank,
        refined, setup, setup_on,
    };
    use crate::geom::{Point, Rect};

    #[test]
    fn a_via_of_a_net_with_a_plane_has_its_own_price_at_either_pitch() {
        // At the first pitch, what a via costs when nets are routed again for fewer of
        // them; at twice the pitch, as much track as ten steps of the first.
        assert_eq!(PLANE_VIA_FIRST, FEWER_VIA);
        assert_eq!(VIA * PLANE_VIA * 2.0, VIA);
    }

    #[test]
    fn a_negotiation_at_twice_the_pitch_has_the_rounds_of_cadlab_s_router() {
        assert_eq!(most_rounds(true, 0), 40);
        assert_eq!(most_rounds(false, 0), 100);
        // And as many more for each time the pads cut off their fills are told, at
        // either pitch.
        assert_eq!(most_rounds(true, 2), 40 + 2 * POUR_MORE);
        assert_eq!(most_rounds(false, 1), 100 + POUR_MORE);
    }

    #[test]
    fn a_net_routed_with_others_stands_if_it_is_in_no_claim_that_it_was_not_in() {
        // In order, a via's cell before the points of the layers.
        let then: Vec<Conflict> = vec![(None, 4), (Some(0), 3), (Some(0), 9), (Some(1), 2)];
        // In fewer of them, or in the same: it stands.
        assert!(none_new(&[], &then));
        assert!(none_new(&[(None, 4), (Some(0), 9)], &then));
        assert!(none_new(&then, &then));
        // In one more, on a layer or by a via: it does not.
        assert!(!none_new(&[(Some(0), 4)], &then));
        assert!(!none_new(&[(None, 5), (Some(0), 3)], &then));
        assert!(!none_new(&[(Some(1), 2)], &[]));
    }

    /// Three nets of two pads on the top of two layers: the first two each under and
    /// back up by two vias, one for nothing and one under the third, a track that goes
    /// on for 20 mm either way; `shift` moves the copper of the third off the grid.
    fn hops(shift: i64) -> (Design, Vec<NetCopper>) {
        let pads = vec![
            smd(0, 0, 1),
            smd(4 * MM, 0, 1),
            smd(0, 40 * MM, 2),
            smd(4 * MM, 40 * MM, 2),
            smd(2 * MM + shift, 20 * MM, 3),
            smd(2 * MM + shift, 60 * MM, 3),
        ];
        let d = design(2, pads, 4);
        let seg = |a: (i64, i64), b: (i64, i64), l: u8| Seg {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            l,
        };
        let hop = |y: i64| {
            let mut c = NetCopper::default();
            c.segs = vec![
                seg((0, y), (MM, y), 0),
                seg((MM, y), (3 * MM, y), 1),
                seg((3 * MM, y), (4 * MM, y), 0),
            ];
            c.vias = vec![Point::new(MM, y), Point::new(3 * MM, y)];
            c
        };
        let mut wall = NetCopper::default();
        wall.segs = vec![seg((2 * MM + shift, 20 * MM), (2 * MM + shift, 60 * MM), 0)];
        (d, vec![hop(0), hop(40 * MM), wall])
    }

    fn world_of<'a>(d: &'a Design, options: &RouteOptions) -> World<'a> {
        let mut world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )
        .unwrap();
        world.configure(d, options);
        world
    }

    #[test]
    fn a_net_routed_again_among_the_others_loses_the_hop_that_is_under_nothing() {
        for shift in [0, 37_000] {
            let (d, copper) = hops(shift);
            let options = RouteOptions::default();
            let base = world_of(&d, &options);
            let mut world = world_of(&d, &options);
            let pitch = router::pitch(&d, &options);
            let jobs = router::jobs(&d, &world, pitch, &[], &[]);
            assert_eq!(jobs.len(), 3);
            let mut copper: Vec<Option<NetCopper>> = copper.into_iter().map(Some).collect();
            for (job, c) in jobs.iter().zip(copper.iter_mut()) {
                c.as_mut().unwrap().place(&mut world, &job.ctx);
            }
            let control = RunControl::default();
            let run = Run::new(&control, Some(10_000_000));
            let bounds = Rect::new(Point::new(-10 * MM, -10 * MM), Point::new(14 * MM, 70 * MM));
            let got = again(
                &d,
                &base,
                &mut world,
                &jobs,
                &mut copper,
                &[true, true, true],
                &options,
                &run,
                (pitch, false),
                bounds,
                None,
            );
            // The hop under nothing goes, and its net is one track on the top layer.
            let free = copper[0].as_ref().unwrap();
            assert!(free.vias.is_empty(), "{shift}: {free:?}");
            assert!(free.segs.iter().all(|s| s.l == 0), "{shift}: {free:?}");
            assert_eq!(free.pieces(&jobs[0].ctx).apart(), 0);
            assert!(
                (free.length() - 4.0 * MM as f64).abs() < 1.0,
                "{shift}: {free:?}"
            );
            // The one under the third net's track stays: on the grid or off it, that
            // track claims what a track of another net may not be in.
            let under = copper[1].as_ref().unwrap();
            assert_eq!(under.vias.len(), 2, "{shift}: {under:?}");
            assert_eq!(under.pieces(&jobs[1].ctx).apart(), 0);
            assert_eq!((got.kept, got.vias), (1, 2), "{shift}: {got:?}");
            // And every net is in the world as it is kept: nothing there is in the way
            // of the copper of any of them once its own is taken out.
            for (job, c) in jobs.iter().zip(copper.iter_mut()) {
                let c = c.as_mut().unwrap();
                c.unplace(&mut world);
                let free = c.free_in(&world, &job.ctx, pitch);
                assert_eq!(
                    (free.segs.len(), free.vias.len()),
                    (c.segs.len(), c.vias.len())
                );
                c.place(&mut world, &job.ctx);
            }
        }
    }

    #[test]
    fn copper_across_another_net_is_not_free_whatever_the_count_of_its_tracks() {
        // The first net with a track across the third's, and its way back up the board
        // in two tracks that are in line: cut where the third net is and joined where it
        // is in line, it is three tracks as it was, and it is not free.
        let (d, _) = hops(0);
        let options = RouteOptions::default();
        let mut world = world_of(&d, &options);
        let pitch = router::pitch(&d, &options);
        let jobs = router::jobs(&d, &world, pitch, &[], &[]);
        let seg = |a: (i64, i64), b: (i64, i64)| Seg {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            l: 0,
        };
        let mut wall = NetCopper::default();
        wall.segs = vec![seg((2 * MM, 20 * MM), (2 * MM, 60 * MM))];
        wall.place(&mut world, &jobs[2].ctx);
        let mut c = NetCopper::default();
        c.segs = vec![
            seg((0, 30 * MM), (4 * MM, 30 * MM)),
            seg((0, 0), (0, 15 * MM)),
            seg((0, 15 * MM), (0, 30 * MM)),
        ];
        let free = c.free_in(&world, &jobs[0].ctx, pitch);
        assert_eq!(free.segs.len(), c.segs.len(), "{free:?}");
        assert!(!c.all_free(&world, &jobs[0].ctx));
        // And without the track across, all of it is.
        c.segs.remove(0);
        assert!(c.all_free(&world, &jobs[0].ctx));
    }

    #[test]
    fn a_net_that_may_not_be_routed_again_is_left_as_it_is() {
        let (d, copper) = hops(0);
        let options = RouteOptions::default();
        let base = world_of(&d, &options);
        let mut world = world_of(&d, &options);
        let pitch = router::pitch(&d, &options);
        let jobs = router::jobs(&d, &world, pitch, &[], &[]);
        let mut copper: Vec<Option<NetCopper>> = copper.into_iter().map(Some).collect();
        for (job, c) in jobs.iter().zip(copper.iter_mut()) {
            c.as_mut().unwrap().place(&mut world, &job.ctx);
        }
        let control = RunControl::default();
        let run = Run::new(&control, Some(10_000_000));
        let bounds = Rect::new(Point::new(-10 * MM, -10 * MM), Point::new(14 * MM, 70 * MM));
        let got = again(
            &d,
            &base,
            &mut world,
            &jobs,
            &mut copper,
            &[false, true, true],
            &options,
            &run,
            (pitch, false),
            bounds,
            None,
        );
        assert_eq!(copper[0].as_ref().unwrap().vias.len(), 2);
        assert_eq!((got.kept, got.vias), (0, 0), "{got:?}");
    }

    #[test]
    fn copper_off_the_grid_claims_what_a_track_between_two_points_would_be_too_near() {
        let (d, _) = hops(0);
        let options = RouteOptions::default();
        let world = world_of(&d, &options);
        let pitch = router::pitch(&d, &options);
        let jobs = router::jobs(&d, &world, pitch, &[], &[]);
        let control = RunControl::default();
        let run = Run::new(&control, Some(10_000_000));
        let bounds = Rect::new(Point::new(-10 * MM, -10 * MM), Point::new(14 * MM, 70 * MM));
        let (board, nets, _) =
            setup(&d, &world, &world, &jobs, (pitch, false, 2), bounds, &run).unwrap();
        let g = board.grid.g;
        let at = board.grid.cell(20, 20);
        let track = |a: Point, b: Point| {
            let mut c = NetCopper::default();
            c.segs = vec![Seg { a, b, l: 0 }];
            board.copper_keys(nets[0].halo, &c).len()
        };
        // Along a line of the grid, a track claims what a wire there does.
        let on = track(at, Point::new(at.x + 8 * g, at.y));
        let mut wire = Vec::new();
        board.claim_seg(
            nets[0].halo,
            0,
            fp(at),
            fp(Point::new(at.x + 8 * g, at.y)),
            false,
            &mut wire,
        );
        wire.sort_unstable();
        wire.dedup();
        assert_eq!(on, wire.len());
        // A third of a step off it, more: by what lies between two points of the grid.
        let off = track(
            Point::new(at.x, at.y + g / 3),
            Point::new(at.x + 8 * g, at.y + g / 3),
        );
        assert!(off > on, "{off} for {on}");
        assert!(board.between(400_000.0) > 0.0 && board.between(400_000.0) < g as f64 / 2.0);
        // What is claimed is given up again whole.
        let mut c = NetCopper::default();
        c.segs = vec![Seg {
            a: at,
            b: Point::new(at.x + 8 * g, at.y + g / 3),
            l: 0,
        }];
        c.vias = vec![Point::new(at.x + g / 2, at.y)];
        let keys = board.copper_keys(nets[0].halo, &c);
        board.occupy(&keys, true);
        assert!(board.occ_t.iter().flatten().any(|a| count(a) == 1));
        assert!(board.occ_v.iter().flatten().any(|a| count(a) == 1));
        board.occupy(&keys, false);
        assert!(board.occ_t.iter().flatten().all(|a| count(a) == 0));
        assert!(board.occ_v.iter().flatten().all(|a| count(a) == 0));
    }

    /// Two nets across each other on a board of two layers, on the grid of twice the
    /// pitch: the first routed where nothing is claimed, the second across it on its own
    /// layer, at no price and with a via out of reach.
    fn across() -> Design {
        let pads = vec![
            smd(0, 0, 1),
            smd(8 * MM, 0, 1),
            smd(4 * MM, -4 * MM, 2),
            smd(4 * MM, 4 * MM, 2),
        ];
        design(2, pads, 3)
    }

    #[test]
    fn a_wire_on_the_grid_twice_as_fine_is_the_same_copper() {
        let d = across();
        let options = RouteOptions::default();
        let world = world_of(&d, &options);
        let pitch = router::pitch(&d, &options);
        let jobs = router::jobs(&d, &world, pitch, &[], &[]);
        let control = RunControl::default();
        let run = Run::new(&control, Some(10_000_000));
        let bounds = Rect::new(Point::new(-6 * MM, -8 * MM), Point::new(14 * MM, 8 * MM));
        let (board, mut nets, mut sc) =
            setup(&d, &world, &world, &jobs, (pitch, true, 2), bounds, &run).unwrap();
        let ctl = Ctl {
            run: Some(&run),
            flag: run.flag(),
            limit: 1_000_000,
        };
        let via = nets[0].via;
        let (res, _) = board.route_net(
            &world,
            &mut sc,
            &mut nets[0],
            Mode::Hard,
            via,
            &ctl,
            None,
            None,
        );
        assert!(res.is_ok() && !nets[0].wires.is_empty());
        let g = board.grid;
        let fine = Grid {
            ox: g.ox,
            oy: g.oy,
            g: g.g / 2,
            w: g.dw(),
            h: g.dh(),
        };
        let (finer, _, _) = setup_on(&d, &world, &world, &jobs, fine, (true, 2), &run).unwrap();
        let length = |b: &Board, w: &Wire| -> (i64, Vec<Point>) {
            let (segs, vias) = b.geometry(w);
            let len: f64 = segs
                .iter()
                .map(|(_, a, c)| ((c.x - a.x) as f64).hypot((c.y - a.y) as f64))
                .sum();
            (len.round() as i64, vias)
        };
        for w in &nets[0].wires {
            let r = refined(&board, &finer, w);
            assert_eq!(length(&board, w), length(&finer, &r));
            assert_eq!((w.from, w.to), (r.from, r.to));
            // Between two nodes of a layer there is the one halfway.
            assert!(r.nodes.len() >= w.nodes.len());
            assert_eq!(finer.node_point(r.nodes[0]), board.node_point(w.nodes[0]));
        }
    }

    #[test]
    fn two_nets_that_share_at_twice_the_pitch_part_on_the_grid_of_the_pitch() {
        let d = across();
        let options = RouteOptions::default();
        let world = world_of(&d, &options);
        let pitch = router::pitch(&d, &options);
        let jobs = router::jobs(&d, &world, pitch, &[], &[]);
        let control = RunControl::default();
        let run = Run::new(&control, Some(50_000_000));
        let bounds = Rect::new(Point::new(-6 * MM, -8 * MM), Point::new(14 * MM, 8 * MM));
        let (board, mut nets, mut sc) =
            setup(&d, &world, &world, &jobs, (pitch, true, 2), bounds, &run).unwrap();
        let ctl = Ctl {
            run: Some(&run),
            flag: run.flag(),
            limit: 1_000_000,
        };
        // The first where nothing is claimed; the second across it, on its layer: a
        // claimed point costs it nothing, and a via more than the board is long.
        let via = nets[0].via;
        let (res, _) = board.route_net(
            &world,
            &mut sc,
            &mut nets[0],
            Mode::Hard,
            via,
            &ctl,
            None,
            None,
        );
        assert!(res.is_ok());
        board.commit(&mut nets[0]);
        let (res, _) = board.route_net(
            &world,
            &mut sc,
            &mut nets[1],
            Mode::Share(0.0),
            10_000.0,
            &ctl,
            None,
            None,
        );
        assert!(res.is_ok());
        board.commit(&mut nets[1]);
        let shared: usize = nets.iter().map(|n| board.conflicts(n).len()).sum();
        assert!(shared > 0);
        let plans = vec![None; nets.len()];
        let mut taken = vec![0u64; nets.len()];
        let all = (None, u64::MAX);
        // On a grid with a cell between every two, the same routes share as they did, and
        // the rounds on it part them: nothing is shared after, both nets are whole, and
        // one of the two went under the other.
        let f = finer_rounds(
            &d,
            &world,
            &jobs,
            &board,
            &nets,
            &plans,
            &run,
            (true, 0),
            all,
            &mut taken,
        )
        .expect("a grid to halve");
        assert_eq!(f.board.grid.g * 2, board.grid.g);
        assert!(f.rounds >= 1);
        assert_eq!((f.before, f.after), (0, 0));
        for net in &f.nets {
            assert!(f.board.conflicts(net).is_empty());
            assert_eq!(open_on_grid(net), 0);
        }
        let vias: usize = f.nets.iter().map(|n| f.board.measure(n).0).sum();
        assert!(vias >= 2);
        // What they are for: a net that was taken out and found no way where nothing is
        // claimed has a connection open, and is routed across the others at a price.
        let mut open = nets.clone();
        board.uncommit(&mut open[1]);
        open[1].wires.clear();
        board.commit(&mut open[1]);
        assert_eq!(open_on_grid(&open[1]), 1);
        let f = finer_rounds(
            &d,
            &world,
            &jobs,
            &board,
            &open,
            &plans,
            &run,
            (true, 0),
            all,
            &mut taken,
        )
        .expect("a grid to halve");
        assert!(f.rounds >= 1);
        assert_eq!((f.before, f.after), (1, 0));
        assert!(f.nets.iter().all(|n| f.board.conflicts(n).is_empty()));
        // With no node to spend there is no round, and nothing to keep.
        let none = finer_rounds(
            &d,
            &world,
            &jobs,
            &board,
            &open,
            &plans,
            &run,
            (true, 0),
            (None, 0),
            &mut taken,
        );
        assert!(none.is_none());
    }

    #[test]
    fn of_two_nodes_that_cost_the_same_the_one_nearer_the_targets_is_first() {
        let mut heap = Ranked::default();
        heap.push(rank(10.0, 4.0), 1);
        heap.push(rank(10.0, 1.0), 2);
        heap.push(rank(9.0, 8.0), 3);
        // Within a sixteenth of a step, the costs are the same.
        heap.push(rank(10.03, 0.5), 4);
        let order: Vec<u32> = core::iter::from_fn(|| heap.pop().map(|e| e.1)).collect();
        assert_eq!(order, [3, 4, 2, 1]);
    }
}
