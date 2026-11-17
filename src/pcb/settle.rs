//! Settling: once rip-up has had its passes, the connections still open are made one at a
//! time, each with the nets in its way moved as far as it takes, and kept only if the
//! board then has more connections than it had.
//!
//! Rip-up takes a net out for another and routes it again when its turn comes, on a board
//! that has changed by then: what it gains for one net it may lose for the next, and a
//! pass that goes on ends with fewer connections than it had at its best. A route for an
//! open connection is looked for here as rip-up looks for it, across other nets' routes
//! at a price. The nets it crosses are cut where it crosses them and joined up again at
//! once, around it, each crossing further nets if it has to, which are joined up again in
//! their turn: a chain of moves, in which a net that has moved is not moved again. When
//! the chain ends, the routes are kept if they make more connections than those before
//! it, and otherwise everything goes back as it was, to the track, and the net that could
//! not be joined up again is not crossed in the next try for the same connection. So no
//! try leaves fewer connections than it found.
//!
//! What is counted is what a host that fills the zones again would count: the connections
//! that the routes leave open, and the pieces of fill with pads on them that the routes
//! cut off the fill their net is planned on, where the run keeps [count](super::pours) of
//! those: a chain that makes a connection by cutting a pad off its fill is undone. The
//! pieces themselves are left to the stubs that join them once routing is over, which move
//! nothing: a chain for each net with such pieces, across what is in the way, made no more
//! of them and cost a board with every net routed a third of its time.

use std::collections::{BTreeMap, VecDeque};

use super::Design;
use super::detail::{self, History, NetCtx, NetRun, Params, RipUp, StepCache};
use super::geometry::NetCopper;
use super::pours::Pours;
use super::router::{Job, completion};
use super::world::World;
use crate::id::NetId;

/// How many nets a chain moves at most, the one it is for included.
const CHAIN: usize = 12;

/// How many times a connection is tried, each time with one more net that may not be
/// crossed: the one that the try before could not join up again.
const TRIES: usize = 4;

/// How many nodes a search that crosses nothing may expand, and one that may cross other
/// nets' routes.
const QUIET_NODES: u64 = 40_000;
const CROSS_NODES: u64 = 300_000;

/// How many nodes the search that joins up a net that was cut may expand before it
/// crosses other nets' routes in its turn.
const MEND_NODES: u64 = 150_000;
const MEND_CROSS_NODES: u64 = 300_000;

/// How many nets a route may cross at most.
const CUT: usize = 4;

/// How many nets deep a chain goes: the nets cut by a net this far from the first are
/// joined up again across nothing, or not at all.
const DEPTH: usize = 3;

/// What crossing another net's route costs a route of a chain, in millimetres of track,
/// as in rip-up. At three times that, with chains half as long and half as deep, a board
/// of two layers kept fewer chains.
const PENALTY: f64 = 10.0;

/// How many times the open nets are gone through at most, for as long as a round makes a
/// connection.
const ROUNDS: usize = 4;

/// How many negotiations in a row may be undone before a settling has no more: on a
/// board whose open pads are shut in by what does not move, each costs a search.
const PATIENCE: usize = 8;

/// In what a job has [spent](Settle::spent): whether a negotiation for it was undone, and
/// whether its chains were.
const TRIED: u8 = 1;
const CHAINED: u8 = 2;

/// What a net had before a chain touched it.
pub(super) struct Before {
    pub(super) job: usize,
    pub(super) copper: Option<NetCopper>,
    pub(super) missing: Vec<usize>,
    pub(super) apart: usize,
}

/// How a chain ended, with nothing undone yet.
pub(super) struct Chain {
    /// Whether every net that it cut is whole again, and its routes leave fewer
    /// connections open than there were.
    pub(super) whole: bool,
    pub(super) journal: Vec<Before>,
    /// The net that could not be joined up again, if that is why it is not whole.
    pub(super) culprit: Option<NetId>,
}

/// The routes of a run, as [`Settle`] changes them.
pub(crate) struct Routes<'a> {
    pub(crate) copper: &'a mut [Option<NetCopper>],
    pub(crate) missing: &'a mut [Vec<usize>],
    pub(crate) apart: &'a mut [usize],
}

