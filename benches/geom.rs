//! The geometry layer: the R-tree against a uniform grid, and clearance checks with and
//! without the `f64` filter.
//!
//! Every board comes from a fixed seed, so that the instances never vary. A board is
//! 100 × 80 mm in nanometres, with the kinds of thing a dense two-layer board has, in
//! proportions that are plausible but not measured from real boards:
//!
//! - twelve quad packages of 100 pads at 0.5 mm pitch, and two 16 × 16 BGAs at 0.8 mm;
//! - 300 two-pad passives, and 800 vias of 0.6 mm;
//! - 3 000 tracks, 0.15 to 0.5 mm wide, at multiples of 45°, with lengths spread evenly
//!   on a log scale from 0.3 to 40 mm, as most tracks are short and a few cross the board;
//! - six pours of 20 to 60 mm and four keepouts of 5 to 15 mm.
//!
//! The workloads are what a router and a DRC do with an index: build it in bulk, or one
//! insert at a time; find everything within 0.2 mm of each item; and rip up 300 tracks and
//! put each back elsewhere, checking around it as it goes in.
//!
//! The grid is here and not in the library, as it didn't earn a place there. Medians on one
//! machine (x86-64, Rust 1.95), with 6 122 items, of which runs differ by 5 to 10%:
//!
//! | Workload | R-tree | Grid, 0.25 mm | Grid, 1 mm | Grid, 4 mm |
//! | --- | --- | --- | --- | --- |
//! | Bulk load | 0.24 ms | 25 ms | 1.3 ms | 0.18 ms |
//! | Insert one at a time | 1.9 ms | 29 ms | 1.3 ms | 0.20 ms |
//! | Query around every item | 12.9 ms | 332 ms | 37 ms | 14.8 ms |
//! | The same, but not around pours and keepouts | 12.6 ms | 306 ms | 34 ms | 15.1 ms |
//! | Rip up and put back 300 tracks | 1.31 ms | 36 ms | 3.3 ms | 1.21 ms |
//!
//! With its best cell size, the grid inserts ten times as fast and churns about as fast,
//! and queries 15% slower. With a cell four times too small, it is three to ten times
//! slower at everything, and sixteen times too small, twenty to thirty. A cell size suits
//! one mixture of item sizes, which a library doesn't know in advance, and the grid needs
//! the board's extent as well. The R-tree needs neither and was never more than 15% behind
//! the best grid, except at inserting one at a time, where a bulk load does as well. So the
//! R-tree stays and the grid goes. Nodes of 8 and 16 children were within a few percent of
//! each other, and 32 was 10 to 20% slower.
//!
//! The clearance group times `Shape::violates` on every pair of items whose boxes are
//! within 0.2 mm, against polyclip's `distance_less_than` on the same cores, which is what
//! `violates` would do without its filter. On the 130 718 pairs without a polygon, the
//! filter makes it 2.6 times as fast, 3.8 ms against 9.9 ms. With the 60 208 pairs with a
//! polygon as well, it is 1.4 times as fast, 16 ms against 22 ms, as the filter can only
//! prove a polygon too close, never far enough: the other shape could be inside it.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use guide::geom::{EntryId, Point, Rect, Shape, SpatialIndex};
use guide::rng::Rng;

const MM: i64 = 1_000_000;
const WIDTH: i64 = 100 * MM;
const HEIGHT: i64 = 80 * MM;
const CLEARANCE: i64 = MM / 5;

/// One item of a synthetic board: its shape, and the same core in polyclip's terms for the
/// comparison without the filter.
struct Item {
    shape: Shape,
    core: Core,
    /// The radius to add to a polyclip distance.
    radius: i64,
}

enum Core {
    Point(polyclip::Point),
    Segment(polyclip::Segment),
    Polygon(polyclip::Polygon),
}

fn pp(p: Point) -> polyclip::Point {
    polyclip::Point::new(p.x, p.y)
}

fn pad(c: Point, w: i64, h: i64) -> Item {
    let (min, max) = (
        Point::new(c.x - w / 2, c.y - h / 2),
        Point::new(c.x + w / 2, c.y + h / 2),
    );
    let ring = polyclip::Ring::from([
        (min.x, min.y),
        (max.x, min.y),
        (max.x, max.y),
        (min.x, max.y),
    ]);
    Item {
        shape: Shape::rect(Rect::new(min, max)).unwrap(),
        core: Core::Polygon(polyclip::Polygon::new(ring, Vec::new())),
        radius: 0,
    }
}

