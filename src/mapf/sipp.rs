//! Safe-interval path planning against the other agents' plans, with their conflicts priced
//! or forbidden.
//!
//! [`Table`] holds every agent's plan as visits, a vertex from one time to another, and moves,
//! with the history that negotiation has put on vertices and edges at times. [`Planner`] plans
//! one agent against it. Its states are a vertex and an interval of time over which nothing
//! there changes: not who else is on it, nor its history. That is safe-interval path planning
//! (Phillips and Likhachev, ICRA 2011), where an interval is safe when nobody else is there; in
//! [`Mode::Soft`], as in SIPPS (Li et al., AAAI 2022), an interval with someone in it may be
//! entered too, at the engine's price for overuse, and in [`Mode::Hard`] it may not.
//!
//! In a time-expanded space a wait is a state of its own at each time, and a search that has to
//! wait for a corridor to clear tries every place and length of wait there is. Here an agent
//! arrives in an interval as early as it can and may stay to its end, so that waiting costs
//! nothing to search, and how many states a vertex has depends on how many agents pass through
//! it, not on the horizon. There is no horizon at all.
//!
//! # Prices
//!
//! A move or a wait costs 1, and being on a vertex or an edge at a time that `o` other agents
//! hold, with history `h` there, costs `h + (1 + h) * present * o` more, the engine's price for
//! a step with a base cost of 1. Holding the goal from arrival on costs only that surcharge, so
//! that a plan alone costs its arrival time, as the sum of costs has it.
//!
//! Earliest arrival is exact in [`Mode::Hard`], where a later arrival in the same interval
//! never does better. With prices, it may: arriving later in a dear interval, after a wait
//! where it was cheap, pays less. The planner keeps the earliest arrival in each interval that
//! it reaches from each state, and every arrival that no other in the same interval beats once
//! it has waited for it, and it tries the next time too when a move would swap places with
//! someone. That is a heuristic for prices, which negotiation only needs to point the way.

use core::cmp::Ordering;
use core::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::collections::BinaryHeap;

use super::graph::Graph;

/// The end of a visit that never ends: an agent on its goal for good.
pub(super) const FOREVER: u32 = u32::MAX;

/// An agent on a vertex from `from` to `to`, both included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Visit {
    from: u32,
    agent: u32,
    to: u32,
}

/// An agent leaving a vertex for `to` at `time`, to be there at `time + 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Move {
    time: u32,
    to: u32,
    agent: u32,
}

/// Calls `f(vertex, from, to)` for each stretch of `plan` on one vertex, the last for ever.
fn for_each_visit(plan: &[u32], mut f: impl FnMut(u32, u32, u32)) {
    let mut from = 0;
    for time in 1..=plan.len() {
        if time == plan.len() {
            f(plan[from], from as u32, FOREVER);
        } else if plan[time] != plan[from] {
            f(plan[from], from as u32, time as u32 - 1);
            from = time;
        }
    }
}

/// Calls `f(from, time, to)` for each move of `plan`.
fn for_each_move(plan: &[u32], mut f: impl FnMut(u32, u32, u32)) {
    for (time, pair) in plan.windows(2).enumerate() {
        if pair[0] != pair[1] {
            f(pair[0], time as u32, pair[1]);
        }
    }
}

/// An overused resource, as [`Table::conflicts`] finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Overuse {
    /// `vertex` from `from` to `to`, both included, with `extra` agents too many on it.
    /// When one of them holds it as its goal for good, history goes from `held`, the time it
    /// arrived, or the end of the overuse before, so that a conflict that moves later from one
    /// iteration to the next doesn't escape it.
    Vertex {
        vertex: u32,
        from: u32,
        to: u32,
        extra: u32,
        held: u32,
    },
    /// The edge between `low` and `high` during the move from `time`.
    Edge {
        low: u32,
        high: u32,
        time: u32,
        extra: u32,
    },
}

impl Overuse {
    /// How many vertex-times or edge-times this is.
    pub(super) fn resources(self) -> u64 {
        match self {
            Overuse::Vertex { from, to, .. } => u64::from(to - from) + 1,
            Overuse::Edge { .. } => 1,
        }
    }
}

