//! Placing a board's components: where each movable part goes on its side of the board, and
//! at which quarter turn, so that its nets are short, its decoupling capacitors are near the
//! pins they serve, and the board still routes.
//!
//! A [`PlacementDesign`] is the board as placement sees it: its edge, keepouts, and
//! [`Component`]s, each with its outline, such as a courtyard's bounding box, the pins of its
//! nets, its side, whether it goes through the board, and whether it is fixed.
//! [`place_components`] turns that into problems for the generic placers of
//! [`crate::place`] and back. `kicad::placement_design`, with the `kicad` feature, builds the
//! design from a KiCad board, deciding what is fixed, and `kicad::replace_placement` writes the
//! result back.
//!
//! # The free region
//!
//! The board is rasterized on a grid of [`PlaceOptions::grid`]: a cell is blocked on a side if
//! it is outside the edge, within the edge clearance of it, or under a fixed component on that
//! side, a keepout, or, with [`PlaceOptions::shadow`], a fixed area array on the other side,
//! such as a BGA, whose fan-out vias need the board under it. Blocked cells are merged into
//! rectangles, which become fixed items of the problem. The rest is free space.
//!
//! # Sides
//!
//! Components keep the side they are on. Those that go through the board, with a hole in any
//! pad, take room on both sides. The placers of [`crate::place`] know one plane, so a board
//! is placed in passes, each on the side whose items are real:
//!
//! 0. components through the board, against what is blocked on either side, with everything
//!    else movable but taking no room, so that it pulls them where its nets want;
//! 1. the front's components, with those through the board now fixed, and the back's movable
//!    but taking no room;
//! 2. the back's components, legalized from where the first pass put them, with the front's
//!    fixed where it put them;
//! 3. the front again, from where it was, with the back's fixed.
//!
//! Each pass is [`place_analytic`], or legalization from the
//! last, then annealing at a low temperature with the routing term below.
//!
//! # Costs
//!
//! Each net is weighted by what it is: nets that a copper zone connects cost nothing, as they
//! go to their plane by a via; nets of many pins weigh a quarter of others. To these, as nets
//! of the problem with a [`CostTerm`] of their own:
//!
//! - **Decoupling.** A capacitor whose two nets are a supply and a ground, as their names say,
//!   such as `+3V3` and `GND`, is tied to a power pin of an integrated circuit on the same
//!   supply, the capacitors of a supply spread over its power pins in order, by a net of
//!   [`PlaceOptions::decoupling_weight`] that costs four times as much again beyond
//!   [`PlaceOptions::decoupling_reach`].
//! - **Thermal grouping**, optional: each group of [`PlaceOptions::thermal`] is a net between
//!   the centres of its components.
//! - **Routability.** After the first placement, a RUDY map of the board's nets, against what
//!   the board's layers can carry, prices nets that cross congested areas, as
//!   [`CongestionCost`] does, in the annealing that follows. Outlines are also grown by
//!   [`PlaceOptions::spacing`], and those of parts with many pins by
//!   [`PlaceOptions::fanout`] more, so that tracks have room between parts; if that leaves no
//!   room, it is taken down by quarters, to none.
//!
//! Everything is deterministic for a seed, as the placers are.

use std::collections::BTreeMap;

use crate::geom::{Point, Rect, Shape};
use crate::id::NetId;
use crate::place::{
    self, AnalyticOptions, AnnealOptions, Annealer, CongestionCost, CongestionGrid, CostTerm,
    ItemId, Layout, NetView, Pin, PlaceError, Placement, PlacementProblem, place_analytic,
};

type PPoint = place::Point;
type PRect = place::Rect;

/// A quarter turn, in millidegrees.
const QUARTER: i32 = 90_000;

/// A side of the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    /// The top, KiCad's `F.Cu`.
    Front,
    /// The bottom, KiCad's `B.Cu`.
    Back,
}

/// A pin of a [`Component`]: where it is now, and its net.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentPin {
    /// Where it is on the board, with the component where it is now.
    pub at: Point,
    /// Its net.
    pub net: NetId,
}

/// A part to place, or a fixed one to place around.
///
/// Its geometry is as it is now: at [`at`](Component::at), turned by
/// [`rotation`](Component::rotation). Placement keeps the rotation's part that is not a whole
/// number of quarter turns, so a part at 45° stays at 45° plus quarter turns.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Component {
    /// Its name, a reference such as `U1`. A name starting with `C` marks a capacitor, and
    /// one starting with `U` or `IC` an integrated circuit, for decoupling.
    pub name: String,
    /// Where its origin is now.
    pub at: Point,
    /// Its rotation now, in millidegrees counterclockwise.
    pub rotation: i32,
    /// Its side.
    pub side: Side,
    /// Whether it goes through the board, and so takes room on both sides.
    pub through: bool,
    /// Whether it stays where it is.
    pub fixed: bool,
    /// The room it takes, on the board as it is now: its courtyard's bounding box. `None`
    /// for something that takes none, such as a logo in silkscreen.
    pub outline: Option<Rect>,
    /// The pins that have a net.
    pub pins: Vec<ComponentPin>,
}

impl Component {
    /// A movable component on a side, with no outline and no pins.
    pub fn new(name: impl Into<String>, at: Point, rotation: i32, side: Side) -> Component {
        Component {
            name: name.into(),
            at,
            rotation,
            side,
            through: false,
            fixed: false,
            outline: None,
            pins: Vec::new(),
        }
    }
}

/// A board as placement sees it.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct PlacementDesign {
    /// The components, fixed and movable.
    pub components: Vec<Component>,
    /// The board's edge, as lines: parts stay inside it.
    pub outline: Vec<Shape>,
    /// How far parts' outlines stay from the edge's lines.
    pub edge_clearance: i64,
    /// Areas that parts keep out of, on one side, or both for `None`.
    pub keepouts: Vec<(Rect, Option<Side>)>,
    /// The name of each net, by [`NetId`], which says what is a supply or a ground.
    pub nets: Vec<String>,
    /// Nets that copper zones connect, each with the bounding box of a zone of it. A pin of
    /// such a net is drawn into its zones' box, rather than to the net's other pins, as it
    /// connects to the zone.
    pub planes: Vec<(NetId, Rect)>,
    /// How many copper layers the board has, for what it can route.
    pub layers: usize,
    /// A track's width plus its clearance, typical of the board, for what it can route.
    pub track_pitch: i64,
    /// The largest clearance between copper of different nets: half of it is kept around
    /// every movable part's outline, however little room there is.
    pub clearance: i64,
}

