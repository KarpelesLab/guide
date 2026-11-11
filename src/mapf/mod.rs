//! Multi-agent path finding, by the negotiated-congestion engine.
//!
//! Agents start on vertices of a graph and have to reach their goals, one step or one wait per
//! time step, without two of them ever being on the same vertex at once or swapping places
//! along an edge, and stay on their goals once there. That is the classic problem of Stern et
//! al., *Multi-Agent Pathfinding: Definitions, Variants, and Benchmarks* (SoCS 2019), with
//! vertex and swap conflicts, agents that stay at their targets, and the sum of costs as the
//! objective.
//!
//! Nothing here is a new algorithm. [`TimeSpace`] expands an [`Instance`] in time, as a
//! [`RoutingSpace`](crate::space::RoutingSpace) whose nodes are a vertex at a time and whose
//! resources are a vertex at a time and an edge during a move, each of capacity 1, and
//! [`solve`] hands it to [`crate::engine`], which negotiates the conflicts away as PathFinder
//! does on an FPGA. Agents are nets, and each net has two terminals, its start at time 0 and
//! its goal. This is what MAPF-LNS2 (Li et al., AAAI 2022) calls soft conflicts: an agent may
//! collide at a price, and the price rises until nobody does. [`solve`] has an example.
//!
//! # Staying on the goal
//!
//! An agent that arrives on its goal for good at time `t` goes on from there through a
//! [`State::Stay`] at each time to the horizon, which holds the goal at that time, and then into
//! [`State::Goal`], the target of every route. Another agent passing through later conflicts
//! with a stay, so the engine sees the rule like any other. Moves and waits cost 2 and stays 1,
//! so that a route's cost is the horizon plus 1 plus its arrival time: [`TimeSpace`] has why.
//!
//! The first version held the goal to the horizon in one step from the goal into
//! [`State::Goal`]. That step had to be priced over every time to the horizon each time a search
//! reached the goal, at each time it did, and on den520d, with a horizon of about 420, pricing
//! it was most of the time. Chains of stays took a run of 600 agents there from 10.4 s to
//! 4.7 s, and gave a lower sum of costs on every map measured (`docs/benchmarks.md`).
//!
//! # The horizon
//!
//! Time has to stop somewhere, and the horizon is a trade: too short and no solution fits, too
//! long and every final step holds more resources. [`solve`] starts from the longest of the
//! agents' shortest paths plus [`slack`](SolveOptions::slack), and, if negotiation doesn't
//! settle within the engine's iterations, lengthens it and runs again from the routes and
//! history it ended with (iterative deepening). Searches are never offered a state from which
//! the goal can't be reached by the horizon, so a horizon costs nothing where no route goes.
//!
//! # Shortening
//!
//! Negotiation stops at the first legal solution, found at prices that no longer hold. With
//! [`improve_passes`](SolveOptions::improve_passes), as by default, [`solve`] then plans each agent again in
//! turn, by an exact search against the others as hard reservations, and keeps what arrives
//! sooner, until a pass changes nothing. No agent gets worse, and the solution stays legal.
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
mod space;

pub use graph::{Agent, Graph, GraphError};
pub use space::{Instance, InstanceError, Resource, State, TimeSpace};

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
}

impl Default for SolveOptions<'_> {
    fn default() -> Self {
        SolveOptions {
            engine: engine_options(),
            slack: 16,
            rounds: 4,
            threads: 1,
            improve_passes: 10,
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

/// Solves `instance` with the negotiated-congestion engine. See the [module
/// documentation](self).
///
/// The result is the same for any number of threads and on any platform.
///
/// # Examples
///
/// Two agents swap ends of a corridor with a siding. At best, one steps into the siding to let
/// the other by, which takes it 4 steps, and the other waits a step, which takes it 3.
/// Negotiation is not exact, and here both wait a step more than they need to.
///
/// ```
/// use guide::mapf::{Agent, Graph, Instance, SolveOptions, check, solve};
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
/// assert_eq!(summary.sum_of_costs, 9);
/// assert_eq!(solution.sum_of_costs(), Some(9));
/// assert!(summary.sum_of_costs >= instance.sum_of_shortest());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn solve(instance: &Instance<'_>, options: &SolveOptions<'_>) -> Solution {
    let nets = instance.nets();
    let mut horizon = instance.longest_shortest().saturating_add(options.slack);
    let mut rounds = Vec::new();
    let mut seeds = Vec::new();
    let mut history = Vec::new();
    let mut budget = options.engine.max_expansions;
    for round in 0..options.rounds.max(1) {
        let space = TimeSpace::new(instance, horizon);
        let mut engine_options = options.engine;
        engine_options.max_expansions = budget;
        let Ok(mut engine) = Engine::new(&space, &nets, engine_options) else {
            // Nets are numbered by agent, so they can't share an ID.
            unreachable!("agents have distinct nets");
        };
        if round > 0 {
            // Both were taken from an engine on the same nets, so neither can fail.
            let _ = engine.seed_routes(seeds.drain(..));
            let _ = engine.seed_history(history.drain(..));
        }
        while engine.step_parallel(options.threads).is_some() {}
        let outcome = engine.outcome();
        let stats = outcome.routes.stats;
        budget = budget.saturating_sub(stats.nodes_expanded);
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
            };
            solution.negotiated_sum_of_costs = solution.sum_of_costs();
            if let Some(plans) = solution.plans.as_mut()
                && options.improve_passes > 0
            {
                let improved = improve::improve(
                    &space,
                    plans,
                    options.improve_passes,
                    budget,
                    options.engine.cancel,
                );
                solution.improve_passes = improved.passes;
                solution.improve_expanded = improved.nodes_expanded;
            }
            return solution;
        }
        if matches!(
            outcome.stop,
            StopReason::Cancelled | StopReason::BudgetExhausted
        ) {
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
    }
}

#[cfg(test)]
mod tests;