impl Routes<'_> {
    /// How many connections net `k` of `nets` still needs.
    pub(super) fn open(&self, nets: &[&NetCtx], k: usize) -> usize {
        match &self.copper[k] {
            Some(_) => self.apart[k],
            None => nets[k].terminals - 1,
        }
    }

    /// How many connections are open in all.
    pub(super) fn open_all(&self, nets: &[&NetCtx]) -> usize {
        (0..nets.len()).map(|k| self.open(nets, k)).sum()
    }
}

/// The settling of a run's routes.
pub(crate) struct Settle<'a> {
    pub(crate) design: &'a Design,
    pub(crate) by_net: &'a BTreeMap<NetId, usize>,
    pub(crate) plane: &'a dyn Fn(NetId) -> bool,
    pub(crate) params: Params,
    /// The pitch of the first pass, which the lattices of pads go on by.
    pub(crate) pitch: i64,
    pub(crate) history: &'a History,
    /// The nets that are not moved: those of pairs routed side by side.
    pub(crate) fixed: &'a [NetId],
    /// The fills that nets are planned on, as cells, if the run keeps count of what its
    /// routes cut off them.
    pub(crate) pours: Option<&'a Pours>,
    /// The nodes its searches expanded.
    pub(crate) expanded: u64,
    /// The first of the nets that the last route found crossed, if they were too many.
    pub(crate) blocked: Option<NetId>,
    /// The chains tried, the chains kept, and the connections those made.
    pub(crate) tried: usize,
    pub(crate) kept: usize,
    pub(crate) made: usize,
    /// Whether what is open has its chains: after the last pass, as the passes on the
    /// finer lattice make more for the work than they do.
    pub(crate) chains: bool,
    /// Whether a connection is first looked for by a negotiation, and the negotiations
    /// tried, those kept, and the connections those made.
    pub(crate) negotiate: bool,
    pub(crate) negotiated: (usize, usize, usize),
    /// By job, whether a negotiation for it was undone and whether its chains all were,
    /// in this settling or one before it: they are not tried again.
    pub(crate) spent: &'a mut [u8],
}

