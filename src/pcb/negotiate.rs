//! Negotiation: a connection that rip-up left open is routed across the routes in its
//! way, and the nets it runs into are routed again in rounds, each across the others at a
//! price that rises, until none is in another's way.
//!
//! A [chain](super::settle) moves one net at a time, and each move is final for the nets
//! moved before it: a pad of a row of fine pitch with the tracks and vias of four nets
//! round it needs all four to give way a little, and a chain of five nets is undone seven
//! times in eight. Here the routes of a round may be in each other's way. A step within a
//! clearance of another net's route costs its length again times a factor, the present
//! price, so that the first routes are near the shortest; the nets whose routes are then
//! in another's way are each routed again, across what is there or around it, whichever
//! is cheaper; the price rises from round to round, and where routes were in each other's
//! way every track costs more from then on, the history, so that a net with another way
//! takes it and leaves the place to the one with none. That is negotiated congestion
//! (McMurchie and Ebeling's PathFinder) on exact geometry: what is shared is not a
//! resource with a capacity, but room within a clearance, as the world measures it.
//!
//! A negotiation starts from routes that are in nobody's way, with one net to complete,
//! and only the nets that a route of it runs into take part, each near where it is in
//! another's way: what it has within a few pitches of there goes, the rest stays, and it
//! is joined up again within a window round the place. If the rounds end with routes
//! still in each other's way, each net gives up what it has in another's, the net that
//! the negotiation is for last, and the nets left in pieces are joined up again with
//! nothing shared. It is kept if it ends with no route in another's way and fewer
//! connections open, by the same count as a chain, and is undone to the track if not.

use super::detail::{self, Fine, NetCtx, NetRun, Params, Share, StepCache};
use super::geometry::NetCopper;
use super::router::{BRANCH_TERMINALS, completion};
use super::settle::{Before, Chain, Routes, Settle};
use super::world::{Verdict, World};
use crate::geom::{LayerId, Point, Rect, Shape};
use crate::id::NetId;

/// How many rounds a negotiation has.
const ROUNDS: usize = 24;

/// The price of room shared in the first round, as a factor of the length shared, and
/// what it is multiplied by from one round to the next.
const PRESENT: f64 = 0.5;
const GROWTH: f64 = 1.6;
const MOST: f64 = 1000.0;

/// What a track costs more for its length, for each step of a route that was in
/// another's way there at the end of a round.
const HISTORY: f32 = 0.25;

/// How many nets take part at most.
const MEMBERS: usize = 32;

/// How many nodes a search of a negotiation may expand, and all the searches of one.
const NODES: u64 = 60_000;
const ALL_NODES: u64 = 3_000_000;

/// How many pitches round the places where a net's copper is in another's way it loses
/// what it has, to be routed there from nothing, and how many it is joined up again
/// within. Joined up again from the ends that a route across it leaves, round after
/// round, a net keeps every bend that it ever made; and at a high price for room shared,
/// a search goes everywhere it can before it shares any.
const CUT: i64 = 12;
const WINDOW: i64 = 24;

/// How many nodes a search may expand once the rounds are over, when nothing is shared
/// any more.
const LEGAL_NODES: u64 = 200_000;

/// The nets whose routes the copper of a net, which is in the world, is within a
/// clearance of, and where: a point and a layer for each step of a pitch of its tracks,
/// and each of its vias, that is.
fn overlaps(
    world: &World<'_>,
    ctx: &NetCtx,
    copper: &NetCopper,
    pitch: i64,
) -> (Vec<NetId>, Vec<(Point, u8)>) {
    let mut nets = Vec::new();
    let mut at = Vec::new();
    let hit = |a: Point, b: Point, l: u8, nets: &mut Vec<NetId>| {
        Shape::track(a, b, ctx.widths[usize::from(l)])
            .is_ok_and(|s| world.track(ctx.net, LayerId(l), &s, Some(nets)) != Verdict::Free)
    };
    for s in &copper.segs {
        if !hit(s.a, s.b, s.l, &mut nets) {
            continue;
        }
        let (dx, dy) = (s.b.x - s.a.x, s.b.y - s.a.y);
        let n = dx.abs().max(dy.abs()) / pitch.max(1);
        if n < 2 || (dx != 0 && dy != 0 && dx.abs() != dy.abs()) {
            at.push((s.b, s.l));
            continue;
        }
        let (ux, uy) = (dx.signum() * pitch, dy.signum() * pitch);
        let point = |k: i64| {
            if k == n {
                s.b
            } else {
                Point::new(s.a.x + k * ux, s.a.y + k * uy)
            }
        };
        let mut scratch = Vec::new();
        for k in 0..n {
            if hit(point(k), point(k + 1), s.l, &mut scratch) {
                at.push((point(k + 1), s.l));
            }
        }
    }
    if let Some((d, drill)) = ctx.via {
        for &v in &copper.vias {
            // A via in the world is within the distance between holes of itself.
            let mut theirs = Vec::new();
            let verdict = world.via(ctx.net, v, d, drill, Some(&mut theirs));
            theirs.retain(|&n| n != ctx.net);
            if verdict == Verdict::Blocked || !theirs.is_empty() {
                for l in 0..ctx.widths.len() as u8 {
                    at.push((v, l));
                }
                for n in theirs {
                    if !nets.contains(&n) {
                        nets.push(n);
                    }
                }
            }
        }
    }
    (nets, at)
}

