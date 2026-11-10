//! The ISPD 2007 and 2008 contests' files: designs (`.gr`) in, routes out.
//!
//! The format is the contests' variation on Labyrinth's, as their pages describe it and as
//! `eval2008.pl` reads it. A design is a grid of global routing cells over several layers, the
//! capacity of each edge between two cells by layer and direction, wire widths and spacings,
//! nets of pins at coordinates, and capacity adjustments for the edges where something blocks
//! the default. Routes are BoxRouter's format: for each net, its name and number, then one line
//! for each wire or via, `(x,y,layer)-(x,y,layer)` in coordinates, then `!`.

use core::fmt;
use std::error::Error;
use std::io::{self, BufRead, Write};

/// A cell of the grid on a layer, with layers counted from 0 (the files count from 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Gcell {
    /// The column, from 0 at the left.
    pub x: u32,
    /// The row, from 0 at the bottom.
    pub y: u32,
    /// The layer, from 0.
    pub layer: u8,
}

impl Gcell {
    /// Creates a cell.
    #[inline]
    pub const fn new(x: u32, y: u32, layer: u8) -> Gcell {
        Gcell { x, y, layer }
    }
}

/// A wire or a via of a route, between two cells.
///
/// A wire has both ends on the same layer and row or column, and a via both ends in the same
/// cell, on different layers. It covers everything between its ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Segment {
    /// One end.
    pub from: Gcell,
    /// The other end.
    pub to: Gcell,
}

/// A pin of a net, where the file has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pin {
    /// The x coordinate.
    pub x: i64,
    /// The y coordinate.
    pub y: i64,
    /// The layer, from 0.
    pub layer: u8,
}

/// A net of a [`Design`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesignNet {
    /// The net's name, which is how route files refer to it.
    pub name: String,
    /// The number the file gives the net, which route files repeat.
    pub number: u64,
    /// The net's wire width.
    pub min_width: u32,
    /// The pins, in the file's order. The first one is where the contest's evaluation starts
    /// looking for a net's connections from.
    pub pins: Vec<Pin>,
}

/// An edge between two neighbouring cells on a layer whose capacity differs from the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Adjustment {
    /// One cell.
    pub from: Gcell,
    /// The neighbouring cell, on the same layer.
    pub to: Gcell,
    /// The edge's capacity, in place of the default.
    pub capacity: u32,
}

/// A global routing problem as the contests give it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Design {
    /// Columns of cells.
    pub cols: u32,
    /// Rows of cells.
    pub rows: u32,
    /// By layer, the default capacity of an edge between two cells one above the other.
    pub vertical_capacity: Vec<u32>,
    /// By layer, the default capacity of an edge between two cells side by side.
    pub horizontal_capacity: Vec<u32>,
    /// By layer, the narrowest a wire can be.
    pub min_width: Vec<u32>,
    /// By layer, the space a wire needs from its neighbours.
    pub min_spacing: Vec<u32>,
    /// By layer, the space vias need. The contests don't use it.
    pub via_spacing: Vec<u32>,
    /// The coordinates of the bottom left corner of the grid.
    pub origin: (i64, i64),
    /// The width and height of a cell.
    pub tile: (i64, i64),
    /// The nets, in the file's order.
    pub nets: Vec<DesignNet>,
    /// The edges whose capacities are not the default, in the file's order.
    pub adjustments: Vec<Adjustment>,
}

impl Design {
    /// The number of layers.
    #[inline]
    pub fn layers(&self) -> usize {
        self.vertical_capacity.len()
    }

    /// The cell that a point is in, as `eval2008.pl` works it out, or `None` if that is off
    /// the grid.
    pub fn cell_of(&self, x: i64, y: i64) -> Option<(u32, u32)> {
        let cx = (x - self.origin.0).checked_div(self.tile.0)?;
        let cy = (y - self.origin.1).checked_div(self.tile.1)?;
        let cx = u32::try_from(cx).ok().filter(|&c| c < self.cols)?;
        let cy = u32::try_from(cy).ok().filter(|&c| c < self.rows)?;
        Some((cx, cy))
    }

