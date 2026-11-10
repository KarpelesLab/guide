//! The time-expanded space that the engine routes agents through.

use core::fmt;
use std::error::Error;

use crate::cost::Cost;
use crate::id::NetId;
use crate::net::{Net, Route, Terminal};
use crate::space::{InternedSlots, RoutingSpace, Step};

use super::graph::{Agent, Graph};

/// A problem: a graph and its agents, with the distances that searches are guided by.
///
/// Agent `i` is routed as the net `NetId(i)`.
#[derive(Clone, Debug)]
pub struct Instance<'g> {
    graph: &'g Graph,
    agents: Vec<Agent>,
    /// For each agent, the distance from every vertex to its goal.
    distances: Vec<Vec<u32>>,
}

/// Why an [`Instance`] could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstanceError {
    /// An agent starts or ends on a vertex that the graph doesn't have.
    VertexOutOfRange {
        /// The agent.
        agent: u32,
    },
    /// Two agents start on the same vertex.
    SharedStart {
        /// The first of the two.
        first: u32,
        /// The second.
        second: u32,
    },
    /// Two agents end on the same vertex, which they can't both stay on.
    SharedGoal {
        /// The first of the two.
        first: u32,
        /// The second.
        second: u32,
    },
    /// An agent's goal can't be reached from its start.
    Unreachable {
        /// The agent.
        agent: u32,
    },
    /// There are more agents than a `u32` can count.
    TooManyAgents,
}

impl fmt::Display for InstanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstanceError::VertexOutOfRange { agent } => {
                write!(f, "agent {agent} starts or ends outside the graph")
            }
            InstanceError::SharedStart { first, second } => {
                write!(f, "agents {first} and {second} start on the same vertex")
            }
            InstanceError::SharedGoal { first, second } => {
                write!(f, "agents {first} and {second} end on the same vertex")
            }
            InstanceError::Unreachable { agent } => {
                write!(f, "agent {agent} can't reach its goal")
            }
            InstanceError::TooManyAgents => f.write_str("too many agents"),
        }
    }
}

impl Error for InstanceError {}

impl<'g> Instance<'g> {
    /// Checks a problem and works out the distance from every vertex to each goal.
    ///
    /// That takes a breadth-first search and four bytes per vertex for each agent.
    ///
    /// # Errors
    ///
    /// Returns an error if an agent is outside the graph, two agents share a start or a goal,
    /// or an agent can't reach its goal.
    pub fn new(graph: &'g Graph, agents: Vec<Agent>) -> Result<Self, InstanceError> {
        if u32::try_from(agents.len()).is_err() {
            return Err(InstanceError::TooManyAgents);
        }
        let vertices = graph.vertex_count();
        let mut start_of = vec![u32::MAX; vertices as usize];
        let mut goal_of = vec![u32::MAX; vertices as usize];
        for (index, agent) in agents.iter().enumerate() {
            let index = index as u32;
            if agent.start >= vertices || agent.goal >= vertices {
                return Err(InstanceError::VertexOutOfRange { agent: index });
            }
            let first = start_of[agent.start as usize];
            if first != u32::MAX {
                return Err(InstanceError::SharedStart {
                    first,
                    second: index,
                });
            }
            start_of[agent.start as usize] = index;
            let first = goal_of[agent.goal as usize];
            if first != u32::MAX {
                return Err(InstanceError::SharedGoal {
                    first,
                    second: index,
                });
            }
            goal_of[agent.goal as usize] = index;
        }
        let mut distances = Vec::with_capacity(agents.len());
        for (index, agent) in agents.iter().enumerate() {
            let distance = graph.distances_to(agent.goal);
            if distance[agent.start as usize] == u32::MAX {
                return Err(InstanceError::Unreachable {
                    agent: index as u32,
                });
            }
            distances.push(distance);
        }
        Ok(Instance {
            graph,
            agents,
            distances,
        })
    }

