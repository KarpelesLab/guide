//! Multi-agent path finding, by negotiated congestion.
//!
//! Agents start on vertices of a graph and have to reach their goals, one step or one wait per
//! time step, without two of them ever being on the same vertex at once or swapping places
//! along an edge, and stay on their goals once there. That is the classic problem of Stern et
//! al., *Multi-Agent Pathfinding: Definitions, Variants, and Benchmarks* (SoCS 2019), with
//! vertex and swap conflicts, agents that stay at their targets, and the sum of costs as the
//! objective.
//!
//! Nothing here is a new algorithm. Agents are planned one at a time, each seeing the others'
//! plans at a price, and the price rises until nobody collides, as PathFinder does on an FPGA
//! and as MAPF-LNS2 (Li et al., AAAI 2022) calls soft conflicts. [`solve`] does it two ways,
//! as [`SolveOptions::method`] says:
//!
//! - [`Method::SafeIntervals`], the default: each agent is planned over [safe
//!   intervals](#safe-intervals), with the engine's schedule and prices, and a first solution
//!   by LaCAM to fall back on.
//! - [`Method::TimeExpanded`]: [`TimeSpace`] expands an [`Instance`] in time, as a
//!   [`RoutingSpace`](crate::space::RoutingSpace) whose nodes are a vertex at a time and whose
//!   resources are a vertex at a time and an edge during a move, each of capacity 1, and
//!   [`crate::engine`] negotiates over it unchanged. Agents are nets, and each net has two
//!   terminals, its start at time 0 and its goal.
//!
//! [`solve`] has an example.
//!
//! # Safe intervals
//!
//! A time-expanded space has a state for each vertex at each time to a horizon, and a search
//! that has to wait for a corridor to clear tries every place and length of wait there is. On
//! the large MovingAI maps, with paths of a thousand steps and more, that made each iteration
//! of negotiation take seconds to minutes. Safe-interval path planning (SIPP, Phillips and
//! Likhachev, ICRA 2011) has a state for each vertex and interval of time over which nobody
//! arrives or leaves: an agent arrives in an interval as early as it can and may stay to its
//! end. There is no horizon, and a vertex has as many states as agents pass through it.
//!
//! Negotiation needs conflicts priced, not forbidden. So an interval here is one over which
//! neither who else is on the vertex nor its history changes, and one with someone in it may
//! be entered, at the engine's price, `h + (1 + h) * present * o` on top of the step's cost of
//! 1, with `o` the agents there and `h` its history; edges are priced the same way, for swaps.
//! That is SIPPS's idea (Li et al., AAAI 2022), with prices for collisions. Earliest arrival
//! is no longer exact once intervals have prices, as arriving later in a dear interval after a
//! wait where it was cheap can pay less, so the planner keeps every arrival in an interval that
//! no other beats once it has waited for it. Negotiation only needs prices to point the way;
//! once they are high, a plan through free intervals alone is the cheapest there is, and that
//! search is exact.
//!
//! The engine's table of resources, a hash map from a resource to who holds it, can't tell a
//! search when a vertex is next free without a lookup per time step, so this negotiation keeps
//! its own table, per vertex and in time order, and runs the engine's loop over it: every agent
//! planned in the first iteration at a present factor of `first_present`, then only those in
//! conflict when their turn comes, with the present factor growing by `present_growth` up to
//! `max_present`, history adding `history_factor` per unit of overuse each iteration, and the
//! engine's iteration limit, budget and cancel flag. The engine itself is unchanged.
//!
//! One thing had to be added. Two agents in a dead end, one holding its goal at the mouth and
//! one that has to pass it to reach its own, deeper, can end up each answering the other's
//! plan: the second waits outside until the first is there, passes, and conflicts, and history
//! on that time sends it a step later the next iteration, and the next, without end. Once the
//! present factor is at its most, history from a conflict on a goal goes on every time from the
//! holder's arrival on, so that arriving early, and passing late, both get dear. On maze-128-128-2
//! with 100 agents, that took negotiation from 100 iterations without a solution to 19.
//!
//! # A first solution
//!
//! Before negotiating, [`solve`] finds a [first solution](first_solution) by LaCAM (Okumura,
//! AAAI 2023) over PIBT, which moves every agent a step at a time and takes milliseconds where
//! negotiation takes seconds. On the 1500 instances of the six MovingAI maps of
//! `docs/benchmarks.md`, its sums of costs were 37% higher than negotiation's in all, and up
//! to twice as high; negotiation's was never higher. So negotiation's solution is the one
//! returned when it settles, and the first one is returned, shortened, when negotiation hits its
//! iteration limit or is stopped, with [`Solution::fell_back`] set. Prioritized planning over
//! hard safe intervals as negotiation's first iteration gave sums of costs up to 4% higher and
//! was no faster, and a legal solution leaves negotiation nothing to do.
//!
//! # Staying on the goal, through time
//!
//! In a [`TimeSpace`], an agent that arrives on its goal for good at time `t` goes on from
//! there through a [`State::Stay`] at each time to the horizon, which holds the goal at that
//! time, and then into [`State::Goal`], the target of every route. Another agent passing
//! through later conflicts with a stay, so the engine sees the rule like any other. Moves and
//! waits cost 2 and stays 1, so that a route's cost is the horizon plus 1 plus its arrival
//! time: [`TimeSpace`] has why.
//!
//! The first version held the goal to the horizon in one step from the goal into
//! [`State::Goal`]. That step had to be priced over every time to the horizon each time a search
//! reached the goal, at each time it did, and on den520d, with a horizon of about 420, pricing
//! it was most of the time. Chains of stays took a run of 600 agents there from 10.4 s to
//! 4.7 s, and gave a lower sum of costs on every map measured (`docs/benchmarks.md`).
//!
//! # The horizon
//!
//! In a [`TimeSpace`], time has to stop somewhere, and the horizon is a trade: too short and no solution fits, too
//! long and every final step holds more resources. [`solve`] starts from the longest of the
//! agents' shortest paths plus [`slack`](SolveOptions::slack), and, if negotiation doesn't
//! settle within the engine's iterations, lengthens it and runs again from the routes and
//! history it ended with (iterative deepening). Searches are never offered a state from which
//! the goal can't be reached by the horizon, so a horizon costs nothing where no route goes.
//!
//! # Shortening
//!
//! Negotiation stops at the first legal solution, found at prices that no longer hold. With
//! [`improve_passes`](SolveOptions::improve_passes), as by default, [`solve`] then plans each
//! agent again in turn, by an exact search against the others as hard reservations, over safe
//! intervals or through time as the method has it, and keeps what arrives sooner, until a pass
//! changes nothing. No agent gets worse, and the solution stays legal.
//!
//! Routing small groups of agents again by negotiation after that, a late agent and those in
//! the way of its shortest path, with everyone else's resources at a capacity of 0, as MAPF-LNS
//! does with prioritized planning, was tried and dropped: 200 groups of up to 8 took the sum
//! of costs up to 1% further down, for 1.5 to 22 times the time, and 1000 no further.
//!
//! # Checking
//!
//! [`check`](check::check) checks plans against the graph and the agents only, and knows
//! nothing of the space or the engine. [`movingai`] reads the MovingAI benchmarks.

