//! Meanders: nets made as long as [`Design::lengths`] asks, after routing.
//!
//! Lengths can only be added. A net shorter than it must be gets *trombone* meanders: on
//! its straight tracks, longest first, the track goes out sideways, along and back, as
//! often and as far as it takes, each meander as high as there is room for up to
//! [`RouteOptions::max_amplitude`], the last one just high enough. Each is checked exactly
//! against everything else, as routing is, and kept a clearance from the net's own copper,
//! as a meander that touched it would short its own length out.
//!
//! The two nets of a differential pair routed side by side get meanders together where
//! they run side by side, the one inside the other at the pair's gap, which add the same
//! length to both. What is left of a skew between the two is then made up on the shorter
//! alone, as for any net.
//!
//! A net still short after that, where its neighbours' meanders took the room it needed, has
//! them taken out: it is meandered first, and they after it, wherever they find room, and
//! that is kept if fewer nets are left short, or by less. This goes round a few times, for
//! as long as it helps.

use super::measure::net_length;
use super::pairs::offset;
use super::world::{Placed, Verdict, World};
use super::{Design, LengthConstraint, RouteOptions, Routed};
use crate::geom::drc::{Track, Via};
use crate::geom::{GeomError, LayerId, Point, Rect, Shape};
use crate::id::NetId;

/// The board as it is routed, with what each routed track put into the world.
struct Tuner<'a> {
    design: &'a Design,
    world: World<'a>,
    tracks: Vec<Track>,
    placed: Vec<Option<Placed>>,
    /// The routed vias, which meanders keep clear of as of the net's own copper.
    vias: Vec<Via>,
    amplitude: i64,
    /// The meanders of single nets, in the order added, to take out again.
    log: Vec<Bump>,
}

/// A meander added to one net's track: the track as it was, by position, and the pieces
/// that replaced it.
#[derive(Clone, Debug)]
struct Bump {
    net: NetId,
    at: usize,
    original: Track,
    pieces: Vec<usize>,
}

/// How many times the nets still short have their neighbours' meanders taken out, to be
/// meandered first.
const MAKE_ROOM_ROUNDS: usize = 3;

/// A straight track as steps along one of the eight directions: the direction, as a step
/// of one nanometre along each axis it moves on, and how many steps.
fn steps_of(t: &Track) -> Option<((i64, i64), i64)> {
    if t.mid.is_some() {
        return None;
    }
    let (dx, dy) = (t.end.x - t.start.x, t.end.y - t.start.y);
    if dx == 0 && dy == 0 {
        return None;
    }
    if dx != 0 && dy != 0 && dx.abs() != dy.abs() {
        return None;
    }
    Some(((dx.signum(), dy.signum()), dx.abs().max(dy.abs())))
}

/// The length of one step in a direction.
fn unit_length(d: (i64, i64)) -> f64 {
    if d.0 != 0 && d.1 != 0 {
        core::f64::consts::SQRT_2
    } else {
        1.0
    }
}

fn at(p: Point, d: (i64, i64), t: i64, n: (i64, i64), h: i64) -> Point {
    Point::new(p.x + d.0 * t + n.0 * h, p.y + d.1 * t + n.1 * h)
}

fn length(a: Point, b: Point) -> f64 {
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    (dx * dx + dy * dy).sqrt()
}

/// Heights to try for a meander, highest first.
fn heights(h: i64, lowest: i64) -> Vec<i64> {
    let mut out = vec![h];
    for (num, den) in [(3, 4), (1, 2), (1, 3), (1, 4), (1, 6), (1, 8)] {
        let x = h * num / den;
        if x >= lowest && x < *out.last().unwrap_or(&i64::MAX) {
            out.push(x);
        }
    }
    out
}

impl<'a> Tuner<'a> {
    fn clearance(&self, net: NetId, layer: LayerId) -> i64 {
        self.design.rules.clearance(Some(net), layer)
    }