/// Every agent's plan as visits and moves, and history.
#[derive(Clone, Debug)]
pub(super) struct Table {
    /// Per vertex, sorted.
    visits: Vec<Vec<Visit>>,
    /// Per vertex left, sorted.
    moves: Vec<Vec<Move>>,
    /// Per vertex, history as breakpoints: each value holds from its time to the next.
    vertex_history: Vec<Vec<(u32, f64)>>,
    /// Per lower vertex of an edge: the time, the higher vertex and the history, sorted.
    edge_history: Vec<Vec<(u32, u32, f64)>>,
    /// Per vertex, the intervals over which neither who is there nor history changes, kept
    /// up to date, for searches to price.
    intervals: Vec<Vec<Interval>>,
    events: Vec<(u32, i32)>,
    times: Vec<u32>,
}

/// An interval of a vertex over which neither how many agents are there nor its history
/// changes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Interval {
    start: u32,
    /// The last time, included, or [`FOREVER`].
    end: u32,
    count: u32,
    history: f64,
}

impl Table {
    pub(super) fn new(vertices: u32) -> Self {
        let n = vertices as usize;
        Table {
            visits: vec![Vec::new(); n],
            moves: vec![Vec::new(); n],
            vertex_history: vec![Vec::new(); n],
            edge_history: vec![Vec::new(); n],
            intervals: vec![
                vec![Interval {
                    start: 0,
                    end: FOREVER,
                    count: 0,
                    history: 0.0,
                }];
                n
            ],
            events: Vec::new(),
            times: Vec::new(),
        }
    }

    /// Works out the intervals of `vertex` again.
    fn refresh(&mut self, vertex: u32) {
        let v = vertex as usize;
        self.events.clear();
        self.times.clear();
        self.times.push(0);
        for visit in &self.visits[v] {
            self.events.push((visit.from, 1));
            self.times.push(visit.from);
            if visit.to != FOREVER {
                self.events.push((visit.to + 1, -1));
                self.times.push(visit.to + 1);
            }
        }
        let history = &self.vertex_history[v];
        self.times.extend(history.iter().map(|&(t, _)| t));
        self.events.sort_unstable();
        self.times.sort_unstable();
        self.times.dedup();
        let list = &mut self.intervals[v];
        list.clear();
        let (mut event, mut count) = (0, 0i32);
        let (mut next_history, mut h) = (0, 0.0);
        for (index, &time) in self.times.iter().enumerate() {
            while event < self.events.len() && self.events[event].0 <= time {
                count += self.events[event].1;
                event += 1;
            }
            while next_history < history.len() && history[next_history].0 <= time {
                h = history[next_history].1;
                next_history += 1;
            }
            let end = self.times.get(index + 1).map_or(FOREVER, |&next| next - 1);
            let count = count.max(0) as u32;
            match list.last_mut() {
                Some(last) if last.count == count && last.history == h => last.end = end,
                _ => list.push(Interval {
                    start: time,
                    end,
                    count,
                    history: h,
                }),
            }
        }
    }

    /// Adds `agent`'s plan.
    pub(super) fn add(&mut self, agent: u32, plan: &[u32]) {
        for_each_visit(plan, |vertex, from, to| {
            let list = &mut self.visits[vertex as usize];
            let visit = Visit { from, agent, to };
            let at = list.partition_point(|v| *v < visit);
            list.insert(at, visit);
            self.refresh(vertex);
        });
        for_each_move(plan, |vertex, time, to| {
            let list = &mut self.moves[vertex as usize];
            let item = Move { time, to, agent };
            let at = list.partition_point(|m| *m < item);
            list.insert(at, item);
        });
    }

    /// Removes `agent`'s plan, which has to be the one added.
    pub(super) fn remove(&mut self, agent: u32, plan: &[u32]) {
        for_each_visit(plan, |vertex, from, to| {
            let list = &mut self.visits[vertex as usize];
            let visit = Visit { from, agent, to };
            if let Ok(at) = list.binary_search(&visit) {
                list.remove(at);
            }
            self.refresh(vertex);
        });
        for_each_move(plan, |vertex, time, to| {
            let list = &mut self.moves[vertex as usize];
            if let Ok(at) = list.binary_search(&Move { time, to, agent }) {
                list.remove(at);
            }
        });
    }

