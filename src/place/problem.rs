//! The placement problem: items, nets, and where items may go.

use core::fmt;

use super::{ItemId, MacroId, Point, Rect, RegionId, SiteId};
use crate::id::NetId;

/// A quarter turn, in millidegrees.
const QUARTER_TURN: i32 = 90_000;
/// A whole turn, in millidegrees.
const WHOLE_TURN: i32 = 360_000;

/// Rotates `point` about the origin by `rotation` millidegrees, counterclockwise, Y up.
///
/// Only quarter turns are supported, which are exact. Rotations are normalized first, so
/// `-90_000` is the same as `270_000`. Anything that is not a whole number of quarter turns
/// leaves the point as it is: [`PlacementProblem::validate`] refuses such rotations, so they
/// never reach here from a placer.
///
/// # Examples
///
/// ```
/// use guide::place::{Point, rotate};
///
/// assert_eq!(rotate(Point::new(2, 1), 90_000), Point::new(-1, 2));
/// assert_eq!(rotate(Point::new(2, 1), -90_000), Point::new(1, -2));
/// ```
pub fn rotate(point: Point, rotation: i32) -> Point {
    match normalize_rotation(rotation) {
        90_000 => Point::new(point.y.saturating_neg(), point.x),
        180_000 => Point::new(point.x.saturating_neg(), point.y.saturating_neg()),
        270_000 => Point::new(point.y, point.x.saturating_neg()),
        _ => point,
    }
}

/// Puts a rotation in `[0, 360_000)`.
pub(crate) fn normalize_rotation(rotation: i32) -> i32 {
    rotation.rem_euclid(WHOLE_TURN)
}

/// Rotates a rectangle about the origin by quarter turns.
pub(crate) fn rotate_rect(rect: &Rect, rotation: i32) -> Rect {
    Rect::new(rotate(rect.min, rotation), rotate(rect.max, rotation))
}

/// A discrete slot that one item of the same kind can occupy.
///
/// Several sites can share a position, as the sub-slots of an FPGA tile do: items on them
/// are at the same point for wirelength, and each site still holds one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Site {
    /// Where an item on this site has its origin.
    pub at: Point,
    /// What kind of item it takes: an item only goes on a site of its own kind.
    pub kind: u32,
}

impl Site {
    /// Creates a site.
    pub const fn new(at: Point, kind: u32) -> Site {
        Site { at, kind }
    }
}

/// Where items may go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Discrete, typed slots, one item each, as on an FPGA. Items' outlines are ignored.
    Sites(Vec<Site>),
    /// A rectangle that items' outlines have to lie in without overlapping, as on a PCB.
    FreeSpace {
        /// The area items' outlines must stay in.
        area: Rect,
        /// The step on which placers put items' origins, at least 1. Moves and the
        /// legalizer's search go by whole multiples of it, from the origin of coordinates.
        /// Fixed items can be anywhere.
        pitch: i64,
    },
}

/// Where a fixed item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fixed {
    /// On a site, for a problem laid out in sites.
    Site(SiteId),
    /// At an origin and rotation, for a problem in free space.
    At {
        /// Where its origin is.
        origin: Point,
        /// Its rotation, in millidegrees.
        rotation: i32,
    },
}

/// Something to place: a logic block, a component.
///
/// Create one with [`Item::new`] and the builder methods.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub(crate) kind: u32,
    pub(crate) outline: Rect,
    pub(crate) rotations: Vec<i32>,
    pub(crate) region: Option<RegionId>,
    pub(crate) fixed: Option<Fixed>,
}

impl Default for Item {
    fn default() -> Self {
        Item::new()
    }
}

impl Item {
    /// A movable item of kind 0, with an empty outline at its origin, that can't rotate.
    pub fn new() -> Item {
        Item {
            kind: 0,
            outline: Rect::default(),
            rotations: vec![0],
            region: None,
            fixed: None,
        }
    }

    /// Sets the kind of site the item needs. Ignored in free space.
    #[must_use]
    pub fn kind(mut self, kind: u32) -> Item {
        self.kind = kind;
        self
    }

    /// Sets the outline, relative to the item's origin, unrotated. Ignored on sites.
    #[must_use]
    pub fn outline(mut self, outline: Rect) -> Item {
        self.outline = outline;
        self
    }

    /// Sets the outline to a `width` by `height` rectangle whose lower left corner is the
    /// origin.
    #[must_use]
    pub fn size(self, width: i64, height: i64) -> Item {
        self.outline(Rect::with_size(0, 0, width, height))
    }