impl Settle<'_> {
    /// Goes through the nets with connections open, until a round makes no connection or
    /// the run has to stop. Every net of a job may be crossed, whatever rip-up locked.
    pub(crate) fn all(&mut self, world: &mut World<'_>, jobs: &[Job], routes: &mut Routes<'_>) {
        let locked: Vec<bool> = jobs.iter().map(|j| world.locked(j.ctx.net)).collect();
        let fixed: Vec<bool> = self.fixed.iter().map(|&n| world.locked(n)).collect();
        for job in jobs.iter() {
            world.set_locked(job.ctx.net, false);
        }
        for &net in self.fixed {
            world.set_locked(net, true);
        }
        // What is open: connections, and pieces of fill that are cut off.
        let mut score = self.score(world, jobs, routes);
        // By job, the nets that a try for it could not join up again.
        let mut walls: Vec<Vec<NetId>> = vec![Vec::new(); jobs.len()];
        let mut idle = false;
        // Every connection has its negotiation before any has its chains: a negotiation
        // makes more for the work than a chain does.
        let stages: &[bool] = match (self.negotiate, self.chains) {
            (true, true) => &[true, false],
            (true, false) => &[true],
            (false, true) => &[false],
            (false, false) => &[],
        };
        'rounds: for (first, round) in stages
            .iter()
            .flat_map(|&first| (0..ROUNDS).map(move |r| (first, r)))
        {
            // The rounds of a stage end with the first that makes nothing.
            if round == 0 {
                idle = false;
            } else if idle {
                continue;
            }
            let mut gained = false;
            // How many negotiations in a row were undone.
            let mut undone = 0;
            for j in 0..jobs.len() {
                if self.fixed.contains(&jobs[j].ctx.net) {
                    continue;
                }
                // After that many, what is left open is left to the chains, and to the
                // settling after the next pass.
                if first && undone >= PATIENCE {
                    break;
                }
                let mut tries = 0;
                // Whether the next try is a negotiation: the first for a connection, and
                // the first after one that was kept.
                let mut negotiate = first && self.spent[j] & TRIED == 0;
                while tries < TRIES {
                    if world.run().is_some_and(|r| r.poll()) {
                        break 'rounds;
                    }
                    let negotiated = core::mem::take(&mut negotiate);
                    let chain = {
                        let nets: Vec<&NetCtx> = jobs.iter().map(|j| &j.ctx).collect();
                        if routes.open(&nets, j) == 0
                            || (!negotiated && (first || self.spent[j] & CHAINED != 0))
                        {
                            break;
                        }
                        if negotiated {
                            self.negotiated.0 += 1;
                            self.negotiation(world, &nets, routes, j)
                        } else {
                            tries += 1;
                            self.tried += 1;
                            self.chain(world, &nets, routes, j, &walls[j])
                        }
                    };
                    let now = if chain.whole {
                        self.score(world, jobs, routes)
                    } else {
                        score
                    };
                    if chain.whole && now < score {
                        if negotiated {
                            self.negotiated.1 += 1;
                            self.negotiated.2 += score - now;
                            negotiate = true;
                            undone = 0;
                        } else {
                            self.kept += 1;
                            self.made += score - now;
                        }
                        score = now;
                        gained = true;
                        tries = 0;
                        walls[j].clear();
                        continue;
                    }
                    let nets: Vec<&NetCtx> = jobs.iter().map(|j| &j.ctx).collect();
                    for b in chain.journal.into_iter().rev() {
                        if let Some(mut c) = routes.copper[b.job].take() {
                            c.unplace(world);
                        }
                        routes.copper[b.job] = b.copper.map(|mut c| {
                            c.place(world, nets[b.job]);
                            c
                        });
                        routes.missing[b.job] = b.missing;
                        routes.apart[b.job] = b.apart;
                    }
                    if negotiated {
                        self.spent[j] |= TRIED;
                        undone += 1;
                        continue;
                    }
                    match chain.culprit {
                        Some(net) if !walls[j].contains(&net) => walls[j].push(net),
                        // Nothing to try another way.
                        _ => break,
                    }
                }
                let nets: Vec<&NetCtx> = jobs.iter().map(|j| &j.ctx).collect();
                if !first && routes.open(&nets, j) > 0 {
                    self.spent[j] |= CHAINED;
                }
            }
            if !gained {
                idle = true;
                continue;
            }
            // What a round changed may let the next cross what this one could not.
            for w in &mut walls {
                w.clear();
            }
        }
        for (job, was) in jobs.iter().zip(locked) {
            world.set_locked(job.ctx.net, was);
        }
        for (&net, was) in self.fixed.iter().zip(fixed) {
            world.set_locked(net, was);
        }
    }

    /// What the routes leave open: the connections, and the pieces of fill with pads on
    /// them that are cut off the largest of their net's.
    fn score(&self, world: &World<'_>, jobs: &[Job], routes: &Routes<'_>) -> usize {
        let nets: Vec<&NetCtx> = jobs.iter().map(|j| &j.ctx).collect();
        let cut: usize = self.pours.map_or(0, |p| {
            p.jobs()
                .filter_map(|j| p.rejoin(world, self.design, jobs, routes.copper, j, self.pitch))
                .map(|again| again.cut)
                .sum()
        });
        routes.open_all(&nets) + cut
    }

    /// One chain for job `j`, with the nets `walls` not to be crossed. Nothing is undone.
    fn chain(
        &mut self,
        world: &mut World<'_>,
        nets: &[&NetCtx],
        routes: &mut Routes<'_>,
        j: usize,
        walls: &[NetId],
    ) -> Chain {
        let open_before = routes.open_all(nets);
        let mut journal: Vec<Before> = Vec::new();
        // The nets that this chain locked, which were not.
        let mut held: Vec<NetId> = Vec::new();
        for &net in walls {
            if !world.locked(net) {
                world.set_locked(net, true);
                held.push(net);
            }
        }
        let mut queue: VecDeque<(usize, usize)> = VecDeque::from([(j, 0)]);
        let mut moved = 0;
        let mut culprit = None;
        let mut broken = false;
        while let Some((k, depth)) = queue.pop_front() {
            if world.run().is_some_and(|r| r.poll()) {
                broken = true;
                culprit = None;
                break;
            }
            if routes.open(nets, k) == 0 {
                continue;
            }
            moved += 1;
            if moved > CHAIN {
                culprit = Some(nets[k].net);
                broken = true;
                break;
            }
            let joined = self.join(world, nets, routes, (k, depth), &mut journal, &mut queue);
            let net = nets[k].net;
            if !world.locked(net) {
                world.set_locked(net, true);
                held.push(net);
            }
            if !joined {
                culprit = self.blocked.take().or(culprit);
            }
            if !joined && k == j {
                broken = true;
                break;
            }
            // A net that was cut and is not whole again ends the chain: it is what the
            // next try does not cross.
            if routes.open(nets, k) > 0 && k != j {
                culprit = Some(net);
                broken = true;
                break;
            }
        }
        for net in held {
            world.set_locked(net, false);
        }
        Chain {
            whole: !broken && routes.open_all(nets) < open_before,
            journal,
            culprit,
        }
    }

    /// Notes what job `k` has, the first time a chain touches it.
    pub(super) fn note(routes: &Routes<'_>, journal: &mut Vec<Before>, k: usize) {
        if journal.iter().all(|b| b.job != k) {
            journal.push(Before {
                job: k,
                copper: routes.copper[k].as_ref().map(NetCopper::detached),
                missing: routes.missing[k].clone(),
                apart: routes.apart[k],
            });
        }
    }

    /// Joins up job `k` from the copper it has, across other nets' routes if it has to:
    /// those are cut where its route crosses them, and wait in `queue` to be joined up
    /// again. Whether it found anything.
    fn join(
        &mut self,
        world: &mut World<'_>,
        nets: &[&NetCtx],
        routes: &mut Routes<'_>,
        (k, depth): (usize, usize),
        journal: &mut Vec<Before>,
        queue: &mut VecDeque<(usize, usize)>,
    ) -> bool {
        let whole: &NetCtx = nets[k];
        let plane = (self.plane)(whole.net);
        let many = whole.terminals > detail::BUDGET_TERMINALS;
        let rest = routes.copper[k]
            .as_ref()
            .map(|c| completion(self.design, whole, c, self.pitch));
        let ctx: &NetCtx = rest.as_ref().unwrap_or(whole);
        let quiet = RipUp {
            penalty: None,
            history: self.history,
        };
        let rip = RipUp {
            penalty: Some(PENALTY),
            history: self.history,
        };
        let near = Params {
            max_expansions: if depth == 0 { QUIET_NODES } else { MEND_NODES },
            partial: false,
            stubs: many,
            forest: true,
            ..self.params
        };
        let far = Params {
            max_expansions: if depth == 0 {
                CROSS_NODES
            } else {
                MEND_CROSS_NODES
            },
            ..near
        };
        let steps = StepCache::default();
        let (tree, victims) = {
            let world_ref = &*world;
            let mut grow = NetRun::new(world_ref, ctx, &steps, near);
            let mut done = grow.stage(near, None, Some(quiet)).is_ok();
            // What the first net of a chain reaches with nothing moved is kept as it is:
            // a chain for the rest is another try.
            if !done && depth == 0 && ctx.terminals > 2 {
                let part = Params {
                    partial: true,
                    stubs: true,
                    ..near
                };
                let _ = grow.stage(part, None, Some(quiet));
                done = grow.apart() + 1 < ctx.terminals;
            }
            let mut crossed = false;
            let mut spent = 0;
            if !done && depth < DEPTH {
                // From nothing again, after a search that gave terminals up: it does not
                // look for them any more.
                if depth == 0 && ctx.terminals > 2 {
                    spent = grow.finish().expanded;
                    grow = NetRun::new(world_ref, ctx, &steps, far);
                }
                crossed = true;
                done = grow.stage(far, None, Some(rip)).is_ok();
            }
            if !done && depth == 0 && (ctx.terminals > 2 || plane) {
                let part = Params {
                    partial: true,
                    stubs: true,
                    ..far
                };
                let _ = grow.stage(part, None, Some(rip));
            }
            let mut tree = grow.finish();
            tree.expanded += spent;
            let victims = if crossed && !tree.edges.is_empty() {
                detail::victims(world_ref, ctx, &tree).0
            } else {
                Vec::new()
            };
            (tree, victims)
        };
        self.expanded += tree.expanded;
        if tree.edges.is_empty() {
            return false;
        }
        Self::note(routes, journal, k);
        let more = NetCopper::from_tree(ctx, &tree);
        // The nets in its way come out, and go back in the pieces that its route leaves.
        let mut cut: Vec<(usize, NetCopper)> = Vec::new();
        // A route across what is no job's can't be placed, and one across many nets
        // would have them all to join up again.
        if victims.iter().any(|v| !self.by_net.contains_key(v)) || victims.len() > CUT {
            self.blocked = victims
                .iter()
                .copied()
                .find(|v| self.by_net.contains_key(v));
            return false;
        }
        for v in victims {
            let Some(&q) = self.by_net.get(&v) else {
                continue;
            };
            if cut.iter().any(|c| c.0 == q) {
                continue;
            }
            Self::note(routes, journal, q);
            if let Some(mut c) = routes.copper[q].take() {
                c.unplace(world);
                cut.push((q, c));
            }
        }
        let mut c = match routes.copper[k].take() {
            Some(mut old) => {
                old.unplace(world);
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
        let mut pieces = c.pieces(whole);
        if pieces.apart() == 0 && whole.terminals < super::router::BRANCH_TERMINALS && !plane {
            c.keep.clear();
            c.prune(whole);
            pieces = c.pieces(whole);
        }
        routes.missing[k] = pieces.missing();
        routes.apart[k] = pieces.apart();
        c.place(world, whole);
        routes.copper[k] = Some(c);
        for (q, old) in cut {
            let ctx = nets[q];
            let mut kept = old.free_in(world, ctx, self.params.pitch);
            if ctx.terminals < super::router::BRANCH_TERMINALS && !(self.plane)(ctx.net) {
                kept.drop_floating(ctx);
            } else {
                kept.prune(ctx);
            }
            if kept.is_empty() {
                routes.missing[q].clear();
                routes.apart[q] = 0;
            } else {
                kept.place(world, ctx);
                let pieces = kept.pieces(ctx);
                routes.missing[q] = pieces.missing();
                routes.apart[q] = pieces.apart();
                routes.copper[q] = Some(kept);
            }
            if queue.iter().all(|w| w.0 != q) {
                queue.push_back((q, depth + 1));
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::detail::{Corridor, History, Params};
    use super::super::geometry::{NetCopper, Seg};
    use super::super::tests::{MM, design, outline, smd};
    use super::super::world::World;
    use super::super::{Design, RouteOptions, router};
    use super::{Routes, Settle};
    use crate::geom::{Point, Rect};
    use crate::id::NetId;

    /// Two layers, with no way round its pads: net 1 from left to right, and net 2, which
    /// has no via, from top to bottom across it.
    fn crossing() -> Design {
        let pads = vec![
            smd(0, 0, 1),
            smd(10 * MM, 0, 1),
            smd(5 * MM, 2 * MM, 2),
            smd(5 * MM, -2 * MM, 2),
        ];
        let mut d = design(2, pads, 3);
        d.nets[0].route = false;
        d.nets[2].via = None;
        d.outline = outline(Rect::new(
            Point::new(-600_000, -2_600_000),
            Point::new(10_600_000, 2_600_000),
        ));
        d.edge_clearance = 200_000;
        d
    }

    /// Settles the board with net 1 routed straight across and net 2 open. Returns the
    /// tracks and vias of each, and the chains tried and kept.
    fn settled(d: &Design) -> ([(usize, usize); 2], usize, usize) {
        let (counts, tried, kept, _) = settled_by(d, false);
        (counts, tried, kept)
    }

    /// As [`settled`], by a negotiation before the chains if `negotiate`: with the
    /// negotiations tried and kept and the connections they made.
    #[allow(clippy::type_complexity)]
    fn settled_by(
        d: &Design,
        negotiate: bool,
    ) -> ([(usize, usize); 2], usize, usize, (usize, usize, usize)) {
        let options = RouteOptions::default();
        let mut world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )
        .unwrap();
        world.configure(d, &options);
        let pitch = router::pitch(d, &options);
        let jobs = router::jobs(d, &world, pitch, &[], &[]);
        let by_net: BTreeMap<NetId, usize> = jobs
            .iter()
            .enumerate()
            .map(|(i, j)| (j.ctx.net, i))
            .collect();
        let (one, two) = (by_net[&NetId(1)], by_net[&NetId(2)]);
        let mut copper: Vec<Option<NetCopper>> = vec![None; jobs.len()];
        let mut straight = NetCopper::default();
        straight.segs.push(Seg {
            a: Point::new(0, 0),
            b: Point::new(10 * MM, 0),
            l: 0,
        });
        straight.place(&mut world, &jobs[one].ctx);
        copper[one] = Some(straight);
        let mut missing = vec![Vec::new(); jobs.len()];
        let mut apart = vec![0; jobs.len()];
        let bounds = Rect::new(Point::new(-MM, -5 * MM), Point::new(11 * MM, 5 * MM));
        let history = History {
            grid: Corridor {
                origin: bounds.min(),
                tile: 16 * MM,
                cols: 1,
                rows: 1,
                cells: vec![0],
            },
            cost: vec![0.0],
        };
        let mut spent = vec![0u8; jobs.len()];
        let mut settle = Settle {
            design: d,
            by_net: &by_net,
            plane: &|_| false,
            params: Params {
                pitch,
                via_cost: options.via_cost,
                layer_bias: 0.0,
                bounds,
                max_expansions: options.max_expansions,
                under_pads: false,
                partial: false,
                stubs: false,
                forest: false,
            },
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
            negotiate,
            negotiated: (0, 0, 0),
            spent: &mut spent,
        };
        let mut routes = Routes {
            copper: &mut copper,
            missing: &mut missing,
            apart: &mut apart,
        };
        settle.all(&mut world, &jobs, &mut routes);
        let (tried, kept, negotiated) = (settle.tried, settle.kept, settle.negotiated);
        let count = |k: usize| {
            copper[k]
                .as_ref()
                .map_or((0, 0), |c| (c.segs.len(), c.vias.len()))
        };
        // What is open is what the copper leaves apart.
        for k in [one, two] {
            if let Some(c) = &copper[k] {
                assert_eq!(c.pieces(&jobs[k].ctx).apart(), apart[k]);
            }
        }
        ([count(one), count(two)], tried, kept, negotiated)
    }

    #[test]
    fn a_negotiation_makes_the_connection_and_the_net_in_its_way_goes_under_it() {
        // Net 2 is routed across net 1's track at a price, net 1 is in its way and is
        // routed again where it was cut, under net 2 through two vias: no route is in
        // another's way, both nets are whole, and no chain was needed.
        let d = crossing();
        let ([one, two], tried, kept, negotiated) = settled_by(&d, true);
        assert_eq!(negotiated, (1, 1, 1));
        assert_eq!((tried, kept), (0, 0));
        assert_eq!(two.1, 0, "{two:?}");
        assert!(two.0 >= 1, "{two:?}");
        assert_eq!(one.1, 2, "{one:?}");
    }

    #[test]
    fn a_negotiation_that_ends_with_no_more_connections_is_undone_to_the_track() {
        // With no via for net 1 either, the two stay in each other's way at any price:
        // the net that the negotiation is for keeps its route, net 1 is left in two
        // pieces, the board has no more connections, and everything goes back.
        let mut d = crossing();
        d.nets[1].via = None;
        let ([one, two], _, kept, negotiated) = settled_by(&d, true);
        assert_eq!(negotiated, (1, 0, 0));
        assert_eq!(kept, 0);
        assert_eq!(one, (1, 0));
        assert_eq!(two, (0, 0));
    }

    #[test]
    fn a_net_in_the_way_of_an_open_connection_is_cut_and_joined_up_again_around_it() {
        // Net 2 has no way but across net 1's track, which goes under it through two
        // vias: one chain, kept, and both nets whole.
        let d = crossing();
        let ([one, two], tried, kept) = settled(&d);
        assert_eq!((tried, kept), (1, 1));
        assert_eq!(two, (1, 0));
        assert_eq!(one.1, 2, "{one:?}");
    }

    #[test]
    fn a_chain_that_leaves_no_more_connections_is_undone_to_the_track() {
        // With no via for net 1 either, it can't be joined up again around net 2: every
        // try is undone, and net 1 has its one track as it had.
        let mut d = crossing();
        d.nets[1].via = None;
        let ([one, two], tried, kept) = settled(&d);
        assert!(tried >= 1);
        assert_eq!(kept, 0);
        assert_eq!(one, (1, 0));
        assert_eq!(two, (0, 0));
    }
}
