//! A pour left whole: the route that cuts a pad off its fill laid another way, for the via
//! that joined the pad again.
//!
//! A pad of a net with a pour on its own layer needs no copper but the pour. Routes of
//! other nets go through that pour, and one that closes a ring of tracks and pads around
//! the pad leaves it on a piece of its own, which a stub then joins to the rest: a via to
//! the pour on the other side, for a pad that had none ([`super::pours`]). The route that
//! closed the ring was the cheapest for its net when it was made, and knew nothing of the
//! via it cost another.
//!
//! Once the routes are where they stay, each such stub is looked at: the pad it joins,
//! the nets whose routes are around the pad's piece of fill ([`Pours::cutters`]), and for
//! each of those in turn, where the fill would join the pad to its largest piece if that
//! net's routes were not there ([`Pours::bridge`]). That way is kept free, as copper of
//! the pour's net as wide as the fill needs there, and the other net is routed again
//! round it. If it then has no more vias than it had and is not much longer, and no pad
//! of any pour is cut off that was not, the stub goes and the new route stays; if not,
//! everything is as it was.

use super::cleanup::{self, VIA_COST_FACTOR};
use super::detail::{self, NetCtx, Params, StepCache, TermPad};
use super::geometry::NetCopper;
use super::pours::Pours;
use super::router::Job;
use super::world::World;
use super::{Design, RouteOptions};
use crate::geom::Shape;

/// How many nets around a pad's piece of fill are tried for a pad, and how many stubs of
/// a run are looked at.
const CUTTERS: usize = 6;
const STUBS: usize = 48;

/// How many nets at most are laid another way together for one pad, how many sets of
/// nets are asked of the cells for a pad, and how many of them are tried.
const TOGETHER: usize = 3;
const SETS: usize = 40;
const TRIES: usize = 6;

/// The share of what a run's deadline leaves that the step may take, and the most.
const SHARE: f64 = 0.1;
const MOST: std::time::Duration = std::time::Duration::from_millis(1500);

/// What the step did: the stubs looked at, those taken out, the vias that the board has
/// fewer, and the nodes that its searches expanded.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Uncut {
    pub(crate) tried: usize,
    pub(crate) kept: usize,
    pub(crate) vias: usize,
    pub(crate) expanded: u64,
}

/// The copper `c` without its part `part` of `parts`.
fn without(c: &NetCopper, parts: &super::geometry::Pieces, part: usize) -> NetCopper {
    let mut out = c.detached();
    let mut k = 0;
    out.segs.retain(|_| {
        k += 1;
        parts.segs[k - 1] != part
    });
    let mut k = 0;
    out.vias.retain(|_| {
        k += 1;
        parts.vias[k - 1] != part
    });
    out
}

/// How many pads the routes cut off the fills of each net with a planned fill, by the
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