/// How [`place_components`] places.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PlaceOptions {
    /// The seed of the placers. The default is 1.
    pub seed: u64,
    /// The step that parts' origins are placed on. The default is 50 µm.
    pub pitch: i64,
    /// The cells of the free region. The default is 100 µm.
    pub grid: i64,
    /// How much every movable part's outline is grown on each side, for tracks and zones'
    /// thermal spokes between parts, and how much further than the edge clearance parts keep
    /// from the board's edge. The default is 0.25 mm.
    pub spacing: i64,
    /// How much more a part of at least [`fanout_pins`](PlaceOptions::fanout_pins) pins is
    /// grown, for its fan-out. The default is 0.35 mm.
    pub fanout: i64,
    /// See [`fanout`](PlaceOptions::fanout). The default is 8.
    pub fanout_pins: usize,
    /// Whether parts may turn by quarter turns. The default is true.
    pub rotate: bool,
    /// The weight of a decoupling capacitor's tie to its power pin, against 4 for a net of a
    /// few pins. The default is 8.
    pub decoupling_weight: u32,
    /// How far a decoupling capacitor's pin may be from its power pin, as half the perimeter
    /// of the box of the two, before it costs four times more. The default is 2 mm.
    pub decoupling_reach: i64,
    /// Groups of components, by index, to keep together, for heat. The default is none.
    pub thermal: Vec<Vec<usize>>,
    /// The weight of a thermal group's net. The default is 4.
    pub thermal_weight: u32,
    /// How much congestion weighs against wirelength, in thousandths, as in
    /// [`CongestionCost`]; zero leaves it out. The default is 1000.
    pub congestion_permille: u64,
    /// Whether a fixed area array, such as a BGA, keeps the other side free under it for its
    /// fan-out. The default is true.
    pub shadow: bool,
    /// Moves of the annealing after each placement, as [`AnnealOptions::inner_num`]. The
    /// default is 3.
    pub effort: f64,
    /// Whether movable surface-mount parts may go to the other side, when theirs is fuller
    /// than [`fill`](PlaceOptions::fill). The default is true.
    pub flip: bool,
    /// How much of a side's free room its parts' outlines, grown, may take before parts go
    /// to the other side. The default is 0.6. If legalization finds no room, a tenth more and
    /// less are tried, and a fifth less, before less spacing.
    pub fill: f64,
}

impl Default for PlaceOptions {
    fn default() -> Self {
        PlaceOptions {
            seed: 1,
            pitch: 50_000,
            grid: 100_000,
            spacing: 250_000,
            fanout: 350_000,
            fanout_pins: 8,
            rotate: true,
            decoupling_weight: 8,
            decoupling_reach: 2_000_000,
            thermal: Vec::new(),
            thermal_weight: 4,
            congestion_permille: 1000,
            shadow: true,
            effort: 3.0,
            flip: true,
            fill: 0.6,
        }
    }
}

/// Where a component ended up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    /// Its origin.
    pub at: Point,
    /// Its rotation, in millidegrees counterclockwise, in `(-180°, 180°]`.
    pub rotation: i32,
    /// Its side.
    pub side: Side,
}

/// What [`place_components`] did.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PlaceReport {
    /// By component, where it is: fixed ones where they were.
    pub placed: Vec<Placed>,
    /// How many components were movable.
    pub movable: usize,
    /// Each decoupling capacitor, with the component of the power pin it was tied to.
    pub decoupling: Vec<(usize, usize)>,
    /// The spacing that left room, which may be less than asked.
    pub spacing: i64,
    /// The half-perimeter wirelength of the nets that count, before and after: of the
    /// components where they were, and where they are.
    pub wirelength: (u64, u64),
    /// The highest ratio of RUDY's demand to capacity in any square millimetre, at the end.
    pub peak_congestion: f64,
}

/// Why a net is in a problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Signal,
    Decoupling,
    Thermal,
}

/// The cost of a board's nets: half-perimeter wirelength, and for a decoupling tie, four times
/// as much again beyond its reach.
#[derive(Clone, Debug)]
struct BoardCost {
    roles: Vec<Role>,
    reach: u64,
}

impl CostTerm for BoardCost {
    fn net_cost(&self, net: &NetView<'_>) -> u64 {
        let d = net.bounds.half_perimeter();
        let extra = match self.roles.get(net.net.index()) {
            Some(Role::Decoupling) => d.saturating_sub(self.reach).saturating_mul(4),
            _ => 0,
        };
        d.saturating_add(extra)
            .saturating_mul(u64::from(net.weight))
    }
}

/// Which side a component takes room on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Takes {
    Front,
    Back,
    Both,
}

/// A component in its frame: turned back by the quarter turns of its rotation.
#[derive(Clone, Debug)]
struct Frame {
    /// The rotation's part that is not quarter turns, in `[0, 90°)`.
    base: i32,
    /// The quarter turns it has now.
    turns: i32,
    /// The outline, relative to the origin, without spacing.
    outline: Option<PRect>,
    /// Each pin, relative to the origin.
    pins: Vec<PPoint>,
    takes: Takes,
}

/// A net of the problems: its pins, as components and offsets in their frames.
#[derive(Clone, Debug)]
struct NetDef {
    weight: u32,
    role: Role,
    pins: Vec<(usize, PPoint)>,
    /// Fixed points of the net, on the board.
    fixed: Vec<PPoint>,
}

/// Cells of the board, blocked or not, on each side.
#[derive(Clone, Debug)]
struct Raster {
    origin: PPoint,
    cell: i64,
    nx: usize,
    ny: usize,
    front: Vec<bool>,
    back: Vec<bool>,
}

impl Raster {
    fn area(&self) -> PRect {
        PRect::with_size(
            self.origin.x,
            self.origin.y,
            self.nx as i64 * self.cell,
            self.ny as i64 * self.cell,
        )
    }

    /// The cells whose interiors a rectangle's interior overlaps.
    fn cells(&self, r: &PRect) -> Option<(usize, usize, usize, usize)> {
        if r.width() == 0 || r.height() == 0 {
            return None;
        }
        let lo = |v: i64, o: i64, n: usize| (v - o).div_euclid(self.cell).clamp(0, n as i64);
        let hi = |v: i64, o: i64, n: usize| {
            ((v - o) + self.cell - 1)
                .div_euclid(self.cell)
                .clamp(0, n as i64)
        };
        let (x0, x1) = (
            lo(r.min.x, self.origin.x, self.nx),
            hi(r.max.x, self.origin.x, self.nx),
        );
        let (y0, y1) = (
            lo(r.min.y, self.origin.y, self.ny),
            hi(r.max.y, self.origin.y, self.ny),
        );
        (x1 > x0 && y1 > y0).then_some((x0 as usize, x1 as usize, y0 as usize, y1 as usize))
    }

    /// Blocks the cells under a rectangle in one grid.
    fn block_in(&self, grid: &mut [bool], r: &PRect) {
        let Some((x0, x1, y0, y1)) = self.cells(r) else {
            return;
        };
        for y in y0..y1 {
            for x in x0..x1 {
                grid[y * self.nx + x] = true;
            }
        }
    }

    fn block(&mut self, r: &PRect, front: bool, back: bool) {
        let Some((x0, x1, y0, y1)) = self.cells(r) else {
            return;
        };
        for y in y0..y1 {
            for x in x0..x1 {
                let c = y * self.nx + x;
                self.front[c] |= front;
                self.back[c] |= back;
            }
        }
    }