    /// Sets the rotations a placer may choose from, in millidegrees. Each has to be a
    /// multiple of 90 degrees. The first is the one used for an item in a macro, which
    /// doesn't rotate. An empty list means 0 only.
    #[must_use]
    pub fn rotations(mut self, rotations: &[i32]) -> Item {
        self.rotations = if rotations.is_empty() {
            vec![0]
        } else {
            rotations.to_vec()
        };
        self
    }

    /// Limits the item to a region: its origin on sites, or its whole outline in free space.
    #[must_use]
    pub fn region(mut self, region: RegionId) -> Item {
        self.region = Some(region);
        self
    }

    /// Fixes the item on a site.
    #[must_use]
    pub fn fixed_at_site(mut self, site: SiteId) -> Item {
        self.fixed = Some(Fixed::Site(site));
        self
    }

    /// Fixes the item at an origin and rotation in free space.
    #[must_use]
    pub fn fixed_at(mut self, origin: Point, rotation: i32) -> Item {
        self.fixed = Some(Fixed::At { origin, rotation });
        self
    }

    /// The kind of site the item needs.
    pub fn site_kind(&self) -> u32 {
        self.kind
    }

    /// The outline, relative to the origin, unrotated.
    pub fn unrotated_outline(&self) -> Rect {
        self.outline
    }

    /// The rotations allowed, the default first.
    pub fn allowed_rotations(&self) -> &[i32] {
        &self.rotations
    }

    /// The region the item is limited to, if any.
    pub fn limited_to(&self) -> Option<RegionId> {
        self.region
    }

    /// Where the item is fixed, if it is.
    pub fn fixed(&self) -> Option<Fixed> {
        self.fixed
    }

    /// Whether a placer may move the item.
    pub fn is_movable(&self) -> bool {
        self.fixed.is_none()
    }
}

/// One end of a net: a point on an item, relative to the item's origin, unrotated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pin {
    /// The item.
    pub item: ItemId,
    /// Where on the item, before the item's rotation.
    pub offset: Point,
}

impl Pin {
    /// A pin at the item's origin.
    pub const fn new(item: ItemId) -> Pin {
        Pin {
            item,
            offset: Point::new(0, 0),
        }
    }

    /// A pin at an offset from the item's origin.
    pub const fn at(item: ItemId, x: i64, y: i64) -> Pin {
        Pin {
            item,
            offset: Point::new(x, y),
        }
    }
}

/// Why a [`PlacementProblem`] is malformed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProblemError {
    /// A pin, macro or fixed location names an item, site or region that doesn't exist.
    UnknownReference,
    /// A rotation is not a whole number of quarter turns.
    UnsupportedRotation {
        /// The item.
        item: ItemId,
        /// The rotation.
        rotation: i32,
    },
    /// A fixed item is fixed the wrong way for the layout, or on a site of another kind.
    BadFixedLocation(ItemId),
    /// An item is in more than one macro.
    InTwoMacros(ItemId),
    /// A macro holds a fixed item, or no item at all.
    BadMacro(MacroId),
    /// Two fixed items are on the same site.
    SiteTakenTwice(SiteId),
    /// There are more items of a kind than sites for them.
    NotEnoughSites {
        /// The kind.
        kind: u32,
        /// Items of that kind.
        items: usize,
        /// Sites of that kind.
        sites: usize,
    },
    /// The free space's pitch is below 1, or its area is empty.
    BadFreeSpace,
}

impl fmt::Display for ProblemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProblemError::UnknownReference => {
                write!(f, "an item, site or region that doesn't exist is named")
            }
            ProblemError::UnsupportedRotation { item, rotation } => write!(
                f,
                "{item:?} may rotate by {rotation} millidegrees, which isn't a quarter turn"
            ),
            ProblemError::BadFixedLocation(item) => {
                write!(f, "{item:?} is fixed somewhere it can't be")
            }
            ProblemError::InTwoMacros(item) => write!(f, "{item:?} is in two macros"),
            ProblemError::BadMacro(id) => {
                write!(f, "{id:?} is empty or holds a fixed item")
            }
            ProblemError::SiteTakenTwice(site) => {
                write!(f, "two fixed items are on {site:?}")
            }
            ProblemError::NotEnoughSites { kind, items, sites } => write!(
                f,
                "{items} items of kind {kind} need a site, and there are {sites}"
            ),
            ProblemError::BadFreeSpace => write!(f, "the free space is empty or its pitch is 0"),
        }
    }
}

