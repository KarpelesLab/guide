//! Legalization: from wished-for positions to a legal placement that moves them little.
//!
//! On sites this is Tetris-like: macros first, from the bottom up, then single items from left
//! to right, each taking the free site of its kind nearest to where it wants to be. In free
//! space it is the same greedy idea, largest outline first, searching outward from each item's
//! target on the pitch lattice for the nearest spot that overlaps nothing placed so far. Either
//! way, items limited to a region go first, smallest region first, so that others don't take
//! their room.
//!
//! In free space, greedy choices leave gaps that nothing fits when space is tight, so rows
//! after Abacus, in the `abacus` module, are tried too, four ways, and the legal result with the
//! shortest wirelength wins.
//!
//! Searches go in rings of growing distance, counted in the grid of the item's kind or in
//! pitches, and stop one ring after the first that has room. Nearest is by Manhattan distance,
//! with ties to the lowest site or the lowest coordinates, so results never depend on anything
//! but the problem and the targets.

use super::abacus::{Rows, legalize_rows};
use super::netlist::{KindGrid, Netlist, Unit};
use super::problem::rotate_rect;
use super::spatial::SpatialIndex;
use super::{ItemId, Layout, PlaceError, Placement, PlacementProblem, Point, Rect, SiteId};

/// Makes a legal placement with each movable item near `targets[item]`, an origin.
///
/// Fixed items stay where they are fixed, and their targets are ignored. A macro goes where
/// its first member's target puts it. In free space, items keep the rotation in
/// `rotations[item]` when it is one they are allowed, or else their first, and macros their
/// first.
///
/// # Errors
///
/// [`PlaceError::Problem`] if the problem is malformed, and [`PlaceError::NoRoom`] if an item
/// finds no legal spot, which in free space can happen before space runs out, when neither
/// greedy packing nor rows leave a gap it fits.
///
/// # Panics
///
/// If `targets` or `rotations` has fewer entries than the problem has items.
///
/// # Examples
///
/// ```
/// use guide::place::{Item, Layout, PlacementProblem, Point, Site, check, legalize};
///
/// let sites = (0..3).map(|x| Site::new(Point::new(x, 0), 0)).collect();
/// let mut problem = PlacementProblem::new(Layout::Sites(sites));
/// let a = problem.add_item(Item::new());
/// let b = problem.add_item(Item::new());
///
/// // Both want the same spot.
/// let placement = legalize(&problem, &[Point::new(1, 0), Point::new(1, 0)], &[0, 0])?;
/// assert!(check(&problem, &placement).is_empty());
/// assert_eq!(placement.origin(a), Point::new(1, 0));
/// assert_eq!(placement.origin(b), Point::new(0, 0));
/// # Ok::<(), guide::place::PlaceError>(())
/// ```
pub fn legalize(
    problem: &PlacementProblem,
    targets: &[Point],
    rotations: &[i32],
) -> Result<Placement, PlaceError> {
    problem.validate()?;
    let netlist = Netlist::new(problem);
    let anchors: Vec<Point> = netlist
        .units
        .iter()
        .map(|unit| {
            let (first, offset) = unit.members[0];
            targets[first.index()].minus(offset)
        })
        .collect();
    let unit_rotations: Vec<i32> = netlist
        .units
        .iter()
        .map(|unit| {
            let item = unit.members[0].0;
            let allowed = &problem.item(item).rotations;
            if unit.is_macro || !allowed.contains(&rotations[item.index()]) {
                allowed[0]
            } else {
                rotations[item.index()]
            }
        })
        .collect();
    legalize_units(problem, &netlist, &anchors, &unit_rotations)
}