    fn length(&self, net: NetId) -> f64 {
        net_length(&self.tracks, net) + net_length(&self.design.board.tracks, net)
    }

    /// Whether a new track of `net` is legal: free in the world, and a clearance from the
    /// net's own copper but for the tracks in `except`.
    fn legal(
        &self,
        net: NetId,
        layer: LayerId,
        a: Point,
        b: Point,
        width: i64,
        except: &[usize],
    ) -> bool {
        let Ok(shape) = Shape::track(a, b, width) else {
            return false;
        };
        if self.world.track(net, layer, &shape, None) != Verdict::Free {
            return false;
        }
        let c = self.clearance(net, layer);
        let bbox = shape.bbox().expand(c + width);
        let near = |r: crate::geom::Rect| r.intersects(&bbox);
        let tracks = self
            .tracks
            .iter()
            .enumerate()
            .chain(
                self.design
                    .board
                    .tracks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| (usize::MAX - i, t)),
            )
            .filter(|(i, t)| {
                t.net == Some(net) && t.layer == layer && t.start != t.end && !except.contains(i)
            });
        for (_, t) in tracks {
            let other = match t.mid {
                None => Shape::track(t.start, t.end, t.width),
                Some(m) => {
                    Shape::arc_track(t.start, m, t.end, t.width, self.design.board.arc_tolerance)
                }
            };
            if let Ok(other) = other
                && near(other.bbox())
                && shape.violates(&other, c).unwrap_or(true)
            {
                return false;
            }
        }
        for p in self
            .design
            .board
            .pads
            .iter()
            .filter(|p| p.net == Some(net) && p.layers.contains(layer))
        {
            if near(p.shape.bbox()) && shape.violates(&p.shape, c).unwrap_or(true) {
                return false;
            }
        }
        let vias = self
            .vias
            .iter()
            .chain(&self.design.board.vias)
            .filter_map(|v| v.net.map(|n| (n, v.at, v.diameter)));
        for (n, at, d) in vias {
            if n == net
                && let Ok(v) = Shape::circle(at, d)
                && near(v.bbox())
                && shape.violates(&v, c).unwrap_or(true)
            {
                return false;
            }
        }
        true
    }

    /// Replaces track `i` with `pieces`, in the world too.
    fn replace(&mut self, i: usize, pieces: Vec<Track>) -> Vec<usize> {
        if let Some(p) = self.placed[i].take() {
            self.world.remove(&p);
        }
        let mut out = Vec::with_capacity(pieces.len());
        for (k, t) in pieces.into_iter().enumerate() {
            let j = if k == 0 {
                self.tracks[i] = t;
                i
            } else {
                self.tracks.push(t);
                self.placed.push(None);
                self.tracks.len() - 1
            };
            self.place(j);
            out.push(j);
        }
        out
    }

    fn place(&mut self, i: usize) {
        let t = &self.tracks[i];
        if let (Some(net), Ok(shape)) = (t.net, Shape::track(t.start, t.end, t.width)) {
            self.placed[i] = Some(self.world.add_track(net, t.layer, shape));
        }
    }

    /// Adds up to `need` nanometres to `net` with meanders on its straight tracks, and
    /// returns how much it added.
    fn lengthen(&mut self, net: NetId, need: f64) -> f64 {
        let mut candidates: Vec<(usize, f64)> = (0..self.tracks.len())
            .filter(|&i| self.tracks[i].net == Some(net) && steps_of(&self.tracks[i]).is_some())
            .map(|i| (i, length(self.tracks[i].start, self.tracks[i].end)))
            .collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut added = 0.0;
        for (i, _) in candidates {
            if need - added < 1.0 {
                break;
            }
            added += self.meander(i, need - added);
        }
        added
    }

    /// Meanders on track `i`, adding up to `need`; returns how much.
    fn meander(&mut self, i: usize, need: f64) -> f64 {
        let t = self.tracks[i].clone();
        let Some(net) = t.net else { return 0.0 };
        let Some((d, total)) = steps_of(&t) else {
            return 0.0;
        };
        let ul = unit_length(d);
        let n = (-d.1, d.0);
        let c = self.clearance(net, t.layer);
        // Legs a clearance apart, and as far from the track's ends.
        let s = ((t.width + c) as f64 / ul).ceil() as i64;
        let margin = s;
        let cap = (self.amplitude as f64 / ul) as i64;
        let lowest = ((t.width as f64 / ul).ceil() as i64).max(1);
        let mut left = need;
        let mut bumps: Vec<(i64, i64)> = Vec::new();
        let mut pos = margin;
        while pos + s + margin <= total && left >= 1.0 {
            let want = ((left / (2.0 * ul)).ceil() as i64).min(cap);
            if want < 1 {
                break;
            }
            let mut best: Option<(i64, i64)> = None;
            for side in [1, -1] {
                let n = (n.0 * side, n.1 * side);
                for h in heights(want, lowest.min(want)) {
                    if best.is_some_and(|(b, _)| b >= h) {
                        break;
                    }
                    let (f1, f2) = (at(t.start, d, pos, n, 0), at(t.start, d, pos + s, n, 0));
                    let (t1, t2) = (at(t.start, d, pos, n, h), at(t.start, d, pos + s, n, h));
                    let ok = [(f1, t1), (t1, t2), (t2, f2)]
                        .iter()
                        .all(|&(a, b)| self.legal(net, t.layer, a, b, t.width, &[i]));
                    if ok {
                        best = Some((h, side));
                        break;
                    }
                }
            }
            match best {
                Some((h, side)) => {
                    bumps.push((pos, h * side));
                    left -= 2.0 * h as f64 * ul;
                    pos += 2 * s;
                }
                None => pos += (s / 2).max(1),
            }
        }
        if bumps.is_empty() {
            return 0.0;
        }
        let mut points = vec![t.start];
        for &(p, h) in &bumps {
            points.extend([
                at(t.start, d, p, n, 0),
                at(t.start, d, p, n, h),
                at(t.start, d, p + s, n, h),
                at(t.start, d, p + s, n, 0),
            ]);
        }
        points.push(t.end);
        let pieces: Vec<Track> = points
            .windows(2)
            .map(|w| Track {
                start: w[0],
                end: w[1],
                ..t.clone()
            })
            .collect();
        let before = length(t.start, t.end);
        let after: f64 = pieces.iter().map(|p| length(p.start, p.end)).sum();
        let pieces = self.replace(i, pieces);
        self.log.push(Bump {
            net,
            at: i,
            original: t,
            pieces,
        });
        after - before
    }

    /// Takes out every meander of `net` that [`meander`](Tuner::meander) added, the last
    /// first.
    fn unmeander(&mut self, net: NetId) {
        let mut k = self.log.len();
        while k > 0 {
            k -= 1;
            if self.log[k].net != net {
                continue;
            }
            let bump = self.log.remove(k);
            for &p in &bump.pieces {
                if let Some(placed) = self.placed[p].take() {
                    self.world.remove(&placed);
                }
                if p != bump.at {
                    self.tracks[p].end = self.tracks[p].start;
                }
            }
            self.tracks[bump.at] = bump.original;
            self.place(bump.at);
        }
    }

    /// The nets other than `net`, and not of a pair, with meanders near its tracks: within
    /// `reach` of them, on the same layer.
    fn neighbours(&self, net: NetId, reach: i64) -> Vec<NetId> {
        let boxes: Vec<(LayerId, Rect)> = self
            .tracks
            .iter()
            .filter(|t| t.net == Some(net) && t.start != t.end)
            .map(|t| {
                (
                    t.layer,
                    Rect::new(t.start, t.start)
                        .union(&Rect::new(t.end, t.end))
                        .expand(reach),
                )
            })
            .collect();
        let mut out: Vec<NetId> = Vec::new();
        for b in &self.log {
            if b.net == net || out.contains(&b.net) || self.paired(b.net) {
                continue;
            }
            let near = b.pieces.iter().any(|&p| {
                let t = &self.tracks[p];
                let r = Rect::new(t.start, t.start).union(&Rect::new(t.end, t.end));
                t.start != t.end
                    && boxes
                        .iter()
                        .any(|(l, bb)| *l == t.layer && bb.intersects(&r))
            });
            if near {
                out.push(b.net);
            }
        }
        out.sort_unstable();
        out
    }

    /// Whether a net is of a differential pair.
    fn paired(&self, net: NetId) -> bool {
        self.design
            .pairs
            .iter()
            .any(|p| p.positive == net || p.negative == net)
    }

    /// The world again, from the tracks and vias, after they were put back as they were.
    fn rebuild(&mut self) -> Result<(), GeomError> {
        let d = self.design;
        self.world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )?;
        self.placed = vec![None; self.tracks.len()];
        for i in 0..self.tracks.len() {
            if self.tracks[i].start != self.tracks[i].end {
                self.place(i);
            }
        }
        for v in &self.vias {
            if let Some(net) = v.net {
                self.world.add_via(net, v.at, v.diameter, v.drill);
            }
        }
        Ok(())
    }

    /// Adds up to `need` to both nets of a pair, with meanders of both together where they
    /// run side by side; returns how much was added to each.
    fn lengthen_pair(&mut self, (pos, neg): (NetId, NetId), gap: i64, need: f64) -> f64 {
        let mut added = 0.0;
        // Tracks of the two nets side by side, longest first.
        let mut couples: Vec<(usize, usize, i64, i64)> = Vec::new();
        for i in 0..self.tracks.len() {
            let tp = &self.tracks[i];
            if tp.net != Some(pos) {
                continue;
            }
            let Some((d, total)) = steps_of(tp) else {
                continue;
            };
            for (j, tn) in self.tracks.iter().enumerate() {
                if tn.net != Some(neg) || tn.layer != tp.layer {
                    continue;
                }
                let Some((e, _)) = steps_of(tn) else { continue };
                if e != d && e != (-d.0, -d.1) {
                    continue;
                }
                // Lateral offset and overlap, in steps along the positive track.
                let n = (-d.1, d.0);
                let ul = unit_length(d);
                let rel = |p: Point| (p.x - tp.start.x, p.y - tp.start.y);
                let (r0, r1) = (rel(tn.start), rel(tn.end));
                let lateral = |r: (i64, i64)| r.0 * n.0 + r.1 * n.1;
                if lateral(r0) != lateral(r1) {
                    continue;
                }
                // A lateral step along `n` is `ul` long.
                let apart = lateral(r0).unsigned_abs() as f64 / ul;
                let spacing = (tp.width + tn.width) as f64 / 2.0 + gap as f64;
                if (apart - spacing).abs() > 100.0 {
                    continue;
                }
                let along = |r: (i64, i64)| (r.0 * d.0 + r.1 * d.1) / (d.0 * d.0 + d.1 * d.1);
                let (u0, u1) = (along(r0).min(along(r1)), along(r0).max(along(r1)));
                let (lo, hi) = (u0.max(0), u1.min(total));
                if hi > lo {
                    couples.push((i, j, lo, hi));
                }
            }
        }
        couples.sort_by_key(|&(i, j, lo, hi)| (core::cmp::Reverse(hi - lo), i, j));
        let mut used: Vec<usize> = Vec::new();
        for (i, j, lo, hi) in couples {
            if need - added < 1.0 {
                break;
            }
            if used.contains(&i) || used.contains(&j) {
                continue;
            }
            let got = self.meander_pair(i, j, lo, hi, need - added);
            if got > 0.0 {
                used.extend([i, j]);
                added += got;
            }
        }
        added
    }

    /// Meanders of a pair's tracks `i` and `j` together, between `lo` and `hi` steps along
    /// `i`, adding up to `need` to each; returns how much was added to the shorter.
    fn meander_pair(&mut self, i: usize, j: usize, lo: i64, hi: i64, need: f64) -> f64 {
        let (tp, tn) = (self.tracks[i].clone(), self.tracks[j].clone());
        let (Some(pos), Some(neg)) = (tp.net, tn.net) else {
            return 0.0;
        };
        let Some((d, _)) = steps_of(&tp) else {
            return 0.0;
        };
        let ul = unit_length(d);
        let n = (-d.1, d.0);
        let layer = tp.layer;
        // The centreline, as the positive track's line moved half way to the other.
        let rel = (tn.start.x - tp.start.x, tn.start.y - tp.start.y);
        let lateral = rel.0 * n.0 + rel.1 * n.1;
        let toward = lateral.signum();
        let half = (lateral.unsigned_abs() as f64 / (ul * ul)) / 2.0;
        let width = tp.width.max(tn.width);
        let c = self.clearance(pos, layer).max(self.clearance(neg, layer));
        let spacing = 2.0 * half * ul;
        let s = ((width as f64 + c as f64 + spacing) / ul).ceil() as i64;
        let margin = 2 * s;
        let cap = (self.amplitude as f64 / ul) as i64;
        let lowest = ((width as f64 / ul).ceil() as i64).max(1);
        // Points of the centreline, `t` steps along and `h` out, rounded.
        let centre = |t: i64, h: i64| {
            let base = at(tp.start, d, t, n, h);
            Point::new(
                (base.x as f64 + n.0 as f64 * toward as f64 * half).round() as i64,
                (base.y as f64 + n.1 as f64 * toward as f64 * half).round() as i64,
            )
        };
        // The two tracks of a meander of the centreline: offset to either side, from a
        // step before it to a step after.
        let tracks_of = |p: i64, h: i64| -> [Vec<Point>; 2] {
            let line = [
                centre(p - s, 0),
                centre(p, 0),
                centre(p, h),
                centre(p + s, h),
                centre(p + s, 0),
                centre(p + 2 * s, 0),
            ];
            // `n` is to the left of `d`, as `offset` goes, and the positive net is on the
            // side of the centreline away from `toward`.
            let off = toward as f64 * spacing / 2.0;
            let (a, b) = (offset(&line, -off), offset(&line, off));
            [a[1..5].to_vec(), b[1..5].to_vec()]
        };
        let mut left = need;
        let mut bumps: Vec<(i64, i64)> = Vec::new();
        let mut pos_at = lo + margin;
        // The tracks are out of the world while their meanders are tried.
        for k in [i, j] {
            if let Some(p) = self.placed[k].take() {
                self.world.remove(&p);
            }
        }
        while pos_at + s + margin <= hi && left >= 1.0 {
            let want = ((left / (2.0 * ul)).ceil() as i64).min(cap);
            if want < 1 {
                break;
            }
            let mut best: Option<(i64, i64)> = None;
            for side in [1, -1] {
                for h in heights(want, lowest.min(want)) {
                    if best.is_some_and(|(b, _)| b >= h) {
                        break;
                    }
                    let [a, b] = tracks_of(pos_at, h * side);
                    if self.pair_legal((pos, neg), layer, (tp.width, tn.width), &a, &b, &[i, j]) {
                        best = Some((h, side));
                        break;
                    }
                }
            }
            match best {
                Some((h, side)) => {
                    bumps.push((pos_at, h * side));
                    left -= 2.0 * h as f64 * ul;
                    pos_at += 2 * s;
                }
                None => pos_at += (s / 2).max(1),
            }
        }
        if bumps.is_empty() {
            self.place(i);
            self.place(j);
            return 0.0;
        }
        // Both tracks in pieces, the negative's in the positive's direction.
        let mut lines: [Vec<Point>; 2] = [vec![tp.start], vec![]];
        let n_forward = (tn.end.x - tn.start.x) * d.0 + (tn.end.y - tn.start.y) * d.1 > 0;
        let (n_first, n_last) = if n_forward {
            (tn.start, tn.end)
        } else {
            (tn.end, tn.start)
        };
        lines[1].push(n_first);
        for &(p, h) in &bumps {
            let [a, b] = tracks_of(p, h);
            lines[0].extend(a);
            lines[1].extend(b);
        }
        lines[0].push(tp.end);
        lines[1].push(n_last);
        let before = [length(tp.start, tp.end), length(tn.start, tn.end)];
        let make = |points: &[Point], t: &Track| -> Vec<Track> {
            points
                .windows(2)
                .filter(|w| w[0] != w[1])
                .map(|w| Track {
                    start: w[0],
                    end: w[1],
                    ..t.clone()
                })
                .collect()
        };
        let (pp, np) = (make(&lines[0], &tp), make(&lines[1], &tn));
        let after = [
            pp.iter().map(|t| length(t.start, t.end)).sum::<f64>(),
            np.iter().map(|t| length(t.start, t.end)).sum::<f64>(),
        ];
        let (old_p, old_n) = (tp.clone(), tn.clone());
        let pi = self.replace(i, pp);
        let ni = self.replace(j, np);
        // Every new track, checked exactly against everything, the other net's included.
        let all_ok = pi.iter().chain(&ni).all(|&k| {
            let t = &self.tracks[k];
            t.net.is_some_and(|net| {
                Shape::track(t.start, t.end, t.width)
                    .is_ok_and(|s| self.world.track(net, t.layer, &s, None) == Verdict::Free)
            })
        });
        if !all_ok {
            self.undo_pieces(i, &pi, old_p);
            self.undo_pieces(j, &ni, old_n);
            return 0.0;
        }
        (after[0] - before[0]).min(after[1] - before[1])
    }

    /// Takes back what [`replace`](Tuner::replace) did, as far as tracks go: the pieces
    /// past the first are left as empty tracks, which are dropped at the end.
    fn undo_pieces(&mut self, i: usize, pieces: &[usize], old: Track) {
        for &k in pieces {
            if let Some(p) = self.placed[k].take() {
                self.world.remove(&p);
            }
            if k != i {
                self.tracks[k].end = self.tracks[k].start;
            }
        }
        self.tracks[i] = old;
        self.place(i);
    }

    /// Whether the two tracks of a pair's meander are legal, each against the world and
    /// its net's own copper, and the two against each other.
    fn pair_legal(
        &mut self,
        (pos, neg): (NetId, NetId),
        layer: LayerId,
        (wp, wn): (i64, i64),
        a: &[Point],
        b: &[Point],
        except: &[usize],
    ) -> bool {
        let ok_a = a
            .windows(2)
            .all(|w| w[0] == w[1] || self.legal(pos, layer, w[0], w[1], wp, except));
        if !ok_a {
            return false;
        }
        let mut placed = Vec::new();
        for w in a.windows(2).filter(|w| w[0] != w[1]) {
            if let Ok(s) = Shape::track(w[0], w[1], wp) {
                placed.push(self.world.add_track(pos, layer, s));
            }
        }
        let ok_b = b
            .windows(2)
            .all(|w| w[0] == w[1] || self.legal(neg, layer, w[0], w[1], wn, except));
        for p in &placed {
            self.world.remove(p);
        }
        ok_b
    }
}