impl std::error::Error for ProblemError {}

/// A placement problem.
///
/// Build one with [`new`](PlacementProblem::new) and the `add_` methods. Nothing is checked
/// as it is added: [`validate`](PlacementProblem::validate) does that, and placers call it.
///
/// # Examples
///
/// Two items on a row of three sites, with a fixed pad at one end:
///
/// ```
/// use guide::place::{
///     AnnealOptions, Item, Layout, Pin, PlacementProblem, Point, SiteId, Wirelength, anneal,
///     check,
/// };
///
/// let sites = (0..4).map(|x| guide::place::Site::new(Point::new(x, 0), 0)).collect();
/// let mut problem = PlacementProblem::new(Layout::Sites(sites));
/// let pad = problem.add_item(Item::new().fixed_at_site(SiteId(0)));
/// let a = problem.add_item(Item::new());
/// let b = problem.add_item(Item::new());
/// problem.add_net(1, &[Pin::new(pad), Pin::new(a)]);
/// problem.add_net(1, &[Pin::new(a), Pin::new(b)]);
///
/// let outcome = anneal(&problem, &Wirelength, AnnealOptions::default())?;
/// assert!(check(&problem, &outcome.placement).is_empty());
/// assert_eq!(outcome.wirelength, 2);
/// # Ok::<(), guide::place::PlaceError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementProblem {
    layout: Layout,
    items: Vec<Item>,
    net_weights: Vec<u32>,
    net_starts: Vec<u32>,
    pins: Vec<Pin>,
    macros: Vec<Vec<(ItemId, Point)>>,
    regions: Vec<Rect>,
}

impl PlacementProblem {
    /// An empty problem on a layout.
    pub fn new(layout: Layout) -> PlacementProblem {
        PlacementProblem {
            layout,
            items: Vec::new(),
            net_weights: Vec::new(),
            net_starts: vec![0],
            pins: Vec::new(),
            macros: Vec::new(),
            regions: Vec::new(),
        }
    }

    /// Adds an item.
    pub fn add_item(&mut self, item: Item) -> ItemId {
        self.items.push(item);
        ItemId(self.items.len() as u32 - 1)
    }

    /// Adds a net between pins, whose cost is multiplied by `weight`. A net with fewer than
    /// two pins costs nothing.
    pub fn add_net(&mut self, weight: u32, pins: &[Pin]) -> NetId {
        self.pins.extend_from_slice(pins);
        self.net_starts.push(self.pins.len() as u32);
        self.net_weights.push(weight);
        NetId(self.net_weights.len() as u32 - 1)
    }

    /// Ties items together: each member's origin stays at its offset from the others', so
    /// that the group moves as one and never rotates. Members keep their first allowed
    /// rotation. On sites, each member still needs a site of its own kind at its offset.
    pub fn add_macro(&mut self, members: &[(ItemId, Point)]) -> MacroId {
        self.macros.push(members.to_vec());
        MacroId(self.macros.len() as u32 - 1)
    }

    /// Adds a region, which items can then be limited to with [`Item::region`]. On sites the
    /// rectangle holds the origins of the sites allowed; in free space, the outlines.
    pub fn add_region(&mut self, area: Rect) -> RegionId {
        self.regions.push(area);
        RegionId(self.regions.len() as u32 - 1)
    }

    /// The layout.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The sites, which are empty in free space.
    pub fn sites(&self) -> &[Site] {
        match &self.layout {
            Layout::Sites(sites) => sites,
            Layout::FreeSpace { .. } => &[],
        }
    }

    /// The items.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// One item.
    ///
    /// # Panics
    ///
    /// If `item` is not one of the problem's.
    pub fn item(&self, item: ItemId) -> &Item {
        &self.items[item.index()]
    }

    /// How many nets there are.
    pub fn net_count(&self) -> usize {
        self.net_weights.len()
    }

    /// A net's pins.
    ///
    /// # Panics
    ///
    /// If `net` is not one of the problem's.
    pub fn net_pins(&self, net: NetId) -> &[Pin] {
        let start = self.net_starts[net.index()] as usize;
        let end = self.net_starts[net.index() + 1] as usize;
        &self.pins[start..end]
    }

    /// A net's weight.
    ///
    /// # Panics
    ///
    /// If `net` is not one of the problem's.
    pub fn net_weight(&self, net: NetId) -> u32 {
        self.net_weights[net.index()]
    }

    /// The macros, each as its members and their offsets.
    pub fn macros(&self) -> &[Vec<(ItemId, Point)>] {
        &self.macros
    }