    /// Returns the graph.
    #[inline]
    pub fn graph(&self) -> &'g Graph {
        self.graph
    }

    /// Returns the agents.
    #[inline]
    pub fn agents(&self) -> &[Agent] {
        &self.agents
    }

    /// Returns the length of agent `agent`'s shortest path, ignoring every other agent, or
    /// `None` for an agent that doesn't exist.
    pub fn shortest(&self, agent: u32) -> Option<u32> {
        let start = self.agents.get(agent as usize)?.start;
        Some(self.distances[agent as usize][start as usize])
    }

    /// Returns the sum of the agents' shortest paths, each ignoring the others: a lower bound
    /// on any solution's sum of costs.
    pub fn sum_of_shortest(&self) -> u64 {
        (0..self.agents.len() as u32)
            .filter_map(|agent| self.shortest(agent))
            .map(u64::from)
            .sum()
    }

    /// Returns the longest of the agents' shortest paths: a lower bound on any solution's
    /// makespan.
    pub fn longest_shortest(&self) -> u32 {
        (0..self.agents.len() as u32)
            .filter_map(|agent| self.shortest(agent))
            .max()
            .unwrap_or(0)
    }

    /// Returns the nets to route through a [`TimeSpace`] of this instance: for agent `i`, net
    /// `NetId(i)` from its start at time 0 to its [goal](State::Goal).
    pub fn nets(&self) -> Vec<Net<State>> {
        self.agents
            .iter()
            .enumerate()
            .map(|(index, agent)| Net {
                id: NetId(index as u32),
                terminals: vec![
                    Terminal::at(State::At {
                        vertex: agent.start,
                        time: 0,
                    }),
                    Terminal::at(State::Goal { vertex: agent.goal }),
                ],
            })
            .collect()
    }

    /// The distance from `vertex` to the goal of the agent of `net`, if both exist.
    #[inline]
    fn distance(&self, net: NetId, vertex: u32) -> Option<u32> {
        self.distances
            .get(net.0 as usize)?
            .get(vertex as usize)
            .copied()
    }

    #[inline]
    fn goal(&self, net: NetId) -> Option<u32> {
        self.agents.get(net.0 as usize).map(|agent| agent.goal)
    }
}

/// A search state of the time-expanded space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum State {
    /// An agent on `vertex` at `time`, free to go on.
    At {
        /// The vertex.
        vertex: u32,
        /// The time step, from 0.
        time: u32,
    },
    /// An agent on `vertex`, its goal, at `time`, that has arrived for good and stays there to
    /// the horizon.
    Stay {
        /// The goal.
        vertex: u32,
        /// The time step.
        time: u32,
    },
    /// The end of every route: an agent that has stayed on `vertex`, its goal, to the horizon.
    Goal {
        /// The goal.
        vertex: u32,
    },
}

/// What agents compete for: a vertex at a time, or an edge during a move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Resource {
    /// `vertex` at `time`. Two agents on it are a vertex conflict.
    Vertex {
        /// The vertex.
        vertex: u32,
        /// The time step.
        time: u32,
    },
    /// The edge between `low` and `high`, in either direction, during the move from `time` to
    /// `time + 1`. Two agents on it are a swap, or edge, conflict.
    Edge {
        /// The lower vertex of the edge.
        low: u32,
        /// The higher.
        high: u32,
        /// The time the move starts at.
        time: u32,
    },
}

/// An [`Instance`] expanded in time up to a horizon, as a [`RoutingSpace`].
///
/// Its nodes are [`State`]s and its resources are [`Resource`]s, each of capacity 1. A route
/// runs from its agent's start at time 0 through [`State::At`]s, a move or a wait at a time, to
/// its goal, and then through a [`State::Stay`] at each time to the horizon, and a last step
/// into [`State::Goal`]. Searches are guided by the exact distance to the goal, and are never
/// offered a step from which the goal can't be reached by the horizon.
///
/// # Costs
///
/// A move or a wait costs 2, a stay and the last step cost 1, so a route that arrives at time
/// `t` for good costs `2t + (horizon - t) + 1`, which is `horizon + 1 + t`: the order of routes
/// by cost is the order by arrival, as the sum of costs has it, and a run's total is the sum of
/// costs plus `horizon + 1` per agent.
///
/// The stays could be free, as they are in the sum of costs, but the engine's price for a step
/// is its base cost times its overuse, and a free step would be free to conflict with anything
/// until history built up on it: and stays are what keep other agents off a goal. One final
/// step that held the goal to the horizon by itself would cost a lookup per time step every
/// time a search priced it, at each time the search reaches the goal, where a chain of stays
/// is priced a time at a time, once.
#[derive(Clone, Copy, Debug)]
pub struct TimeSpace<'i, 'g> {
    instance: &'i Instance<'g>,
    horizon: u32,
}

/// The cost of a move or a wait.
#[inline]
fn move_cost() -> Cost {
    Cost::from(2)
}

impl<'i, 'g> TimeSpace<'i, 'g> {
    /// Expands `instance` up to time `horizon`: every agent has to be on its goal by then, and
    /// stays there.
    pub fn new(instance: &'i Instance<'g>, horizon: u32) -> Self {
        TimeSpace { instance, horizon }
    }

