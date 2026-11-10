//! Synthetic placement problems whose optimal wirelength is known.
//!
//! Each generator builds a reference placement first and then a netlist whose every net is as
//! short as any net of its size can be, so that the reference is optimal and its wirelength is
//! a proven lower bound for every legal placement. The bound for a net of `d` pins on
//! distinct sites of a unit lattice is the least `w + h - 2` over boxes of `w` by `h` sites with
//! `w * h >= d`. That is the idea of Peko, "placement examples with known optimal wirelength"
//! (Chang, Cong and Xie, 2003). Items are then numbered in a random order, so that nothing
//! about the answer is left in the numbering.
//!
//! Every generator is deterministic for its seed.

use super::{Item, Layout, Pin, Placement, PlacementProblem, Point, Rect, Site, SiteId};
use crate::rng::Rng;

/// A generated problem, with its optimum.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Generated {
    /// The problem.
    pub problem: PlacementProblem,
    /// A placement with the optimal wirelength.
    pub reference: Placement,
    /// The optimal wirelength, which no legal placement can beat.
    pub optimum: u64,
}

/// The least half-perimeter of `d` pins on distinct sites of a unit lattice.
pub fn net_lower_bound(d: u64) -> u64 {
    if d < 2 {
        return 0;
    }
    (1..=d).map(|w| w + d.div_ceil(w) - 2).min().unwrap_or(0)
}

/// Builds a sites problem from a reference: an item on each site of `filled`, fixed there
/// where `fixed` says so, numbered in a shuffled order, and nets added by site.
struct Builder {
    problem: PlacementProblem,
    /// The item on each site, if any.
    item_at: Vec<Option<super::ItemId>>,
    reference: Vec<(super::ItemId, SiteId)>,
}

impl Builder {
    fn new(sites: Vec<Site>, filled: &[usize], fixed: &[bool], seed: u64) -> Builder {
        let mut rng = Rng::new(seed);
        let mut order: Vec<usize> = filled.to_vec();
        rng.shuffle(&mut order);
        let mut item_at = vec![None; sites.len()];
        let kinds: Vec<u32> = sites.iter().map(|s| s.kind).collect();
        let mut problem = PlacementProblem::new(Layout::Sites(sites));
        let mut reference = Vec::new();
        for s in order {
            let mut item = Item::new().kind(kinds[s]);
            if fixed[s] {
                item = item.fixed_at_site(SiteId(s as u32));
            }
            let id = problem.add_item(item);
            item_at[s] = Some(id);
            reference.push((id, SiteId(s as u32)));
        }
        Builder {
            problem,
            item_at,
            reference,
        }
    }

    fn net(&mut self, sites: &[usize]) {
        let pins: Vec<Pin> = sites
            .iter()
            .filter_map(|&s| self.item_at[s].map(Pin::new))
            .collect();
        self.problem.add_net(1, &pins);
    }

    fn finish(self) -> Generated {
        let mut reference = Placement::new(&self.problem);
        for &(item, site) in &self.reference {
            reference.set_site(&self.problem, item, site);
        }
        let optimum = self.problem.wirelength(&reference);
        Generated {
            problem: self.problem,
            reference,
            optimum,
        }
    }
}

fn lattice(width: u32, height: u32, kind: impl Fn(u32, u32) -> u32) -> Vec<Site> {
    let mut sites = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            sites.push(Site::new(
                Point::new(i64::from(x), i64::from(y)),
                kind(x, y),
            ));
        }
    }
    sites
}

/// A `width` by `height` grid of items on as many sites, each joined to its right and upper
/// neighbours by a net of two pins. The optimum is one per net, the grid itself, up to its
/// symmetries.
///
/// With no fixed items, nothing anchors the grid, which is hard for analytic placement:
/// [`grid_with_pads`] adds a ring of fixed pads.
pub fn grid(width: u32, height: u32, seed: u64) -> Generated {
    grid_inner(width, height, 0, false, seed)
}

/// [`grid`] inside a ring of fixed pads, one beside each item on the edge (two at the
/// corners), each joined to its item by a net of two pins. Pads are on sites of kind 1, and
/// the optimum is still one per net.
pub fn grid_with_pads(width: u32, height: u32, seed: u64) -> Generated {
    grid_inner(width, height, 0, true, seed)
}

/// [`grid_with_pads`] with each column cut into vertical macros of `length` items, as carry
/// chains are, the last of a column shorter if need be.
pub fn grid_with_macros(width: u32, height: u32, length: u32, seed: u64) -> Generated {
    grid_inner(width, height, length.max(1), true, seed)
}

