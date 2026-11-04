//! The space that routes are searched in.
//!
//! [`RoutingSpace`] is the boundary between the engines and the domains: an engine
//! knows nothing else about what it is routing through.

use core::fmt::Debug;
use core::hash::{BuildHasherDefault, Hash, Hasher};
use core::marker::PhantomData;
use std::collections::{HashMap, HashSet};

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
    /// How negotiation state is keyed for this space's resources.
    ///
    /// Engines keep one array entry for every slot up to the highest in use, so a space
    /// with a dense numbering of its resources should use [`DenseSlots`], and any other
    /// should use [`InternedSlots`]. [`DenseSlots`] on resources numbered far apart costs
    /// memory for all of the numbers between them.
    type ResourceSlots: NodeSlots<Self::Resource>;

    /// Creates an empty mapping from this space's nodes to search state slots.
    fn new_slots(&self) -> Self::Slots;

    /// Creates an empty mapping from this space's resources to negotiation state slots.
    ///
    /// An engine keeps one mapping for a whole run. It assigns a slot when a net first
    /// occupies a resource, and only reads the mapping while routing, so a resource that
    /// has no slot is one that no net has ever occupied.
    fn new_resource_slots(&self) -> Self::ResourceSlots;

    /// Returns whether `node` exists in this space.
    ///
    /// Searches ignore sources and targets for which this is false, engines report a net
    /// with a terminal made up of such nodes as having no path, and the validator treats a
    /// route on one as a fault. A node can exist and be unusable: a blocked cell of a
    /// [`Grid`](grid::Grid) exists, and has a capacity of 0.
    ///
    /// This must be true of every node that [`expand`](RoutingSpace::expand) produces, and
    /// a space with no limit on its nodes can answer true for all of them.
    fn contains(&self, node: Self::Node) -> bool;

    /// Appends the steps `net` may take from `from` to `out`.
    ///
    /// `out` may already hold entries, which must be left in place. Spaces should not
    /// clear it, so that callers can reuse its allocation across calls.
    ///
    /// Routes identify a step by its two nodes, so a call may produce at most one step to
    /// any given node, and none back to `from`. A space with parallel connections offers
    /// the one it prefers.
    ///
    /// Callers only ask about nodes the space [contains](RoutingSpace::contains).
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
    ///
    /// No step enters the node a route starts from, so a call with `from == to` asks for
    /// the resources `net` consumes by occupying that node.
    ///
    /// This may be called for any pair of nodes, including pairs that are not steps,
    /// because the validator checks routes that may be wrong.
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

/// A space that knows how long its steps take, which is what timing-driven routing needs.
///
/// This is a trait of its own because [`Step::cost`] means whatever the space wants it to
/// mean, and most spaces have no delays at all. Nothing about [`RoutingSpace`] changes, and a
/// space without timing never pays for this.
///
/// Delays are in `u32` picoseconds, as in [`timing`](crate::timing), and **each step has a
/// fixed delay**. It can't depend on the rest of the route, so a delay that really depends on
/// the tree around it, as an Elmore delay does, has to be approximated by one number per step.
/// The [engine](crate::engine#timing-driven-routing) says what that means for routes, and
/// [`TimedGraph`](graph::TimedGraph) is a ready-made implementation.
pub trait DelaySpace: RoutingSpace {
    /// Appends the delay of each step that [`expand`](RoutingSpace::expand) produces for the
    /// same arguments to `out`, in the same order and as many of them.
    ///
    /// As with [`expand`](RoutingSpace::expand), existing entries must be left in place. This
    /// answers for every step out of a node at once because that is what a search asks for,
    /// and a space stored by edges can then answer without looking any of them up.
    fn step_delays(&self, net: NetId, from: Self::Node, out: &mut Vec<u32>);

    /// Returns a lower bound on the delay of any path for `net` from `from` to `to`.
    ///
    /// The result must never exceed the smallest sum of step delays between the two. The
    /// default of 0 is always correct. A search for a critical connection is then guided only
    /// by the weight it has left on cost, which at a criticality of 0.99 is not much.
    #[inline]
    fn delay_lower_bound(&self, net: NetId, from: Self::Node, to: Self::Node) -> u32 {
        let _ = (net, from, to);
        0
    }
}

/// Maps a space's nodes or resources to the dense indices that state is stored under.
///
/// Users keep flat arrays indexed by slot, so slots should be small and densely packed. A
/// slot must stay the same for a given key until [`clear`](NodeSlots::clear). The arrays
/// grow as needed, so spaces do not have to know how many slots they will use.
///
/// There are two ways to look up a slot, because there are two kinds of user:
///
/// - A search owns its mapping, and meets the nodes of a generated space as it goes. It
///   calls [`slot`](NodeSlots::slot), which assigns a slot on a miss, and so takes `&mut`.
/// - Pricing a step in an engine has to read a mapping that any number of searches may be
///   sharing, and nothing may change it while they do. It calls [`get`](NodeSlots::get),
///   which takes `&self` and never assigns. For an interned mapping a miss means that the
///   key has never been assigned a slot, and an engine only assigns one when a net occupies
///   a resource, which happens between searches. So a miss means "not occupied".
pub trait NodeSlots<N> {
    /// Returns the slot for `key`, assigning one if needed.
    fn slot(&mut self, key: N) -> usize;

