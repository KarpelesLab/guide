//! The space that routes are searched in.
//!
//! [`RoutingSpace`] is the boundary between the engines and the domains: an engine
//! knows nothing else about what it is routing through.

use core::fmt::Debug;
use core::hash::Hash;
use core::marker::PhantomData;
use std::collections::HashMap;

use crate::cost::Cost;
use crate::id::NetId;

pub mod graph;
pub mod grid;

/// One move out of a node, as produced by [`RoutingSpace::expand`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step<N> {
    /// The node this step arrives at.
    pub to: N,
    /// The cost of taking the step, before any congestion is priced in.
    pub cost: Cost,
}

/// A space that nets can be routed through.
///
/// Implementations must be deterministic: the same call on the same space must produce
/// the same results in the same order, because engines break ties by that order.
pub trait RoutingSpace {
    /// A search state, such as a device wire or a position on a layer.
    type Node: Copy + Eq + Hash + Debug;
    /// Something nets compete for.
    type Resource: Copy + Eq + Ord + Hash + Debug;
    /// How search state is keyed for this space's nodes.
    type Slots: NodeSlots<Self::Node>;

    /// Creates an empty mapping from this space's nodes to search state slots.
    fn new_slots(&self) -> Self::Slots;

    /// Appends the steps `net` may take from `from` to `out`.
    ///
    /// `out` may already hold entries, which must be left in place. Spaces should not
    /// clear it, so that callers can reuse its allocation across calls.
    fn expand(&self, net: NetId, from: Self::Node, out: &mut Vec<Step<Self::Node>>);

    /// Returns a lower bound on the cost for `net` to get from `from` to `to`.
    ///
    /// The result must never exceed the true remaining cost, or searches lose their
    /// optimality guarantee. [`Cost::ZERO`] is always correct, and turns A\* into
    /// Dijkstra's algorithm.
    fn lower_bound(&self, net: NetId, from: Self::Node, to: Self::Node) -> Cost;

    /// Appends the resources `net` consumes by stepping from `from` to `to` to `out`.
    ///
    /// As with [`expand`](RoutingSpace::expand), existing entries must be left in place.
    fn resources(
        &self,
        net: NetId,
        from: Self::Node,
        to: Self::Node,
        out: &mut Vec<Self::Resource>,
    );

    /// Returns how many nets can share `resource` before it counts as overused.
    fn capacity(&self, resource: Self::Resource) -> u32;
}

/// Maps a space's nodes to the dense indices that search state is stored under.
///
/// Searches keep flat arrays indexed by slot, so slots should be small and densely
/// packed. A slot must stay the same for a given node until [`clear`](NodeSlots::clear).
/// The arrays grow as needed, so spaces do not have to know how many slots they will use.
pub trait NodeSlots<N> {
    /// Returns the slot for `node`, assigning one if needed.
    fn slot(&mut self, node: N) -> usize;

    /// Forgets assignments, ready for a new search.
    ///
    /// Search state is discarded separately, so mappings with fixed assignments can do
    /// nothing here.
    fn clear(&mut self);
}

/// Slots for nodes that already are dense indices, such as prebuilt graph nodes.
#[derive(Debug)]
pub struct DenseSlots<N>(PhantomData<fn(N)>);

impl<N> DenseSlots<N> {
    /// Creates the mapping, which holds no state.
    #[inline]
    pub const fn new() -> Self {
        DenseSlots(PhantomData)
    }
}

impl<N> Default for DenseSlots<N> {
    #[inline]
    fn default() -> Self {
        DenseSlots::new()
    }
}

impl<N: Into<usize>> NodeSlots<N> for DenseSlots<N> {
    #[inline]
    fn slot(&mut self, node: N) -> usize {
        node.into()
    }

    #[inline]
    fn clear(&mut self) {}
}

/// Slots for nodes generated on demand, numbered in the order they are first seen.
///
/// The map is only ever looked up and never iterated, so its randomized iteration order
/// cannot affect results.
#[derive(Debug)]
pub struct InternedSlots<N> {
    slots: HashMap<N, usize>,
}

impl<N> InternedSlots<N> {
    /// Creates an empty mapping.
    #[inline]
    pub fn new() -> Self {
        InternedSlots {
            slots: HashMap::new(),
        }
    }
}

impl<N> Default for InternedSlots<N> {
    #[inline]
    fn default() -> Self {
        InternedSlots::new()
    }
}

impl<N: Eq + Hash> NodeSlots<N> for InternedSlots<N> {
    #[inline]
    fn slot(&mut self, node: N) -> usize {
        let next = self.slots.len();
        *self.slots.entry(node).or_insert(next)
    }

    #[inline]
    fn clear(&mut self) {
        self.slots.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NodeId;

    #[test]
    fn dense_slots_use_the_index() {
        let mut slots = DenseSlots::new();
        assert_eq!(slots.slot(NodeId(9)), 9);
        slots.clear();
        assert_eq!(slots.slot(NodeId(9)), 9);
    }

    #[test]
    fn interned_slots_number_by_first_sight_until_cleared() {
        let mut slots = InternedSlots::new();
        assert_eq!(slots.slot((5, 5)), 0);
        assert_eq!(slots.slot((1, 2)), 1);
        assert_eq!(slots.slot((5, 5)), 0);
        slots.clear();
        assert_eq!(slots.slot((1, 2)), 0);
    }
}