    /// How many agents other than `agent` move between `a` and `b`, either way, leaving at
    /// `time`.
    fn edge_users(&self, a: u32, b: u32, time: u32, agent: u32) -> u32 {
        self.movers(a, b, time, agent) + self.movers(b, a, time, agent)
    }

    /// How many agents other than `agent` move from `from` to `to` leaving at `time`.
    fn movers(&self, from: u32, to: u32, time: u32, agent: u32) -> u32 {
        let list = &self.moves[from as usize];
        let first = list.partition_point(|m| (m.time, m.to) < (time, to));
        list[first..]
            .iter()
            .take_while(|m| m.time == time && m.to == to)
            .filter(|m| m.agent != agent)
            .count() as u32
    }

    /// The history of the edge between `a` and `b` during the move from `time`.
    fn edge_history(&self, a: u32, b: u32, time: u32) -> f64 {
        let (low, high) = (a.min(b), a.max(b));
        let list = &self.edge_history[low as usize];
        match list.binary_search_by(|&(t, h, _)| (t, h).cmp(&(time, high))) {
            Ok(at) => list[at].2,
            Err(_) => 0.0,
        }
    }

    /// Whether `agent`, whose plan is `plan`, shares a vertex or an edge with anyone at any
    /// time.
    pub(super) fn in_conflict(&self, agent: u32, plan: &[u32]) -> bool {
        let mut found = false;
        for_each_visit(plan, |vertex, from, to| {
            if found {
                return;
            }
            let list = &self.visits[vertex as usize];
            let end = list.partition_point(|v| v.from <= to);
            found = list[..end].iter().any(|v| v.agent != agent && v.to >= from);
        });
        if found {
            return true;
        }
        for_each_move(plan, |vertex, time, to| {
            if !found && self.edge_users(vertex, to, time, agent) > 0 {
                found = true;
            }
        });
        found
    }

    /// Appends every overused resource, in a fixed order.
    pub(super) fn conflicts(&self, out: &mut Vec<Overuse>) {
        let mut events: Vec<(u32, i32)> = Vec::new();
        for (vertex, list) in self.visits.iter().enumerate() {
            if list.len() < 2 {
                continue;
            }
            events.clear();
            for visit in list {
                events.push((visit.from, 1));
                if visit.to != FOREVER {
                    events.push((visit.to + 1, -1));
                }
            }
            events.sort_unstable();
            // Two plans can't both hold a vertex for ever, as goals differ.
            let hold = list.iter().find(|v| v.to == FOREVER).map(|v| v.from);
            let mut free_from = 0;
            let mut count = 0i32;
            let mut index = 0;
            while index < events.len() {
                let time = events[index].0;
                while index < events.len() && events[index].0 == time {
                    count += events[index].1;
                    index += 1;
                }
                if count >= 2 {
                    let end = events.get(index).map_or(FOREVER, |&(next, _)| next - 1);
                    let held = match hold {
                        Some(arrival) if arrival <= time => arrival.max(free_from),
                        _ => time,
                    };
                    free_from = end.saturating_add(1);
                    out.push(Overuse::Vertex {
                        vertex: vertex as u32,
                        from: time,
                        to: end,
                        extra: count as u32 - 1,
                        held,
                    });
                }
            }
        }
        for (vertex, list) in self.moves.iter().enumerate() {
            let vertex = vertex as u32;
            let mut index = 0;
            while index < list.len() {
                let (time, to) = (list[index].time, list[index].to);
                let mut here = 0u32;
                while index < list.len() && list[index].time == time && list[index].to == to {
                    here += 1;
                    index += 1;
                }
                let back = self.edge_users(to, vertex, time, u32::MAX) - here;
                // Each edge once: from its lower end, or from the higher if nobody comes back.
                let total = here + back;
                if total >= 2 && (vertex < to || back == 0) {
                    out.push(Overuse::Edge {
                        low: vertex.min(to),
                        high: vertex.max(to),
                        time,
                        extra: total - 1,
                    });
                }
            }
        }
    }