fn grid_inner(width: u32, height: u32, macro_length: u32, pads: bool, seed: u64) -> Generated {
    let margin = u32::from(pads);
    let (gw, gh) = (width + 2 * margin, height + 2 * margin);
    let core =
        |x: u32, y: u32| x >= margin && x < width + margin && y >= margin && y < height + margin;
    let corner = |x: u32, y: u32| (x == 0 || x == gw - 1) && (y == 0 || y == gh - 1);
    let sites = lattice(gw, gh, |x, y| u32::from(!core(x, y)));
    let index = |x: u32, y: u32| (y * gw + x) as usize;
    let mut filled = Vec::new();
    let mut fixed = vec![false; sites.len()];
    for y in 0..gh {
        for x in 0..gw {
            if core(x, y) || (pads && !corner(x, y)) {
                filled.push(index(x, y));
                fixed[index(x, y)] = !core(x, y);
            }
        }
    }
    let mut b = Builder::new(sites, &filled, &fixed, seed);
    for y in margin..height + margin {
        for x in margin..width + margin {
            if x + 1 < width + margin {
                b.net(&[index(x, y), index(x + 1, y)]);
            }
            if y + 1 < height + margin {
                b.net(&[index(x, y), index(x, y + 1)]);
            }
        }
    }
    if pads {
        for y in margin..height + margin {
            b.net(&[index(0, y), index(1, y)]);
            b.net(&[index(gw - 1, y), index(gw - 2, y)]);
        }
        for x in margin..width + margin {
            b.net(&[index(x, 0), index(x, 1)]);
            b.net(&[index(x, gh - 1), index(x, gh - 2)]);
        }
    }
    if macro_length > 0 {
        for x in margin..width + margin {
            let mut y = margin;
            while y < height + margin {
                let end = (y + macro_length).min(height + margin);
                let members: Vec<_> = (y..end)
                    .filter_map(|yy| b.item_at[index(x, yy)])
                    .enumerate()
                    .map(|(k, item)| (item, Point::new(0, k as i64)))
                    .collect();
                if members.len() > 1 {
                    b.problem.add_macro(&members);
                }
                y = end;
            }
        }
    }
    b.finish()
}

/// A chain of `length` items, each joined to the next, on a `width` by `height` grid of
/// sites. The optimum is `length - 1`, a path that snakes through the grid.
///
/// # Panics
///
/// If the chain is longer than the grid has sites.
pub fn chain(length: u32, width: u32, height: u32, seed: u64) -> Generated {
    assert!(length <= width * height, "the chain doesn't fit");
    let sites = lattice(width, height, |_, _| 0);
    // The reference snakes: left to right on even rows, back on odd ones.
    let snake: Vec<usize> = (0..length)
        .map(|k| {
            let (y, along) = (k / width, k % width);
            let x = if y % 2 == 0 { along } else { width - 1 - along };
            (y * width + x) as usize
        })
        .collect();
    let mut b = Builder::new(sites.clone(), &snake, &vec![false; sites.len()], seed);
    for pair in snake.windows(2) {
        b.net(pair);
    }
    b.finish()
}

/// What [`peko`] generates.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct PekoOptions {
    /// Columns of sites.
    pub width: u32,
    /// Rows of sites.
    pub height: u32,
    /// Nets per item. Peko's have about one.
    pub nets_per_item: f64,
    /// Whether to add a ring of fixed pads, each joined to the item beside it.
    pub pads: bool,
    /// If set, every this many columns (the last of each run) holds items of kind 2, as an
    /// FPGA's memory columns do.
    pub typed_columns: Option<u32>,
    /// The seed.
    pub seed: u64,
}

impl PekoOptions {
    /// A grid of `width` by `height` sites, one net per item, no pads, one kind.
    pub fn new(width: u32, height: u32, seed: u64) -> PekoOptions {
        PekoOptions {
            width,
            height,
            nets_per_item: 1.0,
            pads: false,
            typed_columns: None,
            seed,
        }
    }
}