/// Legalizes units towards anchor targets, with a rotation for each unit.
///
/// Greedy placement near targets can leave room only in gaps too small for what comes next:
/// columns cut too short for the macros still to come, or space between outlines. In free
/// space, rows after Abacus are tried as well, along x and along y, with each unit in the row
/// where it moves least or with rows filled in order, and the result with the shortest
/// wirelength, greedy or by rows, wins. If none is legal, this tries again packing. On sites,
/// macros go largest first, each to the lowest room near its column. In free space, units go
/// from left to right, each to the leftmost spot with room in the row where that is nearest its
/// target, as in the Tetris legalizer, and if that fails too, largest first to the lowest and
/// then leftmost spot, which is bottom-left packing.
pub(crate) fn legalize_units(
    problem: &PlacementProblem,
    netlist: &Netlist,
    anchors: &[Point],
    rotations: &[i32],
) -> Result<Placement, PlaceError> {
    let free = matches!(problem.layout(), Layout::FreeSpace { .. });
    let mut result = legalize_once(problem, netlist, anchors, rotations, Mode::Nearest);
    if free {
        // Rows both ways, each unit to its nearest row or rows filled in order, and the greedy
        // result if there is one: the shortest wins.
        let mut best = result
            .as_ref()
            .ok()
            .map(|p| (problem.wirelength(p), p.clone()));
        for (rows, balanced) in [
            (Rows::Horizontal, false),
            (Rows::Vertical, false),
            (Rows::Horizontal, true),
            (Rows::Vertical, true),
        ] {
            if let Some(p) = legalize_rows(problem, netlist, anchors, rotations, rows, balanced) {
                let w = problem.wirelength(&p);
                if best.as_ref().is_none_or(|(b, _)| w < *b) {
                    best = Some((w, p));
                }
            }
        }
        if let Some((_, p)) = best {
            return Ok(p);
        }
    }
    if matches!(result, Err(PlaceError::NoRoom(_))) && (free || !problem.macros().is_empty()) {
        result = legalize_once(problem, netlist, anchors, rotations, Mode::Tetris);
    }
    if matches!(result, Err(PlaceError::NoRoom(_))) && free {
        result = legalize_once(problem, netlist, anchors, rotations, Mode::BottomLeft);
    }
    result
}

/// How [`legalize_once`] chooses spots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Nearest,
    Tetris,
    BottomLeft,
}

