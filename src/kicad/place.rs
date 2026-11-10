//! Placing a KiCad board's footprints: the design to place, what stays fixed, and writing
//! the result back.

use std::borrow::Cow;

use super::sexpr::{self, Sexp};
use super::units::rotate;
use super::write::{mm, rewrite};
use super::{Footprint, KicadBoard};
use crate::geom::{Point, Rect};
use crate::pcb::place::{
    Component, ComponentPin, Placed, PlacementDesign, Side, edge_segments, near_segments,
};

/// How far around its pads and silkscreen a footprint without a courtyard is taken to reach:
/// a quarter of a millimetre, the courtyard excess that IPC-7351 gives most parts.
pub const COURTYARD_FALLBACK: i64 = 250_000;

/// Why [`placement_design`] keeps a footprint where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FixedBecause {
    /// It is locked in KiCad.
    Locked,
    /// It has no pads: a logo, a drawing.
    NoPads,
    /// Nobody assembles it: it is `board_only`, or left out of both the bill of materials and
    /// the position files, as test pads, touch pads and edge fingers are.
    NotAssembled,
    /// A mounting hole, by its reference (`H`, `MH`, `MK`), its library (`MountingHole`), or
    /// its pads, all holes of no net.
    Mechanical,
    /// A connector, by its reference (`J`, `P`, `CN`, `CON`, `USB`) or its library
    /// (`Connector`, `Conn`).
    Connector,
    /// A fiducial, by its reference (`FID`) or its library.
    Fiducial,
    /// Something a user handles where the enclosure says: a switch or a battery, by its
    /// reference (`SW`, `BT`, `BAT`).
    UserFacing,
    /// Its courtyard reaches the board's edge: it is mounted on the edge.
    OnEdge,
    /// It draws on `Edge.Cuts`, so it shapes the board.
    ShapesEdge,
}

/// The letters a reference starts with, in capitals.
fn prefix(reference: &str) -> String {
    reference
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_ascii_uppercase()
}

/// The room a footprint of a board takes, on the board: its courtyard's box, with its pads'
/// and copper's if they reach past it, as on footprints whose courtyard is drawn round their
/// body only; or failing a courtyard, the box of its pads, copper and silkscreen grown by
/// [`COURTYARD_FALLBACK`]. `None` for a footprint with no pads and no copper, which takes no
/// room that matters to the rules.
pub fn footprint_outline(board: &KicadBoard, footprint: usize) -> Option<Rect> {
    let fp = board.footprints.get(footprint)?;
    if fp.pads.is_empty() && !fp.copper {
        return None;
    }
    let Some(courtyard) = fp.courtyard else {
        return fp.extent.map(|e| e.expand(COURTYARD_FALLBACK));
    };
    let pads = board.board.pads[fp.board_pads.clone()]
        .iter()
        .map(|p| p.shape.bbox());
    let copper = board.board.zones[fp.board_zones.clone()]
        .iter()
        .map(|z| z.shape.bbox());
    Some(pads.chain(copper).fold(courtyard, |r, b| r.union(&b)))
}

/// Whether placement keeps a footprint of a board where it is, and why.
///
/// In this order: locked; with no pads; not assembled; drawing the board's edge; a mounting
/// hole, connector, fiducial, switch or battery, by its reference or its library's name;
/// with a courtyard, or failing that its outline, that touches or crosses the board's edge.
/// Everything else moves.
pub fn fixed_because(board: &KicadBoard, footprint: usize) -> Option<FixedBecause> {
    let fp = board.footprints.get(footprint)?;
    if fp.locked {
        return Some(FixedBecause::Locked);
    }
    if fp.pads.is_empty() {
        return Some(FixedBecause::NoPads);
    }
    if fp.board_only || (fp.exclude_from_bom && fp.exclude_from_pos_files) {
        return Some(FixedBecause::NotAssembled);
    }
    if fp.edge {
        return Some(FixedBecause::ShapesEdge);
    }
    let p = prefix(&fp.reference);
    let library = fp.library.to_ascii_lowercase();
    let holes = fp
        .pads
        .iter()
        .all(|pad| pad.net.is_none() && (pad.plated || pad.unplated));
    if matches!(p.as_str(), "H" | "MH" | "MK" | "HOLE") || library.contains("mountinghole") || holes
    {
        return Some(FixedBecause::Mechanical);
    }
    if matches!(p.as_str(), "J" | "P" | "CN" | "CON" | "USB")
        || library.contains("connector")
        || library.contains("conn_")
    {
        return Some(FixedBecause::Connector);
    }
    if p == "FID" || library.contains("fiducial") {
        return Some(FixedBecause::Fiducial);
    }
    if matches!(p.as_str(), "SW" | "BT" | "BAT") {
        return Some(FixedBecause::UserFacing);
    }
    let segments = edge_segments(&board.routing.outline);
    if let Some(outline) = footprint_outline(board, footprint)
        && near_segments(&segments, &outline, 0)
    {
        return Some(FixedBecause::OnEdge);
    }
    None
}

