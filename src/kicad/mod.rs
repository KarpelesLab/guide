//! Reading KiCad boards for clearance checks. Enabled by the `kicad` feature, which
//! implies `pcb`.
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
//! - the rules: net classes, their patterns and assignments, differential pairs' gaps, the
//!   minimum clearance, the hole clearance and the hole-to-hole clearance, from the
//!   project, or from the board itself for KiCad 5 files and boards without a project.
//!   KiCad's defaults apply where neither says. Custom rules, in a `.kicad_dru`, are not
//!   read.
//!
//! Formats from KiCad 5 (`20171130`) to KiCad 10 are read. Anything the reader can't model
//! is listed in [`KicadBoard::warnings`], never dropped in silence. See there for what that
//! can be.
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
pub(crate) mod json;
mod project;
mod sexpr;
mod units;

use core::fmt;

use crate::geom::Rules;
use crate::geom::drc::{self, Item};

pub use board::read;

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
mod tests;
