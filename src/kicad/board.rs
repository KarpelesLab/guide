//! Reading a `.kicad_pcb` into a [`drc::Board`].

use std::collections::BTreeMap;

use super::project::{self, ClassDef, ProjectRules};
use super::sexpr::{self, Sexp};
use super::units::{
    degrees_to_millidegrees, mm_to_nm, ratio_to_millionths, rotate, times_ratio, to_point,
};
use super::{KicadBoard, KicadError, NetRouting, Routing, Source};
use crate::geom::drc::{self, Pad, Track, Via, Zone};
use crate::geom::{
    LayerId, LayerSet, NetClass, Overrides, Point, RuleSpec, Rules, Shape, ShapeIndex,
};
use crate::pcb::MaskOpening;

/// The tolerance for arcs in graphics and custom pads, which are approximated as they are
/// read: 10 nm, far inside KiCad's slack of 500 nm. Arc tracks are approximated by
/// [`drc::check`], with the board's [`arc_tolerance`](drc::Board::arc_tolerance).
pub(crate) const GRAPHIC_ARC_TOLERANCE: i64 = 10;
use crate::id::NetId;

/// A vector in KiCad's coordinates, nanometres with Y down.
type Vec2 = (i64, i64);

fn add(a: Vec2, b: Vec2) -> Vec2 {
    (a.0 + b.0, a.1 + b.1)
}

/// Reads a board, and its project's rules if there is a project.
///
/// `pcb` is the text of a `.kicad_pcb`, and `project` that of the `.kicad_pro` beside it.
/// Without a project, the rules are KiCad 5's from the board file, if it has them, or
/// else KiCad's defaults: a clearance of 0.2 mm, a hole-to-hole clearance of 0.25 mm, and
/// no hole clearance.
///
/// Before returning, the reader gives copper the nets that KiCad's connectivity would, as
/// KiCad does each time it loads a board. Copper that touches on a layer it shares is a
/// cluster. If the cluster's pads have one net between them, or none of them has a net and
/// another item does, every track, via and graphic in it takes that net, whatever the file
/// says, while pads and zones keep theirs. A cluster with pads of several nets is shorted,
/// and is left as it is. KiCad joins items by their anchors, such as the ends of tracks,
/// where this joins any that touch, so the two can differ for copper that crosses with no
/// pad between. Each change is counted in the [warnings](KicadBoard::warnings).
///
/// Fails only if the text is not an S-expression, is not a board, or holds rules that
/// [`Rules`] rejects. Items it can't make sense of are skipped with a
/// [warning](KicadBoard::warnings).
///
/// ```
/// use guide::geom::drc::{self, Violation};
///
/// let pcb = r#"(kicad_pcb (version 20240108) (generator "pcbnew")
///   (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
///   (net 0 "") (net 1 "A") (net 2 "B")
///   (segment (start 0 0) (end 10 0) (width 0.25) (layer "F.Cu") (net 1) (uuid "t1"))
///   (segment (start 0 0.4) (end 10 0.4) (width 0.25) (layer "F.Cu") (net 2) (uuid "t2")))"#;
/// let read = guide::kicad::read(pcb, None)?;
/// assert_eq!(read.nets[1], "A");
/// // 0.4 mm apart, less half of each width: 0.15 mm, where KiCad's default is 0.2 mm.
/// let found = drc::check(&read.board, &read.rules)?;
/// let [Violation::Clearance { a, b, required: 200_000, .. }] = found[..] else { panic!() };
/// assert_eq!(read.source(a).unwrap().uuid.as_deref(), Some("t1"));
/// assert_eq!(read.source(b).unwrap().uuid.as_deref(), Some("t2"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn read(pcb: &str, project: Option<&str>) -> Result<KicadBoard, KicadError> {
    let root = sexpr::parse(pcb)?;
    if !root.is("kicad_pcb") {
        return Err(KicadError::other(
            "not a KiCad board: it doesn't start with kicad_pcb",
        ));
    }
    let mut r = Reader::new(&root);
    let rules = match project {
        Some(text) => project::parse(text)?,
        None => r.legacy_rules(&root),
    };
    for item in root.args() {
        match item.name() {
            Some("segment") => r.track(item, false),
            Some("arc") => r.track(item, true),
            Some("via") => r.via(item),
            Some("footprint" | "module") => r.footprint(item),
            Some("zone") => r.zone(item, None),
            Some(name) if name.starts_with("gr_") => r.graphic(item, None),
            _ => {}
        }
    }
    let project = rules;
    let rules = r.rules(&project)?;
    r.propagate();
    r.mask_nets();
    r.rank_zones();
    let Reader {
        board,
        names,
        tracks,
        vias,
        pads,
        zones,
        warnings,
        outline,
        masks,
        text,
        zone_specs,
        zone_lists,
        ..
    } = r;
    let mut routing = routing(&project, &names, outline);
    routing.masks = masks;
    routing.text = text;
    routing.zones = zone_specs;
    routing.zone_lists = zone_lists;
    Ok(KicadBoard {
        board,
        rules,
        nets: names,
        tracks,
        vias,
        pads,
        zones,
        warnings: warnings.into_iter().collect(),
        routing,
    })
}

/// A box that holds a text on copper, with its stroke, turned as the text is. KiCad's
/// stroke font is narrower than its size, so the box is wider than the text.
fn text_box(node: &Sexp<'_>, place: Place) -> Option<Shape> {
    // `(gr_text "text" ...)`, or `(fp_text kind "text" ...)`.
    let text = if node.is("fp_text") {
        node.arg(1)?
    } else {
        node.arg(0)?
    };
    let at = node.child("at");
    let pos = at.and_then(args_pair)?;
    let angle = at
        .and_then(|a| a.arg(2))
        .and_then(degrees_to_millidegrees)
        .unwrap_or(0);
    let effects = node.child("effects");
    let font = effects.and_then(|e| e.child("font"));
    let size = font
        .and_then(|f| pair(f, "size"))
        .unwrap_or((1_000_000, 1_000_000));
    let thickness = font
        .and_then(|f| length(f, "thickness"))
        .unwrap_or(size.1 / 8);
    let lines: Vec<&str> = text.split("\\n").collect();
    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as i64;
    let w = longest * size.1 + thickness;
    let h = lines.len() as i64 * size.0 * 3 / 2 + thickness;
    let justify = effects.and_then(|e| e.child("justify"));
    let has =
        |what: &str| justify.is_some_and(|j| j.args().iter().any(|a| a.as_atom() == Some(what)));
    let (x0, x1) = match (has("left"), has("right"), has("mirror")) {
        (_, _, true) => (-w, w),
        (true, _, _) => (0, w),
        (_, true, _) => (-w, 0),
        _ => (-w / 2, w - w / 2),
    };
    let (y0, y1) = match (has("top"), has("bottom")) {
        (true, _) => (0, h),
        (_, true) => (-h, 0),
        _ => (-h / 2, h - h / 2),
    };
    let base = place.apply(pos);
    let corners: Vec<Point> = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        .into_iter()
        .map(|c| to_point(add(base, rotate(c, angle))))
        .collect();
    Shape::polygon(&corners, &[]).ok()
}