    /// The coordinates of the centre of a cell, which is where routes written here put it.
    pub fn center(&self, x: u32, y: u32) -> (i64, i64) {
        (
            self.origin.0 + i64::from(x) * self.tile.0 + self.tile.0 / 2,
            self.origin.1 + i64::from(y) * self.tile.1 + self.tile.1 / 2,
        )
    }

    /// The space one wire of `net` takes on `layer`: its width, or the layer's least if that is
    /// more, plus the layer's spacing. That is what the contests take from an edge's capacity
    /// for each net that crosses it.
    pub fn wire_usage(&self, net: &DesignNet, layer: usize) -> u32 {
        net.min_width.max(self.min_width[layer]) + self.min_spacing[layer]
    }

    /// The capacities of all edges, by layer, with the adjustments applied.
    pub fn capacities(&self) -> Capacities {
        let (cols, rows) = (self.cols as usize, self.rows as usize);
        let layers = self.layers();
        let hplane = cols.saturating_sub(1) * rows;
        let vplane = cols * rows.saturating_sub(1);
        let mut horizontal = Vec::with_capacity(layers * hplane);
        let mut vertical = Vec::with_capacity(layers * vplane);
        for l in 0..layers {
            horizontal.extend(core::iter::repeat_n(self.horizontal_capacity[l], hplane));
            vertical.extend(core::iter::repeat_n(self.vertical_capacity[l], vplane));
        }
        let mut caps = Capacities {
            cols,
            rows,
            layers,
            horizontal,
            vertical,
        };
        for a in &self.adjustments {
            if let Some(at) = caps.edge_between(a.from, a.to) {
                *caps.get_mut(at) = a.capacity;
            }
        }
        caps
    }
}

/// An edge between two neighbouring cells on a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeAt {
    /// The edge from `(x, y)` to `(x + 1, y)`.
    Horizontal {
        /// The layer.
        layer: usize,
        /// The column of the left cell.
        x: usize,
        /// The row.
        y: usize,
    },
    /// The edge from `(x, y)` to `(x, y + 1)`.
    Vertical {
        /// The layer.
        layer: usize,
        /// The column.
        x: usize,
        /// The row of the lower cell.
        y: usize,
    },
}

/// The capacity of every edge of a [`Design`], by layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capacities {
    cols: usize,
    rows: usize,
    layers: usize,
    /// By layer, then row, then column of the left cell.
    horizontal: Vec<u32>,
    /// By layer, then the row of the lower cell, then column.
    vertical: Vec<u32>,
}

impl Capacities {
    /// The edge between two cells, if they are neighbours on the same layer of the grid.
    pub fn edge_between(&self, a: Gcell, b: Gcell) -> Option<EdgeAt> {
        let layer = usize::from(a.layer);
        if a.layer != b.layer || layer >= self.layers {
            return None;
        }
        let (ax, ay, bx, by) = (a.x as usize, a.y as usize, b.x as usize, b.y as usize);
        if ay == by && ax.abs_diff(bx) == 1 && ay < self.rows && ax.max(bx) < self.cols {
            Some(EdgeAt::Horizontal {
                layer,
                x: ax.min(bx),
                y: ay,
            })
        } else if ax == bx && ay.abs_diff(by) == 1 && ax < self.cols && ay.max(by) < self.rows {
            Some(EdgeAt::Vertical {
                layer,
                x: ax,
                y: ay.min(by),
            })
        } else {
            None
        }
    }

    /// Where an edge's capacity is stored: in the horizontal table or not, and the index.
    fn slot(&self, at: EdgeAt) -> (bool, usize) {
        match at {
            EdgeAt::Horizontal { layer, x, y } => {
                (true, (layer * self.rows + y) * (self.cols - 1) + x)
            }
            EdgeAt::Vertical { layer, x, y } => {
                (false, (layer * (self.rows - 1) + y) * self.cols + x)
            }
        }
    }

    /// The capacity of an edge.
    ///
    /// # Panics
    ///
    /// If the edge is not on the grid.
    pub fn get(&self, at: EdgeAt) -> u32 {
        match self.slot(at) {
            (true, i) => self.horizontal[i],
            (false, i) => self.vertical[i],
        }
    }