/// The design to place from a board, with one [component](Component) for each of its
/// [footprints](KicadBoard::footprints), in the same order.
///
/// Each takes the room of [`footprint_outline`], on its side, or on both if any of its pads
/// has a hole, and is fixed if [`fixed_because`] says. Its pins are its pads that have a net.
/// The board's edge, copper and zones are as [`read`](super::read) read them: copper
/// graphics, text on copper, openings drawn in the solder mask and rule areas that keep pads
/// or tracks out are kept out of on the outer layers they are on, and the nets of copper zones
/// are planes. If the project
/// makes silkscreen overlaps errors, [`KicadBoard::silk_is_error`], the room a movable
/// footprint takes includes its silkscreen, and fixed footprints' silkscreen and the board's
/// own are kept out of.
pub fn placement_design(board: &KicadBoard) -> PlacementDesign {
    let spec = board.rules.spec();
    let layers = spec.layers.len();
    let side_of = |layer: crate::geom::LayerId| -> Option<Option<Side>> {
        if layer.0 == 0 {
            Some(Some(Side::Front))
        } else if usize::from(layer.0) + 1 == layers {
            Some(Some(Side::Back))
        } else {
            None
        }
    };
    let mut design = PlacementDesign {
        outline: board.routing.outline.clone(),
        edge_clearance: board.routing.edge_clearance,
        nets: board.nets.clone(),
        layers,
        track_pitch: spec.track_width + spec.clearance,
        clearance: board.rules.max_clearance(),
        ..PlacementDesign::default()
    };
    for (i, fp) in board.footprints.iter().enumerate() {
        let side = if fp.back { Side::Back } else { Side::Front };
        let mut c = Component::new(fp.reference.clone(), fp.at, fp.angle, side);
        c.through = fp.pads.iter().any(|p| p.plated || p.unplated);
        c.fixed = fixed_because(board, i).is_some();
        c.outline = footprint_outline(board, i);
        if board.silk_is_error {
            let side = if fp.back { Side::Back } else { Side::Front };
            if c.fixed {
                for &b in fp.silkscreen.iter().chain(&fp.silk_texts) {
                    design.keepouts.push((b, Some(side)));
                }
            } else if let Some(o) = c.outline {
                c.outline = Some(fp.silkscreen.iter().fold(o, |o, b| o.union(b)));
            }
        }
        if c.fixed && fp.courtyard.is_none() {
            // Without a courtyard, KiCad checks nothing of its room but its copper, so that
            // is what is kept out of: its pads, and its copper graphics, with clearance.
            c.outline = None;
            for pad in &board.board.pads[fp.board_pads.clone()] {
                let sides = if pad.hole.is_some() {
                    None
                } else if fp.back {
                    Some(Side::Back)
                } else {
                    Some(Side::Front)
                };
                design
                    .keepouts
                    .push((pad.shape.bbox().expand(COURTYARD_FALLBACK), sides));
            }
            for zone in &board.board.zones[fp.board_zones.clone()] {
                let side = if fp.back { Side::Back } else { Side::Front };
                design
                    .keepouts
                    .push((zone.shape.bbox().expand(COURTYARD_FALLBACK), Some(side)));
            }
        }
        c.pins = fp
            .pads
            .iter()
            .filter_map(|p| {
                Some(ComponentPin {
                    at: p.at,
                    net: p.net?,
                })
            })
            .collect();
        design.components.push(c);
    }
    for k in &board.board.keepouts {
        if !(k.pads || k.tracks) {
            continue;
        }
        let r = k.shape.bbox();
        let (front, back) = (
            k.layers.contains(crate::geom::LayerId(0)),
            layers > 0 && k.layers.contains(crate::geom::LayerId(layers as u8 - 1)),
        );
        match (front, back) {
            (true, true) => design.keepouts.push((r, None)),
            (true, false) => design.keepouts.push((r, Some(Side::Front))),
            (false, true) => design.keepouts.push((r, Some(Side::Back))),
            _ => {}
        }
    }
    for (zone, source) in board.board.zones.iter().zip(&board.zones) {
        if source.description.starts_with("graphic on")
            && let Some(side) = side_of(zone.layer)
        {
            design.keepouts.push((zone.shape.bbox(), side));
        }
    }
    for (shape, layer) in &board.routing.text {
        if let Some(side) = side_of(*layer) {
            design.keepouts.push((shape.bbox(), side));
        }
    }
    // The board's own openings in the mask would bridge pads put under them.
    for &(b, back) in &board.mask_graphics {
        let side = if back { Side::Back } else { Side::Front };
        design.keepouts.push((b, Some(side)));
    }
    if board.silk_is_error {
        for &(b, back) in &board.silkscreen {
            let side = if back { Side::Back } else { Side::Front };
            design.keepouts.push((b, Some(side)));
        }
    }
    let mut planes = Vec::new();
    for z in &board.routing.zones.zones {
        let (Some(net), Some(&first)) = (z.net, z.outline.first()) else {
            continue;
        };
        let r = z
            .outline
            .iter()
            .fold(Rect::new(first, first), |r, &p| r.union(&Rect::new(p, p)));
        planes.push((net, r));
    }
    design.planes = planes;
    design
}

