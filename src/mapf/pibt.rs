//! A first solution, fast and far from the best: LaCAM over PIBT.
//!
//! PIBT (Okumura, Machida, Défago and Tamura, *Artificial Intelligence* 310, 2022) moves every
//! agent one step at a time. At each step, agents choose in order of priority, each the free
//! neighbour or its own vertex nearest its goal; an agent that wants a vertex where another
//! agent stands makes that one choose first, at its priority, and tries its next choice if
//! that one is stuck. An agent's priority grows each step it is away from its goal and falls
//! back once there, so that every agent gets its turn to push through. Nobody plans more than
//! a step ahead, which is why it is fast, and why its plans wander, and on some maps agents go
//! round for ever.
//!
//! LaCAM (Okumura, AAAI 2023) makes it complete: a depth-first search over configurations,
//! where each configuration's successors are PIBT's step from it under constraints that fix
//! where some agents go, more of them each time the search comes back to it, so that in the
//! end every possible step from it is tried. A configuration met before is not added again.
//! This is LaCAM's first search, with the swap that LaCAM3's PIBT has for corridors (Okumura,
//! IJCAI 2023), ported from LaCAM3's MIT-licensed code, but without LaCAM\* and LaCAM3's
//! refinements after the first solution, which is all [`solve`](super::solve) needs from it.

use core::sync::atomic::{AtomicBool, Ordering};
use std::collections::VecDeque;

use crate::space::FastMap;

use super::space::Instance;

const NONE: u32 = u32::MAX;

/// How many constraints a search may make at most, about 160 MB of them: each configuration's
/// tree of constraints grows by a node per neighbour each time the search comes back to it,
/// and on a maze with a thousand agents that is what runs out first.
const MAX_CONSTRAINTS: usize = 10_000_000;

/// A hash for breaking ties, the same on every platform.
fn mix(a: u32, b: u32, c: u32) -> u32 {
    let mut x = u64::from(a) ^ (u64::from(b) << 21) ^ (u64::from(c) << 42);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^= x >> 33;
    x as u32
}

/// A constraint of a configuration's tree: `who` goes to `to`, after its parent's.
#[derive(Clone, Copy, Debug)]
struct Low {
    parent: u32,
    who: u32,
    to: u32,
    depth: u32,
}

/// A configuration the search has reached.
struct High {
    /// Where each agent is.
    at: Vec<u32>,
    parent: u32,
    /// Each agent's priority.
    priority: Vec<f64>,
    /// The agents in order of priority, highest first.
    order: Vec<u32>,
    /// Constraints still to try, breadth first.
    tree: VecDeque<u32>,
}

/// PIBT's step, with the scratch it reuses.
struct Step {
    next: Vec<u32>,
    /// Who is on each vertex now, and who will be next.
    here: Vec<u32>,
    there: Vec<u32>,
    candidates: Vec<(u32, u32, u32)>,
    salt: u32,
}

impl Step {
    /// Chooses where `agent` goes next, as LaCAM3's PIBT does, swap included. Returns
    /// whether it got a vertex, rather than staying put against its will.
    fn choose(&mut self, instance: &Instance<'_>, at: &[u32], agent: u32) -> bool {
        let a = agent as usize;
        let distance = instance.distances(a);
        let here = at[a];
        let start = self.candidates.len();
        for &v in instance
            .graph()
            .neighbours(here)
            .iter()
            .chain([here].iter())
        {
            self.candidates
                .push((distance[v as usize], mix(agent, self.salt, v), v));
        }
        self.candidates[start..].sort_unstable();
        let swap = self.swap_agent(instance, at, agent, self.candidates[start].2);
        if swap != NONE {
            self.candidates[start..].reverse();
        }
        let end = self.candidates.len();
        for k in start..end {
            let v = self.candidates[k].2;
            if self.there[v as usize] != NONE {
                continue;
            }
            let other = self.here[v as usize];
            // No swapping places with an agent that has already chosen to come here.
            if other != NONE && self.next[other as usize] == here {
                continue;
            }
            self.there[v as usize] = agent;
            self.next[a] = v;
            if other != NONE
                && v != here
                && self.next[other as usize] == NONE
                && !self.choose(instance, at, other)
            {
                continue;
            }
            if k == start
                && swap != NONE
                && self.next[swap as usize] == NONE
                && self.there[here as usize] == NONE
            {
                // Pull the agent to swap with into the vertex left.
                self.there[here as usize] = swap;
                self.next[swap as usize] = here;
            }
            self.candidates.truncate(start);
            return true;
        }
        self.candidates.truncate(start);
        self.next[a] = here;
        self.there[here as usize] = agent;
        false
    }