fn circle(c: Point, d: i64) -> Item {
    Item {
        shape: Shape::circle(c, d).unwrap(),
        core: Core::Point(pp(c)),
        radius: d / 2,
    }
}

fn track(a: Point, b: Point, w: i64) -> Item {
    Item {
        shape: Shape::track(a, b, w).unwrap(),
        core: Core::Segment(polyclip::Segment { a: pp(a), b: pp(b) }),
        radius: w / 2,
    }
}

fn uniform(rng: &mut Rng, lo: i64, hi: i64) -> i64 {
    lo + rng.below((hi - lo) as u64) as i64
}

/// A random track inside the board: even widths, so that radii are whole for polyclip.
fn random_track(rng: &mut Rng) -> Item {
    let len = (0.3 * MM as f64 * (40.0f64 / 0.3).powf(rng.unit())) as i64;
    let dirs = [(1, 0), (1, 1), (0, 1), (-1, 1)];
    let (dx, dy) = dirs[rng.below(4) as usize];
    let step = if dx != 0 && dy != 0 {
        len * 7 / 10
    } else {
        len
    };
    let a = Point::new(
        uniform(rng, step, WIDTH - step),
        uniform(rng, 0, HEIGHT - step),
    );
    let b = Point::new(a.x + dx * step, a.y + dy * step);
    track(a, b, 2 * uniform(rng, 75_000, 250_000))
}

fn board(seed: u64) -> Vec<Item> {
    let mut rng = Rng::new(seed);
    let mut items = Vec::new();
    // Quad packages: 25 pads a side.
    for _ in 0..12 {
        let c = Point::new(
            uniform(&mut rng, 10 * MM, WIDTH - 10 * MM),
            uniform(&mut rng, 10 * MM, HEIGHT - 10 * MM),
        );
        for k in 0..25 {
            let along = (k - 12) * MM / 2;
            let off = 7 * MM;
            items.push(pad(
                Point::new(c.x + along, c.y - off),
                MM * 3 / 10,
                MM * 3 / 2,
            ));
            items.push(pad(
                Point::new(c.x + along, c.y + off),
                MM * 3 / 10,
                MM * 3 / 2,
            ));
            items.push(pad(
                Point::new(c.x - off, c.y + along),
                MM * 3 / 2,
                MM * 3 / 10,
            ));
            items.push(pad(
                Point::new(c.x + off, c.y + along),
                MM * 3 / 2,
                MM * 3 / 10,
            ));
        }
    }
    for _ in 0..2 {
        let c = Point::new(
            uniform(&mut rng, 10 * MM, WIDTH - 10 * MM),
            uniform(&mut rng, 10 * MM, HEIGHT - 10 * MM),
        );
        for i in 0..16 {
            for j in 0..16 {
                let at = Point::new(c.x + (i - 8) * MM * 4 / 5, c.y + (j - 8) * MM * 4 / 5);
                items.push(circle(at, MM * 2 / 5));
            }
        }
    }
    for _ in 0..300 {
        let c = Point::new(
            uniform(&mut rng, MM, WIDTH - MM),
            uniform(&mut rng, MM, HEIGHT - MM),
        );
        items.push(pad(Point::new(c.x - MM / 2, c.y), MM * 3 / 5, MM * 9 / 10));
        items.push(pad(Point::new(c.x + MM / 2, c.y), MM * 3 / 5, MM * 9 / 10));
    }
    for _ in 0..800 {
        let c = Point::new(
            uniform(&mut rng, MM, WIDTH - MM),
            uniform(&mut rng, MM, HEIGHT - MM),
        );
        items.push(circle(c, MM * 3 / 5));
    }
    for _ in 0..3000 {
        items.push(random_track(&mut rng));
    }
    for (count, lo, hi) in [(6, 20, 60), (4, 5, 15)] {
        for _ in 0..count {
            let (w, h) = (
                uniform(&mut rng, lo * MM, hi * MM),
                uniform(&mut rng, lo * MM, hi * MM),
            );
            let c = Point::new(
                uniform(&mut rng, w / 2, WIDTH - w / 2),
                uniform(&mut rng, h / 2, HEIGHT - h / 2),
            );
            items.push(pad(c, w, h));
        }
    }
    items
}

/// What an index has to do here.
trait Index {
    fn bulk(rects: &[Rect]) -> Self;
    fn empty() -> Self;
    fn insert(&mut self, r: Rect) -> u64;
    fn remove(&mut self, id: u64);
    fn query(&self, r: &Rect, out: &mut Vec<u64>);
}