/// What new routes of each net take, from the classes, and the board's edge.
fn routing(project: &ProjectRules, names: &[String], outline: Vec<Shape>) -> Routing {
    let defaults = project::defaults().classes.remove(0);
    let default = project
        .classes
        .iter()
        .find(|c| c.name == "Default")
        .unwrap_or(&defaults);
    let mut nets = Vec::with_capacity(names.len());
    for name in names {
        let class = project
            .routing_class(name)
            .map_or(default, |c| &project.classes[c]);
        let pick = |f: fn(&ClassDef) -> Option<i64>| {
            f(class)
                .or_else(|| f(default))
                .or_else(|| f(&defaults))
                .unwrap_or(0)
        };
        // The class's via, made as large as the board's minimums ask: KiCad checks those
        // whatever the class says.
        let drill = pick(|c| c.via_drill);
        let drill = if drill > 0 {
            drill.max(project.min_hole)
        } else {
            drill
        };
        let diameter = pick(|c| c.via_diameter);
        let diameter = if diameter > 0 && drill > 0 {
            diameter
                .max(project.min_via_diameter)
                .max(drill + 2 * project.min_via_annular)
        } else {
            diameter
        };
        nets.push(NetRouting {
            track_width: pick(|c| c.track_width).max(project.min_track_width).max(1),
            via_diameter: diameter,
            via_drill: drill,
        });
    }
    // Differential pairs, by their names, at the class's width and gap: the class that
    // gives the positive net its track width, or failing that, `Default`.
    let ids: BTreeMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut pairs = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let Some(partner) = project::pair_partner(name) else {
            continue;
        };
        let Some(&j) = ids.get(partner.as_str()) else {
            continue;
        };
        // Each pair once, from its positive net: `P` or `+`.
        let stem = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '_');
        let positive = stem.ends_with('P') || stem.ends_with('+');
        if !positive || name.is_empty() {
            continue;
        }
        let class = project
            .routing_class(name)
            .map_or(default, |c| &project.classes[c]);
        let pick = |f: fn(&ClassDef) -> Option<i64>| {
            f(class)
                .or_else(|| f(default))
                .or_else(|| f(&defaults))
                .unwrap_or(0)
        };
        pairs.push(crate::pcb::DiffPair {
            positive: NetId(i as u32),
            negative: NetId(j as u32),
            width: pick(|c| c.pair_width).max(project.min_track_width).max(1),
            gap: pick(|c| c.pair_gap),
        });
    }
    Routing {
        outline,
        edge_clearance: project.edge_clearance,
        nets,
        masks: Vec::new(),
        text: Vec::new(),
        pairs,
        zones: crate::pcb::Zones::default(),
        zone_lists: Vec::new(),
    }
}

/// Where a footprint puts what it holds.
#[derive(Clone, Copy)]
struct Place {
    at: Vec2,
    angle: i32,
}

impl Place {
    const BOARD: Place = Place {
        at: (0, 0),
        angle: 0,
    };

    fn apply(self, local: Vec2) -> Vec2 {
        add(self.at, rotate(local, self.angle))
    }
}

/// A footprint, as its pads and graphics need it.
struct Footprint {
    place: Place,
    reference: String,
    clearance: Option<i64>,
    /// Its pads' solder mask expansion, if it sets one.
    mask_margin: Option<i64>,
    /// How zones join its pads, and their thermal reliefs, if it says.
    zone: crate::pcb::PadConnection,
}

/// Whether KiCad's connectivity may give an item another net: tracks, vias and graphics
/// can, pads and zones can't.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Fixed,
    Changeable,
}

struct Reader {
    /// The copper layers' names, by `LayerId`.
    copper: Vec<String>,
    /// KiCad's net numbers, for files that refer to nets by number.
    numbers: BTreeMap<i64, String>,
    /// Net names to IDs, and back.
    ids: BTreeMap<String, NetId>,
    names: Vec<String>,
    board: drc::Board,
    tracks: Vec<Source>,
    vias: Vec<Source>,
    pads: Vec<Source>,
    zones: Vec<Source>,
    /// Whether each track and zone may change net. Vias always can, and pads never.
    track_kinds: Vec<Kind>,
    zone_kinds: Vec<Kind>,
    warnings: BTreeMap<String, usize>,
    /// The board's edge, from `Edge.Cuts`.
    outline: Vec<Shape>,
    /// Openings in the solder mask, from pads and graphics on the mask layers.
    masks: Vec<MaskOpening>,
    /// Boxes holding text on copper, which isn't read as copper.
    text: Vec<(Shape, LayerId)>,
    /// The board's solder mask expansion of pads.
    mask_margin: i64,
    /// Zones' outlines, pads' zone connections, and pour keepouts.
    zone_specs: crate::pcb::Zones,
    /// By zone of `zone_specs`, the position of its list among all zones'.
    zone_lists: Vec<usize>,
    /// How many `(zone ...)` lists have been read.
    zone_count: usize,
    /// By zone of `zone_specs`, its UUID, which breaks ties between priorities.
    zone_uuids: Vec<String>,
}

/// The text of the UUID of an item: `uuid` from KiCad 7 on, `tstamp` before.
fn uuid(node: &Sexp<'_>) -> Option<String> {
    node.child("uuid")
        .or_else(|| node.child("tstamp"))
        .and_then(|u| u.arg(0))
        .map(str::to_string)
}

/// A length in a child list, as in `(width 0.25)`.
fn length(node: &Sexp<'_>, name: &str) -> Option<i64> {
    node.child(name).and_then(|c| c.arg(0)).and_then(mm_to_nm)
}

/// A pair of lengths in a child list, as in `(start 1 2)`.
fn pair(node: &Sexp<'_>, name: &str) -> Option<Vec2> {
    node.child(name).and_then(args_pair)
}

fn args_pair(node: &Sexp<'_>) -> Option<Vec2> {
    Some((mm_to_nm(node.arg(0)?)?, mm_to_nm(node.arg(1)?)?))
}

/// The points of a `(pts (xy x y) ...)` list, with how many arcs it held, which are taken
/// as their end points.
fn points(node: &Sexp<'_>) -> Option<(Vec<Vec2>, usize)> {
    let pts = node.child("pts")?;
    let mut out = Vec::new();
    let mut arcs = 0;
    for p in pts.args() {
        match p.name() {
            Some("xy") => out.push(args_pair(p)?),
            Some("arc") => {
                arcs += 1;
                out.push(pair(p, "start")?);
                out.push(pair(p, "mid")?);
                out.push(pair(p, "end")?);
            }
            _ => {}
        }
    }
    Some((out, arcs))
}

/// The stroke width of a graphic: `(stroke (width w))` from KiCad 7, `(width w)` before.
fn stroke_width(node: &Sexp<'_>) -> Option<i64> {
    node.child("stroke")
        .and_then(|s| length(s, "width"))
        .or_else(|| length(node, "width"))
}

/// Whether a graphic is filled: `(fill solid)` or `(fill yes)`, or KiCad 5's polygons,
/// which were always filled.
fn filled(node: &Sexp<'_>) -> bool {
    match node.child("fill").and_then(|f| f.arg(0)) {
        Some(fill) => matches!(fill, "solid" | "yes"),
        None => node.is("gr_poly") || node.is("fp_poly"),
    }
}

/// What a pad or a footprint says of how zones join it, falling back on `outer`, the
/// footprint's, for what a pad doesn't say.
fn pad_connection(
    node: &Sexp<'_>,
    outer: Option<&crate::pcb::PadConnection>,
) -> crate::pcb::PadConnection {
    use crate::pcb::ZoneConnection;
    let outer = outer.copied().unwrap_or_default();
    let connection = match node.child("zone_connect").and_then(|z| z.arg(0)) {
        Some("0") => Some(ZoneConnection::None),
        Some("1") => Some(ZoneConnection::Thermal),
        Some("2") => Some(ZoneConnection::Solid),
        Some("3") => Some(ZoneConnection::ThermalThroughHole),
        _ => outer.connection,
    };
    let positive = |v: Option<i64>| v.filter(|&v| v > 0);
    crate::pcb::PadConnection {
        connection,
        thermal_gap: positive(length(node, "thermal_gap")).or(outer.thermal_gap),
        spoke_width: positive(
            length(node, "thermal_bridge_width").or_else(|| length(node, "thermal_width")),
        )
        .or(outer.spoke_width),
        anchor: None,
        spoke_direction: None,
        spoke_reach: None,
    }
}