    /// The blocked cells of one grid, merged into disjoint rectangles: runs along each row,
    /// and runs that repeat in the rows above joined to them.
    fn rects(&self, grid: &[bool]) -> Vec<PRect> {
        let mut out = Vec::new();
        // Runs open in the row below: (start, end) -> first row.
        let mut open: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        let close = |out: &mut Vec<PRect>, (x0, x1): (usize, usize), y0: usize, y1: usize| {
            out.push(PRect::new(
                PPoint::new(
                    self.origin.x + x0 as i64 * self.cell,
                    self.origin.y + y0 as i64 * self.cell,
                ),
                PPoint::new(
                    self.origin.x + x1 as i64 * self.cell,
                    self.origin.y + y1 as i64 * self.cell,
                ),
            ));
        };
        for y in 0..=self.ny {
            let mut runs = Vec::new();
            if y < self.ny {
                let row = &grid[y * self.nx..(y + 1) * self.nx];
                let mut x = 0;
                while x < self.nx {
                    if row[x] {
                        let start = x;
                        while x < self.nx && row[x] {
                            x += 1;
                        }
                        runs.push((start, x));
                    } else {
                        x += 1;
                    }
                }
            }
            let mut next = BTreeMap::new();
            for run in runs {
                let first = open.remove(&run).unwrap_or(y);
                next.insert(run, first);
            }
            for (run, first) in core::mem::take(&mut open) {
                close(&mut out, run, first, y);
            }
            open = next;
        }
        out
    }
}

/// The edge's lines, as segments.
pub(crate) fn edge_segments(outline: &[Shape]) -> Vec<(Point, Point)> {
    let mut out = Vec::new();
    for shape in outline {
        if let Some(line) = shape.centerline() {
            for w in line.windows(2) {
                out.push((w[0], w[1]));
            }
        }
    }
    out
}

/// The squared distance from a point to a segment, in `f64`.
fn distance_sq(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx * dx + dy * dy;
    let t = if len > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (x, y) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
    x * x + y * y
}

/// Whether a rectangle comes within `distance` of any segment, or crosses one.
#[cfg_attr(not(feature = "kicad"), allow(dead_code))]
pub(crate) fn near_segments(segments: &[(Point, Point)], rect: &Rect, distance: i64) -> bool {
    let (min, max) = (rect.min(), rect.max());
    let grown = rect.expand(distance);
    let f = |p: Point| (p.x as f64, p.y as f64);
    let corners = [min, Point::new(max.x, min.y), max, Point::new(min.x, max.y)];
    let d2 = (distance as f64) * (distance as f64);
    segments.iter().any(|&(a, b)| {
        if !Rect::new(a, b).intersects(&grown) {
            return false;
        }
        let inside = |p: Point| min.x <= p.x && p.x <= max.x && min.y <= p.y && p.y <= max.y;
        if inside(a) || inside(b) {
            return true;
        }
        // An end near the rectangle, a corner near the segment, or the segment crossing a
        // side.
        let to_rect = |p: Point| {
            let dx = (min.x - p.x).max(p.x - max.x).max(0) as f64;
            let dy = (min.y - p.y).max(p.y - max.y).max(0) as f64;
            dx * dx + dy * dy
        };
        if to_rect(a) <= d2 || to_rect(b) <= d2 {
            return true;
        }
        if corners.iter().any(|&c| distance_sq(f(c), f(a), f(b)) <= d2) {
            return true;
        }
        (0..4).any(|i| crosses(a, b, corners[i], corners[(i + 1) % 4]))
    })
}

/// Whether two segments cross, in exact integers.
#[cfg_attr(not(feature = "kicad"), allow(dead_code))]
fn crosses(a: Point, b: Point, c: Point, d: Point) -> bool {
    let orient = |p: Point, q: Point, r: Point| {
        let v = i128::from(q.x - p.x) * i128::from(r.y - p.y)
            - i128::from(q.y - p.y) * i128::from(r.x - p.x);
        v.signum()
    };
    let (o1, o2, o3, o4) = (
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    );
    o1 * o2 < 0 && o3 * o4 < 0
}

fn to_p(p: Point) -> PPoint {
    PPoint::new(p.x, p.y)
}

fn to_g(p: PPoint) -> Point {
    Point::new(p.x, p.y)
}

/// The rotation in `(-180°, 180°]`.
fn normalized(rotation: i32) -> i32 {
    let r = rotation.rem_euclid(360_000);
    if r > 180_000 { r - 360_000 } else { r }
}

/// Whether pins make an area array, as a BGA's: at least 16, on at least four distinct
/// columns and rows, filling at least half of the grid of them.
fn area_array(pins: &[PPoint]) -> bool {
    if pins.len() < 16 {
        return false;
    }
    let mut xs: Vec<i64> = pins.iter().map(|p| p.x).collect();
    let mut ys: Vec<i64> = pins.iter().map(|p| p.y).collect();
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    let mut distinct: Vec<PPoint> = pins.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    xs.len() >= 4 && ys.len() >= 4 && 2 * distinct.len() >= xs.len() * ys.len()
}

/// A net name without its sheet path, in capitals.
fn bare(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_ascii_uppercase()
}

/// Whether a net's name says ground.
fn is_ground(name: &str) -> bool {
    let n = bare(name);
    n.contains("GND") || n.starts_with("VSS") || n == "0V"
}

/// Whether a net's name says supply.
fn is_supply(name: &str) -> bool {
    let n = bare(name);
    if is_ground(name) || n.starts_with("NET-") {
        return false;
    }
    n.starts_with('+')
        || [
            "VCC", "VDD", "VBUS", "VBAT", "VIN", "VPP", "VREF", "VSYS", "PWR",
        ]
        .iter()
        .any(|s| n.starts_with(s))
        || ["3V3", "1V8", "1V2", "1V1", "2V5", "5V", "12V", "VDDQ"]
            .iter()
            .any(|s| n.contains(s))
}

/// The letters a reference starts with.
fn prefix(name: &str) -> &str {
    let end = name
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(name.len());
    &name[..end]
}

/// A component in its frame, mirrored through the horizontal line through its origin if it
/// is `flipped` to the other side, as KiCad flips a footprint, top to bottom.
fn frame(design: &PlacementDesign, options: &PlaceOptions, c: usize, flipped: bool) -> Frame {
    let comp = &design.components[c];
    let mirror = |p: PPoint| if flipped { PPoint::new(p.x, -p.y) } else { p };
    let rotation = if flipped {
        -comp.rotation
    } else {
        comp.rotation
    };
    let base = rotation.rem_euclid(QUARTER);
    let turns = (rotation - base).div_euclid(QUARTER).rem_euclid(4);
    let back = -turns * QUARTER;
    let at = to_p(comp.at);
    let pins: Vec<PPoint> = comp
        .pins
        .iter()
        .map(|p| place::rotate(mirror(to_p(p.at).minus(at)), back))
        .collect();
    let outline = comp.outline.map(|r| {
        PRect::new(
            place::rotate(mirror(to_p(r.min()).minus(at)), back),
            place::rotate(mirror(to_p(r.max()).minus(at)), back),
        )
    });
    let side = if flipped {
        match comp.side {
            Side::Front => Side::Back,
            Side::Back => Side::Front,
        }
    } else {
        comp.side
    };
    let takes = if comp.through || (options.shadow && comp.fixed && area_array(&pins)) {
        Takes::Both
    } else if side == Side::Front {
        Takes::Front
    } else {
        Takes::Back
    };
    Frame {
        base,
        turns,
        outline,
        pins,
        takes,
    }
}

/// Which items a pass makes real, movable but taking no room, or fixed where they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Presence {
    Real,
    Ghost,
    Anchor,
}

/// What [`Model::problem`] made: the problem, and each component's item.
struct Built {
    problem: PlacementProblem,
    items: Vec<ItemId>,
    roles: Vec<Role>,
}

