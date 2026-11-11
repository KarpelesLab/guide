//! Specctra DSN and SES files: the autorouter interface that KiCad, through its Freerouting
//! workflow, and many other PCB tools speak. Enabled by the `dsn` feature, which implies
//! `pcb`.
//!
//! [`read`] takes a design, a `.dsn` file, and returns a [`DsnBoard`]: the same
//! [`drc::Board`] and [`Rules`] that `kicad::read` gives, so that [`drc::check`] and a
//! router see a DSN board as they see a KiCad one, with what each item came from, the
//! placement, and what a router needs besides. [`write_session`] goes the other way, from
//! tracks and vias in that model to a session, a `.ses` file, in the form KiCad's "Import
//! Specctra Session" accepts.
//!
//! Routing a design into a session, as `guide dsn route` does in place of Freerouting:
//!
//! ```no_run
//! use guide::dsn::{self, Session};
//! use guide::pcb::{self, Design, RouteOptions};
//!
//! let read = dsn::read(&std::fs::read_to_string("board.dsn")?)?;
//! // The wiring already in the file stays, as fixed copper.
//! let design = Design::from_dsn(&read);
//! let routed = pcb::route(&design, &RouteOptions::default())?;
//! println!("{} nets left unrouted", routed.unrouted.len());
//! let layers = &read.rules.spec().layers;
//! let session = Session::new("board.dsn", layers, &read.nets, &routed.tracks, &routed.vias);
//! std::fs::write("board.ses", dsn::write_session(&session)?)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # What is read
//!
//! - **The parser section**: the quote character and whether quoted strings hold spaces,
//!   as the file declares them (see the crate's S-expression reader).
//! - **Units**: `(resolution unit n)` and `(unit unit)`, at the top or in any section that
//!   gives its own, in inches, mils, centimetres, millimetres or micrometres. Lengths are
//!   decimal text, converted to nanometres exactly, with no floating point, and rounded
//!   half away from zero where a value has more decimals than a nanometre holds, as a mil's
//!   might. The resolution is kept, and not used to snap anything: KiCad writes coarser
//!   values than its resolution anyway.
//! - **Structure**: the layers, in their order, which is the copper stack from the top;
//!   the boundary; planes; keepouts, via keepouts and wire keepouts; the vias that may be
//!   used; and the rules, widths and clearances, for the board and for each layer.
//! - **Placement**: each component's position, side and rotation.
//! - **Library**: images, whose pins are padstacks at a position and a rotation of their
//!   own, and whose keepouts go with them; padstacks, with a shape on each layer: circles,
//!   rectangles, polygons and paths, which make oval pads.
//! - **Network**: nets and their pins, which give pads their nets, and classes with their
//!   nets, rules and vias.
//! - **Wiring**: wires, as paths and quarter arcs, and vias, with their nets and types,
//!   such as `fix` and `protect`.
//!
//! Anything it can't model is listed in [`DsnBoard::warnings`], never dropped in silence.
//!
//! # Coordinates and placement
//!
//! DSN's Y runs up, as `guide`'s does, so coordinates are taken as they are. A pin is its
//! padstack turned by the pin's rotation, counter-clockwise, about the pin. A component on
//! the front turns its image counter-clockwise by its rotation about its origin, and moves
//! it there. One on the back mirrors it through its Y axis first, then turns it the same
//! way, and puts what the image has on a layer on the layer that mirrors it through the
//! middle of the stack: the top on the bottom. This is how KiCad's exporter writes
//! components on the back, as the curated boards' pads show, which are all where KiCad has
//! them. Rotations by other than right angles round each coordinate to the
//! nearest nanometre, with sines and cosines that are the same on every platform.
//!
//! # What a DSN file doesn't have
//!
//! A design is what an autorouter needs, which is less than a board, so a DSN board is
//! checked with less than a KiCad one. Compared with reading the `.kicad_pcb` that KiCad
//! exported it from, a DSN board has:
//!
//! - **no holes.** Pads have no drills, so there are no hole clearances or hole-to-hole
//!   checks. Vias' drills are only known from KiCad's padstack names, such as
//!   `Via[0-1]_800:400_um`, for a diameter of 800 µm and a drill of 400 µm, and are half
//!   the diameter otherwise, with a warning. KiCad exports non-plated holes as keepouts,
//!   grown by the board's hole clearance, which read as keepouts for tracks and vias.
//! - **no zones' fills.** Planes are only outlines, and are kept in [`DsnBoard::planes`],
//!   not checked as copper. KiCad exports teardrops as planes too.
//! - **no copper graphics or text**, which KiCad doesn't export.
//! - **arcs as chords**, as KiCad exports them: one straight wire from end to end.
//! - **custom pads as their convex hulls**, as KiCad exports them, which can cover more than
//!   the pad does.
//! - **coarser coordinates.** KiCad writes six significant digits in micrometres, which
//!   is 0.1 µm below 100 mm and 1 µm beyond.
//! - **no clearances of pads' own**, no minimum clearance under every other, and no
//!   differential pairs' gaps.
//! - **routing widths** in place of minimum widths: a class's width is the width to route
//!   at, so tracks narrower than their class's are reported by [`drc::check`].
//! - **clearances by type of object**, such as KiCad's `(clearance 50 (type smd_smd))`,
//!   are listed in the warnings, and not used.
//!
//! The `dsn` tests compare the two readings of every curated board, finding by finding:
//! `docs/benchmarks.md` in the repository has the numbers.

