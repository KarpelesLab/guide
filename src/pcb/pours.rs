//! What routes cut off the fills that nets are planned on, counted cheaply, during the run.
//!
//! A net with zones is planned on their fill as it is before anything is routed: the pads
//! that the fill joins are one terminal, and need no route. On an outer layer the routes of
//! other nets then go through that fill, and what a host fills again around them leaves
//! some of those pads on a piece of their own, or on none: between two tracks, or behind a
//! via. Filling the zones again to find that out costs seconds a time, and after routing
//! is over it is too late for more than a stub, where everything is fixed copper and the
//! pad is shut in.
//!
//! So the fills are kept as cells of a grid, as fine as the lattice, and from time to time
//! while routes are still moved, the cells too near another net's routes for copper of the
//! fill's least width are taken out, and what is left is told into its connected pieces,
//! through the net's own pads, tracks and vias. A pad of the fill's terminal that is not on
//! the largest piece is *cut off*: its net gets a turn in which it is a terminal of its
//! own, to join to the largest piece again by a stub, as a net routed in part is
//! completed, with the right to move what is in its way.
//!
//! The grid errs on the side of a cut: a neck of fill of just the least width between two
//! routes is not seen, and such a pad gets a stub that it did not need.

use std::collections::VecDeque;

use super::Design;
use super::detail::{NetCtx, TermPad};
use super::geometry::NetCopper;
use super::router::{Job, LATTICE_REACH};
use super::world::{Verdict, World};
use super::zones::Fill;
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::NetId;

/// How many cells a grid has at most on a layer: a board too large for that at the
/// lattice's pitch has coarser cells.
const MAX_CELLS: i64 = 4_000_000;

/// How far from a pad that is cut off a place to join it to its fill again may be, and how
/// many such places a pad has at most.
const REACH: i64 = 4_000_000;
const SITES: usize = 8;

/// The fill of one net on one layer, as cells.
struct Layer {
    layer: LayerId,
    /// By cell, whether its centre is in the net's planned fill, or in the relief of a pad
    /// that the fill joins.
    fill: Vec<bool>,
    /// How far from another net's copper the fill is cut: its clearance, and half its
    /// least width.
    cut: i64,
}

/// The fills of one net.
struct Net {
    net: NetId,
    job: usize,
    layers: Vec<Layer>,
    /// The pads of the fill's terminal that the cells leave off its largest piece with
    /// nothing routed, by position in the job's: joined by a neck that is narrower than
    /// a cell, they are never told to be cut off.
    exempt: Vec<usize>,
    /// The spokes that the host's fill joins pads to it by: for each, the pad's box, the
    /// layer by position in `layers`, and the cells along it, the last in the fill. A pad
    /// with any on a layer is joined to its fill there by those that no route crosses,
    /// and by nothing else.
    spokes: Vec<(Rect, usize, Vec<usize>)>,
}

/// The planned fills of a design, as cells.
pub(crate) struct Pours {
    origin: Point,
    cell: i64,
    cols: usize,
    rows: usize,
    nets: Vec<Net>,
}

fn find(parent: &mut [u32], mut i: u32) -> u32 {
    while parent[i as usize] != i {
        parent[i as usize] = parent[parent[i as usize] as usize];
        i = parent[i as usize];
    }
    i
}

fn union(parent: &mut [u32], a: u32, b: u32) {
    let (x, y) = (find(parent, a), find(parent, b));
    if x != y {
        parent[x.max(y) as usize] = x.min(y);
    }
}

