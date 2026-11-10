//! Shortening a legal solution, one agent at a time.
//!
//! Negotiation stops at the first legal solution, whose routes were found at prices that no
//! longer hold: an agent may have gone around a conflict that later moved away, and history
//! keeps it off the short way still. So each agent in turn is planned again, with an exact
//! search through the time-expanded space, against the others as they are, as hard
//! reservations, and keeps the new plan only if it arrives sooner. The old plan is always still
//! open to it, so no agent ever gets worse, and the solution stays legal. Passes go on until one
//! changes nothing, or the passes or the budget run out.
//!
//! This is the neighbourhood of one agent of MAPF-LNS (Li et al., IJCAI 2021), in a fixed
//! order, which makes it deterministic.

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::cost::Cost;
use crate::id::NetId;
use crate::net::Route;
use crate::search::{Search, SearchLimits, SearchOutcome, Source, Target};
use crate::space::{FastMap, InternedSlots, RoutingSpace, Step};

use super::check::plan_cost;
use super::space::{Resource, State, TimeSpace};

/// A [`TimeSpace`] without the steps that take what other agents hold.
struct Reserved<'a, 'i, 'g> {
    space: &'a TimeSpace<'i, 'g>,
    /// How many agents hold each resource. Only ever looked up, never iterated.
    taken: &'a FastMap<Resource, u32>,
    buffer: &'a RefCell<Vec<Resource>>,
}

impl RoutingSpace for Reserved<'_, '_, '_> {
    type Node = State;
    type Resource = Resource;
    type Slots = InternedSlots<State>;
    type ResourceSlots = InternedSlots<Resource>;

    fn new_slots(&self) -> Self::Slots {
        self.space.new_slots()
    }

    fn new_resource_slots(&self) -> Self::ResourceSlots {
        self.space.new_resource_slots()
    }

    fn contains(&self, node: State) -> bool {
        self.space.contains(node)
    }

    fn expand(&self, net: NetId, from: State, out: &mut Vec<Step<State>>) {
        let start = out.len();
        self.space.expand(net, from, out);
        let mut buffer = self.buffer.borrow_mut();
        let mut kept = start;
        for index in start..out.len() {
            let step = out[index];
            buffer.clear();
            self.space.resources(net, from, step.to, &mut buffer);
            if buffer
                .iter()
                .all(|resource| !self.taken.contains_key(resource))
            {
                out[kept] = step;
                kept += 1;
            }
        }
        out.truncate(kept);
    }

    fn lower_bound(&self, net: NetId, from: State, to: State) -> Cost {
        self.space.lower_bound(net, from, to)
    }

    fn resources(&self, net: NetId, from: State, to: State, out: &mut Vec<Resource>) {
        self.space.resources(net, from, to, out);
    }

    fn capacity(&self, resource: Resource) -> u32 {
        self.space.capacity(resource)
    }
}

/// Appends the resources that `plan` holds in `space`, stays to the horizon included.
fn plan_resources(space: &TimeSpace<'_, '_>, plan: &[u32], out: &mut Vec<Resource>) {
    for (time, pair) in plan.windows(2).enumerate() {
        let time = time as u32;
        let (a, b) = (pair[0], pair[1]);
        if a != b {
            out.push(Resource::Edge {
                low: a.min(b),
                high: a.max(b),
                time,
            });
        }
    }
    for (time, &vertex) in plan.iter().enumerate() {
        out.push(Resource::Vertex {
            vertex,
            time: time as u32,
        });
    }
    if let Some(&goal) = plan.last() {
        out.extend(
            (plan.len() as u32..=space.horizon())
                .map(|time| Resource::Vertex { vertex: goal, time }),
        );
    }
}

/// What [`improve`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Improved {
    pub passes: u32,
    pub nodes_expanded: u64,
}

/// Shortens `plans`, a legal solution in `space`, as the [module documentation](self) says.
pub(super) fn improve(
    space: &TimeSpace<'_, '_>,
    plans: &mut [Vec<u32>],
    max_passes: u32,
    budget: u64,
    cancel: Option<&AtomicBool>,
) -> Improved {
    let mut taken: FastMap<Resource, u32> = FastMap::default();
    let mut held = Vec::new();
    for plan in plans.iter() {
        held.clear();
        plan_resources(space, plan, &mut held);
        for &resource in &held {
            *taken.entry(resource).or_insert(0) += 1;
        }
    }
    let agents = space.instance().agents().to_vec();
    let mut search = Search::new(space);
    let buffer = RefCell::new(Vec::new());
    let mut improved = Improved::default();
    while improved.passes < max_passes {
        improved.passes += 1;
        let mut changed = false;
        for (index, agent) in agents.iter().enumerate() {
            held.clear();
            plan_resources(space, &plans[index], &mut held);
            for resource in &held {
                if let Some(count) = taken.get_mut(resource) {
                    *count -= 1;
                    if *count == 0 {
                        taken.remove(resource);
                    }
                }
            }
            let reserved = Reserved {
                space,
                taken: &taken,
                buffer: &buffer,
            };
            let limits = SearchLimits {
                max_expansions: budget.saturating_sub(improved.nodes_expanded),
                cancel,
            };
            let outcome = search.find_path_with(
                &reserved,
                NetId(index as u32),
                [Source::new(State::At {
                    vertex: agent.start,
                    time: 0,
                })],
                [Target::new(State::Goal { vertex: agent.goal })],
                limits,
            );
            let found = match outcome {
                SearchOutcome::Found(found) => {
                    let mut route = Route::new();
                    route
                        .add_path(found.path)
                        .ok()
                        .and_then(|_| TimeSpace::plan(&route))
                }
                _ => None,
            };
            improved.nodes_expanded += search.stats().expanded;
            if let Some(plan) = found
                && plan_cost(&plan) < plan_cost(&plans[index])
            {
                plans[index] = plan;
                changed = true;
            }
            held.clear();
            plan_resources(space, &plans[index], &mut held);
            for &resource in &held {
                *taken.entry(resource).or_insert(0) += 1;
            }
            let cancelled = cancel.is_some_and(|flag| flag.load(Ordering::Relaxed));
            if cancelled || improved.nodes_expanded >= budget {
                return improved;
            }
        }
        if !changed {
            break;
        }
    }
    improved
}