/// The design in frames, with its nets.
struct Model<'d> {
    design: &'d PlacementDesign,
    options: &'d PlaceOptions,
    frames: Vec<Frame>,
    /// Each component's side, which may be the other for a movable one.
    sides: Vec<Side>,
    nets: Vec<NetDef>,
    decoupling: Vec<(usize, usize)>,
    raster: Raster,
}

impl<'d> Model<'d> {
    /// The design in frames, with outlines grown by `scale` of the spacing, and with movable
    /// parts moved to the other side where theirs is fuller than `flip` says, if
    /// [`PlaceOptions::flip`] allows.
    fn new(
        design: &'d PlacementDesign,
        options: &'d PlaceOptions,
        scale: (i64, i64),
        flip: Option<f64>,
    ) -> Model<'d> {
        let sides: Vec<Side> = design.components.iter().map(|c| c.side).collect();
        let frames = (0..design.components.len())
            .map(|c| frame(design, options, c, false))
            .collect();
        let mut model = Model {
            design,
            options,
            frames,
            sides,
            nets: Vec::new(),
            decoupling: Vec::new(),
            raster: Raster {
                origin: PPoint::default(),
                cell: 1,
                nx: 0,
                ny: 0,
                front: Vec::new(),
                back: Vec::new(),
            },
        };
        model.raster = model.rasterize(scale);
        if let Some(fill) = flip.filter(|_| options.flip) {
            model.balance(scale, fill);
        }
        model.build_nets();
        model
    }

    /// Moves surface-mount parts to the other side while theirs is fuller than `fill` of its
    /// free room and the other side has room: parts of two pins first, smallest first.
    fn balance(&mut self, scale: (i64, i64), fill: f64) {
        let cell = (self.raster.cell as f64).powi(2);
        let free = |g: &[bool]| g.iter().filter(|&&b| !b).count() as f64 * cell;
        let room = [
            free(&self.raster.front) * fill,
            free(&self.raster.back) * fill,
        ];
        let comps = &self.design.components;
        let areas: Vec<f64> = (0..comps.len())
            .map(|c| {
                let r = self.grown(c, scale);
                r.width() as f64 * r.height() as f64
            })
            .collect();
        let area = |c: usize| areas[c];
        let index = |s: Side| usize::from(s == Side::Back);
        let mut need = [0.0, 0.0];
        for (c, comp) in comps.iter().enumerate() {
            if comp.fixed {
                continue;
            }
            match self.frames[c].takes {
                Takes::Both => {
                    need[0] += area(c);
                    need[1] += area(c);
                }
                _ => need[index(comp.side)] += area(c),
            }
        }
        let mut order: Vec<usize> = (0..comps.len())
            .filter(|&c| !comps[c].fixed && self.frames[c].takes != Takes::Both)
            .collect();
        order.sort_by(|&a, &b| {
            let key = |c: usize| (comps[c].pins.len() > 2, area(c));
            let (ka, kb) = (key(a), key(b));
            ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1)).then(a.cmp(&b))
        });
        for from in [Side::Front, Side::Back] {
            let (f, t) = (index(from), 1 - index(from));
            for &c in &order {
                if need[f] <= room[f] {
                    break;
                }
                if self.sides[c] != from || need[t] + area(c) > room[t] {
                    continue;
                }
                need[f] -= area(c);
                need[t] += area(c);
                self.sides[c] = if from == Side::Front {
                    Side::Back
                } else {
                    Side::Front
                };
                self.frames[c] = frame(self.design, self.options, c, true);
            }
        }
    }

    fn build_nets(&mut self) {
        let design = self.design;
        // Each plane net's zones' box, and the board's.
        let mut planes: BTreeMap<NetId, Rect> = BTreeMap::new();
        for &(net, r) in &design.planes {
            planes
                .entry(net)
                .and_modify(|b| *b = b.union(&r))
                .or_insert(r);
        }
        let board = edge_segments(&design.outline)
            .iter()
            .map(|&(a, b)| Rect::new(a, b))
            .reduce(|a, b| a.union(&b));
        let area = |r: &Rect| (r.max().x - r.min().x) as f64 * (r.max().y - r.min().y) as f64;
        let mut by_net: BTreeMap<NetId, Vec<(usize, PPoint)>> = BTreeMap::new();
        for (c, comp) in design.components.iter().enumerate() {
            for (p, pin) in comp.pins.iter().enumerate() {
                by_net
                    .entry(pin.net)
                    .or_default()
                    .push((c, self.frames[c].pins[p]));
            }
        }
        for (net, pins) in &by_net {
            if let Some(region) = planes.get(net) {
                // A zone that covers most of the board holds every pin anyway.
                if board.is_some_and(|b| area(region) >= 0.8 * area(&b)) {
                    continue;
                }
                for &(c, o) in pins {
                    if !design.components[c].fixed {
                        self.nets.push(NetDef {
                            weight: 4,
                            role: Role::Signal,
                            pins: vec![(c, o)],
                            fixed: vec![to_p(region.min()), to_p(region.max())],
                        });
                    }
                }
                continue;
            }
            if pins.len() < 2 {
                continue;
            }
            let parts = {
                let mut v: Vec<usize> = pins.iter().map(|&(c, _)| c).collect();
                v.dedup();
                v.len()
            };
            if parts < 2 {
                continue;
            }
            let weight = if pins.len() > 12 { 1 } else { 4 };
            self.nets.push(NetDef {
                weight,
                role: Role::Signal,
                pins: pins.clone(),
                fixed: Vec::new(),
            });
        }
        // Decoupling: capacitors between a supply and a ground, tied to power pins.
        let name = |n: NetId| design.nets.get(n.index()).map_or("", String::as_str);
        let mut caps: BTreeMap<NetId, Vec<(usize, PPoint)>> = BTreeMap::new();
        for (c, comp) in design.components.iter().enumerate() {
            if prefix(&comp.name) != "C" || comp.fixed {
                continue;
            }
            let mut nets: Vec<NetId> = comp.pins.iter().map(|p| p.net).collect();
            nets.sort_unstable();
            nets.dedup();
            let [a, b] = nets[..] else { continue };
            let supply = match (is_supply(name(a)), is_supply(name(b))) {
                (true, false) if is_ground(name(b)) => a,
                (false, true) if is_ground(name(a)) => b,
                _ => continue,
            };
            let p = comp.pins.iter().position(|p| p.net == supply).unwrap_or(0);
            caps.entry(supply)
                .or_default()
                .push((c, self.frames[c].pins[p]));
        }
        for (supply, caps) in caps {
            let power: Vec<(usize, PPoint)> = design
                .components
                .iter()
                .enumerate()
                .filter(|(_, comp)| {
                    let p = prefix(&comp.name);
                    p == "U" || p == "IC"
                })
                .flat_map(|(c, comp)| {
                    comp.pins
                        .iter()
                        .enumerate()
                        .filter(|(_, pin)| pin.net == supply)
                        .map(move |(p, _)| (c, p))
                })
                .map(|(c, p)| (c, self.frames[c].pins[p]))
                .collect();
            if power.is_empty() {
                continue;
            }
            for (k, &cap) in caps.iter().enumerate() {
                let target = power[((2 * k + 1) * power.len()) / (2 * caps.len())];
                self.decoupling.push((cap.0, target.0));
                self.nets.push(NetDef {
                    weight: self.options.decoupling_weight,
                    role: Role::Decoupling,
                    pins: vec![cap, target],
                    fixed: Vec::new(),
                });
            }
        }
        for group in &self.options.thermal {
            let pins: Vec<(usize, PPoint)> = group
                .iter()
                .filter(|&&c| c < self.frames.len())
                .map(|&c| {
                    let centre = self.frames[c].outline.map_or(PPoint::default(), |r| {
                        PPoint::new((r.min.x + r.max.x) / 2, (r.min.y + r.max.y) / 2)
                    });
                    (c, centre)
                })
                .collect();
            if pins.len() >= 2 {
                self.nets.push(NetDef {
                    weight: self.options.thermal_weight,
                    role: Role::Thermal,
                    pins,
                    fixed: Vec::new(),
                });
            }
        }
    }

    /// The free region: the board's cells, blocked outside the edge, near it, and under
    /// fixed components and keepouts.
    fn rasterize(&self, scale: (i64, i64)) -> Raster {
        let design = self.design;
        let segments = edge_segments(&design.outline);
        let mut bounds: Option<Rect> = None;
        let grow = |bounds: &mut Option<Rect>, r: Rect| {
            *bounds = Some(bounds.map_or(r, |b| b.union(&r)));
        };
        for &(a, b) in &segments {
            grow(&mut bounds, Rect::new(a, b));
        }
        let no_edge = bounds.is_none();
        for c in &design.components {
            if no_edge && let Some(o) = c.outline {
                grow(&mut bounds, o.expand(1_000_000));
            }
            // Fixed origins have to be in the area, as anchors.
            if c.fixed || no_edge {
                grow(&mut bounds, Rect::new(c.at, c.at));
            }
        }
        let bounds = bounds.unwrap_or_default();
        let cell = self.options.grid.max(1);
        let origin = PPoint::new(bounds.min().x - cell, bounds.min().y - cell);
        let nx = ((bounds.max().x - origin.x) / cell + 2) as usize;
        let ny = ((bounds.max().y - origin.y) / cell + 2) as usize;
        let mut raster = Raster {
            origin,
            cell,
            nx,
            ny,
            front: vec![false; nx * ny],
            back: vec![false; nx * ny],
        };
        if !segments.is_empty() {
            // Outside the edge, by crossings of each row's centre line.
            let mut xs: Vec<f64> = Vec::new();
            for y in 0..ny {
                let yc = (origin.y + y as i64 * cell) as f64 + cell as f64 / 2.0;
                xs.clear();
                for &(a, b) in &segments {
                    let (ay, by) = (a.y as f64, b.y as f64);
                    if (ay <= yc) != (by <= yc) {
                        let t = (yc - ay) / (by - ay);
                        xs.push(a.x as f64 + t * (b.x - a.x) as f64);
                    }
                }
                xs.sort_by(f64::total_cmp);
                for x in 0..nx {
                    let xc = (origin.x + x as i64 * cell) as f64 + cell as f64 / 2.0;
                    let crossings = xs.iter().filter(|&&v| v < xc).count();
                    if crossings % 2 == 0 {
                        raster.front[y * nx + x] = true;
                        raster.back[y * nx + x] = true;
                    }
                }
            }
            // Near the edge, which is kept as far from as a neighbour, with the edge
            // clearance, so that zones fill between.
            let margin =
                design.edge_clearance.max(0) + 2 * self.options.spacing * scale.0 / scale.1;
            let reach = margin as f64 + cell as f64 * 0.7072;
            for &(a, b) in &segments {
                let r = Rect::new(a, b).expand(margin + cell);
                let Some((x0, x1, y0, y1)) =
                    raster.cells(&PRect::new(to_p(r.min()), to_p(r.max())))
                else {
                    continue;
                };
                let (fa, fb) = ((a.x as f64, a.y as f64), (b.x as f64, b.y as f64));
                for y in y0..y1 {
                    for x in x0..x1 {
                        let c = (
                            (origin.x + x as i64 * cell) as f64 + cell as f64 / 2.0,
                            (origin.y + y as i64 * cell) as f64 + cell as f64 / 2.0,
                        );
                        if distance_sq(c, fa, fb) <= reach * reach {
                            raster.front[y * nx + x] = true;
                            raster.back[y * nx + x] = true;
                        }
                    }
                }
            }
        }
        // Fixed things are kept as far from as movable ones are from each other.
        let grow = (self.options.spacing * scale.0 / scale.1).max(design.clearance / 2);
        for (c, comp) in design.components.iter().enumerate() {
            if !comp.fixed {
                continue;
            }
            let Some(o) = comp.outline else { continue };
            let o = o.expand(grow);
            let r = PRect::new(to_p(o.min()), to_p(o.max()));
            match self.frames[c].takes {
                Takes::Front => raster.block(&r, true, false),
                Takes::Back => raster.block(&r, false, true),
                Takes::Both => raster.block(&r, true, true),
            }
        }
        for &(r, side) in &design.keepouts {
            let r = r.expand(grow);
            let r = PRect::new(to_p(r.min()), to_p(r.max()));
            raster.block(&r, side != Some(Side::Back), side != Some(Side::Front));
        }
        raster
    }

    /// The margin a movable component's outline is grown by.
    fn margin(&self, c: usize, scale: (i64, i64)) -> i64 {
        let pins = self.design.components[c].pins.len();
        let fanout = if pins >= self.options.fanout_pins {
            self.options.fanout
        } else {
            0
        };
        ((self.options.spacing + fanout) * scale.0 / scale.1).max(self.design.clearance / 2)
    }

    /// The outline of a movable component in its frame, grown.
    fn grown(&self, c: usize, scale: (i64, i64)) -> PRect {
        let f = &self.frames[c];
        let base = f.outline.unwrap_or_else(|| {
            let mut r = PRect::point(f.pins.first().copied().unwrap_or_default());
            for &p in &f.pins {
                r = r.including(p);
            }
            r
        });
        let m = self.margin(c, scale);
        PRect::new(
            PPoint::new(base.min.x - m, base.min.y - m),
            PPoint::new(base.max.x + m, base.max.y + m),
        )
    }

    fn rotations(&self) -> Vec<i32> {
        if self.options.rotate {
            vec![0, QUARTER, 2 * QUARTER, 3 * QUARTER]
        } else {
            vec![0]
        }
    }

    /// A problem on the grid of `blocked` cells, with each component as `role` says, at
    /// `at` if it is an anchor and has been placed.
    fn problem(
        &self,
        blocked: &[bool],
        role: &dyn Fn(usize) -> Presence,
        at: &[Option<(PPoint, i32)>],
        scale: (i64, i64),
    ) -> Built {
        let area = self.raster.area();
        let mut problem = PlacementProblem::new(Layout::FreeSpace {
            area,
            pitch: self.options.pitch.max(1),
        });
        for r in self.raster.rects(blocked) {
            problem.add_item(
                place::Item::new()
                    .outline(PRect::with_size(0, 0, r.width() as i64, r.height() as i64))
                    .fixed_at(r.min, 0),
            );
        }
        let rotations = self.rotations();
        let mut items = Vec::with_capacity(self.frames.len());
        for (c, comp) in self.design.components.iter().enumerate() {
            let f = &self.frames[c];
            let item = if comp.fixed {
                place::Item::new().fixed_at(to_p(comp.at), f.turns * QUARTER)
            } else {
                match role(c) {
                    Presence::Real => place::Item::new()
                        .outline(self.grown(c, scale))
                        .rotations(&rotations),
                    Presence::Ghost => place::Item::new().rotations(&rotations),
                    Presence::Anchor => {
                        let (p, r) = at[c].unwrap_or((to_p(comp.at), f.turns * QUARTER));
                        place::Item::new().fixed_at(p, r)
                    }
                }
            };
            items.push(problem.add_item(item));
        }
        // One fixed item at the area's corner holds the nets' fixed points.
        let corner = problem.add_item(place::Item::new().fixed_at(area.min, 0));
        let mut roles = Vec::with_capacity(self.nets.len());
        for net in &self.nets {
            let pins: Vec<Pin> = net
                .pins
                .iter()
                .map(|&(c, o)| Pin::at(items[c], o.x, o.y))
                .chain(net.fixed.iter().map(|p| {
                    let o = p.minus(area.min);
                    Pin::at(corner, o.x, o.y)
                }))
                .collect();
            problem.add_net(net.weight, &pins);
            roles.push(net.role);
        }
        Built {
            problem,
            items,
            roles,
        }
    }

    fn cost(&self, roles: &[Role]) -> BoardCost {
        BoardCost {
            roles: roles.to_vec(),
            reach: self.options.decoupling_reach.max(0) as u64,
        }
    }

    /// A RUDY map of the board's real nets, with the components where `at` says.
    fn congestion(&self, at: &[(PPoint, i32)]) -> CongestionGrid {
        let area = self.raster.area();
        let bin = 1_000_000;
        let pitch = self.design.track_pitch.max(1) as f64;
        // Half of what the layers could carry, for vias, pads and detours.
        let capacity = self.design.layers.max(1) as f64 * (bin as f64 / pitch) * bin as f64 * 0.5;
        let mut grid = CongestionGrid::new(area, bin, capacity);
        for net in self.nets.iter().filter(|n| n.role == Role::Signal) {
            if !net.fixed.is_empty() {
                continue;
            }
            let mut bounds: Option<PRect> = None;
            for &(c, o) in &net.pins {
                let p = at[c].0.offset(place::rotate(o, at[c].1));
                bounds = Some(bounds.map_or(PRect::point(p), |b| b.including(p)));
            }
            if let Some(b) = bounds {
                grid.spread(&b, b.half_perimeter() as f64);
            }
        }
        grid
    }

    /// The real nets' half-perimeter wirelength, with the components where `at` says.
    fn wirelength(&self, at: &[(PPoint, i32)]) -> u64 {
        self.nets
            .iter()
            .filter(|n| n.role == Role::Signal && n.fixed.is_empty())
            .map(|net| {
                let mut bounds: Option<PRect> = None;
                for &(c, o) in &net.pins {
                    let p = at[c].0.offset(place::rotate(o, at[c].1));
                    bounds = Some(bounds.map_or(PRect::point(p), |b| b.including(p)));
                }
                bounds.map_or(0, |b| b.half_perimeter())
            })
            .sum()
    }

    /// Anneals from a legal placement, with congestion priced from it.
    fn refine(
        &self,
        built: &Built,
        start: Placement,
        at: &[(PPoint, i32)],
    ) -> Result<Placement, PlaceError> {
        let options = AnnealOptions {
            seed: self.options.seed,
            inner_num: self.options.effort,
            ..AnnealOptions::detailed()
        };
        let cost = self.cost(&built.roles);
        let outcome = if self.options.congestion_permille > 0 {
            let both = (
                cost,
                CongestionCost {
                    estimate: self.congestion(at),
                    factor_permille: self.options.congestion_permille,
                },
            );
            Annealer::new(&built.problem, both, start, options)?.run()
        } else {
            Annealer::new(&built.problem, cost, start, options)?.run()
        };
        Ok(outcome.placement)
    }

    /// Analytic placement of a problem, then annealing with congestion.
    fn analytic(&self, built: &Built, at: &[(PPoint, i32)]) -> Result<Placement, PlaceError> {
        let options = AnalyticOptions {
            seed: self.options.seed,
            detailed: Some(AnnealOptions {
                seed: self.options.seed,
                inner_num: self.options.effort,
                ..AnnealOptions::detailed()
            }),
            ..AnalyticOptions::default()
        };
        let (outcome, _) = place_analytic(&built.problem, self.cost(&built.roles), options)?;
        let mut now = at.to_vec();
        self.take(built, &outcome.placement, &mut now, &|_| true);
        self.refine(built, outcome.placement, &now)
    }

    /// Copies where the movable components of `which` are in a placement into `at`.
    fn take(
        &self,
        built: &Built,
        placement: &Placement,
        at: &mut [(PPoint, i32)],
        which: &dyn Fn(usize) -> bool,
    ) {
        for (c, comp) in self.design.components.iter().enumerate() {
            if !comp.fixed && which(c) {
                let item = built.items[c];
                at[c] = (placement.origin(item), placement.rotation(item));
            }
        }
    }

    /// Places every movable component, with outlines grown by `scale` of the spacing.
    fn run(&self, scale: (i64, i64)) -> Result<Vec<(PPoint, i32)>, PlaceError> {
        let comps = &self.design.components;
        let mut at: Vec<(PPoint, i32)> = comps
            .iter()
            .zip(&self.frames)
            .map(|(c, f)| (to_p(c.at), f.turns * QUARTER))
            .collect();
        let movable = |t: Takes| {
            comps
                .iter()
                .zip(&self.frames)
                .any(|(c, f)| !c.fixed && f.takes == t)
        };
        let takes = |c: usize| self.frames[c].takes;
        let mut front = self.raster.front.clone();
        let mut back = self.raster.back.clone();

        // 0. Through the board, against both sides.
        if movable(Takes::Both) {
            let both: Vec<bool> = front.iter().zip(&back).map(|(a, b)| *a || *b).collect();
            let role = |c: usize| {
                if takes(c) == Takes::Both {
                    Presence::Real
                } else {
                    Presence::Ghost
                }
            };
            let built = self.problem(&both, &role, &[], scale);
            let placement = self.analytic(&built, &at)?;
            self.take(&built, &placement, &mut at, &|c| takes(c) == Takes::Both);
            for (c, comp) in comps.iter().enumerate() {
                if !comp.fixed && takes(c) == Takes::Both {
                    let g = self.grown(c, (0, 1));
                    let r =
                        PRect::new(place::rotate(g.min, at[c].1), place::rotate(g.max, at[c].1))
                            .translate(at[c].0);
                    self.raster.block_in(&mut front, &r);
                    self.raster.block_in(&mut back, &r);
                }
            }
        }
        let placed_both = |c: usize| takes(c) == Takes::Both;
        let anchors: Vec<Option<(PPoint, i32)>> = at.iter().map(|&a| Some(a)).collect();

        // 1. The front, with the back's components taking no room.
        let has_front = movable(Takes::Front);
        let has_back = movable(Takes::Back);
        let mut front_placement = None;
        if has_front || has_back {
            let role = |c: usize| match takes(c) {
                Takes::Front => Presence::Real,
                Takes::Back => Presence::Ghost,
                Takes::Both => Presence::Anchor,
            };
            let built = self.problem(&front, &role, &anchors, scale);
            let placement = self.analytic(&built, &at)?;
            self.take(&built, &placement, &mut at, &|c| !placed_both(c));
            front_placement = Some(placement);
        }
        if !has_back {
            return Ok(at);
        }

        // 2. The back, from where the first pass put it.
        let anchors: Vec<Option<(PPoint, i32)>> = at.iter().map(|&a| Some(a)).collect();
        let role = |c: usize| match takes(c) {
            Takes::Back => Presence::Real,
            _ => Presence::Anchor,
        };
        let built = self.problem(&back, &role, &anchors, scale);
        let mut targets = vec![PPoint::default(); built.problem.items().len()];
        let mut rotations = vec![0; built.problem.items().len()];
        for (c, &item) in built.items.iter().enumerate() {
            targets[item.index()] = at[c].0;
            rotations[item.index()] = at[c].1;
        }
        let start = place::legalize(&built.problem, &targets, &rotations)?;
        let mut now = at.clone();
        self.take(&built, &start, &mut now, &|c| takes(c) == Takes::Back);
        let placement = self.refine(&built, start, &now)?;
        self.take(&built, &placement, &mut at, &|c| takes(c) == Takes::Back);

        // 3. The front again, with the back where it is.
        if has_front && front_placement.is_some() {
            let anchors: Vec<Option<(PPoint, i32)>> = at.iter().map(|&a| Some(a)).collect();
            let role = |c: usize| match takes(c) {
                Takes::Front => Presence::Real,
                _ => Presence::Anchor,
            };
            let built = self.problem(&front, &role, &anchors, scale);
            let mut start = Placement::new(&built.problem);
            for (c, comp) in comps.iter().enumerate() {
                if !comp.fixed && takes(c) == Takes::Front {
                    start.set_origin(built.items[c], at[c].0, at[c].1);
                }
            }
            let placement = self.refine(&built, start, &at)?;
            self.take(&built, &placement, &mut at, &|c| takes(c) == Takes::Front);
        }
        Ok(at)
    }
}

