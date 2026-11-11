//! Reading KiCad boards for clearance checks and routing, and writing routes back. Enabled
//! by the `kicad` feature, which implies `pcb`.
//!
//! [`read`] takes a `.kicad_pcb` file, and the board's `.kicad_pro` if it has one, and
//! returns a [`drc::Board`] and [`Rules`] for [`drc::check`], with what each item came from.
//! It reads what clearance checks need, and skips everything else, including tokens it has
//! never heard of, so that newer files still load:
//!
//! - the copper layers, from the top: `F.Cu`, `In1.Cu` and so on, then `B.Cu`;
//! - nets, by number or by name;
//! - tracks, arcs and vias, with their spans;
//! - footprints' pads, of every shape KiCad has: circles, rectangles, ovals, rounded and
//!   chamfered rectangles, trapezoids, and custom shapes made of an anchor and polygons,
//!   lines, arcs, circles and rectangles; with their offsets, holes and slots, and their
//!   own clearances or their footprint's;
//! - copper graphics, on the board and in footprints;
//! - zones' fills, as saved: `kicad-cli pcb drc` doesn't refill them either, unless asked;
//! - for filling and routing, zones' outlines with their net, layers, priority, clearance,
//!   minimum width, thermal reliefs and island removal, pads' own zone connections, and
//!   rule areas that keep copper pours out;
//! - rule areas that keep tracks, vias or pads out, as [keepouts](drc::Keepout);
//! - for routing, in [`KicadBoard::routing`], the board's edge, the copper-to-edge
//!   clearance, the track width and via size of each net's class, and differential pairs
//!   with their class's width and gap;
//! - the rules: net classes, their patterns and assignments, differential pairs' gaps, the
//!   minimum clearance, the hole clearance and the hole-to-hole clearance, from the
//!   project, or from the board itself for KiCad 5 files and boards without a project.
//!   KiCad's defaults apply where neither says. Custom rules, in a `.kicad_dru`, are not
//!   read for the checks; [`length_rules`] reads what they ask of routing: lengths and
//!   skews, track widths, and differential pairs' gaps and uncoupled lengths;
//! - for placement, in [`KicadBoard::footprints`], each footprint's position, side, angle,
//!   attributes and lock, the boxes of its courtyard, of its pads and of its silkscreen, and
//!   its pads with their nets; and the board's own silkscreen.
//!
//! [`placement_design`] makes a [`PlacementDesign`](crate::pcb::place::PlacementDesign) of
//! a board for [`place_components`](crate::pcb::place::place_components), deciding what is
//! fixed by [`fixed_because`], and [`replace_placement`] writes the footprints where it put
//! them back into the board's text, flipping those that change sides as KiCad does.
//! [`place_and_route`] places, swaps pins, routes, and places again where routing found
//! trouble, rerouting only what moved, in a loop; [`replace_pad_nets`] writes pads' new nets
//! after pin swaps or engineering changes.
//!
//! [`replace_routing`] writes routes back: it takes a board's text, and replaces its
//! tracks, vias and zone fills with others, such as [`pcb::route`](crate::pcb::route)'s,
//! keeping everything else as it was, for KiCad to check or carry on with.
//!
//! Formats from KiCad 5 (`20171130`) to KiCad 10 are read. Anything the reader can't model
//! is listed in [`KicadBoard::warnings`], never dropped in silence. See there for what that
//! can be.
//!
//! ```no_run
//! use guide::geom::drc;
//! use guide::kicad::{self, FlowOptions};
//!
//! let pcb = std::fs::read_to_string("board.kicad_pcb")?;
//! let project = std::fs::read_to_string("board.kicad_pro").ok();
//!
//! // Check the board as KiCad's DRC would.
//! let board = kicad::read(&pcb, project.as_deref())?;
//! for violation in drc::check(&board.board, &board.rules)? {
//!     println!("{violation:?}");
//! }
//!
//! // Place it, route it, and write it back for KiCad.
//! let done = kicad::place_and_route(&pcb, project.as_deref(), &FlowOptions::default())?;
//! println!("{} nets left unrouted", done.routed.unrouted.len());
//! std::fs::write("placed-and-routed.kicad_pcb", &done.text)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The reader was checked against `kicad-cli pcb drc` from KiCad 10.0.3 on nine real
//! boards, finding by finding: `docs/benchmarks.md` in the repository has the results, and
//! every difference, each explained.
//!
//! # Units and coordinates
//!
//! KiCad writes millimetres, with up to six decimals, and Y running down the page. The
//! reader converts every length to nanometres exactly, with no floating point, rounding
//! anything beyond six decimals half away from zero, which is what KiCad's own rounding
//! does. Y is negated, as `guide`'s runs up, which mirrors the picture and changes no
//! distance. Angles are read in millidegrees, rounded the same way.
//!
//! Footprints and pads are placed as KiCad places them: rotated about their origin by
//! KiCad's `RotatePoint`, which is exact for multiples of 90°, and otherwise rounds each
//! coordinate to the nearest nanometre after computing in `f64`. The sines and cosines
//! here are computed the same way on every platform. KiCad uses the C library's, so the two
//! can round a coordinate one nanometre apart, in rare cases.
//!
//! # What KiCad does that this follows
//!
//! - Pads' angles in a `.kicad_pcb` are their own, footprint's rotation included, while
//!   their positions are relative to the footprint, which rotates them.
//! - A clearance that a pad or its footprint sets of its own replaces the net classes'
//!   between it and anything else, as [`drc::clearance`] describes. A zone's own clearance
//!   is only what it was filled with: KiCad's DRC doesn't use it.
//! - The board's minimum clearance is a floor under every clearance, in
//!   [`RuleSpec::min_clearance`](crate::geom::RuleSpec::min_clearance).
//! - The two nets of a differential pair, such as `D+` and `D-` or `TX_P` and `TX_N`, may
//!   come as close as their class's gap, where that is less than its clearance, in
//!   [`RuleSpec::pairs`](crate::geom::RuleSpec::pairs). A larger gap changes nothing.
//! - Tracks, vias and graphics with no net are all of one net, the one named `""`, which
//!   KiCad never checks against itself. Pads and zone fills with no net have no net at
//!   all, `None`, as KiCad checks them against everything. Where the two meet, this
//!   checks a little more than KiCad: it doesn't check a pad with no net against a track,
//!   via or graphic with no net.
//! - Copper takes the net that KiCad's connectivity gives it: see [`read`].
//! - A plated hole is drilled through every copper layer, whichever layers its pad has
//!   copper on, and a non-plated pad has no copper at all if it is round or oval and its
//!   hole is no smaller than it.
//! - Checks that the project's severities turn off are off: the hole clearance and the
//!   hole-to-hole clearance are then zero.

