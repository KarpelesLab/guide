//! A uniform grid of bins over free space, to find overlapping outlines quickly.

use super::{Point, Rect};

/// Which items' outlines touch each bin.
///
/// The order of items within a bin changes as items come and go, so nothing that affects
/// results may depend on it: queries only answer yes or no, or pick the lowest item.
#[derive(Clone, Debug)]
pub(crate) struct SpatialIndex {
    origin: Point,
    bin: i64,
    nx: usize,
    ny: usize,
    bins: Vec<Vec<u32>>,
    outlines: Vec<Option<Rect>>,
}

impl SpatialIndex {
    /// An empty index over `area`, for items numbered below `items`, with bins about
    /// `typical` wide.
    pub fn new(area: Rect, items: usize, typical: i64) -> SpatialIndex {
        let side = (area.width().max(area.height())).max(1) as i64;
        // At most about 512 bins on a side, and no smaller than a typical item.
        let bin = typical.max(side / 512).max(1);
        let nx = (area.width() as i64 / bin + 1) as usize;
        let ny = (area.height() as i64 / bin + 1) as usize;
        SpatialIndex {
            origin: area.min,
            bin,
            nx,
            ny,
            bins: vec![Vec::new(); nx * ny],
            outlines: vec![None; items],
        }
    }

    fn span(&self, rect: &Rect) -> (usize, usize, usize, usize) {
        let clamp_x =
            |v: i64| ((v - self.origin.x).div_euclid(self.bin)).clamp(0, self.nx as i64 - 1);
        let clamp_y =
            |v: i64| ((v - self.origin.y).div_euclid(self.bin)).clamp(0, self.ny as i64 - 1);
        (
            clamp_x(rect.min.x) as usize,
            clamp_x(rect.max.x) as usize,
            clamp_y(rect.min.y) as usize,
            clamp_y(rect.max.y) as usize,
        )
    }

    /// Records an item's outline. Outlines without area overlap nothing and aren't kept.
    pub fn insert(&mut self, item: u32, outline: Rect) {
        if outline.width() == 0 || outline.height() == 0 {
            return;
        }
        let (x0, x1, y0, y1) = self.span(&outline);
        for x in x0..=x1 {
            for y in y0..=y1 {
                self.bins[x * self.ny + y].push(item);
            }
        }
        self.outlines[item as usize] = Some(outline);
    }

    /// Forgets an item's outline.
    pub fn remove(&mut self, item: u32) {
        let Some(outline) = self.outlines[item as usize].take() else {
            return;
        };
        let (x0, x1, y0, y1) = self.span(&outline);
        for x in x0..=x1 {
            for y in y0..=y1 {
                let bin = &mut self.bins[x * self.ny + y];
                if let Some(at) = bin.iter().position(|&i| i == item) {
                    bin.swap_remove(at);
                }
            }
        }
    }

    /// Whether `outline` overlaps any item's but those for which `skip` is true.
    pub fn overlaps(&self, outline: &Rect, skip: impl Fn(u32) -> bool) -> bool {
        if outline.width() == 0 || outline.height() == 0 {
            return false;
        }
        let (x0, x1, y0, y1) = self.span(outline);
        for x in x0..=x1 {
            for y in y0..=y1 {
                for &item in &self.bins[x * self.ny + y] {
                    if !skip(item)
                        && self.outlines[item as usize].is_some_and(|o| o.overlaps(outline))
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// The lowest-numbered item whose outline holds `point`, edges included.
    pub fn item_at(&self, point: Point) -> Option<u32> {
        let (x, _, y, _) = self.span(&Rect::point(point));
        self.bins[x * self.ny + y]
            .iter()
            .copied()
            .filter(|&item| self.outlines[item as usize].is_some_and(|o| o.contains(point)))
            .min()
    }
}