/// An own clearance as KiCad stores it: zero is none.
fn own_clearance(value: Option<i64>) -> Option<i64> {
    value.filter(|&v| v > 0)
}

impl Reader {
    fn new(root: &Sexp<'_>) -> Reader {
        let mut copper: Vec<(u32, String)> = Vec::new();
        if let Some(layers) = root.child("layers") {
            for layer in layers.args() {
                let Some(name) = layer.arg(0) else { continue };
                // F.Cu first, then the inner layers by number, then B.Cu.
                let order = match name {
                    "F.Cu" => 0,
                    "B.Cu" => u32::MAX,
                    _ => match name.strip_prefix("In").and_then(|n| n.strip_suffix(".Cu")) {
                        Some(n) => match n.parse::<u32>() {
                            Ok(n) => n,
                            Err(_) => continue,
                        },
                        None => continue,
                    },
                };
                copper.push((order, name.to_string()));
            }
        }
        copper.sort();
        copper.dedup();
        let mut numbers = BTreeMap::new();
        for net in root.children("net") {
            if let (Some(n), Some(name)) = (net.arg(0).and_then(|n| n.parse().ok()), net.arg(1)) {
                numbers.insert(n, name.to_string());
            }
        }
        let mut r = Reader {
            copper: copper.into_iter().map(|(_, name)| name).collect(),
            numbers,
            ids: BTreeMap::new(),
            names: Vec::new(),
            board: drc::Board::default(),
            tracks: Vec::new(),
            vias: Vec::new(),
            pads: Vec::new(),
            zones: Vec::new(),
            track_kinds: Vec::new(),
            zone_kinds: Vec::new(),
            warnings: BTreeMap::new(),
            outline: Vec::new(),
            masks: Vec::new(),
            text: Vec::new(),
            mask_margin: root
                .child("setup")
                .and_then(|s| length(s, "pad_to_mask_clearance"))
                .unwrap_or(0),
            zone_specs: crate::pcb::Zones::default(),
            zone_lists: Vec::new(),
            zone_count: 0,
            zone_uuids: Vec::new(),
        };
        // Copper with no net is of the net with no name, so that it is never checked
        // against itself, as in KiCad.
        r.net_id("");
        let declared: Vec<String> = r.numbers.values().cloned().collect();
        for name in declared {
            r.net_id(&name);
        }
        r
    }

    fn warn(&mut self, message: impl Into<String>) {
        *self.warnings.entry(message.into()).or_insert(0) += 1;
    }

    fn net_id(&mut self, name: &str) -> NetId {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = NetId(self.names.len() as u32);
        self.ids.insert(name.to_string(), id);
        self.names.push(name.to_string());
        id
    }

    /// The net of an item: `(net 3)` or `(net 3 "name")` up to KiCad 9, `(net "name")`
    /// from KiCad 10.
    fn net(&mut self, node: &Sexp<'_>) -> NetId {
        let Some(net) = node.child("net") else {
            return NetId(0);
        };
        let name = match (net.arg(0), net.arg(1)) {
            (Some(n), name) if n.parse::<i64>().is_ok() && !self.numbers.is_empty() => {
                let n: i64 = n.parse().unwrap_or(0);
                match self.numbers.get(&n) {
                    Some(name) => name.clone(),
                    None => name.unwrap_or("").to_string(),
                }
            }
            (Some(name), _) => name.to_string(),
            (None, _) => String::new(),
        };
        self.net_id(&name)
    }

    fn layer(&self, name: &str) -> Option<LayerId> {
        let i = self.copper.iter().position(|l| l == name)?;
        Some(LayerId(i as u8))
    }

    /// The copper layers named in a `(layers ...)` list, with KiCad's wildcards.
    fn layer_set(&self, node: Option<&Sexp<'_>>) -> LayerSet {
        let mut set = LayerSet::EMPTY;
        let Some(last) = self.copper.len().checked_sub(1) else {
            return set;
        };
        for name in node
            .map_or(&[][..], Sexp::args)
            .iter()
            .filter_map(Sexp::as_atom)
        {
            match name {
                "*.Cu" => set = LayerSet::first(self.copper.len()),
                "F&B.Cu" => set = set.with(LayerId(0)).with(LayerId(last as u8)),
                name => {
                    if let Some(l) = self.layer(name) {
                        set = set.with(l);
                    }
                }
            }
        }
        set
    }

    fn track(&mut self, node: &Sexp<'_>, arc: bool) {
        let what = if arc { "arc" } else { "track" };
        let Some(layer) = node.child("layer").and_then(|l| l.arg(0)) else {
            return self.warn(format!("{what} with no layer, skipped"));
        };
        let Some(l) = self.layer(layer) else { return };
        let (Some(start), Some(end), Some(width)) = (
            pair(node, "start"),
            pair(node, "end"),
            length(node, "width"),
        ) else {
            return self.warn(format!("malformed {what}, skipped"));
        };
        let mid = if arc {
            match pair(node, "mid") {
                Some(mid) => Some(to_point(mid)),
                None => return self.warn("arc with no mid point, skipped"),
            }
        } else {
            None
        };
        if width <= 0 {
            return self.warn(format!("{what} of zero width, skipped"));
        }
        let net = self.net(node);
        self.push_track(
            Track {
                start: to_point(start),
                end: to_point(end),
                mid,
                width,
                layer: l,
                net: Some(net),
            },
            Source {
                uuid: uuid(node),
                description: format!("{what} on {layer}"),
            },
            Kind::Changeable,
        );
    }

    fn push_track(&mut self, track: Track, source: Source, kind: Kind) {
        self.board.tracks.push(track);
        self.tracks.push(source);
        self.track_kinds.push(kind);
    }

    fn push_zone(&mut self, zone: Zone, source: Source, kind: Kind) {
        self.board.zones.push(zone);
        self.zones.push(source);
        self.zone_kinds.push(kind);
    }

    fn via(&mut self, node: &Sexp<'_>) {
        let (Some(at), Some(size), Some(drill)) = (
            pair(node, "at"),
            length(node, "size"),
            length(node, "drill"),
        ) else {
            return self.warn("malformed via, skipped");
        };
        let layers: Vec<LayerId> = node
            .child("layers")
            .map_or(&[][..], Sexp::args)
            .iter()
            .filter_map(Sexp::as_atom)
            .filter_map(|name| self.layer(name))
            .collect();
        let (Some(&from), Some(&to)) = (layers.iter().min(), layers.iter().max()) else {
            return self.warn("via with no copper layers on the board, skipped");
        };
        if size <= 0 || drill <= 0 {
            return self.warn("via of zero size or drill, skipped");
        }
        if node.flag("remove_unused_layers") {
            self.warn("pad or via that drops its copper where nothing connects to it: kept on every layer");
        }
        self.padstack(node);
        let net = self.net(node);
        self.board.vias.push(Via {
            at: to_point(at),
            drill,
            diameter: size,
            from,
            to,
            net: Some(net),
        });
        self.vias.push(Source {
            uuid: uuid(node),
            description: "via".into(),
        });
    }

    /// Warns about KiCad 9's padstacks that give other layers other shapes.
    fn padstack(&mut self, node: &Sexp<'_>) {
        if let Some(stack) = node.child("padstack")
            && stack.child("mode").and_then(|m| m.arg(0)) != Some("normal")
            && stack.child("layer").is_some()
        {
            self.warn("pad or via with other shapes on other layers: its first shape is used on every layer");
        }
    }

