//! Reading a `.dsn` design into a [`drc::Board`].

use std::collections::BTreeMap;

use super::{
    Component, DsnBoard, DsnError, PadSource, Plane, Resolution, Side, Unit, ViaPadstack,
    WireSource, WireType,
};
use crate::geom::drc::{self, Keepout, Pad, Track, Via};
use crate::geom::{
    LayerId, LayerSet, MAX_COORD, MAX_LAYERS, NetClass, Overrides, Point, RuleSpec, Rules, Shape,
    ViaDef, ViaId,
};
use crate::id::NetId;
use crate::sexpr::{self, Sexp};
use crate::units::{normalize, rotate_y_up, scaled};

/// Reads a design.
///
/// Items it can't make sense of are skipped with a [warning](DsnBoard::warnings).
///
/// # Errors
///
/// [`DsnError`] only if the text is not an S-expression, is not a design (`(pcb ...)`), has
/// no layers, or holds rules that [`Rules`] rejects.
///
/// ```
/// use guide::geom::drc::{self, Item};
///
/// let dsn = r#"(pcb "demo.dsn"
///   (parser (string_quote ") (space_in_quoted_tokens on))
///   (resolution um 10)
///   (unit um)
///   (structure
///     (layer F.Cu (type signal)) (layer B.Cu (type signal))
///     (rule (width 250) (clearance 200)))
///   (placement
///     (component R_0603 (place R1 1000 0 front 90)))
///   (library
///     (image R_0603
///       (pin Rect[T]Pad_800x950_um 1 -800 0)
///       (pin Rect[T]Pad_800x950_um 2 800 0))
///     (padstack Rect[T]Pad_800x950_um (shape (rect F.Cu -400 -475 400 475))))
///   (network
///     (net "A" (pins R1-1))
///     (net "B" (pins R1-2)))
///   (wiring
///     (wire (path F.Cu 250 1000 -800 5000 -800) (net "A") (type route))
///     (wire (path F.Cu 250 1350 1000 5000 1000) (net "B") (type protect))))"#;
/// let read = guide::dsn::read(dsn)?;
/// // Turned a quarter, R1's pins are 800 µm above and below its origin.
/// assert_eq!(read.board.pads.len(), 2);
/// assert_eq!(read.describe(Item::Pad(1)).as_deref(), Some("pin 2 of R1"));
/// assert!(read.tracks[1].kind.is_fixed());
/// // Each wire starts on its own pin, and keeps clear of the other.
/// let found = drc::check(&read.board, &read.rules)?;
/// assert!(found.is_empty(), "{found:?}");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn read(text: &str) -> Result<DsnBoard, DsnError> {
    let root =
        sexpr::parse_specctra(text).map_err(|e| DsnError::syntax(text, e.offset, e.message))?;
    if !root.is("pcb") {
        return Err(DsnError::other(
            "not a Specctra design: it doesn't start with pcb",
        ));
    }
    let mut r = Reader::new(&root);
    let structure = root.child("structure");
    if let Some(s) = structure {
        r.layers(s);
    }
    if r.layers.is_empty() {
        return Err(DsnError::other("the design has no layers"));
    }
    let structure_units = r.units_in(structure, r.units);
    if let Some(s) = structure {
        r.structure(s, structure_units);
    }
    if let Some(network) = root.child("network") {
        r.network(network);
    }
    if let Some(library) = root.child("library") {
        let units = r.units_in(Some(library), r.units);
        r.library(library, units);
    }
    if let Some(placement) = root.child("placement") {
        let units = r.units_in(Some(placement), r.units);
        r.placement(placement, units);
    }
    if let Some(wiring) = root.child("wiring") {
        let units = r.units_in(Some(wiring), r.units);
        r.wiring(wiring, units);
    }
    let rules = r.rules(structure, structure_units)?;
    Ok(DsnBoard {
        board: r.board,
        rules,
        nets: r.names,
        name: root.arg(0).unwrap_or("").to_string(),
        resolution: r.resolution,
        boundary: r.boundary,
        planes: r.planes,
        components: r.components,
        via_padstacks: r.via_padstacks,
        pads: r.pads,
        tracks: r.tracks,
        vias: r.vias,
        keepouts: r.keepouts,
        warnings: r.warnings.into_iter().collect(),
    })
}

/// A shape of a padstack or keepout, in its own frame, in nanometres with Y up.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Prim {
    Circle { diameter: i64, at: Point },
    Rect { a: Point, b: Point },
    Polygon { aperture: i64, points: Vec<Point> },
    Path { aperture: i64, points: Vec<Point> },
}

