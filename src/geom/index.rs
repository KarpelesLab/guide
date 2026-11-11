//! A spatial index over rectangles, and one over shapes built on it.

use super::shape::Shape;
use super::types::{GeomError, MAX_COORD, Rect};

/// Most children per node. In `benches/geom.rs`, 8 and 16 were within a few percent of each
/// other, and 32 was slower.
const MAX_CHILDREN: usize = 8;
/// Fewest children per node, once removals are at work: a node left with fewer is taken
/// out, and its entries are inserted again.
const MIN_CHILDREN: usize = 3;

/// No node, or no leaf for a free slot.
const NONE: u32 = u32::MAX;

/// Identifies an entry of an index.
///
/// Identifiers order by slot, and slots are given out deterministically, so that the same
/// operations always give the same identifiers. A slot is reused after a removal, with a
/// new generation, so that an old identifier never finds the new entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryId {
    slot: u32,
    generation: u32,
}

impl EntryId {
    /// The slot, a small integer usable as an index into a table of the caller's. The
    /// entries of a bulk load have their positions. A slot may be reused after a removal.
    #[inline]
    pub const fn slot(self) -> u32 {
        self.slot
    }
}

#[derive(Clone, Debug)]
struct Slot {
    rect: Rect,
    generation: u32,
    /// The leaf holding this entry, or [`NONE`] if the slot is free.
    leaf: u32,
}

#[derive(Clone, Debug)]
struct Node {
    leaf: bool,
    len: u8,
    parent: u32,
    rects: [Rect; MAX_CHILDREN],
    /// Slots in a leaf, nodes otherwise.
    kids: [u32; MAX_CHILDREN],
}

impl Node {
    fn new(leaf: bool) -> Node {
        Node {
            leaf,
            len: 0,
            parent: NONE,
            rects: [Rect::default(); MAX_CHILDREN],
            kids: [NONE; MAX_CHILDREN],
        }
    }

    fn cover(&self) -> Rect {
        self.rects[..usize::from(self.len)]
            .iter()
            .copied()
            .reduce(|a, b| a.union(&b))
            .unwrap_or_default()
    }

    fn position(&self, kid: u32) -> Option<usize> {
        self.kids[..usize::from(self.len)]
            .iter()
            .position(|&k| k == kid)
    }

    fn remove(&mut self, at: usize) {
        let last = usize::from(self.len) - 1;
        self.rects[at] = self.rects[last];
        self.kids[at] = self.kids[last];
        self.len -= 1;
    }
}

/// A dynamic R-tree over rectangles, for finding what is near something.
///
/// It was chosen over a uniform grid by `benches/geom.rs`, for the reasons the
/// [module documentation](super#the-index) gives. Bulk loading packs the tree
/// sort-tile-recursively, insertion splits nodes by Guttman's quadratic method, and removal
/// takes out nodes left too small and inserts their entries again.
///
/// The tree's shape depends on the order of operations, but nothing visible does:
/// [`query`](SpatialIndex::query) sorts what it finds by identifier, and nothing is hashed.
///
/// ```
/// use guide::geom::{Point, Rect, SpatialIndex};
///
/// let square = |x, y| Rect::new(Point::new(x, y), Point::new(x + 10, y + 10));
/// let mut index = SpatialIndex::new();
/// let a = index.insert(square(0, 0));
/// let b = index.insert(square(100, 0));
///
/// let mut found = Vec::new();
/// index.query(&square(5, 5), &mut found);
/// assert_eq!(found, [a]);
///
/// assert!(index.remove(a));
/// index.query(&square(5, 5), &mut found);
/// assert!(found.is_empty());
/// assert_eq!(index.len(), 1);
/// assert_eq!(index.rect(b), Some(square(100, 0)));
/// ```
#[derive(Clone, Debug)]
pub struct SpatialIndex {
    nodes: Vec<Node>,
    free_nodes: Vec<u32>,
    root: u32,
    slots: Vec<Slot>,
    free_slots: Vec<u32>,
    len: usize,
}

impl Default for SpatialIndex {
    fn default() -> SpatialIndex {
        SpatialIndex::new()
    }
}