impl Pours {
    /// The fills in `planned` of the nets of `jobs` whose first terminal is their fill's, on
    /// a grid over `bounds`.
    pub(crate) fn plan(
        design: &Design,
        world: &World<'_>,
        planned: &[Fill],
        jobs: &[Job],
        bounds: Rect,
        pitch: i64,
    ) -> Pours {
        let (w, h) = (
            (bounds.max().x - bounds.min().x).max(1),
            (bounds.max().y - bounds.min().y).max(1),
        );
        let mut cell = pitch.max(1000);
        while (w / cell + 1) * (h / cell + 1) > MAX_CELLS {
            cell *= 2;
        }
        let mut pours = Pours {
            origin: bounds.min(),
            cell,
            cols: (w / cell + 1) as usize,
            rows: (h / cell + 1) as usize,
            nets: Vec::new(),
        };
        let rules = &design.rules;
        let zones = &design.zones;
        for (j, job) in jobs.iter().enumerate().filter(|(_, j)| j.plane) {
            let net = job.ctx.net;
            let mut layers: Vec<Layer> = Vec::new();
            let mut spokes: Vec<(Rect, usize, Vec<usize>)> = Vec::new();
            for f in planned {
                let Some(zone) = design
                    .zones
                    .zones
                    .get(f.zone)
                    .filter(|z| z.net == Some(net))
                else {
                    continue;
                };
                let at = match layers.iter().position(|l| l.layer == f.layer) {
                    Some(at) => at,
                    None => {
                        layers.push(Layer {
                            layer: f.layer,
                            fill: vec![false; pours.cols * pours.rows],
                            cut: 0,
                        });
                        layers.len() - 1
                    }
                };
                let clearance = zone.clearance.max(rules.clearance(Some(net), f.layer));
                layers[at].cut = layers[at].cut.max(clearance + zone.min_width / 2);
                pours.raster(&f.shape, &mut layers[at].fill);
                // The relief of a pad that the fill joins is crossed by its spokes, which
                // cells are too coarse to see.
                let gap = zone.thermal_gap.max(0) + cell;
                for p in job.ctx.pads.iter().filter(|p| own_pad(p)) {
                    if !(p.layers.contains(f.layer) && f.shape.bbox().intersects(&p.bbox)) {
                        continue;
                    }
                    // A host that fills its zones says where the spokes are: they are
                    // then what joins the pad, each for as long as nothing crosses it.
                    let mut any = spokes.iter().any(|(b, l, _)| *b == p.bbox && *l == at);
                    for s in zones.spokes.iter().chain(&zones.spare_spokes) {
                        let own =
                            s.layer == f.layer
                                && design.board.pads.get(s.pad).is_some_and(|q| {
                                    q.net == Some(net) && q.shape.bbox() == p.bbox
                                });
                        if !own
                            || spokes.iter().any(|(b, l, c)| {
                                *b == p.bbox && *l == at && pours.cell_of(s.to) == c.last().copied()
                            })
                        {
                            continue;
                        }
                        let (dx, dy) = (s.to.x - s.from.x, s.to.y - s.from.y);
                        let steps = (dx.abs().max(dy.abs()) / (cell / 2).max(1)).max(1);
                        let mut cells: Vec<usize> = Vec::new();
                        for k in 0..=steps {
                            let q =
                                Point::new(s.from.x + dx * k / steps, s.from.y + dy * k / steps);
                            if let Some(c) = pours.cell_of(q)
                                && cells.last() != Some(&c)
                            {
                                cells.push(c);
                            }
                        }
                        if cells.is_empty() {
                            continue;
                        }
                        for &c in &cells {
                            layers[at].fill[c] = true;
                        }
                        spokes.push((p.bbox, at, cells));
                        any = true;
                    }
                    if !any {
                        pours.each_cell(&p.bbox.expand(gap), |k| layers[at].fill[k] = true);
                    }
                }
            }
            if !layers.is_empty() {
                pours.nets.push(Net {
                    net,
                    job: j,
                    layers,
                    exempt: Vec::new(),
                    spokes,
                });
            }
        }
        // What the cells leave apart before anything is routed is their own coarseness.
        let nothing: Vec<Option<NetCopper>> = vec![None; jobs.len()];
        for at in 0..pours.nets.len() {
            let job = pours.nets[at].job;
            let ctx = &jobs[job].ctx;
            if let Some(again) = pours.rejoin(world, design, jobs, &nothing, job, pitch) {
                // The pads of the fill's terminal are in the order of the job's.
                let own = ctx.pads.iter().enumerate().filter(|(_, p)| own_pad(p));
                let theirs = again.ctx.pads.iter().filter(|p| own_pad(p));
                pours.nets[at].exempt = own
                    .zip(theirs)
                    .filter(|((_, p), q)| p.terminal == 0 && q.terminal != 0)
                    .map(|((i, _), _)| i)
                    .collect();
            }
        }
        pours
    }