    fn footprint(&mut self, node: &Sexp<'_>) {
        let at = node.child("at");
        let place = Place {
            at: at.and_then(args_pair).unwrap_or((0, 0)),
            angle: at
                .and_then(|a| a.arg(2))
                .and_then(degrees_to_millidegrees)
                .unwrap_or(0),
        };
        // The reference is a property from KiCad 8, and text before.
        let reference = node
            .children("property")
            .find(|p| p.arg(0) == Some("Reference"))
            .and_then(|p| p.arg(1))
            .or_else(|| {
                node.children("fp_text")
                    .find(|t| t.arg(0) == Some("reference"))
                    .and_then(|t| t.arg(1))
            })
            .unwrap_or("")
            .to_string();
        let fp = Footprint {
            place,
            reference,
            clearance: own_clearance(
                length(node, "clearance").or_else(|| length(node, "local_clearance")),
            ),
            mask_margin: length(node, "solder_mask_margin"),
            zone: pad_connection(node, None),
        };
        for item in node.args() {
            match item.name() {
                Some("pad") => self.pad(&fp, item),
                Some("zone") => self.zone(item, Some(&fp)),
                Some(name) if name.starts_with("fp_") => self.graphic(item, Some(&fp)),
                _ => {}
            }
        }
    }

    fn pad(&mut self, fp: &Footprint, node: &Sexp<'_>) {
        let number = node.arg(0).unwrap_or("");
        let description = if number.is_empty() {
            format!("pad of {}", fp.reference)
        } else {
            format!("pad {number} of {}", fp.reference)
        };
        let kind = node.arg(1).unwrap_or("");
        let shape = node.arg(2).unwrap_or("");
        let layers = self.layer_set(node.child("layers"));
        let at = node.child("at");
        let (Some(local), Some(size)) = (at.and_then(args_pair), pair(node, "size")) else {
            return self.warn("malformed pad, skipped");
        };
        // The angle in a board file is the pad's own, the footprint's rotation included.
        let angle = at
            .and_then(|a| a.arg(2))
            .and_then(degrees_to_millidegrees)
            .unwrap_or(0);
        let center = fp.place.apply(local);
        let drill = node.child("drill");
        let offset = drill.and_then(|d| pair(d, "offset")).unwrap_or((0, 0));
        let hole = match drill {
            Some(d) => match self.hole(d, center, angle) {
                Ok(hole) => hole,
                Err(()) => return self.warn("pad with a malformed drill, skipped"),
            },
            None => None,
        };
        // A pad with no net is connected to nothing, and KiCad checks it against other pads
        // with no net, where tracks, vias, graphics and zones with no net are one net.
        let net = Some(self.net(node)).filter(|&n| n != NetId(0));
        if node.flag("remove_unused_layers") {
            self.warn("pad or via that drops its copper where nothing connects to it: kept on every layer");
        }
        self.padstack(node);
        let clearance = own_clearance(length(node, "clearance")).or(fp.clearance);
        let source = Source {
            uuid: uuid(node),
            description,
        };
        let geometry = PadGeometry {
            origin: add(center, rotate(offset, angle)),
            angle,
        };
        let mut zone = pad_connection(node, Some(&fp.zone));
        // Spokes along the pad's axes, or at 45° to them for a round pad, unless it says.
        let spoke = node
            .child("thermal_bridge_angle")
            .and_then(|a| a.arg(0))
            .and_then(degrees_to_millidegrees)
            .unwrap_or(if shape == "circle" { 45_000 } else { 0 });
        let (dx, dy) = rotate((1_000_000, 0), angle + spoke);
        zone.spoke_direction = Some((dx, -dy));
        zone.anchor = Some(to_point(geometry.origin));
        // KiCad lays spokes out on the pad's size, but a custom pad's reaches further.
        zone.spoke_reach = (shape != "custom").then_some((size.0 / 2, size.1 / 2));

        // A plain round or oval hole as big as its pad leaves no copper.
        let bare = kind == "np_thru_hole"
            && offset == (0, 0)
            && match (shape, drill) {
                ("circle", Some(d)) if !d.args().iter().any(|a| a.as_atom() == Some("oval")) => {
                    d.arg(0).and_then(mm_to_nm).is_some_and(|d| d >= size.0)
                }
                ("oval", Some(d)) => {
                    let (w, h) = (d.arg(1).and_then(mm_to_nm), d.arg(2).and_then(mm_to_nm));
                    d.arg(0) == Some("oval")
                        && w.is_some_and(|w| w >= size.0)
                        && h.is_some_and(|h| h >= size.1)
                }
                _ => false,
            };
        let shapes = if bare || layers == LayerSet::EMPTY {
            Vec::new()
        } else {
            match self.pad_shapes(node, shape, size, &geometry) {
                Some(shapes) => shapes,
                None => {
                    return self.warn(format!(
                        "pad of shape {shape:?} that could not be built, skipped"
                    ));
                }
            }
        };
        if shapes.is_empty() {
            // A hole with no copper still needs checking against copper and holes. Its
            // copper is the hole itself, on no layers.
            if let Some(hole) = hole {
                self.board.pads.push(Pad {
                    shape: hole.clone(),
                    layers: LayerSet::EMPTY,
                    hole: Some(hole),
                    net,
                    clearance,
                });
                self.pads.push(source);
                self.pad_zone(zone);
            }
            return;
        }
        let sides = self.mask_sides(node.child("layers"));
        let margin = length(node, "solder_mask_margin")
            .or(fp.mask_margin)
            .unwrap_or(self.mask_margin);
        let mut hole = hole;
        for shape in shapes {
            for &layer in &sides {
                self.masks.push(MaskOpening {
                    shape: shape.clone(),
                    layer,
                    margin,
                    net,
                });
            }
            self.board.pads.push(Pad {
                shape,
                layers,
                hole: hole.take(),
                net,
                clearance,
            });
            self.pads.push(source.clone());
            self.pad_zone(zone);
        }
    }

    /// What the pad just pushed says of zones.
    fn pad_zone(&mut self, zone: crate::pcb::PadConnection) {
        let n = self.board.pads.len();
        self.zone_specs
            .pads
            .resize(n, crate::pcb::PadConnection::default());
        self.zone_specs.pads[n - 1] = zone;
    }

    /// A pad's hole: round, or an oval slot, centred on the pad's position and turned with
    /// it.
    fn hole(&self, drill: &Sexp<'_>, center: Vec2, angle: i32) -> Result<Option<Shape>, ()> {
        let numbers: Vec<i64> = drill
            .args()
            .iter()
            .filter_map(Sexp::as_atom)
            .filter(|&a| a != "oval")
            .map(|a| mm_to_nm(a).ok_or(()))
            .collect::<Result<_, _>>()?;
        let oval = drill.args().iter().any(|a| a.as_atom() == Some("oval"));
        let (w, h) = match numbers[..] {
            [d] => (d, d),
            [w, h] => (w, h),
            [] => return Ok(None),
            _ => return Err(()),
        };
        if w <= 0 {
            return Ok(None);
        }
        let place = Place { at: center, angle };
        if !oval || w == h || h <= 0 {
            return Shape::circle(to_point(center), w).map(Some).map_err(|_| ());
        }
        let (hw, hh) = (w / 2, h / 2);
        let r = hw.min(hh);
        let (a, b) = ((-(hw - r), -(hh - r)), (hw - r, hh - r));
        Shape::track(to_point(place.apply(a)), to_point(place.apply(b)), 2 * r)
            .map(Some)
            .map_err(|_| ())
    }