pub mod check;
mod graph;
mod improve;
pub mod movingai;
mod negotiate;
mod pibt;
mod sipp;
mod space;

pub use graph::{Agent, Graph, GraphError};
pub use space::{Instance, InstanceError, Resource, State, TimeSpace};

use crate::control::{Progress, Run, RunControl, Stage, Stopped};
use crate::cost::Cost;
use crate::engine::{Engine, EngineOptions, StopReason};

/// How [`solve`] runs.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct SolveOptions<'c> {
    /// The engine's options for each round. Its expansion budget and cancel flag cover the
    /// whole of a solve. The default is [`engine_options`].
    pub engine: EngineOptions<'c>,
    /// How far the first horizon is past the longest of the agents' shortest paths. The
    /// default is 16.
    pub slack: u32,
    /// How many rounds to run at most, each with a longer horizon. The default is 4.
    pub rounds: u32,
    /// How many threads search at once, which only helps with a
    /// [`batch_size`](EngineOptions::batch_size) above 1, and never changes results. The
    /// default is 1.
    pub threads: usize,
    /// How many passes to make over the agents after a legal solution, planning each again
    /// against the others to arrive sooner, as [shortening](self#shortening) has it. The
    /// default is 10, and 0 turns it off.
    ///
    /// On the MovingAI maps of `docs/benchmarks.md`, it took sums of costs up to 2% lower, least
    /// on the large maps, for 15% to 30% more time.
    pub improve_passes: u32,
    /// What agents are planned through. The default is [`Method::SafeIntervals`].
    pub method: Method,
    /// Whether to find a [first solution](first_solution) before negotiating, to fall back
    /// on if negotiation doesn't settle or is stopped, with [`Method::SafeIntervals`]. The
    /// default is true.
    pub first: bool,
}

