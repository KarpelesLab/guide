//! Crossings in a row of fine pitch: two nets whose pads take turns along it.
//!
//! The pads of a USB receptacle's two data lines are side by side in the order D+ D− D+
//! D−, at a pitch that leaves no track a way between two of them, with the board's edge
//! behind the row. On the pads' layer only one of the two nets can join its pads: the
//! track that joins them goes round the pad of the other net between them, and that pad
//! is then shut in, as a via at its end is in the way of the tracks out of the pads
//! beside it. A search for one net at a time finds the track first, and the price of
//! crossing it does not move it far enough: the place for the via exists only if the two
//! tracks beside it both step aside, and no route of one net says so.
//!
//! So the crossing is planned before anything is routed, where it is the hard one. The
//! pad between two pads of another net, with a pad of its own net beyond one of them,
//! is looked for a via straight out of its end, at the least depth at which the via is
//! legal and both pads beside it still have a way out past it: straight on, a step aside
//! at 45° by as much as the via needs, and straight on to the via's depth.
//!
//! Where the two ways then have room to join round the via, nothing is planned: rip-up
//! and the negotiations find that crossing themselves, and copper fixed there was no
//! gain on three boards and in the way on two. Where they have none, as with a part's
//! pad a millimetre and a half in front of the receptacle, the two nets have to cross on
//! another layer, and that is what a run did not find: the net's other pad then gets a
//! via of its own, clear of the way between the two, a track on another layer joins the
//! two vias, and the pads further along the row that the ways step towards get a step
//! aside of their own, each by less, until one has room straight on. All of it is fixed
//! copper of its nets from then on, as the escapes of an array are: as routes that
//! rip-up may take out, the via and the ways were gone before the nets beside them had
//! settled, on a board where every net near the receptacle is in another's way.

use std::collections::BTreeMap;

use super::Design;
use super::escape;
use super::geometry::Seg;
use super::router::{Job, pad_center};
use super::rows::{self, Row, Stub};
use super::world::{Placed, Verdict, World};
use crate::geom::{LayerId, Point, Shape};
use crate::id::NetId;

/// The step of a via's depth, and the deepest a via is looked for past its pad's end.
const STEP: i64 = 25_000;
const DEPTH: i64 = 3_000_000;

/// How many pads beyond those beside a via are looked at, either way, for a way out.
const CASCADE: usize = 4;

/// What is added to a clearance for the rounding of a step aside.
const TOL: i64 = 2_000;

/// A direction of a row, with what a nanometre along it is in each coordinate, in parts
/// of 10 000: a row along a diagonal has steps of a unit each way.
#[derive(Clone, Copy)]
struct Dir {
    x: i64,
    y: i64,
    diagonal: bool,
}

impl Dir {
    fn of((x, y): (i64, i64)) -> Dir {
        Dir {
            x,
            y,
            diagonal: x != 0 && y != 0,
        }
    }

    /// `p` moved by `len` this way.
    fn go(self, p: Point, len: i64) -> Point {
        let c = if self.diagonal {
            len * 7071 / 10_000
        } else {
            len
        };
        Point::new(p.x + self.x * c, p.y + self.y * c)
    }

    fn back(self) -> Dir {
        Dir {
            x: -self.x,
            y: -self.y,
            ..self
        }
    }
}

/// A pad of a row that has a net to route: its job, net, centre, terminal, track width
/// on the row's layer, and via.
#[derive(Clone, Copy)]
struct Lead {
    job: usize,
    net: NetId,
    center: Point,
    terminal: usize,
    width: i64,
    via: Option<(i64, i64)>,
}

/// A crossing as planned: the first via's depth, the vias by lead of the row, and the
/// tracks by lead and layer, as points.
struct Crossing {
    depth: i64,
    vias: Vec<(usize, Point)>,
    tracks: Vec<(usize, LayerId, Vec<Point>)>,
    /// Whether the two ways past the via are joined round it.
    around: bool,
}