    /// Adds `amount` times each overuse's extra agents to its history, from the time it
    /// [`held`](Overuse::Vertex) its vertex if `from_held`.
    pub(super) fn add_history(&mut self, overuse: Overuse, amount: f64, from_held: bool) {
        match overuse {
            Overuse::Vertex {
                vertex,
                from,
                to,
                extra,
                held,
            } => {
                let list = &mut self.vertex_history[vertex as usize];
                let value = amount * f64::from(extra);
                let first = split(list, if from_held { held } else { from });
                // A breakpoint after `from` leaves `first` where it is.
                let end = if to == FOREVER {
                    list.len()
                } else {
                    split(list, to + 1)
                };
                for entry in &mut list[first..end] {
                    entry.1 += value;
                }
                self.refresh(vertex);
            }
            Overuse::Edge {
                low,
                high,
                time,
                extra,
            } => {
                let list = &mut self.edge_history[low as usize];
                let value = amount * f64::from(extra);
                match list.binary_search_by(|&(t, h, _)| (t, h).cmp(&(time, high))) {
                    Ok(at) => list[at].2 += value,
                    Err(at) => list.insert(at, (time, high, value)),
                }
            }
        }
    }
}

/// Makes sure `list` has a breakpoint at `time`, and returns its index.
fn split(list: &mut Vec<(u32, f64)>, time: u32) -> usize {
    let at = list.partition_point(|&(t, _)| t < time);
    if list.get(at).is_some_and(|&(t, _)| t == time) {
        return at;
    }
    let value = if at == 0 { 0.0 } else { list[at - 1].1 };
    list.insert(at, (time, value));
    at
}

/// How a [`Planner`] treats the other agents.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Mode {
    /// Their vertices and edges may be shared, at the engine's price with this present
    /// factor, and history counts.
    Soft { present: f64 },
    /// They may not be shared, and history doesn't count.
    Hard,
}

/// An interval of a vertex over which its price doesn't change.
#[derive(Clone, Copy, Debug)]
struct Segment {
    start: u32,
    /// The last time, included, or [`FOREVER`].
    end: u32,
    /// What being there costs at each time, beyond the step's own cost of 1.
    price: f64,
    occupied: bool,
}

const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug)]
struct Label {
    vertex: u32,
    segment: u32,
    time: u32,
    parent: u32,
    /// The next label of the same segment.
    next: u32,
    g: f64,
    dead: bool,
    done: bool,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    f: f64,
    h: u32,
    label: u32,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    /// The heap's greatest is the least `f`, then the least `h`, then the first made.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .f
            .total_cmp(&self.f)
            .then_with(|| other.h.cmp(&self.h))
            .then_with(|| other.label.cmp(&self.label))
    }
}

/// What a search found.
#[derive(Clone, Debug)]
pub(super) enum Planned {
    /// A plan, the vertex at each time up to the last arrival on the goal.
    Found(Vec<u32>),
    /// No plan avoids the others (only in [`Mode::Hard`]).
    NoPlan,
    /// The budget ran out, or the run was cancelled.
    Stopped,
}

/// Plans one agent at a time against a [`Table`], reusing its memory.
#[derive(Debug, Default)]
pub(super) struct Planner {
    generation: u32,
    /// Per vertex: the generation its segments are of, the first and how many.
    segment_generation: Vec<u32>,
    segment_first: Vec<u32>,
    segment_count: Vec<u32>,
    segments: Vec<Segment>,
    /// Per segment, the first of its labels.
    heads: Vec<u32>,
    /// For the goal's segments, what holding it from the end of each costs.
    hold_after: Vec<f64>,
    labels: Vec<Label>,
    open: BinaryHeap<Entry>,
    /// Nodes expanded by every search so far.
    pub(super) expanded: u64,
}

impl Planner {
    pub(super) fn new(vertices: u32) -> Self {
        let n = vertices as usize;
        Planner {
            segment_generation: vec![0; n],
            segment_first: vec![0; n],
            segment_count: vec![0; n],
            ..Planner::default()
        }
    }