impl SpatialIndex {
    /// An empty index.
    pub fn new() -> SpatialIndex {
        SpatialIndex {
            nodes: vec![Node::new(true)],
            free_nodes: Vec::new(),
            root: 0,
            slots: Vec::new(),
            free_slots: Vec::new(),
            len: 0,
        }
    }

    /// An index of these rectangles, the `k`th in slot `k`. This is faster than inserting
    /// them one at a time, and gives a tree that answers queries faster.
    pub fn bulk_load(rects: impl IntoIterator<Item = Rect>) -> SpatialIndex {
        let slots: Vec<Slot> = rects
            .into_iter()
            .map(|rect| Slot {
                rect,
                generation: 0,
                leaf: NONE,
            })
            .collect();
        let mut index = SpatialIndex {
            nodes: Vec::new(),
            free_nodes: Vec::new(),
            root: 0,
            len: slots.len(),
            slots,
            free_slots: Vec::new(),
        };
        if index.slots.is_empty() {
            index.nodes.push(Node::new(true));
            return index;
        }
        // The children of the level being built: their rectangles, and slots or nodes.
        let mut level: Vec<(Rect, u32)> = (0..index.slots.len())
            .map(|i| (index.slots[i].rect, i as u32))
            .collect();
        let mut leaf = true;
        loop {
            tile(&mut level);
            let mut parents = Vec::with_capacity(level.len().div_ceil(MAX_CHILDREN));
            for group in level.chunks(MAX_CHILDREN) {
                let id = index.nodes.len() as u32;
                let mut node = Node::new(leaf);
                for (i, &(rect, kid)) in group.iter().enumerate() {
                    node.rects[i] = rect;
                    node.kids[i] = kid;
                    index.adopt(id, kid, leaf);
                }
                node.len = group.len() as u8;
                parents.push((node.cover(), id));
                index.nodes.push(node);
            }
            if parents.len() == 1 {
                index.root = parents[0].1;
                return index;
            }
            level = parents;
            leaf = false;
        }
    }

    /// How many entries there are.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether there are no entries.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The rectangle of an entry, if it is still there.
    pub fn rect(&self, id: EntryId) -> Option<Rect> {
        self.slots
            .get(id.slot as usize)
            .filter(|s| s.leaf != NONE && s.generation == id.generation)
            .map(|s| s.rect)
    }