    /// The regions.
    pub fn regions(&self) -> &[Rect] {
        &self.regions
    }

    /// The bounding box of where items may go: of the sites, or the free space's area.
    pub fn extent(&self) -> Rect {
        match &self.layout {
            Layout::Sites(sites) => sites.iter().skip(1).fold(
                sites.first().map_or(Rect::default(), |s| Rect::point(s.at)),
                |r, s| r.including(s.at),
            ),
            Layout::FreeSpace { area, .. } => *area,
        }
    }

    /// Where a pin is, for the item's place and rotation in `placement`.
    ///
    /// # Panics
    ///
    /// If the pin's item is not one of the placement's.
    pub fn pin_position(&self, placement: &Placement, pin: Pin) -> Point {
        let item = pin.item.index();
        placement.origins[item].offset(rotate(pin.offset, placement.rotations[item]))
    }

    /// The bounding box of a net's pins, or `None` for a net without pins.
    ///
    /// # Panics
    ///
    /// If `net` is not one of the problem's, or a pin's item is not one of the placement's.
    pub fn net_box(&self, placement: &Placement, net: NetId) -> Option<Rect> {
        let mut pins = self.net_pins(net).iter();
        let first = Rect::point(self.pin_position(placement, *pins.next()?));
        Some(pins.fold(first, |r, &pin| {
            r.including(self.pin_position(placement, pin))
        }))
    }

    /// The total weighted half-perimeter wirelength, saturating. This is the cost that
    /// [`Wirelength`](super::Wirelength) gives, computed directly.
    pub fn wirelength(&self, placement: &Placement) -> u64 {
        (0..self.net_count() as u32)
            .map(|net| {
                let net = NetId(net);
                self.net_box(placement, net).map_or(0, |r| {
                    r.half_perimeter()
                        .saturating_mul(u64::from(self.net_weight(net)))
                })
            })
            .fold(0u64, u64::saturating_add)
    }

    /// The outline an item would have with its origin at `origin`, rotated by `rotation`.
    ///
    /// # Panics
    ///
    /// If `item` is not one of the problem's.
    pub fn outline_at(&self, item: ItemId, origin: Point, rotation: i32) -> Rect {
        rotate_rect(&self.items[item.index()].outline, rotation).translate(origin)
    }

    /// Which macro each item is in, if any. Items in two macros are refused by `validate`,
    /// and here only the last counts.
    pub(crate) fn macro_of(&self) -> Vec<Option<MacroId>> {
        let mut of = vec![None; self.items.len()];
        for (m, members) in self.macros.iter().enumerate() {
            for &(item, _) in members {
                if let Some(slot) = of.get_mut(item.index()) {
                    *slot = Some(MacroId(m as u32));
                }
            }
        }
        of
    }

    /// Checks that the problem is well formed, and that there are enough sites for every
    /// kind of item.
    ///
    /// # Errors
    ///
    /// The first [`ProblemError`] found: a reference to an item, site or region that doesn't
    /// exist, a free space with no area or pitch, a rotation that isn't a quarter turn, an
    /// item fixed where it can't be, a bad macro, two fixed items on one site, or a kind of
    /// item with too few sites.
    pub fn validate(&self) -> Result<(), ProblemError> {
        let items = self.items.len();
        if self.pins.iter().any(|pin| pin.item.index() >= items) {
            return Err(ProblemError::UnknownReference);
        }
        let sites = self.sites();
        let free = matches!(self.layout, Layout::FreeSpace { .. });
        if let Layout::FreeSpace { area, pitch } = &self.layout
            && (*pitch < 1 || area.width() == 0 || area.height() == 0)
        {
            return Err(ProblemError::BadFreeSpace);
        }
        let mut fixed_on = vec![false; sites.len()];
        for (i, item) in self.items.iter().enumerate() {
            let id = ItemId(i as u32);
            if item.region.is_some_and(|r| r.index() >= self.regions.len()) {
                return Err(ProblemError::UnknownReference);
            }
            for &rotation in &item.rotations {
                if rotation % QUARTER_TURN != 0 {
                    return Err(ProblemError::UnsupportedRotation { item: id, rotation });
                }
            }
            match item.fixed {
                None => {}
                Some(Fixed::Site(site)) => {
                    let Some(s) = sites.get(site.index()) else {
                        return Err(ProblemError::BadFixedLocation(id));
                    };
                    if free || s.kind != item.kind {
                        return Err(ProblemError::BadFixedLocation(id));
                    }
                    if core::mem::replace(&mut fixed_on[site.index()], true) {
                        return Err(ProblemError::SiteTakenTwice(site));
                    }
                }
                Some(Fixed::At { rotation, .. }) => {
                    if !free {
                        return Err(ProblemError::BadFixedLocation(id));
                    }
                    if rotation % QUARTER_TURN != 0 {
                        return Err(ProblemError::UnsupportedRotation { item: id, rotation });
                    }
                }
            }
        }
        let mut in_macro = vec![false; items];
        for (m, members) in self.macros.iter().enumerate() {
            if members.is_empty() {
                return Err(ProblemError::BadMacro(MacroId(m as u32)));
            }
            for &(item, _) in members {
                let Some(seen) = in_macro.get_mut(item.index()) else {
                    return Err(ProblemError::UnknownReference);
                };
                if core::mem::replace(seen, true) {
                    return Err(ProblemError::InTwoMacros(item));
                }
                if self.items[item.index()].fixed.is_some() {
                    return Err(ProblemError::BadMacro(MacroId(m as u32)));
                }
            }
        }
        if !free {
            let mut need = std::collections::BTreeMap::<u32, (usize, usize)>::new();
            for item in &self.items {
                need.entry(item.kind).or_default().0 += 1;
            }
            for site in sites {
                need.entry(site.kind).or_default().1 += 1;
            }
            for (kind, (items, sites)) in need {
                if items > sites {
                    return Err(ProblemError::NotEnoughSites { kind, items, sites });
                }
            }
        }
        Ok(())
    }
}