    fn get_mut(&mut self, at: EdgeAt) -> &mut u32 {
        match self.slot(at) {
            (true, i) => &mut self.horizontal[i],
            (false, i) => &mut self.vertical[i],
        }
    }

    /// Columns of cells.
    #[inline]
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Rows of cells.
    #[inline]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Layers.
    #[inline]
    pub fn layers(&self) -> usize {
        self.layers
    }
}

/// Why a design or a route file could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum ReadError {
    /// Reading failed.
    Io(io::Error),
    /// A line is not what the format has there.
    Syntax {
        /// The line, from 1.
        line: usize,
        /// What was expected.
        expected: &'static str,
    },
    /// The file ended early.
    UnexpectedEnd {
        /// What was expected.
        expected: &'static str,
    },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "reading failed: {e}"),
            ReadError::Syntax { line, expected } => {
                write!(f, "line {line}: expected {expected}")
            }
            ReadError::UnexpectedEnd { expected } => {
                write!(f, "the file ended where {expected} was expected")
            }
        }
    }
}

impl Error for ReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ReadError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ReadError {
    fn from(e: io::Error) -> Self {
        ReadError::Io(e)
    }
}

/// Lines of a file, with their numbers, skipping blank ones.
struct Lines<R> {
    input: R,
    buf: String,
    number: usize,
}

impl<R: BufRead> Lines<R> {
    fn new(input: R) -> Self {
        Lines {
            input,
            buf: String::new(),
            number: 0,
        }
    }

    /// The next line that isn't blank, trimmed, and its number, or `None` at the end.
    fn next(&mut self) -> Result<Option<(&str, usize)>, ReadError> {
        loop {
            self.buf.clear();
            if self.input.read_line(&mut self.buf)? == 0 {
                return Ok(None);
            }
            self.number += 1;
            if !self.buf.trim().is_empty() {
                return Ok(Some((self.buf.trim(), self.number)));
            }
        }
    }

    fn expect(&mut self, expected: &'static str) -> Result<(&str, usize), ReadError> {
        self.next()?.ok_or(ReadError::UnexpectedEnd { expected })
    }
}

fn numbers<T: core::str::FromStr>(text: &str) -> Option<Vec<T>> {
    text.split_whitespace().map(|t| t.parse().ok()).collect()
}

/// Reads a line `keyword n n n ...` with one number per layer.
fn per_layer<R: BufRead>(
    lines: &mut Lines<R>,
    keyword: &str,
    layers: usize,
    expected: &'static str,
) -> Result<Vec<u32>, ReadError> {
    let (line, number) = lines.expect(expected)?;
    let syntax = ReadError::Syntax {
        line: number,
        expected,
    };
    let Some(rest) = line.strip_prefix(keyword) else {
        return Err(syntax);
    };
    match numbers::<u32>(rest) {
        Some(v) if v.len() == layers => Ok(v),
        _ => Err(syntax),
    }
}