    /// All entries, ordered by identifier.
    pub fn ids(&self) -> impl Iterator<Item = EntryId> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            (s.leaf != NONE).then_some(EntryId {
                slot: i as u32,
                generation: s.generation,
            })
        })
    }

    /// Replaces the contents of `out` with the entries whose rectangles share a point with
    /// `rect`, ordered by identifier. Touching counts.
    pub fn query(&self, rect: &Rect, out: &mut Vec<EntryId>) {
        out.clear();
        self.visit(self.root, rect, out);
        out.sort_unstable();
    }

    fn visit(&self, node: u32, rect: &Rect, out: &mut Vec<EntryId>) {
        let node = &self.nodes[node as usize];
        for i in 0..usize::from(node.len) {
            if node.rects[i].intersects(rect) {
                if node.leaf {
                    let slot = node.kids[i];
                    out.push(EntryId {
                        slot,
                        generation: self.slots[slot as usize].generation,
                    });
                } else {
                    self.visit(node.kids[i], rect, out);
                }
            }
        }
    }

    /// Adds a rectangle.
    pub fn insert(&mut self, rect: Rect) -> EntryId {
        let slot = match self.free_slots.pop() {
            Some(slot) => {
                self.slots[slot as usize].rect = rect;
                slot
            }
            None => {
                self.slots.push(Slot {
                    rect,
                    generation: 0,
                    leaf: NONE,
                });
                (self.slots.len() - 1) as u32
            }
        };
        self.len += 1;
        self.place(slot);
        EntryId {
            slot,
            generation: self.slots[slot as usize].generation,
        }
    }

    /// Removes an entry, and returns whether it was there.
    pub fn remove(&mut self, id: EntryId) -> bool {
        if self.rect(id).is_none() {
            return false;
        }
        let slot = &mut self.slots[id.slot as usize];
        let leaf = slot.leaf;
        slot.leaf = NONE;
        slot.generation = slot.generation.wrapping_add(1);
        self.free_slots.push(id.slot);
        self.len -= 1;
        let node = &mut self.nodes[leaf as usize];
        if let Some(at) = node.position(id.slot) {
            node.remove(at);
        }
        self.condense(leaf);
        true
    }

    /// Walks up from a node that lost a child, tightening the boxes above it and taking
    /// out nodes left too small, and then inserts their entries again.
    fn condense(&mut self, start: u32) {
        let mut orphans: Vec<u32> = Vec::new();
        let mut node = start;
        while node != self.root {
            let parent = self.nodes[node as usize].parent;
            let at = self.nodes[parent as usize].position(node);
            if usize::from(self.nodes[node as usize].len) < MIN_CHILDREN {
                if let Some(at) = at {
                    self.nodes[parent as usize].remove(at);
                }
                self.collect(node, &mut orphans);
            } else if let Some(at) = at {
                self.nodes[parent as usize].rects[at] = self.nodes[node as usize].cover();
            }
            node = parent;
        }
        // A root with one child adds nothing, and one with none has to become a leaf.
        loop {
            let root = &mut self.nodes[self.root as usize];
            if root.leaf || root.len > 1 {
                break;
            }
            if root.len == 0 {
                root.leaf = true;
                break;
            }
            let child = root.kids[0];
            self.free_nodes.push(self.root);
            self.root = child;
            self.nodes[child as usize].parent = NONE;
        }
        // Sorted, so that the result doesn't depend on the order of the walk.
        orphans.sort_unstable();
        for slot in orphans {
            self.place(slot);
        }
    }

    /// Collects the slots under a node, and frees it and every node below it.
    fn collect(&mut self, node: u32, out: &mut Vec<u32>) {
        let n = &self.nodes[node as usize];
        let (leaf, len, kids) = (n.leaf, usize::from(n.len), n.kids);
        if leaf {
            out.extend_from_slice(&kids[..len]);
        } else {
            for &kid in &kids[..len] {
                self.collect(kid, out);
            }
        }
        self.free_nodes.push(node);
    }

    fn new_node(&mut self, leaf: bool) -> u32 {
        match self.free_nodes.pop() {
            Some(id) => {
                self.nodes[id as usize] = Node::new(leaf);
                id
            }
            None => {
                self.nodes.push(Node::new(leaf));
                (self.nodes.len() - 1) as u32
            }
        }
    }

    /// Puts an occupied slot into the tree.
    fn place(&mut self, slot: u32) {
        let rect = self.slots[slot as usize].rect;
        let mut at = self.root;
        while !self.nodes[at as usize].leaf {
            let node = &mut self.nodes[at as usize];
            let i = choose_child(node, &rect);
            node.rects[i] = node.rects[i].union(&rect);
            at = node.kids[i];
        }
        self.add_child(at, rect, slot);
    }

    /// Adds a child to a node, splitting the node if it is full, and its parent likewise.
    fn add_child(&mut self, at: u32, rect: Rect, kid: u32) {
        let node = &mut self.nodes[at as usize];
        let (leaf, parent, n) = (node.leaf, node.parent, usize::from(node.len));
        if n < MAX_CHILDREN {
            node.rects[n] = rect;
            node.kids[n] = kid;
            node.len += 1;
            self.adopt(at, kid, leaf);
            return;
        }
        let mut all: Vec<(Rect, u32)> = (0..MAX_CHILDREN)
            .map(|i| (node.rects[i], node.kids[i]))
            .collect();
        all.push((rect, kid));
        let (one, two) = quadratic_split(&all);
        let other = self.new_node(leaf);
        for (id, group) in [(at, &one), (other, &two)] {
            let node = &mut self.nodes[id as usize];
            node.len = group.len() as u8;
            for (i, &k) in group.iter().enumerate() {
                (node.rects[i], node.kids[i]) = all[k];
            }
            for &k in group {
                self.adopt(id, all[k].1, leaf);
            }
        }
        let cover_at = self.nodes[at as usize].cover();
        let cover_other = self.nodes[other as usize].cover();
        if parent == NONE {
            let root = self.new_node(false);
            let node = &mut self.nodes[root as usize];
            (node.rects[0], node.kids[0]) = (cover_at, at);
            (node.rects[1], node.kids[1]) = (cover_other, other);
            node.len = 2;
            self.nodes[at as usize].parent = root;
            self.nodes[other as usize].parent = root;
            self.root = root;
        } else {
            let p = &mut self.nodes[parent as usize];
            if let Some(i) = p.position(at) {
                p.rects[i] = cover_at;
            }
            self.add_child(parent, cover_other, other);
        }
    }

    /// Records that `kid`, a slot or a node, now belongs to `node`.
    fn adopt(&mut self, node: u32, kid: u32, leaf: bool) {
        if leaf {
            self.slots[kid as usize].leaf = node;
        } else {
            self.nodes[kid as usize].parent = node;
        }
    }
}