mod read;
mod ses;

use core::fmt;

use crate::geom::drc::{self, Item};
use crate::geom::{LayerId, Point, Rules};
use crate::id::NetId;

pub use read::read;
pub use ses::{Session, write_session};

/// A board read from a DSN file, ready for [`drc::check`].
#[derive(Clone, Debug)]
pub struct DsnBoard {
    /// The items to check, with no zones. Its clearance slack is zero, which makes checks
    /// exact.
    pub board: drc::Board,
    /// The rules.
    pub rules: Rules,
    /// The name of each net, by [`NetId`]. The first is `""`, the net of wires with none,
    /// as in `kicad::read`.
    pub nets: Vec<String>,
    /// The design's name, from `(pcb name ...)`.
    pub name: String,
    /// The file's resolution.
    pub resolution: Resolution,
    /// The board's outlines, from the structure's boundaries on layer `pcb`.
    pub boundary: Vec<Vec<Point>>,
    /// The planes, which are outlines of copper to be poured, not copper.
    pub planes: Vec<Plane>,
    /// The components, in the order of the placement.
    pub components: Vec<Component>,
    /// The via padstacks: those the structure allows, in its order, then any other that
    /// the wiring uses. The rules' vias are those of them with a drill.
    pub via_padstacks: Vec<ViaPadstack>,
    /// What each pad came from, by position in [`drc::Board::pads`]. A pin with several
    /// shapes is several pads, with one source.
    pub pads: Vec<PadSource>,
    /// What each track came from, by position in [`drc::Board::tracks`]. A wire with a
    /// path of several segments is several tracks, with one wire.
    pub tracks: Vec<WireSource>,
    /// What each via came from.
    pub vias: Vec<WireSource>,
    /// What each keepout came from: the component whose image has it, if any.
    pub keepouts: Vec<Option<usize>>,
    /// What the reader skipped or could only approximate, each with how many times, in
    /// order of the message.
    pub warnings: Vec<(String, usize)>,
}

impl DsnBoard {
    /// Describes an item, as in `pin 3 of U1` or `wire 12 on F.Cu`.
    pub fn describe(&self, item: Item) -> Option<String> {
        let layer = |l: LayerId| {
            self.rules
                .spec()
                .layers
                .get(usize::from(l.0))
                .map_or("?", String::as_str)
                .to_string()
        };
        Some(match item {
            Item::Track(i) => {
                let wire = self.tracks.get(i)?;
                format!(
                    "wire {} on {}",
                    wire.wire,
                    layer(self.board.tracks[i].layer)
                )
            }
            Item::Via(i) => format!("via {}", self.vias.get(i)?.wire),
            Item::Pad(i) => {
                let pad = self.pads.get(i)?;
                let component = &self.components[pad.component];
                format!("pin {} of {}", pad.pin, component.reference)
            }
            Item::Zone(_) => return None,
            Item::Keepout(i) => match self.keepouts.get(i)? {
                Some(c) => format!("keepout of {}", self.components[*c].reference),
                None => format!("keepout {i}"),
            },
        })
    }
}

/// A unit of length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    /// Inches, 25.4 mm.
    Inch,
    /// Thousandths of an inch, 25.4 µm.
    Mil,
    /// Centimetres.
    Cm,
    /// Millimetres.
    Mm,
    /// Micrometres.
    Um,
}

impl Unit {
    /// The unit's name in a DSN file.
    pub fn name(self) -> &'static str {
        match self {
            Unit::Inch => "inch",
            Unit::Mil => "mil",
            Unit::Cm => "cm",
            Unit::Mm => "mm",
            Unit::Um => "um",
        }
    }

    /// The unit named so in a DSN file.
    pub fn from_name(name: &str) -> Option<Unit> {
        Some(match name.to_ascii_lowercase().as_str() {
            "inch" => Unit::Inch,
            "mil" => Unit::Mil,
            "cm" => Unit::Cm,
            "mm" => Unit::Mm,
            "um" => Unit::Um,
            _ => return None,
        })
    }

    /// Nanometres in one of the unit.
    pub fn nanometres(self) -> i64 {
        match self {
            Unit::Inch => 25_400_000,
            Unit::Mil => 25_400,
            Unit::Cm => 10_000_000,
            Unit::Mm => 1_000_000,
            Unit::Um => 1_000,
        }
    }

    /// The unit as `factor * 10^scale` nanometres.
    pub(crate) fn factor(self) -> (u128, i32) {
        match self {
            Unit::Inch => (254, 5),
            Unit::Mil => (254, 2),
            Unit::Cm => (1, 7),
            Unit::Mm => (1, 6),
            Unit::Um => (1, 3),
        }
    }
}