    /// The jobs with fills.
    pub(crate) fn jobs(&self) -> impl Iterator<Item = usize> + '_ {
        self.nets.iter().map(|n| n.job)
    }

    /// The cell that holds a point, if the grid does.
    fn cell_of(&self, p: Point) -> Option<usize> {
        let (x, y) = (
            (p.x - self.origin.x).div_euclid(self.cell),
            (p.y - self.origin.y).div_euclid(self.cell),
        );
        (x >= 0 && y >= 0 && (x as usize) < self.cols && (y as usize) < self.rows)
            .then(|| y as usize * self.cols + x as usize)
    }

    /// The centre of a cell.
    fn center(&self, k: usize) -> Point {
        let (x, y) = ((k % self.cols) as i64, (k / self.cols) as i64);
        Point::new(
            self.origin.x + x * self.cell + self.cell / 2,
            self.origin.y + y * self.cell + self.cell / 2,
        )
    }

    /// Calls `f` with each cell whose centre is in `r`.
    fn each_cell(&self, r: &Rect, mut f: impl FnMut(usize)) {
        let half = self.cell / 2;
        let lo = |v: i64, o: i64| (v - o - half + self.cell - 1).div_euclid(self.cell).max(0);
        let hi = |v: i64, o: i64, n: usize| (v - o - half).div_euclid(self.cell).min(n as i64 - 1);
        let (x0, x1) = (
            lo(r.min().x, self.origin.x),
            hi(r.max().x, self.origin.x, self.cols),
        );
        let (y0, y1) = (
            lo(r.min().y, self.origin.y),
            hi(r.max().y, self.origin.y, self.rows),
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                f(y as usize * self.cols + x as usize);
            }
        }
    }

    /// Sets the cells whose centres are in a polygon with holes: by row, between the
    /// crossings of its edges, an odd number of which are left of a point inside.
    fn raster(&self, shape: &Shape, cells: &mut [bool]) {
        let rings: Vec<Vec<Point>> = match shape.rings() {
            Some(rings) => rings,
            None => match shape.outline(1000) {
                Ok(polygons) => polygons.into_iter().flatten().collect(),
                Err(_) => return,
            },
        };
        let half = self.cell / 2;
        let mut crossings: Vec<Vec<i64>> = vec![Vec::new(); self.rows];
        for ring in &rings {
            for i in 0..ring.len() {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                if a.y == b.y {
                    continue;
                }
                let (lo, hi) = if a.y < b.y { (a, b) } else { (b, a) };
                // The rows whose centre line the edge crosses: from its lower end, which
                // it holds, to its upper, which it does not.
                let first = (lo.y - self.origin.y - half + self.cell - 1)
                    .div_euclid(self.cell)
                    .max(0);
                let last = (hi.y - self.origin.y - half - 1)
                    .div_euclid(self.cell)
                    .min(self.rows as i64 - 1);
                for row in first..=last {
                    let y = self.origin.y + row * self.cell + half;
                    let x = lo.x as i128
                        + (hi.x - lo.x) as i128 * (y - lo.y) as i128 / (hi.y - lo.y) as i128;
                    crossings[row as usize].push(x as i64);
                }
            }
        }
        for (row, xs) in crossings.iter_mut().enumerate() {
            xs.sort_unstable();
            for pair in xs.as_chunks::<2>().0 {
                let first = (pair[0] - self.origin.x - half + self.cell - 1)
                    .div_euclid(self.cell)
                    .max(0);
                let last = (pair[1] - self.origin.x - half)
                    .div_euclid(self.cell)
                    .min(self.cols as i64 - 1);
                for x in first..=last {
                    cells[row * self.cols + x as usize] = true;
                }
            }
        }
    }

    /// Clears the cells whose centres are within `reach` of the segment from `a` to `b`.
    fn cut(&self, cells: &mut [bool], a: Point, b: Point, reach: i64) {
        let r = Rect::new(a, b).expand(reach);
        let (ux, uy) = ((b.x - a.x) as i128, (b.y - a.y) as i128);
        let len2 = ux * ux + uy * uy;
        let reach2 = reach as i128 * reach as i128;
        self.each_cell(&r, |k| {
            let p = self.center(k);
            let (vx, vy) = ((p.x - a.x) as i128, (p.y - a.y) as i128);
            let dot = vx * ux + vy * uy;
            // The square of the distance to the segment, times `len2` where the foot of
            // the point is inside it.
            let near = if len2 == 0 || dot <= 0 {
                vx * vx + vy * vy <= reach2
            } else if dot >= len2 {
                let (wx, wy) = ((p.x - b.x) as i128, (p.y - b.y) as i128);
                wx * wx + wy * wy <= reach2
            } else {
                let cross = vx * uy - vy * ux;
                cross * cross <= reach2 * len2
            };
            if near {
                cells[k] = false;
            }
        });
    }

    /// The net of job `job` as one to join up again, if routes have cut pads off its
    /// fill: its pads that are cut off as terminals of their own, one for each piece of
    /// fill they are left on, after the terminals it has, and the places of its first
    /// terminal in what is left of the fill's largest piece, with new ones near the pads
    /// cut off. `None` if no pad is cut off.
    pub(crate) fn rejoin(
        &self,
        world: &World<'_>,
        design: &Design,
        jobs: &[Job],
        copper: &[Option<NetCopper>],
        job: usize,
        pitch: i64,
    ) -> Option<Rejoin> {
        let net = self.nets.iter().find(|n| n.job == job)?;
        let ctx = &jobs[job].ctx;
        let cells = self.cols * self.rows;
        // The fill that other nets' copper leaves, by layer.
        let mut open: Vec<Vec<bool>> = net.layers.iter().map(|l| l.fill.clone()).collect();
        for (at, layer) in net.layers.iter().enumerate() {
            let l = usize::from(layer.layer.0);
            for (k, other) in jobs.iter().enumerate() {
                let Some(c) = copper.get(k).and_then(Option::as_ref).filter(|_| k != job) else {
                    continue;
                };
                let width = other.ctx.widths.get(l).copied().unwrap_or(0);
                for s in c.segs.iter().filter(|s| usize::from(s.l) == l) {
                    self.cut(&mut open[at], s.a, s.b, width / 2 + layer.cut);
                }
                if let Some((d, _)) = other.ctx.via {
                    for &v in &c.vias {
                        self.cut(&mut open[at], v, v, d / 2 + layer.cut);
                    }
                }
            }
        }
        // The pieces of each layer's fill, numbered over all layers from 1.
        let mut piece: Vec<Vec<u32>> = vec![vec![0; cells]; net.layers.len()];
        let mut sizes: Vec<u64> = vec![0];
        let mut queue: VecDeque<usize> = VecDeque::new();
        for (at, open) in open.iter().enumerate() {
            for start in 0..cells {
                if !open[start] || piece[at][start] != 0 {
                    continue;
                }
                let id = sizes.len() as u32;
                sizes.push(0);
                piece[at][start] = id;
                queue.push_back(start);
                while let Some(k) = queue.pop_front() {
                    sizes[id as usize] += 1;
                    let (x, y) = (k % self.cols, k / self.cols);
                    let mut visit = |n: usize| {
                        if open[n] && piece[at][n] == 0 {
                            piece[at][n] = id;
                            queue.push_back(n);
                        }
                    };
                    if x > 0 {
                        visit(k - 1);
                    }
                    if x + 1 < self.cols {
                        visit(k + 1);
                    }
                    if y > 0 {
                        visit(k - self.cols);
                    }
                    if y + 1 < self.rows {
                        visit(k + self.cols);
                    }
                }
            }
        }
        // What joins the pieces: the pads of the fill's terminal, each to the pieces at
        // it, and the net's own copper, each connected part of it to the pieces that its
        // vias and its tracks' ends are in and to the pads that they are at. The other
        // terminals are joined by that copper alone.
        let pieces = sizes.len() as u32;
        let own: Vec<usize> = (0..ctx.pads.len())
            .filter(|&i| ctx.pads[i].terminal == 0 && own_pad(&ctx.pads[i]))
            .collect();
        let empty = NetCopper::default();
        let routes = copper.get(job).and_then(Option::as_ref).unwrap_or(&empty);
        // The copper's parts by its tracks and vias alone: with the net's pads, every
        // stub to a place in the fill would be one part with the fill's terminal.
        let bare = NetCtx::new(ctx.net, ctx.widths.clone(), ctx.via, Vec::new(), 0, 0);
        let parts = routes.pieces(&bare);
        let pad_node = |i: usize| pieces + i as u32;
        let terminal_node = |t: usize| pieces + (own.len() + t) as u32;
        let part_node = |p: usize| pieces + (own.len() + ctx.terminals + p) as u32;
        let nodes = pieces as usize + own.len() + ctx.terminals + parts.count;
        let mut parent: Vec<u32> = (0..nodes as u32).collect();
        let at_layer = |l: LayerId| net.layers.iter().position(|x| x.layer == l);
        let piece_at =
            |at: usize, p: Point| self.cell_of(p).map(|k| piece[at][k]).filter(|&id| id != 0);
        for (i, &p) in own.iter().enumerate() {
            let pad = &ctx.pads[p];
            for l in pad.layers.iter() {
                let Some(at) = at_layer(l) else { continue };
                // By its spokes, where the host gave them: those with every cell left.
                let mut spoked = false;
                for (_, _, cells) in net
                    .spokes
                    .iter()
                    .filter(|(b, l, _)| *b == pad.bbox && *l == at)
                {
                    spoked = true;
                    if let Some(&last) = cells.last()
                        && cells.iter().all(|&c| open[at][c])
                        && piece[at][last] != 0
                    {
                        union(&mut parent, pad_node(i), piece[at][last]);
                    }
                }
                if spoked {
                    continue;
                }
                self.each_cell(&pad.bbox.expand(self.cell), |k| {
                    if piece[at][k] != 0 {
                        union(&mut parent, pad_node(i), piece[at][k]);
                    }
                });
            }
        }
        let touch = |parent: &mut Vec<u32>, node: u32, at: Point, layers: LayerSet| {
            for l in layers.iter() {
                if let Some(id) = at_layer(l).and_then(|a| piece_at(a, at)) {
                    union(parent, node, id);
                }
            }
            for (i, p) in ctx.pads.iter().enumerate() {
                if p.center != at || p.layers.0 & layers.0 == 0 {
                    continue;
                }
                if p.terminal != 0 {
                    union(parent, node, terminal_node(p.terminal));
                } else if let Ok(k) = own.binary_search(&i) {
                    union(parent, node, pad_node(k));
                }
            }
        };
        let all = design.rules.layers();
        for (s, &part) in routes.segs.iter().zip(&parts.segs) {
            let on = LayerSet::EMPTY.with(LayerId(s.l));
            touch(&mut parent, part_node(part), s.a, on);
            touch(&mut parent, part_node(part), s.b, on);
        }
        for (&v, &part) in routes.vias.iter().zip(&parts.vias) {
            touch(&mut parent, part_node(part), v, all);
        }
        // The net's fixed vias join the layers where they are.
        for v in design.board.vias.iter().filter(|v| v.net == Some(net.net)) {
            let ids: Vec<u32> = (0..net.layers.len())
                .filter_map(|at| piece_at(at, v.at))
                .collect();
            for w in ids.windows(2) {
                union(&mut parent, w[0], w[1]);
            }
        }
        // The largest piece, with everything joined to it.
        let mut total: Vec<u64> = vec![0; pieces as usize];
        for id in 1..pieces {
            let root = find(&mut parent, id);
            if root < pieces {
                total[root as usize] += sizes[id as usize];
            }
        }
        let main = (1..pieces).max_by_key(|&id| (total[id as usize], core::cmp::Reverse(id)))?;
        let main = find(&mut parent, main);
        // What is cut off, by the piece of fill it is left on, each piece a terminal to
        // join to the largest: the pads of the fill's terminal that are not on the
        // largest piece, and the other terminals that the net's copper joins to the
        // fill's as the run counts them, by a stub to a place in the fill that is on
        // another piece now.
        let mut groups: Vec<(u32, Vec<usize>)> = Vec::new();
        let group_of = |root: u32, groups: &mut Vec<(u32, Vec<usize>)>| match groups
            .iter()
            .position(|(r, _)| *r == root)
        {
            Some(at) => at,
            None => {
                groups.push((root, Vec::new()));
                groups.len() - 1
            }
        };
        for (i, &p) in own.iter().enumerate() {
            let root = find(&mut parent, pad_node(i));
            if root != main && !net.exempt.contains(&p) {
                let at = group_of(root, &mut groups);
                groups[at].1.push(p);
            }
        }
        let counted = routes.pieces(ctx);
        let stale: Vec<usize> = (1..ctx.terminals)
            .filter(|&t| {
                counted.terminals[t] == counted.terminals[0]
                    && find(&mut parent, terminal_node(t)) != main
            })
            .collect();
        for &t in &stale {
            // One that ends on no piece of fill at all is a terminal by itself, as it is.
            let root = find(&mut parent, terminal_node(t));
            if root < pieces {
                group_of(root, &mut groups);
            }
        }
        if groups.is_empty() && stale.is_empty() {
            return None;
        }
        let in_main = |parent: &mut Vec<u32>, at: usize, p: Point| {
            piece_at(at, p).is_some_and(|id| find(parent, id) == main)
        };
        let mut pads: Vec<TermPad> = Vec::new();
        for (i, p) in ctx.pads.iter().enumerate() {
            if p.terminal != 0 {
                pads.push(p.clone());
            } else if own_pad(p) {
                let group = groups.iter().position(|(_, list)| list.contains(&i));
                pads.push(TermPad {
                    terminal: group.map_or(0, |g| ctx.terminals + g),
                    ..p.clone()
                });
            } else {
                // A place in the fill is of the piece it is on now: the largest, or one
                // that is cut off with what ends there, and of none if it is on neither.
                let root = p
                    .layers
                    .iter()
                    .find_map(|l| at_layer(l).and_then(|at| piece_at(at, p.center)))
                    .map(|id| find(&mut parent, id));
                let terminal = match root {
                    Some(r) if r == main => Some(0),
                    Some(r) => groups
                        .iter()
                        .position(|(g, _)| *g == r)
                        .map(|g| ctx.terminals + g),
                    None => None,
                };
                if let Some(terminal) = terminal {
                    pads.push(TermPad {
                        terminal,
                        ..p.clone()
                    });
                }
            }
        }
        // Places in the largest piece near the pads cut off, nearest first: where a
        // track's end fits on a layer of the pad, or a via on another.
        let n = REACH / self.cell;
        let mut offsets: Vec<(i64, i64, i64)> = Vec::new();
        for i in -n..=n {
            for j in -n..=n {
                if i * i + j * j <= n * n {
                    offsets.push((i * i + j * j, i, j));
                }
            }
        }
        offsets.sort_unstable();
        let mut sites: Vec<TermPad> = Vec::new();
        let lost = groups
            .iter()
            .flat_map(|(_, list)| list.iter().copied())
            .chain((0..ctx.pads.len()).filter(|&i| stale.contains(&ctx.pads[i].terminal)))
            .collect::<Vec<usize>>();
        for p in lost {
            let pad = &ctx.pads[p];
            let Some(k) = self.cell_of(pad.center).filter(|_| own_pad(pad)) else {
                continue;
            };
            let (x, y) = ((k % self.cols) as i64, (k / self.cols) as i64);
            let mut found = 0;
            for &(_, i, j) in &offsets {
                let (cx, cy) = (x + i, y + j);
                if cx < 0 || cy < 0 || cx >= self.cols as i64 || cy >= self.rows as i64 {
                    continue;
                }
                let q = self.center(cy as usize * self.cols + cx as usize);
                for (at, layer) in net.layers.iter().enumerate() {
                    let known = |s: &TermPad| {
                        s.terminal == 0 && s.center == q && s.layers.contains(layer.layer)
                    };
                    if !in_main(&mut parent, at, q) || pads.iter().chain(&sites).any(known) {
                        continue;
                    }
                    let fits = if pad.layers.contains(layer.layer) {
                        Shape::circle(q, ctx.widths[usize::from(layer.layer.0)]).is_ok_and(|end| {
                            world.track(ctx.net, layer.layer, &end, None) == Verdict::Free
                        })
                    } else {
                        ctx.via.is_some_and(|(d, drill)| {
                            world.via(ctx.net, q, d, drill, None) == Verdict::Free
                        })
                    };
                    if fits {
                        sites.push(TermPad {
                            center: q,
                            layers: LayerSet::EMPTY.with(layer.layer),
                            bbox: Rect::new(q, q),
                            terminal: 0,
                        });
                        found += 1;
                    }
                }
                if found >= SITES {
                    break;
                }
            }
        }
        pads.extend(sites.iter().cloned());
        if !pads.iter().any(|p| p.terminal == 0) {
            return None;
        }
        // A piece that nothing of the net's is known to end on is no terminal: the ones
        // that are, are numbered after the net's own.
        let mut used: Vec<usize> = pads
            .iter()
            .map(|p| p.terminal)
            .filter(|&t| t >= ctx.terminals)
            .collect();
        used.sort_unstable();
        used.dedup();
        for p in &mut pads {
            if let Ok(at) = used.binary_search(&p.terminal) {
                p.terminal = ctx.terminals + at;
            }
        }
        let again = NetCtx::new(
            ctx.net,
            ctx.widths.clone(),
            ctx.via,
            pads,
            ctx.terminals + used.len(),
            LATTICE_REACH * pitch,
        )
        .on_lattice(ctx.lattice());
        Some(Rejoin {
            ctx: again,
            sites,
            cut: used.len().max(1),
        })
    }
}