/// Orders children for packing: into vertical slices by centre, and then by centre within
/// each. The sorts are stable, so that ties keep their order.
fn tile(items: &mut [(Rect, u32)]) {
    let n = items.len();
    if n <= MAX_CHILDREN {
        return;
    }
    let centre_x = |r: &Rect| r.min().x as i128 + r.max().x as i128;
    let centre_y = |r: &Rect| r.min().y as i128 + r.max().y as i128;
    items.sort_by_key(|(r, _)| centre_x(r));
    let leaves = n.div_ceil(MAX_CHILDREN);
    // The square root, rounded up, in integers so that no float has a say in the tree.
    let mut slices = 1;
    while slices * slices < leaves {
        slices += 1;
    }
    for slice in items.chunks_mut(slices * MAX_CHILDREN) {
        slice.sort_by_key(|(r, _)| centre_y(r));
    }
}

/// The child that would grow least to hold `rect`, then the smallest, then the first.
fn choose_child(node: &Node, rect: &Rect) -> usize {
    let mut best = 0;
    let mut best_key = (i128::MAX, i128::MAX);
    for (i, r) in node.rects[..usize::from(node.len)].iter().enumerate() {
        let area = r.area();
        let key = (r.union(rect).area().saturating_sub(area), area);
        if key < best_key {
            (best, best_key) = (i, key);
        }
    }
    best
}

/// Guttman's quadratic split: two groups of positions in `items`, each with at least
/// [`MIN_CHILDREN`].
fn quadratic_split(items: &[(Rect, u32)]) -> (Vec<usize>, Vec<usize>) {
    // The seeds are the pair that would waste the most area together.
    let (mut seed_one, mut seed_two) = (0, 1);
    let mut worst = i128::MIN;
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            let (a, b) = (&items[i].0, &items[j].0);
            let waste = a
                .union(b)
                .area()
                .saturating_sub(a.area())
                .saturating_sub(b.area());
            if waste > worst {
                (worst, seed_one, seed_two) = (waste, i, j);
            }
        }
    }
    let (mut one, mut two) = (vec![seed_one], vec![seed_two]);
    let (mut cover_one, mut cover_two) = (items[seed_one].0, items[seed_two].0);
    let mut rest: Vec<usize> = (0..items.len())
        .filter(|&i| i != seed_one && i != seed_two)
        .collect();
    let growth = |cover: &Rect, r: &Rect| cover.union(r).area().saturating_sub(cover.area());
    while !rest.is_empty() {
        // What is left may all be needed to bring a group up to the minimum.
        if one.len() + rest.len() <= MIN_CHILDREN {
            one.append(&mut rest);
            break;
        }
        if two.len() + rest.len() <= MIN_CHILDREN {
            two.append(&mut rest);
            break;
        }
        // Next is the item that cares most which group it joins.
        let mut pick = 0;
        let mut strongest = None;
        for (k, &i) in rest.iter().enumerate() {
            let r = &items[i].0;
            let preference = growth(&cover_one, r).abs_diff(growth(&cover_two, r));
            if Some(preference) > strongest {
                (pick, strongest) = (k, Some(preference));
            }
        }
        let i = rest.remove(pick);
        let r = &items[i].0;
        let key_one = (growth(&cover_one, r), cover_one.area(), one.len());
        let key_two = (growth(&cover_two, r), cover_two.area(), two.len());
        if key_one <= key_two {
            one.push(i);
            cover_one = cover_one.union(r);
        } else {
            two.push(i);
            cover_two = cover_two.union(r);
        }
    }
    (one, two)
}