/// What [`solve`] plans agents through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Method {
    /// A [`TimeSpace`], negotiated by [`crate::engine::Engine`], with a horizon that grows
    /// over [`rounds`](SolveOptions::rounds).
    TimeExpanded,
    /// Safe intervals, negotiated with the engine's schedule and prices, as [safe
    /// intervals](self#safe-intervals) has it. [`slack`](SolveOptions::slack),
    /// [`rounds`](SolveOptions::rounds), [`threads`](SolveOptions::threads) and the engine's
    /// options other than its schedule, history, limits and cancel flag don't apply.
    #[default]
    SafeIntervals,
}

impl Default for SolveOptions<'_> {
    fn default() -> Self {
        SolveOptions {
            engine: engine_options(),
            slack: 16,
            rounds: 4,
            threads: 1,
            improve_passes: 10,
            method: Method::default(),
            first: true,
        }
    }
}

/// The engine's options for multi-agent path finding: the engine's defaults, with a history
/// factor of 0.5.
///
/// A move costs 2 in a [`TimeSpace`], so the default history of 1 is half a move per unit of
/// overuse. A quarter of one gave sums of costs up to 1% lower on the MovingAI maps of
/// `docs/benchmarks.md`, and solved as many instances or more within the time limit; an eighth
/// was no better. A first present factor of 1000, which makes the first iteration nearly
/// prioritized planning, gave sums of costs 4% to 17% higher, and solved far fewer instances on
/// den520d. A present factor growth of 1.3 gave sums of costs up to 1.5% lower and took 1.2
/// to 1.7 times as long.
pub fn engine_options() -> EngineOptions<'static> {
    EngineOptions {
        history_factor: Cost::new(0.5).unwrap_or(Cost::ONE),
        ..EngineOptions::default()
    }
}

/// What one round of [`solve`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Round {
    /// Its horizon.
    pub horizon: u32,
    /// Why it stopped.
    pub stop: StopReason,
    /// How many iterations it ran.
    pub iterations: u64,
    /// How many nodes its searches expanded.
    pub nodes_expanded: u64,
    /// How many resources its best routes left overused.
    pub conflicts: usize,
}

/// The result of [`solve`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Solution {
    /// One plan per agent, the vertex at each time from 0 to its last arrival on its goal, if
    /// every conflict was settled.
    pub plans: Option<Vec<Vec<u32>>>,
    /// Each round, in order.
    pub rounds: Vec<Round>,
    /// The sum of costs of the solution that negotiation found, before
    /// [shortening](self#shortening) it, if it found one.
    pub negotiated_sum_of_costs: Option<u64>,
    /// How many passes shortening made.
    pub improve_passes: u32,
    /// How many nodes shortening expanded.
    pub improve_expanded: u64,
    /// Why the solve was stopped before its end, if it was: see [`solve_with`].
    pub stopped: Option<Stopped>,
    /// The sum of costs of the [first solution](first_solution), if one was looked for and
    /// found.
    pub first_sum_of_costs: Option<u64>,
    /// Whether the plans are the first solution's, shortened, because negotiation didn't
    /// settle.
    pub fell_back: bool,
}