    /// The segments of `vertex`, as the first index and how many, made on first use in a
    /// search.
    fn segments_of(&mut self, table: &Table, vertex: u32, mode: Mode) -> (u32, u32) {
        let v = vertex as usize;
        if self.segment_generation[v] == self.generation {
            return (self.segment_first[v], self.segment_count[v]);
        }
        let first = self.segments.len() as u32;
        for interval in &table.intervals[v] {
            let occupied = interval.count > 0;
            let price = match mode {
                Mode::Soft { present } => {
                    let h = interval.history;
                    h + (1.0 + h) * present * f64::from(interval.count)
                }
                Mode::Hard => 0.0,
            };
            let own = self.segments.len() as u32 > first;
            match self.segments.last_mut() {
                Some(last) if own && last.occupied == occupied && last.price == price => {
                    last.end = interval.end;
                }
                _ => {
                    self.segments.push(Segment {
                        start: interval.start,
                        end: interval.end,
                        price,
                        occupied,
                    });
                    self.heads.push(NONE);
                }
            }
        }
        let count = self.segments.len() as u32 - first;
        self.segment_generation[v] = self.generation;
        self.segment_first[v] = first;
        self.segment_count[v] = count;
        (first, count)
    }

    /// Adds a label unless one in the same segment arrives no later and costs no more once it
    /// has waited for it, and drops those it beats in turn.
    fn push(&mut self, label: Label, h: u32) {
        let segment = label.segment as usize;
        let wait = 1.0 + self.segments[segment].price;
        let mut at = self.heads[segment];
        while at != NONE {
            let other = self.labels[at as usize];
            if !other.dead {
                if other.time <= label.time
                    && other.g + f64::from(label.time - other.time) * wait <= label.g
                {
                    return;
                }
                if label.time <= other.time
                    && label.g + f64::from(other.time - label.time) * wait <= other.g
                {
                    self.labels[at as usize].dead = true;
                }
            }
            at = other.next;
        }
        let id = self.labels.len() as u32;
        self.labels.push(Label {
            next: self.heads[segment],
            ..label
        });
        self.heads[segment] = id;
        self.open.push(Entry {
            f: label.g + f64::from(h),
            h,
            label: id,
        });
    }