/// Places a board's movable components.
///
/// Fixed components stay where they are. Each movable one keeps its side, and turns by
/// quarter turns from the part of its rotation that isn't one, if
/// [`PlaceOptions::rotate`] allows. Nothing of where movable components were is used, but
/// their sides: they start from nothing. See the [module](self) for how.
///
/// # Errors
///
/// [`PlaceError::NoRoom`] if the movable components don't fit on the board even without
/// spacing, and [`PlaceError::Problem`] if a problem made from the design is malformed, as
/// when the board has no area.
pub fn place_components(
    design: &PlacementDesign,
    options: &PlaceOptions,
) -> Result<PlaceReport, PlaceError> {
    let movable = design.components.iter().filter(|c| !c.fixed).count();
    let unmoved = Model::new(design, options, (1, 1), None);
    let original: Vec<(PPoint, i32)> = design
        .components
        .iter()
        .zip(&unmoved.frames)
        .map(|(c, f)| (to_p(c.at), f.turns * QUARTER))
        .collect();
    let before = unmoved.wirelength(&original);
    let mut result = None;
    let mut error = None;
    // Fuller sides first, then less spacing: legalization in fragmented room can fail where
    // a little more or less on a side would not.
    let fills = [0.0, -0.1, 0.1, -0.2].map(|d: f64| (options.fill + d).clamp(0.1, 1.0));
    'search: for scale in [(4, 4), (3, 4), (2, 4), (1, 4), (0, 4)] {
        let spacing = options.spacing * scale.0 / scale.1;
        if movable == 0 {
            result = Some((original.clone(), spacing, unmoved));
            break;
        }
        for (k, &fill) in fills.iter().enumerate() {
            if k > 0 && !options.flip {
                break;
            }
            let model = Model::new(design, options, scale, Some(fill));
            match model.run(scale) {
                Ok(at) => {
                    result = Some((at, spacing, model));
                    break 'search;
                }
                Err(e @ PlaceError::NoRoom(_)) => error = Some(e),
                Err(e) => return Err(e),
            }
        }
    }
    let Some((at, spacing, model)) = result else {
        return Err(error.unwrap_or(PlaceError::NoRoom(ItemId(0))));
    };
    let placed = design
        .components
        .iter()
        .zip(&model.frames)
        .zip(&at)
        .zip(&model.sides)
        .map(|(((c, f), &(p, r)), &side)| {
            if c.fixed {
                Placed {
                    at: c.at,
                    rotation: c.rotation,
                    side: c.side,
                }
            } else {
                Placed {
                    at: to_g(p),
                    rotation: normalized(f.base + r),
                    side,
                }
            }
        })
        .collect();
    Ok(PlaceReport {
        placed,
        movable,
        decoupling: model.decoupling.clone(),
        spacing,
        wirelength: (before, model.wirelength(&at)),
        peak_congestion: model.congestion(&at).peak_utilization(),
    })
}