/// Where an atom is in the text it was parsed from, if it is a slice of it.
fn span(text: &str, atom: &Sexp<'_>) -> Option<(usize, usize)> {
    let Sexp::Atom(Cow::Borrowed(s)) = atom else {
        return None;
    };
    let start = (s.as_ptr() as usize).checked_sub(text.as_ptr() as usize)?;
    (start + s.len() <= text.len()).then_some((start, start + s.len()))
}

/// Degrees with up to three decimals, from millidegrees.
fn degrees(millidegrees: i32) -> String {
    let sign = if millidegrees < 0 { "-" } else { "" };
    let a = millidegrees.unsigned_abs();
    let (whole, frac) = (a / 1000, a % 1000);
    if frac == 0 {
        format!("{sign}{whole}")
    } else {
        let f = format!("{frac:03}");
        format!("{sign}{whole}.{}", f.trim_end_matches('0'))
    }
}

/// A rotation in `(-180°, 180°]`.
fn normalized(rotation: i32) -> i32 {
    let r = rotation.rem_euclid(360_000);
    if r > 180_000 { r - 360_000 } else { r }
}

/// The edits that set an `(at x y angle)` list: its position, if given, and its angle.
fn set_at(
    text: &str,
    at: &Sexp<'_>,
    position: Option<(i64, i64)>,
    angle: i32,
    edits: &mut Vec<(usize, usize, String)>,
) -> Option<()> {
    let args = at.args();
    let x = span(text, args.first()?)?;
    let y = span(text, args.get(1)?)?;
    let old = args
        .get(2)
        .filter(|a| a.as_atom().is_some_and(|t| t.parse::<f64>().is_ok()));
    if let Some((px, py)) = position {
        edits.push((x.0, y.1, format!("{} {}", mm(px), mm(py))));
    }
    match old {
        Some(a) => {
            let a = span(text, a)?;
            edits.push((a.0, a.1, degrees(angle)));
        }
        None if angle != 0 => edits.push((y.1, y.1, format!(" {}", degrees(angle)))),
        None => {}
    }
    Some(())
}

/// The edits that swap the sides of the layers an atom list names: `F.Cu` for `B.Cu`, and
/// so on, leaving `*.Cu` and `F&B.Cu` as they are.
fn swap_layers(text: &str, list: &Sexp<'_>, edits: &mut Vec<(usize, usize, String)>) {
    for atom in list.args() {
        let Some(name) = atom.as_atom() else { continue };
        let swapped = if let Some(rest) = name.strip_prefix("F.") {
            format!("B.{rest}")
        } else if let Some(rest) = name.strip_prefix("B.") {
            format!("F.{rest}")
        } else {
            continue;
        };
        if let Some((a, b)) = span(text, atom) {
            edits.push((a, b, swapped));
        }
    }
}

/// The edits that negate the second number of a list, as `(start x y)`: Y mirrored.
fn negate_y(text: &str, list: &Sexp<'_>, edits: &mut Vec<(usize, usize, String)>) {
    let Some(atom) = list.args().get(1) else {
        return;
    };
    let (Some(v), Some((a, b))) = (
        atom.as_atom().and_then(super::units::mm_to_nm),
        span(text, atom),
    ) else {
        return;
    };
    edits.push((a, b, mm(-v)));
}