mod board;
mod flow;
pub(crate) mod json;
mod place;
mod project;
mod rules;
mod sexpr;
mod units;
mod write;

use core::fmt;

use crate::geom::Rules;
use crate::geom::drc::{self, Item};

pub use board::read;
pub use flow::{
    FlowError, FlowOptions, FlowReport, PadNetChange, Round, Score, pad_changes, place_and_route,
    swap_group, swap_groups,
};
pub use place::{
    COURTYARD_FALLBACK, FixedBecause, fixed_because, footprint_outline, placement_design,
    replace_pad_nets, replace_placement,
};
pub use rules::{LengthRules, length_rules};
pub use write::{replace_routing, replace_routing_and_fills};

/// A board read from KiCad's files, ready for [`drc::check`].
#[derive(Clone, Debug)]
pub struct KicadBoard {
    /// The items to check. Its clearance slack is zero, which makes checks exact: set it
    /// to [`drc::KICAD_CLEARANCE_SLACK`] to decide as KiCad does.
    pub board: drc::Board,
    /// The rules.
    pub rules: Rules,
    /// The name of each net, by [`NetId`](crate::id::NetId).
    pub nets: Vec<String>,
    /// What each track came from, by position in [`drc::Board::tracks`].
    pub tracks: Vec<Source>,
    /// What each via came from.
    pub vias: Vec<Source>,
    /// What each pad came from. A custom pad with more than one part is several pads,
    /// with one source.
    pub pads: Vec<Source>,
    /// What each zone came from. A zone's fill is one zone for each connected piece of it,
    /// on each layer, and a filled graphic is a zone too.
    pub zones: Vec<Source>,
    /// What the reader skipped or could only approximate, each with how many times, in
    /// order of the message.
    ///
    /// These can be: text on copper, which isn't read; Bézier curves; arcs in polygons,
    /// taken as straight; zone fills with outlines of their own thickness, from KiCad 5;
    /// pads that KiCad 9's padstacks give different shapes on different layers; pads and
    /// vias that drop their copper on layers where nothing connects to them, which keep it
    /// here; custom pads whose clearance is measured from their convex hull; chamfered
    /// pads that are rounded as well, taken as chamfered only; net class patterns that
    /// look like regular expressions and match no net as wildcards; a clearance check
    /// that the project turns off; copper given another net by KiCad's connectivity; and
    /// anything malformed, which is skipped.
    pub warnings: Vec<(String, usize)>,
    /// What routing the board needs beyond the checks: its edge, and the track width and
    /// via of each net.
    pub routing: Routing,
    /// The footprints, in the order of the file, as placement needs them.
    pub footprints: Vec<Footprint>,
    /// Boxes around the board's own graphics and text on silkscreen, each with whether it is
    /// on the back: of each stroke, or of the whole of a filled shape or a text.
    pub silkscreen: Vec<(crate::geom::Rect, bool)>,
    /// Whether the project makes silkscreen that overlaps silkscreen or pads an error,
    /// `silk_overlap` or `silk_over_copper`, which by default are warnings.
    pub silk_is_error: bool,
    /// Boxes around the board's own openings in the solder mask, graphics on `F.Mask` or
    /// `B.Mask`, each with whether it is on the back.
    pub mask_graphics: Vec<(crate::geom::Rect, bool)>,
}