/// Places `net`'s track through `pts` in `world`, if every part of it is free.
fn lay(
    world: &mut World<'_>,
    net: NetId,
    layer: LayerId,
    width: i64,
    pts: &[Point],
    placed: &mut Vec<Placed>,
) -> bool {
    let from = placed.len();
    for s in pts.windows(2) {
        let free = Shape::track(s[0], s[1], width)
            .ok()
            .filter(|t| world.track(net, layer, t, None) == Verdict::Free);
        match free {
            Some(t) => placed.push(world.add_track(net, layer, t)),
            None => {
                for p in placed.drain(from..) {
                    world.remove(&p);
                }
                return false;
            }
        }
    }
    true
}

/// Whether `net`'s track through `pts` is free in `world`.
fn clear(world: &World<'_>, net: NetId, layer: LayerId, width: i64, pts: &[Point]) -> bool {
    pts.windows(2).all(|s| {
        Shape::track(s[0], s[1], width)
            .is_ok_and(|t| world.track(net, layer, &t, None) == Verdict::Free)
    })
}

/// The points of a way out of `from`: `t0` straight on, a step aside by `aside` at 45°
/// towards `away`, and straight on to `depth`.
fn way(from: Point, out: Dir, away: Dir, t0: i64, aside: i64, depth: i64) -> Vec<Point> {
    let p1 = out.go(from, t0);
    let p2 = away.go(out.go(p1, aside), aside);
    let p3 = out.go(p2, depth - t0 - aside);
    let mut pts = vec![from, p1, p2, p3];
    pts.dedup();
    pts
}