fn legalize_once(
    problem: &PlacementProblem,
    netlist: &Netlist,
    anchors: &[Point],
    rotations: &[i32],
    mode: Mode,
) -> Result<Placement, PlaceError> {
    let bottom = problem.extent().min.y;
    let mut placement = Placement::new(problem);
    match problem.layout() {
        Layout::Sites(sites) => {
            let mut taken = vec![false; sites.len()];
            for item in problem.items() {
                if let Some(super::Fixed::Site(site)) = item.fixed {
                    taken[site.index()] = true;
                }
            }
            let mut order: Vec<usize> = (0..netlist.units.len()).collect();
            // Macros go from the bottom up, and single items from left to right.
            order.sort_by_key(|&u| {
                let unit = &netlist.units[u];
                let (a, b) = if unit.is_macro {
                    (anchors[u].y, anchors[u].x)
                } else {
                    (anchors[u].x, anchors[u].y)
                };
                let size = if mode != Mode::Nearest {
                    usize::MAX - unit.members.len()
                } else {
                    0
                };
                // Items limited to a region go before others can take its room.
                let room = unit
                    .members
                    .iter()
                    .filter_map(|&(item, _)| problem.item(item).region)
                    .map(|region| problem.regions()[region.index()].area())
                    .min()
                    .unwrap_or(u64::MAX);
                (!unit.is_macro, size, room, a, b, u)
            });
            for u in order {
                let unit = &netlist.units[u];
                let target = if mode != Mode::Nearest && unit.is_macro {
                    Point::new(anchors[u].x, bottom)
                } else {
                    anchors[u]
                };
                let chosen = nearest_sites(problem, netlist, unit, target, &taken)
                    .ok_or(PlaceError::NoRoom(unit.members[0].0))?;
                for (&(item, _), site) in unit.members.iter().zip(chosen) {
                    taken[site.index()] = true;
                    placement.set_site(problem, item, site);
                    placement.rotations[item.index()] = if unit.is_macro {
                        problem.item(item).rotations[0]
                    } else {
                        rotations[u]
                    };
                }
            }
        }
        Layout::FreeSpace { area, pitch } => {
            let mut index = SpatialIndex::new(*area, problem.items().len(), typical_size(problem));
            for (i, item) in problem.items().iter().enumerate() {
                if item.fixed.is_some() {
                    let id = ItemId(i as u32);
                    let outline =
                        problem.outline_at(id, placement.origins[i], placement.rotations[i]);
                    index.insert(i as u32, outline);
                }
            }
            let mut order: Vec<usize> = (0..netlist.units.len()).collect();
            let areas: Vec<u64> = (0..netlist.units.len())
                .map(|u| unit_box(problem, &netlist.units[u], rotations[u]).area())
                .collect();
            let rooms: Vec<u64> = netlist
                .units
                .iter()
                .map(|unit| {
                    unit.members
                        .iter()
                        .filter_map(|&(item, _)| problem.item(item).region)
                        .map(|region| problem.regions()[region.index()].area())
                        .min()
                        .unwrap_or(u64::MAX)
                })
                .collect();
            match mode {
                Mode::Nearest => order.sort_by_key(|&u| {
                    (rooms[u], u64::MAX - areas[u], anchors[u].x, anchors[u].y, u)
                }),
                Mode::Tetris => {
                    order.sort_by_key(|&u| (rooms[u], anchors[u].x, anchors[u].y, u));
                }
                Mode::BottomLeft => order.sort_by_key(|&u| {
                    (rooms[u], u64::MAX - areas[u], anchors[u].x, anchors[u].y, u)
                }),
            }
            for u in order {
                let unit = &netlist.units[u];
                let spot = if mode != Mode::Nearest {
                    let target = (mode == Mode::Tetris).then_some(anchors[u]);
                    packed_spot(problem, unit, rotations[u], target, *area, *pitch, &index)
                } else {
                    nearest_free_spot(
                        problem,
                        unit,
                        rotations[u],
                        anchors[u],
                        *area,
                        *pitch,
                        &index,
                    )
                };
                let anchor = spot.ok_or(PlaceError::NoRoom(unit.members[0].0))?;
                for &(item, offset) in &unit.members {
                    let rotation = if unit.is_macro {
                        problem.item(item).rotations[0]
                    } else {
                        rotations[u]
                    };
                    let origin = anchor.offset(offset);
                    placement.set_origin(item, origin, rotation);
                    let outline = problem.outline_at(item, origin, rotation);
                    index.insert(item.0, outline);
                }
            }
        }
    }
    Ok(placement)
}

/// A typical side of an outline, for sizing bins.
pub(crate) fn typical_size(problem: &PlacementProblem) -> i64 {
    let items = problem.items();
    if items.is_empty() {
        return 1;
    }
    let total: u64 = items
        .iter()
        .map(|item| item.outline.width().max(item.outline.height()))
        .fold(0, u64::saturating_add);
    (total / items.len() as u64).max(1) as i64
}

/// The bounding box of a unit's outlines, relative to its anchor.
pub(crate) fn unit_box(problem: &PlacementProblem, unit: &Unit, rotation: i32) -> Rect {
    let mut bounds: Option<Rect> = None;
    for &(item, offset) in &unit.members {
        let r = if unit.is_macro {
            problem.item(item).rotations[0]
        } else {
            rotation
        };
        let outline = rotate_rect(&problem.item(item).outline, r).translate(offset);
        bounds = Some(bounds.map_or(outline, |b| b.union(&outline)));
    }
    bounds.unwrap_or_default()
}

/// Snaps an anchor to the unit's lattice: the one on which the left and bottom of its
/// bounding box are whole pitches from those of the area.
pub(crate) fn snap(anchor: Point, bounds: &Rect, area: &Rect, pitch: i64) -> Point {
    let base_x = area.min.x - bounds.min.x;
    let base_y = area.min.y - bounds.min.y;
    let round = |v: i64, base: i64| {
        let steps = (v - base + pitch / 2).div_euclid(pitch);
        base + steps * pitch
    };
    Point::new(round(anchor.x, base_x), round(anchor.y, base_y))
}