    /// Returns the slot for `key` if it has one, and changes nothing.
    ///
    /// A mapping with fixed assignments always has one, even for a key that the space does
    /// not contain, so it is up to the caller to check that it lies within its arrays.
    fn get(&self, key: N) -> Option<usize>;

    /// Forgets assignments, ready for a new search.
    ///
    /// Search state is discarded separately, so mappings with fixed assignments can do
    /// nothing here. An engine never calls this on a mapping for resources, whose state
    /// lasts for the whole run.
    fn clear(&mut self);
}

/// Slots for nodes or resources that already are dense indices, such as prebuilt graph
/// nodes.
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
    fn slot(&mut self, key: N) -> usize {
        key.into()
    }

    #[inline]
    fn get(&self, key: N) -> Option<usize> {
        Some(key.into())
    }

    #[inline]
    fn clear(&mut self) {}
}

/// Slots for nodes or resources generated on demand, numbered in the order they are first
/// assigned.
///
/// The map is only ever looked up and never iterated, so neither its order nor its hash
/// values can affect results.
#[derive(Debug)]
pub struct InternedSlots<N> {
    slots: FastMap<N, usize>,
}

impl<N> InternedSlots<N> {
    /// Creates an empty mapping.
    #[inline]
    pub fn new() -> Self {
        InternedSlots {
            slots: FastMap::default(),
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
    fn slot(&mut self, key: N) -> usize {
        let next = self.slots.len();
        *self.slots.entry(key).or_insert(next)
    }

    #[inline]
    fn get(&self, key: N) -> Option<usize> {
        self.slots.get(&key).copied()
    }

    #[inline]
    fn clear(&mut self) {
        self.slots.clear();
    }
}

/// A map for hot lookups, keyed by small plain values and never iterated.
pub(crate) type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;
/// A set for hot lookups, keyed by small plain values and never iterated.
pub(crate) type FastSet<K> = HashSet<K, BuildHasherDefault<FastHasher>>;

/// A multiplicative hash, in the style of FxHash, for the lookups made once per search step.
///
/// The keys are node and resource identifiers chosen by the problem, and an adversary who
/// could choose them could only slow a run down, so std's collision-resistant hash is not
/// worth what it costs here. No result depends on the hash values, because these
/// tables are never iterated.
#[derive(Clone, Copy, Default)]
pub(crate) struct FastHasher(u64);

impl FastHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        const MULTIPLIER: u64 = 0xF135_7AEA_2E62_A9C5;
        self.0 = (self.0 ^ word).wrapping_mul(MULTIPLIER);
    }
}

impl Hasher for FastHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        // A multiplication leaves its best bits at the top and the hash table reads the
        // bottom ones for the bucket.
        self.0.rotate_left(26)
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
    fn dense_slots_have_a_slot_for_anything_without_assigning() {
        let slots = DenseSlots::new();
        assert_eq!(slots.get(NodeId(9)), Some(9));
        assert_eq!(slots.get(NodeId(u32::MAX)), Some(u32::MAX as usize));
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

    #[test]
    fn looking_up_an_interned_slot_does_not_assign_one() {
        let mut slots = InternedSlots::new();
        assert_eq!(slots.get((5, 5)), None);
        assert_eq!(slots.get((5, 5)), None);
        assert_eq!(slots.slot((1, 2)), 0);
        assert_eq!(slots.get((1, 2)), Some(0));
        assert_eq!(slots.get((5, 5)), None);
        // Had the misses above assigned anything, this would not be next.
        assert_eq!(slots.slot((5, 5)), 1);
        assert_eq!(slots.get((5, 5)), Some(1));
        slots.clear();
        assert_eq!(slots.get((1, 2)), None);
    }

    #[test]
    fn lookups_can_be_shared_between_threads() {
        let mut interned = InternedSlots::new();
        for key in 0..100 {
            interned.slot(NodeId(key * 7));
        }
        let dense = DenseSlots::new();
        let (interned, dense) = (&interned, &dense);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(move || {
                    for key in 0..100 {
                        assert_eq!(interned.get(NodeId(key * 7)), Some(key as usize));
                        assert_eq!(interned.get(NodeId(key * 7 + 1)), None);
                        assert_eq!(dense.get(NodeId(key)), Some(key as usize));
                    }
                });
            }
        });
    }

    #[test]
    fn hasher_treats_every_key_shape_consistently() {
        let hash = |value: &dyn Fn(&mut FastHasher)| {
            let mut hasher = FastHasher::default();
            value(&mut hasher);
            hasher.finish()
        };
        assert_eq!(hash(&|h| h.write_u32(7)), hash(&|h| h.write_u64(7)));
        assert_eq!(hash(&|h| h.write_u8(7)), hash(&|h| h.write_u16(7)));
        assert_eq!(hash(&|h| h.write_usize(7)), hash(&|h| h.write_u64(7)));
        assert_ne!(hash(&|h| h.write_u32(7)), hash(&|h| h.write_u32(8)));
        assert_eq!(
            hash(&|h| h.write(&[7, 0, 0, 0, 0, 0, 0, 0])),
            hash(&|h| h.write_u64(7))
        );
        assert_ne!(hash(&|h| h.write(&[1; 9])), hash(&|h| h.write(&[1; 8])));
    }
}