/// Plans the via of the lead at `k` of `row` on the side `out`, at the least depth at
/// which it and the ways past it are legal in `world`, which is left as it was. With
/// `looped`, only where the two ways have room to join round the via.
fn crossing(
    design: &Design,
    jobs: &[Job],
    world: &mut World<'_>,
    (row, leads): (&Row, &[Option<Lead>]),
    (k, out): (usize, Dir),
    looped: bool,
) -> Option<Crossing> {
    let lead = leads[k]?;
    let (d, drill) = lead.via?;
    let layer = row.layer;
    let rules = &design.rules;
    // Along the row, towards the pads after `k`.
    let (a, b) = (leads[k - 1]?, leads[k + 1]?);
    let along = {
        let (dx, dy) = (b.center.x - lead.center.x, b.center.y - lead.center.y);
        Dir::of((dx.signum(), dy.signum()))
    };
    let mut depth = row.half.div_euclid(STEP) * STEP + STEP;
    while depth <= row.half + DEPTH {
        let v = out.go(lead.center, depth);
        depth += STEP;
        if world.via(lead.net, v, d, drill, None) != Verdict::Free {
            continue;
        }
        let mut placed: Vec<Placed> = Vec::new();
        if !lay(
            world,
            lead.net,
            layer,
            lead.width,
            &[lead.center, v],
            &mut placed,
        ) {
            continue;
        }
        placed.push(world.add_via(lead.net, v, d, drill));
        let deep = depth - STEP;
        let mut exits: Vec<(usize, Vec<Point>)> = Vec::new();
        let mut asides = [0i64; 2];
        for (n, (i, other, away)) in [(k - 1, a, along.back()), (k + 1, b, along)]
            .into_iter()
            .enumerate()
        {
            let gap = rules.clearance_between(Some(lead.net), Some(other.net), layer);
            let aside = (d / 2 + other.width / 2 + gap + TOL - row.pitch).max(0);
            asides[n] = aside;
            // The deepest turn first: the way stays in its pad's line for as long as the
            // via lets it, which leaves the pad beyond it room to turn away first.
            let mut t0 = (deep - aside).div_euclid(STEP) * STEP;
            while t0 >= 0 {
                let pts = way(other.center, out, away, t0, aside, deep);
                if lay(world, other.net, layer, other.width, &pts, &mut placed) {
                    exits.push((i, pts));
                    break;
                }
                t0 -= STEP;
            }
            if exits.len() <= n {
                break;
            }
        }
        let mut tracks: Vec<(usize, LayerId, Vec<Point>)> = vec![(k, layer, vec![lead.center, v])];
        let mut vias = vec![(k, v)];
        let mut ok = exits.len() == 2;
        tracks.extend(exits.into_iter().map(|(i, pts)| (i, layer, pts)));
        // The two ways are of one net, which joins them round the via: straight on from
        // each, and across beyond the via, the corners cut at 45°.
        let mut around = false;
        if ok && looped {
            let reach = row.pitch + asides[0];
            let cut = reach * 4142 / 10_000;
            let (back, on) = (along.back(), along);
            let from = tracks[1].2.last().copied().unwrap_or(v);
            let to = tracks[2].2.last().copied().unwrap_or(v);
            let pts = vec![
                from,
                out.go(from, cut),
                out.go(back.go(v, cut), reach),
                out.go(on.go(v, cut), reach),
                out.go(to, cut),
                to,
            ];
            around = asides[0] == asides[1] && lay(world, a.net, layer, a.width, &pts, &mut placed);
            ok = around;
            tracks.push((k - 1, layer, pts));
        }
        let far = if around {
            deep + row.pitch + asides[0]
        } else {
            deep
        };
        // The pads beyond those two still get out: straight on, or with a step aside
        // before the way beside them takes its own, which is then copper of theirs too,
        // as a lattice has no such step; and so on along the row, each stepping aside by
        // less, until one has room straight on. The one of the via's own net, where the
        // ways are not joined round the via, gets a via of its own, clear of the way
        // between the two, and the two vias are joined on another layer: a track from that
        // pad to the first via on the row's layer would be across the way between them.
        for (n, away) in [(0, along.back()), (1, along)] {
            // How far the way before this one stepped towards it, and how deep it goes.
            let mut pushed = asides[n];
            let mut far = far;
            for i in 2..2 + CASCADE {
                if !ok {
                    break;
                }
                let o = if n == 0 {
                    k.checked_sub(i)
                } else {
                    Some(k + i)
                };
                let Some(o) = o.filter(|&o| o < leads.len()) else {
                    break;
                };
                // A pad of no net has nothing to get out.
                let Some(outer) = leads[o] else { continue };
                if i == 2 && !around && outer.net == lead.net && outer.terminal != lead.terminal {
                    let between = if n == 0 { a } else { b };
                    let gap = rules.clearance_between(Some(lead.net), Some(between.net), layer);
                    let shift =
                        (d / 2 + between.width / 2 + gap + TOL - (row.pitch - pushed)).max(0);
                    let mut found = None;
                    let mut t0 = 0;
                    'partner: while t0 + shift <= DEPTH {
                        let turn = way(outer.center, out, away, t0, shift, t0 + shift);
                        if !clear(world, outer.net, layer, outer.width, &turn) {
                            t0 += STEP;
                            continue;
                        }
                        let mut end = t0 + shift;
                        while end <= row.half + DEPTH {
                            let pts = way(outer.center, out, away, t0, shift, end);
                            let at = pts.last().copied().unwrap_or(outer.center);
                            if end > row.half
                                && world.via(outer.net, at, d, drill, None) == Verdict::Free
                                && clear(world, outer.net, layer, outer.width, &pts)
                            {
                                found = Some((pts, at, end));
                                break 'partner;
                            }
                            end += STEP;
                        }
                        t0 += STEP;
                    }
                    let Some((pts, at, end)) = found else {
                        ok = false;
                        break;
                    };
                    ok = lay(world, outer.net, layer, outer.width, &pts, &mut placed);
                    placed.push(world.add_via(outer.net, at, d, drill));
                    tracks.push((o, layer, pts));
                    vias.push((o, at));
                    // The two vias joined on the layer furthest from the row's that has
                    // room.
                    let layers = rules.layer_count() as u8;
                    let mut others: Vec<u8> = (0..layers).filter(|&l| l != layer.0).collect();
                    if layer.0 == 0 {
                        others.reverse();
                    }
                    let (dx, dy) = (v.x - at.x, v.y - at.y);
                    let m = dx.abs().min(dy.abs());
                    let diagonal = Point::new(at.x + dx.signum() * m, at.y + dy.signum() * m);
                    let straight = Point::new(v.x - dx.signum() * m, v.y - dy.signum() * m);
                    'join: for l in others {
                        let width = jobs[outer.job].ctx.widths[usize::from(l)];
                        for mid in [diagonal, straight] {
                            let mut pts = vec![at, mid, v];
                            pts.dedup();
                            if lay(world, outer.net, LayerId(l), width, &pts, &mut placed) {
                                tracks.push((o, LayerId(l), pts));
                                break 'join;
                            }
                        }
                    }
                    pushed = shift + d / 2 - outer.width / 2;
                    far = far.max(end + d / 2);
                    continue;
                }
                let on = [outer.center, out.go(outer.center, far)];
                if pushed == 0 || clear(world, outer.net, layer, outer.width, &on) {
                    break;
                }
                let mut found = None;
                let mut aside = STEP;
                'aside: while aside <= pushed + 2 * STEP {
                    let mut t0 = 0;
                    while t0 + aside <= far {
                        let pts = way(outer.center, out, away, t0, aside, far);
                        if clear(world, outer.net, layer, outer.width, &pts) {
                            found = Some((pts, aside));
                            break 'aside;
                        }
                        t0 += STEP;
                    }
                    aside += STEP;
                }
                let Some((pts, aside)) = found else {
                    ok = false;
                    break;
                };
                ok = lay(world, outer.net, layer, outer.width, &pts, &mut placed);
                tracks.push((o, layer, pts));
                pushed = aside;
            }
        }
        for p in &placed {
            world.remove(p);
        }
        if ok {
            return Some(Crossing {
                depth: deep,
                vias,
                tracks,
                around,
            });
        }
    }
    None
}