/// Reads a design in the contests' format.
///
/// # Errors
///
/// If reading fails, or the input is not in the format.
///
/// # Examples
///
/// The contest's own sample, a net across a grid of 3 by 3 cells on two layers:
///
/// ```
/// use guide::asic::read_design;
///
/// let text = "grid 3 3 2
/// vertical capacity 0 1
/// horizontal capacity 1 0
/// minimum width 1 1
/// minimum spacing 0 0
/// via spacing 0 0
/// 0 0 10 10
/// num net 1
/// A 0 2 1
///  5  5 1
/// 25  5 1
/// 4
/// 1 0 1   2 0 1   0
/// 1 1 1   2 1 1   0
/// 0 0 2   0 1 2   0
/// 1 1 2   1 2 2   0
/// ";
/// let design = read_design(text.as_bytes())?;
/// assert_eq!((design.cols, design.rows, design.layers()), (3, 3, 2));
/// assert_eq!(design.nets[0].pins.len(), 2);
/// assert_eq!(design.adjustments.len(), 4);
/// # Ok::<(), guide::asic::ReadError>(())
/// ```
pub fn read_design<R: BufRead>(input: R) -> Result<Design, ReadError> {
    let mut lines = Lines::new(input);
    const GRID: &str = "`grid X Y LAYERS`";
    let (line, number) = lines.expect(GRID)?;
    let grid = line
        .strip_prefix("grid")
        .and_then(numbers::<u32>)
        .filter(|v| v.len() == 3 && v[0] > 0 && v[1] > 0 && (1..=255).contains(&v[2]))
        .ok_or(ReadError::Syntax {
            line: number,
            expected: GRID,
        })?;
    let (cols, rows, layers) = (grid[0], grid[1], grid[2] as usize);
    let vertical_capacity = per_layer(
        &mut lines,
        "vertical capacity",
        layers,
        "`vertical capacity` and a number per layer",
    )?;
    let horizontal_capacity = per_layer(
        &mut lines,
        "horizontal capacity",
        layers,
        "`horizontal capacity` and a number per layer",
    )?;
    let min_width = per_layer(
        &mut lines,
        "minimum width",
        layers,
        "`minimum width` and a number per layer",
    )?;
    let min_spacing = per_layer(
        &mut lines,
        "minimum spacing",
        layers,
        "`minimum spacing` and a number per layer",
    )?;
    let via_spacing = per_layer(
        &mut lines,
        "via spacing",
        layers,
        "`via spacing` and a number per layer",
    )?;
    const ORIGIN: &str = "the lower left corner and the width and height of a cell";
    let (line, number) = lines.expect(ORIGIN)?;
    let origin = numbers::<i64>(line)
        .filter(|v| v.len() == 4 && v[2] > 0 && v[3] > 0)
        .ok_or(ReadError::Syntax {
            line: number,
            expected: ORIGIN,
        })?;
    const NUM_NET: &str = "`num net N`";
    let (line, number) = lines.expect(NUM_NET)?;
    let count: usize = line
        .strip_prefix("num net")
        .and_then(|r| r.trim().parse().ok())
        .ok_or(ReadError::Syntax {
            line: number,
            expected: NUM_NET,
        })?;
    let mut nets = Vec::with_capacity(count);
    for _ in 0..count {
        const NET: &str = "a net: its name, number, number of pins and width";
        let (line, number) = lines.expect(NET)?;
        let mut words = line.split_whitespace();
        let parsed = (|| {
            let name = words.next()?.to_owned();
            let id: u64 = words.next()?.parse().ok()?;
            let pins: usize = words.next()?.parse().ok()?;
            let width: u32 = words.next()?.parse().ok()?;
            words.next().is_none().then_some((name, id, pins, width))
        })();
        let (name, id, pin_count, min_width) = parsed.ok_or(ReadError::Syntax {
            line: number,
            expected: NET,
        })?;
        let mut pins = Vec::with_capacity(pin_count);
        for _ in 0..pin_count {
            const PIN: &str = "a pin: `x y layer`, with a layer from 1";
            let (line, number) = lines.expect(PIN)?;
            let pin = numbers::<i64>(line)
                .filter(|v| v.len() == 3 && v[2] >= 1 && v[2] as usize <= layers)
                .ok_or(ReadError::Syntax {
                    line: number,
                    expected: PIN,
                })?;
            pins.push(Pin {
                x: pin[0],
                y: pin[1],
                layer: (pin[2] - 1) as u8,
            });
        }
        nets.push(DesignNet {
            name,
            number: id,
            min_width,
            pins,
        });
    }
    let mut adjustments = Vec::new();
    while let Some((line, number)) = lines.next()? {
        const COUNT: &str = "the number of capacity adjustments";
        let count: usize = line.parse().map_err(|_| ReadError::Syntax {
            line: number,
            expected: COUNT,
        })?;
        adjustments.reserve(count);
        for _ in 0..count {
            const ADJUSTMENT: &str =
                "a capacity adjustment: `x y layer x y layer capacity`, with layers from 1";
            let (line, number) = lines.expect(ADJUSTMENT)?;
            let v = numbers::<u32>(line)
                .filter(|v| {
                    v.len() == 7
                        && (1..=layers).contains(&(v[2] as usize))
                        && (1..=layers).contains(&(v[5] as usize))
                })
                .ok_or(ReadError::Syntax {
                    line: number,
                    expected: ADJUSTMENT,
                })?;
            adjustments.push(Adjustment {
                from: Gcell::new(v[0], v[1], (v[2] - 1) as u8),
                to: Gcell::new(v[3], v[4], (v[5] - 1) as u8),
                capacity: v[6],
            });
        }
    }
    Ok(Design {
        cols,
        rows,
        vertical_capacity,
        horizontal_capacity,
        min_width,
        min_spacing,
        via_spacing,
        origin: (origin[0], origin[1]),
        tile: (origin[2], origin[3]),
        nets,
        adjustments,
    })
}