struct Tree(SpatialIndex, Vec<EntryId>, std::cell::RefCell<Vec<EntryId>>);

impl Index for Tree {
    fn bulk(rects: &[Rect]) -> Self {
        let index = SpatialIndex::bulk_load(rects.iter().copied());
        let ids = index.ids().collect();
        Tree(index, ids, Default::default())
    }
    fn empty() -> Self {
        Tree(SpatialIndex::new(), Vec::new(), Default::default())
    }
    fn insert(&mut self, r: Rect) -> u64 {
        let id = self.0.insert(r);
        let slot = id.slot() as usize;
        if slot >= self.1.len() {
            self.1.resize(slot + 1, id);
        }
        self.1[slot] = id;
        slot as u64
    }
    fn remove(&mut self, id: u64) {
        self.0.remove(self.1[id as usize]);
    }
    fn query(&self, r: &Rect, out: &mut Vec<u64>) {
        let mut found = self.2.borrow_mut();
        self.0.query(r, &mut found);
        out.clear();
        out.extend(found.iter().map(|id| u64::from(id.slot())));
    }
}

/// A uniform grid over the board, holding each entry in every cell its box meets.
struct Grid {
    cell: i64,
    cols: i64,
    rows: i64,
    cells: Vec<Vec<u32>>,
    rects: Vec<Option<Rect>>,
    free: Vec<u32>,
}

impl Grid {
    fn new(cell: i64) -> Grid {
        let (cols, rows) = (WIDTH / cell + 1, HEIGHT / cell + 1);
        Grid {
            cell,
            cols,
            rows,
            cells: vec![Vec::new(); (cols * rows) as usize],
            rects: Vec::new(),
            free: Vec::new(),
        }
    }

    fn span(&self, r: &Rect) -> (i64, i64, i64, i64) {
        let col = |x: i64| (x.div_euclid(self.cell)).clamp(0, self.cols - 1);
        let row = |y: i64| (y.div_euclid(self.cell)).clamp(0, self.rows - 1);
        (
            col(r.min().x),
            row(r.min().y),
            col(r.max().x),
            row(r.max().y),
        )
    }

    fn cells_of(&self, r: &Rect) -> impl Iterator<Item = usize> + use<> {
        let (x0, y0, x1, y1) = self.span(r);
        let cols = self.cols;
        (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| (y * cols + x) as usize))
    }

    fn put(&mut self, r: Rect) -> u64 {
        let id = match self.free.pop() {
            Some(id) => {
                self.rects[id as usize] = Some(r);
                id
            }
            None => {
                self.rects.push(Some(r));
                (self.rects.len() - 1) as u32
            }
        };
        for c in self.cells_of(&r) {
            self.cells[c].push(id);
        }
        u64::from(id)
    }
}

struct Cells<const CELL: i64>(Grid);

impl<const CELL: i64> Index for Cells<CELL> {
    fn bulk(rects: &[Rect]) -> Self {
        let mut grid = Grid::new(CELL);
        for r in rects {
            grid.put(*r);
        }
        Cells(grid)
    }
    fn empty() -> Self {
        Cells(Grid::new(CELL))
    }
    fn insert(&mut self, r: Rect) -> u64 {
        self.0.put(r)
    }
    fn remove(&mut self, id: u64) {
        let Some(r) = self.0.rects[id as usize].take() else {
            return;
        };
        for c in self.0.cells_of(&r) {
            self.0.cells[c].retain(|&k| u64::from(k) != id);
        }
        self.0.free.push(id as u32);
    }
    fn query(&self, r: &Rect, out: &mut Vec<u64>) {
        out.clear();
        for c in self.0.cells_of(r) {
            out.extend(self.0.cells[c].iter().map(|&k| u64::from(k)));
        }
        out.sort_unstable();
        out.dedup();
        out.retain(|&k| self.0.rects[k as usize].is_some_and(|b| b.intersects(r)));
    }
}

fn rects(items: &[Item]) -> Vec<Rect> {
    items.iter().map(|i| i.shape.bbox()).collect()
}

