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
//! The grid is only where congestion is priced. Whether the board's own copper lets a
//! track or a via be somewhere is asked of the [`World`], exactly, once for each place
//! that a search reaches, and what the negotiation ends with is given to the router as
//! copper of its nets, which keeps of it what the world says is legal beside the rest and
//! goes on from there.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering};
use std::sync::{Mutex, RwLock, mpsc};

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
use crate::space::FastMap;

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

/// The price of a claimed point at first, what it is multiplied by after each round, and
/// the most it is: thirty times a step, at which a net goes thirty steps round a point
/// rather than share it. Past that a search only goes further for it, and what is still
/// shared is the history's to price, which rises for as long as a point is shared: at
/// 1000, as VPR has it and cadlab's router, `corne-cherry` ended a minute with 559 vias
/// and 8276 mm of track over four orders where it has 498 and 7621, for as many
/// connections, and the nine boards that 40 million nodes cut had 620 connections open
/// for 621 (`docs/benchmarks.md`).
const PRES: f32 = 0.6;
const PRES_STEP: f32 = 1.6;
const PRES_MAX: f32 = 30.0;
/// What a round adds to the price of a point that was shared in it.
const HISTORY: f32 = 0.5;
/// How many rounds there are at most, and how many may pass since the best.
const ROUNDS: usize = 100;
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

/// The most nets routed side by side against the same claims.
const BATCH: usize = 64;
/// How far round its pads a net's region reaches, for telling the nets that may be routed
/// side by side: this, and a quarter of its extent.
const REGION: i64 = 1_500_000;

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
    owners: RwLock<FastMap<u64, u32>>,
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
    /// The box round its pads that tells which nets may be routed side by side.
    region: Rect,
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

/// A search's own memory.
struct Scratch {
    cost: Vec<f32>,
    parent: Vec<u32>,
    dir: Vec<u8>,
    seen: Vec<u32>,
    target: Vec<u32>,
    stamp: u32,
    heap: Heap,
    own_vias: Vec<(i32, i32)>,
    near: Vec<EntryId>,
    /// What the nodes nearest a search's targets cost to them, by stamp.
    lb_cost: Vec<f32>,
    lb_seen: Vec<u32>,
    lb_stamp: u32,
    lb_heap: Heap,
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
    occ_t: Vec<Vec<u16>>,
    /// By class of halo and cell, how many nets claim it against a via.
    occ_v: Vec<Vec<u16>>,
    hist_t: Vec<f32>,
    hist_v: Vec<f32>,
    /// By layer and cell, the net that a plane or a spoke's room there is of, plus one.
    plane: Vec<u32>,
    spoke: Vec<u32>,
    plane_cost: f32,
    spoke_cost: f32,
    /// By layer, the direction its tracks run along: 0 across, 1 up, 2 either.
    along: Vec<u8>,
    hole_clearance: i64,
    hole_to_hole: i64,
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