impl Solution {
    /// Returns the sum over agents of the time of their last arrival on their goal, for a
    /// solution.
    pub fn sum_of_costs(&self) -> Option<u64> {
        let plans = self.plans.as_ref()?;
        Some(
            plans
                .iter()
                .map(|plan| u64::from(check::plan_cost(plan)))
                .sum(),
        )
    }

    /// Returns the latest arrival, for a solution.
    pub fn makespan(&self) -> Option<u32> {
        let plans = self.plans.as_ref()?;
        Some(
            plans
                .iter()
                .map(|plan| check::plan_cost(plan))
                .max()
                .unwrap_or(0),
        )
    }

    /// Returns how many nodes all rounds and shortening expanded.
    pub fn nodes_expanded(&self) -> u64 {
        self.rounds
            .iter()
            .map(|round| round.nodes_expanded)
            .sum::<u64>()
            + self.improve_expanded
    }

    /// Returns how many iterations all rounds ran.
    pub fn iterations(&self) -> u64 {
        self.rounds.iter().map(|round| round.iterations).sum()
    }
}

/// Solves `instance` by negotiated congestion. See the [module documentation](self).
///
/// The result is the same for any number of threads and on any platform.
///
/// # Examples
///
/// Two agents swap ends of a corridor with a siding. At best, one steps into the siding to let
/// the other by, which takes it 4 steps, and the other waits a step, which takes it 3.
/// Negotiation over safe intervals finds that; through time, it is not exact, and here has
/// both wait a step more than they need to.
///
/// ```
/// use guide::mapf::{Agent, Graph, Instance, Method, SolveOptions, check, solve};
///
/// //  0 - 1 - 2
/// //      |
/// //      3
/// let graph = Graph::from_edges(4, &[(0, 1), (1, 2), (1, 3)])?;
/// let agents = vec![Agent { start: 0, goal: 2 }, Agent { start: 2, goal: 0 }];
/// let instance = Instance::new(&graph, agents.clone())?;
///
/// let solution = solve(&instance, &SolveOptions::default());
/// let plans = solution.plans.as_ref().expect("the siding lets them pass");
/// let summary = check::check(&graph, &agents, plans).expect("the plans are legal");
/// assert_eq!(summary.sum_of_costs, 7);
/// assert!(summary.sum_of_costs >= instance.sum_of_shortest());
///
/// let mut options = SolveOptions::default();
/// options.method = Method::TimeExpanded;
/// assert_eq!(solve(&instance, &options).sum_of_costs(), Some(9));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn solve(instance: &Instance<'_>, options: &SolveOptions<'_>) -> Solution {
    solve_with(instance, options, &RunControl::default())
}