/// Whether a unit fits with its anchor here: inside the area and its regions, overlapping
/// nothing in the index but its own members.
pub(crate) fn unit_fits(
    problem: &PlacementProblem,
    unit: &Unit,
    rotation: i32,
    anchor: Point,
    area: &Rect,
    index: &SpatialIndex,
) -> bool {
    unit.members.iter().all(|&(item, offset)| {
        let r = if unit.is_macro {
            problem.item(item).rotations[0]
        } else {
            rotation
        };
        let outline = problem.outline_at(item, anchor.offset(offset), r);
        area.contains_rect(&outline)
            && problem
                .item(item)
                .region
                .is_none_or(|region| problem.regions()[region.index()].contains_rect(&outline))
            && !index.overlaps(&outline, |other| {
                unit.members.iter().any(|&(member, _)| member.0 == other)
            })
    })
}

/// Visits the cells at Chebyshev distance `r` from `(cx, cy)` in an `nx` by `ny` grid.
pub(crate) fn ring(
    nx: usize,
    ny: usize,
    cx: usize,
    cy: usize,
    r: usize,
    mut visit: impl FnMut(usize, usize),
) {
    let (cx, cy, r, nx, ny) = (cx as i64, cy as i64, r as i64, nx as i64, ny as i64);
    let mut cell = |x: i64, y: i64| {
        if (0..nx).contains(&x) && (0..ny).contains(&y) {
            visit(x as usize, y as usize);
        }
    };
    if r == 0 {
        cell(cx, cy);
        return;
    }
    for x in cx - r..=cx + r {
        cell(x, cy - r);
        cell(x, cy + r);
    }
    for y in cy - r + 1..cy + r {
        cell(cx - r, y);
        cell(cx + r, y);
    }
}

/// The sites for a unit's members, with its anchor nearest to `target`, or `None`.
pub(crate) fn nearest_sites(
    problem: &PlacementProblem,
    netlist: &Netlist,
    unit: &Unit,
    target: Point,
    taken: &[bool],
) -> Option<Vec<SiteId>> {
    let sites = problem.sites();
    let (first, first_offset) = unit.members[0];
    let grid: &KindGrid = &netlist.grids[netlist.grid_of(problem.item(first).kind)?];
    let want = target.offset(first_offset);
    let (cx, cy) = (grid.nearest_column(want.x), grid.nearest_row(want.y));
    let reach = grid.xs.len().max(grid.ys.len());
    let mut best: Option<(u64, SiteId, Vec<SiteId>)> = None;
    let mut found_at = None;
    let mut members = Vec::with_capacity(unit.members.len());
    for r in 0..reach {
        if found_at.is_some_and(|f| r > f + 1) {
            break;
        }
        ring(grid.xs.len(), grid.ys.len(), cx, cy, r, |x, y| {
            for &site in grid.cell(x, y) {
                if taken[site.index()] {
                    continue;
                }
                let anchor = sites[site.index()].at.minus(first_offset);
                if !site_unit_fits(problem, netlist, unit, site, anchor, taken, &mut members) {
                    continue;
                }
                let distance = anchor.manhattan(target);
                if best
                    .as_ref()
                    .is_none_or(|(d, s, _)| (distance, site) < (*d, *s))
                {
                    best = Some((distance, site, members.clone()));
                }
            }
        });
        if best.is_some() && found_at.is_none() {
            found_at = Some(r);
        }
    }
    best.map(|(_, _, members)| members)
}

/// Whether a unit fits with its first member on `first_site`, filling `members` with the
/// site of each member if it does.
pub(crate) fn site_unit_fits(
    problem: &PlacementProblem,
    netlist: &Netlist,
    unit: &Unit,
    first_site: SiteId,
    anchor: Point,
    taken: &[bool],
    members: &mut Vec<SiteId>,
) -> bool {
    members.clear();
    let sites = problem.sites();
    let sub = netlist.site_cell[first_site.index()].2 as usize;
    for (i, &(item, offset)) in unit.members.iter().enumerate() {
        let it = problem.item(item);
        let site = if i == 0 {
            first_site
        } else {
            let Some(g) = netlist.grid_of(it.kind) else {
                return false;
            };
            let grid = &netlist.grids[g];
            let Some((x, y)) = grid.locate(anchor.offset(offset)) else {
                return false;
            };
            let cell = grid.cell(x, y);
            match cell.get(sub).or(cell.first()) {
                Some(&site) => site,
                None => return false,
            }
        };
        if taken[site.index()] || members.contains(&site) {
            return false;
        }
        if it.region.is_some_and(|region| {
            !problem.regions()[region.index()].contains(sites[site.index()].at)
        }) {
            return false;
        }
        members.push(site);
    }
    true
}

