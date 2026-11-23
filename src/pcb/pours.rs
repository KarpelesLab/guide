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
use super::control::Run;
use super::detail::{NetCtx, TermPad};
use super::geometry::NetCopper;
use super::planes::{self, Piece};
use super::router::{Job, LATTICE_REACH, copper_tracks, copper_vias};
use super::world::{Verdict, World};
use super::zones::Fill;
use crate::geom::drc::{Track, Via};
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

/// The host's own fills around the routes as they were when it was asked
/// ([`RunControl::refill`](super::RunControl::refill)): by net with a planned fill, the
/// pieces on the layers of that fill.
pub(crate) struct Refilled {
    nets: Vec<(NetId, Vec<Piece>)>,
    /// Every fill as the host gave it.
    pub(crate) fills: Vec<Fill>,
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
            if let Some(again) = pours.rejoin(world, design, jobs, &nothing, job, pitch, None, &[])
            {
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

    /// The host's fills around the routes `copper` of `jobs` and the tracks and vias
    /// `fixed`, which are the copper that the run added to the design as its own: `None`
    /// if the run has no host to ask, or the host has none to give.
    pub(crate) fn refill(
        &self,
        run: &Run<'_>,
        design: &Design,
        jobs: &[Job],
        copper: &[Option<NetCopper>],
        fixed: (&[Track], &[Via]),
        last: bool,
    ) -> Option<Refilled> {
        if !run.refills() || self.nets.is_empty() {
            return None;
        }
        let (mut tracks, mut vias) = (fixed.0.to_vec(), fixed.1.to_vec());
        for (job, c) in jobs.iter().zip(copper) {
            if let Some(c) = c {
                tracks.extend(copper_tracks(&job.ctx, c));
                vias.extend(copper_vias(design, &job.ctx, c));
            }
        }
        let fills = run.refill(&tracks, &vias, last)?;
        // Of the nets planned on fills, on the layers of those.
        let planned: Vec<Fill> = fills
            .iter()
            .filter(|f| {
                let net = design.zones.zones.get(f.zone).and_then(|z| z.net);
                self.nets
                    .iter()
                    .any(|n| Some(n.net) == net && n.layers.iter().any(|l| l.layer == f.layer))
            })
            .cloned()
            .collect();
        Some(Refilled {
            nets: planes::pieces_by_net(design, &planned),
            fills,
        })
    }

    /// The copper `c` of the net of job `job` without what the host's fills `real` make
    /// needless of it ([`needless`]), and what that is. `None` if nothing is.
    pub(crate) fn without_needless(
        &self,
        design: &Design,
        jobs: &[Job],
        job: usize,
        c: &NetCopper,
        real: &Refilled,
    ) -> Option<(NetCopper, NetCopper)> {
        let net = self.nets.iter().find(|n| n.job == job)?;
        let ctx = &jobs[job].ctx;
        let widths: Vec<i64> = c
            .segs
            .iter()
            .map(|s| ctx.widths.get(usize::from(s.l)).copied().unwrap_or(0))
            .collect();
        let diameters = vec![ctx.via.map_or(0, |(d, _)| d); c.vias.len()];
        let gone = needless(
            design,
            net.net,
            c,
            (&widths, &diameters),
            &real.fills,
            false,
        )?;
        let ns = c.segs.len();
        let (mut out, mut taken) = (c.detached(), c.detached());
        out.keep.clear();
        taken.keep.clear();
        let mut k = 0;
        out.segs.retain(|_| {
            k += 1;
            !gone[k - 1]
        });
        let mut k = 0;
        out.vias.retain(|_| {
            k += 1;
            !gone[ns + k - 1]
        });
        let mut k = 0;
        taken.segs.retain(|_| {
            k += 1;
            gone[k - 1]
        });
        let mut k = 0;
        taken.vias.retain(|_| {
            k += 1;
            gone[ns + k - 1]
        });
        Some((out, taken))
    }

    /// Whether any net is planned on a fill of an outer layer, which routes cut: the
    /// fills of inner layers are planes, which few routes cross, and what the cells tell
    /// of them is near enough.
    pub(crate) fn outer(&self, design: &Design) -> bool {
        let layers = design.rules.layer_count();
        self.nets
            .iter()
            .any(|n| n.layers.iter().any(|l| !planes::inner(l.layer, layers)))
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
        self.each_near(a, b, reach, |k| cells[k] = false);
    }

    /// Calls `f` with each cell whose centre is within `reach` of the segment from `a` to
    /// `b`.
    fn each_near(&self, a: Point, b: Point, reach: i64, mut f: impl FnMut(usize)) {
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
                f(k);
            }
        });
    }

    /// The cells of each layer of `net`'s fill that the copper of the other jobs leaves it,
    /// those of the jobs `skip` taken as not there; and with `owners`, by layer and cell, which
    /// job's copper is too near a cell for the fill to have it: its number plus one, or
    /// `u32::MAX` for more than one, or zero for none.
    fn left_open(
        &self,
        net: &Net,
        jobs: &[Job],
        copper: &[Option<NetCopper>],
        skip: &[usize],
        mut owners: Option<&mut Vec<Vec<u32>>>,
        frees: Option<&dyn Fn(usize, usize) -> bool>,
    ) -> Vec<Vec<bool>> {
        let mut open: Vec<Vec<bool>> = net.layers.iter().map(|l| l.fill.clone()).collect();
        // The fill that a host gave is around the routes as they were when it made it:
        // the cells that those of `skip` took of it then are the fill's again where
        // `frees` says that nothing else has them.
        if let Some(frees) = frees {
            for (at, layer) in net.layers.iter().enumerate() {
                let l = usize::from(layer.layer.0);
                for &k in skip {
                    let (Some(other), Some(c)) =
                        (jobs.get(k), copper.get(k).and_then(Option::as_ref))
                    else {
                        continue;
                    };
                    let width = other.ctx.widths.get(l).copied().unwrap_or(0);
                    let segs = c.segs.iter().filter(|s| usize::from(s.l) == l);
                    let cuts = segs.map(|s| (s.a, s.b, width / 2 + layer.cut));
                    let vias = other.ctx.via.map(|(d, _)| d / 2 + layer.cut);
                    let vias = c.vias.iter().filter_map(|&v| vias.map(|r| (v, v, r)));
                    for (a, b, reach) in cuts.chain(vias) {
                        self.each_near(a, b, reach, |c| {
                            if !open[at][c] && frees(at, c) {
                                open[at][c] = true;
                            }
                        });
                    }
                }
            }
        }
        if let Some(owners) = owners.as_deref_mut() {
            *owners = vec![vec![0; self.cols * self.rows]; net.layers.len()];
        }
        for (at, layer) in net.layers.iter().enumerate() {
            let l = usize::from(layer.layer.0);
            for (k, other) in jobs.iter().enumerate() {
                let Some(c) = copper
                    .get(k)
                    .and_then(Option::as_ref)
                    .filter(|_| k != net.job && !skip.contains(&k))
                else {
                    continue;
                };
                let width = other.ctx.widths.get(l).copied().unwrap_or(0);
                let segs = c.segs.iter().filter(|s| usize::from(s.l) == l);
                let cuts = segs.map(|s| (s.a, s.b, width / 2 + layer.cut));
                let vias = other.ctx.via.map(|(d, _)| d / 2 + layer.cut);
                let vias = c.vias.iter().filter_map(|&v| vias.map(|r| (v, v, r)));
                for (a, b, reach) in cuts.chain(vias) {
                    let Some(owners) = owners.as_deref_mut() else {
                        self.cut(&mut open[at], a, b, reach);
                        continue;
                    };
                    // The cells that this track or via takes, of those the fill has.
                    self.each_near(a, b, reach, |c| {
                        // A host's fill is around the routes already: a cell near one
                        // is that route's whether the fill had it or not.
                        open[at][c] = false;
                        let mine = k as u32 + 1;
                        owners[at][c] = match owners[at][c] {
                            0 => mine,
                            o if o == mine => mine,
                            _ => u32::MAX,
                        };
                    });
                }
            }
        }
        open
    }

    /// Whether the fill of `net` on its layer `at` would have the cell `c`, which it has
    /// not, were the routes of the nets `but` not there: the cell's centre is in a zone of
    /// the net on that layer, and copper of the fill's least width there keeps its
    /// clearance from everything else in the world.
    fn would_fill(
        &self,
        world: &World<'_>,
        design: &Design,
        net: &Net,
        at: usize,
        c: usize,
        but: &[NetId],
    ) -> bool {
        let layer = &net.layers[at];
        let q = self.center(c);
        let inside = |ring: &[Point]| {
            let mut odd = false;
            for i in 0..ring.len() {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                if (a.y <= q.y) != (b.y <= q.y) {
                    let x = a.x as i128
                        + (b.x - a.x) as i128 * (q.y - a.y) as i128 / (b.y - a.y) as i128;
                    if (q.x as i128) < x {
                        odd = !odd;
                    }
                }
            }
            odd
        };
        let zoned = design.zones.zones.iter().any(|z| {
            z.net == Some(net.net)
                && z.layers.contains(layer.layer)
                && inside(&z.outline)
                && !z.holes.iter().any(|h| inside(h))
        });
        if !zoned {
            return false;
        }
        // As a track's end of the fill's net: the world keeps that net's clearance from
        // it, and the fill's own may be more.
        let own = design.rules.clearance(Some(net.net), layer.layer);
        let across = (2 * (layer.cut - own)).max(1000);
        let Ok(disc) = Shape::circle(q, across) else {
            return false;
        };
        let mut routed = Vec::new();
        match world.track(net.net, layer.layer, &disc, Some(&mut routed)) {
            Verdict::Free => true,
            Verdict::Routed => routed.iter().all(|n| but.contains(n)),
            Verdict::Blocked => false,
        }
    }

    /// The cells on layer `at` of `net`'s fill that a pad of the net is joined to the fill
    /// at, of those in `open`: the ends of its spokes that have every cell, where the host
    /// gave its spokes, and the cells at the pad where it gave none.
    fn joined_at(&self, net: &Net, pad: &TermPad, at: usize, open: &[bool]) -> Vec<usize> {
        let mut out = Vec::new();
        let mut spoked = false;
        for (_, _, cells) in net
            .spokes
            .iter()
            .filter(|(b, l, _)| *b == pad.bbox && *l == at)
        {
            spoked = true;
            if let Some(&last) = cells.last()
                && cells.iter().all(|&c| open[c])
            {
                out.push(last);
            }
        }
        if !spoked {
            self.each_cell(&pad.bbox.expand(self.cell), |k| {
                if open[k] {
                    out.push(k);
                }
            });
        }
        out
    }

    /// The cells of `open` in one piece with those of `from`, by the four cells beside
    /// each: by cell, whether it is.
    fn piece_of(&self, open: &[bool], from: &[usize]) -> Vec<bool> {
        let mut seen = vec![false; open.len()];
        let mut queue: VecDeque<usize> = VecDeque::new();
        for &k in from {
            if open[k] && !seen[k] {
                seen[k] = true;
                queue.push_back(k);
            }
        }
        while let Some(k) = queue.pop_front() {
            for n in self.beside(k) {
                if open[n] && !seen[n] {
                    seen[n] = true;
                    queue.push_back(n);
                }
            }
        }
        seen
    }

    /// The four cells beside a cell, those that the grid has.
    fn beside(&self, k: usize) -> impl Iterator<Item = usize> {
        let (x, y) = (k % self.cols, k / self.cols);
        [
            (x > 0).then(|| k - 1),
            (x + 1 < self.cols).then(|| k + 1),
            (y > 0).then(|| k - self.cols),
            (y + 1 < self.rows).then(|| k + self.cols),
        ]
        .into_iter()
        .flatten()
    }

    /// The largest piece of `open`, by cell whether it is of it.
    fn largest(&self, open: &[bool]) -> Vec<bool> {
        let mut label = vec![0u32; open.len()];
        let mut best = (0usize, 0u32, 0usize);
        let mut queue: VecDeque<usize> = VecDeque::new();
        let mut id = 0;
        for start in 0..open.len() {
            if !open[start] || label[start] != 0 {
                continue;
            }
            id += 1;
            label[start] = id;
            queue.push_back(start);
            let mut size = 0;
            while let Some(k) = queue.pop_front() {
                size += 1;
                for n in self.beside(k) {
                    if open[n] && label[n] == 0 {
                        label[n] = id;
                        queue.push_back(n);
                    }
                }
            }
            if size > best.0 {
                best = (size, id, start);
            }
        }
        label.iter().map(|&l| l != 0 && l == best.1).collect()
    }

    /// The jobs whose routes are around the piece of fill that `pad`, a pad of job `job`'s
    /// net off the largest piece of its fill on its layers, is left on, with the routes
    /// of the jobs `skip` taken as not there: those that took
    /// the cells beside that piece, alone, the one with the most such cells first. The
    /// routes that one of these would have to go another way for the pad to be on its
    /// fill again ([`bridge`](Pours::bridge)).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cutters(
        &self,
        world: &World<'_>,
        design: &Design,
        jobs: &[Job],
        copper: &[Option<NetCopper>],
        job: usize,
        pad: &TermPad,
        skip: &[usize],
    ) -> Vec<usize> {
        let Some(net) = self.nets.iter().find(|n| n.job == job) else {
            return Vec::new();
        };
        let mut owners = Vec::new();
        let but: Vec<NetId> = skip
            .iter()
            .filter_map(|&k| jobs.get(k))
            .map(|j| j.ctx.net)
            .collect();
        let frees = |at: usize, c: usize| self.would_fill(world, design, net, at, c, &but);
        let open = self.left_open(net, jobs, copper, skip, Some(&mut owners), Some(&frees));
        let mut count: std::collections::BTreeMap<usize, usize> = Default::default();
        for (at, layer) in net.layers.iter().enumerate() {
            if !pad.layers.contains(layer.layer) {
                continue;
            }
            // The pad's own cells are of its piece: a pad with every spoke crossed is
            // on none, and the routes around it are those that crossed them.
            let mut from = self.joined_at(net, pad, at, &open[at]);
            let mut around: Vec<usize> = Vec::new();
            if from.is_empty() {
                self.each_cell(&pad.bbox.expand(self.cell), |k| around.push(k));
                for (_, _, cells) in net
                    .spokes
                    .iter()
                    .filter(|(b, l, _)| *b == pad.bbox && *l == at)
                {
                    around.extend(cells.iter().copied());
                }
                from = around.iter().copied().filter(|&k| open[at][k]).collect();
            }
            let piece = self.piece_of(&open[at], &from);
            let edge = (0..piece.len())
                .filter(|&k| piece[k])
                .flat_map(|k| self.beside(k))
                .chain(around);
            let mut seen: Vec<usize> = edge.filter(|&n| !open[at][n]).collect();
            seen.sort_unstable();
            seen.dedup();
            for n in seen {
                let o = owners[at][n];
                if o != 0 && o != u32::MAX {
                    *count.entry(o as usize - 1).or_default() += 1;
                }
            }
        }
        let mut out: Vec<(usize, usize)> = count.into_iter().collect();
        out.sort_by_key(|&(k, n)| (core::cmp::Reverse(n), k));
        out.into_iter().map(|(k, _)| k).collect()
    }

    /// Where the fill of job `job`'s net would join `pad`, a pad of the net that the routes
    /// leave off the largest piece of its fill, to that piece on a layer of the pad, were
    /// the routes of the jobs `other` not there: the layer, the centres of the cells from one
    /// that the pad is joined at, through those that `other` take, to the first of the
    /// largest piece, and how far from them the copper of another net has to stay for
    /// the fill to go that way. `None` if the fill would not join it so.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn bridge(
        &self,
        world: &World<'_>,
        design: &Design,
        jobs: &[Job],
        copper: &[Option<NetCopper>],
        job: usize,
        pad: &TermPad,
        other: &[usize],
    ) -> Option<(LayerId, Vec<Point>, i64)> {
        let net = self.nets.iter().find(|n| n.job == job)?;
        let but: Vec<NetId> = other
            .iter()
            .filter_map(|&k| jobs.get(k))
            .map(|j| j.ctx.net)
            .collect();
        let frees = |at: usize, c: usize| self.would_fill(world, design, net, at, c, &but);
        let with = self.left_open(net, jobs, copper, &[], None, None);
        let without = self.left_open(net, jobs, copper, other, None, Some(&frees));
        let mut best: Option<(LayerId, Vec<Point>, i64)> = None;
        for (at, layer) in net.layers.iter().enumerate() {
            if !pad.layers.contains(layer.layer) {
                continue;
            }
            let from = self.joined_at(net, pad, at, &without[at]);
            if from.is_empty() {
                continue;
            }
            let main = self.largest(&with[at]);
            // The pad is on the largest piece here as the routes are.
            let joined = self.joined_at(net, pad, at, &with[at]);
            if joined.iter().any(|&k| main[k]) {
                return None;
            }
            // The shortest way from the pad's cells to the largest piece, through the
            // cells that the fill has without the other net's routes.
            let mut prev: Vec<u32> = vec![u32::MAX; without[at].len()];
            let mut queue: VecDeque<usize> = VecDeque::new();
            for &k in &from {
                if prev[k] == u32::MAX {
                    prev[k] = k as u32;
                    queue.push_back(k);
                }
            }
            let mut end = None;
            while let Some(k) = queue.pop_front() {
                if main[k] {
                    end = Some(k);
                    break;
                }
                for n in self.beside(k) {
                    if without[at][n] && prev[n] == u32::MAX {
                        prev[n] = k as u32;
                        queue.push_back(n);
                    }
                }
            }
            let Some(mut k) = end else { continue };
            let mut path = vec![self.center(k)];
            while prev[k] as usize != k {
                k = prev[k] as usize;
                path.push(self.center(k));
            }
            path.reverse();
            // And the spoke that took the pad there, if it is one of the host's.
            if let Some((_, _, cells)) = net
                .spokes
                .iter()
                .find(|(b, l, cells)| *b == pad.bbox && *l == at && cells.last() == Some(&k))
            {
                let mut spoke: Vec<Point> = cells.iter().map(|&c| self.center(c)).collect();
                spoke.pop();
                spoke.extend(path);
                path = spoke;
            }
            if best.as_ref().is_none_or(|(_, p, _)| path.len() < p.len()) {
                best = Some((layer.layer, path, layer.cut + self.cell / 2));
            }
        }
        best
    }

    /// The net of job `job` as one to join up again, if routes have cut pads off its
    /// fill: its pads that are cut off as terminals of their own, one for each piece of
    /// fill they are left on, after the terminals it has, and the places of its first
    /// terminal in what is left of the fill's largest piece, with new ones near the pads
    /// cut off. `None` if no pad is cut off.
    ///
    /// With `real`, the host's fills around these routes, the pieces are those, to the
    /// nanometre, and a pad is on the pieces that touch it; with none, they are told by
    /// the cells. `extra` are places in the fill that the net was given since `jobs` were
    /// made, as [`Rejoin::sites`] of an earlier call has them.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) fn rejoin(
        &self,
        world: &World<'_>,
        design: &Design,
        jobs: &[Job],
        copper: &[Option<NetCopper>],
        job: usize,
        pitch: i64,
        real: Option<&Refilled>,
        extra: &[TermPad],
    ) -> Option<Rejoin> {
        let net = self.nets.iter().find(|n| n.job == job)?;
        let with_extra;
        let ctx = if extra.is_empty() {
            &jobs[job].ctx
        } else {
            with_extra = with_sites(&jobs[job].ctx, extra, pitch);
            &with_extra
        };
        let cells = self.cols * self.rows;
        // The host's pieces of this net's fill, if it gave its fills.
        let real: Option<&[Piece]> = real.map(|r| {
            r.nets
                .iter()
                .find(|(n, _)| *n == net.net)
                .map_or(&[][..], |(_, list)| list.as_slice())
        });
        // The fill that other nets' copper leaves, by layer.
        let open: Vec<Vec<bool>> = if real.is_some() {
            Vec::new()
        } else {
            self.left_open(net, jobs, copper, &[], None, None)
        };
        // The pieces of each layer's fill, numbered over all layers from 1.
        let mut piece: Vec<Vec<u32>> = if real.is_some() {
            Vec::new()
        } else {
            vec![vec![0; cells]; net.layers.len()]
        };
        let mut sizes: Vec<u128> = vec![0];
        // The host's pieces are numbered as it gave them.
        if let Some(fills) = real {
            sizes.extend(fills.iter().map(|f| f.size.unsigned_abs()));
        }
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
        let piece_at = |at: usize, p: Point| match real {
            None => self.cell_of(p).map(|k| piece[at][k]).filter(|&id| id != 0),
            Some(fills) => fills
                .iter()
                .position(|f| {
                    f.layer == net.layers[at].layer
                        && f.bbox.intersects(&Rect::new(p, p))
                        && f.holds(p)
                })
                .map(|f| f as u32 + 1),
        };
        // The host's pieces with the pads that each touches, and with each other where
        // the board's own copper of the net joins them.
        if let Some(fills) = real {
            let joined = planes::pieces(&design.board, design, net.net, fills);
            for (pads, theirs) in joined.pads.iter().zip(&joined.fills) {
                let Some(&first) = theirs.first() else {
                    continue;
                };
                for &f in theirs {
                    union(&mut parent, first as u32 + 1, f as u32 + 1);
                }
                for &p in pads {
                    let bbox = design.board.pads[p].shape.bbox();
                    for (i, q) in ctx.pads.iter().enumerate() {
                        if q.bbox != bbox || !own_pad(q) {
                            continue;
                        }
                        if q.terminal != 0 {
                            union(&mut parent, first as u32 + 1, terminal_node(q.terminal));
                        } else if let Ok(k) = own.binary_search(&i) {
                            union(&mut parent, first as u32 + 1, pad_node(k));
                        }
                    }
                }
            }
        }
        for (i, &p) in own.iter().enumerate() {
            if real.is_some() {
                break;
            }
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
        let mut total: Vec<u128> = vec![0; pieces as usize];
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
            if root != main && (real.is_some() || !net.exempt.contains(&p)) {
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

/// A piece of fill with no pad on it counts as one to keep joined if its area is no less
/// than the largest's over this ([`needless`]).
const ISLAND: i128 = 16;

/// Whether a pad has copper on one of `layers` that `copper` touches.
fn pad_touches(pad: &crate::geom::drc::Pad, copper: &Shape, layers: LayerSet) -> bool {
    let near = copper.bbox();
    let touches = |on: LayerSet, shape: &Shape| {
        on.0 & layers.0 != 0 && shape.bbox().intersects(&near) && shape.touches(copper)
    };
    touches(pad.layers, &pad.shape) || pad.stack.iter().any(|(on, shape)| touches(*on, shape))
}

/// What of the copper `c` of `net` the fills `fills` make needless: by track and then by
/// via, whether it is. A via is needless if the pads of the net are joined to each other
/// without it as they are with it, by the fills on every layer, the board's own copper
/// and the rest of `c`: whatever is one piece with it stays one, joined to the largest
/// fill or not. A small piece of fill with no pad on it is joined to nothing for its own
/// sake, unless a zone of the net keeps its islands. And a track that a via taken out
/// leaves leading nowhere is needless too; with `between`, so is every track that the
/// groups are as joined without, one from a pad to another that a fill joins among them. `widths` are the tracks' widths, by
/// position in `c`'s, and `diameters` its vias'. `None` if nothing is needless, or the
/// net has no fill.
///
/// A net that plans on a plane has a stub for every pad that the plane does not reach,
/// whatever a pour of an outer layer joins: guide plans on no pour that routes may cut.
/// And a net whose pads the routes of others cut off their pour is joined up again each
/// time that is told, while those routes still move: by a wire through a via for a piece
/// that another wire, or the pour as it is at the end, joins as well. Once the routes are
/// where they stay, the fill around them says what the pours join, and the vias are taken
/// one at a time, so that a pour with five pads keeps the one that takes it to the plane.
///
/// A track or a via is one with every pad and every piece of fill that its copper
/// touches, anywhere along it: a track across a piece of a pour joins that piece, and may
/// be all that does.
pub(crate) fn needless(
    design: &Design,
    net: NetId,
    c: &NetCopper,
    (widths, diameters): (&[i64], &[i64]),
    fills: &[Fill],
    between: bool,
) -> Option<Vec<bool>> {
    let own: Vec<Fill> = fills
        .iter()
        .filter(|f| design.zones.zones.get(f.zone).and_then(|z| z.net) == Some(net))
        .cloned()
        .collect();
    let by_net = planes::pieces_by_net(design, &own);
    let fills: &[Piece] = by_net.first().map_or(&[][..], |(_, list)| list.as_slice());
    // What the fills and the board's own copper join, without the net's routes.
    let groups = planes::pieces(&design.board, design, net, fills);
    groups.main(fills)?;
    let count = groups.pads.len();
    let mut of_fill = vec![usize::MAX; fills.len()];
    for (g, theirs) in groups.fills.iter().enumerate() {
        for &f in theirs {
            of_fill[f] = g;
        }
    }
    let pads: Vec<(usize, usize)> = groups
        .pads
        .iter()
        .enumerate()
        .flat_map(|(g, pads)| pads.iter().map(move |&p| (p, g)))
        .collect();
    // The groups that `copper` on `layers`, with ends at `ends`, is part of: a pad of the
    // net or a fill that it touches, a track's end or a via of the board's own at one of
    // its ends.
    let touched = |copper: &Shape, ends: &[Point], layers: LayerSet, out: &mut Vec<usize>| {
        let near = copper.bbox();
        for &(p, g) in &pads {
            if pad_touches(&design.board.pads[p], copper, layers) {
                out.push(g);
            }
        }
        for (f, piece) in fills.iter().enumerate() {
            if layers.contains(piece.layer)
                && of_fill[f] != usize::MAX
                && piece.bbox.intersects(&near)
                && (ends.iter().any(|&at| piece.holds(at)) || piece.touches(copper))
            {
                out.push(of_fill[f]);
            }
        }
        for (g, points) in groups.points.iter().enumerate() {
            if points
                .iter()
                .any(|&(q, on)| ends.contains(&q) && on.0 & layers.0 != 0)
            {
                out.push(g);
            }
        }
    };
    let all = design.rules.layers();
    let (ns, nv) = (c.segs.len(), c.vias.len());
    // By track and then by via, the groups that it is part of.
    let mut of: Vec<Vec<usize>> = vec![Vec::new(); ns + nv];
    // The copper of each, with its layers. One that is no shape is taken for a net that
    // nothing is told of.
    let mut shapes: Vec<(Shape, LayerSet)> = Vec::with_capacity(ns + nv);
    for (i, s) in c.segs.iter().enumerate() {
        let on = LayerSet::EMPTY.with(LayerId(s.l));
        let copper = widths
            .get(i)
            .and_then(|&w| Shape::track(s.a, s.b, w).ok())?;
        touched(&copper, &[s.a, s.b], on, &mut of[i]);
        shapes.push((copper, on));
    }
    for (v, &at) in c.vias.iter().enumerate() {
        let copper = diameters.get(v).and_then(|&d| Shape::circle(at, d).ok())?;
        touched(&copper, &[at], all, &mut of[ns + v]);
        shapes.push((copper, all));
    }
    for g in &mut of {
        g.sort_unstable();
        g.dedup();
    }
    // What touches what: two of them with a layer in common whose copper does.
    let mut touching: Vec<(usize, usize)> = Vec::new();
    {
        let boxes: Vec<Rect> = shapes.iter().map(|(s, _)| s.bbox()).collect();
        let mut order: Vec<usize> = (0..shapes.len()).collect();
        order.sort_by_key(|&i| (boxes[i].min().x, i));
        for (k, &i) in order.iter().enumerate() {
            for &j in &order[k + 1..] {
                if boxes[j].min().x > boxes[i].max().x {
                    break;
                }
                if shapes[i].1.0 & shapes[j].1.0 != 0
                    && boxes[i].intersects(&boxes[j])
                    && shapes[i].0.touches(&shapes[j].0)
                {
                    touching.push((i.min(j), i.max(j)));
                }
            }
        }
    }
    // The groups that count: those with a pad, or with copper of the board's own, or
    // with fill of a sixteenth of the largest group's, as a plane with no pad on its
    // layer has. A small piece of fill that touches no pad is an island once nothing of
    // the net's routes touches it, which a zone that takes its islands away does not
    // keep, and a via that only ties such a piece to the rest is needless. Where a zone
    // of the net keeps its islands, every group counts.
    let keeps = design
        .zones
        .zones
        .iter()
        .any(|z| z.net == Some(net) && z.islands != super::Islands::Remove);
    let size = |g: usize| -> i128 { groups.fills[g].iter().map(|&f| fills[f].size).sum() };
    let largest = (0..count).map(size).max().unwrap_or(0);
    let counted: Vec<usize> = (0..count)
        .filter(|&g| {
            keeps
                || !groups.pads[g].is_empty()
                || !groups.points[g].is_empty()
                || size(g) * ISLAND >= largest
        })
        .collect();
    // By group that counts, the first of those that it is one with, by the tracks and
    // vias that are not `gone`.
    let joined = |gone: &[bool]| {
        let mut parent: Vec<u32> = (0..(count + ns + nv) as u32).collect();
        for (e, groups) in of.iter().enumerate() {
            if !gone[e] {
                for &g in groups {
                    union(&mut parent, g as u32, (count + e) as u32);
                }
            }
        }
        for &(a, b) in &touching {
            if !gone[a] && !gone[b] {
                union(&mut parent, (count + a) as u32, (count + b) as u32);
            }
        }
        let mut first = vec![u32::MAX; count + ns + nv];
        for &g in &counted {
            let root = find(&mut parent, g as u32) as usize;
            first[root] = first[root].min(g as u32);
        }
        counted
            .iter()
            .map(|&g| first[find(&mut parent, g as u32) as usize])
            .collect::<Vec<u32>>()
    };
    let mut gone = vec![false; ns + nv];
    let before = joined(&gone);
    // The vias first, each with nothing else: one that the groups are as joined without
    // goes. Then the tracks that this leaves leading nowhere, over and over until none
    // goes: one with an end that nothing of the copper that stays is at, and no pad. A
    // track from pad to pad stays, whatever fill joins the two: taking it out leaves
    // room that another net's pour may fill, and the fill that joined them is then
    // another.
    for v in 0..nv {
        gone[ns + v] = true;
        if joined(&gone) != before {
            gone[ns + v] = false;
        }
    }
    // With `between`, the tracks as well, the longest first: one from pad to pad whose
    // two pads a fill joins, or part of the way between them. For a host that fills its
    // zones again without them and looks at what that fill joins.
    if between {
        let mut order: Vec<usize> = (0..ns).collect();
        order.sort_by(|&a, &b| {
            c.segs[b]
                .length()
                .total_cmp(&c.segs[a].length())
                .then(a.cmp(&b))
        });
        for i in order {
            gone[i] = true;
            if joined(&gone) != before {
                gone[i] = false;
            }
        }
    }
    if !gone.iter().any(|&g| g) {
        return None;
    }
    // By track, for each of its ends, what touches it there and whether a pad does.
    let dot = |at: Point| Shape::circle(at, 2).ok();
    let mut ends: Vec<[(Vec<usize>, bool); 2]> = Vec::with_capacity(ns);
    for (i, s) in c.segs.iter().enumerate() {
        let on = LayerSet::EMPTY.with(LayerId(s.l));
        let end = |at: Point| {
            let Some(dot) = dot(at) else {
                return (Vec::new(), true);
            };
            let near: Vec<usize> = touching
                .iter()
                .filter(|&&(a, b)| a == i || b == i)
                .map(|&(a, b)| if a == i { b } else { a })
                .filter(|&o| shapes[o].0.touches(&dot))
                .collect();
            let pad = pads
                .iter()
                .any(|&(p, _)| pad_touches(&design.board.pads[p], &dot, on));
            (near, pad)
        };
        ends.push([end(s.a), end(s.b)]);
    }
    loop {
        let mut any = false;
        for i in 0..ns {
            if gone[i] {
                continue;
            }
            let loose = ends[i]
                .iter()
                .any(|(near, pad)| !pad && near.iter().all(|&o| gone[o]));
            if !loose {
                continue;
            }
            gone[i] = true;
            if joined(&gone) == before {
                any = true;
            } else {
                gone[i] = false;
            }
        }
        if !any {
            break;
        }
    }
    Some(gone)
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
    fn the_route_that_cuts_a_pad_off_is_told_and_the_way_that_the_fill_has_without_it() {
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
        let bounds = Rect::new(Point::new(-5 * MM, -5 * MM), Point::new(25 * MM, 12 * MM));
        let pours = Pours::plan(&d, &world, &fills, &jobs, bounds, pitch);
        // A track of the other net across the strip, between the two pads on it.
        let mut copper: Vec<Option<NetCopper>> = vec![None; jobs.len()];
        let mut across = NetCopper::default();
        across.segs.push(Seg {
            a: Point::new(12 * MM, -3 * MM),
            b: Point::new(12 * MM, 7 * MM),
            l: 0,
        });
        copper[other] = Some(across);
        let cut = pours
            .rejoin(&world, &d, &jobs, &copper, ground, pitch, None, &[])
            .expect("a pad is cut off");
        let pad = cut
            .ctx
            .pads
            .iter()
            .find(|p| p.terminal >= jobs[ground].ctx.terminals && p.bbox.min() != p.bbox.max())
            .expect("the pad cut off");
        assert_eq!(pad.center, Point::new(14_500_000, 2 * MM));
        assert_eq!(
            pours.cutters(&world, &d, &jobs, &copper, ground, pad, &[]),
            [other]
        );
        // With nothing taken as not there, the fill has no way; without the track, it
        // goes from the pad's side of it to the other, on the pad's layer.
        assert!(
            pours
                .bridge(&world, &d, &jobs, &copper, ground, pad, &[])
                .is_none()
        );
        let (layer, path, reach) = pours
            .bridge(&world, &d, &jobs, &copper, ground, pad, &[other])
            .expect("a way through where the track is");
        assert_eq!(layer, crate::geom::LayerId(0));
        assert!(reach >= 300_000 + 100_000, "{reach}");
        let (first, last) = (path[0], path[path.len() - 1]);
        assert!(first.x > 12 * MM && last.x < 12 * MM, "{path:?}");
        assert!(path.iter().all(|p| p.y > 0 && p.y < 4 * MM), "{path:?}");
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
        let rejoin = |copper: &[Option<NetCopper>]| {
            pours.rejoin(&world, &d, &jobs, copper, ground, pitch, None, &[])
        };
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
    fn the_connections_open_on_the_fills_count_the_pads_cut_off() {
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
        let bounds = Rect::new(Point::new(-5 * MM, -5 * MM), Point::new(25 * MM, 12 * MM));
        let pours = Pours::plan(&d, &world, &fills, &jobs, bounds, pitch);
        let open = |copper: &[Option<NetCopper>]| {
            super::super::whole::open_on_fills(&world, &d, &jobs, &pours, copper, None, &[], pitch)
        };
        // Nothing routed: the pad off the strip is to be joined to it, and the two pads
        // of the other net to each other; the strip joins its own two.
        let mut copper: Vec<Option<NetCopper>> = vec![None; jobs.len()];
        let (opens, plans) = open(&copper);
        assert_eq!((opens[ground], opens[other]), (1, 1));
        assert!(plans.is_empty());
        // The other net routed across the strip: it has nothing open, and the strip's
        // net one more, the pad behind the track, with the net planned again for it.
        let mut c = NetCopper::default();
        c.segs.push(Seg {
            a: Point::new(12 * MM, -3 * MM),
            b: Point::new(12 * MM, 7 * MM),
            l: 0,
        });
        copper[other] = Some(c);
        let (opens, plans) = open(&copper);
        assert_eq!((opens[ground], opens[other]), (2, 0));
        assert_eq!(plans.len(), 1);
        assert_eq!((plans[0].0, plans[0].1.cut), (ground, 1));
        // On the other layer the strip is whole, and the track joins no pad.
        copper[other].as_mut().unwrap().segs[0].l = 1;
        let (opens, plans) = open(&copper);
        assert_eq!((opens[ground], opens[other]), (1, 1));
        assert!(plans.is_empty());
    }

    #[test]
    fn a_negotiation_that_makes_its_routes_legal_first_joins_the_pad_cut_off() {
        let d = strip();
        // Every net against every other at once, and the pads cut off told on the routes
        // made legal.
        let options = RouteOptions {
            work_budget: Some(50_000_000),
            join: false,
            whole: true,
            legal_pours: true,
            ..RouteOptions::default()
        };
        let routed = route_checked(&d, &options);
        assert!(routed.stats.pour_cuts >= 1, "{:?}", routed.stats);
        assert_eq!(routed.stats.pours_back, 0);
        assert!(routed.unrouted.is_empty(), "{:?}", routed.unrouted);
        // The same on any number of threads.
        let three = route_checked(
            &d,
            &RouteOptions {
                threads: 3,
                ..options
            },
        );
        assert_eq!((three.tracks, three.vias), (routed.tracks, routed.vias));
    }

    #[test]
    fn a_net_with_a_plane_is_planned_on_its_outer_pours_if_asked() {
        use super::super::tests::design;
        use super::super::{RunControl, Zones, route_with};
        // Four layers. Net 1 has a plane on the second and a pour on the first, which
        // joins two of its pads there; the third is beside the pour.
        let pads = vec![
            smd(3 * MM, 2 * MM, 1),
            smd(8 * MM, 2 * MM, 1),
            smd(6 * MM, 8 * MM, 1),
        ];
        let mut d = design(4, pads, 2);
        let mut plane = zone(1, 1, 4 * MM);
        plane.outline[1].x = 12 * MM;
        plane.outline[2].x = 12 * MM;
        plane.connection = super::super::ZoneConnection::Solid;
        plane.plane = Some(true);
        let mut pour = plane.clone();
        pour.layers = crate::geom::LayerSet::EMPTY.with(crate::geom::LayerId(0));
        d.zones = Zones {
            zones: vec![plane, pour],
            fills: vec![
                piece(0, 1, (0, 12 * MM), (0, 4 * MM)),
                piece(1, 0, (0, 12 * MM), (0, 4 * MM)),
            ],
            ..Zones::default()
        };
        let options = RouteOptions {
            work_budget: Some(50_000_000),
            join: false,
            fills: false,
            ..RouteOptions::default()
        };
        // Planned on the plane alone, each pad has its stub to it.
        let inner = route_with(&d, &options, &RunControl::default()).unwrap();
        assert_eq!(inner.vias.len(), 3, "{:?}", inner.stats);
        assert!(inner.unrouted.is_empty(), "{:?}", inner.unrouted);
        // On the pour too, the two pads in it are joined, and the third has a track to
        // the pour on its own layer.
        let outer = RouteOptions {
            outer_pours: true,
            ..options
        };
        let routed = route_with(&d, &outer, &RunControl::default()).unwrap();
        assert_eq!(routed.vias.len(), 0, "{:?}", routed.stats);
        assert!(!routed.tracks.is_empty());
        assert!(
            routed
                .tracks
                .iter()
                .all(|t| t.layer == crate::geom::LayerId(0))
        );
        assert!(routed.unrouted.is_empty(), "{:?}", routed.unrouted);
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
                .rejoin(&world, d, &jobs, &copper, ground, pitch, None, &[])
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

    /// A rectangle of fill of zone `zone` on `layer`.
    fn piece(zone: usize, layer: u8, x: (i64, i64), y: (i64, i64)) -> super::Fill {
        super::Fill {
            zone,
            layer: crate::geom::LayerId(layer),
            shape: crate::geom::Shape::rect(Rect::new(Point::new(x.0, y.0), Point::new(x.1, y.1)))
                .unwrap(),
        }
    }

    #[test]
    fn the_host_s_fills_say_which_pads_are_cut_off() {
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
        let bounds = Rect::new(Point::new(-5 * MM, -5 * MM), Point::new(25 * MM, 12 * MM));
        let pours = Pours::plan(&d, &world, &fills, &jobs, bounds, pitch);
        let mut copper: Vec<Option<NetCopper>> = vec![None; jobs.len()];
        let mut c = NetCopper::default();
        c.segs.push(Seg {
            a: Point::new(12 * MM, -3 * MM),
            b: Point::new(12 * MM, 3 * MM),
            l: 0,
        });
        copper[other] = Some(c);
        // A track of the other net three quarters of the way into the strip: the cells
        // leave a neck of fill past its end, and tell no pad cut off.
        let by = |copper: &[Option<NetCopper>], real: Option<&super::Refilled>| {
            pours
                .rejoin(&world, &d, &jobs, copper, ground, pitch, real, &[])
                .map(|r| r.cut)
        };
        assert_eq!(by(&copper, None), None);
        // The host's fill has no copper as narrow as that neck: two pieces, and the pad
        // at 14.5 mm on the smaller.
        let of = |fills: Vec<super::Fill>| super::Refilled {
            nets: planes::pieces_by_net(&d, &fills),
            fills,
        };
        let halves = of(vec![
            piece(0, 0, (0, 11_400_000), (0, 4 * MM)),
            piece(0, 0, (12_600_000, 20 * MM), (0, 4 * MM)),
        ]);
        assert_eq!(by(&copper, Some(&halves)), Some(1));
        let again = pours
            .rejoin(
                &world,
                &d,
                &jobs,
                &copper,
                ground,
                pitch,
                Some(&halves),
                &[],
            )
            .unwrap();
        for s in &again.sites {
            assert!(s.center.x < 11_400_000 && s.terminal == 0, "{s:?}");
        }
        // A host whose fill is in one piece has none cut off, whatever the cells tell: a
        // track across the strip, and the host's fill round it by a way that the grid
        // has not.
        copper[other].as_mut().unwrap().segs[0].b.y = 7 * MM;
        assert_eq!(by(&copper, None), Some(1));
        let one = of(vec![piece(0, 0, (0, 20 * MM), (0, 4 * MM))]);
        assert_eq!(by(&copper, Some(&one)), None);
        // And a pad that the host's fill does not touch is cut off, though fill is all
        // round it: its relief has no spoke left.
        let relieved = of(vec![super::Fill {
            zone: 0,
            layer: crate::geom::LayerId(0),
            shape: crate::geom::Shape::polygon(
                &[
                    Point::new(0, 0),
                    Point::new(20 * MM, 0),
                    Point::new(20 * MM, 4 * MM),
                    Point::new(0, 4 * MM),
                ],
                &[&[
                    Point::new(14 * MM, 1_500_000),
                    Point::new(15 * MM, 1_500_000),
                    Point::new(15 * MM, 2_500_000),
                    Point::new(14 * MM, 2_500_000),
                ]],
            )
            .unwrap(),
        }]);
        copper[other] = None;
        assert_eq!(by(&copper, Some(&relieved)), Some(1));
    }

    #[test]
    fn a_run_asks_its_host_for_its_fills_and_takes_the_needless_stubs_out() {
        use super::super::tests::design;
        use super::super::{RunControl, Zones, route_with};
        use std::cell::Cell;
        // Four layers. Net 1 has a plane on the second and a pour on the first, which
        // joins its two pads there; guide plans on the plane alone, and gives each pad
        // its stub.
        let pads = vec![smd(3 * MM, 2 * MM, 1), smd(8 * MM, 2 * MM, 1)];
        let mut d = design(4, pads, 2);
        let mut plane = zone(1, 1, 4 * MM);
        plane.outline[1].x = 12 * MM;
        plane.outline[2].x = 12 * MM;
        plane.connection = super::super::ZoneConnection::Solid;
        plane.plane = Some(true);
        let mut pour = plane.clone();
        pour.layers = crate::geom::LayerSet::EMPTY.with(crate::geom::LayerId(0));
        let fills = vec![
            piece(0, 1, (0, 12 * MM), (0, 4 * MM)),
            piece(1, 0, (0, 12 * MM), (0, 4 * MM)),
        ];
        d.zones = Zones {
            zones: vec![plane, pour],
            fills: fills.clone(),
            ..Zones::default()
        };
        let options = RouteOptions {
            work_budget: Some(50_000_000),
            join: false,
            fills: false,
            ..RouteOptions::default()
        };
        // With no host to ask, both stubs stay.
        let alone = route_with(&d, &options, &RunControl::default()).unwrap();
        assert_eq!(
            (alone.vias.len(), alone.stats.refills),
            (2, 0),
            "{:?}",
            alone.stats
        );
        // With the host's fills, one is enough: the pour joins the other pad to it.
        let asked = Cell::new((0, 0));
        let host = |r: &super::super::Refill<'_>| {
            let (n, last) = asked.get();
            asked.set((n + 1, last + usize::from(r.last)));
            assert!(r.vias.iter().all(|v| v.net == Some(NetId(1))));
            Some(fills.clone())
        };
        let mut control = RunControl {
            refill: Some(&host),
            ..RunControl::default()
        };
        let routed = route_with(&d, &options, &control).unwrap();
        assert_eq!(routed.vias.len(), 1, "{:?}", routed.stats);
        assert_eq!(routed.stats.stubs_pruned, 1);
        // The stub that went is told, for a host that goes on from these routes.
        assert_eq!(routed.spare.1.len(), 1);
        let gone = &routed.spare.1[0];
        assert!(alone.vias.contains(gone) && !routed.vias.contains(gone));
        assert!(alone.spare.0.is_empty() && alone.spare.1.is_empty());
        // Asked once when the routes were where they stay, and not before on a board
        // with no fill planned on an outer layer.
        assert_eq!(asked.get(), (1, 1));
        assert_eq!(routed.stats.refills, 1);
        assert!(routed.unrouted.is_empty(), "{:?}", routed.unrouted);
        // The same again: the host's answers are all that it depends on.
        let again = route_with(&d, &options, &control).unwrap();
        assert_eq!((again.tracks, again.vias), (routed.tracks, routed.vias));
        // A host that has no fill to give leaves the run as it is without one.
        let none = |_: &super::super::Refill<'_>| None;
        control.refill = Some(&none);
        let routed = route_with(&d, &options, &control).unwrap();
        assert_eq!((routed.tracks, routed.vias), (alone.tracks, alone.vias));
    }

    #[test]
    fn the_fills_say_which_vias_and_tracks_of_a_net_are_needless() {
        use super::super::tests::design;
        use super::super::{Zones, needless};
        use crate::geom::drc::{Track, Via};
        use crate::geom::{LayerId, LayerSet};
        // Four layers. Net 1 has a plane on the second and a pour on the first, which
        // joins its two pads there, and two more pads off both, 20 mm away.
        let pads = vec![
            smd(3 * MM, 2 * MM, 1),
            smd(8 * MM, 2 * MM, 1),
            smd(20 * MM, 2 * MM, 1),
            smd(24 * MM, 2 * MM, 1),
        ];
        let mut d = design(4, pads, 2);
        let mut plane = zone(1, 1, 4 * MM);
        plane.outline[1].x = 12 * MM;
        plane.outline[2].x = 12 * MM;
        plane.plane = Some(true);
        let mut pour = plane.clone();
        pour.layers = LayerSet::EMPTY.with(LayerId(0));
        d.zones = Zones {
            zones: vec![plane, pour],
            ..Zones::default()
        };
        let fills = vec![
            piece(0, 1, (0, 12 * MM), (0, 4 * MM)),
            piece(1, 0, (0, 12 * MM), (0, 4 * MM)),
        ];
        let track = |a: (i64, i64), b: (i64, i64)| Track {
            start: Point::new(a.0, a.1),
            end: Point::new(b.0, b.1),
            mid: None,
            width: 200_000,
            layer: LayerId(0),
            net: Some(NetId(1)),
        };
        let via = |x: i64, y: i64| Via {
            at: Point::new(x, y),
            drill: 300_000,
            diameter: 600_000,
            from: LayerId(0),
            to: LayerId(3),
            net: Some(NetId(1)),
        };
        // A stub to the plane from each pad on the pour, a track round a corner to a
        // via; and a track between the two pads off it.
        let tracks = vec![
            track((3 * MM, 2 * MM), (3 * MM, 3 * MM)),
            track((3 * MM, 3 * MM), (4 * MM, 3 * MM)),
            track((8 * MM, 2 * MM), (8 * MM, 3 * MM)),
            track((20 * MM, 2 * MM), (24 * MM, 2 * MM)),
        ];
        let vias = vec![via(4 * MM, 3 * MM), via(8 * MM, 3 * MM)];
        // The pour joins the two pads, so one via to the plane is enough: the first goes,
        // with the two tracks that led to it, and the second stays, as the one thing
        // that joins the pour to the plane. The track between the pads that no fill
        // reaches stays too: they are one piece by it, if not with the plane.
        assert_eq!(needless(&d, &tracks, &vias, &fills), (vec![0, 1], vec![0]));
        // A track from one pad on the pour to the other is not told, whatever the pour
        // joins, unless it is asked for: the fill without it is another fill, for the
        // host to make and look at. Asked for, it goes, and with it the track from the
        // second pad to the via that stays, which is in the pour as the pad is. The
        // track between the pads off the pour stays then too.
        let mut more = tracks.clone();
        more.push(track((3 * MM, 2 * MM), (8 * MM, 2 * MM)));
        assert_eq!(needless(&d, &more, &vias, &fills), (vec![0, 1], vec![0]));
        assert_eq!(
            super::super::needless_between(&d, &more, &vias, &fills),
            (vec![0, 1, 2, 4], vec![0])
        );
        // With no pour, each pad needs its stub.
        let none: (Vec<usize>, Vec<usize>) = Default::default();
        assert_eq!(needless(&d, &tracks, &vias, &fills[..1]), none);
        // And with no fill at all there is nothing to tell by.
        assert_eq!(needless(&d, &tracks, &vias, &[]), none);
        // A net with a via that does not go through the board is left as it is.
        let mut blind = vias.clone();
        blind[1].to = LayerId(1);
        assert_eq!(needless(&d, &tracks, &blind, &fills), none);
        // A small piece of fill on the far side with no pad on it, and a via that ties
        // it to the plane where the pour does not reach: the via joins no pad to
        // another, and goes. Unless the zone keeps its islands.
        let island = vec![
            piece(0, 1, (0, 12 * MM), (0, 4 * MM)),
            piece(1, 0, (0, 10 * MM), (0, 4 * MM)),
            piece(1, 3, (10_500_000, 11_500_000), (1_500_000, 2_500_000)),
        ];
        let mut tied = vias.clone();
        tied.push(via(11 * MM, 2 * MM));
        assert_eq!(
            needless(&d, &tracks, &tied, &island),
            (vec![0, 1], vec![0, 2])
        );
        d.zones.zones[1].islands = super::super::Islands::Keep;
        assert_eq!(needless(&d, &tracks, &tied, &island), (vec![0, 1], vec![0]));
    }

    #[test]
    fn a_via_of_its_net_may_be_in_a_pad_that_the_host_names_and_no_track_leaves() {
        use super::super::tests::design;
        use super::super::world::Verdict;
        use crate::geom::drc::Pad;
        use crate::geom::{LayerId, LayerSet, Shape};
        // The exposed pad of a package, 3 mm square, of net 1, and the pads of the
        // package round it, of net 2, with no room between two of them.
        let big = Pad::new(
            Shape::rect(Rect::new(Point::new(0, 0), Point::new(3 * MM, 3 * MM))).unwrap(),
            LayerSet::EMPTY.with(LayerId(0)),
            Some(NetId(1)),
        );
        let mut pads = vec![big];
        for k in 0..8 {
            let at = -800_000 + 650_000 * k;
            for (x, y) in [
                (at, -800_000),
                (at, 3_800_000),
                (-800_000, at),
                (3_800_000, at),
            ] {
                pads.push(smd(x, y, 2));
            }
        }
        let shut = design(2, pads, 3);
        let options = RouteOptions {
            vias_in_pads: false,
            ..RouteOptions::default()
        };
        let via = |d: &Design, net: u32, x: i64| {
            let mut world = World::new(
                &d.board,
                &d.rules,
                &d.outline,
                d.edge_clearance,
                &d.obstacles,
            )
            .unwrap();
            world.configure(d, &options);
            world.via(NetId(net), Point::new(x, 1_500_000), 600_000, 300_000, None)
        };
        // Vias are kept out of pads.
        assert_eq!(via(&shut, 1, 1_500_000), Verdict::Blocked);
        // Named, the pad has room for a via of its net that is all inside it, and for no
        // other net's.
        let mut named = shut.clone();
        named.zones.via_pads = vec![0];
        assert_eq!(via(&named, 1, 1_500_000), Verdict::Free);
        assert_eq!(via(&named, 1, 2_900_000), Verdict::Blocked);
        assert_eq!(via(&named, 2, 1_500_000), Verdict::Blocked);
        // With a side of the package open, a track leaves the pad, and it is joined by
        // tracks as any other.
        let mut open = named.clone();
        open.board.pads.retain(|p| p.shape.bbox().min().x < 3 * MM);
        assert_eq!(via(&open, 1, 1_500_000), Verdict::Blocked);
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