    /// Plans an agent from `start` at time 0 to `goal`, which it then holds for ever, against
    /// everyone in `table`, which it isn't in, guided by `distance`, each vertex's distance to
    /// the goal, and stops after `budget` expansions or once `cancel` is set.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn plan(
        &mut self,
        graph: &Graph,
        table: &Table,
        distance: &[u32],
        start: u32,
        goal: u32,
        mode: Mode,
        budget: u64,
        cancel: Option<&AtomicBool>,
    ) -> Planned {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.segment_generation.fill(0);
            self.generation = 1;
        }
        self.segments.clear();
        self.heads.clear();
        self.labels.clear();
        self.open.clear();
        let present = match mode {
            Mode::Soft { present } => present,
            Mode::Hard => 0.0,
        };
        let hard = mode == Mode::Hard;

        // What holding the goal costs from the end of each of its segments on.
        let (goal_first, goal_count) = self.segments_of(table, goal, mode);
        self.hold_after.clear();
        self.hold_after.resize(goal_count as usize, 0.0);
        for k in (0..goal_count.saturating_sub(1) as usize).rev() {
            let next = self.segments[goal_first as usize + k + 1];
            let length = if next.end == FOREVER {
                0.0
            } else {
                f64::from(next.end - next.start) + 1.0
            };
            self.hold_after[k] = self.hold_after[k + 1] + length * next.price;
        }

        let (first, count) = self.segments_of(table, start, mode);
        let segment = first + self.find(first, count, 0);
        if hard && self.segments[segment as usize].occupied {
            return Planned::NoPlan;
        }
        let start_label = Label {
            vertex: start,
            segment,
            time: 0,
            parent: NONE,
            next: NONE,
            g: 0.0,
            dead: false,
            done: false,
        };
        self.push(start_label, distance[start as usize]);

        let mut expanded = 0u64;
        while let Some(entry) = self.open.pop() {
            let label = self.labels[entry.label as usize];
            if label.dead {
                continue;
            }
            if label.done {
                self.expanded += expanded;
                let plan = self.read_plan(label.parent);
                return Planned::Found(plan);
            }
            expanded += 1;
            if expanded >= budget {
                self.expanded += expanded;
                return Planned::Stopped;
            }
            if expanded.is_multiple_of(1024)
                && cancel.is_some_and(|flag| flag.load(AtomicOrdering::Relaxed))
            {
                self.expanded += expanded;
                return Planned::Stopped;
            }
            let here = self.segments[label.segment as usize];
            let wait = 1.0 + here.price;
            let (v, t, g) = (label.vertex, label.time, label.g);

            if v == goal {
                let k = (label.segment - goal_first) as usize;
                let finish = if hard {
                    (here.end == FOREVER).then_some(g)
                } else {
                    let rest = if here.end == FOREVER {
                        0.0
                    } else {
                        f64::from(here.end - t) * here.price
                    };
                    Some(g + rest + self.hold_after[k])
                };
                if let Some(cost) = finish {
                    let id = self.labels.len() as u32;
                    self.labels.push(Label {
                        parent: entry.label,
                        g: cost,
                        done: true,
                        next: NONE,
                        ..label
                    });
                    self.open.push(Entry {
                        f: cost,
                        h: 0,
                        label: id,
                    });
                }
            }

            // Waiting into the next segment.
            if here.end != FOREVER && !hard {
                let next = label.segment + 1;
                let price = self.segments[next as usize].price;
                self.push(
                    Label {
                        vertex: v,
                        segment: next,
                        time: here.end + 1,
                        parent: entry.label,
                        next: NONE,
                        g: g + f64::from(here.end - t) * wait + 1.0 + price,
                        dead: false,
                        done: false,
                    },
                    distance[v as usize],
                );
            }

            let latest = if here.end == FOREVER {
                FOREVER
            } else {
                here.end + 1
            };
            for &u in graph.neighbours(v) {
                let h = distance[u as usize];
                if h == u32::MAX {
                    continue;
                }
                let (first, count) = self.segments_of(table, u, mode);
                let mut k = self.find(first, count, t + 1);
                while k < count {
                    let there = self.segments[(first + k) as usize];
                    k += 1;
                    if there.start > latest {
                        break;
                    }
                    if hard && there.occupied {
                        continue;
                    }
                    let mut arrive = there.start.max(t + 1);
                    let last = there.end.min(latest);
                    let mut tries = 0;
                    while arrive <= last {
                        let leave = arrive - 1;
                        // Someone coming the other way arrives here when this interval
                        // ends, unless someone else leaves then, which is a conflict here
                        // already, going the same way shares both intervals.
                        let mut users = 0;
                        if leave == here.end || here.occupied {
                            users += table.movers(u, v, leave, NONE);
                        }
                        if here.occupied && there.occupied {
                            users += table.movers(v, u, leave, NONE);
                        }
                        let cost = if hard {
                            (users == 0).then(|| g + f64::from(leave - t) + 1.0)
                        } else {
                            let history = table.edge_history(v, u, leave);
                            Some(
                                g + f64::from(leave - t) * wait
                                    + 1.0
                                    + there.price
                                    + history
                                    + (1.0 + history) * present * f64::from(users),
                            )
                        };
                        if let Some(cost) = cost {
                            self.push(
                                Label {
                                    vertex: u,
                                    segment: first + k - 1,
                                    time: arrive,
                                    parent: entry.label,
                                    next: NONE,
                                    g: cost,
                                    dead: false,
                                    done: false,
                                },
                                h,
                            );
                        }
                        tries += 1;
                        if users == 0 || (!hard && tries >= 2) || arrive == FOREVER - 1 {
                            break;
                        }
                        arrive += 1;
                    }
                }
            }
        }
        self.expanded += expanded;
        Planned::NoPlan
    }

    /// The index, among the `count` segments from `first`, of the one that `time` is in.
    fn find(&self, first: u32, count: u32, time: u32) -> u32 {
        let list = &self.segments[first as usize..(first + count) as usize];
        (list.partition_point(|s| s.start <= time) as u32).saturating_sub(1)
    }

    /// The plan that ends with `label`, without waits on the goal at its end.
    fn read_plan(&self, mut label: u32) -> Vec<u32> {
        let mut stops = Vec::new();
        while label != NONE {
            let l = self.labels[label as usize];
            stops.push((l.vertex, l.time));
            label = l.parent;
        }
        stops.reverse();
        let mut plan = Vec::new();
        for (vertex, time) in stops {
            while (plan.len() as u32) < time {
                let last = *plan.last().unwrap_or(&vertex);
                plan.push(last);
            }
            plan.push(vertex);
        }
        while plan.len() > 1 && plan[plan.len() - 2] == plan[plan.len() - 1] {
            plan.pop();
        }
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: u32) -> Graph {
        let edges: Vec<_> = (1..n).map(|v| (v - 1, v)).collect();
        Graph::from_edges(n, &edges).unwrap()
    }

    fn plan_one(graph: &Graph, table: &Table, start: u32, goal: u32, mode: Mode) -> Planned {
        let distance = graph.distances_to(goal);
        Planner::new(graph.vertex_count()).plan(
            graph,
            table,
            &distance,
            start,
            goal,
            mode,
            u64::MAX,
            None,
        )
    }

    fn found(planned: Planned) -> Vec<u32> {
        match planned {
            Planned::Found(plan) => plan,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn hard_plans_wait_for_a_corridor_to_clear() {
        let graph = line(4);
        let mut table = Table::new(4);
        table.add(1, &[1, 1, 1, 1, 2, 3]);
        assert_eq!(
            found(plan_one(&graph, &table, 0, 2, Mode::Hard)),
            [0, 0, 0, 0, 1, 2]
        );
        // A goal that someone passes later can't be held before.
        let mut table = Table::new(4);
        table.add(1, &[3, 3, 3, 2, 1]);
        assert!(matches!(
            plan_one(&graph, &table, 0, 2, Mode::Hard),
            Planned::NoPlan
        ));
    }

    #[test]
    fn hard_plans_do_not_swap() {
        // 0 - 1 - 2, with 3 off 1: the other agent comes from 2 to 0 through 1.
        let graph = Graph::from_edges(4, &[(0, 1), (1, 2), (1, 3)]).unwrap();
        let mut table = Table::new(4);
        table.add(1, &[2, 1, 0]);
        // From 1 to 2 at once would swap with it, so it steps aside to 3 and back.
        let plan = found(plan_one(&graph, &table, 1, 2, Mode::Hard));
        assert_eq!(plan, [1, 3, 1, 2]);
    }

    #[test]
    fn soft_plans_pay_for_what_they_share_with_history() {
        let graph = line(5);
        let mut table = Table::new(5);
        table.add(1, &[4, 4, 4, 4, 3, 2, 1, 0]);
        let soft = Mode::Soft { present: 10.0 };
        // Holding the goal costs 10 when the other passes at time 5.
        assert_eq!(found(plan_one(&graph, &table, 1, 2, soft)), [1, 2]);
        let overuse = Overuse::Vertex {
            vertex: 2,
            from: 5,
            to: 5,
            extra: 1,
            held: 1,
        };
        table.add_history(overuse, 7.5, false);
        // Now it costs 7.5 + 8.5 * 10 more, so it waits on its start to swap places with the
        // other, which costs 10.
        assert_eq!(
            found(plan_one(&graph, &table, 1, 2, soft)),
            [1, 1, 1, 1, 1, 1, 2]
        );
        assert!(matches!(
            plan_one(&graph, &table, 1, 2, Mode::Hard),
            Planned::NoPlan
        ));
    }

    #[test]
    fn conflicts_are_found_and_history_spans_a_hold_when_asked() {
        let graph = line(4);
        let mut table = Table::new(graph.vertex_count());
        // The first agent swaps with the second along 0-1, and then crosses 2, which the third
        // holds from the start.
        let first = [0, 1, 2, 3];
        table.add(0, &first);
        table.add(1, &[1, 0]);
        table.add(2, &[2]);
        let mut overuse = Vec::new();
        table.conflicts(&mut overuse);
        assert_eq!(
            overuse,
            [
                Overuse::Vertex {
                    vertex: 2,
                    from: 2,
                    to: 2,
                    extra: 1,
                    held: 0
                },
                Overuse::Edge {
                    low: 0,
                    high: 1,
                    time: 0,
                    extra: 1
                },
            ]
        );
        assert_eq!(overuse[0].resources(), 1);
        assert!(table.in_conflict(0, &first));
        assert!(table.in_conflict(2, &[2]));
        table.add_history(overuse[0], 1.0, false);
        assert_eq!(table.vertex_history[2], [(2, 1.0), (3, 0.0)]);
        table.add_history(overuse[0], 1.0, true);
        assert_eq!(table.vertex_history[2], [(0, 1.0), (2, 2.0), (3, 0.0)]);
        table.remove(0, &first);
        assert!(!table.in_conflict(1, &[1, 0]));
        overuse.clear();
        table.conflicts(&mut overuse);
        assert!(overuse.is_empty());
    }
}
