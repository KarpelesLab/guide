//! Negotiated congestion over safe intervals.
//!
//! This is the engine's negotiation, with its schedule and its prices, run over
//! [`sipp`](super::sipp) rather than through [`crate::engine::Engine`]: every agent is planned
//! in turn against the others at a present factor, the factor grows each iteration, history
//! builds up on what stays overused, and from the second iteration on only agents in conflict
//! when their turn comes are planned again. What differs is the table: a safe-interval search
//! has to ask when a vertex is next free, or next changes price, which the engine's table of
//! resources, a hash map from a resource to its occupancy, can't answer without a lookup per
//! time step. So the table here is kept per vertex, in time order.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::engine::{EngineOptions, StopReason};

use super::check::plan_cost;
use super::sipp::{Mode, Planned, Planner, Table};
use super::space::Instance;

/// What [`negotiate`] did.
#[derive(Clone, Debug)]
pub(super) struct Negotiated {
    /// The plans, if they are free of conflicts.
    pub plans: Option<Vec<Vec<u32>>>,
    pub stop: StopReason,
    pub iterations: u64,
    pub nodes_expanded: u64,
    /// How many vertex-times and edge-times the last plans overuse.
    pub conflicts: usize,
}

/// The planner and table that [`negotiate`] ends with, for [`shorten`].
pub(super) struct State {
    table: Table,
    planner: Planner,
}

impl State {
    /// An empty table for `instance`.
    pub(super) fn new(instance: &Instance<'_>) -> Self {
        let vertices = instance.graph().vertex_count();
        State {
            table: Table::new(vertices),
            planner: Planner::new(vertices),
        }
    }

    /// Adds every agent's plan.
    pub(super) fn add_all(&mut self, plans: &[Vec<u32>]) {
        for (agent, plan) in plans.iter().enumerate() {
            self.table.add(agent as u32, plan);
        }
    }
}

/// Negotiates plans for every agent of `instance`, as the [module documentation](self) says,
/// with the schedule, history, limits and cancel flag of `options`, and tells `tell` of each
/// iteration, its overused vertex-times and edge-times, and the nodes expanded so far.
///
/// History is in units of a move, which costs 1 here and 2 in a
/// [`TimeSpace`](super::TimeSpace), so it is taken at half of `options.history_factor`, to mean
/// the same.
pub(super) fn negotiate(
    instance: &Instance<'_>,
    options: &EngineOptions<'_>,
    tell: &dyn Fn(u64, u64, u64),
) -> (Negotiated, Option<State>) {
    let graph = instance.graph();
    let agents = instance.agents();
    let mut table = Table::new(graph.vertex_count());
    let mut planner = Planner::new(graph.vertex_count());
    let mut plans: Vec<Vec<u32>> = vec![Vec::new(); agents.len()];
    let history = options.history_factor.get() * 0.5;
    let mut present = options.first_present.get();
    let mut overuse = Vec::new();
    let mut iterations = 0u64;
    let cancel = options.cancel;
    loop {
        for (index, agent) in agents.iter().enumerate() {
            let id = index as u32;
            if iterations > 0 {
                if !table.in_conflict(id, &plans[index]) {
                    continue;
                }
                table.remove(id, &plans[index]);
            }
            let planned = planner.plan(
                graph,
                &table,
                instance.distances(index),
                agent.start,
                agent.goal,
                Mode::Soft { present },
                options.max_expansions.saturating_sub(planner.expanded),
                cancel,
            );
            let Planned::Found(plan) = planned else {
                // Nothing is forbidden, so only a stop gets here.
                let stop = if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                    StopReason::Cancelled
                } else {
                    StopReason::BudgetExhausted
                };
                let negotiated = Negotiated {
                    plans: None,
                    stop,
                    iterations: iterations + 1,
                    nodes_expanded: planner.expanded,
                    conflicts: 0,
                };
                return (negotiated, None);
            };
            plans[index] = plan;
            table.add(id, &plans[index]);
        }
        iterations += 1;
        overuse.clear();
        table.conflicts(&mut overuse);
        let conflicts: u64 = overuse.iter().map(|o| o.resources()).sum();
        tell(iterations, conflicts, planner.expanded);
        if overuse.is_empty() {
            let negotiated = Negotiated {
                plans: Some(plans),
                stop: StopReason::Converged,
                iterations,
                nodes_expanded: planner.expanded,
                conflicts: 0,
            };
            return (negotiated, Some(State { table, planner }));
        }
        if iterations >= u64::from(options.max_iterations.max(1)) {
            let negotiated = Negotiated {
                plans: None,
                stop: StopReason::IterationLimit,
                iterations,
                nodes_expanded: planner.expanded,
                conflicts: conflicts as usize,
            };
            return (negotiated, None);
        }
        // Until the present factor is at its most, history goes where the overuse is; then,
        // conflicts on a goal that slide later each iteration take it from the arrival on.
        let stuck = present >= options.max_present.get();
        for &o in &overuse {
            table.add_history(o, history, stuck);
        }
        present = if iterations == 1 {
            options.initial_present.get()
        } else {
            (present * options.present_growth.get()).min(options.max_present.get())
        };
    }
}

/// What [`shorten`] did.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Shortened {
    pub passes: u32,
    pub nodes_expanded: u64,
}

/// Plans each agent again in turn against the others, as hard reservations, and keeps what
/// arrives sooner, until a pass changes nothing, as [shortening](super#shortening) has it.
pub(super) fn shorten(
    instance: &Instance<'_>,
    state: &mut State,
    plans: &mut [Vec<u32>],
    max_passes: u32,
    budget: u64,
    cancel: Option<&AtomicBool>,
) -> Shortened {
    let graph = instance.graph();
    let agents = instance.agents();
    let before = state.planner.expanded;
    let mut shortened = Shortened::default();
    while shortened.passes < max_passes {
        shortened.passes += 1;
        let mut changed = false;
        for (index, agent) in agents.iter().enumerate() {
            let id = index as u32;
            state.table.remove(id, &plans[index]);
            let spent = state.planner.expanded - before;
            let planned = state.planner.plan(
                graph,
                &state.table,
                instance.distances(index),
                agent.start,
                agent.goal,
                Mode::Hard,
                budget.saturating_sub(spent),
                cancel,
            );
            let stop = matches!(planned, Planned::Stopped);
            if let Planned::Found(plan) = planned
                && plan_cost(&plan) < plan_cost(&plans[index])
            {
                plans[index] = plan;
                changed = true;
            }
            state.table.add(id, &plans[index]);
            if stop {
                shortened.nodes_expanded = state.planner.expanded - before;
                return shortened;
            }
        }
        if !changed {
            break;
        }
    }
    shortened.nodes_expanded = state.planner.expanded - before;
    shortened
}