/// Where each item of a problem is: its origin and rotation, and on sites, its site.
///
/// A placement only makes sense with the problem it was made for. [`check`](super::check)
/// says whether it is legal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Placement {
    pub(crate) origins: Vec<Point>,
    pub(crate) rotations: Vec<i32>,
    pub(crate) sites: Vec<Option<SiteId>>,
}

impl Placement {
    /// A placement of every item at the origin of coordinates, at its first rotation, and on
    /// no site, with fixed items where they are fixed.
    pub fn new(problem: &PlacementProblem) -> Placement {
        let n = problem.items.len();
        let mut placement = Placement {
            origins: vec![Point::default(); n],
            rotations: problem.items.iter().map(|item| item.rotations[0]).collect(),
            sites: vec![None; n],
        };
        for (i, item) in problem.items.iter().enumerate() {
            match item.fixed {
                Some(Fixed::Site(site)) => {
                    if let Some(s) = problem.sites().get(site.index()) {
                        placement.origins[i] = s.at;
                        placement.sites[i] = Some(site);
                    }
                }
                Some(Fixed::At { origin, rotation }) => {
                    placement.origins[i] = origin;
                    placement.rotations[i] = rotation;
                }
                None => {}
            }
        }
        placement
    }

    /// The item's origin.
    ///
    /// # Panics
    ///
    /// If `item` is not one of the placement's.
    pub fn origin(&self, item: ItemId) -> Point {
        self.origins[item.index()]
    }

    /// The item's rotation, in millidegrees.
    ///
    /// # Panics
    ///
    /// If `item` is not one of the placement's.
    pub fn rotation(&self, item: ItemId) -> i32 {
        self.rotations[item.index()]
    }

    /// The item's site, on a problem laid out in sites.
    ///
    /// # Panics
    ///
    /// If `item` is not one of the placement's.
    pub fn site(&self, item: ItemId) -> Option<SiteId> {
        self.sites[item.index()]
    }

    /// Puts an item on a site, which also sets its origin to the site's.
    ///
    /// # Panics
    ///
    /// If the site isn't one of the problem's, or `item` not one of the placement's.
    pub fn set_site(&mut self, problem: &PlacementProblem, item: ItemId, site: SiteId) {
        self.origins[item.index()] = problem.sites()[site.index()].at;
        self.sites[item.index()] = Some(site);
    }

    /// Puts an item at an origin and rotation, for free space.
    ///
    /// # Panics
    ///
    /// If `item` is not one of the placement's.
    pub fn set_origin(&mut self, item: ItemId, origin: Point, rotation: i32) {
        self.origins[item.index()] = origin;
        self.rotations[item.index()] = rotation;
    }

    /// How many items there are.
    pub fn len(&self) -> usize {
        self.origins.len()
    }

    /// Whether there are no items.
    pub fn is_empty(&self) -> bool {
        self.origins.is_empty()
    }
}