    fn unpack(&self, n: u32) -> (usize, i32, i32) {
        let cells = self.grid.cells();
        let (l, c) = (n as usize / cells, n as usize % cells);
        (
            l,
            (c % self.grid.w as usize) as i32,
            (c / self.grid.w as usize) as i32,
        )
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
        if !owners.is_empty()
            && let Ok(mut map) = group.owners.write()
        {
            map.extend(owners);
        }
        let (blocks_w, blocks) = self.blocks();
        group.filled[layer * blocks + (by * blocks_w + bx) as usize].store(1, Ordering::Release);
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

    fn owned(&self, gi: usize, key: u64, net: NetId) -> bool {
        self.groups[gi]
            .owners
            .read()
            .is_ok_and(|m| m.get(&key) == Some(&net.0))
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
                    if let Ok(mut map) = group.owners.write() {
                        map.insert(owner_key(2, cell, 0), n.0);
                    }
                    3
                }
            };
            group.vias[cell].store(c, Ordering::Release);
        }
        match c {
            1 => true,
            3 => self.owned(gi, owner_key(2, cell, 0), net),
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

    /// The keys that a piece of track of a net claims.
    fn claim_seg(
        &self,
        halo: usize,
        layer: usize,
        a: (f64, f64),
        b: (f64, f64),
        out: &mut Vec<u64>,
    ) {
        let mine = self.halos[halo];
        let (min, max) = ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1)));
        let dn = self.grid.dpoints();
        for (hi, theirs) in self.halos.iter().enumerate() {
            let r = self.track_track(&mine, theirs) - 1.0;
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
    fn claim_via(&self, halo: usize, at: (f64, f64), out: &mut Vec<u64>) {
        let mine = self.halos[halo];
        let dn = self.grid.dpoints();
        for (hi, theirs) in self.halos.iter().enumerate() {
            if let Some(r) = self.track_via(theirs, &mine) {
                let r = r - 1.0;
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
    fn commit(&mut self, net: &mut WNet) {
        let mut keys = Vec::new();
        let mut bbox = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for w in &net.wires {
            let (segs, vias) = self.geometry(w);
            for (l, a, b) in segs {
                self.claim_seg(net.halo, l, fp(a), fp(b), &mut keys);
            }
            for v in vias {
                self.claim_via(net.halo, fp(v), &mut keys);
            }
            if w.nodes.len() == 1 {
                let (l, _, _) = self.unpack(w.nodes[0]);
                let p = fp(self.node_point(w.nodes[0]));
                self.claim_seg(net.halo, l, p, p, &mut keys);
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
            if map == self.layers {
                self.occ_v[hi][idx] = self.occ_v[hi][idx].saturating_add(1);
            } else {
                self.occ_t[hi][idx] = self.occ_t[hi][idx].saturating_add(1);
            }
        }
        net.keys = keys;
        net.bbox = bbox;
    }

    /// Gives up a net's claims.
    fn uncommit(&mut self, net: &mut WNet) {
        for &k in &net.keys {
            let (m, idx) = ((k >> 32) as usize, (k & 0xFFFF_FFFF) as usize);
            let (hi, map) = (m / (self.layers + 1), m % (self.layers + 1));
            if map == self.layers {
                self.occ_v[hi][idx] = self.occ_v[hi][idx].saturating_sub(1);
            } else {
                self.occ_t[hi][idx] = self.occ_t[hi][idx].saturating_sub(1);
            }
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
                    if occ[l * dn + idx] > 1 {
                        out.push((Some(l as u8), idx as u32));
                    }
                    continue;
                }
                let p = w.nodes[k - 1];
                if p as usize / cells != l {
                    let idx = self.grid.cidx(x, y);
                    if self.occ_v[net.halo][idx] > 1 {
                        out.push((None, idx as u32));
                    }
                    continue;
                }
                let (_, px, py) = self.unpack(p);
                for (i, j) in [(px + x, py + y), (2 * x, 2 * y)] {
                    let idx = self.grid.didx(i, j);
                    if occ[l * dn + idx] > 1 {
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

    fn add_history(&mut self, c: Conflict, amount: f32) {
        match c.0 {
            Some(l) => self.hist_t[usize::from(l) * self.grid.dpoints() + c.1 as usize] += amount,
            None => self.hist_v[c.1 as usize] += amount,
        }
    }

    // ---- searching ----

    fn scratch(&self) -> Scratch {
        let n = self.layers * self.grid.cells();
        Scratch {
            cost: vec![0.0; n],
            parent: vec![0; n],
            dir: vec![NONE; n],
            seen: vec![0; n],
            target: vec![0; n],
            stamp: 0,
            heap: Heap::default(),
            own_vias: Vec::new(),
            near: Vec::new(),
            lb_cost: vec![0.0; n],
            lb_seen: vec![0; n],
            lb_stamp: 0,
            lb_heap: Heap::default(),
        }
    }

    /// Whether the targets of a search are walled in by the board's own copper, with no
    /// source among them: everything that a target can be reached from, whatever other
    /// nets claim, is looked at backwards from the targets, [`POCKET`] nodes at most. A
    /// search for what has no way would look at everything that the sources reach before
    /// it said so, which on a board of several layers is most of it.
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
        let has_via = self.groups[gi].via.is_some() && self.layers > 1;
        // The sources are marked in the same map, by a cost that no node has.
        for &(n, _) in sources {
            sc.lb_seen[n as usize] = stamp;
            sc.lb_cost[n as usize] = -1.0;
        }
        let mut stack: Vec<u32> = Vec::new();
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
            stack.push(t);
        }
        let mut seen = 0usize;
        while let Some(v) = stack.pop() {
            seen += 1;
            if seen > POCKET {
                return false;
            }
            let vi = v as usize;
            let (l, x, y) = self.unpack(v);
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
                stack.push(ui as u32);
            }
            if has_via && self.via_ok(world, gi, id, x, y) {
                let cell = grid.cidx(x, y);
                for l2 in (0..self.layers).filter(|&l2| l2 != l) {
                    let ui = l2 * cells + cell;
                    if sc.lb_seen[ui] == stamp {
                        if sc.lb_cost[ui] < 0.0 {
                            return false;
                        }
                        continue;
                    }
                    sc.lb_seen[ui] = stamp;
                    sc.lb_cost[ui] = 0.0;
                    stack.push(ui as u32);
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
                let occ = f32::from(occ_l[km] + occ_l[kq]);
                let c = d + (base[l][nd] + hist_l[km] + hist_l[kq]) * (1.0 + pres * occ);
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
                let occ = f32::from(self.occ_v[hi][cell]);
                let c = d + (via_cost + self.hist_v[cell]) * (1.0 + pres * occ);
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
    ) -> (Result<Option<Vec<u32>>, Stop>, u64) {
        if sources.is_empty() || targets.is_empty() {
            return (Ok(None), 0);
        }
        sc.stamp = sc.stamp.wrapping_add(1);
        if sc.stamp == 0 {
            sc.seen.iter_mut().for_each(|v| *v = 0);
            sc.target.iter_mut().for_each(|v| *v = 0);
            sc.stamp = 1;
        }
        let stamp = sc.stamp;
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
            sc.target[t as usize] = stamp;
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
        let own_min = self.groups[gi]
            .via
            .map_or(0.0, |(_, drill)| (drill + self.hole_to_hole) as f64 - 1.0);
        sc.heap.clear();
        for &(n, c0) in sources {
            let (l, x, y) = self.unpack(n);
            let k = l * dn + grid.didx(2 * x, 2 * y);
            let occ = f32::from(self.occ_t[hi][k]);
            if hard && occ > 0.0 {
                continue;
            }
            let c = c0 + self.hist_t[k] + pres * occ;
            let i = n as usize;
            if sc.seen[i] == stamp && sc.cost[i] <= c {
                continue;
            }
            sc.seen[i] = stamp;
            sc.cost[i] = c;
            sc.parent[i] = u32::MAX;
            sc.dir[i] = NONE;
            let h = bound(sc, floor, i, l, x, y);
            sc.heap.push(c + h, n);
        }
        let mid_off: [isize; 8] = DIRS.map(|(dx, dy)| dy as isize * dw + dx as isize);
        let cell_off: [isize; 8] = DIRS.map(|(dx, dy)| dy as isize * gw as isize + dx as isize);
        let base_cost = self.base_costs();
        let bend_cost = [0.0, BEND45, BEND90];
        let tolls = !self.plane.is_empty() || !self.spoke.is_empty();
        let mut pops = 0u64;
        let mut found = None;
        let mut stopped = false;
        while let Some((f, u)) = sc.heap.pop() {
            let ui = u as usize;
            let (l, x, y) = self.unpack(u);
            let gu = sc.cost[ui];
            if f > gu + bound(sc, floor, ui, l, x, y) + 1e-3 {
                continue;
            }
            if sc.target[ui] == stamp {
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
            let du = sc.dir[ui];
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
                let km = (k0 + mid_off[nd]) as usize;
                let kq = (k0 + 2 * mid_off[nd]) as usize;
                let occ = f32::from(occ_l[km] + occ_l[kq]);
                if hard && occ > 0.0 {
                    continue;
                }
                let vi = (ui as isize + cell_off[nd]) as usize;
                let mut base = base_cost[l][nd];
                if tolls {
                    if let Some(&p) = self.plane.get(vi)
                        && p != 0
                        && p != own
                    {
                        base *= 1.0 + self.plane_cost;
                    }
                    if let Some(&p) = self.spoke.get(vi)
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
                }
                let hist = hist_l[km] + hist_l[kq];
                let c = gu + (base + hist) * (1.0 + pres * occ) + bend_cost[t as usize];
                if sc.seen[vi] == stamp && sc.cost[vi] <= c {
                    continue;
                }
                sc.seen[vi] = stamp;
                sc.cost[vi] = c;
                sc.parent[vi] = u;
                sc.dir[vi] = nd as u8;
                let (dx, dy) = DIRS[nd];
                let h = bound(sc, floor, vi, l, x + dx, y + dy);
                sc.heap.push(c + h, vi as u32);
            }
            if has_via {
                let cell = grid.cidx(x, y);
                let occ = f32::from(self.occ_v[hi][cell]);
                if hard && occ > 0.0 {
                    continue;
                }
                if !self.via_ok(world, gi, id, x, y) {
                    continue;
                }
                // Hole to hole from the vias of the net's own wires.
                if sc.own_vias.iter().any(|&(vx, vy)| {
                    let (dx, dy) = (f64::from(vx - x), f64::from(vy - y));
                    (vx, vy) != (x, y) && (dx * dx + dy * dy).sqrt() * (grid.g as f64) < own_min
                }) {
                    continue;
                }
                let c0 = gu + (via_cost + self.hist_v[cell]) * (1.0 + pres * occ);
                for l2 in 0..ns {
                    if l2 == l {
                        continue;
                    }
                    let v = (l2 * cells + cell) as u32;
                    let vi = v as usize;
                    if sc.seen[vi] == stamp && sc.cost[vi] <= c0 {
                        continue;
                    }
                    if !self.point_ok(world, gi, id, v, &mut sc.near) {
                        continue;
                    }
                    sc.seen[vi] = stamp;
                    sc.cost[vi] = c0;
                    sc.parent[vi] = u;
                    sc.dir[vi] = NONE;
                    let h = bound(sc, floor, vi, l2, x, y);
                    sc.heap.push(c0 + h, v);
                }
            }
        }
        if stopped {
            return (Err(Stop), pops);
        }
        let path = found.map(|u| {
            let mut path = vec![u];
            let mut c = u;
            while sc.parent[c as usize] != u32::MAX {
                c = sc.parent[c as usize];
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
                    if self.occ_t[net.halo][l * dn + self.grid.didx(2 * x, 2 * y)] > 0 {
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
    ) -> (Result<(), Stop>, u64) {
        let old = core::mem::take(&mut net.wires);
        sc.own_vias.clear();
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
                        sc.own_vias.push((x, y));
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
                run: ctl.run,
                flag: ctl.flag,
                limit: ctl.limit.saturating_sub(expanded),
            };
            let (found, pops) =
                self.search(world, sc, net, &sources, &targets, mode, via_cost, &left);
            expanded += pops;
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
                    sc.own_vias.push((x, y));
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
    /// Whether the rounds ended on their own, and not with their share.
    pub(crate) ended: bool,
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

/// Negotiates for every net of `jobs` at once, and returns what each ends with.
///
/// `make_world` makes a world as `world` is for a thread of its own to ask, with no run:
/// `threads` of them route the nets of a batch, whose regions are apart, side by side
/// against the same claims, and the batches are told by the nets alone, so that what
/// comes out is the same for any number of threads.
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
    (pitch, coarse): (i64, bool),
    bounds: Rect,
) -> Result<Whole, NoWhole> {
    let rules = &design.rules;
    let layers = rules.layer_count();
    if jobs.is_empty() || layers == 0 {
        return Err(NoWhole::Not);
    }
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
    let grid = make_grid(design, jobs, pitch, bounds);
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
        let mut region: Option<Rect> = None;
        for p in &ctx.pads {
            let r = Rect::new(p.center, p.center);
            boxes[p.terminal] = if any[p.terminal] {
                boxes[p.terminal].union(&r)
            } else {
                r
            };
            any[p.terminal] = true;
            region = Some(region.map_or(r, |q| q.union(&r)));
        }
        let region = region.unwrap_or_default();
        let extent = (region.max().x - region.min().x).max(region.max().y - region.min().y);
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
            region: region.expand(REGION + extent / 4),
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
            owners: RwLock::new(FastMap::default()),
        })
        .collect();
    let dn = grid.dpoints();
    let mut board = Board {
        grid,
        layers,
        occ_t: vec![vec![0u16; layers * dn]; halos.len()],
        occ_v: vec![vec![0u16; cells]; halos.len()],
        halos,
        groups,
        hist_t: vec![0.0; layers * dn],
        hist_v: vec![0.0; cells],
        plane: Vec::new(),
        spoke: Vec::new(),
        plane_cost: 0.0,
        spoke_cost: 0.0,
        along: (0..layers)
            .map(|l| if layers < 2 { 2 } else { (l % 2) as u8 })
            .collect(),
        hole_clearance: rules.spec().hole_clearance,
        hole_to_hole: rules.spec().hole_to_hole,
    };
    // Where planes and the rooms of spokes are.
    {
        let (mut plane, mut spoke) = (Vec::new(), Vec::new());
        let (mut any_plane, mut any_spoke) = (false, false);
        for l in 0..layers {
            for y in 0..grid.h {
                for x in 0..grid.w {
                    let at = grid.cell(x, y);
                    let (p, pc) = world.plane_toll(l as u8, at);
                    let (s, scost) = world.spoke_toll(l as u8, at);
                    any_plane |= p != 0;
                    any_spoke |= s != 0;
                    board.plane_cost = pc as f32;
                    board.spoke_cost = scost as f32;
                    plane.push(p);
                    spoke.push(s);
                }
            }
            if run.poll() {
                return Err(NoWhole::Not);
            }
        }
        if any_plane {
            board.plane = plane;
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
    let mut pour_rounds = 0;
    let mut pour_cuts = 0;
    let mut refills = 0;
    let mut sites: Vec<(usize, Vec<TermPad>)> = Vec::new();
    // What the first round took, in time and in nodes, once it has ended.
    let mut first_round: Option<(std::time::Duration, u64)> = None;
    let threads = options.threads.max(1);
    let lock = RwLock::new(board);
    #[allow(clippy::type_complexity)]
    let queue: Mutex<(Vec<(usize, WNet, Mode, Option<Vec<bool>>)>, bool)> =
        Mutex::new((Vec::new(), false));
    let wake = std::sync::Condvar::new();
    // The nodes that a search of the batch under way may expand.
    let limit = std::sync::atomic::AtomicU64::new(u64::MAX);
    let flag = run.flag();
    let (done_tx, done_rx) = mpsc::channel::<(usize, WNet, bool, u64)>();
    macro_rules! read {
        () => {
            lock.read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
    }
    macro_rules! write {
        () => {
            lock.write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
    }
    std::thread::scope(|scope| {
        for _ in 1..threads {
            let done_tx = done_tx.clone();
            let (lock, queue, wake, limit) = (&lock, &queue, &wake, &limit);
            scope.spawn(move || {
                let Some(world) = make_world() else { return };
                let mut sc = lock
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .scratch();
                loop {
                    let task = {
                        let mut q = queue
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        loop {
                            if let Some(t) = q.0.pop() {
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
                    let Some((i, mut net, mode, keep)) = task else {
                        break;
                    };
                    let old = net.wires.clone();
                    let ctl = Ctl {
                        run: None,
                        flag,
                        limit: limit.load(Ordering::Relaxed),
                    };
                    let (res, pops) = lock
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .route_net(&world, &mut sc, &mut net, mode, VIA, &ctl, keep.as_deref());
                    if res.is_err() {
                        net.wires = old;
                    }
                    if done_tx.send((i, net, res.is_err(), pops)).is_err() {
                        break;
                    }
                }
            });
        }
        // Routes a batch of nets against the claims as they are without any of them, and
        // claims for each what it ends with. Says whether it was stopped.
        let run_batch = |nets: &mut Vec<WNet>,
                         sc: &mut Scratch,
                         batch: &[usize],
                         mode: Mode,
                         partial: bool| {
            // Of each net, the wires that are in no other net's way are kept.
            let mut keeps: Vec<Option<Vec<bool>>> = Vec::new();
            {
                let mut board = write!();
                for &i in batch {
                    keeps.push(partial.then(|| {
                        nets[i]
                            .wires
                            .iter()
                            .map(|w| {
                                let mut c = Vec::new();
                                board.wire_conflicts(&nets[i], w, &mut c);
                                c.is_empty()
                            })
                            .collect()
                    }));
                }
                for &i in batch {
                    board.uncommit(&mut nets[i]);
                }
            }
            limit.store(run.limits(u64::MAX).max_expansions, Ordering::Relaxed);
            {
                let mut q = queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Taken from the end: the first of the batch first.
                for (&i, keep) in batch.iter().zip(keeps).rev() {
                    q.0.push((
                        i,
                        core::mem::replace(&mut nets[i], placeholder()),
                        mode,
                        keep,
                    ));
                }
            }
            wake.notify_all();
            let mut left = batch.len();
            let mut stopped = false;
            let mut expanded = 0u64;
            while left > 0 {
                let task = queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0
                    .pop();
                if let Some((i, mut net, mode, keep)) = task {
                    let old = net.wires.clone();
                    let ctl = Ctl {
                        run: Some(run),
                        flag,
                        limit: limit.load(Ordering::Relaxed),
                    };
                    let (res, pops) =
                        read!().route_net(world, sc, &mut net, mode, VIA, &ctl, keep.as_deref());
                    if res.is_err() {
                        net.wires = old;
                        stopped = true;
                    }
                    expanded += pops;
                    nets[i] = net;
                    left -= 1;
                    continue;
                }
                match done_rx.recv_timeout(std::time::Duration::from_millis(5)) {
                    Ok((i, net, failed, pops)) => {
                        nets[i] = net;
                        stopped |= failed;
                        expanded += pops;
                        left -= 1;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        run.poll();
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            let mut board = write!();
            for &i in batch {
                board.commit(&mut nets[i]);
            }
            drop(board);
            run.charge(expanded);
            stopped || run.poll()
        };
        let began = (std::time::Instant::now(), run.work());
        let total_length: i64 = nets.iter().map(|n| n.length).sum();
        let mut sampled = false;
        let mut length_done = 0i64;
        // By net, the box that tells which nets are routed side by side: round its pads
        // at first, and then round its wires that are routed again.
        let mut regions: Vec<Rect> = nets.iter().map(|n| n.region).collect();
        let connections: usize = nets.iter().map(|n| n.conns.len()).sum();
        let mut extended = false;
        let mut it = 0;
        let mut round_began = began;
        // Each time the pads cut off are told, the rounds have as many more for them.
        while it < ROUNDS + POUR_MORE * pour_rounds {
            it += 1;
            let it = it - 1;
            rounds = it + 1;
            let mut todo = batches(&reroute, &regions);
            todo.reverse();
            while let Some(batch) = todo.pop() {
                // A batch that is stopped is as it was before it.
                let before: Vec<Vec<Wire>> = batch.iter().map(|&i| nets[i].wires.clone()).collect();
                if run_batch(&mut nets, &mut sc, &batch, Mode::Share(pres), it > 0) {
                    let mut board = write!();
                    for (&i, w) in batch.iter().zip(before) {
                        board.uncommit(&mut nets[i]);
                        nets[i].wires = w;
                        board.commit(&mut nets[i]);
                    }
                    // Near its end, it goes on once.
                    let shared: usize = nets.iter().map(|n| board.conflicts(n).len()).sum();
                    drop(board);
                    if !extended
                        && it > 0
                        && run.stopped().is_none()
                        && shared * NEARLY < connections
                    {
                        extended = true;
                        run.extend(share, share, was);
                        if !run.ending() {
                            todo.push(batch);
                            continue;
                        }
                    }
                    cut = true;
                    break;
                }
                // The first round, once it has routed a share of the nets' length, says
                // what the whole negotiation takes: with less than that left, it is given
                // up here.
                if it == 0 && !sampled {
                    length_done += batch.iter().map(|&i| nets[i].length).sum::<i64>();
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
                        let short_of_time = run
                            .part_left()
                            .is_some_and(|left| factor * took / share > left.as_secs_f64() + took);
                        if short_of_work || short_of_time {
                            given_up = true;
                            cut = true;
                            break;
                        }
                    }
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
            let board = read!();
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
            if total == 0 || cut || it - best_at >= STALE {
                // The pads that the routes as they are cut off their fills: their nets
                // are planned again with them, and the rounds go on from the prices as
                // they are, for the nets round them to make room.
                let mut changed: Vec<usize> = Vec::new();
                if let Some(pours) = pours.filter(|_| !cut && pour_rounds < POUR_ROUNDS) {
                    pour_rounds += 1;
                    let copper: Vec<Option<NetCopper>> = nets
                        .iter()
                        .map(|n| {
                            (!n.wires.is_empty()).then(|| {
                                NetCopper::from_tree(&jobs[n.job].ctx, &tree_of(&board, n))
                            })
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
                        let ctx = &again.ctx;
                        let net = &mut nets[k];
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
                drop(board);
                reroute = order
                    .iter()
                    .copied()
                    .filter(|i| changed.contains(i))
                    .collect();
                for &i in &reroute {
                    regions[i] = nets[i].region;
                }
                best = None;
                best_at = it;
                continue;
            }
            let mut hot: Vec<(i32, i32)> = points
                .iter()
                .map(|(c, _)| board.conflict_cell(*c))
                .collect();
            drop(board);
            {
                let mut board = write!();
                for (c, _) in &points {
                    board.add_history(*c, HISTORY);
                }
            }
            pres = (pres * PRES_STEP).min(PRES_MAX);
            // The nets with something shared, and those whose routes are near it.
            let mut set: BTreeSet<usize> = BTreeSet::new();
            hot.sort_unstable();
            hot.dedup();
            for (_, i) in &points {
                set.insert(*i);
            }
            let _ = &hot;
            reroute = order.iter().copied().filter(|i| set.contains(i)).collect();
            // Their regions are round the wires that are in another net's way.
            {
                let board = read!();
                for &i in &reroute {
                    let net = &nets[i];
                    let mut r: Option<Rect> = None;
                    for w in &net.wires {
                        let mut c = Vec::new();
                        board.wire_conflicts(net, w, &mut c);
                        if c.is_empty() {
                            continue;
                        }
                        for &n in [w.nodes.first(), w.nodes.last()].into_iter().flatten() {
                            let p = board.node_point(n);
                            let q = Rect::new(p, p);
                            r = Some(r.map_or(q, |o| o.union(&q)));
                        }
                    }
                    regions[i] = match r {
                        Some(r) => {
                            let extent = (r.max().x - r.min().x).max(r.max().y - r.min().y);
                            r.expand(REGION + extent / 4)
                        }
                        None => net.region,
                    };
                }
            }
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
    let mut board = lock
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Back to the best round.
    let mut shared = 0;
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
    // What is still shared is taken out, the net with the most first, and routed again
    // where nothing is claimed.
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
    fn ctl<'a, 'r>(run: &'a Run<'r>) -> Ctl<'a, 'r> {
        Ctl {
            run: Some(run),
            flag: run.flag(),
            limit: run.limits(u64::MAX).max_expansions,
        }
    }
    for i in out {
        if run.ending() {
            break;
        }
        let before = nets[i].wires.clone();
        let all = vec![true; before.len()];
        board.uncommit(&mut nets[i]);
        let (res, pops) = board.route_net(
            world,
            &mut sc,
            &mut nets[i],
            Mode::Hard,
            VIA,
            &ctl(run),
            Some(&all),
        );
        if res.is_err() {
            nets[i].wires = before;
        }
        board.commit(&mut nets[i]);
        run.charge(pops);
        if run.poll() {
            break;
        }
    }
    // Fewer vias: each net with any again, with vias dear, kept if it has fewer and is
    // little longer. After a negotiation that its share ended, the nets with the most
    // first: the time may end before all of them have had their turn, and those are
    // the ones with the most to lose.
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
        let (res, pops) =
            board.route_net(world, &mut sc, net, Mode::Hard, FEWER_VIA, &ctl(run), None);
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
        ended: !cut,
    })
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

/// The nets of `order` in batches of nets whose regions are apart: a net is in the first
/// batch after every batch with an earlier net whose region meets its own, so that nets
/// that may be in each other's way keep their order. By the nets alone, whatever routes
/// them.
fn batches(order: &[usize], regions: &[Rect]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut level: Vec<(usize, usize)> = Vec::new();
    for &i in order {
        let r = &regions[i];
        let mut b = level
            .iter()
            .filter(|(o, _)| regions[*o].intersects(r))
            .map(|(_, b)| b + 1)
            .max()
            .unwrap_or(0);
        while b < out.len() && out[b].len() >= BATCH {
            b += 1;
        }
        if b == out.len() {
            out.push(Vec::new());
        }
        out[b].push(i);
        level.push((i, b));
    }
    out
}

/// A net that stands in for one that is being routed.
fn placeholder() -> WNet {
    WNet {
        job: 0,
        net: NetId(0),
        halo: 0,
        group: 0,
        terms: Vec::new(),
        boxes: Vec::new(),
        conns: Vec::new(),
        dead: Vec::new(),
        looked: Vec::new(),
        wires: Vec::new(),
        keys: Vec::new(),
        bbox: (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
        length: 0,
        region: Rect::default(),
    }
}