/// Where a point of a component, on the board as it is now, is with the component where
/// `placed` puts it: mirrored top to bottom about its origin if it changed sides, turned, and
/// moved. `None` if the turn isn't a whole number of quarter turns.
fn placed_point(comp: &Component, placed: &Placed, q: Point) -> Option<Point> {
    let flipped = placed.side != comp.side;
    let was = if flipped {
        -comp.rotation
    } else {
        comp.rotation
    };
    let turn = placed.rotation - was;
    if turn.rem_euclid(QUARTER) != 0 {
        return None;
    }
    let v = to_p(q).minus(to_p(comp.at));
    let v = if flipped { PPoint::new(v.x, -v.y) } else { v };
    Some(to_g(place::rotate(v, turn).offset(to_p(placed.at))))
}

/// Where a component's pins are, with it where `placed` puts it.
#[cfg(all(test, feature = "kicad"))]
pub(crate) fn pin_positions(design: &PlacementDesign, c: usize, placed: &Placed) -> Vec<Point> {
    let comp = &design.components[c];
    comp.pins
        .iter()
        .filter_map(|p| placed_point(comp, placed, p.at))
        .collect()
}

/// Pairs of components whose outlines overlap, with each where `placed` puts it, flipped if
/// it changed sides: on the same side, or either through the board. Outlines that only touch don't overlap. A check that
/// shares nothing with the placers.
pub fn outline_overlaps(design: &PlacementDesign, placed: &[Placed]) -> Vec<(usize, usize)> {
    let boxes: Vec<Option<(Rect, Side, bool)>> = design
        .components
        .iter()
        .zip(placed)
        .map(|(c, p)| {
            let o = c.outline?;
            let a = placed_point(c, p, o.min())?;
            let b = placed_point(c, p, o.max())?;
            Some((Rect::new(a, b), p.side, c.through))
        })
        .collect();
    let mut out = Vec::new();
    for i in 0..boxes.len() {
        let Some((a, sa, ta)) = boxes[i] else {
            continue;
        };
        for (j, b) in boxes.iter().enumerate().skip(i + 1) {
            let Some((b, sb, tb)) = *b else { continue };
            if sa != sb && !ta && !tb {
                continue;
            }
            let overlap = a.min().x < b.max().x
                && b.min().x < a.max().x
                && a.min().y < b.max().y
                && b.min().y < a.max().y;
            if overlap {
                out.push((i, j));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(at: Point, half: i64) -> Rect {
        Rect::new(
            Point::new(at.x - half, at.y - half),
            Point::new(at.x + half, at.y + half),
        )
    }

    fn board(side: i64) -> Vec<Shape> {
        let c = [
            Point::new(0, 0),
            Point::new(side, 0),
            Point::new(side, side),
            Point::new(0, side),
        ];
        (0..4)
            .map(|i| Shape::track(c[i], c[(i + 1) % 4], 100_000).unwrap())
            .collect()
    }

    /// A connector fixed at the left, an IC, and capacitors and resistors around.
    fn design() -> PlacementDesign {
        let mm = 1_000_000;
        let mut d = PlacementDesign {
            outline: board(20 * mm),
            edge_clearance: 300_000,
            layers: 2,
            track_pitch: 400_000,
            nets: vec![
                String::new(),
                "GND".into(),
                "+3V3".into(),
                "A".into(),
                "B".into(),
            ],
            ..PlacementDesign::default()
        };
        let mut j = Component::new("J1", Point::new(2 * mm, 10 * mm), 0, Side::Front);
        j.fixed = true;
        j.outline = Some(square(j.at, mm));
        for (k, net) in [1, 2, 3, 4].into_iter().enumerate() {
            j.pins.push(ComponentPin {
                at: Point::new(2 * mm, 10 * mm + (k as i64 - 2) * 500_000),
                net: NetId(net),
            });
        }
        d.components.push(j);
        // Spread out at first, as if the original placement didn't matter.
        let mut u = Component::new("U1", Point::new(15 * mm, 15 * mm), 90_000, Side::Front);
        u.outline = Some(square(u.at, 1_500_000));
        for (k, net) in [1, 2, 3, 4, 2, 1, 3, 4].into_iter().enumerate() {
            u.pins.push(ComponentPin {
                at: Point::new(
                    15 * mm + (k as i64 % 4 - 2) * 500_000,
                    15 * mm + (k as i64 / 4) * mm,
                ),
                net: NetId(net),
            });
        }
        d.components.push(u);
        for k in 0..3 {
            let at = Point::new((5 + 4 * k) * mm, 4 * mm);
            let mut c = Component::new(format!("C{k}"), at, 0, Side::Back);
            c.outline = Some(square(at, 600_000));
            c.pins.push(ComponentPin {
                at: Point::new(at.x - 400_000, at.y),
                net: NetId(2),
            });
            c.pins.push(ComponentPin {
                at: Point::new(at.x + 400_000, at.y),
                net: NetId(1),
            });
            d.components.push(c);
        }
        let mut r = Component::new("R1", Point::new(18 * mm, 3 * mm), 0, Side::Front);
        r.outline = Some(square(r.at, 700_000));
        r.through = true;
        for (dx, net) in [(-400_000, 3), (400_000, 4)] {
            r.pins.push(ComponentPin {
                at: Point::new(r.at.x + dx, r.at.y),
                net: NetId(net),
            });
        }
        d.components.push(r);
        d
    }

    #[test]
    fn places_legally_and_deterministically() {
        let d = design();
        let options = PlaceOptions::default();
        let report = place_components(&d, &options).unwrap();
        assert_eq!(report.movable, 5);
        assert_eq!(report.decoupling.len(), 3);
        assert!(report.decoupling.iter().all(|&(_, ic)| ic == 1));
        assert!(outline_overlaps(&d, &report.placed).is_empty());
        // Fixed stays; everything inside the board, clear of its edge.
        assert_eq!(report.placed[0].at, d.components[0].at);
        let segments = edge_segments(&d.outline);
        for (c, p) in d.components.iter().zip(&report.placed) {
            let o = c.outline.unwrap();
            let turn = p.rotation - c.rotation;
            let a = place::rotate(to_p(o.min()).minus(to_p(c.at)), turn).offset(to_p(p.at));
            let b = place::rotate(to_p(o.max()).minus(to_p(c.at)), turn).offset(to_p(p.at));
            let r = Rect::new(to_g(a), to_g(b));
            assert!(!near_segments(&segments, &r, 300_000), "{}: {r:?}", c.name);
            assert!(r.min().x > 0 && r.max().x < 20_000_000, "{}", c.name);
            assert_eq!(p.side, c.side);
        }
        // Shorter than spread out.
        assert!(report.wirelength.1 < report.wirelength.0, "{report:?}");
        // Decoupling capacitors near the IC.
        for &(cap, _) in &report.decoupling {
            let d = report.placed[cap].at;
            let u = report.placed[1].at;
            assert!(
                d.x.abs_diff(u.x) + d.y.abs_diff(u.y) < 6_000_000,
                "{d:?} {u:?}"
            );
        }
        let again = place_components(&d, &options).unwrap();
        assert_eq!(report, again);
    }

    #[test]
    fn thermal_groups_stay_together() {
        let mut d = design();
        // Two resistors far apart on nets of their own.
        for (k, x) in [(0, 3), (1, 17)] {
            let at = Point::new(x * 1_000_000, 17_000_000);
            let mut r = Component::new(format!("R{}", 10 + k), at, 0, Side::Front);
            r.outline = Some(square(at, 600_000));
            d.components.push(r);
        }
        let n = d.components.len();
        let apart = |report: &PlaceReport| {
            let (a, b) = (report.placed[n - 2].at, report.placed[n - 1].at);
            a.x.abs_diff(b.x) + a.y.abs_diff(b.y)
        };
        let options = PlaceOptions {
            thermal: vec![vec![n - 2, n - 1]],
            ..PlaceOptions::default()
        };
        let grouped = place_components(&d, &options).unwrap();
        assert!(outline_overlaps(&d, &grouped.placed).is_empty());
        assert!(apart(&grouped) < 3_000_000, "{}", apart(&grouped));
    }

    #[test]
    fn rasters_merge_into_disjoint_rectangles() {
        let d = design();
        let options = PlaceOptions::default();
        let model = Model::new(&d, &options, (1, 1), None);
        let rects = model.raster.rects(&model.raster.front);
        let blocked = model.raster.front.iter().filter(|&&b| b).count() as u64;
        let cell = (options.grid * options.grid) as u64;
        assert_eq!(rects.iter().map(PRect::area).sum::<u64>(), blocked * cell);
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(!a.overlaps(b));
            }
        }
    }

    #[test]
    fn names_say_supplies_and_grounds() {
        assert!(is_ground("GND") && is_ground("/power/AGND") && is_ground("VSS"));
        assert!(is_supply("+3V3") && is_supply("VDDQ") && is_supply("/VBUS"));
        assert!(!is_supply("GND") && !is_supply("Net-(C1-Pad1)") && !is_supply("SDA"));
        assert!(area_array(
            &(0..25)
                .map(|k| PPoint::new(k % 5, k / 5))
                .collect::<Vec<_>>()
        ));
        assert!(!area_array(
            &(0..20)
                .map(|k| PPoint::new(if k < 10 { 0 } else { 5 }, k % 10))
                .collect::<Vec<_>>()
        ));
    }
}
