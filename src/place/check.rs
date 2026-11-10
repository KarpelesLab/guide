//! Legality of a placement, checked independently of the placers.
//!
//! Nothing here is shared with the annealer or the legalizer: overlaps are found by a sweep
//! over sorted outlines, not by their spatial index, so that a bug in one isn't hidden by the
//! other.

use super::problem::normalize_rotation;
use super::{Fixed, ItemId, Layout, MacroId, Placement, PlacementProblem, SiteId};

/// Something illegal about a placement.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Violation {
    /// The placement is for a different number of items.
    WrongSize,
    /// On sites, an item has no site, or a site that doesn't exist, or its origin is not
    /// its site's.
    NotOnSite(ItemId),
    /// An item is on a site of another kind.
    WrongKind(ItemId),
    /// Two items share a site.
    SharedSite(SiteId),
    /// A fixed item is not where it is fixed.
    Moved(ItemId),
    /// An item has a rotation it isn't allowed.
    BadRotation(ItemId),
    /// In free space, an item's outline leaves the area.
    OutsideArea(ItemId),
    /// In free space, two items' outlines overlap.
    Overlap(ItemId, ItemId),
    /// An item is outside the region it is limited to.
    OutsideRegion(ItemId),
    /// A macro's members are not at their offsets from each other, or not at their first
    /// rotation.
    BrokenMacro(MacroId),
}

/// Lists everything illegal about `placement`, or nothing if it is legal.
///
/// # Examples
///
/// ```
/// use guide::place::{Item, Layout, Placement, PlacementProblem, Point, Rect, Violation, check};
///
/// let area = Rect::with_size(0, 0, 10, 10);
/// let mut problem = PlacementProblem::new(Layout::FreeSpace { area, pitch: 1 });
/// let a = problem.add_item(Item::new().size(4, 4));
/// let b = problem.add_item(Item::new().size(4, 4));
///
/// let mut placement = Placement::new(&problem);
/// placement.set_origin(b, Point::new(2, 2), 0);
/// assert_eq!(check(&problem, &placement), [Violation::Overlap(a, b)]);
///
/// placement.set_origin(b, Point::new(4, 0), 0);
/// assert!(check(&problem, &placement).is_empty());
/// ```
pub fn check(problem: &PlacementProblem, placement: &Placement) -> Vec<Violation> {
    let mut found = Vec::new();
    let n = problem.items().len();
    if placement.origins.len() != n || placement.rotations.len() != n || placement.sites.len() != n
    {
        return vec![Violation::WrongSize];
    }

    for (i, item) in problem.items().iter().enumerate() {
        let id = ItemId(i as u32);
        let rotation = placement.rotations[i];
        match item.fixed {
            Some(Fixed::Site(site)) => {
                if placement.sites[i] != Some(site)
                    || normalize_rotation(rotation) != normalize_rotation(item.rotations[0])
                {
                    found.push(Violation::Moved(id));
                }
            }
            Some(Fixed::At {
                origin,
                rotation: fixed_rotation,
            }) => {
                if placement.origins[i] != origin
                    || normalize_rotation(rotation) != normalize_rotation(fixed_rotation)
                {
                    found.push(Violation::Moved(id));
                }
            }
            None => {
                if !item
                    .rotations
                    .iter()
                    .any(|&r| normalize_rotation(r) == normalize_rotation(rotation))
                {
                    found.push(Violation::BadRotation(id));
                }
            }
        }
    }

    match problem.layout() {
        Layout::Sites(sites) => {
            let mut holder: Vec<Option<ItemId>> = vec![None; sites.len()];
            for (i, item) in problem.items().iter().enumerate() {
                let id = ItemId(i as u32);
                let Some(site) = placement.sites[i] else {
                    found.push(Violation::NotOnSite(id));
                    continue;
                };
                let Some(s) = sites.get(site.index()) else {
                    found.push(Violation::NotOnSite(id));
                    continue;
                };
                if s.at != placement.origins[i] {
                    found.push(Violation::NotOnSite(id));
                }
                if s.kind != item.kind {
                    found.push(Violation::WrongKind(id));
                }
                if holder[site.index()].replace(id).is_some() {
                    found.push(Violation::SharedSite(site));
                }
                if let Some(region) = item.region
                    && !problem.regions()[region.index()].contains(s.at)
                {
                    found.push(Violation::OutsideRegion(id));
                }
            }
        }
        Layout::FreeSpace { area, .. } => {
            let mut outlines = Vec::with_capacity(n);
            for (i, item) in problem.items().iter().enumerate() {
                let id = ItemId(i as u32);
                let outline = problem.outline_at(id, placement.origins[i], placement.rotations[i]);
                if !area.contains_rect(&outline) {
                    found.push(Violation::OutsideArea(id));
                }
                if let Some(region) = item.region
                    && !problem.regions()[region.index()].contains_rect(&outline)
                {
                    found.push(Violation::OutsideRegion(id));
                }
                if outline.width() > 0 && outline.height() > 0 {
                    outlines.push((outline, id));
                }
            }
            // Sweep from left to right, keeping the outlines that the sweep line crosses.
            outlines.sort_by_key(|&(r, id)| (r.min.x, id));
            let mut active: Vec<(super::Rect, ItemId)> = Vec::new();
            let mut overlaps = Vec::new();
            for &(r, id) in &outlines {
                active.retain(|(a, _)| a.max.x > r.min.x);
                for &(a, other) in &active {
                    if a.overlaps(&r) {
                        overlaps.push(if other < id { (other, id) } else { (id, other) });
                    }
                }
                active.push((r, id));
            }
            overlaps.sort();
            found.extend(overlaps.into_iter().map(|(a, b)| Violation::Overlap(a, b)));
        }
    }

    for (m, members) in problem.macros().iter().enumerate() {
        let Some(&(first, first_offset)) = members.first() else {
            continue;
        };
        let base = placement.origins[first.index()].minus(first_offset);
        let broken = members.iter().any(|&(item, offset)| {
            placement.origins[item.index()] != base.offset(offset)
                || normalize_rotation(placement.rotations[item.index()])
                    != normalize_rotation(problem.item(item).rotations[0])
        });
        if broken {
            found.push(Violation::BrokenMacro(MacroId(m as u32)));
        }
    }
    found
}