    /// The copper of a pad, in one or more shapes.
    fn pad_shapes(
        &mut self,
        node: &Sexp<'_>,
        shape: &str,
        size: Vec2,
        g: &PadGeometry,
    ) -> Option<Vec<Shape>> {
        let (w, h) = size;
        if w <= 0 || h <= 0 {
            return None;
        }
        let (hw, hh) = (w / 2, h / 2);
        let rect = |g: &PadGeometry| g.polygon(&[(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)], 0);
        let one = |s: Option<Shape>| s.map(|s| vec![s]);
        match shape {
            "circle" => one(Shape::circle(g.point((0, 0)), 2 * (w / 2)).ok()),
            "rect" => one(rect(g)),
            "oval" => one(g.stadium(w, h)),
            "trapezoid" => {
                let (dx, dy) = node
                    .child("rect_delta")
                    .and_then(args_pair)
                    .map_or((0, 0), |(x, y)| (x / 2, y / 2));
                let corners = [
                    (-hw - dy, hh + dx),
                    (hw + dy, hh - dx),
                    (hw - dy, -hh + dx),
                    (-hw + dy, -hh - dx),
                ];
                one(g.polygon(&corners, 0))
            }
            "roundrect" => {
                let ratio = |name: &str| {
                    node.child(name)
                        .and_then(|r| r.arg(0))
                        .and_then(ratio_to_millionths)
                        .unwrap_or(0)
                };
                let radius = times_ratio(w.min(h), ratio("roundrect_rratio").min(500_000))
                    .clamp(0, hw.min(hh));
                let chamfer = times_ratio(w.min(h), ratio("chamfer_ratio").min(500_000));
                let corners = node.child("chamfer").map_or(&[][..], Sexp::args);
                let cut = |name: &str| corners.iter().any(|c| c.as_atom() == Some(name));
                if chamfer > 0 && !corners.is_empty() {
                    if radius > 0 {
                        self.warn("chamfered pad that is rounded as well: its other corners taken as sharp");
                    }
                    let c = chamfer.min(hw.min(hh));
                    let mut ring = Vec::new();
                    let mut corner = |name: &str, at: Vec2, before: Vec2, after: Vec2| {
                        if cut(name) {
                            ring.push(before);
                            ring.push(after);
                        } else {
                            ring.push(at);
                        }
                    };
                    // Clockwise on screen, from the top left, in KiCad's Y-down frame.
                    corner("top_left", (-hw, -hh), (-hw, -hh + c), (-hw + c, -hh));
                    corner("top_right", (hw, -hh), (hw - c, -hh), (hw, -hh + c));
                    corner("bottom_right", (hw, hh), (hw, hh - c), (hw - c, hh));
                    corner("bottom_left", (-hw, hh), (-hw + c, hh), (-hw, hh - c));
                    ring.dedup();
                    if ring.first() == ring.last() && ring.len() > 1 {
                        ring.pop();
                    }
                    return one(g.polygon(&ring, 0));
                }
                let (iw, ih) = (hw - radius, hh - radius);
                if radius == 0 {
                    one(rect(g))
                } else if iw == 0 && ih == 0 {
                    one(Shape::circle(g.point((0, 0)), 2 * radius).ok())
                } else if iw == 0 || ih == 0 {
                    one(Shape::track(g.point((-iw, -ih)), g.point((iw, ih)), 2 * radius).ok())
                } else {
                    one(g.polygon(&[(-iw, -ih), (iw, -ih), (iw, ih), (-iw, ih)], radius))
                }
            }
            "custom" => {
                let options = node.child("options");
                if options
                    .and_then(|o| o.child("clearance"))
                    .and_then(|c| c.arg(0))
                    == Some("convexhull")
                {
                    self.warn("custom pad whose clearance is from its convex hull: measured from its outline");
                }
                let anchor = options
                    .and_then(|o| o.child("anchor"))
                    .and_then(|a| a.arg(0))
                    .unwrap_or("circle");
                let mut shapes = Vec::new();
                shapes.extend(if anchor == "rect" {
                    rect(g)
                } else {
                    Shape::circle(g.point((0, 0)), 2 * (w / 2)).ok()
                });
                if let Some(primitives) = node.child("primitives") {
                    for p in primitives.args() {
                        shapes.extend(self.graphic_shapes(p, &|v| g.point(v), "custom pad")?);
                    }
                }
                Some(shapes)
            }
            _ => None,
        }
    }

    /// The shapes of a graphic, with `to` placing its points: strokes and fills alike.
    /// `None` if it is malformed; an empty list for what is skipped with a warning.
    fn graphic_shapes(
        &mut self,
        node: &Sexp<'_>,
        to: &dyn Fn(Vec2) -> Point,
        what: &str,
    ) -> Option<Vec<Shape>> {
        let edge = what == "board edge";
        let width = stroke_width(node).unwrap_or(0);
        // The board's edge is a line, whatever its width and fill.
        let (width, fill) = if edge {
            (width.max(1), false)
        } else {
            (width, filled(node))
        };
        let kind = node.name().unwrap_or("");
        let kind = kind
            .strip_prefix("gr_")
            .or_else(|| kind.strip_prefix("fp_"))
            .unwrap_or(kind);
        let stroke = |a: Vec2, b: Vec2| -> Option<Shape> { Shape::track(to(a), to(b), width).ok() };
        let mut out = Vec::new();
        match kind {
            "line" => {
                if width > 0 {
                    out.push(stroke(pair(node, "start")?, pair(node, "end")?)?);
                }
            }
            "arc" => {
                if width > 0 {
                    let (start, mid, end) = arc_points(node)?;
                    out.push(
                        Shape::arc_track(to(start), to(mid), to(end), width, GRAPHIC_ARC_TOLERANCE)
                            .ok()?,
                    );
                }
            }
            "circle" => {
                let center = pair(node, "center")?;
                let end = pair(node, "end")?;
                let radius = round_hypot(end.0 - center.0, end.1 - center.1);
                if fill {
                    out.push(Shape::circle(to(center), 2 * radius + width).ok()?);
                } else if width > 0 && radius > 0 {
                    let (a, b) = ((center.0 + radius, center.1), (center.0 - radius, center.1));
                    out.push(
                        Shape::arc_track(to(a), to(b), to(a), width, GRAPHIC_ARC_TOLERANCE).ok()?,
                    );
                }
            }
            "rect" | "poly" => {
                let ring = if kind == "rect" {
                    let (a, b) = (pair(node, "start")?, pair(node, "end")?);
                    vec![a, (b.0, a.1), b, (a.0, b.1)]
                } else {
                    let (ring, arcs) = points(node)?;
                    if arcs > 0 {
                        self.warn(format!(
                            "{what} outline with arcs: taken as straight between their points"
                        ));
                    }
                    ring
                };
                if ring.len() < 2 {
                    return None;
                }
                let ring: Vec<Point> = ring.into_iter().map(to).collect();
                if fill {
                    let radius = width / 2;
                    match Shape::rounded_polygon(&ring, &[], radius) {
                        Ok(s) => out.push(s),
                        Err(_) => {
                            // Not simple: take the area it encloses, and its outline.
                            out.extend(Shape::region(&[&ring]).ok()?);
                            if width > 0 {
                                for (i, &a) in ring.iter().enumerate() {
                                    let b = ring[(i + 1) % ring.len()];
                                    out.push(Shape::track(a, b, width).ok()?);
                                }
                            }
                        }
                    }
                } else if width > 0 {
                    let closed = ring.len() > 2;
                    let n = if closed { ring.len() } else { ring.len() - 1 };
                    for i in 0..n {
                        out.push(Shape::track(ring[i], ring[(i + 1) % ring.len()], width).ok()?);
                    }
                }
            }
            "curve" | "bezier" => self.warn(format!("Bézier curve in a {what}, skipped")),
            _ => {}
        }
        Some(out)
    }

