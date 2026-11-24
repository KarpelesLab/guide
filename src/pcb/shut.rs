//! A pad shut in: the routes that leave a pad of a pour no way to its fill laid another
//! way, round the stub that joins it.
//!
//! Once the routes are where they stay, what they cut off the fills is joined to them
//! again by stubs that move nothing ([`super::pours`]). A pad between the tracks of its
//! neighbours on a package of fine pitch has no such stub: every way from it is another
//! net's copper, and the search for it comes back with nothing. The pad then leaves the
//! run on a piece of its own, a connection open that no later search closes among routes
//! that are fixed.
//!
//! So for each net with a piece that the stubs left cut off, the search for the stubs is
//! made again with the places near the pads that it was not given before, and if that
//! joins nothing more, nets round what is cut off are taken out: first those that the
//! stub's cheapest way across the routes runs into, at a price for each route crossed and
//! for the places where nets have been in each other's way, then those with copper nearest
//! to it, one at a time and then two together. The stub is searched for again, and with it in place
//! those nets are routed again round it, moving nothing, or by a chain that moves what is
//! in their way and joins it up again ([`Settle::mend`]), the stub's net not crossed. If
//! the stub was found, every net is whole again and the cells tell no pad of any pour
//! cut off that was not, and this net's with less cut off ([`Pours::rejoin`]), that stays;
//! if not, everything is as it was, to the track.

use super::Design;
use super::cleanup;
use super::detail::{self, Params, RipUp, StepCache};
use super::geometry::NetCopper;
use super::pours::{self, Pours, Refilled};
use super::router::{Job, completion};
use super::settle::{Before, Routes, Settle};
use super::world::World;
use crate::geom::Point;

/// How far from a pad that is shut in the copper of a net may be for the net to be one of
/// those taken out for it.
const NEAR: i64 = 1_200_000;

/// How many nets near a pad are tried alone, how many of the nearest are tried two
/// together, and how many tries a net with pads shut in has in all.
const ALONE: usize = 5;
const PAIRED: usize = 4;
const TRIES: usize = 9;

/// How many times the search for the stubs is made again with new places to end at.
const AGAIN: usize = 3;

/// What crossing a route costs the search for the stub that tells which nets to take out
/// first, in millimetres of track, and how many nets at most it may run into for those to
/// be a try.
const CROSSING: f64 = 10.0;
const CROSSED: usize = 3;

/// What the step did: the tries made, whether one was kept, and the nodes that its
/// searches expanded.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Opened {
    pub(crate) tried: usize,
    pub(crate) kept: bool,
    pub(crate) expanded: u64,
}

/// What lays a net again by a chain ([`Settle::mend`]): the settling of the run, what its
/// nets are missing, and how many connections the routes leave open in all.
pub(crate) struct Mender<'a, 'b> {
    pub(crate) settle: Settle<'a>,
    pub(crate) missing: &'b mut [Vec<usize>],
    pub(crate) apart: &'b mut [usize],
    pub(crate) open: usize,
}

