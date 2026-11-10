//! An independent checker for solutions to multi-agent path finding.
//!
//! This knows nothing of the time-expanded space or the engine: it takes the graph, the
//! agents, and a plan per agent as the vertex it is on at each time from 0, and checks the
//! plans by the usual rules (Stern et al., 2019):
//!
//! - each plan starts on its agent's start and ends on its goal;
//! - each step waits, or moves along an edge of the graph;
//! - no two agents are on the same vertex at the same time (a vertex conflict), counting an
//!   agent as staying on its goal for ever once its plan ends;
//! - no two agents cross the same edge in opposite directions at the same time (a swap
//!   conflict).
//!
//! The cost of a plan is the time of its agent's last arrival on its goal, so waits at the end
//! of a plan, on the goal, cost nothing.

use core::fmt;

use super::graph::{Agent, Graph};

/// A fault in a solution, as [`check`] finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Fault {
    /// There isn't one plan per agent.
    PlanCount {
        /// How many agents there are.
        agents: usize,
        /// How many plans there are.
        plans: usize,
    },
    /// A plan is empty, or doesn't start on its agent's start.
    WrongStart {
        /// The agent.
        agent: u32,
    },
    /// A plan doesn't end on its agent's goal.
    WrongGoal {
        /// The agent.
        agent: u32,
    },
    /// A plan jumps between vertices that are not joined by an edge, or leaves the graph.
    BadStep {
        /// The agent.
        agent: u32,
        /// The time the step starts at.
        time: u32,
    },
    /// Two agents are on the same vertex at the same time.
    VertexConflict {
        /// The lower-numbered agent.
        first: u32,
        /// The other agent.
        second: u32,
        /// The vertex.
        vertex: u32,
        /// The time.
        time: u32,
    },
    /// Two agents swap places along an edge.
    SwapConflict {
        /// The lower-numbered agent.
        first: u32,
        /// The other agent.
        second: u32,
        /// The time the swap starts at.
        time: u32,
    },
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Fault::PlanCount { agents, plans } => {
                write!(f, "{plans} plans for {agents} agents")
            }
            Fault::WrongStart { agent } => write!(f, "agent {agent} doesn't start on its start"),
            Fault::WrongGoal { agent } => write!(f, "agent {agent} doesn't end on its goal"),
            Fault::BadStep { agent, time } => {
                write!(f, "agent {agent} makes an impossible step at time {time}")
            }
            Fault::VertexConflict {
                first,
                second,
                vertex,
                time,
            } => write!(
                f,
                "agents {first} and {second} are both on vertex {vertex} at time {time}"
            ),
            Fault::SwapConflict {
                first,
                second,
                time,
            } => write!(f, "agents {first} and {second} swap places at time {time}"),
        }
    }
}

impl std::error::Error for Fault {}

/// The measures of a solution that [`check`] found legal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Summary {
    /// The sum over agents of the time of their last arrival on their goal.
    pub sum_of_costs: u64,
    /// The latest of those times.
    pub makespan: u32,
}

/// Returns the cost of a plan: the time of its last arrival on its last vertex.
pub fn plan_cost(plan: &[u32]) -> u32 {
    let Some(&last) = plan.last() else {
        return 0;
    };
    let arrival = plan
        .iter()
        .rposition(|&vertex| vertex != last)
        .map_or(0, |index| index + 1);
    u32::try_from(arrival).unwrap_or(u32::MAX)
}