/// A [`SpatialIndex`] that holds shapes, and finds those within a clearance of another.
///
/// ```
/// use guide::geom::{Point, Shape, ShapeIndex};
///
/// let mut copper = ShapeIndex::new();
/// let a = copper.insert(Shape::circle(Point::new(0, 0), 100)?);
/// let b = copper.insert(Shape::circle(Point::new(1000, 0), 100)?);
///
/// // 200 from the centre of `a`, less radii of 50 and 50, leaves a gap of 100.
/// let track = Shape::track(Point::new(-500, 200), Point::new(500, 200), 100)?;
/// let mut hits = Vec::new();
/// copper.within(&track, 101, &mut hits)?;
/// assert_eq!(hits, [a]);
/// copper.within(&track, 100, &mut hits)?;
/// assert!(hits.is_empty());
///
/// copper.remove(a);
/// copper.within(&track, 1000, &mut hits)?;
/// assert_eq!(hits, [b]);
/// # Ok::<(), guide::geom::GeomError>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct ShapeIndex {
    index: SpatialIndex,
    /// By slot.
    shapes: Vec<Option<Shape>>,
}

impl ShapeIndex {
    /// An empty index.
    pub fn new() -> ShapeIndex {
        ShapeIndex::default()
    }

    /// An index of these shapes, the `k`th in slot `k`.
    pub fn bulk_load(shapes: impl IntoIterator<Item = Shape>) -> ShapeIndex {
        let shapes: Vec<Option<Shape>> = shapes.into_iter().map(Some).collect();
        let index = SpatialIndex::bulk_load(shapes.iter().flatten().map(Shape::bbox));
        ShapeIndex { index, shapes }
    }

    /// How many shapes there are.
    #[inline]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether there are no shapes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// All entries, ordered by identifier.
    pub fn ids(&self) -> impl Iterator<Item = EntryId> + '_ {
        self.index.ids()
    }

    /// Adds a shape.
    pub fn insert(&mut self, shape: Shape) -> EntryId {
        let id = self.index.insert(shape.bbox());
        let slot = id.slot as usize;
        if slot >= self.shapes.len() {
            self.shapes.resize(slot + 1, None);
        }
        self.shapes[slot] = Some(shape);
        id
    }

    /// Removes an entry and returns its shape, if it was there.
    pub fn remove(&mut self, id: EntryId) -> Option<Shape> {
        if self.index.remove(id) {
            self.shapes.get_mut(id.slot as usize)?.take()
        } else {
            None
        }
    }

    /// The shape of an entry, if it is still there.
    pub fn get(&self, id: EntryId) -> Option<&Shape> {
        self.index.rect(id)?;
        self.shapes.get(id.slot as usize)?.as_ref()
    }

    /// Replaces the contents of `out` with the entries whose boxes share a point with
    /// `rect`, ordered by identifier.
    pub fn query(&self, rect: &Rect, out: &mut Vec<EntryId>) {
        self.index.query(rect, out);
    }

    /// Replaces the contents of `out` with the entries that `shape` is closer to than
    /// `clearance`, as [`Shape::violates`] decides, ordered by identifier.
    ///
    /// # Errors
    ///
    /// The [`GeomError`] that `violates` would give for `clearance`, leaving `out` empty.
    pub fn within(
        &self,
        shape: &Shape,
        clearance: i64,
        out: &mut Vec<EntryId>,
    ) -> Result<(), GeomError> {
        out.clear();
        if !(0..=MAX_COORD).contains(&clearance) {
            return Err(GeomError::InvalidDimension {
                what: "clearance",
                value: clearance,
            });
        }
        // Shapes closer than the clearance have boxes closer than it, so this box meets
        // every box that matters.
        self.index.query(&shape.bbox().expand(clearance), out);
        let mut kept = 0;
        for i in 0..out.len() {
            let id = out[i];
            if let Some(other) = self.get(id)
                && shape.violates(other, clearance)?
            {
                out[kept] = id;
                kept += 1;
            }
        }
        out.truncate(kept);
        Ok(())
    }
}
