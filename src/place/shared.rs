//! What the placers keep of [`Shared`](super::Shared) resources: the values drawn from each,
//! updated as items move, so that a move is judged by the resources its items' sites draw on.

use super::{ItemId, PlacementProblem, SiteId};

/// The values drawn from each shared resource by the items placed so far.
#[derive(Clone, Debug)]
pub(crate) struct SharedState {
    /// By site, where its draws start in `draws`, and one past the last site.
    site_start: Vec<u32>,
    /// `(resource, key, whether a source)` for each resource a site is a member of.
    draws: Vec<(u32, u32, bool)>,
    /// By resource, how many of its sources hold an item that draws a value.
    supplied: Vec<u32>,
    /// By resource, its capacity.
    capacity: Vec<u32>,
    /// By resource, each distinct value drawn and how many items draw it.
    counts: Vec<Vec<(u32, u32)>>,
    /// Resources an item was added to by the last [`SharedState::shift`].
    added: Vec<u32>,
}

/// One item's move: the item, its site before and its site after.
pub(crate) type Shift = (ItemId, Option<SiteId>, Option<SiteId>);

impl SharedState {
    /// The state with nothing placed, or `None` if the problem has no shared resources.
    pub fn new(problem: &PlacementProblem) -> Option<SharedState> {
        let shared = problem.shared();
        if shared.is_empty() {
            return None;
        }
        let sites = problem.sites().len();
        let mut lists: Vec<Vec<(u32, u32, bool)>> = vec![Vec::new(); sites];
        for (r, resource) in shared.iter().enumerate() {
            let members = resource.members.iter().map(|&m| (m, false));
            let sources = resource.sources.iter().map(|&m| (m, true));
            for ((site, key), source) in members.chain(sources) {
                if let Some(list) = lists.get_mut(site.index()) {
                    list.push((r as u32, key, source));
                }
            }
        }
        let mut site_start = Vec::with_capacity(sites + 1);
        let mut draws = Vec::new();
        site_start.push(0);
        for list in lists {
            draws.extend(list);
            site_start.push(draws.len() as u32);
        }
        Some(SharedState {
            site_start,
            draws,
            capacity: shared.iter().map(|r| r.capacity).collect(),
            supplied: vec![0; shared.len()],
            counts: vec![Vec::new(); shared.len()],
            added: Vec::new(),
        })
    }

    /// The state for a placement: every item that is on a site, added.
    pub fn of(
        problem: &PlacementProblem,
        sites: impl Iterator<Item = (ItemId, SiteId)>,
    ) -> Option<SharedState> {
        let mut state = SharedState::new(problem)?;
        for (item, site) in sites {
            state.add(problem, item, site);
        }
        Some(state)
    }

    fn site_draws(&self, site: SiteId) -> core::ops::Range<usize> {
        self.site_start[site.index()] as usize..self.site_start[site.index() + 1] as usize
    }

    /// Adds an item on a site, noting the resources it draws a value from.
    fn add(&mut self, problem: &PlacementProblem, item: ItemId, site: SiteId) {
        for d in self.site_draws(site) {
            let (resource, key, source) = self.draws[d];
            let Some(value) = problem.drawn(item, key) else {
                continue;
            };
            self.supplied[resource as usize] += u32::from(source);
            let counts = &mut self.counts[resource as usize];
            match counts.iter_mut().find(|(v, _)| *v == value) {
                Some((_, count)) => *count += 1,
                None => counts.push((value, 1)),
            }
            self.added.push(resource);
        }
    }

    /// Takes an item off a site.
    fn remove(&mut self, problem: &PlacementProblem, item: ItemId, site: SiteId) {
        for d in self.site_draws(site) {
            let (resource, key, source) = self.draws[d];
            let Some(value) = problem.drawn(item, key) else {
                continue;
            };
            self.supplied[resource as usize] -= u32::from(source);
            let counts = &mut self.counts[resource as usize];
            if let Some(at) = counts.iter().position(|(v, _)| *v == value) {
                counts[at].1 -= 1;
                if counts[at].1 == 0 {
                    counts.swap_remove(at);
                }
            }
        }
    }

    /// Applies moves, all items off their old sites first, and returns whether every resource
    /// an item was added to is within its capacity. The moves stay applied either way:
    /// [`SharedState::unshift`] takes them back.
    pub fn shift(&mut self, problem: &PlacementProblem, moves: &[Shift]) -> bool {
        for &(item, from, _) in moves {
            if let Some(site) = from {
                self.remove(problem, item, site);
            }
        }
        self.added.clear();
        for &(item, _, to) in moves {
            if let Some(site) = to {
                self.add(problem, item, site);
            }
        }
        self.added.iter().all(|&r| {
            let r = r as usize;
            self.counts[r].len() as u32 <= self.capacity[r] + self.supplied[r]
        })
    }

    /// Takes back moves that [`SharedState::shift`] applied.
    pub fn unshift(&mut self, problem: &PlacementProblem, moves: &[Shift]) {
        for &(item, _, to) in moves {
            if let Some(site) = to {
                self.remove(problem, item, site);
            }
        }
        for &(item, from, _) in moves {
            if let Some(site) = from {
                self.add(problem, item, site);
            }
        }
    }

    /// Whether the moves would leave every resource an item is added to within its
    /// capacity, leaving nothing applied.
    pub fn allows(&mut self, problem: &PlacementProblem, moves: &[Shift]) -> bool {
        let ok = self.shift(problem, moves);
        self.unshift(problem, moves);
        ok
    }
}