/// Where an image is put: turned and maybe mirrored, then moved.
#[derive(Clone, Copy, Debug)]
struct Place {
    at: Point,
    /// Millidegrees, counter-clockwise, applied after mirroring.
    angle: i32,
    mirror: bool,
}

impl Place {
    const IDENTITY: Place = Place {
        at: Point::new(0, 0),
        angle: 0,
        mirror: false,
    };

    fn apply(&self, p: Point) -> Point {
        let x = if self.mirror { -p.x } else { p.x };
        let (x, y) = rotate_y_up((x, p.y), self.angle);
        Point::new(self.at.x.saturating_add(x), self.at.y.saturating_add(y))
    }

    /// This, after `inner`.
    fn then(inner: Place, outer: Place) -> impl Fn(Point) -> Point {
        move |p| outer.apply(inner.apply(p))
    }
}

/// An image, as the placement needs it.
#[derive(Default)]
struct Image {
    pins: Vec<Pin>,
    keepouts: Vec<KeepoutDef>,
}

struct Pin {
    name: String,
    padstack: String,
    at: Point,
    angle: i32,
}

struct KeepoutDef {
    prim: Prim,
    layers: LayerSet,
    tracks: bool,
    vias: bool,
}

/// A padstack: its shapes, each with the layers it is on.
struct Padstack {
    shapes: Vec<(Prim, LayerSet)>,
}

struct Reader {
    units: Unit,
    resolution: Resolution,
    layers: Vec<String>,
    ids: BTreeMap<String, NetId>,
    names: Vec<String>,
    /// Each pin, `REF-PIN`, with its net.
    pin_nets: BTreeMap<String, NetId>,
    images: BTreeMap<String, Image>,
    padstacks: BTreeMap<String, Padstack>,
    board: drc::Board,
    boundary: Vec<Vec<Point>>,
    planes: Vec<Plane>,
    components: Vec<Component>,
    via_padstacks: Vec<ViaPadstack>,
    /// The padstacks of the structure's vias.
    allowed_vias: Vec<String>,
    pads: Vec<PadSource>,
    tracks: Vec<WireSource>,
    vias: Vec<WireSource>,
    keepouts: Vec<Option<usize>>,
    /// Classes: name, nets, overrides, layer overrides, vias by padstack name.
    classes: Vec<ClassDef>,
    warnings: BTreeMap<String, usize>,
}

struct ClassDef {
    name: String,
    nets: Vec<NetId>,
    overrides: Overrides,
    layers: Vec<(LayerId, Overrides)>,
    vias: Vec<String>,
}

/// The atoms among the items of a list, after its name.
fn atoms<'s>(node: &'s Sexp<'_>) -> impl Iterator<Item = &'s str> {
    node.args().iter().filter_map(Sexp::as_atom)
}