    /// Returns the horizon.
    #[inline]
    pub fn horizon(&self) -> u32 {
        self.horizon
    }

    /// Returns the instance.
    #[inline]
    pub fn instance(&self) -> &'i Instance<'g> {
        self.instance
    }

    /// Reads a route of this space as a plan: the vertex at each time from 0, up to the last
    /// arrival on the goal. Returns `None` for a route that isn't a single path from a start at
    /// time 0, through consecutive times, to a [`State::Goal`] on the vertex it ends on.
    pub fn plan(route: &Route<State>) -> Option<Vec<u32>> {
        let State::At { vertex, time: 0 } = route.root()? else {
            return None;
        };
        let mut plan = vec![vertex];
        let mut at = route.root()?;
        let mut staying = false;
        let mut done = false;
        for edge in route.edges() {
            if edge.from != at || done {
                return None;
            }
            match edge.to {
                State::At { vertex, time } if !staying && time as usize == plan.len() => {
                    plan.push(vertex);
                }
                State::Stay { vertex, time }
                    if time as usize == plan.len() && plan.last() == Some(&vertex) =>
                {
                    staying = true;
                    plan.push(vertex);
                }
                State::Goal { vertex } if plan.last() == Some(&vertex) => done = true,
                _ => return None,
            }
            at = edge.to;
        }
        if !done {
            return None;
        }
        // Waits on the goal at the end cost nothing in a plan.
        while plan.len() > 1 && plan[plan.len() - 2] == plan[plan.len() - 1] {
            plan.pop();
        }
        Some(plan)
    }

    /// Whether `net` at `vertex` at `time` can still reach its goal by the horizon.
    #[inline]
    fn viable(&self, net: NetId, vertex: u32, time: u32) -> bool {
        self.instance.distance(net, vertex).is_some_and(|distance| {
            u64::from(time) + u64::from(distance) <= u64::from(self.horizon)
        })
    }

    /// The step on from the goal at `time`: a stay at the next time, or the end.
    #[inline]
    fn stay(&self, vertex: u32, time: u32) -> Step<State> {
        let to = if time >= self.horizon {
            State::Goal { vertex }
        } else {
            State::Stay {
                vertex,
                time: time + 1,
            }
        };
        Step {
            to,
            cost: Cost::ONE,
        }
    }
}

impl RoutingSpace for TimeSpace<'_, '_> {
    type Node = State;
    type Resource = Resource;
    type Slots = InternedSlots<State>;
    type ResourceSlots = InternedSlots<Resource>;

    fn new_slots(&self) -> Self::Slots {
        InternedSlots::new()
    }

    fn new_resource_slots(&self) -> Self::ResourceSlots {
        InternedSlots::new()
    }

    fn contains(&self, node: State) -> bool {
        let vertices = self.instance.graph.vertex_count();
        match node {
            State::At { vertex, time } | State::Stay { vertex, time } => {
                vertex < vertices && time <= self.horizon
            }
            State::Goal { vertex } => vertex < vertices,
        }
    }

    fn expand(&self, net: NetId, from: State, out: &mut Vec<Step<State>>) {
        let (vertex, time) = match from {
            State::At { vertex, time } => (vertex, time),
            State::Stay { vertex, time } => {
                out.push(self.stay(vertex, time));
                return;
            }
            State::Goal { .. } => return,
        };
        if self.instance.goal(net) == Some(vertex) {
            out.push(self.stay(vertex, time));
        }
        if time >= self.horizon {
            return;
        }
        let next = time + 1;
        if self.viable(net, vertex, next) {
            out.push(Step {
                to: State::At { vertex, time: next },
                cost: move_cost(),
            });
        }
        for &neighbour in self.instance.graph.neighbours(vertex) {
            if self.viable(net, neighbour, next) {
                out.push(Step {
                    to: State::At {
                        vertex: neighbour,
                        time: next,
                    },
                    cost: move_cost(),
                });
            }
        }
    }

    fn lower_bound(&self, net: NetId, from: State, to: State) -> Cost {
        let State::Goal { vertex: goal } = to else {
            return Cost::ZERO;
        };
        if self.instance.goal(net) != Some(goal) {
            return Cost::ZERO;
        }
        match from {
            // `distance` moves at 2, the rest of the time in stays at 1, and the end.
            State::At { vertex, time } => match self.instance.distance(net, vertex) {
                Some(distance) if self.viable(net, vertex, time) => {
                    Cost::from(self.horizon - time) + Cost::from(distance) + Cost::ONE
                }
                _ => Cost::MAX,
            },
            State::Stay { vertex, time } if vertex == goal && time <= self.horizon => {
                Cost::from(self.horizon - time) + Cost::ONE
            }
            State::Stay { .. } => Cost::MAX,
            State::Goal { .. } => Cost::ZERO,
        }
    }