/// What a constraint asks of each of its nets, given their lengths: the shortest it may
/// be, and the length to aim at if it is shorter.
fn window(c: &LengthConstraint, lengths: &[f64]) -> (f64, f64) {
    let longest = lengths.iter().copied().fold(0.0, f64::max);
    let min = c.min.map_or(0.0, |m| m as f64);
    let lo = match c.max_skew {
        Some(skew) => min.max(longest - skew as f64),
        None => min,
    };
    let aim = match (c.target, c.min, c.max) {
        (Some(t), _, _) => t as f64,
        (None, Some(a), Some(b)) => (a as f64 + b as f64) / 2.0,
        (None, Some(a), None) => a as f64,
        _ => 0.0,
    };
    let aim = if c.max_skew.is_some() {
        aim.max(longest)
    } else {
        aim
    };
    let aim = match c.max {
        Some(m) => aim.min(m as f64),
        None => aim,
    };
    (lo, aim.max(lo))
}

/// Whether the lengths meet a constraint, to within a nanometre.
fn met(c: &LengthConstraint, lengths: &[f64]) -> bool {
    let longest = lengths.iter().copied().fold(0.0, f64::max);
    let shortest = lengths.iter().copied().fold(f64::INFINITY, f64::min);
    c.min.is_none_or(|m| shortest + 1.0 >= m as f64)
        && c.max.is_none_or(|m| longest <= m as f64 + 1.0)
        && c.max_skew
            .is_none_or(|s| longest - shortest <= s as f64 + 1.0)
}