impl Reader {
    fn new(root: &Sexp<'_>) -> Reader {
        let resolution = root
            .child("resolution")
            .and_then(|r| {
                Some(Resolution {
                    unit: Unit::from_name(r.arg(0)?)?,
                    steps: r.arg(1)?.parse().ok().filter(|&s| s > 0)?,
                })
            })
            .unwrap_or(Resolution {
                unit: Unit::Inch,
                steps: 1000,
            });
        let units = root
            .child("unit")
            .and_then(|u| u.arg(0))
            .and_then(Unit::from_name)
            .unwrap_or(resolution.unit);
        let mut r = Reader {
            units,
            resolution,
            layers: Vec::new(),
            ids: BTreeMap::new(),
            names: Vec::new(),
            pin_nets: BTreeMap::new(),
            images: BTreeMap::new(),
            padstacks: BTreeMap::new(),
            board: drc::Board::default(),
            boundary: Vec::new(),
            planes: Vec::new(),
            components: Vec::new(),
            via_padstacks: Vec::new(),
            allowed_vias: Vec::new(),
            pads: Vec::new(),
            tracks: Vec::new(),
            vias: Vec::new(),
            keepouts: Vec::new(),
            classes: Vec::new(),
            warnings: BTreeMap::new(),
        };
        // Wires with no net are of the net with no name, never checked against itself.
        r.net_id("");
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

    /// The units of a section: its own `(unit ...)` or `(resolution ...)`, or `outer`.
    fn units_in(&self, node: Option<&Sexp<'_>>, outer: Unit) -> Unit {
        let Some(node) = node else { return outer };
        node.child("unit")
            .or_else(|| node.child("resolution"))
            .and_then(|u| u.arg(0))
            .and_then(Unit::from_name)
            .unwrap_or(outer)
    }

    fn length(&self, text: &str, units: Unit) -> Option<i64> {
        let (factor, scale) = units.factor();
        scaled(text, factor, scale, MAX_COORD)
    }

    fn point(&self, x: &str, y: &str, units: Unit) -> Option<Point> {
        Some(Point::new(self.length(x, units)?, self.length(y, units)?))
    }

    /// Points from consecutive pairs of atoms.
    fn points(&self, texts: &[&str], units: Unit) -> Option<Vec<Point>> {
        if !texts.len().is_multiple_of(2) {
            return None;
        }
        texts
            .chunks(2)
            .map(|c| self.point(c[0], c[1], units))
            .collect()
    }

    fn layer(&self, name: &str) -> Option<LayerId> {
        let i = self.layers.iter().position(|l| l == name)?;
        Some(LayerId(i as u8))
    }

    /// The layers a shape names: one, or all of them for `signal`.
    fn layer_set(&self, name: &str) -> Option<LayerSet> {
        match name {
            "signal" | "power" | "*" => Some(LayerSet::first(self.layers.len())),
            name => self.layer(name).map(|l| LayerSet::EMPTY.with(l)),
        }
    }

    fn layers(&mut self, structure: &Sexp<'_>) {
        for layer in structure.children("layer") {
            let Some(name) = layer.arg(0) else { continue };
            if self.layers.len() == MAX_LAYERS {
                self.warn(format!("more than {MAX_LAYERS} layers: the rest skipped"));
                break;
            }
            if self.layers.iter().any(|l| l == name) {
                self.warn("layer declared twice, skipped");
                continue;
            }
            self.layers.push(name.to_string());
        }
    }

    /// A shape, as `(circle layer d [x y])`, `(rect layer x1 y1 x2 y2)`,
    /// `(polygon layer w x y ...)` or `(path layer w x y ...)`, with the layers it is on.
    /// `None`, with a warning, for anything else.
    fn prim(&mut self, node: &Sexp<'_>, units: Unit, what: &str) -> Option<(Prim, LayerSet)> {
        let kind = node.name().unwrap_or("");
        let args: Vec<&str> = atoms(node).collect();
        let Some((&layer, rest)) = args.split_first() else {
            self.warn(format!("malformed {kind} in a {what}, skipped"));
            return None;
        };
        let Some(layers) = self.layer_set(layer) else {
            // A shape on a layer that isn't copper, such as an outline: nothing to check.
            return None;
        };
        let prim = match kind {
            "circle" => match rest {
                [d] => self.length(d, units).map(|diameter| Prim::Circle {
                    diameter,
                    at: Point::new(0, 0),
                }),
                [d, x, y] => match (self.length(d, units), self.point(x, y, units)) {
                    (Some(diameter), Some(at)) => Some(Prim::Circle { diameter, at }),
                    _ => None,
                },
                _ => None,
            },
            "rect" => match self.points(rest, units).as_deref() {
                Some(&[a, b]) => Some(Prim::Rect { a, b }),
                _ => None,
            },
            "polygon" | "path" => match rest.split_first() {
                Some((w, coords)) => match (self.length(w, units), self.points(coords, units)) {
                    (Some(aperture), Some(points)) if !points.is_empty() => {
                        if kind == "polygon" {
                            Some(Prim::Polygon { aperture, points })
                        } else {
                            Some(Prim::Path { aperture, points })
                        }
                    }
                    _ => None,
                },
                None => None,
            },
            "qarc" => {
                self.warn(format!("arc in a {what}, skipped"));
                return None;
            }
            _ => {
                self.warn(format!("shape {kind:?} in a {what}, skipped"));
                return None;
            }
        };
        if prim.is_none() {
            self.warn(format!("malformed {kind} in a {what}, skipped"));
        }
        Some((prim?, layers))
    }

    /// The shapes of a primitive once placed: a polygon, a circle, or tracks for a path.
    fn shapes(&mut self, prim: &Prim, to: &dyn Fn(Point) -> Point, what: &str) -> Vec<Shape> {
        let built: Option<Vec<Shape>> = match prim {
            Prim::Circle { diameter, at } => {
                Shape::circle(to(*at), *diameter).ok().map(|s| vec![s])
            }
            Prim::Rect { a, b } => {
                let corners = [*a, Point::new(b.x, a.y), *b, Point::new(a.x, b.y)];
                let ring: Vec<Point> = corners.iter().map(|&p| to(p)).collect();
                Shape::polygon(&ring, &[]).ok().map(|s| vec![s])
            }
            Prim::Polygon { aperture, points } => {
                let mut ring: Vec<Point> = points.iter().map(|&p| to(p)).collect();
                ring.dedup();
                if ring.len() > 1 && ring.first() == ring.last() {
                    ring.pop();
                }
                let radius = aperture / 2;
                match Shape::rounded_polygon(&ring, &[], radius) {
                    Ok(s) => Some(vec![s]),
                    // Not simple: the area it encloses, and its outline if it has a width.
                    Err(_) => Shape::region(&[&ring]).ok().map(|mut pieces| {
                        if *aperture > 0 {
                            for (i, &a) in ring.iter().enumerate() {
                                let b = ring[(i + 1) % ring.len()];
                                pieces.extend(Shape::track(a, b, *aperture));
                            }
                        }
                        pieces
                    }),
                }
            }
            Prim::Path { aperture, points } => {
                let pts: Vec<Point> = points.iter().map(|&p| to(p)).collect();
                if pts.len() == 1 {
                    Shape::track(pts[0], pts[0], *aperture)
                        .ok()
                        .map(|s| vec![s])
                } else {
                    pts.windows(2)
                        .map(|w| Shape::track(w[0], w[1], *aperture).ok())
                        .collect()
                }
            }
        };
        match built {
            Some(shapes) if !shapes.is_empty() => shapes,
            _ => {
                self.warn(format!("{what} shape that could not be built, skipped"));
                Vec::new()
            }
        }
    }

    fn structure(&mut self, s: &Sexp<'_>, units: Unit) {
        for item in s.args() {
            match item.name() {
                Some("boundary") => {
                    for shape in item.args() {
                        let args: Vec<&str> = atoms(shape).collect();
                        let ring = match (shape.name(), args.split_first()) {
                            (Some("path" | "polygon"), Some((&"pcb", rest)))
                                if !rest.is_empty() =>
                            {
                                self.points(&rest[1..], units)
                            }
                            (Some("rect"), Some((&"pcb", rest))) => {
                                match self.points(rest, units).as_deref() {
                                    Some(&[a, b]) => {
                                        Some(vec![a, Point::new(b.x, a.y), b, Point::new(a.x, b.y)])
                                    }
                                    _ => None,
                                }
                            }
                            // A signal boundary, where routing may go, is not the board's.
                            _ => continue,
                        };
                        match ring {
                            Some(ring) => self.boundary.push(ring),
                            None => self.warn("malformed boundary, skipped"),
                        }
                    }
                }
                Some("plane") => self.plane(item, units),
                Some(kind @ ("keepout" | "via_keepout" | "wire_keepout")) => {
                    for def in self.keepout_defs(item, kind, units) {
                        self.push_keepout(&def, &|p| p, None);
                    }
                }
                Some("via") => {
                    // Padstacks the library has yet to define: resolved in `library`.
                    self.allowed_vias.extend(atoms(item).map(str::to_string));
                }
                _ => {}
            }
        }
    }

    fn plane(&mut self, item: &Sexp<'_>, units: Unit) {
        let net = item.arg(0).map(|n| self.net_id(n));
        for shape in item.args().iter().filter(|a| a.as_atom().is_none()) {
            let Some((prim, layers)) = self.prim(shape, units, "plane") else {
                continue;
            };
            let outline = match prim {
                Prim::Polygon { points, .. } | Prim::Path { points, .. } => points,
                Prim::Rect { a, b } => vec![a, Point::new(b.x, a.y), b, Point::new(a.x, b.y)],
                Prim::Circle { .. } => {
                    self.warn("round plane, skipped");
                    continue;
                }
            };
            for layer in layers.iter() {
                self.planes.push(Plane {
                    net,
                    layer,
                    outline: outline.clone(),
                });
            }
        }
    }

    /// The shapes of a keepout, `(keepout ["id"] shape ...)`.
    fn keepout_defs(&mut self, item: &Sexp<'_>, kind: &str, units: Unit) -> Vec<KeepoutDef> {
        let mut out = Vec::new();
        for shape in item.args() {
            if !matches!(
                shape.name(),
                Some("circle" | "rect" | "polygon" | "path" | "qarc")
            ) {
                continue;
            }
            if let Some((prim, layers)) = self.prim(shape, units, "keepout") {
                out.push(KeepoutDef {
                    prim,
                    layers,
                    tracks: kind != "via_keepout",
                    vias: kind != "wire_keepout",
                });
            }
        }
        out
    }

    fn push_keepout(
        &mut self,
        def: &KeepoutDef,
        to: &dyn Fn(Point) -> Point,
        owner: Option<usize>,
    ) {
        for shape in self.shapes(&def.prim, to, "keepout") {
            self.board.keepouts.push(Keepout {
                shape,
                layers: def.layers,
                tracks: def.tracks,
                vias: def.vias,
                // Pads are where they are: a keepout tells a router where not to go.
                pads: false,
                except: Vec::new(),
            });
            self.keepouts.push(owner);
        }
    }

    fn network(&mut self, network: &Sexp<'_>) {
        for net in network.children("net") {
            let Some(name) = net.arg(0) else { continue };
            let id = self.net_id(name);
            for pins in net.children("pins") {
                for pin in atoms(pins) {
                    if self.pin_nets.insert(pin.to_string(), id).is_some() {
                        self.warn("pin in two nets: the last one kept");
                    }
                }
            }
        }
        for class in network.children("class") {
            let Some(name) = class.arg(0) else { continue };
            let nets: Vec<NetId> = class.args()[1..]
                .iter()
                .filter_map(Sexp::as_atom)
                .map(|n| self.net_id(n))
                .collect();
            let units = self.units;
            let overrides = class
                .child("rule")
                .map_or_else(Overrides::default, |r| self.overrides(r, units));
            let mut layers = Vec::new();
            for lr in class.children("layer_rule") {
                let o = lr
                    .child("rule")
                    .map_or_else(Overrides::default, |r| self.overrides(r, units));
                for name in atoms(lr) {
                    if let Some(l) = self.layer(name) {
                        layers.push((l, o));
                    }
                }
            }
            let vias = class
                .child("circuit")
                .and_then(|c| c.child("use_via"))
                .map(|u| atoms(u).map(str::to_string).collect())
                .unwrap_or_default();
            self.classes.push(ClassDef {
                name: name.to_string(),
                nets,
                overrides,
                layers,
                vias,
            });
        }
    }

    /// A rule's width and clearance. Clearances of a type, such as `smd_smd`, are counted
    /// in the warnings.
    fn overrides(&mut self, rule: &Sexp<'_>, units: Unit) -> Overrides {
        let mut o = Overrides::default();
        if let Some(w) = rule.child("width").and_then(|w| w.arg(0)) {
            o.track_width = self.length(w, units);
        }
        for c in rule.children("clearance") {
            let Some(v) = c.arg(0).and_then(|v| self.length(v, units)) else {
                continue;
            };
            match c.child("type").and_then(|t| t.arg(0)) {
                None => o.clearance = Some(v),
                Some(kind) => self.warn(format!("clearance for objects of type {kind}, not used")),
            }
        }
        o
    }

    fn library(&mut self, library: &Sexp<'_>, units: Unit) {
        for ps in library.children("padstack") {
            let Some(name) = ps.arg(0) else { continue };
            let units = self.units_in(Some(ps), units);
            let mut shapes: Vec<(Prim, LayerSet)> = Vec::new();
            for shape in ps.children("shape") {
                for prim in shape.args() {
                    let Some((prim, layers)) = self.prim(prim, units, "padstack") else {
                        continue;
                    };
                    // The same shape on several layers is one pad on all of them.
                    match shapes.iter_mut().find(|(p, _)| *p == prim) {
                        Some((_, set)) => *set = LayerSet(set.0 | layers.0),
                        None => shapes.push((prim, layers)),
                    }
                }
            }
            self.padstacks.insert(name.to_string(), Padstack { shapes });
        }
        for image in library.children("image") {
            let Some(name) = image.arg(0) else { continue };
            let units = self.units_in(Some(image), units);
            let mut def = Image::default();
            for item in image.args() {
                match item.name() {
                    Some("pin") => {
                        let args: Vec<&str> = atoms(item).collect();
                        let angle = item
                            .child("rotate")
                            .and_then(|r| r.arg(0))
                            .and_then(degrees);
                        let pin = match args[..] {
                            [padstack, name, x, y] => self.point(x, y, units).map(|at| Pin {
                                name: name.to_string(),
                                padstack: padstack.to_string(),
                                at,
                                angle: angle.unwrap_or(0),
                            }),
                            _ => None,
                        };
                        match pin {
                            Some(pin) => def.pins.push(pin),
                            None => self.warn("malformed pin, skipped"),
                        }
                    }
                    Some(kind @ ("keepout" | "via_keepout" | "wire_keepout")) => {
                        let defs = self.keepout_defs(item, kind, units);
                        def.keepouts.extend(defs);
                    }
                    _ => {}
                }
            }
            if self.images.insert(name.to_string(), def).is_some() {
                self.warn("image defined twice: the last one kept");
            }
        }
        // The structure's vias, now that their padstacks are known.
        for name in self.allowed_vias.clone() {
            if self.via_padstacks.iter().any(|p| p.name == name) {
                continue;
            }
            match self.via_padstack(&name) {
                Some(ps) => self.via_padstacks.push(ps),
                None => self.warn("via whose padstack is missing or isn't round, skipped"),
            }
        }
    }

    /// A via padstack from the library: round shapes, on a span of layers.
    fn via_padstack(&mut self, name: &str) -> Option<ViaPadstack> {
        let ps = self.padstacks.get(name)?;
        let mut diameter = 0;
        let mut layers = LayerSet::EMPTY;
        for (prim, set) in &ps.shapes {
            match prim {
                Prim::Circle { diameter: d, .. } => diameter = diameter.max(*d),
                _ => return None,
            }
            layers = LayerSet(layers.0 | set.0);
        }
        let from = layers.iter().next()?;
        let to = layers.iter().last()?;
        let drill = kicad_via_drill(name);
        if drill.is_none() {
            self.warn("via padstack with no drill in its name: drill taken as half its diameter");
        }
        Some(ViaPadstack {
            name: name.to_string(),
            diameter,
            drill,
            from,
            to,
        })
    }

    fn placement(&mut self, placement: &Sexp<'_>, units: Unit) {
        let n = self.layers.len();
        for component in placement.children("component") {
            let Some(image_name) = component.arg(0) else {
                continue;
            };
            let units = self.units_in(Some(component), units);
            for place in component.children("place") {
                let args: Vec<&str> = atoms(place).collect();
                let parsed = match args[..] {
                    [reference, x, y, side, rotation, ..] => {
                        let side = match side {
                            "front" => Some(Side::Front),
                            "back" => Some(Side::Back),
                            _ => None,
                        };
                        match (self.point(x, y, units), side, degrees(rotation)) {
                            (Some(at), Some(side), Some(rotation)) => Some(Component {
                                reference: reference.to_string(),
                                image: image_name.to_string(),
                                at,
                                side,
                                rotation: normalize(rotation),
                            }),
                            _ => None,
                        }
                    }
                    [_] => {
                        self.warn("component that isn't placed, skipped");
                        continue;
                    }
                    _ => None,
                };
                let Some(c) = parsed else {
                    self.warn("malformed placement, skipped");
                    continue;
                };
                let index = self.components.len();
                self.components.push(c);
                self.place(index, n);
            }
        }
    }

    /// Puts a component's pads and keepouts on the board.
    fn place(&mut self, index: usize, layer_count: usize) {
        let c = &self.components[index];
        let back = c.side == Side::Back;
        // On the back, the image is mirrored through its Y axis, then turned.
        let place = Place {
            at: c.at,
            angle: c.rotation,
            mirror: back,
        };
        let flip = |set: LayerSet| -> LayerSet {
            if !back {
                return set;
            }
            set.iter().fold(LayerSet::EMPTY, |acc, l| {
                acc.with(LayerId((layer_count - 1 - usize::from(l.0)) as u8))
            })
        };
        let reference = c.reference.clone();
        let Some(image) = self.images.remove(&c.image) else {
            self.warn("component whose image is missing, skipped");
            return;
        };
        for pin in &image.pins {
            let net = self
                .pin_nets
                .get(&format!("{reference}-{}", pin.name))
                .copied();
            let Some(padstack) = self.padstacks.get(&pin.padstack) else {
                self.warn("pin whose padstack is missing, skipped");
                continue;
            };
            let shapes = padstack.shapes.clone();
            if shapes.is_empty() {
                self.warn("pin whose padstack has no copper, skipped");
                continue;
            }
            let local = Place {
                at: pin.at,
                angle: pin.angle,
                mirror: false,
            };
            let to = Place::then(local, place);
            for (prim, layers) in &shapes {
                for shape in self.shapes(prim, &to, "pin") {
                    self.board.pads.push(Pad {
                        shape,
                        layers: flip(*layers),
                        hole: None,
                        net,
                        clearance: None,
                        stack: Vec::new(),
                    });
                    self.pads.push(PadSource {
                        component: index,
                        pin: pin.name.clone(),
                        padstack: pin.padstack.clone(),
                    });
                }
            }
        }
        for def in &image.keepouts {
            let flipped = KeepoutDef {
                prim: def.prim.clone(),
                layers: flip(def.layers),
                tracks: def.tracks,
                vias: def.vias,
            };
            let to = Place::then(Place::IDENTITY, place);
            self.push_keepout(&flipped, &to, Some(index));
        }
        let name = self.components[index].image.clone();
        self.images.insert(name, image);
    }

    fn wiring(&mut self, wiring: &Sexp<'_>, units: Unit) {
        for (n, item) in wiring.args().iter().enumerate() {
            let kind = match item.child("type").and_then(|t| t.arg(0)) {
                None | Some("normal") => WireType::Normal,
                Some("route") => WireType::Route,
                Some("protect") => WireType::Protect,
                Some("fix") => WireType::Fix,
                Some(other) => {
                    self.warn(format!("wire of type {other:?}, taken as normal"));
                    WireType::Normal
                }
            };
            let net = item
                .child("net")
                .and_then(|n| n.arg(0))
                .map_or(NetId(0), |name| self.net_id(name));
            match item.name() {
                Some("wire") => self.wire(item, n, kind, net, units),
                Some("via") => self.via(item, n, kind, net, units),
                _ => {}
            }
        }
    }

    fn wire(&mut self, item: &Sexp<'_>, n: usize, kind: WireType, net: NetId, units: Unit) {
        let Some(shape) = item
            .args()
            .iter()
            .find(|a| a.as_atom().is_none() && !matches!(a.name(), Some("net" | "type")))
        else {
            return self.warn("wire with no shape, skipped");
        };
        let args: Vec<&str> = atoms(shape).collect();
        let parsed = args.split_first().and_then(|(layer, rest)| {
            let layer = self.layer(layer)?;
            let (w, coords) = rest.split_first()?;
            Some((layer, self.length(w, units)?, self.points(coords, units)?))
        });
        let Some((layer, width, points)) = parsed else {
            return self.warn("malformed wire, or one on a layer that isn't copper, skipped");
        };
        if width <= 0 {
            return self.warn("wire of zero width, skipped");
        }
        let source = WireSource {
            wire: n,
            kind,
            padstack: None,
        };
        match shape.name() {
            Some("path") => {
                let segments: Vec<(Point, Point)> = match points.len() {
                    0 => Vec::new(),
                    1 => vec![(points[0], points[0])],
                    _ => points.windows(2).map(|w| (w[0], w[1])).collect(),
                };
                for (start, end) in segments {
                    self.push_track(start, end, None, width, layer, net, &source);
                }
            }
            Some("qarc") => match points[..] {
                [start, end, center] => {
                    let mid = arc_mid(start, end, center);
                    self.push_track(start, end, Some(mid), width, layer, net, &source);
                }
                _ => self.warn("malformed arc wire, skipped"),
            },
            Some(other) => self.warn(format!("wire of shape {other:?}, skipped")),
            None => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_track(
        &mut self,
        start: Point,
        end: Point,
        mid: Option<Point>,
        width: i64,
        layer: LayerId,
        net: NetId,
        source: &WireSource,
    ) {
        self.board.tracks.push(Track {
            start,
            end,
            mid,
            width,
            layer,
            net: Some(net),
        });
        self.tracks.push(source.clone());
    }

    fn via(&mut self, item: &Sexp<'_>, n: usize, kind: WireType, net: NetId, units: Unit) {
        let args: Vec<&str> = atoms(item).collect();
        let Some((name, coords)) = args.split_first() else {
            return self.warn("malformed via, skipped");
        };
        let index = match self.via_padstacks.iter().position(|p| p.name == *name) {
            Some(i) => i,
            None => match self.via_padstack(name) {
                Some(ps) => {
                    self.via_padstacks.push(ps);
                    self.via_padstacks.len() - 1
                }
                None => return self.warn("via whose padstack is missing or isn't round, skipped"),
            },
        };
        let Some(points) = self.points(coords, units).filter(|p| !p.is_empty()) else {
            return self.warn("malformed via, skipped");
        };
        let ps = &self.via_padstacks[index];
        let (diameter, from, to) = (ps.diameter, ps.from, ps.to);
        let drill = ps.drill.unwrap_or(diameter / 2).max(1);
        for at in points {
            self.board.vias.push(Via {
                at,
                drill,
                diameter,
                from,
                to,
                net: Some(net),
            });
            self.vias.push(WireSource {
                wire: n,
                kind,
                padstack: Some(index),
            });
        }
    }

    /// The rules, from the structure's and the classes'.
    fn rules(&mut self, structure: Option<&Sexp<'_>>, units: Unit) -> Result<Rules, DsnError> {
        let board = structure
            .and_then(|s| s.child("rule"))
            .map_or_else(Overrides::default, |r| self.overrides(r, units));
        let width = board.track_width.unwrap_or_else(|| {
            self.warn("no default wire width: none required");
            1
        });
        let clearance = board.clearance.unwrap_or_else(|| {
            self.warn("no default clearance: none required");
            0
        });
        let names: Vec<&str> = self.layers.iter().map(String::as_str).collect();
        let mut spec = RuleSpec::new(&names, width.max(1), clearance);
        if let Some(s) = structure {
            for layer in s.children("layer") {
                let (Some(name), Some(rule)) = (layer.arg(0), layer.child("rule")) else {
                    continue;
                };
                let o = self.overrides(rule, units);
                if let Some(l) = self.layer(name)
                    && !spec.layer_overrides.iter().any(|(x, _)| *x == l)
                {
                    spec.layer_overrides.push((l, o));
                }
            }
        }
        // The vias with a drill, by padstack, and the default: the structure's first.
        let mut via_ids: BTreeMap<String, ViaId> = BTreeMap::new();
        for ps in &self.via_padstacks {
            if via_ids.contains_key(&ps.name) {
                continue;
            }
            let Some(drill) = ps.drill.filter(|&d| d > 0 && d < ps.diameter) else {
                continue;
            };
            via_ids.insert(ps.name.clone(), ViaId(spec.vias.len() as u32));
            spec.vias.push(ViaDef {
                name: ps.name.clone(),
                drill,
                annular_ring: (ps.diameter - drill) / 2,
                from: ps.from,
                to: ps.to,
                cost: crate::cost::Cost::ZERO,
            });
            if spec.default_via.is_none() && self.allowed_vias.first() == Some(&ps.name) {
                spec.default_via = via_ids.get(&ps.name).copied();
            }
        }
        let mut taken: BTreeMap<NetId, String> = BTreeMap::new();
        let classes = std::mem::take(&mut self.classes);
        for class in classes {
            let mut nets = Vec::new();
            for net in class.nets {
                match taken.get(&net) {
                    Some(_) => self.warn("net in two classes: kept in the first"),
                    None => {
                        taken.insert(net, class.name.clone());
                        nets.push(net);
                    }
                }
            }
            if spec.classes.iter().any(|c| c.name == class.name) {
                self.warn("class defined twice, skipped");
                continue;
            }
            let mut layers: Vec<(LayerId, Overrides)> = Vec::new();
            for (l, o) in class.layers {
                if !layers.iter().any(|(x, _)| *x == l) {
                    layers.push((l, o));
                }
            }
            spec.classes.push(NetClass {
                name: class.name,
                nets,
                overrides: class.overrides,
                layers,
                via: class.vias.iter().find_map(|v| via_ids.get(v).copied()),
            });
        }
        Rules::new(spec).map_err(|e| DsnError::other(format!("invalid rules: {e}")))
    }
}

/// Degrees, as text, in millidegrees, rounded half away from zero.
fn degrees(text: &str) -> Option<i32> {
    scaled(text, 1, 3, 3_600_000).map(|v| v as i32)
}

/// The drill of a via padstack that KiCad named, as `Via[0-1]_800:400_um`: what follows the
/// colon, to the last underscore, in micrometres. This is how KiCad's own session import
/// finds a via's drill.
pub(crate) fn kicad_via_drill(name: &str) -> Option<i64> {
    let after = &name[name.find(':')? + 1..];
    let (number, unit) = after.rsplit_once('_')?;
    let unit = Unit::from_name(unit)?;
    let (factor, scale) = unit.factor();
    scaled(number, factor, scale, MAX_COORD).filter(|&d| d > 0)
}

/// The middle of the arc that turns counter-clockwise about `center` from `start` to
/// `end`, as a Specctra `qarc` does, rounded to the nearest nanometre. A full circle if
/// `start` is `end`.
fn arc_mid(start: Point, end: Point, center: Point) -> Point {
    if start == end {
        return Point::new(2 * center.x - start.x, 2 * center.y - start.y);
    }
    let (sx, sy) = ((start.x - center.x) as f64, (start.y - center.y) as f64);
    let (ex, ey) = ((end.x - center.x) as f64, (end.y - center.y) as f64);
    let r = (sx * sx + sy * sy).sqrt();
    // The bisector of the two radii, on the side the arc turns through.
    let cross = sx * ey - sy * ex;
    let (mut bx, mut by) = (sx + ex, sy + ey);
    let len = (bx * bx + by * by).sqrt();
    if len < 1e-9 * r.max(1.0) {
        // Half a circle, or a whole one: a quarter turn on from the start.
        (bx, by) = (-sy, sx);
    } else {
        (bx, by) = (bx / len * r, by / len * r);
        if cross < 0.0 {
            // More than half a turn: the other side.
            (bx, by) = (-bx, -by);
        }
    }
    Point::new(center.x + bx.round() as i64, center.y + by.round() as i64)
}