fn run_index<I: Index>(c: &mut Criterion, name: &str, items: &[Item]) {
    let boxes = rects(items);
    let mut group = c.benchmark_group("index");
    group.bench_function(BenchmarkId::new("bulk_load", name), |b| {
        b.iter(|| I::bulk(black_box(&boxes)))
    });
    group.bench_function(BenchmarkId::new("insert", name), |b| {
        b.iter(|| {
            let mut index = I::empty();
            for r in &boxes {
                index.insert(*r);
            }
            index
        })
    });
    let index = I::bulk(&boxes);
    // Around everything, as a DRC does, and then around everything but the ten pours and
    // keepouts, whose windows take up most of the time otherwise.
    for (what, windows) in [
        ("query", &boxes[..]),
        ("query_small", &boxes[..boxes.len() - 10]),
    ] {
        group.bench_function(BenchmarkId::new(what, name), |b| {
            let mut out = Vec::new();
            b.iter(|| {
                let mut found = 0;
                for r in windows {
                    index.query(&r.expand(CLEARANCE), &mut out);
                    found += out.len();
                }
                found
            })
        });
    }
    // Rip up the 300 tracks after the pads and vias, and put each back elsewhere.
    let first_track = items.len() - 3010;
    let mut rng = Rng::new(7);
    let moved: Vec<Rect> = (0..300)
        .map(|_| random_track(&mut rng).shape.bbox())
        .collect();
    group.bench_function(BenchmarkId::new("churn", name), |b| {
        let mut out = Vec::new();
        b.iter_batched_ref(
            // A bulk load numbers entries in order, so the tracks are where they were.
            || I::bulk(&boxes),
            |index| {
                let mut found = 0;
                for (k, r) in moved.iter().enumerate() {
                    index.remove((first_track + k) as u64);
                    index.query(&r.expand(CLEARANCE), &mut out);
                    found += out.len();
                    index.insert(*r);
                }
                found
            },
            criterion::BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn indexes(c: &mut Criterion) {
    let items = board(1);
    // The grid's sizes only fit this board's extent.
    assert!(items.iter().all(|i| {
        let b = i.shape.bbox();
        b.min().x >= -MM && b.max().x <= WIDTH + MM && b.min().y >= -MM && b.max().y <= HEIGHT + MM
    }));
    run_index::<Tree>(c, "rtree", &items);
    run_index::<Cells<{ MM / 4 }>>(c, "grid_0.25mm", &items);
    run_index::<Cells<MM>>(c, "grid_1mm", &items);
    run_index::<Cells<{ 4 * MM }>>(c, "grid_4mm", &items);
}

fn geometry(core: &Core) -> &dyn polyclip::Geometry {
    match core {
        Core::Point(p) => p,
        Core::Segment(s) => s,
        Core::Polygon(p) => p,
    }
}

fn clearances(c: &mut Criterion) {
    let items = board(1);
    let index = SpatialIndex::bulk_load(rects(&items));
    let mut pairs = Vec::new();
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        index.query(&item.shape.bbox().expand(CLEARANCE), &mut out);
        pairs.extend(
            out.iter()
                .map(|id| id.slot() as usize)
                .filter(|&j| j > i)
                .map(|j| (i, j)),
        );
    }
    // Pairs without a polygon, where the filter can answer either way, and all of them.
    let round: Vec<(usize, usize)> = pairs
        .iter()
        .copied()
        .filter(|&(i, j)| {
            !matches!(items[i].core, Core::Polygon(_)) && !matches!(items[j].core, Core::Polygon(_))
        })
        .collect();
    let mut group = c.benchmark_group("clearance");
    for (name, set) in [("round", &round), ("all", &pairs)] {
        group.bench_function(BenchmarkId::new("violates", name), |b| {
            b.iter(|| {
                set.iter()
                    .filter(|&&(i, j)| items[i].shape.violates(&items[j].shape, CLEARANCE).unwrap())
                    .count()
            })
        });
        group.bench_function(BenchmarkId::new("polyclip", name), |b| {
            b.iter(|| {
                set.iter()
                    .filter(|&&(i, j)| {
                        let d = CLEARANCE + items[i].radius + items[j].radius;
                        polyclip::distance_less_than(
                            geometry(&items[i].core),
                            geometry(&items[j].core),
                            d,
                        )
                    })
                    .count()
            })
        });
    }
    group.finish();
    // Both must agree, or the comparison means nothing.
    for &(i, j) in &pairs {
        let d = CLEARANCE + items[i].radius + items[j].radius;
        let exact =
            polyclip::distance_less_than(geometry(&items[i].core), geometry(&items[j].core), d);
        assert_eq!(
            items[i].shape.violates(&items[j].shape, CLEARANCE).unwrap(),
            exact
        );
    }
    eprintln!(
        "clearance pairs: {} round, {} in all, of {} items",
        round.len(),
        pairs.len(),
        items.len()
    );
}

fn configured() -> Criterion {
    Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_millis(1500))
}

criterion_group! {
    name = benches;
    config = configured();
    targets = indexes, clearances
}
criterion_main!(benches);