    fn resources(&self, net: NetId, from: State, to: State, out: &mut Vec<Resource>) {
        let _ = net;
        let (a, time) = match from {
            State::At { vertex, time } => (vertex, time),
            State::Stay { vertex, time } => {
                if let State::Stay {
                    vertex: b,
                    time: next,
                } = to
                    && b == vertex
                    && u64::from(next) == u64::from(time) + 1
                {
                    out.push(Resource::Vertex { vertex, time: next });
                }
                return;
            }
            State::Goal { .. } => return,
        };
        match to {
            // The start of a route.
            State::At { .. } if from == to => out.push(Resource::Vertex { vertex: a, time }),
            State::At {
                vertex: b,
                time: next,
            } if u64::from(next) == u64::from(time) + 1 => {
                out.push(Resource::Vertex {
                    vertex: b,
                    time: next,
                });
                if a != b {
                    out.push(Resource::Edge {
                        low: a.min(b),
                        high: a.max(b),
                        time,
                    });
                }
            }
            State::Stay {
                vertex: b,
                time: next,
            } if b == a && u64::from(next) == u64::from(time) + 1 => {
                out.push(Resource::Vertex {
                    vertex: b,
                    time: next,
                });
            }
            _ => {}
        }
    }

    fn capacity(&self, resource: Resource) -> u32 {
        let _ = resource;
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: u32) -> Graph {
        let edges: Vec<_> = (1..n).map(|v| (v - 1, v)).collect();
        Graph::from_edges(n, &edges).unwrap()
    }

    #[test]
    fn instances_reject_what_cannot_be_solved() {
        let graph = Graph::from_edges(4, &[(0, 1), (1, 2)]).unwrap();
        let agent = |start, goal| Agent { start, goal };
        assert_eq!(
            Instance::new(&graph, vec![agent(0, 9)]).unwrap_err(),
            InstanceError::VertexOutOfRange { agent: 0 }
        );
        assert_eq!(
            Instance::new(&graph, vec![agent(0, 1), agent(0, 2)]).unwrap_err(),
            InstanceError::SharedStart {
                first: 0,
                second: 1
            }
        );
        assert_eq!(
            Instance::new(&graph, vec![agent(0, 1), agent(2, 1)]).unwrap_err(),
            InstanceError::SharedGoal {
                first: 0,
                second: 1
            }
        );
        assert_eq!(
            Instance::new(&graph, vec![agent(0, 1), agent(3, 2)]).unwrap_err(),
            InstanceError::Unreachable { agent: 1 }
        );
        let instance = Instance::new(&graph, vec![agent(0, 2), agent(1, 1)]).unwrap();
        assert_eq!(instance.shortest(0), Some(2));
        assert_eq!(instance.shortest(2), None);
        assert_eq!(instance.sum_of_shortest(), 2);
        assert_eq!(instance.longest_shortest(), 2);
    }