/// [`solve`], under a [`RunControl`].
///
/// Its work budget counts nodes expanded, as the engine's
/// [`max_expansions`](EngineOptions::max_expansions) in [`SolveOptions::engine`] does, and
/// the solve stops at whichever runs out first. Its flag and deadline stop it through a
/// thread that sets the engine's cancel flag. A solve stopped before a legal solution has no
/// plans, as one that runs out of rounds; one stopped while [shortening](self#shortening) has
/// the plans shortened so far, which are legal. [`Solution::stopped`] says why. Its listener
/// hears of each iteration, with the round. With the default control, the result is
/// [`solve`]'s.
pub fn solve_with(
    instance: &Instance<'_>,
    options: &SolveOptions<'_>,
    control: &RunControl<'_>,
) -> Solution {
    let run = Run::new(control);
    if options.method == Method::SafeIntervals {
        return solve_safe_intervals(instance, options, &run);
    }
    let own_flag = options.engine.cancel;
    let nets = instance.nets();
    let mut horizon = instance.longest_shortest().saturating_add(options.slack);
    let mut rounds = Vec::new();
    let mut seeds = Vec::new();
    let mut history = Vec::new();
    let mut budget = options.engine.max_expansions;
    for round in 0..options.rounds.max(1) {
        run.set_round(round);
        let space = TimeSpace::new(instance, horizon);
        let mut engine_options = options.engine;
        engine_options.max_expansions = run.budget(budget);
        engine_options.cancel = run.engine_flag(own_flag);
        let Ok(mut engine) = Engine::new(&space, &nets, engine_options) else {
            // Nets are numbered by agent, so they can't share an ID.
            unreachable!("agents have distinct nets");
        };
        if round > 0 {
            // Both were taken from an engine on the same nets, so neither can fail.
            let _ = engine.seed_routes(seeds.drain(..));
            let _ = engine.seed_history(history.drain(..));
        }
        let outcome = run.watch(own_flag, || {
            let mut expanded = 0;
            while let Some(report) = engine.step_parallel(options.threads) {
                expanded += report.nodes_expanded;
                run.tell_iteration(&report, expanded);
            }
            engine.outcome()
        });
        let stats = outcome.routes.stats;
        budget = budget.saturating_sub(stats.nodes_expanded);
        run.charge(stats.nodes_expanded);
        run.engine_stopped(outcome.stop);
        rounds.push(Round {
            horizon,
            stop: outcome.stop,
            iterations: stats.iterations,
            nodes_expanded: stats.nodes_expanded,
            conflicts: outcome.overused.len(),
        });
        if outcome.is_success() {
            let mut solution = Solution {
                plans: outcome
                    .routes
                    .routes()
                    .map(|(_, route)| TimeSpace::plan(route))
                    .collect::<Option<Vec<_>>>(),
                rounds,
                negotiated_sum_of_costs: None,
                improve_passes: 0,
                improve_expanded: 0,
                stopped: None,
                first_sum_of_costs: None,
                fell_back: false,
            };
            solution.negotiated_sum_of_costs = solution.sum_of_costs();
            if let Some(plans) = solution.plans.as_mut()
                && options.improve_passes > 0
            {
                run.tell(Progress::new(Stage::Improve, 0), 0);
                let improved = run.watch(own_flag, || {
                    improve::improve(
                        &space,
                        plans,
                        options.improve_passes,
                        run.budget(budget),
                        run.engine_flag(own_flag),
                    )
                });
                run.charge(improved.nodes_expanded);
                solution.improve_passes = improved.passes;
                solution.improve_expanded = improved.nodes_expanded;
            }
            solution.stopped = run.stopped();
            return solution;
        }
        if matches!(
            outcome.stop,
            StopReason::Cancelled | StopReason::BudgetExhausted
        ) || run.stopped().is_some()
        {
            break;
        }
        history = engine.history();
        seeds = outcome
            .routes
            .routes()
            .map(|(net, route)| (net, route.clone()))
            .collect();
        horizon = horizon.saturating_add(options.slack.max(horizon / 4));
    }
    Solution {
        plans: None,
        rounds,
        negotiated_sum_of_costs: None,
        improve_passes: 0,
        improve_expanded: 0,
        stopped: run.stopped(),
        first_sum_of_costs: None,
        fell_back: false,
    }
}

/// How many configurations [`first_solution`] may reach in [`solve`]: as many as take
/// about 240 MB, 10 million agent positions, and at least 1000.
pub fn first_configurations(instance: &Instance<'_>) -> u32 {
    let agents = instance.agents().len().max(1) as u64;
    (10_000_000 / agents).clamp(1000, u64::from(u32::MAX)) as u32
}