    /// A graphic on copper, of the board or of a footprint.
    fn graphic(&mut self, node: &Sexp<'_>, fp: Option<&Footprint>) {
        let Some(layer) = node.child("layer").and_then(|l| l.arg(0)) else {
            return;
        };
        if layer == "Edge.Cuts" {
            return self.edge(node, fp);
        }
        if let Some(side) = self.mask_side(layer) {
            return self.mask_graphic(node, fp, side);
        }
        let Some(l) = self.layer(layer) else { return };
        let kind = node.name().unwrap_or("");
        if kind.ends_with("text") || kind.ends_with("text_box") || kind.ends_with("textbox") {
            if let Some(b) = text_box(node, fp.map_or(Place::BOARD, |f| f.place)) {
                self.text.push((b, l));
            }
            return self.warn("text on copper, skipped");
        }
        let place = fp.map_or(Place::BOARD, |f| f.place);
        let what = match fp {
            Some(fp) => format!("graphic of {} on {layer}", fp.reference),
            None => format!("graphic on {layer}"),
        };
        let Some(shapes) = self.graphic_shapes(node, &|v| to_point(place.apply(v)), "graphic")
        else {
            return self.warn("malformed graphic on copper, skipped");
        };
        let net = self.net(node);
        let source = Source {
            uuid: uuid(node),
            description: what,
        };
        // Zones take any shape, which tracks don't.
        for shape in shapes {
            self.push_zone(
                Zone {
                    shape,
                    layer: l,
                    net: Some(net),
                    clearance: None,
                },
                source.clone(),
                Kind::Changeable,
            );
        }
    }

    /// The outer copper layer under a solder mask layer.
    fn mask_side(&self, layer: &str) -> Option<LayerId> {
        let last = self.copper.len().checked_sub(1)?;
        match layer {
            "F.Mask" => Some(LayerId(0)),
            "B.Mask" => Some(LayerId(last as u8)),
            _ => None,
        }
    }

    /// The outer copper layers whose mask a pad's `(layers ...)` opens.
    fn mask_sides(&self, node: Option<&Sexp<'_>>) -> Vec<LayerId> {
        let Some(last) = self.copper.len().checked_sub(1) else {
            return Vec::new();
        };
        let (mut top, mut bottom) = (false, false);
        for name in node
            .map_or(&[][..], Sexp::args)
            .iter()
            .filter_map(Sexp::as_atom)
        {
            match name {
                "F.Mask" => top = true,
                "B.Mask" => bottom = true,
                "*.Mask" | "F&B.Mask" => (top, bottom) = (true, true),
                _ => {}
            }
        }
        let mut out = Vec::new();
        if top {
            out.push(LayerId(0));
        }
        if bottom && last > 0 {
            out.push(LayerId(last as u8));
        }
        out
    }

    /// A graphic on a solder mask layer: an opening in the mask.
    fn mask_graphic(&mut self, node: &Sexp<'_>, fp: Option<&Footprint>, layer: LayerId) {
        let kind = node.name().unwrap_or("");
        if kind.ends_with("text") || kind.ends_with("text_box") || kind.ends_with("textbox") {
            return;
        }
        let place = fp.map_or(Place::BOARD, |f| f.place);
        if let Some(shapes) = self.graphic_shapes(node, &|v| to_point(place.apply(v)), "mask") {
            for shape in shapes {
                self.masks.push(MaskOpening {
                    shape,
                    layer,
                    margin: 0,
                    net: None,
                });
            }
        }
    }

    /// Gives each opening from a graphic the net of the pads in it, if they have one net.
    /// An opening with pads of several nets bridges them already, and KiCad reports any
    /// other copper in it too, so it keeps the net `None`, which every net keeps out of.
    fn mask_nets(&mut self) {
        let pads = ShapeIndex::bulk_load(self.board.pads.iter().map(|p| p.shape.clone()));
        let mut near = Vec::new();
        for m in self.masks.iter_mut() {
            if m.net.is_some() || m.margin != 0 || pads.within(&m.shape, 1, &mut near).is_err() {
                continue;
            }
            let mut nets: Vec<Option<NetId>> = near
                .iter()
                .map(|id| &self.board.pads[id.slot() as usize])
                .filter(|p| p.layers.contains(m.layer))
                .map(|p| p.net)
                .collect();
            nets.sort_unstable();
            nets.dedup();
            if let [net] = nets[..] {
                m.net = net;
            }
        }
    }

    /// A graphic on `Edge.Cuts`, of the board or of a footprint: part of the board's edge.
    fn edge(&mut self, node: &Sexp<'_>, fp: Option<&Footprint>) {
        let kind = node.name().unwrap_or("");
        if kind.ends_with("text") || kind.ends_with("text_box") || kind.ends_with("textbox") {
            return;
        }
        let place = fp.map_or(Place::BOARD, |f| f.place);
        match self.graphic_shapes(node, &|v| to_point(place.apply(v)), "board edge") {
            Some(shapes) => self.outline.extend(shapes),
            None => self.warn("malformed board edge, skipped"),
        }
    }

    /// A rule area, as a keepout if it keeps tracks, vias or pads out.
    fn keepout(&mut self, node: &Sexp<'_>, rules: &Sexp<'_>) {
        let not_allowed =
            |what: &str| rules.child(what).and_then(|c| c.arg(0)) == Some("not_allowed");
        let (tracks, vias, pads) = (
            not_allowed("tracks"),
            not_allowed("vias"),
            not_allowed("pads"),
        );
        let pours = not_allowed("copperpour");
        if !(tracks || vias || pads || pours) {
            return;
        }
        // `(layers ...)`, or `(layer ...)` for a rule area on one layer.
        let layers = LayerSet(
            self.layer_set(node.child("layers")).0 | self.layer_set(node.child("layer")).0,
        );
        for polygon in node.children("polygon") {
            let Some((ring, arcs)) = points(polygon) else {
                self.warn("malformed rule area, skipped");
                continue;
            };
            if arcs > 0 {
                self.warn("zone outline with arcs: taken as straight between their points");
            }
            // A footprint's rule areas are saved in board coordinates, as its zones are.
            let ring: Vec<Point> = ring.into_iter().map(to_point).collect();
            let Ok(pieces) = Shape::region(&[&ring]) else {
                self.warn("rule area that could not be built, skipped");
                continue;
            };
            for shape in pieces {
                if pours {
                    self.zone_specs.keepouts.push((shape.clone(), layers));
                }
                if tracks || vias || pads {
                    self.board.keepouts.push(drc::Keepout {
                        shape,
                        layers,
                        tracks,
                        vias,
                        pads,
                    });
                }
            }
        }
    }

    /// A zone's fill, which is copper unless the zone is a rule area.
    fn zone(&mut self, node: &Sexp<'_>, fp: Option<&Footprint>) {
        let list = self.zone_count;
        self.zone_count += 1;
        if let Some(rules) = node.child("keepout") {
            return self.keepout(node, rules);
        }
        let net = self.net(node);
        if let Some(spec) = self.zone_spec(node, net) {
            self.zone_specs.zones.push(spec);
            self.zone_lists.push(list);
            self.zone_uuids.push(uuid(node).unwrap_or_default());
        }
        let name = &self.names[net.index()];
        let description = match (fp, name.is_empty()) {
            (Some(fp), _) => format!("zone of {}", fp.reference),
            (None, true) => "zone".to_string(),
            (None, false) => format!("zone of {name}"),
        };
        // A zone's own clearance is what it was filled with. KiCad's DRC doesn't use it:
        // between a fill and anything else, the net classes' clearance applies.
        let clearance = None;
        if node.child("filled_areas_thickness").and_then(|t| t.arg(0)) == Some("yes") {
            self.warn(
                "zone fill with outlines of their own thickness, from KiCad 5: taken as thin",
            );
        }
        // KiCad 5's fills don't name their layer, which is the zone's.
        let zone_layer = node.child("layer").and_then(|l| l.arg(0));
        for fill in node.children("filled_polygon") {
            let Some(layer) = fill.child("layer").and_then(|l| l.arg(0)).or(zone_layer) else {
                continue;
            };
            let Some(l) = self.layer(layer) else { continue };
            let Some((ring, arcs)) = points(fill) else {
                self.warn("malformed zone fill, skipped");
                continue;
            };
            if arcs > 0 {
                self.warn("zone outline with arcs: taken as straight between their points");
            }
            let ring: Vec<Point> = ring.into_iter().map(to_point).collect();
            let Ok(pieces) = Shape::region(&[&ring]) else {
                self.warn("zone fill that could not be built, skipped");
                continue;
            };
            for shape in pieces {
                self.push_zone(
                    Zone {
                        shape,
                        layer: l,
                        // A fill with no net is checked against all copper, as in KiCad.
                        net: Some(net).filter(|&n| n != NetId(0)),
                        clearance,
                    },
                    Source {
                        uuid: uuid(node),
                        description: format!("{description} on {layer}"),
                    },
                    Kind::Fixed,
                );
            }
        }
    }