/// A net to join up again: as [`Pours::rejoin`] has it, the places in its fill that are
/// new, which its routes may end at from now on, and how many pieces of its fill with pads
/// or stubs of its on them are cut off the largest.
pub(crate) struct Rejoin {
    pub(crate) ctx: NetCtx,
    pub(crate) sites: Vec<TermPad>,
    pub(crate) cut: usize,
}

/// The same net with more places of its first terminal.
pub(crate) fn with_sites(ctx: &NetCtx, sites: &[TermPad], pitch: i64) -> NetCtx {
    let mut pads = ctx.pads.clone();
    pads.extend(sites.iter().cloned());
    NetCtx::new(
        ctx.net,
        ctx.widths.clone(),
        ctx.via,
        pads,
        ctx.terminals,
        LATTICE_REACH * pitch,
    )
    .on_lattice(ctx.lattice())
}

/// Whether a place where a net's routes end is a pad, and not a point: the end of a fixed
/// track, a via, or a place in a fill.
fn own_pad(p: &TermPad) -> bool {
    p.bbox.min() != p.bbox.max()
}

#[cfg(test)]
mod tests {
    use super::super::geometry::{NetCopper, Seg};
    use super::super::tests::{MM, design, route_checked, smd};
    use super::super::world::World;
    use super::super::zone_tests::zone;
    use super::super::{Design, RouteOptions, fill_zones, planes, router};
    use super::Pours;
    use crate::geom::{Point, Rect};
    use crate::id::NetId;