/// A resolution: how many steps make a unit, as in `(resolution um 10)`, for steps of
/// 0.1 µm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Resolution {
    /// The unit.
    pub unit: Unit,
    /// Steps in one unit, at least 1.
    pub steps: u32,
}

impl Resolution {
    /// Steps of a nanometre, `(resolution um 1000)`: what [`write_session`] writes by
    /// default, so that a session holds the model's coordinates exactly. KiCad reads it.
    pub const NANOMETRE: Resolution = Resolution {
        unit: Unit::Um,
        steps: 1000,
    };
}

/// Which side of the board a component is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// The front, or top.
    Front,
    /// The back, or bottom.
    Back,
}

/// A component: an image, placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    /// Its reference, unique on the board, such as `U1`.
    pub reference: String,
    /// The name of its image in the library.
    pub image: String,
    /// Where its image's origin is.
    pub at: Point,
    /// Its side.
    pub side: Side,
    /// Its rotation in millidegrees, in `[0, 360000)`, counter-clockwise, after mirroring
    /// on the back.
    pub rotation: i32,
}

/// A plane: the outline of copper to be poured on a layer, for a net.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plane {
    /// Its net, if the file names one.
    pub net: Option<NetId>,
    /// Its layer.
    pub layer: LayerId,
    /// Its outline.
    pub outline: Vec<Point>,
}

/// A via padstack: round copper on a span of layers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViaPadstack {
    /// Its name.
    pub name: String,
    /// The diameter of its copper, the largest of its shapes'.
    pub diameter: i64,
    /// Its drill, if its name gives it as KiCad's do.
    pub drill: Option<i64>,
    /// Its top layer.
    pub from: LayerId,
    /// Its bottom layer.
    pub to: LayerId,
}

/// Where a pad of a [`DsnBoard`] came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadSource {
    /// Its component, by position in [`DsnBoard::components`].
    pub component: usize,
    /// The pin's name in the image, such as `3`. KiCad adds `@1` and so on to the second
    /// and later pins of one name.
    pub pin: String,
    /// Its padstack's name.
    pub padstack: String,
}

/// What the wiring says of a wire or a via.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WireType {
    /// No type, or `normal`: the router may change it.
    #[default]
    Normal,
    /// `route`: routed, by an autorouter or by hand. The router may change it.
    Route,
    /// `protect`: the router must keep it.
    Protect,
    /// `fix`: the router must keep it, and so must anything else.
    Fix,
}

impl WireType {
    /// Whether the router must keep it.
    pub fn is_fixed(self) -> bool {
        matches!(self, WireType::Protect | WireType::Fix)
    }

    /// The type's name in a DSN file, `None` for [`WireType::Normal`], which is written
    /// as no type at all.
    pub fn name(self) -> Option<&'static str> {
        match self {
            WireType::Normal => None,
            WireType::Route => Some("route"),
            WireType::Protect => Some("protect"),
            WireType::Fix => Some("fix"),
        }
    }
}

/// Where a track or a via of a [`DsnBoard`] came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireSource {
    /// The wire's or via's position in the wiring, counted from 0 over wires and vias
    /// together.
    pub wire: usize,
    /// Its type.
    pub kind: WireType,
    /// A via's padstack, by position in [`DsnBoard::via_padstacks`].
    pub padstack: Option<usize>,
}

/// Why a DSN file could not be read, or a session written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsnError {
    line: Option<usize>,
    message: String,
}

impl DsnError {
    pub(crate) fn syntax(text: &str, offset: usize, message: &str) -> DsnError {
        DsnError {
            line: Some(crate::sexpr::line_of(text, offset)),
            message: message.to_string(),
        }
    }

    pub(crate) fn other(message: impl Into<String>) -> DsnError {
        DsnError {
            line: None,
            message: message.into(),
        }
    }

    /// The line it is on, counted from 1, if it is in a file.
    pub fn line(&self) -> Option<usize> {
        self.line
    }

    /// What is wrong.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for DsnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for DsnError {}

#[cfg(all(test, feature = "kicad"))]
mod curated_tests;
#[cfg(test)]
mod tests;