/// Plans the crossings of a design's rows of fine pitch against `world`, which is left as
/// it was: for each pair of nets whose pads take turns along a row, a via out of one pad
/// and the ways of the pads beside it past the via, as stubs of their nets.
pub(crate) fn plan(design: &Design, world: &mut World<'_>, jobs: &[Job]) -> Vec<Stub> {
    let rules = &design.rules;
    if rules.layer_count() < 2 {
        return Vec::new();
    }
    let pads = &design.board.pads;
    let need = |job: &Job, layer: LayerId| {
        job.ctx.via.map(|(d, _)| {
            d / 2
                + rules.clearance(Some(job.ctx.net), layer)
                + job.ctx.widths[usize::from(layer.0)] / 2
        })
    };
    let Some(reach) = jobs
        .iter()
        .filter_map(|j| need(j, LayerId(0)))
        .max()
        .map(|n| 2 * n)
    else {
        return Vec::new();
    };
    let rows = rows::rows(design, reach);
    if rows.is_empty() {
        return Vec::new();
    }
    let mut by_box: BTreeMap<(Point, Point), usize> = BTreeMap::new();
    for r in &rows {
        for &p in &r.pads {
            let b = pads[p].shape.bbox();
            by_box.insert((b.min(), b.max()), p);
        }
    }
    let mut lead_of: BTreeMap<usize, Lead> = BTreeMap::new();
    for (j, job) in jobs.iter().enumerate() {
        for t in &job.ctx.pads {
            if let Some(&p) = by_box.get(&(t.bbox.min(), t.bbox.max()))
                && pads[p].net == Some(job.ctx.net)
                && pad_center(&pads[p].shape) == Some(t.center)
                && !escape::joined(design, p)
            {
                lead_of.entry(p).or_insert(Lead {
                    job: j,
                    net: job.ctx.net,
                    center: t.center,
                    terminal: t.terminal,
                    width: 0,
                    via: job.ctx.via,
                });
            }
        }
    }
    // By row, its leads with their widths on its layer, and the pads that cross: the
    // lead between two pads of another net, with one of its own net beyond either.
    let mut found: Vec<(i64, NetId, usize, usize, Dir)> = Vec::new();
    let mut leads_of: Vec<Vec<Option<Lead>>> = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let l = usize::from(row.layer.0);
        let leads: Vec<Option<Lead>> = row
            .pads
            .iter()
            .map(|p| {
                lead_of.get(p).map(|lead| Lead {
                    width: jobs[lead.job].ctx.widths[l],
                    ..*lead
                })
            })
            .collect();
        for k in 1..leads.len().saturating_sub(1) {
            let (Some(x), Some(a), Some(b)) = (leads[k], leads[k - 1], leads[k + 1]) else {
                continue;
            };
            let fine = need(&jobs[x.job], row.layer).is_some_and(|n| row.pitch < n);
            if !fine || a.net == x.net || a.net != b.net || a.terminal == b.terminal {
                continue;
            }
            let own = |o: Option<usize>| {
                o.and_then(|o| leads.get(o).copied().flatten())
                    .is_some_and(|o| o.net == x.net && o.terminal != x.terminal)
            };
            if !own(k.checked_sub(2)) && !own(Some(k + 2)) {
                continue;
            }
            for side in rows::sides(row.way) {
                let out = Dir::of(side);
                let c = crossing(design, jobs, world, (row, &leads), (k, out), true)
                    .or_else(|| crossing(design, jobs, world, (row, &leads), (k, out), false));
                if let Some(c) = c {
                    found.push((c.depth + i64::from(!c.around) * DEPTH, x.net, r, k, out));
                }
            }
        }
        leads_of.push(leads);
    }
    found.sort_by_key(|&(depth, net, r, k, out)| (depth, net, r, k, out.x, out.y));
    let mut placed: Vec<Placed> = Vec::new();
    let mut done: Vec<(NetId, NetId)> = Vec::new();
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut out_stubs: Vec<Stub> = Vec::new();
    for (_, net, r, k, out) in found {
        let (row, leads) = (&rows[r], &leads_of[r]);
        let (Some(x), Some(a)) = (leads[k], leads[k - 1]) else {
            continue;
        };
        let pair = (net.min(a.net), net.max(a.net));
        if done.contains(&pair) || (k - 1..=k + 1).any(|i| taken.contains(&(r, i))) {
            continue;
        }
        // Planned again with the crossings placed so far.
        let c = crossing(design, jobs, world, (row, leads), (k, out), true)
            .or_else(|| crossing(design, jobs, world, (row, leads), (k, out), false));
        let Some(c) = c else { continue };
        // A crossing with room for the two ways to join round the via is left to the
        // routing.
        if c.around {
            done.push(pair);
            continue;
        }
        let Some((d, drill)) = x.via else { continue };
        done.push(pair);
        taken.extend((k - 1..=k + 1).map(|i| (r, i)));
        for (i, layer, pts) in c.tracks {
            let Some(other) = leads[i] else { continue };
            let width = jobs[other.job].ctx.widths[usize::from(layer.0)];
            for s in pts.windows(2) {
                if let Ok(t) = Shape::track(s[0], s[1], width) {
                    placed.push(world.add_track(other.net, layer, t));
                }
                let at = c.vias.iter().find(|&&(v, at)| v == i && at == s[1]);
                out_stubs.push(Stub {
                    job: other.job,
                    seg: Seg {
                        a: s[0],
                        b: s[1],
                        l: layer.0,
                    },
                    via: at.filter(|_| layer == row.layer).map(|&(_, at)| at),
                });
            }
        }
        for &(_, at) in &c.vias {
            placed.push(world.add_via(net, at, d, drill));
        }
    }
    for p in &placed {
        world.remove(p);
    }
    out_stubs
}