    /// A strip of fill of net 1 on the top layer, 20 mm by 4, with two of the net's pads
    /// on it, at 3 and 14.5 mm, and one off it, above; and two pads of net 2, below the
    /// strip and above it at 12 mm, which has no via.
    fn strip() -> Design {
        let pads = vec![
            smd(3 * MM, 2 * MM, 1),
            smd(14_500_000, 2 * MM, 1),
            smd(5 * MM, 8 * MM, 1),
            smd(12 * MM, -3 * MM, 2),
            smd(12 * MM, 7 * MM, 2),
        ];
        let mut d = design(2, pads, 3);
        let mut z = zone(1, 0, 4 * MM);
        z.outline[1].x = 20 * MM;
        z.outline[2].x = 20 * MM;
        // A plane, though it is a pour between three pads.
        z.plane = Some(true);
        d.zones.zones.push(z);
        d.nets[2].via = None;
        d
    }

    #[test]
    fn a_route_through_a_fill_cuts_the_pads_behind_it_off() {
        let d = strip();
        let options = RouteOptions::default();
        let fills = fill_zones(&d.board, &d).unwrap();
        let mut grouped = d.clone();
        grouped
            .board
            .zones
            .extend(fills.iter().map(|f| f.to_zone(&d)));
        let pieces = planes::pieces_by_net(&d, &fills);
        let mut world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )
        .unwrap();
        world.configure(&d, &options);
        let pitch = router::pitch(&d, &options);
        let jobs = router::jobs(&grouped, &world, pitch, &pieces, &[]);
        let ground = jobs.iter().position(|j| j.ctx.net == NetId(1)).unwrap();
        let other = jobs.iter().position(|j| j.ctx.net == NetId(2)).unwrap();
        assert!(jobs[ground].plane);
        let bounds = Rect::new(Point::new(-5 * MM, -5 * MM), Point::new(25 * MM, 12 * MM));
        let pours = Pours::plan(&d, &world, &fills, &jobs, bounds, pitch);
        assert_eq!(pours.jobs().collect::<Vec<_>>(), [ground]);
        // Nothing is routed: nothing is cut off.
        let mut copper: Vec<Option<NetCopper>> = vec![None; jobs.len()];
        let rejoin =
            |copper: &[Option<NetCopper>]| pours.rejoin(&world, &d, &jobs, copper, ground, pitch);
        assert!(rejoin(&copper).is_none());
        // A track of the other net half way into the strip leaves it in one piece.
        let track = |to: i64| {
            let mut c = NetCopper::default();
            c.segs.push(Seg {
                a: Point::new(12 * MM, -3 * MM),
                b: Point::new(12 * MM, to),
                l: 0,
            });
            Some(c)
        };
        copper[other] = track(MM);
        assert!(rejoin(&copper).is_none());
        // Across it, the pad at 14.5 mm is on the smaller piece: a terminal of its own,
        // with places to join the fill again on the other side of the track.
        copper[other] = track(7 * MM);
        let again = rejoin(&copper).expect("a pad cut off");
        assert_eq!(again.cut, 1);
        let ctx = &jobs[ground].ctx;
        assert_eq!(again.ctx.terminals, ctx.terminals + 1);
        let cut: Vec<Point> = again
            .ctx
            .pads
            .iter()
            .filter(|p| p.terminal == ctx.terminals)
            .map(|p| p.center)
            .collect();
        assert_eq!(cut, [Point::new(14_500_000, 2 * MM)]);
        assert!(!again.sites.is_empty());
        for s in &again.sites {
            assert!(s.center.x < 11_500_000 && s.terminal == 0, "{s:?}");
        }
        // On another layer it cuts nothing.
        copper[other].as_mut().unwrap().segs[0].l = 1;
        assert!(rejoin(&copper).is_none());
    }

    #[test]
    fn a_pad_is_joined_by_the_spokes_that_its_host_gives_and_by_nothing_else() {
        // The strip's pad at 3 mm, with the two spokes that a host's fill makes for it,
        // to the left and to the right, and a spare one upwards.
        let mut d = strip();
        let spoke = |dx: i64, dy: i64| super::super::Spoke {
            pad: 0,
            layer: crate::geom::LayerId(0),
            from: Point::new(3 * MM + dx * 300_000, 2 * MM + dy * 300_000),
            to: Point::new(3 * MM + dx * 1_100_000, 2 * MM + dy * 1_100_000),
            width: 300_000,
        };
        d.zones.spokes = vec![spoke(-1, 0), spoke(1, 0)];
        let options = RouteOptions::default();
        let fills = fill_zones(&d.board, &d).unwrap();
        let mut grouped = d.clone();
        grouped
            .board
            .zones
            .extend(fills.iter().map(|f| f.to_zone(&d)));
        let pieces = planes::pieces_by_net(&d, &fills);
        let mut world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )
        .unwrap();
        world.configure(&d, &options);
        let pitch = router::pitch(&d, &options);
        let jobs = router::jobs(&grouped, &world, pitch, &pieces, &[]);
        let ground = jobs.iter().position(|j| j.ctx.net == NetId(1)).unwrap();
        let other = jobs.iter().position(|j| j.ctx.net == NetId(2)).unwrap();
        let bounds = Rect::new(Point::new(-5 * MM, -5 * MM), Point::new(25 * MM, 12 * MM));
        // Tracks of the other net across each spoke, close to the pad, and no further:
        // the fill is whole beside them, above the pad and below it.
        let across = |xs: &[i64]| {
            let mut c = NetCopper::default();
            for &x in xs {
                c.segs.push(Seg {
                    a: Point::new(x, 1_600_000),
                    b: Point::new(x, 2_400_000),
                    l: 0,
                });
            }
            Some(c)
        };
        let cut = |d: &Design, xs: &[i64]| {
            let pours = Pours::plan(d, &world, &fills, &jobs, bounds, pitch);
            let mut copper: Vec<Option<NetCopper>> = vec![None; jobs.len()];
            copper[other] = across(xs);
            pours
                .rejoin(&world, d, &jobs, &copper, ground, pitch)
                .map(|r| r.cut)
        };
        // One spoke crossed leaves the other; both crossed, the pad is cut off, though
        // the fill is beside it.
        assert_eq!(cut(&d, &[3_800_000]), None);
        assert_eq!(cut(&d, &[2_200_000, 3_800_000]), Some(1));
        // With a spare spoke that nothing crosses, it is still joined.
        let mut spare = d.clone();
        spare.zones.spare_spokes = vec![spoke(0, 1)];
        assert_eq!(cut(&spare, &[2_200_000, 3_800_000]), None);
        // And a host that gives no spokes has the pad joined by the fill beside it.
        let mut none = d.clone();
        none.zones.spokes.clear();
        assert_eq!(cut(&none, &[2_200_000, 3_800_000]), None);
    }

    #[test]
    fn a_run_with_a_budget_joins_a_pad_cut_off_its_fill_again() {
        let d = strip();
        // Counted on the fill of the routed board, with nothing joined after routing.
        let options = RouteOptions {
            work_budget: Some(50_000_000),
            join: false,
            ..RouteOptions::default()
        };
        let routed = route_checked(&d, &options);
        assert!(routed.stats.pour_cuts >= 1, "{:?}", routed.stats);
        assert!(routed.unrouted.is_empty(), "{:?}", routed.unrouted);
        // Without the count, the pad behind the other net's track is left on its piece.
        let blind = RouteOptions {
            pour_cuts: false,
            ..options
        };
        let routed = route_checked(&d, &blind);
        assert_eq!(routed.stats.pour_cuts, 0);
        assert_eq!(routed.unrouted, [NetId(1)]);
    }
}