/// The edits that mirror everything a footprint holds in its own frame, top to bottom, as
/// KiCad does when it flips one: Y negated in points, layers to the other side, chamfers'
/// corners swapped, and texts mirrored. Positions and angles in `(at ...)`, zones and 3D
/// models are left to the caller.
fn flip_local(text: &str, node: &Sexp<'_>, edits: &mut Vec<(usize, usize, String)>) {
    for child in node.args() {
        let Some(name) = child.name() else { continue };
        match name {
            "at" | "zone" | "model" | "group" => {}
            "start" | "end" | "mid" | "center" | "xy" | "rect_delta" => {
                negate_y(text, child, edits);
            }
            "offset" if node.is("drill") => negate_y(text, child, edits),
            "angle" if node.is("fp_arc") => {
                if let (Some(atom), Some(v)) = (
                    child.args().first(),
                    child.arg(0).and_then(super::units::degrees_to_millidegrees),
                ) && let Some((a, b)) = span(text, atom)
                {
                    edits.push((a, b, degrees(-v)));
                }
            }
            "layer" | "layers" => swap_layers(text, child, edits),
            "chamfer" => {
                for atom in child.args() {
                    let swapped = match atom.as_atom() {
                        Some("top_left") => "bottom_left",
                        Some("bottom_left") => "top_left",
                        Some("top_right") => "bottom_right",
                        Some("bottom_right") => "top_right",
                        _ => continue,
                    };
                    if let Some((a, b)) = span(text, atom) {
                        edits.push((a, b, swapped.to_string()));
                    }
                }
            }
            "effects" => match child.child("justify") {
                Some(justify) => {
                    let mirror = justify
                        .args()
                        .iter()
                        .find(|a| a.as_atom() == Some("mirror"));
                    match mirror {
                        Some(mirror) => {
                            if let Some((a, b)) = span(text, mirror) {
                                edits.push((a, b, String::new()));
                            }
                        }
                        None => {
                            let last = justify.items().last();
                            if let Some((_, b)) = last.and_then(|l| span(text, l)) {
                                edits.push((b, b, " mirror".to_string()));
                            }
                        }
                    }
                }
                None => {
                    if let Some((_, b)) = child.items().first().and_then(|n| span(text, n)) {
                        edits.push((b, b, " (justify mirror)".to_string()));
                    }
                }
            },
            _ => flip_local(text, child, edits),
        }
    }
}

/// A rectangle of a footprint's, where it is on the board, where `p` puts the footprint:
/// mirrored top to bottom about its origin if it changes sides, turned, and moved.
fn moved(r: Rect, fp: &Footprint, p: &Placed) -> Rect {
    let flip = (p.side == Side::Back) != fp.back;
    let turn = p.rotation - if flip { -fp.angle } else { fp.angle };
    let corner = |q: Point| {
        let v = crate::place::Point::new(q.x - fp.at.x, q.y - fp.at.y);
        let v = if flip {
            crate::place::Point::new(v.x, -v.y)
        } else {
            v
        };
        let v = crate::place::rotate(v, turn);
        Point::new(p.at.x + v.x, p.at.y + v.y)
    };
    Rect::new(corner(r.min()), corner(r.max()))
}

/// Whether two rectangles' interiors overlap.
fn overlap(a: &Rect, b: &Rect) -> bool {
    a.min().x < b.max().x && b.min().x < a.max().x && a.min().y < b.max().y && b.min().y < a.max().y
}

/// Where silkscreen and pads are on each side once footprints are placed, for fitting the
/// texts of those that moved where they overlap nothing.
struct Silk {
    /// By side, front then back: what a text may not overlap.
    taken: [Vec<Rect>; 2],
    /// By footprint, its room where it is placed.
    outlines: Vec<Option<Rect>>,
    /// The board's box.
    board: Option<Rect>,
    /// By footprint, whether it is on the back.
    backs: Vec<bool>,
}