#[cfg(test)]
mod tests {
    use super::super::control::Run;
    use super::super::router;
    use super::super::rows::Stub;
    use super::super::tests::{MM, design, outline, route_checked, smd, through};
    use super::super::world::World;
    use super::super::{Design, RouteOptions, RunControl};
    use super::plan;
    use crate::geom::drc::Pad;
    use crate::geom::{Point, Rect, Shape};

    /// A pad of 0.3 by 0.8 mm, upright, on the top layer.
    fn lead(x: i64, net: u32) -> Pad {
        Pad {
            shape: Shape::rect(Rect::new(
                Point::new(x - 150_000, -400_000),
                Point::new(x + 150_000, 400_000),
            ))
            .unwrap(),
            ..smd(x, 0, net)
        }
    }

    /// A receptacle at the board's edge: six leads at 0.5 mm, the four in the middle of
    /// two nets that take turns, and each net's other pad far above the row. With
    /// `diode`, a pad of another net 1.85 mm above the row, over the two in the middle.
    fn receptacle(diode: bool) -> Design {
        let mut pads = vec![
            lead(0, 3),
            lead(500_000, 1),
            lead(1_000_000, 2),
            lead(1_500_000, 1),
            lead(2_000_000, 2),
            lead(2_500_000, 4),
        ];
        for net in 1..=4 {
            pads.push(through(i64::from(net) * 2 * MM - 4 * MM, 8 * MM, net, 2));
        }
        if diode {
            pads.push(smd(1_250_000, 1_850_000, 3));
        }
        let mut d = design(2, pads, 5);
        d.outline = outline(Rect::new(
            Point::new(-6 * MM, -600_000),
            Point::new(8 * MM, 10 * MM),
        ));
        d
    }