/// Looks at each stub with a via of each net planned on a fill, as the module says.
/// `may` says by job whether its routes may be laid another way.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn run(
    world: &mut World<'_>,
    design: &Design,
    jobs: &[Job],
    copper: &mut [Option<NetCopper>],
    pours: &Pours,
    params: Params,
    options: &RouteOptions,
    may: &[bool],
) -> Uncut {
    let mut out = Uncut::default();
    // In a run with a deadline, no longer than a share of what the deadline leaves: the
    // cells of a large board are a million to go through for each set of nets asked of
    // them.
    let until = world
        .run()
        .and_then(super::control::Run::left)
        .map(|left| std::time::Instant::now() + left.mul_f64(SHARE).min(MOST));
    let stopped = |world: &World<'_>| {
        world.run().is_some_and(super::control::Run::poll)
            || until.is_some_and(|t| std::time::Instant::now() >= t)
    };
    let pitch = params.pitch;
    // What the cells tell as cut off before anything is tried: no try is kept that
    // leaves more.
    let mut before = cut_off(pours, world, design, jobs, copper, pitch);
    for (at, k) in pours.jobs().collect::<Vec<usize>>().into_iter().enumerate() {
        if before[at] != 0 {
            continue;
        }
        let ctx = &jobs[k].ctx;
        let bare = NetCtx::new(ctx.net, ctx.widths.clone(), ctx.via, Vec::new(), 0, 0);
        let mut part = 0;
        while let Some(c) = copper[k].as_ref() {
            let parts = c.pieces(&bare);
            if part >= parts.count || out.tried >= STUBS || stopped(world) {
                break;
            }
            let this = part;
            part += 1;
            let vias = parts.vias.iter().filter(|&&p| p == this).count();
            if vias == 0 {
                continue;
            }
            // The net without this stub, and the pads that this leaves cut off: one
            // piece of fill with pads of its own, or it is no stub for a pour that was
            // cut.
            let less = without(c, &parts, this);
            let stub_length = c.length() - less.length();
            let old_n = copper[k].replace(less);
            let cut = pours.rejoin(world, design, jobs, copper, k, pitch, None, &[]);
            let pads: Vec<TermPad> = cut
                .as_ref()
                .filter(|r| r.cut == 1)
                .map(|r| {
                    let own =
                        |p: &&TermPad| p.terminal >= ctx.terminals && p.bbox.min() != p.bbox.max();
                    r.ctx.pads.iter().filter(own).cloned().collect()
                })
                .unwrap_or_default();
            let Some(pad) = pads.first().filter(|_| pads.len() == 1) else {
                copper[k] = old_n;
                continue;
            };
            out.tried += 1;
            // The nets to lay another way, one, two or three of them together: those
            // around the pad's piece, and where the fill would still not join the pad
            // without one of them, those around the piece that it would then be on.
            let allowed = |x: usize| may.get(x).copied().unwrap_or(false) && !jobs[x].plane;
            let around = |skip: &[usize], most: usize| -> Vec<usize> {
                let all = pours.cutters(world, design, jobs, copper, k, pad, skip);
                all.into_iter().filter(|&x| allowed(x)).take(most).collect()
            };
            type Way = (crate::geom::LayerId, Vec<crate::geom::Point>, i64);
            let mut sets: Vec<(Vec<usize>, Way)> = Vec::new();
            let mut stack: Vec<Vec<usize>> =
                around(&[], CUTTERS).into_iter().map(|x| vec![x]).collect();
            stack.reverse();
            let mut looked = 0;
            while let Some(set) = stack.pop() {
                if looked >= SETS || stopped(world) {
                    break;
                }
                looked += 1;
                if let Some(way) = pours.bridge(world, design, jobs, copper, k, pad, &set) {
                    let mut sorted = set.clone();
                    sorted.sort_unstable();
                    if !sets.iter().any(|(s, _)| *s == sorted) {
                        sets.push((sorted, way));
                    }
                } else if set.len() < TOGETHER {
                    let mut more = around(&set, CUTTERS / 2);
                    more.reverse();
                    for x in more {
                        let mut next = set.clone();
                        next.push(x);
                        stack.push(next);
                    }
                }
            }
            sets.sort_by_key(|(s, way)| (s.len(), way.1.len()));
            sets.truncate(TRIES);
            // The world as the net is without the stub, for the searches of the tries.
            let mut old_n = old_n;
            if let Some(o) = old_n.as_mut() {
                o.unplace(world);
            }
            if let Some(n) = copper[k].as_mut() {
                n.place(world, ctx);
            }
            let mut done = false;
            for (set, (layer, path, reach)) in sets {
                if stopped(world) {
                    break;
                }
                let mut old: Vec<(usize, NetCopper)> = Vec::new();
                for &x in &set {
                    if let Some(mut c) = copper[x].take() {
                        c.unplace(world);
                        old.push((x, c));
                    }
                }
                // The way of the fill, as copper of its net for as long as the searches are.
                let guards: Vec<_> = path
                    .windows(2)
                    .filter_map(|w| Shape::track(w[0], w[1], 2 * reach).ok())
                    .map(|shape| world.add_track(ctx.net, layer, shape))
                    .collect();
                // Each net again, the shortest first, round the way and round those laid
                // before it.
                old.sort_by(|a, b| a.1.length().total_cmp(&b.1.length()).then(a.0.cmp(&b.0)));
                let mut whole = old.len() == set.len();
                for (x, _) in &old {
                    if !whole {
                        break;
                    }
                    let job = &jobs[*x];
                    let p = Params {
                        via_cost: options.via_cost * VIA_COST_FACTOR,
                        max_expansions: params
                            .max_expansions
                            .min(cleanup::node_budget(job.ctx.terminals as u64 - 1)),
                        ..params
                    };
                    let steps = StepCache::default();
                    match detail::route_net(world, &job.ctx, &steps, p, None, None) {
                        Ok(tree) if tree.missing.is_empty() => {
                            out.expanded += tree.expanded;
                            let mut new = NetCopper::from_tree(&job.ctx, &tree);
                            new.place(world, &job.ctx);
                            copper[*x] = Some(new);
                        }
                        Ok(tree) => {
                            out.expanded += tree.expanded;
                            whole = false;
                        }
                        Err((_, expanded)) => {
                            out.expanded += expanded;
                            whole = false;
                        }
                    }
                }
                // No more vias than they had, and no longer by more than the stub and half
                // the price of the via that goes, in millimetres, as the costs are.
                let was: (usize, f64) = old.iter().fold((0, 0.0), |(v, l), (_, c)| {
                    (v + c.vias.len(), l + c.length())
                });
                let limit = was.1 + stub_length + 1e6 * options.via_cost * VIA_COST_FACTOR / 2.0;
                let mut kept = false;
                if !whole {
                    for g in &guards {
                        world.remove(g);
                    }
                }
                if whole {
                    let mut now = (0, 0.0);
                    // Pulled tight while the way of the fill is still kept: the straight
                    // line across it is what they had.
                    for &x in &set {
                        if let Some(c) = copper[x].as_mut() {
                            cleanup::tidy(world, &jobs[x].ctx, c);
                            now = (now.0 + c.vias.len(), now.1 + c.length());
                        }
                    }
                    for g in &guards {
                        world.remove(g);
                    }
                    if now.0 <= was.0 && now.1 <= limit {
                        let after = cut_off(pours, world, design, jobs, copper, pitch);
                        kept = after.iter().zip(&before).all(|(a, b)| a <= b);
                        if kept {
                            before = after;
                        }
                    }
                }
                if kept {
                    done = true;
                    break;
                }
                for &x in &set {
                    if let Some(mut c) = copper[x].take() {
                        c.unplace(world);
                    }
                }
                for (x, mut c) in old {
                    c.place(world, &jobs[x].ctx);
                    copper[x] = Some(c);
                }
            }
            if done {
                out.kept += 1;
                out.vias += vias;
                // The parts are numbered again without the one that went.
                part = 0;
            } else {
                if let Some(mut n) = copper[k].take() {
                    n.unplace(world);
                }
                if let Some(o) = old_n.as_mut() {
                    o.place(world, ctx);
                }
                copper[k] = old_n;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::detail::Params;
    use super::super::geometry::{NetCopper, Seg};
    use super::super::pours::Pours;
    use super::super::tests::{MM, design, smd};
    use super::super::world::World;
    use super::super::zone_tests::zone;
    use super::super::{Design, RouteOptions, fill_zones, planes, router};
    use crate::geom::{Point, Rect};
    use crate::id::NetId;

    /// A strip of fill of net 1 on the top layer, 20 mm by 4, with pads of the net at 3,
    /// 14.5 and 19 mm and one off it, above, and two pads of net 2 below and above it at
    /// `x`.
    fn strip(x: i64) -> Design {
        let pads = vec![
            smd(3 * MM, 2 * MM, 1),
            smd(14_500_000, 2 * MM, 1),
            smd(19 * MM, 2 * MM, 1),
            smd(5 * MM, 9 * MM, 1),
            smd(x, -3 * MM, 2),
            smd(x, 7 * MM, 2),
        ];
        let mut d = design(2, pads, 3);
        let mut z = zone(1, 0, 4 * MM);
        z.outline[1].x = 20 * MM;
        z.outline[2].x = 20 * MM;
        z.plane = Some(true);
        d.zones.zones.push(z);
        d
    }

    /// The strip with a track of net 2 straight across it at `x`, and the pad at 19 mm
    /// joined to the rest of its fill again under that track, through two vias: what
    /// [`run`](super::run) makes of it on a board that goes as far as `right`, with the
    /// copper of net 1 and of net 2 after it.
    fn uncut(x: i64, right: i64) -> (super::Uncut, NetCopper, NetCopper) {
        let d = strip(x);
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
        let bounds = Rect::new(Point::new(0, -5 * MM), Point::new(right, 12 * MM));
        let pours = Pours::plan(&d, &world, &fills, &jobs, bounds, pitch);
        let seg = |a: (i64, i64), b: (i64, i64), l: u8| Seg {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            l,
        };
        let mut copper: Vec<Option<NetCopper>> = vec![None; jobs.len()];
        let mut across = NetCopper::default();
        across.segs = vec![seg((x, -3 * MM), (x, 7 * MM), 0)];
        let mut hop = NetCopper::default();
        hop.segs = vec![seg((19 * MM, 2 * MM), (14_500_000, 2 * MM), 1)];
        hop.vias = vec![Point::new(19 * MM, 2 * MM), Point::new(14_500_000, 2 * MM)];
        copper[other] = Some(across);
        copper[ground] = Some(hop);
        for (job, c) in jobs.iter().zip(copper.iter_mut()) {
            if let Some(c) = c {
                c.place(&mut world, &job.ctx);
            }
        }
        // With the hop, nothing is cut off.
        assert!(
            pours
                .rejoin(&world, &d, &jobs, &copper, ground, pitch, None, &[])
                .is_none()
        );
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
        let may = vec![true; jobs.len()];
        let done = super::run(
            &mut world,
            &d,
            &jobs,
            &mut copper,
            &pours,
            params,
            &options,
            &may,
        );
        // Whatever it did, nothing is cut off after it and net 2 is whole.
        assert!(
            pours
                .rejoin(&world, &d, &jobs, &copper, ground, pitch, None, &[])
                .is_none()
        );
        let theirs = copper[other].clone().unwrap();
        assert_eq!(theirs.pieces(&jobs[other].ctx).apart(), 0);
        (done, copper[ground].clone().unwrap(), theirs)
    }

    #[test]
    fn the_track_that_cuts_a_pad_off_its_pour_goes_the_other_side_of_the_pad_and_the_vias_go() {
        // The track crosses the strip between the last pad and the one before it: the
        // other side of the last pad is a millimetre further, for two vias and the track
        // under it.
        let (done, ground, other) = uncut(18 * MM, 26 * MM);
        assert_eq!((done.tried, done.kept, done.vias), (1, 1, 2), "{done:?}");
        assert!(
            ground.vias.is_empty() && ground.segs.is_empty(),
            "{ground:?}"
        );
        assert!(other.vias.is_empty(), "{other:?}");
        assert!(other.segs.iter().all(|s| s.l == 0));
        // Where it crosses the middle of the strip, the pad is between it and the rest.
        let middle = 2 * MM;
        let mut crossed = 0;
        for s in &other.segs {
            if (s.a.y - middle) * (s.b.y - middle) <= 0 && s.a.y != s.b.y {
                crossed += 1;
                let x = s.a.x + (s.b.x - s.a.x) * (middle - s.a.y) / (s.b.y - s.a.y);
                assert!(x > 19_300_000, "{other:?}");
            }
        }
        assert!(crossed >= 1, "{other:?}");
        assert!(other.length() < 10e6 + 6e6, "{}", other.length());
    }

    #[test]
    fn a_track_with_no_other_way_on_its_layer_stays_and_so_do_the_vias() {
        // The board ends at the last pad: the only other way is under the strip.
        let (done, ground, other) = uncut(18 * MM, 19_500_000);
        assert_eq!((done.tried, done.kept), (1, 0), "{done:?}");
        assert_eq!(ground.vias.len(), 2);
        assert_eq!(other.segs.len(), 1);
    }
}