impl Settle<'_> {
    /// One negotiation for job `j`. Nothing is undone: what it changed is in the journal
    /// of what it returns, which is whole if it ended with no route in another's way and
    /// fewer connections open.
    pub(super) fn negotiation(
        &mut self,
        world: &mut World<'_>,
        nets: &[&NetCtx],
        routes: &mut Routes<'_>,
        j: usize,
    ) -> Chain {
        let open_before = routes.open_all(nets);
        let mut journal: Vec<Before> = Vec::new();
        let mut fine = Fine::new(2 * self.pitch);
        let mut present = PRESENT;
        // The nets that take part, in the order they came in, and those with a turn in
        // this round.
        let mut members: Vec<usize> = vec![j];
        let mut turn: Vec<usize> = vec![j];
        let began = self.expanded;
        let mut settled = false;
        'rounds: for round in 0..ROUNDS {
            for &k in &turn {
                if world.run().is_some_and(|r| r.poll()) {
                    break 'rounds;
                }
                // A net that the others of this round have moved away from keeps its
                // routes.
                if round > 0
                    && routes.open(nets, k) == 0
                    && routes.copper[k]
                        .as_ref()
                        .is_some_and(|c| overlaps(world, nets[k], c, self.pitch).0.is_empty())
                {
                    continue;
                }
                let share = Share {
                    present,
                    fine: &fine,
                };
                self.renew(world, nets, routes, k, Some(share), &mut journal);
            }
            // The routes in another's way, and whose.
            let mut hot: Vec<usize> = Vec::new();
            let mut foreign = false;
            for m in 0..members.len() {
                let k = members[m];
                let Some(c) = routes.copper[k].as_ref() else {
                    continue;
                };
                let (victims, at) = overlaps(world, nets[k], c, self.pitch);
                if victims.is_empty() && at.is_empty() {
                    continue;
                }
                hot.push(k);
                for v in victims {
                    match self.by_net.get(&v) {
                        Some(&q) => {
                            hot.push(q);
                            if !members.contains(&q) {
                                members.push(q);
                            }
                        }
                        None => foreign = true,
                    }
                }
                for (p, l) in at {
                    fine.add(p, l, HISTORY);
                }
            }
            if hot.is_empty() {
                // A net that a search left in pieces has one more round, with nothing
                // in its way but what it found no way round.
                let open: Vec<usize> = members
                    .iter()
                    .copied()
                    .filter(|&k| routes.open(nets, k) > 0)
                    .collect();
                if open.is_empty() || open == turn {
                    settled = true;
                    break;
                }
                turn = open;
                continue;
            }
            if foreign || members.len() > MEMBERS || self.expanded - began > ALL_NODES {
                break;
            }
            hot.sort_unstable();
            hot.dedup();
            // A net with connections open has its turn too: a round may have left it room.
            for &k in &members {
                if routes.open(nets, k) > 0 && !hot.contains(&k) {
                    hot.push(k);
                }
            }
            turn = hot;
            present = (present * GROWTH).min(MOST);
        }
        // Rounds that end with routes still in each other's way leave few of them: each
        // net gives up what it has in another's way, the net that the negotiation is for
        // last, so that it keeps its own, and those then left in pieces are joined up
        // again with nothing shared, that net first.
        let stopped = world.run().is_some_and(|r| r.poll());
        if !settled && !stopped {
            let mut order: Vec<usize> = members.iter().copied().filter(|&k| k != j).collect();
            order.sort_unstable_by(|a, b| b.cmp(a));
            order.push(j);
            for &k in &order {
                let Some(c) = routes.copper[k].as_ref() else {
                    continue;
                };
                if overlaps(world, nets[k], c, self.pitch).1.is_empty() {
                    continue;
                }
                Self::note(routes, &mut journal, k);
                let Some(mut old) = routes.copper[k].take() else {
                    continue;
                };
                old.unplace(world);
                let whole = nets[k];
                let mut kept = old.free_in(world, whole, self.pitch);
                if !(self.plane)(whole.net) && whole.terminals < BRANCH_TERMINALS {
                    kept.drop_floating(whole);
                } else {
                    kept.prune(whole);
                }
                if kept.is_empty() {
                    routes.missing[k].clear();
                    routes.apart[k] = 0;
                    continue;
                }
                let pieces = kept.pieces(whole);
                routes.missing[k] = pieces.missing();
                routes.apart[k] = pieces.apart();
                kept.place(world, whole);
                routes.copper[k] = Some(kept);
            }
            order.reverse();
            for &k in &order {
                if world.run().is_some_and(|r| r.poll()) {
                    break;
                }
                if routes.open(nets, k) > 0 {
                    self.renew(world, nets, routes, k, None, &mut journal);
                }
            }
            settled = members.iter().all(|&k| {
                routes.copper[k]
                    .as_ref()
                    .is_none_or(|c| overlaps(world, nets[k], c, self.pitch).1.is_empty())
            });
        }
        Chain {
            whole: settled && routes.open_all(nets) < open_before,
            journal,
            culprit: None,
        }
    }

    /// Routes job `k` again, across other nets' routes as `share` prices them, or across
    /// none without it: from what of its copper is in nobody's way, less what it has
    /// within [`CUT`] pitches of where it is in another's, and within [`WINDOW`] pitches
    /// of there. A net with nothing in another's way is completed from what it has, over
    /// the whole board.
    fn renew(
        &mut self,
        world: &mut World<'_>,
        nets: &[&NetCtx],
        routes: &mut Routes<'_>,
        k: usize,
        share: Option<Share<'_>>,
        journal: &mut Vec<Before>,
    ) {
        let whole: &NetCtx = nets[k];
        let plane = (self.plane)(whole.net);
        let many = whole.terminals > detail::BUDGET_TERMINALS;
        let small = !plane && whole.terminals < BRANCH_TERMINALS;
        Self::note(routes, journal, k);
        // Where its copper is in another's way: it is joined up again near there.
        let hits = routes.copper[k]
            .as_ref()
            .filter(|_| share.is_some())
            .map(|c| overlaps(world, whole, c, self.pitch).1)
            .and_then(|at| {
                at.iter()
                    .map(|&(p, _)| Rect::new(p, p))
                    .reduce(|a, b| a.union(&b))
            });
        let near = hits.map(|r| {
            let b = self.params.bounds;
            let r = r.expand(WINDOW * self.pitch);
            Rect::new(
                Point::new(r.min().x.max(b.min().x), r.min().y.max(b.min().y)),
                Point::new(r.max().x.min(b.max().x), r.max().y.min(b.max().y)),
            )
        });
        let kept = routes.copper[k].take().and_then(|mut old| {
            old.unplace(world);
            // What it has near where it is in another's way goes, and what is in nobody's
            // way further off stays.
            let mut kept = match hits {
                Some(r) => old
                    .outside(&r.expand(CUT * self.pitch), whole, self.pitch)
                    .free_in(world, whole, self.pitch),
                None => old.free_in(world, whole, self.pitch),
            };
            if small {
                kept.drop_floating(whole);
            } else {
                kept.prune(whole);
            }
            (!kept.is_empty()).then_some(kept)
        });
        let rest = kept
            .as_ref()
            .map(|c| completion(self.design, whole, c, self.pitch));
        let ctx: &NetCtx = rest.as_ref().unwrap_or(whole);
        let params = Params {
            max_expansions: if share.is_some() { NODES } else { LEGAL_NODES },
            partial: plane || many,
            stubs: many,
            pitch: self.pitch,
            under_pads: false,
            bounds: near.unwrap_or(self.params.bounds),
            ..self.params
        };
        let steps = StepCache::default();
        let tree = {
            let mut grow = NetRun::new(&*world, ctx, &steps, params);
            grow.share(share);
            if grow.stage(params, None, None).is_err() && !params.partial {
                // What it reaches is kept, for the rounds to come.
                let part = Params {
                    partial: true,
                    stubs: true,
                    ..params
                };
                let _ = grow.stage(part, None, None);
            }
            grow.finish()
        };
        self.expanded += tree.expanded;
        let more = NetCopper::from_tree(ctx, &tree);
        let mut c = match kept {
            Some(mut old) => {
                old.segs.extend(more.segs);
                for v in more.vias {
                    if !old.vias.contains(&v) {
                        old.vias.push(v);
                    }
                }
                old.merge(whole);
                old
            }
            None => more,
        };
        if c.is_empty() {
            routes.missing[k].clear();
            routes.apart[k] = 0;
            return;
        }
        let mut pieces = c.pieces(whole);
        if pieces.apart() == 0 && small {
            c.keep.clear();
            c.prune(whole);
            pieces = c.pieces(whole);
        }
        routes.missing[k] = pieces.missing();
        routes.apart[k] = pieces.apart();
        c.place(world, whole);
        routes.copper[k] = Some(c);
    }
}