/// A footprint of a [`KicadBoard`]: where it is, what it is, and how much room it takes.
///
/// Every position is on the board, in `guide`'s coordinates, with Y up, as everything the
/// reader returns.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Footprint {
    /// Its reference, such as `U1`.
    pub reference: String,
    /// Its library name, such as `Capacitor_SMD:C_0603_1608Metric`.
    pub library: String,
    /// The UUID of the footprint itself, if the file has one.
    pub uuid: Option<String>,
    /// Where its origin is.
    pub at: crate::geom::Point,
    /// Its rotation, in millidegrees counterclockwise, as KiCad's.
    pub angle: i32,
    /// Whether it is on the back, `B.Cu`.
    pub back: bool,
    /// Whether it is locked in KiCad.
    pub locked: bool,
    /// Whether its attributes say surface mount, `(attr smd)`.
    pub smd: bool,
    /// Whether its attributes say through hole, `(attr through_hole)`.
    pub through_hole: bool,
    /// Whether it only exists on the board, `board_only`: a logo, a test pad.
    pub board_only: bool,
    /// Whether it is left out of the bill of materials.
    pub exclude_from_bom: bool,
    /// Whether it is left out of position files, those that an assembler places from.
    pub exclude_from_pos_files: bool,
    /// The bounding box of what it draws on its side's courtyard layer, `F.CrtYd` or
    /// `B.CrtYd`, if it draws anything there.
    pub courtyard: Option<crate::geom::Rect>,
    /// The bounding box of its pads, copper graphics and silkscreen, if it has any.
    pub extent: Option<crate::geom::Rect>,
    /// Whether it has graphics on copper, beside its pads.
    pub copper: bool,
    /// Whether it draws on `Edge.Cuts`, so that it shapes the board.
    pub edge: bool,
    /// Boxes around what it draws on its silkscreen, but text: of each stroke, or of the
    /// whole of a filled shape.
    pub silkscreen: Vec<crate::geom::Rect>,
    /// Boxes around its visible text on silkscreen, such as its reference, a little larger
    /// than the text.
    pub silk_texts: Vec<crate::geom::Rect>,
    /// Its pads.
    pub pads: Vec<FootprintPad>,
    /// Where its pads are among the board's, [`drc::Board::pads`]: a custom pad of several
    /// parts is several.
    pub board_pads: core::ops::Range<usize>,
    /// Where its graphics on copper are among the board's zones, [`drc::Board::zones`].
    pub board_zones: core::ops::Range<usize>,
}