impl Silk {
    fn new(board: &KicadBoard, placed: &[Placed]) -> Silk {
        let mut taken = [Vec::new(), Vec::new()];
        let layers = board.rules.spec().layers.len();
        for &(b, back) in &board.silkscreen {
            taken[usize::from(back)].push(b);
        }
        let mut outlines = Vec::new();
        let mut backs = Vec::new();
        for (i, fp) in board.footprints.iter().enumerate() {
            let p = placed.get(i).copied().unwrap_or(Placed {
                at: fp.at,
                rotation: fp.angle,
                side: if fp.back { Side::Back } else { Side::Front },
            });
            let still = p.at == fp.at
                && normalized(p.rotation) == normalized(fp.angle)
                && (p.side == Side::Back) == fp.back;
            let side = usize::from(p.side == Side::Back);
            let flip = (p.side == Side::Back) != fp.back;
            for &b in &fp.silkscreen {
                taken[side].push(moved(b, fp, &p));
            }
            if still {
                taken[side].extend(fp.silk_texts.iter().copied());
            }
            for pad in &board.board.pads[fp.board_pads.clone()] {
                let b = moved(pad.shape.bbox(), fp, &p);
                let front = pad.layers.contains(crate::geom::LayerId(0));
                let back =
                    layers > 0 && pad.layers.contains(crate::geom::LayerId(layers as u8 - 1));
                let (front, back) = if flip { (back, front) } else { (front, back) };
                if front || pad.hole.is_some() {
                    taken[0].push(b);
                }
                if back || pad.hole.is_some() {
                    taken[1].push(b);
                }
            }
            outlines.push(footprint_outline(board, i).map(|o| moved(o, fp, &p)));
            backs.push(p.side == Side::Back);
        }
        let segments = edge_segments(&board.routing.outline);
        let board = segments
            .iter()
            .map(|&(a, b)| Rect::new(a, b))
            .reduce(|a, b| a.union(&b));
        Silk {
            taken,
            outlines,
            board,
            backs,
        }
    }

    /// How far a text of footprint `f`, whose box where the footprint is placed is `text`,
    /// moves on the board: not at all if it overlaps nothing there, or to be centred above,
    /// below, left or right of the footprint's outline, the first that overlaps nothing; or
    /// `None` if none fits, and it is to be hidden. A text that goes somewhere is then taken.
    fn fit(&mut self, f: usize, text: &Rect) -> Option<Point> {
        let side = usize::from(self.backs[f]);
        let (w, h) = (text.max().x - text.min().x, text.max().y - text.min().y);
        let centre = Point::new(text.min().x + w / 2, text.min().y + h / 2);
        let mut candidates = vec![centre];
        if let Some(o) = self.outlines[f] {
            let gap = 50_000;
            let (cx, cy) = ((o.min().x + o.max().x) / 2, (o.min().y + o.max().y) / 2);
            candidates.extend([
                Point::new(cx, o.max().y + h / 2 + gap),
                Point::new(cx, o.min().y - h / 2 - gap),
                Point::new(o.min().x - w / 2 - gap, cy),
                Point::new(o.max().x + w / 2 + gap, cy),
            ]);
        }
        for c in candidates {
            let b = Rect::new(
                Point::new(c.x - w / 2, c.y - h / 2),
                Point::new(c.x - w / 2 + w, c.y - h / 2 + h),
            );
            let inside = self.board.is_none_or(|board| {
                board.min().x <= b.min().x
                    && b.max().x <= board.max().x
                    && board.min().y <= b.min().y
                    && b.max().y <= board.max().y
            });
            if !inside || self.taken[side].iter().any(|t| overlap(t, &b)) {
                continue;
            }
            self.taken[side].push(b);
            return Some(Point::new(c.x - centre.x, c.y - centre.y));
        }
        None
    }
}

/// The edit that hides a text: `(hide yes)` from KiCad 8, a bare `hide` before.
fn hide(
    text: &str,
    item: &Sexp<'_>,
    at: &Sexp<'_>,
    kicad8: bool,
    edits: &mut Vec<(usize, usize, String)>,
) {
    if item.flag("hide") {
        return;
    }
    let Some((_, end)) = at.items().last().and_then(|a| span(text, a)) else {
        return;
    };
    let Some(close) = text[end..].find(')') else {
        return;
    };
    let after = end + close + 1;
    let what = if kicad8 { " (hide yes)" } else { " hide" };
    edits.push((after, after, what.to_string()));
}