/// A first solution, fast and far from the best, by LaCAM (Okumura, AAAI 2023) over PIBT
/// (Okumura et al., *Artificial Intelligence* 310, 2022): one plan per agent, up to its last
/// arrival on its goal, if one was found among at most `max_configurations` configurations
/// and 10 million constraints, about 160 MB, before `cancel`, if given, was set.
///
/// PIBT moves every agent a step at a time, each to the free neighbour nearest its goal in
/// order of priority, with agents in the way pushed aside; LaCAM searches over the
/// configurations it reaches, depth first, and tries PIBT's step under ever more constraints
/// where it gets stuck, which makes it complete. This is LaCAM's first search, without the
/// refinements of LaCAM\* and LaCAM3 after it. [`solve`] starts with it, by default, to fall
/// back on if negotiation doesn't settle.
pub fn first_solution(
    instance: &Instance<'_>,
    max_configurations: u32,
    cancel: Option<&core::sync::atomic::AtomicBool>,
) -> Option<Vec<Vec<u32>>> {
    pibt::lacam(instance, max_configurations, cancel)
}

/// [`solve_with`] by [`Method::SafeIntervals`].
fn solve_safe_intervals(
    instance: &Instance<'_>,
    options: &SolveOptions<'_>,
    run: &Run<'_>,
) -> Solution {
    let own_flag = options.engine.cancel;
    let first = if options.first {
        run.watch(own_flag, || {
            pibt::lacam(
                instance,
                first_configurations(instance),
                run.engine_flag(own_flag),
            )
        })
    } else {
        None
    };
    // Negotiation starts: a listener hears it, step 0, once the first solution is there.
    run.tell(Progress::new(Stage::Route, 0), 0);
    let mut engine_options = options.engine;
    engine_options.max_expansions = run.budget(options.engine.max_expansions);
    let (negotiated, state) = run.watch(own_flag, || {
        engine_options.cancel = run.engine_flag(own_flag);
        negotiate::negotiate(
            instance,
            &engine_options,
            &|iteration, conflicts, expanded| {
                if run.listening() {
                    let mut progress = Progress::new(Stage::Route, iteration);
                    progress.conflicts = Some(conflicts);
                    run.tell(progress, expanded);
                }
            },
        )
    });
    run.charge(negotiated.nodes_expanded);
    run.engine_stopped(negotiated.stop);
    let first_sum_of_costs = first.as_ref().map(|plans| {
        plans
            .iter()
            .map(|plan| u64::from(check::plan_cost(plan)))
            .sum()
    });
    let mut solution = Solution {
        plans: negotiated.plans,
        rounds: Vec::new(),
        negotiated_sum_of_costs: None,
        improve_passes: 0,
        improve_expanded: 0,
        stopped: None,
        first_sum_of_costs,
        fell_back: false,
    };
    solution.rounds.push(Round {
        horizon: solution.makespan().unwrap_or(0),
        stop: negotiated.stop,
        iterations: negotiated.iterations,
        nodes_expanded: negotiated.nodes_expanded,
        conflicts: negotiated.conflicts,
    });
    solution.negotiated_sum_of_costs = solution.sum_of_costs();
    let mut state = state;
    if solution.plans.is_none()
        && let Some(first) = first
    {
        let mut table = negotiate::State::new(instance);
        table.add_all(&first);
        state = Some(table);
        solution.plans = Some(first);
        solution.fell_back = true;
    }
    if let (Some(plans), Some(mut state)) = (solution.plans.as_mut(), state)
        && options.improve_passes > 0
        && run.stopped().is_none()
    {
        run.tell(Progress::new(Stage::Improve, 0), 0);
        let budget = run.budget(
            options
                .engine
                .max_expansions
                .saturating_sub(negotiated.nodes_expanded),
        );
        let shortened = run.watch(own_flag, || {
            negotiate::shorten(
                instance,
                &mut state,
                plans,
                options.improve_passes,
                budget,
                run.engine_flag(own_flag),
            )
        });
        run.charge(shortened.nodes_expanded);
        solution.improve_passes = shortened.passes;
        solution.improve_expanded = shortened.nodes_expanded;
    }
    solution.stopped = run.stopped();
    solution
}

#[cfg(test)]
mod tests;