    /// The agent `agent` should swap places with, by LaCAM3's rules, if it wants `wanted`
    /// most: one in a corridor that only a swap gets them both through.
    fn swap_agent(&self, instance: &Instance<'_>, at: &[u32], agent: u32, wanted: u32) -> u32 {
        let here = at[agent as usize];
        let j = self.here[wanted as usize];
        if j != NONE
            && j != agent
            && self.next[j as usize] == NONE
            && self.swap_required(instance, agent, j, here, at[j as usize])
            && self.swap_possible(instance, at[j as usize], here)
        {
            return j;
        }
        if wanted != here {
            for &u in instance.graph().neighbours(here) {
                let k = self.here[u as usize];
                if k != NONE
                    && wanted != at[k as usize]
                    && self.swap_required(instance, k, agent, here, wanted)
                    && self.swap_possible(instance, wanted, here)
                {
                    return k;
                }
            }
        }
        NONE
    }

    /// The neighbours of `puller` other than `pusher` and dead ends held by agents on their
    /// goals: how many, and the last.
    fn ways_on(&self, instance: &Instance<'_>, pusher: u32, puller: u32) -> (usize, u32) {
        let graph = instance.graph();
        let mut count = 0;
        let mut last = NONE;
        for &u in graph.neighbours(puller) {
            let holder = self.here[u as usize];
            let parked = graph.neighbours(u).len() == 1
                && holder != NONE
                && instance.agents()[holder as usize].goal == u;
            if u != pusher && !parked {
                count += 1;
                last = u;
            }
        }
        (count, last)
    }

    fn swap_required(
        &self,
        instance: &Instance<'_>,
        pusher: u32,
        puller: u32,
        pusher_from: u32,
        puller_from: u32,
    ) -> bool {
        let d_pusher = instance.distances(pusher as usize);
        let d_puller = instance.distances(puller as usize);
        let (mut v_pusher, mut v_puller) = (pusher_from, puller_from);
        while d_pusher[v_puller as usize] < d_pusher[v_pusher as usize] {
            let (n, next) = self.ways_on(instance, v_pusher, v_puller);
            if n >= 2 {
                return false;
            }
            if n == 0 {
                break;
            }
            v_pusher = v_puller;
            v_puller = next;
        }
        d_puller[v_pusher as usize] < d_puller[v_puller as usize]
            && (d_pusher[v_pusher as usize] == 0
                || d_pusher[v_puller as usize] < d_pusher[v_pusher as usize])
    }

    fn swap_possible(&self, instance: &Instance<'_>, pusher_from: u32, puller_from: u32) -> bool {
        let (mut v_pusher, mut v_puller) = (pusher_from, puller_from);
        while v_puller != pusher_from {
            let (n, next) = self.ways_on(instance, v_pusher, v_puller);
            if n >= 2 {
                return true;
            }
            if n == 0 {
                return false;
            }
            v_pusher = v_puller;
            v_puller = next;
        }
        false
    }

    /// The next configuration from `at`, with the agents of `fixed` going where it says and
    /// the others by PIBT in `order`, or `None` if the fixed moves collide.
    fn run(
        &mut self,
        instance: &Instance<'_>,
        at: &[u32],
        order: &[u32],
        fixed: &[(u32, u32)],
    ) -> Option<Vec<u32>> {
        for (agent, &v) in at.iter().enumerate() {
            self.here[v as usize] = agent as u32;
        }
        let mut ok = true;
        for &(who, to) in fixed {
            if self.there[to as usize] != NONE {
                ok = false;
                break;
            }
            // A swap with another fixed agent.
            let other = self.here[to as usize];
            if other != NONE && other != who && self.next[other as usize] == at[who as usize] {
                ok = false;
                break;
            }
            self.there[to as usize] = who;
            self.next[who as usize] = to;
        }
        if ok {
            for &agent in order {
                if self.next[agent as usize] == NONE && !self.choose(instance, at, agent) {
                    ok = false;
                    break;
                }
            }
        }
        for &(_, to) in fixed {
            self.there[to as usize] = NONE;
        }
        for &v in &self.next {
            if v != NONE {
                self.there[v as usize] = NONE;
            }
        }
        // An agent that can't move from a vertex a fixed agent goes to stays there all the
        // same, and that configuration is no good; so is a swap.
        if ok {
            for (agent, &v) in self.next.iter().enumerate() {
                let other = self.here[v as usize];
                if self.there[v as usize] != NONE
                    || (other != NONE
                        && other != agent as u32
                        && self.next[other as usize] == at[agent])
                {
                    ok = false;
                    break;
                }
                self.there[v as usize] = agent as u32;
            }
            for &v in &self.next {
                self.there[v as usize] = NONE;
            }
        }
        let result = ok.then(|| self.next.clone());
        for (agent, &v) in at.iter().enumerate() {
            self.here[v as usize] = NONE;
            self.next[agent] = NONE;
        }
        result
    }
}