/// Adds meanders to `routed` for the design's lengths.
pub(crate) fn run(
    design: &Design,
    routed: &mut Routed,
    options: &RouteOptions,
) -> Result<(), GeomError> {
    let rules = &design.rules;
    let mut world = World::new(
        &design.board,
        rules,
        &design.outline,
        design.edge_clearance,
        &design.obstacles,
    )?;
    let mut placed = Vec::with_capacity(routed.tracks.len());
    for t in &routed.tracks {
        let shape = match t.mid {
            None => Shape::track(t.start, t.end, t.width)?,
            Some(m) => Shape::arc_track(t.start, m, t.end, t.width, design.board.arc_tolerance)?,
        };
        placed.push(t.net.map(|net| world.add_track(net, t.layer, shape)));
    }
    for v in &routed.vias {
        if let Some(net) = v.net {
            world.add_via(net, v.at, v.diameter, v.drill);
        }
    }
    let mut tuner = Tuner {
        design,
        world,
        tracks: routed.tracks.clone(),
        placed,
        vias: routed.vias.clone(),
        amplitude: options.max_amplitude.max(0),
        log: Vec::new(),
    };
    let total = |tracks: &[Track]| -> f64 {
        tracks
            .iter()
            .filter(|t| t.mid.is_none())
            .map(|t| length(t.start, t.end))
            .sum()
    };
    let before = total(&tuner.tracks);
    let routed_net = |net: NetId| !routed.unrouted.contains(&net);
    // By constraint, its nets that are routed, and what it asks of them before meanders.
    let mut groups: Vec<(Vec<NetId>, f64, f64)> = Vec::new();
    for c in &design.lengths {
        let mut nets: Vec<NetId> = Vec::new();
        for &n in &c.nets {
            if !nets.contains(&n) && routed_net(n) && tuner.length(n) > 0.0 {
                nets.push(n);
            }
        }
        if nets.is_empty() {
            continue;
        }
        let lengths: Vec<f64> = nets.iter().map(|&n| tuner.length(n)).collect();
        let (lo, aim) = window(c, &lengths);
        groups.push((nets.clone(), lo, aim));
        // Pairs first, together.
        for pair in &design.pairs {
            if !(nets.contains(&pair.positive) && nets.contains(&pair.negative)) {
                continue;
            }
            let longer = tuner.length(pair.positive).max(tuner.length(pair.negative));
            if longer < lo {
                let gap = (0..rules.layer_count())
                    .map(|l| {
                        rules.clearance_between(
                            Some(pair.positive),
                            Some(pair.negative),
                            LayerId(l as u8),
                        )
                    })
                    .fold(pair.gap, i64::max);
                tuner.lengthen_pair((pair.positive, pair.negative), gap, aim - longer);
            }
        }
        // Then each net on its own, to what is left, with the pairs' skews.
        let lengths: Vec<f64> = nets.iter().map(|&n| tuner.length(n)).collect();
        let (lo, aim) = window(c, &lengths);
        // The shortest first, as they need the most room.
        let mut order: Vec<(f64, NetId)> = nets.iter().map(|&n| (tuner.length(n), n)).collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (have, n) in order {
            if have < lo {
                tuner.lengthen(n, aim - have);
            }
        }
    }
    // Nets still short: their neighbours' meanders taken out, they are meandered first,
    // then the neighbours again, and that is kept if fewer nets are short, or by less.
    let shortfall = |tuner: &Tuner<'_>| -> (usize, f64) {
        let mut count = 0;
        let mut sum = 0.0;
        for (nets, lo, _) in &groups {
            for &n in nets {
                let have = tuner.length(n);
                if have + 1.0 < *lo {
                    count += 1;
                    sum += lo - have;
                }
            }
        }
        (count, sum)
    };
    for _ in 0..MAKE_ROOM_ROUNDS {
        let mut short: Vec<(f64, NetId, f64, f64)> = Vec::new();
        for (nets, lo, aim) in &groups {
            for &n in nets {
                let have = tuner.length(n);
                if have + 1.0 < *lo && !tuner.paired(n) {
                    short.push((lo - have, n, *lo, *aim));
                }
            }
        }
        if short.is_empty() {
            break;
        }
        short.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut better = false;
        for (need, n, lo, aim) in short {
            if tuner.length(n) + 1.0 >= lo {
                continue;
            }
            let was = shortfall(&tuner);
            let saved = (tuner.tracks.clone(), tuner.log.clone());
            let width = design.nets.get(n.index()).map_or(0, |s| s.track_width);
            let reach = ((need / 2.0) as i64 + 2 * (width + rules.max_clearance()))
                .min(tuner.amplitude + 2 * (width + rules.max_clearance()));
            let near = tuner.neighbours(n, reach);
            if near.is_empty() {
                continue;
            }
            for &v in &near {
                tuner.unmeander(v);
            }
            let have = tuner.length(n);
            tuner.lengthen(n, aim - have);
            // The neighbours again, those that need most first.
            let mut again: Vec<(f64, NetId, f64)> = Vec::new();
            for &v in &near {
                if let Some((_, lo, aim)) = groups.iter().find(|(nets, _, _)| nets.contains(&v)) {
                    let have = tuner.length(v);
                    if have < *lo {
                        again.push((aim - have, v, *aim));
                    }
                }
            }
            again.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            for (_, v, aim) in again {
                let have = tuner.length(v);
                tuner.lengthen(v, aim - have);
            }
            let now = shortfall(&tuner);
            if now.0 < was.0 || (now.0 == was.0 && now.1 + 1.0 < was.1) {
                better = true;
            } else {
                (tuner.tracks, tuner.log) = saved;
                tuner.rebuild()?;
            }
        }
        if !better {
            break;
        }
    }
    routed.stats.meanders = total(&tuner.tracks) - before;
    routed.stats.lengths = (
        design.lengths.len(),
        design
            .lengths
            .iter()
            .filter(|c| {
                let lengths: Vec<f64> = c
                    .nets
                    .iter()
                    .filter(|&&n| routed_net(n))
                    .map(|&n| tuner.length(n))
                    .collect();
                met(c, &lengths)
            })
            .count(),
    );
    // A track is what it was, unless a meander took its place: the pieces of one are
    // added past the tracks there were, but for the first, which takes the track's place.
    let mut kinds = Vec::with_capacity(tuner.tracks.len());
    for (i, t) in tuner.tracks.iter().enumerate() {
        if t.start == t.end {
            continue;
        }
        kinds.push(match (routed.tracks.get(i), routed.track_kinds.get(i)) {
            (Some(old), Some(&kind)) if old == t => kind,
            _ => super::CopperKind::Meander,
        });
    }
    routed.track_kinds = kinds;
    tuner.tracks.retain(|t| t.start != t.end);
    routed.tracks = tuner.tracks;
    Ok(())
}