/// The route of one net in a route file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetRoute {
    /// The net's name.
    pub name: String,
    /// The number the file gives it.
    pub number: u64,
    /// The wires and vias, as they appear, in coordinates.
    pub segments: Vec<(Point3, Point3)>,
}

/// Reads a route file, keeping coordinates as they are, with layers from 0.
///
/// # Errors
///
/// If reading fails, or a line is not in the format.
pub fn read_routes<R: BufRead>(input: R) -> Result<Vec<NetRoute>, ReadError> {
    let mut lines = Lines::new(input);
    let mut out = Vec::new();
    while let Some((line, number)) = lines.next()? {
        const HEADER: &str = "a net's name and number, and maybe its number of segments";
        let mut words = line.split_whitespace();
        let header = (|| {
            let name = words.next()?.to_owned();
            let id: u64 = words.next()?.parse().ok()?;
            if let Some(w) = words.next() {
                w.parse::<u64>().ok()?;
            }
            words.next().is_none().then_some((name, id))
        })();
        let (name, id) = header.ok_or(ReadError::Syntax {
            line: number,
            expected: HEADER,
        })?;
        let mut segments = Vec::new();
        loop {
            const SEGMENT: &str = "`(x,y,layer)-(x,y,layer)` or `!`";
            let (line, number) = lines.expect(SEGMENT)?;
            if line == "!" {
                break;
            }
            let segment = parse_segment(line).ok_or(ReadError::Syntax {
                line: number,
                expected: SEGMENT,
            })?;
            segments.push(segment);
        }
        out.push(NetRoute {
            name,
            number: id,
            segments,
        });
    }
    Ok(out)
}

/// A point of a route file: its coordinates, and its layer from 0.
pub type Point3 = (i64, i64, u8);

fn parse_segment(line: &str) -> Option<(Point3, Point3)> {
    let (a, b) = line.split_once(")-(")?;
    let a = a.trim().strip_prefix('(')?;
    let b = b.trim().strip_suffix(')')?;
    let point = |s: &str| -> Option<Point3> {
        let v: Vec<i64> = s
            .split(',')
            .map(|t| t.trim().parse().ok())
            .collect::<Option<_>>()?;
        (v.len() == 3 && v[2] >= 1 && v[2] <= 256).then(|| (v[0], v[1], (v[2] - 1) as u8))
    };
    Some((point(a)?, point(b)?))
}

/// Writes the route of one net in the contests' format: its name and number, its wires and vias
/// with each cell at its centre, and `!`.
///
/// # Errors
///
/// If writing fails.
pub fn write_net_route<W: Write>(
    out: &mut W,
    design: &Design,
    net: &DesignNet,
    segments: &[Segment],
) -> io::Result<()> {
    writeln!(out, "{} {} {}", net.name, net.number, segments.len())?;
    for s in segments {
        let (ax, ay) = design.center(s.from.x, s.from.y);
        let (bx, by) = design.center(s.to.x, s.to.y);
        writeln!(
            out,
            "({},{},{})-({},{},{})",
            ax,
            ay,
            u32::from(s.from.layer) + 1,
            bx,
            by,
            u32::from(s.to.layer) + 1
        )?;
    }
    writeln!(out, "!")
}