/// Finds a solution by LaCAM, as the [module documentation](self) says, with at most
/// `max_configurations` configurations and [`MAX_CONSTRAINTS`] constraints, and stops when
/// `cancel` is set. Returns one plan per
/// agent, up to its last arrival on its goal.
pub(super) fn lacam(
    instance: &Instance<'_>,
    max_configurations: u32,
    cancel: Option<&AtomicBool>,
) -> Option<Vec<Vec<u32>>> {
    let graph = instance.graph();
    let agents = instance.agents();
    let n = agents.len();
    let vertices = graph.vertex_count() as usize;
    let goals: Vec<u32> = agents.iter().map(|a| a.goal).collect();
    let mut step = Step {
        next: vec![NONE; n],
        here: vec![NONE; vertices],
        there: vec![NONE; vertices],
        candidates: Vec::new(),
        salt: 0,
    };
    let mut lows = vec![Low {
        parent: NONE,
        who: NONE,
        to: NONE,
        depth: 0,
    }];
    let start: Vec<u32> = agents.iter().map(|a| a.start).collect();
    let priority: Vec<f64> = (0..n)
        .map(|i| f64::from(instance.distances(i)[agents[i].start as usize]) / vertices as f64)
        .collect();
    let mut highs = vec![high(start.clone(), NONE, priority)];
    // Looked up, never iterated.
    let mut seen: FastMap<Vec<u32>, u32> = FastMap::default();
    seen.insert(start, 0);
    let mut open = vec![0u32];
    let mut fixed = Vec::new();
    while let Some(&top) = open.last() {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return None;
        }
        let node = &mut highs[top as usize];
        if node.at == goals {
            return Some(plans(&highs, top, &goals));
        }
        let Some(low) = node.tree.pop_front() else {
            open.pop();
            continue;
        };
        let constraint = lows[low as usize];
        if lows.len() >= MAX_CONSTRAINTS {
            return None;
        }
        if (constraint.depth as usize) < n {
            let who = node.order[constraint.depth as usize];
            let here = node.at[who as usize];
            let mut choices: Vec<u32> = graph.neighbours(here).to_vec();
            choices.push(here);
            choices.sort_by_key(|&v| mix(who, top, v));
            for to in choices {
                node.tree.push_back(lows.len() as u32);
                lows.push(Low {
                    parent: low,
                    who,
                    to,
                    depth: constraint.depth + 1,
                });
            }
        }
        fixed.clear();
        let mut at = low;
        while lows[at as usize].parent != NONE {
            let l = lows[at as usize];
            fixed.push((l.who, l.to));
            at = l.parent;
        }
        fixed.reverse();
        step.salt = step.salt.wrapping_add(1);
        let node = &highs[top as usize];
        let Some(next) = step.run(instance, &node.at, &node.order, &fixed) else {
            continue;
        };
        if let Some(&known) = seen.get(&next) {
            open.push(known);
            continue;
        }
        if highs.len() as u32 >= max_configurations {
            return None;
        }
        let priority = node
            .priority
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                if next[i] == goals[i] {
                    p - p.floor()
                } else {
                    p + 1.0
                }
            })
            .collect();
        let id = highs.len() as u32;
        seen.insert(next.clone(), id);
        highs.push(high(next, top, priority));
        open.push(id);
    }
    None
}

fn high(at: Vec<u32>, parent: u32, priority: Vec<f64>) -> High {
    let mut order: Vec<u32> = (0..at.len() as u32).collect();
    order.sort_by(|&a, &b| {
        priority[b as usize]
            .total_cmp(&priority[a as usize])
            .then(a.cmp(&b))
    });
    High {
        at,
        parent,
        priority,
        order,
        tree: VecDeque::from([0]),
    }
}

/// Each agent's plan, from the start to configuration `last`, up to its last arrival.
fn plans(highs: &[High], mut last: u32, goals: &[u32]) -> Vec<Vec<u32>> {
    let mut chain = Vec::new();
    while last != NONE {
        chain.push(last);
        last = highs[last as usize].parent;
    }
    chain.reverse();
    (0..goals.len())
        .map(|agent| {
            let mut plan: Vec<u32> = chain.iter().map(|&h| highs[h as usize].at[agent]).collect();
            while plan.len() > 1 && plan[plan.len() - 2] == goals[agent] {
                plan.pop();
            }
            plan
        })
        .collect()
}