/// A pad of a [`Footprint`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FootprintPad {
    /// Its number, such as `1` or `A3`.
    pub number: String,
    /// Where it is, its centre before any offset of its drill.
    pub at: crate::geom::Point,
    /// Its net, if it has one.
    pub net: Option<crate::id::NetId>,
    /// Whether it has a plated hole.
    pub plated: bool,
    /// Whether it has a hole that isn't plated.
    pub unplated: bool,
}

/// What a router needs from a KiCad board beyond what [`drc::check`] looks at.
///
/// The widths and vias of the net classes are what KiCad gives new routes, not limits,
/// which is why they are not in the [`Rules`]: KiCad's DRC only checks the minimum width.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Routing {
    /// The board's edge, from `Edge.Cuts`, as lines of their own width, or of one
    /// nanometre if they have none. Copper keeps
    /// [`edge_clearance`](Routing::edge_clearance) from them.
    pub outline: Vec<crate::geom::Shape>,
    /// The clearance between copper and the board's edge, KiCad's
    /// `min_copper_edge_clearance`.
    pub edge_clearance: i64,
    /// By [`NetId`](crate::id::NetId), what new routes of the net take, from its class.
    pub nets: Vec<NetRouting>,
    /// Boxes around text on copper, which the reader doesn't take as copper, a little
    /// larger than the text, with their layers.
    pub text: Vec<(crate::geom::Shape, crate::geom::LayerId)>,
    /// Differential pairs, as KiCad finds them by their names: past any digits and
    /// underscores at the end, `P` pairs with `N` and `+` with `-`. Each has the width and
    /// gap of the class that gives its positive net its track width.
    pub pairs: Vec<crate::pcb::DiffPair>,
    /// The copper zones, as outlines to fill with their settings, with what each pad says of
    /// how zones join it, by position in [`drc::Board::pads`], and rule areas that keep
    /// copper pours out. Teardrops, which KiCad keeps as zones, are left out: they belong
    /// to the tracks they widen.
    pub zones: crate::pcb::Zones,
    /// By zone of [`zones`](Routing::zones), the position of its `(zone ...)` list among all
    /// those of the file, rule areas included, in the order they come in, for
    /// [`replace_routing_and_fills`] to write its fill into.
    pub zone_lists: Vec<usize>,
}

/// What new routes of one net take, from its net class: see [`Routing`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetRouting {
    /// The track width, at least the board's minimum.
    pub track_width: i64,
    /// The diameter of a via's copper.
    pub via_diameter: i64,
    /// The diameter of a via's hole.
    pub via_drill: i64,
}

/// Where an item of a [`KicadBoard`] came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The KiCad object's UUID, as in its file and in KiCad's DRC reports. Old files don't
    /// always have one.
    pub uuid: Option<String>,
    /// What it is, as in `pad 3 of U1` or `track on F.Cu`.
    pub description: String,
}

impl KicadBoard {
    /// Where an item came from, if it is on the board.
    pub fn source(&self, item: Item) -> Option<&Source> {
        match item {
            Item::Track(i) => self.tracks.get(i),
            Item::Via(i) => self.vias.get(i),
            Item::Pad(i) => self.pads.get(i),
            Item::Zone(i) => self.zones.get(i),
            Item::Keepout(_) => None,
        }
    }
}

/// Why KiCad's files could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KicadError {
    line: Option<usize>,
    message: String,
}

impl KicadError {
    /// An error at a byte offset of a file.
    pub(crate) fn syntax(text: &str, offset: usize, message: &str) -> KicadError {
        KicadError {
            line: Some(crate::sexpr::line_of(text, offset)),
            message: message.to_string(),
        }
    }

    /// An error with no position.
    pub(crate) fn other(message: impl Into<String>) -> KicadError {
        KicadError {
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

impl fmt::Display for KicadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for KicadError {}

#[cfg(test)]
pub(crate) mod curated_tests;
#[cfg(test)]
mod eco_tests;
#[cfg(test)]
mod flow_tests;
#[cfg(test)]
mod freerouting_tests;
#[cfg(test)]
mod placement_tests;
#[cfg(test)]
mod routing_tests;
#[cfg(test)]
mod swap_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod zone_tests;