    /// Makes zones' priorities distinct, as KiCad does: between two of the same priority,
    /// the one with the greater UUID comes first (`ZONE::HigherPriority`).
    fn rank_zones(&mut self) {
        let zones = &mut self.zone_specs.zones;
        let mut order: Vec<usize> = (0..zones.len()).collect();
        order.sort_by(|&a, &b| {
            (zones[a].priority, &self.zone_uuids[a]).cmp(&(zones[b].priority, &self.zone_uuids[b]))
        });
        for (rank, &z) in order.iter().enumerate() {
            zones[z].priority = rank as u32;
        }
    }

    /// A zone's outline and settings, for filling it, or `None` for a teardrop.
    fn zone_spec(&mut self, node: &Sexp<'_>, net: NetId) -> Option<crate::pcb::CopperZone> {
        use crate::pcb::{CopperZone, Islands, ZoneConnection};
        let teardrop = node
            .child("name")
            .and_then(|n| n.arg(0))
            .is_some_and(|n| n.starts_with("$teardrop"))
            || node
                .child("attr")
                .is_some_and(|a| a.child("teardrop").is_some());
        if teardrop {
            return None;
        }
        let layers = LayerSet(
            self.layer_set(node.child("layers")).0 | self.layer_set(node.child("layer")).0,
        );
        let mut rings = Vec::new();
        for polygon in node.children("polygon") {
            let Some((ring, arcs)) = points(polygon) else {
                self.warn("malformed zone outline, skipped");
                return None;
            };
            if arcs > 0 {
                self.warn("zone outline with arcs: taken as straight between their points");
            }
            rings.push(ring.into_iter().map(to_point).collect::<Vec<Point>>());
        }
        if rings.is_empty() || layers == LayerSet::EMPTY {
            return None;
        }
        let outline = rings.remove(0);
        let connect = node.child("connect_pads");
        let connection = match connect.and_then(|c| c.arg(0)) {
            Some("yes") => ZoneConnection::Solid,
            Some("no") => ZoneConnection::None,
            Some("thru_hole_only") => ZoneConnection::ThermalThroughHole,
            _ => ZoneConnection::Thermal,
        };
        let fill = node.child("fill");
        if fill
            .and_then(|f| f.child("mode"))
            .and_then(|m| m.arg(0))
            .is_some_and(|m| m == "hatch" || m == "hatched")
        {
            self.warn("hatched zone: filled solid");
        }
        let islands = match fill
            .and_then(|f| f.child("island_removal_mode"))
            .and_then(|m| m.arg(0))
        {
            Some("1") => Islands::Keep,
            Some("2") => Islands::RemoveBelow(
                fill.and_then(|f| length(f, "island_area_min"))
                    .map_or(0, |a| i128::from(a) * 1_000_000),
            ),
            _ => Islands::Remove,
        };
        Some(CopperZone {
            outline,
            holes: rings,
            layers,
            net: Some(net).filter(|&n| n != NetId(0)),
            priority: node
                .child("priority")
                .and_then(|p| p.arg(0))
                .and_then(|p| p.parse().ok())
                .unwrap_or(0),
            clearance: connect.and_then(|c| length(c, "clearance")).unwrap_or(0),
            min_width: length(node, "min_thickness").unwrap_or(0),
            connection,
            thermal_gap: fill.and_then(|f| length(f, "thermal_gap")).unwrap_or(0),
            spoke_width: fill
                .and_then(|f| length(f, "thermal_bridge_width"))
                .unwrap_or(0),
            islands,
        })
    }

    /// KiCad 5's rules, kept in the board file: net classes and a few limits.
    fn legacy_rules(&self, root: &Sexp<'_>) -> ProjectRules {
        let mut rules = project::defaults();
        if let Some(setup) = root.child("setup") {
            if let Some(v) = length(setup, "hole_to_hole_min") {
                rules.hole_to_hole = v;
            }
            if let Some(v) = length(setup, "trace_min") {
                rules.min_track_width = v;
            }
            if let Some(v) = length(setup, "edge_clearance") {
                rules.edge_clearance = v;
            }
        }
        let classes: Vec<&Sexp<'_>> = root.children("net_class").collect();
        if !classes.is_empty() {
            rules.classes.clear();
            for class in classes {
                let Some(name) = class.arg(0) else { continue };
                rules.classes.push(project::ClassDef {
                    name: name.to_string(),
                    clearance: length(class, "clearance"),
                    pair_gap: length(class, "diff_pair_gap"),
                    pair_width: length(class, "diff_pair_width"),
                    priority: None,
                    track_width: length(class, "trace_width"),
                    via_diameter: length(class, "via_dia"),
                    via_drill: length(class, "via_drill"),
                });
                for net in class.children("add_net").filter_map(|n| n.arg(0)) {
                    rules
                        .assignments
                        .push((net.to_string(), vec![name.to_string()]));
                }
            }
            if !rules.classes.iter().any(|c| c.name == "Default") {
                rules
                    .classes
                    .insert(0, project::defaults().classes.remove(0));
            }
        }
        rules
    }

    /// The rules, with each net in its class.
    fn rules(&mut self, rules: &ProjectRules) -> Result<Rules, KicadError> {
        if self.copper.is_empty() {
            return Err(KicadError::other("the board has no copper layers"));
        }
        let names: Vec<&str> = self.copper.iter().map(String::as_str).collect();
        let mut spec = RuleSpec::new(
            &names,
            rules.min_track_width.max(1),
            rules.default_clearance(),
        );
        spec.min_clearance = rules.min_clearance;
        spec.hole_to_hole = rules.hole_to_hole;
        spec.hole_clearance = rules.hole_clearance;
        let ignored = |check: &str| rules.ignored.iter().any(|c| c == check);
        if ignored("hole_clearance") {
            spec.hole_clearance = 0;
        }
        if ignored("hole_to_hole") {
            spec.hole_to_hole = 0;
        }
        if ignored("clearance") {
            self.warn("the project turns KiCad's clearance check off, but it is kept here");
        }
        let mut members: Vec<Vec<NetId>> = vec![Vec::new(); rules.classes.len()];
        let mut class_of = Vec::with_capacity(self.names.len());
        for (i, name) in self.names.iter().enumerate() {
            let class = rules.class_of(name);
            if let Some(c) = class {
                members[c].push(NetId(i as u32));
            }
            class_of.push(class);
        }
        for pattern in rules.unmatched_regexes(&self.names) {
            self.warn(format!(
                "net class pattern {pattern:?} matches no net as a wildcard, and regular expressions aren't read"
            ));
        }
        // The two nets of a differential pair may come closer, down to their class's gap,
        // if that is smaller than its clearance; a larger gap changes nothing. This is what
        // `kicad-cli` 10 does, though KiCad's documentation doesn't say.
        let clearance = |class: Option<usize>| match class {
            Some(c) => rules.classes[c].clearance.unwrap_or(spec.clearance),
            None => spec.clearance,
        };
        for (i, name) in self.names.iter().enumerate() {
            let Some(&j) = project::pair_partner(name).and_then(|p| self.ids.get(&p)) else {
                continue;
            };
            let j = j.index();
            if i < j {
                let usual = clearance(class_of[i]).max(clearance(class_of[j]));
                let gaps = [rules.pair_gap(class_of[i]), rules.pair_gap(class_of[j])];
                if let Some(gap) = gaps.into_iter().flatten().min()
                    && gap < usual
                {
                    spec.pairs.push((NetId(i as u32), NetId(j as u32), gap));
                }
            }
        }
        for (class, nets) in rules.classes.iter().zip(members) {
            if nets.is_empty() || class.name == "Default" {
                continue;
            }
            spec.classes.push(NetClass {
                name: class.name.clone(),
                nets,
                overrides: Overrides {
                    clearance: class.clearance,
                    ..Overrides::default()
                },
                ..NetClass::default()
            });
        }
        Rules::new(spec).map_err(|e| KicadError::other(format!("invalid rules: {e}")))
    }