/// The square of the distance from `p` to the segment `a`–`b`, in nm², as a float.
fn distance2(p: Point, a: Point, b: Point) -> f64 {
    let (px, py) = ((p.x - a.x) as f64, (p.y - a.y) as f64);
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        ((px * dx + py * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (ex, ey) = (px - t * dx, py - t * dy);
    ex * ex + ey * ey
}

/// How many pieces the routes cut off the fills of each net with a planned fill, by the
/// cells.
fn cut_off(
    pours: &Pours,
    world: &World<'_>,
    design: &Design,
    jobs: &[Job],
    copper: &[Option<NetCopper>],
    pitch: i64,
) -> Vec<usize> {
    pours
        .jobs()
        .map(|k| {
            pours
                .rejoin(world, design, jobs, copper, k, pitch, None, &[])
                .map_or(0, |r| r.cut)
        })
        .collect()
}

/// Tries to join what is still cut off the fill of job `k`'s net, as the module says:
/// the search for its stubs left `left` terminals not joined. `may` says by job whether
/// its routes may be laid another way, `stubs` are the parameters of the search for the
/// stubs and `params` those for the nets laid again, `mender` lays a net again by a
/// chain where it has no way with nothing moved, and `stop` is asked between two tries.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn open(
    world: &mut World<'_>,
    design: &Design,
    jobs: &mut [Job],
    copper: &mut [Option<NetCopper>],
    pours: &Pours,
    real: Option<&Refilled>,
    (k, left): (usize, usize),
    may: &[bool],
    (stubs, params): (Params, Params),
    mender: &mut Mender<'_, '_>,
    stop: &dyn Fn() -> bool,
) -> Opened {
    let mut out = Opened::default();
    let pitch = params.pitch;
    // The search as the routes are, with the places near the pads that the net was not
    // given before: what it finds is kept, and how many terminals it leaves is what a
    // try has to better.
    let mut found = None;
    let mut left = left;
    for _ in 0..AGAIN {
        let Some(again) = pours.rejoin(world, design, jobs, copper, k, pitch, real, &[]) else {
            return out;
        };
        let Some(own) = copper[k].as_ref() else {
            return out;
        };
        let ctx = completion(design, &again.ctx, own, pitch);
        let with_sites = pours::with_sites(&jobs[k].ctx, &again.sites, pitch);
        let steps = StepCache::default();
        let (more, open_now) = match detail::route_net(world, &ctx, &steps, stubs, None, None) {
            Ok(tree) => {
                out.expanded += tree.expanded;
                (NetCopper::from_tree(&ctx, &tree), tree.missing.len())
            }
            Err((_, expanded)) => {
                out.expanded += expanded;
                (NetCopper::default(), ctx.terminals)
            }
        };
        // What joins nothing more is not laid: the nets round the pads are tried.
        if more.is_empty() || open_now >= left {
            if open_now != 0 {
                found = Some((again, ctx, with_sites, open_now));
            }
            break;
        }
        left = open_now;
        let mut c = copper[k].take().expect("the net has copper");
        c.unplace(world);
        c.segs.extend(more.segs);
        for v in more.vias {
            if !c.vias.contains(&v) {
                c.vias.push(v);
            }
        }
        jobs[k].ctx = with_sites;
        c.merge(&jobs[k].ctx);
        c.place(world, &jobs[k].ctx);
        copper[k] = Some(c);
        out.kept = true;
        if open_now == 0 {
            break;
        }
    }
    let Some((_, ctx, with_sites, open_now)) = found else {
        return out;
    };
    // What is cut off: the pads, and the ends of the tracks and the vias, of the pieces
    // that the search is to join to the first.
    let targets: Vec<Point> = ctx
        .pads
        .iter()
        .filter(|p| p.terminal != 0)
        .map(|p| p.center)
        .collect();
    if targets.is_empty() {
        return out;
    }
    // The nets near what is cut off, the nearest first.
    let near2 = (NEAR as f64) * (NEAR as f64);
    let mut near: Vec<(f64, usize)> = Vec::new();
    for (x, c) in copper.iter().enumerate() {
        let Some(c) = c else { continue };
        if x == k || jobs[x].plane || !may.get(x).copied().unwrap_or(false) {
            continue;
        }
        let mut best = f64::INFINITY;
        for &t in &targets {
            for s in &c.segs {
                best = best.min(distance2(t, s.a, s.b));
            }
            for &v in &c.vias {
                best = best.min(distance2(t, v, v));
            }
        }
        if best <= near2 {
            near.push((best, x));
        }
    }
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let near: Vec<usize> = near.into_iter().map(|(_, x)| x).collect();
    let mut sets: Vec<Vec<usize>> = near.iter().take(ALONE).map(|&x| vec![x]).collect();
    let paired = near.len().min(PAIRED);
    for i in 0..paired {
        for j in i + 1..paired {
            sets.push(vec![near[i], near[j]]);
        }
    }
    // Before those, the nets that the stub's cheapest way across the routes runs into, by
    // the price of a route crossed and of the places where nets have been in each other's
    // way: the nearest net is not the one with another way, and where the routes have
    // fought, the net taken out has none back.
    {
        let steps = StepCache::default();
        let rip = RipUp {
            penalty: Some(CROSSING),
            history: mender.settle.history,
        };
        match detail::route_net(world, &ctx, &steps, stubs, None, Some(rip)) {
            Ok(tree) => {
                out.expanded += tree.expanded;
                let (victims, _) = detail::victims(world, &ctx, &tree);
                let set: Option<Vec<usize>> = victims
                    .iter()
                    .map(|v| {
                        jobs.iter().position(|j| j.ctx.net == *v).filter(|&x| {
                            x != k
                                && !jobs[x].plane
                                && copper[x].is_some()
                                && may.get(x).copied().unwrap_or(false)
                        })
                    })
                    .collect();
                if let Some(mut set) = set.filter(|s| !s.is_empty() && s.len() <= CROSSED) {
                    set.sort_unstable();
                    set.dedup();
                    sets.retain(|other| {
                        let mut other = other.clone();
                        other.sort_unstable();
                        other != set
                    });
                    sets.insert(0, set);
                }
            }
            Err((_, expanded)) => out.expanded += expanded,
        }
    }
    sets.truncate(TRIES);
    if sets.is_empty() {
        return out;
    }
    let before = cut_off(pours, world, design, jobs, copper, pitch);
    let Some(at_k) = pours.jobs().position(|j| j == k) else {
        return out;
    };
    for set in sets {
        if stop() {
            break;
        }
        out.tried += 1;
        let mut old: Vec<(usize, NetCopper)> = Vec::new();
        for &x in &set {
            if let Some(mut c) = copper[x].take() {
                c.unplace(world);
                old.push((x, c));
            }
        }
        // The stub, with those nets away.
        let steps = StepCache::default();
        let more = match detail::route_net(world, &ctx, &steps, stubs, None, None) {
            Ok(tree) => {
                out.expanded += tree.expanded;
                (tree.missing.len() < open_now)
                    .then(|| NetCopper::from_tree(&ctx, &tree))
                    .filter(|m| !m.is_empty())
            }
            Err((_, expanded)) => {
                out.expanded += expanded;
                None
            }
        };
        let mut old_k: Option<NetCopper> = None;
        let mut whole = false;
        // What the chains of this try moved, to undo.
        let mut mended: Vec<Vec<Before>> = Vec::new();
        if let Some(more) = more {
            let mut was = copper[k].take().expect("the net has copper");
            was.unplace(world);
            let mut new = was.detached();
            old_k = Some(was);
            new.segs.extend(more.segs);
            for v in more.vias {
                if !new.vias.contains(&v) {
                    new.vias.push(v);
                }
            }
            new.merge(&with_sites);
            new.place(world, &with_sites);
            copper[k] = Some(new);
            // Each net again, the shortest first, round the stub and round those laid
            // before it.
            old.sort_by(|a, b| a.1.length().total_cmp(&b.1.length()).then(a.0.cmp(&b.0)));
            whole = true;
            for (x, _) in &old {
                let job = &jobs[*x];
                let p = Params {
                    max_expansions: params
                        .max_expansions
                        .min(cleanup::node_budget(job.ctx.terminals as u64 - 1)),
                    ..params
                };
                let steps = StepCache::default();
                let again = match detail::route_net(world, &job.ctx, &steps, p, None, None) {
                    Ok(tree) => {
                        out.expanded += tree.expanded;
                        tree.missing
                            .is_empty()
                            .then(|| NetCopper::from_tree(&job.ctx, &tree))
                    }
                    Err((_, expanded)) => {
                        out.expanded += expanded;
                        None
                    }
                };
                if let Some(mut new) = again {
                    new.place(world, &job.ctx);
                    copper[*x] = Some(new);
                    continue;
                }
                // With nothing moved it has no way: by a chain then, which moves the
                // nets in its way and joins them up again, and not the stub's.
                let mut routes = Routes {
                    copper: &mut *copper,
                    missing: &mut *mender.missing,
                    apart: &mut *mender.apart,
                };
                let walls = [jobs[k].ctx.net];
                let spent = mender.settle.expanded;
                // The nets of the set that are still out count as open until their turn.
                let out_still: usize = old
                    .iter()
                    .filter(|(y, _)| y != x && routes.copper[*y].is_none())
                    .map(|(y, _)| jobs[*y].ctx.terminals - 1)
                    .sum();
                let journal = mender.settle.mend(
                    world,
                    jobs,
                    &mut routes,
                    *x,
                    &walls,
                    mender.open + out_still,
                );
                out.expanded += mender.settle.expanded - spent;
                match journal {
                    Some(journal) => mended.push(journal),
                    None => {
                        whole = false;
                        break;
                    }
                }
            }
        }
        if whole {
            for &x in &set {
                if let Some(c) = copper[x].as_mut() {
                    cleanup::tidy(world, &jobs[x].ctx, c);
                }
            }
            // No net with more cut off its fill by the cells, and this one with less
            // where they told of it.
            let after = cut_off(pours, world, design, jobs, copper, pitch);
            whole = after.iter().zip(&before).all(|(a, b)| a <= b)
                && (before[at_k] == 0 || after[at_k] < before[at_k]);
        }
        if whole {
            jobs[k].ctx = with_sites;
            out.kept = true;
            return out;
        }
        // As it was.
        for journal in mended.into_iter().rev() {
            let mut routes = Routes {
                copper: &mut *copper,
                missing: &mut *mender.missing,
                apart: &mut *mender.apart,
            };
            Settle::undo(world, jobs, &mut routes, journal);
        }
        for &x in &set {
            if let Some(mut c) = copper[x].take() {
                c.unplace(world);
            }
        }
        if let Some(mut was) = old_k {
            if let Some(mut c) = copper[k].take() {
                c.unplace(world);
            }
            was.place(world, &jobs[k].ctx);
            copper[k] = Some(was);
        }
        for (x, mut c) in old {
            c.place(world, &jobs[x].ctx);
            copper[x] = Some(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::detail::{Corridor, History, Params};
    use super::super::geometry::{NetCopper, Seg};
    use super::super::pours::Pours;
    use super::super::settle::Settle;
    use super::super::tests::{MM, design, smd};
    use super::super::world::World;
    use super::super::zone_tests::zone;
    use super::super::{RouteOptions, fill_zones, planes, router};
    use super::Mender;
    use crate::geom::{Point, Rect};
    use crate::id::NetId;

    /// A strip of fill of net 1 on the top layer, 20 mm by 4, with pads of the net at 3,
    /// 14.5 and 19 mm, and net 2 from below the strip to above it, by a track that
    /// crosses it at 18 mm: the pad at 19 mm is cut off, and net 1 has no via to go
    /// under that track by. What [`open`](super::open) makes of it, with the copper of
    /// net 2 after it and whether the cells tell the pad cut off still.
    fn shut(may: bool) -> (super::Opened, NetCopper, bool) {
        let pads = vec![
            smd(3 * MM, 2 * MM, 1),
            smd(14_500_000, 2 * MM, 1),
            smd(19 * MM, 2 * MM, 1),
            smd(5 * MM, 6_500_000, 1),
            smd(19_200_000, -3 * MM, 2),
            smd(19_200_000, 7 * MM, 2),
        ];
        let mut d = design(2, pads, 3);
        let mut z = zone(1, 0, 4 * MM);
        z.outline[1].x = 20 * MM;
        z.outline[2].x = 20 * MM;
        z.plane = Some(true);
        d.zones.zones.push(z);
        d.nets[1].via = None;
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
        let mut jobs = router::jobs(&grouped, &world, pitch, &pieces, &[]);
        let by_net: BTreeMap<NetId, usize> = jobs
            .iter()
            .enumerate()
            .map(|(i, j)| (j.ctx.net, i))
            .collect();
        let (ground, other) = (by_net[&NetId(1)], by_net[&NetId(2)]);
        // The board ends just past the pads of net 2: no way round them.
        let bounds = Rect::new(Point::new(0, -3_500_000), Point::new(26 * MM, 7_500_000));
        let pours = Pours::plan(&d, &world, &fills, &jobs, bounds, pitch);
        let seg = |a: (i64, i64), b: (i64, i64)| Seg {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            l: 0,
        };
        let mut copper: Vec<Option<NetCopper>> = vec![None; jobs.len()];
        let mut across = NetCopper::default();
        across.segs = vec![
            seg((19_200_000, -3 * MM), (18 * MM, -1_800_000)),
            seg((18 * MM, -1_800_000), (18 * MM, 5_800_000)),
            seg((18 * MM, 5_800_000), (19_200_000, 7 * MM)),
        ];
        // The pad off the strip joined to it by a track, as routing leaves it.
        let mut stub = NetCopper::default();
        stub.segs = vec![seg((5 * MM, 6_500_000), (5 * MM, 3 * MM))];
        copper[other] = Some(across);
        copper[ground] = Some(stub);
        for (job, c) in jobs.iter().zip(copper.iter_mut()) {
            if let Some(c) = c {
                c.place(&mut world, &job.ctx);
            }
        }
        let cut = |world: &World<'_>, jobs: &[router::Job], copper: &[Option<NetCopper>]| {
            pours
                .rejoin(world, &d, jobs, copper, ground, pitch, None, &[])
                .is_some()
        };
        assert!(cut(&world, &jobs, &copper));
        let params = Params {
            pitch,
            via_cost: options.via_cost,
            layer_bias: options.layer_bias,
            bounds,
            max_expansions: options.max_expansions,
            under_pads: false,
            partial: false,
            stubs: false,
            forest: false,
        };
        let stubs = Params {
            partial: true,
            stubs: true,
            forest: true,
            ..params
        };
        let history = History {
            grid: Corridor {
                origin: bounds.min(),
                tile: 32 * MM,
                cols: 1,
                rows: 1,
                cells: vec![0],
            },
            cost: vec![0.0],
        };
        let mut spent = vec![0u8; jobs.len()];
        let mut missing = vec![Vec::new(); jobs.len()];
        let mut apart = vec![0; jobs.len()];
        let mut mender = Mender {
            settle: Settle {
                design: &d,
                by_net: &by_net,
                plane: &|n| n == NetId(1),
                params,
                pitch,
                history: &history,
                fixed: &[],
                pours: None,
                expanded: 0,
                blocked: None,
                tried: 0,
                kept: 0,
                made: 0,
                chains: true,
                negotiate: false,
                negotiated: (0, 0, 0),
                places: false,
                spent: &mut spent,
                skip: Vec::new(),
            },
            missing: &mut missing,
            apart: &mut apart,
            open: 0,
        };
        let may = vec![may; jobs.len()];
        let done = super::open(
            &mut world,
            &d,
            &mut jobs,
            &mut copper,
            &pours,
            None,
            (ground, 1),
            &may,
            (stubs, params),
            &mut mender,
            &|| false,
        );
        let theirs = copper[other].clone().unwrap();
        assert_eq!(theirs.pieces(&jobs[other].ctx).apart(), 0);
        assert!(theirs.all_free(&world, &jobs[other].ctx));
        let still = cut(&world, &jobs, &copper);
        (done, theirs, still)
    }

    #[test]
    fn the_track_that_shuts_a_pad_in_goes_round_the_stub_that_joins_the_pad_to_its_fill() {
        let (done, other, cut) = shut(true);
        assert!(done.kept && done.tried == 1, "{done:?}");
        assert!(!cut);
        // Net 2 is whole on its layer, with no via, and no longer between the pad and
        // the rest of the strip where it crosses the strip's middle.
        assert!(other.vias.is_empty(), "{other:?}");
        let middle = 2 * MM;
        for s in &other.segs {
            if (s.a.y - middle) * (s.b.y - middle) <= 0 && s.a.y != s.b.y {
                let x = s.a.x + (s.b.x - s.a.x) * (middle - s.a.y) / (s.b.y - s.a.y);
                assert!(x > 19_300_000, "{other:?}");
            }
        }
    }

    #[test]
    fn a_net_that_may_not_be_laid_another_way_stays_and_the_pad_is_as_it_was() {
        let (done, other, cut) = shut(false);
        assert!(!done.kept && done.tried == 0, "{done:?}");
        assert!(cut);
        assert_eq!(other.segs.len(), 3);
    }
}