#[cfg(test)]
mod tests {
    use super::super::detail::Fine;
    use super::super::geometry::{NetCopper, Seg};
    use super::super::tests::{MM, design, smd};
    use super::super::world::World;
    use super::super::{RouteOptions, router};
    use super::overlaps;
    use crate::geom::Point;
    use crate::id::NetId;

    #[test]
    fn the_history_is_kept_by_cell_and_layer() {
        let mut fine = Fine::new(400_000);
        let p = Point::new(1_000_000, -300_000);
        assert_eq!(fine.at(p, 0), 0.0);
        fine.add(p, 0, 0.25);
        fine.add(Point::new(900_000, -100_000), 0, 0.25);
        assert_eq!(fine.at(p, 0), 0.5);
        // Another layer and another cell have none.
        assert_eq!(fine.at(p, 1), 0.0);
        assert_eq!(fine.at(Point::new(1_300_000, -300_000), 0), 0.0);
    }

    #[test]
    fn copper_is_in_the_way_of_another_net_s_and_not_of_its_own() {
        // Net 1 from left to right through a via of its own, and net 2 across its track:
        // net 1's via is within the distance between holes of itself, which says nothing,
        // and the two tracks are in each other's way where they cross, and only there.
        let pads = vec![
            smd(0, 0, 1),
            smd(10 * MM, 0, 1),
            smd(5 * MM, 2 * MM, 2),
            smd(5 * MM, -2 * MM, 2),
        ];
        let mut d = design(2, pads, 3);
        d.nets[0].route = false;
        let options = RouteOptions::default();
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
        let jobs = router::jobs(&d, &world, pitch, &[], &[]);
        let job = |n: u32| jobs.iter().find(|j| j.ctx.net == NetId(n)).unwrap();
        let (one, two) = (&job(1).ctx, &job(2).ctx);
        let mut across = NetCopper::default();
        across.segs.push(Seg {
            a: Point::new(0, 0),
            b: Point::new(10 * MM, 0),
            l: 0,
        });
        across.vias.push(Point::new(2 * MM, 0));
        across.place(&mut world, one);
        let (nets, at) = overlaps(&world, one, &across, pitch);
        assert!(nets.is_empty() && at.is_empty(), "{nets:?} {at:?}");
        let mut down = NetCopper::default();
        down.segs.push(Seg {
            a: Point::new(5 * MM, 2 * MM),
            b: Point::new(5 * MM, -2 * MM),
            l: 0,
        });
        down.place(&mut world, two);
        let (nets, at) = overlaps(&world, two, &down, pitch);
        assert_eq!(nets, [NetId(1)]);
        assert!(!at.is_empty());
        assert!(at.iter().all(|&(p, l)| l == 0 && p.y.abs() <= MM), "{at:?}");
        let (nets, at) = overlaps(&world, one, &across, pitch);
        assert_eq!(nets, [NetId(2)]);
        assert!(
            at.iter().all(|&(p, _)| (p.x - 5 * MM).abs() <= MM),
            "{at:?}"
        );
    }
}