/// Checks `plans`, one per agent in the order of `agents`, and returns their sum of costs and
/// makespan, or every fault found, in the order of time and then of agents.
///
/// # Errors
///
/// Returns the faults, if there are any.
pub fn check(graph: &Graph, agents: &[Agent], plans: &[Vec<u32>]) -> Result<Summary, Vec<Fault>> {
    if agents.len() != plans.len() {
        return Err(vec![Fault::PlanCount {
            agents: agents.len(),
            plans: plans.len(),
        }]);
    }
    let mut faults = Vec::new();
    let vertices = graph.vertex_count();
    for (index, (agent, plan)) in agents.iter().zip(plans).enumerate() {
        let index = index as u32;
        if plan.first() != Some(&agent.start) {
            faults.push(Fault::WrongStart { agent: index });
        }
        if plan.last() != Some(&agent.goal) {
            faults.push(Fault::WrongGoal { agent: index });
        }
        for (time, pair) in plan.windows(2).enumerate() {
            let (from, to) = (pair[0], pair[1]);
            let legal = from < vertices && (from == to || graph.adjacent(from, to));
            if !legal {
                faults.push(Fault::BadStep {
                    agent: index,
                    time: time as u32,
                });
            }
        }
        if plan.len() == 1 && plan[0] >= vertices {
            faults.push(Fault::BadStep {
                agent: index,
                time: 0,
            });
        }
    }
    if !faults.is_empty() {
        return Err(faults);
    }

    // Every plan is now non-empty and on the graph. An agent past the end of its plan is on its
    // goal, so conflicts can only happen up to the end of the longest plan.
    let end = plans.iter().map(Vec::len).max().unwrap_or(0);
    let at = |plan: &Vec<u32>, time: usize| plan[time.min(plan.len() - 1)];
    // Which agent is on each vertex at the time being checked, stamped with the time so that
    // nothing needs clearing.
    let mut occupant = vec![(usize::MAX, 0u32); vertices as usize];
    for time in 0..end {
        for (index, plan) in plans.iter().enumerate() {
            let vertex = at(plan, time);
            let (stamp, other) = occupant[vertex as usize];
            if stamp == time {
                // Agents are visited in order, so `other` is the lower-numbered.
                faults.push(Fault::VertexConflict {
                    first: other,
                    second: index as u32,
                    vertex,
                    time: time as u32,
                });
            } else {
                occupant[vertex as usize] = (time, index as u32);
            }
        }
        if time + 1 == end {
            break;
        }
        // A swap: this agent moves from `a` to `b`, and whoever was on `b` moves to `a`.
        for (index, plan) in plans.iter().enumerate() {
            let (a, b) = (at(plan, time), at(plan, time + 1));
            if a == b {
                continue;
            }
            let (stamp, other) = occupant[b as usize];
            let other = other as usize;
            if stamp == time && other > index && at(&plans[other], time + 1) == a {
                faults.push(Fault::SwapConflict {
                    first: index as u32,
                    second: other as u32,
                    time: time as u32,
                });
            }
        }
    }
    if !faults.is_empty() {
        return Err(faults);
    }
    let costs = plans.iter().map(|plan| plan_cost(plan));
    Ok(Summary {
        sum_of_costs: costs.clone().map(u64::from).sum(),
        makespan: costs.max().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3 x 3 grid with every cell open.
    fn grid() -> Graph {
        Graph::from_grid(3, 3, &[true; 9]).unwrap()
    }

    #[test]
    fn a_legal_solution_has_its_costs() {
        let graph = grid();
        let agents = [Agent { start: 0, goal: 2 }, Agent { start: 6, goal: 6 }];
        let plans = [vec![0, 1, 1, 2, 2], vec![6]];
        assert_eq!(
            check(&graph, &agents, &plans),
            Ok(Summary {
                sum_of_costs: 3,
                makespan: 3
            })
        );
        assert_eq!(plan_cost(&[4, 4, 4]), 0);
        assert_eq!(plan_cost(&[4, 5, 4]), 2);
        assert_eq!(plan_cost(&[]), 0);
    }

    #[test]
    fn plans_have_to_start_end_and_step_right() {
        let graph = grid();
        let agents = [Agent { start: 0, goal: 2 }];
        assert_eq!(
            check(&graph, &agents, &[]),
            Err(vec![Fault::PlanCount {
                agents: 1,
                plans: 0
            }])
        );
        assert_eq!(
            check(&graph, &agents, &[vec![1, 2]]),
            Err(vec![Fault::WrongStart { agent: 0 }])
        );
        assert_eq!(
            check(&graph, &agents, &[vec![0, 1]]),
            Err(vec![Fault::WrongGoal { agent: 0 }])
        );
        assert_eq!(
            check(&graph, &agents, &[vec![0, 4, 2]]),
            Err(vec![
                Fault::BadStep { agent: 0, time: 0 },
                Fault::BadStep { agent: 0, time: 1 }
            ])
        );
        assert_eq!(
            check(&graph, &agents, &[vec![]]),
            Err(vec![
                Fault::WrongStart { agent: 0 },
                Fault::WrongGoal { agent: 0 }
            ])
        );
        let agents = [Agent {
            start: 99,
            goal: 99,
        }];
        assert_eq!(
            check(&graph, &agents, &[vec![99]]),
            Err(vec![Fault::BadStep { agent: 0, time: 0 }])
        );
    }

    #[test]
    fn conflicts_are_found_including_with_agents_that_have_arrived() {
        let graph = grid();
        // Two agents meet on 1 at time 1.
        let agents = [Agent { start: 0, goal: 2 }, Agent { start: 4, goal: 7 }];
        assert_eq!(
            check(&graph, &agents, &[vec![0, 1, 2], vec![4, 1, 4, 7]]),
            Err(vec![Fault::VertexConflict {
                first: 0,
                second: 1,
                vertex: 1,
                time: 1
            }])
        );
        // Agent 1 sits on its goal, 1, from time 0, and agent 0 passes through later.
        let agents = [Agent { start: 0, goal: 2 }, Agent { start: 1, goal: 1 }];
        assert_eq!(
            check(&graph, &agents, &[vec![0, 0, 1, 2], vec![1]]),
            Err(vec![Fault::VertexConflict {
                first: 0,
                second: 1,
                vertex: 1,
                time: 2
            }])
        );
        // A swap along the edge between 0 and 1.
        let agents = [Agent { start: 0, goal: 1 }, Agent { start: 1, goal: 0 }];
        assert_eq!(
            check(&graph, &agents, &[vec![0, 1], vec![1, 0]]),
            Err(vec![Fault::SwapConflict {
                first: 0,
                second: 1,
                time: 0
            }])
        );
        // Following another agent into the vertex it leaves is allowed.
        let agents = [Agent { start: 0, goal: 1 }, Agent { start: 1, goal: 2 }];
        assert!(check(&graph, &agents, &[vec![0, 1], vec![1, 2]]).is_ok());
    }
}