    #[test]
    fn steps_wait_move_stay_and_end_within_the_horizon() {
        let graph = line(4);
        let instance = Instance::new(&graph, vec![Agent { start: 1, goal: 2 }]).unwrap();
        let space = TimeSpace::new(&instance, 3);
        let at = |vertex, time| State::At { vertex, time };
        let stay = |vertex, time| State::Stay { vertex, time };
        let steps = |from| {
            let mut out = Vec::new();
            space.expand(NetId(0), from, &mut out);
            out.into_iter()
                .map(|step| (step.to, step.cost.get()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            steps(at(1, 0)),
            [(at(1, 1), 2.0), (at(0, 1), 2.0), (at(2, 1), 2.0)]
        );
        // From vertex 0 at time 3 the goal would be two steps away, past the horizon.
        assert_eq!(steps(at(1, 2)), [(at(2, 3), 2.0)]);
        // On the goal, it can stay for good or wait, but a neighbour at the horizon is too far.
        assert_eq!(steps(at(2, 2)), [(stay(2, 3), 1.0), (at(2, 3), 2.0)]);
        assert_eq!(
            steps(at(2, 1)),
            [
                (stay(2, 2), 1.0),
                (at(2, 2), 2.0),
                (at(1, 2), 2.0),
                (at(3, 2), 2.0)
            ]
        );
        assert_eq!(steps(stay(2, 2)), [(stay(2, 3), 1.0)]);
        assert_eq!(steps(stay(2, 3)), [(State::Goal { vertex: 2 }, 1.0)]);
        assert_eq!(steps(at(2, 3)), [(State::Goal { vertex: 2 }, 1.0)]);
        assert_eq!(steps(State::Goal { vertex: 2 }), []);
        assert!(space.contains(at(3, 3)));
        assert!(space.contains(stay(3, 3)));
        assert!(!space.contains(at(3, 4)));
        assert!(!space.contains(stay(3, 4)));
        assert!(!space.contains(at(4, 0)));
        // Two moves at 2, a stay and the end: arriving at 2, a route costs 3 + 1 + 2.
        let goal = State::Goal { vertex: 2 };
        assert_eq!(space.lower_bound(NetId(0), at(0, 0), goal), Cost::from(6));
        assert_eq!(space.lower_bound(NetId(0), at(0, 2), goal), Cost::MAX);
        assert_eq!(space.lower_bound(NetId(0), stay(2, 1), goal), Cost::from(3));
        assert_eq!(space.lower_bound(NetId(0), stay(1, 1), goal), Cost::MAX);
        assert_eq!(space.lower_bound(NetId(0), at(0, 0), at(1, 1)), Cost::ZERO);
    }

    #[test]
    fn resources_cover_vertices_edges_both_ways_and_stays() {
        let graph = line(3);
        let instance = Instance::new(&graph, vec![Agent { start: 0, goal: 2 }]).unwrap();
        let space = TimeSpace::new(&instance, 4);
        let held = |from, to| {
            let mut out = Vec::new();
            space.resources(NetId(0), from, to, &mut out);
            out
        };
        let at = |vertex, time| State::At { vertex, time };
        let stay = |vertex, time| State::Stay { vertex, time };
        let vertex = |vertex, time| Resource::Vertex { vertex, time };
        assert_eq!(held(at(0, 0), at(0, 0)), [vertex(0, 0)]);
        let edge = Resource::Edge {
            low: 0,
            high: 1,
            time: 2,
        };
        assert_eq!(held(at(1, 2), at(0, 3)), [vertex(0, 3), edge]);
        assert_eq!(held(at(0, 2), at(1, 3)), [vertex(1, 3), edge]);
        assert_eq!(held(at(1, 2), at(1, 3)), [vertex(1, 3)]);
        assert_eq!(held(at(2, 2), stay(2, 3)), [vertex(2, 3)]);
        assert_eq!(held(stay(2, 3), stay(2, 4)), [vertex(2, 4)]);
        assert_eq!(held(stay(2, 4), State::Goal { vertex: 2 }), []);
        assert_eq!(held(at(1, 2), stay(2, 3)), []);
        assert_eq!(held(stay(2, 2), stay(2, 4)), []);
        assert_eq!(held(at(1, 2), at(1, 4)), []);
        assert_eq!(held(State::Goal { vertex: 2 }, at(1, 4)), []);
    }

    #[test]
    fn plans_are_read_from_paths_without_the_waits_at_the_end() {
        let at = |vertex, time| State::At { vertex, time };
        let stay = |vertex, time| State::Stay { vertex, time };
        let plan = |path: &[State]| {
            let mut route = Route::new();
            route.add_path(path).unwrap();
            TimeSpace::plan(&route)
        };
        let goal = State::Goal { vertex: 2 };
        assert_eq!(
            plan(&[at(0, 0), at(1, 1), at(1, 2), at(2, 3), stay(2, 4), goal]),
            Some(vec![0, 1, 1, 2])
        );
        assert_eq!(
            plan(&[at(0, 0), at(1, 1), at(2, 2), at(2, 3), stay(2, 4), goal]),
            Some(vec![0, 1, 2])
        );
        assert_eq!(plan(&[at(0, 0), at(1, 1)]), None);
        assert_eq!(plan(&[at(0, 1), State::Goal { vertex: 0 }]), None);
        assert_eq!(plan(&[at(2, 0), stay(2, 2), goal]), None);
        assert_eq!(plan(&[at(1, 0), stay(1, 1), goal]), None);
        assert_eq!(plan(&[at(2, 0), stay(2, 1), at(2, 2), goal]), None);
        assert_eq!(
            plan(&[at(5, 0), stay(5, 1), State::Goal { vertex: 5 }]),
            Some(vec![5])
        );
    }
}