    /// Gives copper the nets KiCad's connectivity would, as [`read`] describes, after
    /// `propagateConnections` in KiCad's `connectivity_algo.cpp`. A cluster with no pad
    /// of a net takes the net of its first item with one, in item order.
    fn propagate(&mut self) {
        let b = &self.board;
        // Every item with its shape, layers and kind, in item order.
        let mut items: Vec<(Shape, LayerSet, Kind)> = Vec::new();
        let tolerance = b.arc_tolerance;
        for (t, &kind) in b.tracks.iter().zip(&self.track_kinds) {
            let shape = match t.mid {
                None => Shape::track(t.start, t.end, t.width),
                Some(mid) => Shape::arc_track(t.start, mid, t.end, t.width, tolerance),
            };
            let Ok(shape) = shape else { return };
            items.push((shape, LayerSet::EMPTY.with(t.layer), kind));
        }
        for v in &b.vias {
            let Ok(shape) = Shape::circle(v.at, v.diameter) else {
                return;
            };
            items.push((shape, LayerSet::span(v.from, v.to), Kind::Changeable));
        }
        for p in &b.pads {
            items.push((p.shape.clone(), p.layers, Kind::Fixed));
        }
        for (z, &kind) in b.zones.iter().zip(&self.zone_kinds) {
            items.push((z.shape.clone(), LayerSet::EMPTY.with(z.layer), kind));
        }
        let nets: Vec<NetId> = b
            .tracks
            .iter()
            .map(|t| t.net)
            .chain(b.vias.iter().map(|v| v.net))
            .chain(b.pads.iter().map(|p| p.net))
            .chain(b.zones.iter().map(|z| z.net))
            .map(|n| n.unwrap_or(NetId(0)))
            .collect();
        let (n_tracks, n_vias, n_pads) = (b.tracks.len(), b.vias.len(), b.pads.len());

        // Union-find over touching items.
        let mut parent: Vec<usize> = (0..items.len()).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let index = ShapeIndex::bulk_load(items.iter().map(|(s, _, _)| s.clone()));
        let mut near = Vec::new();
        for (i, (shape, layers, _)) in items.iter().enumerate() {
            index.query(&shape.bbox(), &mut near);
            for id in &near {
                let j = id.slot() as usize;
                if j <= i || items[j].1.0 & layers.0 == 0 {
                    continue;
                }
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b && shape.touches(&items[j].0) {
                    parent[a.max(b)] = a.min(b);
                }
            }
        }
        // By cluster, the net of its pads, or a conflict, and its first net.
        #[derive(Clone, Copy)]
        enum PadNet {
            None,
            One(NetId),
            Conflict,
        }
        let mut pad_net: BTreeMap<usize, PadNet> = BTreeMap::new();
        let mut first_net: BTreeMap<usize, NetId> = BTreeMap::new();
        for (i, &net) in nets.iter().enumerate() {
            let root = find(&mut parent, i);
            if net == NetId(0) {
                continue;
            }
            first_net.entry(root).or_insert(net);
            let is_pad = (n_tracks + n_vias..n_tracks + n_vias + n_pads).contains(&i);
            if is_pad {
                let entry = pad_net.entry(root).or_insert(PadNet::None);
                *entry = match *entry {
                    PadNet::None => PadNet::One(net),
                    PadNet::One(n) if n == net => PadNet::One(n),
                    _ => PadNet::Conflict,
                };
            }
        }
        let mut changed = 0;
        for i in 0..items.len() {
            if items[i].2 != Kind::Changeable {
                continue;
            }
            let root = find(&mut parent, i);
            let origin = match pad_net.get(&root).copied().unwrap_or(PadNet::None) {
                PadNet::Conflict => continue,
                PadNet::One(n) => n,
                PadNet::None => match first_net.get(&root) {
                    Some(&n) => n,
                    None => continue,
                },
            };
            if nets[i] == origin {
                continue;
            }
            changed += 1;
            let net = Some(origin);
            if i < n_tracks {
                self.board.tracks[i].net = net;
            } else if i < n_tracks + n_vias {
                self.board.vias[i - n_tracks].net = net;
            } else if i >= n_tracks + n_vias + n_pads {
                self.board.zones[i - n_tracks - n_vias - n_pads].net = net;
            }
        }
        if changed > 0 {
            let message =
                "track, via or graphic given the net of the copper it touches, as KiCad does";
            *self.warnings.entry(message.into()).or_insert(0) += changed;
        }
    }
}

/// The length of a vector, rounded to the nearest integer, halves up, exactly.
fn round_hypot(dx: i64, dy: i64) -> i64 {
    let sq = i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy);
    // The integer square root, from a float guess corrected both ways.
    let mut r = (sq as f64).sqrt() as i128;
    while r * r > sq {
        r -= 1;
    }
    while (r + 1) * (r + 1) <= sq {
        r += 1;
    }
    // Up if the length is at least r + 1/2, that is, if 4 sq >= (2r + 1)^2.
    (r + i128::from(4 * sq >= (2 * r + 1) * (2 * r + 1))) as i64
}

/// The start, middle and end of an arc graphic. KiCad 6 and later save those. KiCad 5
/// saved the centre as `start`, the start as `end`, and the angle it turns through.
fn arc_points(node: &Sexp<'_>) -> Option<(Vec2, Vec2, Vec2)> {
    if let Some(mid) = pair(node, "mid") {
        return Some((pair(node, "start")?, mid, pair(node, "end")?));
    }
    let center = pair(node, "start")?;
    let start = pair(node, "end")?;
    let angle = node
        .child("angle")
        .and_then(|a| a.arg(0))
        .and_then(degrees_to_millidegrees)?;
    let v = (start.0 - center.0, start.1 - center.1);
    // KiCad 5's angles turn clockwise on screen, which is the opposite of RotatePoint's.
    let mid = add(center, rotate(v, -angle / 2));
    let end = add(center, rotate(v, -angle));
    Some((start, mid, end))
}

/// Where a pad's shape is: its origin, offset included, and its angle.
struct PadGeometry {
    origin: Vec2,
    angle: i32,
}

impl PadGeometry {
    /// A point of the pad's shape, from its own frame to the board's.
    fn point(&self, local: Vec2) -> Point {
        to_point(add(self.origin, rotate(local, self.angle)))
    }

    fn polygon(&self, corners: &[Vec2], radius: i64) -> Option<Shape> {
        let ring: Vec<Point> = corners.iter().map(|&c| self.point(c)).collect();
        Shape::rounded_polygon(&ring, &[], radius).ok()
    }

    /// An oval: a stadium along its longer side, or a circle. KiCad halves the sizes
    /// first, which can drop a nanometre from odd ones.
    fn stadium(&self, w: i64, h: i64) -> Option<Shape> {
        if w == h {
            return Shape::circle(self.point((0, 0)), 2 * (w / 2)).ok();
        }
        let (hw, hh) = (w / 2, h / 2);
        let r = hw.min(hh);
        let (a, b) = ((-(hw - r), -(hh - r)), (hw - r, hh - r));
        Shape::track(self.point(a), self.point(b), 2 * r).ok()
    }
}