/// A Peko-style problem: a full grid of items with nets of mixed sizes, each as short as its
/// size allows, so that the reference is optimal.
///
/// Net sizes follow a distribution like that of real netlists: 58% of two pins, 17% of
/// three, 8% of four, 5% of five, 8% of six to ten, and 4% of eleven to twenty. Each net
/// chooses a box of least half-perimeter for its size, puts it somewhere on the grid, and takes
/// two opposite corners and random other cells in it. Items that no net reaches get a net to
/// a neighbour.
pub fn peko(options: PekoOptions) -> Generated {
    let PekoOptions {
        width,
        height,
        nets_per_item,
        pads,
        typed_columns,
        seed,
    } = options;
    let margin = u32::from(pads);
    let (gw, gh) = (width + 2 * margin, height + 2 * margin);
    let core =
        |x: u32, y: u32| x >= margin && x < width + margin && y >= margin && y < height + margin;
    let corner = |x: u32, y: u32| (x == 0 || x == gw - 1) && (y == 0 || y == gh - 1);
    let sites = lattice(gw, gh, |x, y| {
        if !core(x, y) {
            1
        } else if typed_columns.is_some_and(|k| k > 0 && (x - margin) % k == k - 1) {
            2
        } else {
            0
        }
    });
    let index = |x: u32, y: u32| (y * gw + x) as usize;
    let mut filled = Vec::new();
    let mut fixed = vec![false; sites.len()];
    for y in 0..gh {
        for x in 0..gw {
            if core(x, y) || (pads && !corner(x, y)) {
                filled.push(index(x, y));
                fixed[index(x, y)] = !core(x, y);
            }
        }
    }
    let mut b = Builder::new(sites, &filled, &fixed, seed);
    let mut rng = Rng::new(seed ^ 0x5045_4B4F);
    let cells = width * height;
    let nets = (f64::from(cells) * nets_per_item) as u32;
    let mut covered = vec![false; (gw * gh) as usize];
    let mut pins = Vec::new();
    for _ in 0..nets {
        let roll = rng.below(100);
        let d: u64 = match roll {
            0..58 => 2,
            58..75 => 3,
            75..83 => 4,
            83..88 => 5,
            88..96 => 6 + rng.below(5),
            _ => 11 + rng.below(10),
        };
        let d = d.min(u64::from(cells));
        if d < 2 {
            break;
        }
        let best = net_lower_bound(d);
        let shapes: Vec<(u32, u32)> = (1..=width)
            .filter_map(|w| {
                let h = d.div_ceil(u64::from(w)) as u32;
                (h <= height && u64::from(w + h - 2) == best).then_some((w, h))
            })
            .collect();
        let Some(&(w, h)) = shapes.get(rng.below(shapes.len() as u64) as usize) else {
            continue;
        };
        let x0 = margin + rng.below(u64::from(width - w + 1)) as u32;
        let y0 = margin + rng.below(u64::from(height - h + 1)) as u32;
        let mut inside: Vec<usize> = Vec::with_capacity((w * h) as usize);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                inside.push(index(x, y));
            }
        }
        let (first, last) = (index(x0, y0), index(x0 + w - 1, y0 + h - 1));
        let mut rest: Vec<usize> = inside
            .into_iter()
            .filter(|&c| c != first && c != last)
            .collect();
        rng.shuffle(&mut rest);
        pins.clear();
        pins.push(first);
        pins.push(last);
        pins.extend(rest.into_iter().take(d as usize - 2));
        for &c in &pins {
            covered[c] = true;
        }
        b.net(&pins);
    }
    for y in margin..height + margin {
        for x in margin..width + margin {
            if covered[index(x, y)] {
                continue;
            }
            let neighbour = if x + 1 < width + margin {
                index(x + 1, y)
            } else if x > margin {
                index(x - 1, y)
            } else if y + 1 < height + margin {
                index(x, y + 1)
            } else if y > margin {
                index(x, y - 1)
            } else {
                continue;
            };
            b.net(&[index(x, y), neighbour]);
        }
    }
    if pads {
        for y in margin..height + margin {
            b.net(&[index(0, y), index(1, y)]);
            b.net(&[index(gw - 1, y), index(gw - 2, y)]);
        }
        for x in margin..width + margin {
            b.net(&[index(x, 0), index(x, 1)]);
            b.net(&[index(x, gh - 1), index(x, gh - 2)]);
        }
    }
    b.finish()
}

/// [`grid`] in free space: square items `size` wide, an even number, that can turn by quarter
/// turns, with their pins at their centres, in an area that holds them exactly, placed on a
/// lattice of `pitch`. Two squares that don't overlap have centres at least `size` apart, so
/// the optimum is `size` per net.
///
/// # Panics
///
/// If `size` is odd or below 2, or `pitch` below 1.
pub fn free_grid(width: u32, height: u32, size: i64, pitch: i64, seed: u64) -> Generated {
    assert!(
        size >= 2 && size % 2 == 0 && pitch >= 1,
        "bad size or pitch"
    );
    let area = Rect::with_size(0, 0, i64::from(width) * size, i64::from(height) * size);
    let mut centred = PlacementProblem::new(Layout::FreeSpace { area, pitch });
    let mut rng = Rng::new(seed);
    let mut order: Vec<u32> = (0..width * height).collect();
    rng.shuffle(&mut order);
    let half = size / 2;
    let mut item_at = vec![super::ItemId(0); (width * height) as usize];
    let mut reference_at = Vec::new();
    for cell in order {
        // The outline is centred on the origin, so that a pin there stays at the centre
        // whatever the rotation.
        let item = centred.add_item(
            Item::new()
                .outline(Rect::new(Point::new(-half, -half), Point::new(half, half)))
                .rotations(&[0, 90_000, 180_000, 270_000]),
        );
        item_at[cell as usize] = item;
        let (x, y) = (cell % width, cell / width);
        reference_at.push((item, Point::new(i64::from(x) * size, i64::from(y) * size)));
    }
    let at = |x: u32, y: u32| item_at[(y * width + x) as usize];
    for y in 0..height {
        for x in 0..width {
            if x + 1 < width {
                centred.add_net(1, &[Pin::new(at(x, y)), Pin::new(at(x + 1, y))]);
            }
            if y + 1 < height {
                centred.add_net(1, &[Pin::new(at(x, y)), Pin::new(at(x, y + 1))]);
            }
        }
    }
    let mut reference = Placement::new(&centred);
    for (item, corner) in reference_at {
        reference.set_origin(item, corner.offset(Point::new(half, half)), 0);
    }
    let optimum = centred.wirelength(&reference);
    Generated {
        problem: centred,
        reference,
        optimum,
    }
}