    fn planned(d: &Design) -> Vec<Stub> {
        let options = RouteOptions {
            vias_in_pads: false,
            ..RouteOptions::default()
        };
        let control = RunControl::default();
        let run = Run::new(&control, Some(1_000_000));
        let mut world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )
        .unwrap();
        world.configure(d, &options);
        world.set_run(&run);
        let pitch = router::pitch(d, &options);
        let jobs = router::jobs(d, &world, pitch, &[], &[]);
        plan(d, &mut world, &jobs)
    }

    #[test]
    fn a_crossing_with_room_round_the_via_is_left_to_the_routing() {
        assert!(planned(&receptacle(false)).is_empty());
    }

    #[test]
    fn with_no_room_round_the_via_the_net_s_other_pad_gets_one_and_a_track_under() {
        let stubs = planned(&receptacle(true));
        let vias: Vec<Point> = stubs.iter().filter_map(|s| s.via).collect();
        assert_eq!(vias.len(), 2, "{stubs:?}");
        // The second via is that of the pad two along, further from the first than the
        // pads are from each other, and a track on the other layer joins the two.
        assert!((vias[0].x - vias[1].x).abs() > 1_000_000, "{vias:?}");
        let under: Vec<_> = stubs.iter().filter(|s| s.seg.l == 1).collect();
        assert!(!under.is_empty(), "{stubs:?}");
        for v in &vias {
            assert!(
                under.iter().any(|s| s.seg.a == *v || s.seg.b == *v),
                "{stubs:?}"
            );
        }
    }

    #[test]
    fn a_run_with_a_budget_joins_both_nets_of_a_crossing() {
        for diode in [false, true] {
            let d = receptacle(diode);
            let options = RouteOptions {
                vias_in_pads: false,
                work_budget: Some(50_000_000),
                ..RouteOptions::default()
            };
            let routed = route_checked(&d, &options);
            assert!(routed.unrouted.is_empty(), "{:?}", routed.unrouted);
            assert_eq!(routed.stats.crossings, 2 * usize::from(diode));
            let without = RouteOptions {
                crossings: false,
                ..options
            };
            assert_eq!(route_checked(&d, &without).stats.crossings, 0);
        }
    }
}