/// A packed spot for a unit's anchor on its lattice, or `None`.
///
/// With a target, each row of the lattice offers its leftmost spot with room, and the row whose
/// spot is nearest the target wins, as in the Tetris legalizer for standard cells: taken in
/// order from left to right, units pack against each other and leave no gaps. Without, the
/// lowest row with room wins, and its leftmost spot, which is bottom-left packing.
fn packed_spot(
    problem: &PlacementProblem,
    unit: &Unit,
    rotation: i32,
    target: Option<Point>,
    area: Rect,
    pitch: i64,
    index: &SpatialIndex,
) -> Option<Point> {
    let bounds = unit_box(problem, unit, rotation);
    let lowest = Point::new(area.min.x - bounds.min.x, area.min.y - bounds.min.y);
    let highest = Point::new(area.max.x - bounds.max.x, area.max.y - bounds.max.y);
    let mut best: Option<(u64, Point)> = None;
    let mut y = lowest.y;
    while y <= highest.y {
        let mut x = lowest.x;
        while x <= highest.x {
            let spot = Point::new(x, y);
            if unit_fits(problem, unit, rotation, spot, &area, index) {
                let Some(target) = target else {
                    return Some(spot);
                };
                let distance = spot.manhattan(target);
                if best.is_none_or(|(d, _)| distance < d) {
                    best = Some((distance, spot));
                }
                break;
            }
            x += pitch;
        }
        y += pitch;
    }
    best.map(|(_, spot)| spot)
}

/// The free spot for a unit's anchor nearest to `target`, on its lattice, or `None`.
pub(crate) fn nearest_free_spot(
    problem: &PlacementProblem,
    unit: &Unit,
    rotation: i32,
    target: Point,
    area: Rect,
    pitch: i64,
    index: &SpatialIndex,
) -> Option<Point> {
    let bounds = unit_box(problem, unit, rotation);
    // Keep the target where the unit could be at all, then put it on the lattice.
    let lowest = Point::new(area.min.x - bounds.min.x, area.min.y - bounds.min.y);
    let highest = Point::new(area.max.x - bounds.max.x, area.max.y - bounds.max.y);
    if highest.x < lowest.x || highest.y < lowest.y {
        return None;
    }
    let clamped = Rect {
        min: lowest,
        max: highest,
    }
    .clamp(target);
    let start = snap(clamped, &bounds, &area, pitch);
    let steps_x = (area.width() as i64 / pitch + 1) as usize;
    let steps_y = (area.height() as i64 / pitch + 1) as usize;
    // Ring coordinates are centred on the start, so the grid is twice as wide.
    let (nx, ny) = (2 * steps_x + 1, 2 * steps_y + 1);
    let mut best: Option<(u64, Point)> = None;
    let mut found_at = None;
    for r in 0..steps_x.max(steps_y) + 1 {
        if found_at.is_some_and(|f| r > f + 1) {
            break;
        }
        ring(nx, ny, steps_x, steps_y, r, |x, y| {
            let anchor = Point::new(
                start.x + (x as i64 - steps_x as i64) * pitch,
                start.y + (y as i64 - steps_y as i64) * pitch,
            );
            if anchor.x < lowest.x
                || anchor.x > highest.x
                || anchor.y < lowest.y
                || anchor.y > highest.y
            {
                return;
            }
            let distance = anchor.manhattan(target);
            if best.is_some_and(|(d, p)| (d, p) <= (distance, anchor)) {
                return;
            }
            if unit_fits(problem, unit, rotation, anchor, &area, index) {
                best = Some((distance, anchor));
            }
        });
        if best.is_some() && found_at.is_none() {
            found_at = Some(r);
        }
    }
    best.map(|(_, p)| p)
}