/// Replaces where footprints are, and returns the new text.
///
/// `placed` has an entry for each of the board's [footprints](KicadBoard::footprints), in
/// order, as [`place_components`](crate::pcb::place::place_components) returns them for a
/// design from [`placement_design`]; extra entries are ignored, and footprints with none stay.
/// A footprint that moves or turns gets its new position and angle, and its pads, texts
/// and fields, whose angles a board file holds with the footprint's own included, turn with
/// it; rule areas and zones that it holds, which a board file holds in board coordinates,
/// move with it. A footprint put on the other side is flipped as KiCad flips it, top to
/// bottom: its angle negated and everything it holds mirrored, layers, texts and all, before
/// it turns. Its 3D model is left as it was. Everything else is kept as it was, byte for byte,
/// but the routing: every track, arc and via, every zone's fill, and teardrops, all made for
/// the old placement, are taken out, as [`replace_routing`](super::replace_routing) does.
///
/// ```
/// use guide::geom::Point;
/// use guide::pcb::place::{Placed, Side};
///
/// let pcb = r#"(kicad_pcb (version 20240108) (generator "pcbnew")
///   (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
///   (net 0 "") (net 1 "A")
///   (footprint "R" (layer "F.Cu") (at 10 20 90)
///     (property "Reference" "R1" (at 0 -1 90) (layer "F.SilkS"))
///     (pad "1" smd rect (at -1 0 90) (size 1 1) (layers "F.Cu") (net 1 "A"))))"#;
/// let read = guide::kicad::read(pcb, None)?;
/// let placed = [Placed { at: Point::new(5_000_000, -6_000_000), rotation: 180_000, side: Side::Front }];
/// let text = guide::kicad::replace_placement(pcb, &read, &placed);
/// assert!(text.contains("(at 5 6 180)"));
/// assert!(text.contains("(at -1 0 180)"));
/// let again = guide::kicad::read(&text, None)?;
/// assert_eq!(again.footprints[0].angle, 180_000);
/// assert_eq!(again.footprints[0].pads[0].at, Point::new(6_000_000, -6_000_000));
/// # Ok::<(), guide::kicad::KicadError>(())
/// ```
pub fn replace_placement(pcb: &str, board: &KicadBoard, placed: &[Placed]) -> String {
    let Ok(root) = sexpr::parse(pcb) else {
        return pcb.to_string();
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let footprints = root
        .args()
        .iter()
        .filter(|n| matches!(n.name(), Some("footprint" | "module")));
    let kicad8 = root
        .child("version")
        .and_then(|v| v.arg(0))
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|v| v >= 20_240_108);
    let mut silk = board.silk_is_error.then(|| Silk::new(board, placed));
    for (f, ((node, fp), p)) in footprints.zip(&board.footprints).zip(placed).enumerate() {
        let flip = (p.side == Side::Back) != fp.back;
        if !flip && p.at == fp.at && normalized(p.rotation) == normalized(fp.angle) {
            continue;
        }
        // Flipped as KiCad flips, top to bottom, about its origin: its angle negated, and
        // everything in it mirrored; then turned.
        let was = if flip { -fp.angle } else { fp.angle };
        let turn = p.rotation - was;
        let Some(at) = node.child("at") else { continue };
        if set_at(
            pcb,
            at,
            Some((p.at.x, -p.at.y)),
            normalized(p.rotation),
            &mut edits,
        )
        .is_none()
        {
            continue;
        }
        if flip {
            flip_local(pcb, node, &mut edits);
        }
        let old = (fp.at.x, -fp.at.y);
        let new = (p.at.x, -p.at.y);
        let old_k = old;
        let move_rect = |r: Rect| moved(r, fp, p);
        for item in node.args() {
            match item.name() {
                Some(kind @ ("pad" | "fp_text" | "property")) => {
                    let Some(at) = item.child("at") else { continue };
                    let angle = at
                        .arg(2)
                        .and_then(super::units::degrees_to_millidegrees)
                        .unwrap_or(0);
                    let position = flip
                        .then(|| {
                            let x = at.arg(0).and_then(super::units::mm_to_nm)?;
                            let y = at.arg(1).and_then(super::units::mm_to_nm)?;
                            Some((x, -y))
                        })
                        .flatten();
                    let angle = match (flip, kind) {
                        (false, _) => angle,
                        (true, "pad") => -angle,
                        (true, _) => 180_000 - angle,
                    };
                    let mut position = position;
                    if kind != "pad"
                        && let Some(silk) = silk.as_mut()
                        && let Some(b) = super::board::silk_text_box(item, old_k, fp.angle)
                    {
                        match silk.fit(f, &move_rect(b)) {
                            Some(shift) if shift != Point::new(0, 0) => {
                                // The text moves on the board as its box does: back to the
                                // footprint's frame, KiCad's, with Y down.
                                let here = position.or_else(|| {
                                    Some((
                                        at.arg(0).and_then(super::units::mm_to_nm)?,
                                        at.arg(1).and_then(super::units::mm_to_nm)?,
                                    ))
                                });
                                if let Some(here) = here {
                                    let (dx, dy) = rotate((shift.x, -shift.y), -p.rotation);
                                    position = Some((here.0 + dx, here.1 + dy));
                                }
                            }
                            Some(_) => {}
                            None => hide(pcb, item, at, kicad8, &mut edits),
                        }
                    }
                    set_at(pcb, at, position, normalized(angle + turn), &mut edits);
                }
                Some("zone") => {
                    for polygon in item.children("polygon") {
                        let Some(pts) = polygon.child("pts") else {
                            continue;
                        };
                        for xy in pts.children("xy") {
                            let (Some(x), Some(y)) = (
                                xy.arg(0).and_then(super::units::mm_to_nm),
                                xy.arg(1).and_then(super::units::mm_to_nm),
                            ) else {
                                continue;
                            };
                            let local = if flip {
                                (x - old.0, old.1 - y)
                            } else {
                                (x - old.0, y - old.1)
                            };
                            let (dx, dy) = rotate(local, turn);
                            if let (Some(a), Some(b)) = (
                                xy.args().first().and_then(|a| span(pcb, a)),
                                xy.args().get(1).and_then(|a| span(pcb, a)),
                            ) {
                                edits.push((
                                    a.0,
                                    b.1,
                                    format!("{} {}", mm(new.0 + dx), mm(new.1 + dy)),
                                ));
                            }
                        }
                    }
                    if flip {
                        for layers in item.children("layer").chain(item.children("layers")) {
                            swap_layers(pcb, layers, &mut edits);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = String::with_capacity(pcb.len());
    let mut at = 0;
    for (start, end, text) in edits {
        if start < at {
            continue;
        }
        out.push_str(&pcb[at..start]);
        out.push_str(&text);
        at = end;
    }
    out.push_str(&pcb[at..]);
    rewrite(&out, "", &[], true)
}

/// Where a list is in the text it was parsed from: from its `(` to its `)`.
fn list_span(text: &str, list: &Sexp<'_>) -> Option<(usize, usize)> {
    let (first, _) = span(text, list.items().first()?)?;
    let start = text[..first].rfind('(')?;
    if !text[start + 1..first].trim().is_empty() {
        return None;
    }
    let (_, last) = span(text, list.items().last()?)?;
    let end = last + text[last..].find(')')? + 1;
    Some((start, end))
}

/// Gives pads other nets, and returns the new text: for pin and gate swaps, as
/// [`pcb::swap`](crate::pcb::swap) finds them, and for engineering changes.
///
/// Each entry of `nets` is a footprint, by position in [`KicadBoard::footprints`], one of its
/// [pads](Footprint::pads), by position, and its new net, or `None` for no net. A pad's
/// `(net ...)` is written as another pad of that net has it in the file, so in the file's own
/// form, numbered or by name; a net that no pad has is written by name, as KiCad 10 writes
/// it, after its number if the file declares one. Everything else is kept, byte for byte.
///
/// ```
/// use guide::id::NetId;
///
/// let pcb = r#"(kicad_pcb (version 20240108) (generator "pcbnew")
///   (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
///   (net 0 "") (net 1 "A") (net 2 "B")
///   (footprint "R" (layer "F.Cu") (at 10 20)
///     (pad "1" smd rect (at -1 0) (size 1 1) (layers "F.Cu") (net 1 "A"))
///     (pad "2" smd rect (at 1 0) (size 1 1) (layers "F.Cu") (net 2 "B"))))"#;
/// let read = guide::kicad::read(pcb, None)?;
/// let (a, b) = (read.footprints[0].pads[0].net, read.footprints[0].pads[1].net);
/// let text = guide::kicad::replace_pad_nets(pcb, &read, &[(0, 0, b), (0, 1, a)]);
/// let again = guide::kicad::read(&text, None)?;
/// assert_eq!(again.nets[again.footprints[0].pads[0].net.unwrap().index()], "B");
/// assert_eq!(again.nets[again.footprints[0].pads[1].net.unwrap().index()], "A");
/// # Ok::<(), guide::kicad::KicadError>(())
/// ```
pub fn replace_pad_nets(
    pcb: &str,
    board: &KicadBoard,
    nets: &[(usize, usize, Option<crate::id::NetId>)],
) -> String {
    let Ok(root) = sexpr::parse(pcb) else {
        return pcb.to_string();
    };
    let footprints: Vec<&Sexp<'_>> = root
        .args()
        .iter()
        .filter(|n| matches!(n.name(), Some("footprint" | "module")))
        .collect();
    // By footprint, its pads as the reader takes them: those with a position.
    let pads_of = |node: &Sexp<'_>| -> Vec<usize> {
        node.args()
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is("pad") && n.child("at").is_some())
            .map(|(i, _)| i)
            .collect()
    };
    // Each net's `(net ...)`, as a pad of it has it.
    let mut written: std::collections::BTreeMap<crate::id::NetId, String> = Default::default();
    for (node, fp) in footprints.iter().zip(&board.footprints) {
        for (k, i) in pads_of(node).into_iter().enumerate() {
            let (Some(pad), Some(Some(net))) = (node.args().get(i), fp.pads.get(k).map(|p| p.net))
            else {
                continue;
            };
            if let Some((a, b)) = pad.child("net").and_then(|n| list_span(pcb, n)) {
                written.entry(net).or_insert_with(|| pcb[a..b].to_string());
            }
        }
    }
    let numbers: std::collections::BTreeMap<&str, &str> = root
        .children("net")
        .filter_map(|n| Some((n.arg(1)?, n.arg(0)?)))
        .collect();
    let text_of = |net: crate::id::NetId| -> String {
        written.get(&net).cloned().unwrap_or_else(|| {
            let name = board.nets.get(net.index()).map_or("", String::as_str);
            let quoted = format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""));
            match numbers.get(name) {
                Some(n) => format!("(net {n} {quoted})"),
                None => format!("(net {quoted})"),
            }
        })
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for &(f, k, net) in nets {
        let Some(node) = footprints.get(f) else {
            continue;
        };
        let Some(&i) = pads_of(node).get(k) else {
            continue;
        };
        let pad = &node.args()[i];
        match (pad.child("net").and_then(|n| list_span(pcb, n)), net) {
            (Some((a, b)), Some(net)) => edits.push((a, b, text_of(net))),
            (Some((a, b)), None) => {
                // The space before it goes too.
                let a = pcb[..a].trim_end().len();
                edits.push((a, b, String::new()));
            }
            (None, Some(net)) => {
                if let Some((_, end)) = list_span(pcb, pad) {
                    edits.push((end - 1, end - 1, format!(" {}", text_of(net))));
                }
            }
            (None, None) => {}
        }
    }
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = String::with_capacity(pcb.len());
    let mut at = 0;
    for (start, end, text) in edits {
        if start < at {
            continue;
        }
        out.push_str(&pcb[at..start]);
        out.push_str(&text);
        at = end;
    }
    out.push_str(&pcb[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PCB: &str = r#"(kicad_pcb (version 20240108) (generator "pcbnew")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
  (net 0 "") (net 1 "A") (net 2 "B")
  (footprint "C" (layer "F.Cu") (at 10 20 90)
    (property "Reference" "C1" (at 0 -1.5 90) (layer "F.SilkS") (effects (font (size 1 1))))
    (fp_line (start -1 -0.5) (end 1 -0.5) (stroke (width 0.05)) (layer "F.CrtYd"))
    (pad "1" smd roundrect (at -0.5 0.25 90) (size 0.6 0.5) (layers "F.Cu" "F.Mask")
      (chamfer top_left) (net 1 "A"))
    (pad "2" smd rect (at 0.5 0 90) (size 0.6 0.5) (layers "F.Cu" "F.Mask") (net 2 "B"))))"#;

    #[test]
    fn flips_as_kicad_does() {
        let read = crate::kicad::read(PCB, None).unwrap();
        let placed = [Placed {
            at: Point::new(30_000_000, -40_000_000),
            rotation: 0,
            side: Side::Back,
        }];
        let text = replace_placement(PCB, &read, &placed);
        // Its layers swap and its points mirror; flipped, its angle is -90°, which turns
        // by 90° to 0°, and so do its pads', and texts' at 180° less theirs.
        assert!(text.contains(r#"(layer "B.Cu") (at 30 40 0)"#), "{text}");
        assert!(
            text.contains(r#"(at 0 1.5 180) (layer "B.SilkS") (effects (justify mirror) (font"#)
        );
        assert!(text.contains("(start -1 0.5) (end 1 0.5)"));
        assert!(text.contains(r#"(at -0.5 -0.25 0) (size 0.6 0.5) (layers "B.Cu" "B.Mask")"#));
        assert!(text.contains("(chamfer bottom_left)"));
        let again = crate::kicad::read(&text, None).unwrap();
        let fp = &again.footprints[0];
        assert!(fp.back && fp.angle == 0 && fp.courtyard.is_some());
        let design = placement_design(&read);
        assert_eq!(
            fp.pads.iter().map(|p| p.at).collect::<Vec<_>>(),
            crate::pcb::place::pin_positions(&design, 0, &placed[0])
        );
    }
}
