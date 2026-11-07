//! Tests of the DSN reader and the SES writer on small, hand-written files.

use super::*;
use crate::geom::drc::Track;
use crate::geom::{DEFAULT_ARC_TOLERANCE, LayerSet, Shape};
use crate::sexpr::{Sexp, parse_specctra};

/// A mil in nanometres.
const MIL: i64 = 25_400;

/// A design in mils, named in the default quotes before declaring its own, three layers, components on both sides and at
/// other angles, classes, keepouts, a plane, and wiring of every kind.
const DESIGN: &str = r#"(pcb "my board"
  (parser (string_quote ') (space_in_quoted_tokens on) (host_cad "test"))
  (resolution mil 1000)
  (unit mil)
  (structure
    (layer Top (type signal))
    (layer Inner (type power) (rule (clearance 12)))
    (layer Bottom (type signal))
    (boundary (rect pcb 0 0 1000 500))
    (boundary (path signal 0 10 10 990 10 990 490 10 490 10 10))
    (plane GND (polygon Inner 0 0 0 1000 0 1000 500 0 500))
    (keepout 'k 1' (rect Top 100 100 200 200))
    (via_keepout (circle signal 50 500 250))
    (via 'Via[0-2]_600:300_um' plain)
    (rule (width 10) (clearance 8) (clearance 2 (type smd_smd))))
  (placement
    (component R
      (place R1 300 200 front 90)
      (place R2 600 200 back 90))
    (component U (place U1 800 300 front 30))
    (component X (place X1)))
  (library
    (image R
      (pin smd 1 -20 0)
      (pin smd (rotate 90) 2 20 0)
      (keepout (circle Top 10 0 30)))
    (image U (outline (rect signal -50 -50 50 50)) (pin oval 1 0 0))
    (padstack smd (shape (rect Top -5 -10 5 10)) (attach off))
    (padstack oval (shape (path signal 10 -5 0 5 0)))
    (padstack 'Via[0-2]_600:300_um'
      (shape (circle Top 24)) (shape (circle Inner 24)) (shape (circle Bottom 24)))
    (padstack plain (shape (circle Top 20)) (shape (circle Bottom 20))))
  (network
    (net 'A 1' (pins R1-1 R2-2))
    (net B (pins R1-2 U1-1))
    (class wide B
      (circuit (use_via plain 'Via[0-2]_600:300_um'))
      (rule (width 20) (clearance 15))
      (layer_rule Bottom (rule (clearance 25)))))
  (wiring
    (wire (path Top 10 280 200 100 200 100 400) (net 'A 1') (type protect))
    (wire (qarc Top 10 0 100 100 0 0 0) (net B))
    (via 'Via[0-2]_600:300_um' 500 400 (net B) (type fix))
    (via plain 520 400 (net B))
    (wire (path Nowhere 10 0 0 1 1) (net B))
    (wire (path Top 10 0 0 10 0))))
"#;

fn p(x: i64, y: i64) -> Point {
    Point::new(x * MIL, y * MIL)
}

fn warned(b: &DsnBoard, text: &str) -> bool {
    b.warnings.iter().any(|(w, _)| w.contains(text))
}

#[test]
fn reads_units_layers_and_rules() {
    let b = read(DESIGN).unwrap();
    assert_eq!(b.name, "my board");
    assert_eq!(
        b.resolution,
        Resolution {
            unit: Unit::Mil,
            steps: 1000
        }
    );
    let spec = b.rules.spec();
    assert_eq!(spec.layers, ["Top", "Inner", "Bottom"]);
    assert_eq!(spec.track_width, 10 * MIL);
    assert_eq!(spec.clearance, 8 * MIL);
    assert_eq!(spec.layer_overrides.len(), 1);
    assert_eq!(spec.layer_overrides[0].0, LayerId(1));
    assert_eq!(spec.layer_overrides[0].1.clearance, Some(12 * MIL));
    assert!(warned(&b, "type smd_smd"));

    // Only the via with a drill in its name is a rule's via, and it is the default.
    assert_eq!(spec.vias.len(), 1);
    let via = &spec.vias[0];
    assert_eq!(via.name, "Via[0-2]_600:300_um");
    assert_eq!((via.drill, via.diameter()), (300_000, 24 * MIL));
    assert_eq!((via.from, via.to), (LayerId(0), LayerId(2)));
    assert_eq!(spec.default_via, Some(crate::geom::ViaId(0)));
    assert!(warned(&b, "no drill in its name"));
    let plain = b.via_padstacks.iter().find(|v| v.name == "plain").unwrap();
    assert_eq!((plain.diameter, plain.drill), (20 * MIL, None));

    // The class, with its nets, rules and the first of its vias that has a drill.
    let class = &spec.classes[0];
    assert_eq!(class.name, "wide");
    assert_eq!(class.nets, [b.net("B").unwrap()]);
    assert_eq!(class.overrides.track_width, Some(20 * MIL));
    assert_eq!(class.overrides.clearance, Some(15 * MIL));
    assert_eq!(class.layers.len(), 1);
    assert_eq!(class.layers[0].1.clearance, Some(25 * MIL));
    assert_eq!(class.via, Some(crate::geom::ViaId(0)));
    let (a, bb) = (b.net("A 1"), b.net("B"));
    assert_eq!(b.rules.clearance(bb, LayerId(2)), 25 * MIL);
    assert_eq!(b.rules.clearance(a, LayerId(1)), 12 * MIL);

    // The board's outline, and not the signal boundary; the plane, as an outline only.
    assert_eq!(
        b.boundary,
        [vec![p(0, 0), p(1000, 0), p(1000, 500), p(0, 500)]]
    );
    assert_eq!(b.planes.len(), 1);
    assert_eq!(b.planes[0].layer, LayerId(1));
    assert_eq!(b.planes[0].net, b.net("GND"));
    assert!(b.board.zones.is_empty());
}

impl DsnBoard {
    fn net(&self, name: &str) -> Option<crate::id::NetId> {
        let i = self.nets.iter().position(|n| n == name)?;
        Some(crate::id::NetId(i as u32))
    }
}

#[test]
fn places_pins_on_both_sides_and_at_any_angle() {
    let b = read(DESIGN).unwrap();
    let refs: Vec<&str> = b.components.iter().map(|c| c.reference.as_str()).collect();
    assert_eq!(refs, ["R1", "R2", "U1"]);
    assert!(warned(&b, "isn't placed"));
    assert_eq!(b.components[1].side, Side::Back);
    assert_eq!(b.components[2].rotation, 30_000);

    let pad = |reference: &str, pin: &str| {
        let i = b
            .pads
            .iter()
            .position(|s| b.components[s.component].reference == reference && s.pin == pin)
            .unwrap();
        &b.board.pads[i]
    };
    let top = LayerSet::EMPTY.with(LayerId(0));
    let bottom = LayerSet::EMPTY.with(LayerId(2));

    // R1, turned a quarter: pin 1 from (-20, 0) to (0, -20), and 20 wide by 10 high.
    let r1 = pad("R1", "1");
    assert_eq!(r1.layers, top);
    assert_eq!(
        r1.shape.bbox(),
        crate::geom::Rect::new(p(290, 175), p(310, 185))
    );
    assert_eq!(r1.net, b.net("A 1"));
    // Pin 2 turns a quarter of its own, so a half in all: 10 wide by 20 high.
    let r2 = pad("R1", "2");
    assert_eq!(
        r2.shape.bbox(),
        crate::geom::Rect::new(p(295, 210), p(305, 230))
    );
    assert_eq!(r2.net, b.net("B"));

    // R2, on the back: mirrored, then turned a quarter, on the bottom layer.
    let r2_1 = pad("R2", "1");
    assert_eq!(r2_1.layers, bottom);
    assert_eq!(
        r2_1.shape.bbox(),
        crate::geom::Rect::new(p(590, 215), p(610, 225))
    );
    assert_eq!(r2_1.net, None);
    assert_eq!(pad("R2", "2").net, b.net("A 1"));

    // U1, at 30°: an oval, on every layer, its ends rounded to the nanometre.
    let u1 = pad("U1", "1");
    assert_eq!(u1.layers, LayerSet::first(3));
    let (c, s) = (127_000.0 * 0.75f64.sqrt(), 63_500.0);
    let expected = Shape::track(
        Point::new(800 * MIL - c.round() as i64, 300 * MIL - s as i64),
        Point::new(800 * MIL + c.round() as i64, 300 * MIL + s as i64),
        10 * MIL,
    )
    .unwrap();
    assert_eq!(u1.shape, expected);
}

#[test]
fn reads_keepouts_with_their_components() {
    let b = read(DESIGN).unwrap();
    // The structure's two, then R1's and R2's.
    assert_eq!(b.board.keepouts.len(), 4);
    assert_eq!(b.keepouts, [None, None, Some(0), Some(1)]);
    let k = &b.board.keepouts;
    assert!(k[0].tracks && k[0].vias && !k[0].pads);
    assert!(!k[1].tracks && k[1].vias);
    assert_eq!(k[1].layers, LayerSet::first(3));
    assert_eq!(k[1].shape, Shape::circle(p(500, 250), 50 * MIL).unwrap());
    // R1's circle at (0, 30), turned a quarter; R2's mirrored first, and on the bottom.
    assert_eq!(k[2].shape, Shape::circle(p(270, 200), 10 * MIL).unwrap());
    assert_eq!(k[3].shape, Shape::circle(p(570, 200), 10 * MIL).unwrap());
    assert_eq!(k[3].layers, LayerSet::EMPTY.with(LayerId(2)));
    assert_eq!(
        b.describe(Item::Keepout(3)).as_deref(),
        Some("keepout of R2")
    );
}

#[test]
fn reads_wires_arcs_and_vias() {
    let b = read(DESIGN).unwrap();
    let t = &b.board.tracks;
    // A path of three points is two tracks of one wire; then the arc; then one wire with
    // no net.
    assert_eq!(t.len(), 4);
    assert_eq!((t[0].start, t[0].end), (p(280, 200), p(100, 200)));
    assert_eq!((t[1].start, t[1].end), (p(100, 200), p(100, 400)));
    assert_eq!(t[0].width, 10 * MIL);
    assert_eq!(b.tracks[0].kind, WireType::Protect);
    assert_eq!(b.tracks[1], b.tracks[0]);
    assert!(b.tracks[0].kind.is_fixed());
    // Counter-clockwise from (0, 100) to (100, 0) about the origin: three quarters of a
    // turn, through (-70.7, -70.7).
    let r = (100.0 * MIL as f64 * 0.5f64.sqrt()).round() as i64;
    assert_eq!(t[2].mid, Some(Point::new(-r, -r)));
    assert_eq!(b.tracks[2].kind, WireType::Normal);
    assert_eq!(t[3].net, Some(crate::id::NetId(0)));
    assert_eq!(b.nets[0], "");
    assert!(warned(&b, "isn't copper"));

    let v = &b.board.vias;
    assert_eq!(v.len(), 2);
    assert_eq!(
        (v[0].at, v[0].drill, v[0].diameter),
        (p(500, 400), 300_000, 24 * MIL)
    );
    assert_eq!((v[0].from, v[0].to), (LayerId(0), LayerId(2)));
    assert_eq!(b.vias[0].kind, WireType::Fix);
    // No drill in the name: half the diameter.
    assert_eq!((v[1].drill, v[1].diameter), (10 * MIL, 20 * MIL));
    assert_eq!(b.describe(Item::Via(1)).as_deref(), Some("via 3"));
    assert_eq!(b.describe(Item::Track(1)).as_deref(), Some("wire 0 on Top"));

    // And the board can be checked.
    let found = drc::check(&b.board, &b.rules).unwrap();
    assert!(!found.is_empty());
}

#[test]
fn arcs_turn_counter_clockwise() {
    let b = |start: (i64, i64), end: (i64, i64)| {
        let text = format!(
            "(pcb x (structure (layer A)) (wiring (wire (qarc A 1 {} {} {} {} 0 0))))",
            start.0, start.1, end.0, end.1
        );
        let b = read(&text).unwrap();
        b.board.tracks[0].mid.unwrap()
    };
    // The resolution defaults to inches, as Specctra's does.
    let inch = 25_400_000;
    let half = (inch as f64 * 0.5f64.sqrt()).round() as i64;
    assert_eq!(b((1, 0), (0, 1)), Point::new(half, half));
    // Half a turn, and a whole one.
    assert_eq!(b((1, 0), (-1, 0)), Point::new(0, inch));
    assert_eq!(b((1, 0), (1, 0)), Point::new(-inch, 0));
}

#[test]
fn exact_lengths_in_every_unit() {
    for (unit, text, nm) in [
        ("inch", "0.1", 2_540_000),
        ("mil", "0.00002", 1),
        ("mil", "-0.00002", -1),
        ("mil", "0.00001", 0),
        ("cm", "1.5", 15_000_000),
        ("mm", "0.0000005", 1),
        ("um", "67170.7", 67_170_700),
    ] {
        let text = format!(
            "(pcb x (unit {unit}) (structure (layer A)) (wiring (wire (path A 1 0 0 {text} 0))))"
        );
        let b = read(&text).unwrap();
        assert_eq!(b.board.tracks[0].end.x, nm, "{unit} {text}");
    }
    // A section's own unit.
    let b = read(
        "(pcb x (unit mm) (structure (layer A)) (wiring (unit um) (wire (path A 1 0 0 5 0))))",
    )
    .unwrap();
    assert_eq!(b.board.tracks[0].end.x, 5_000);
}

#[test]
fn rejects_what_isnt_a_design() {
    assert!(read("(session x)").is_err());
    assert!(
        read("(pcb x (structure))")
            .unwrap_err()
            .message()
            .contains("no layers")
    );
    let e = read("(pcb x\n  (structure (layer A)\n").unwrap_err();
    assert_eq!(e.line(), Some(2));
    let e = read("(pcb x\n  (net \"open))\n").unwrap_err();
    assert_eq!(e.line(), Some(2));
}

/// The wires and vias of a session, as `(layer, width, points)` and `(padstack, at)`, in
/// steps of its resolution.
type Wire = (String, i64, Vec<(i64, i64)>);

fn routes(ses: &str) -> (Vec<Wire>, Vec<(String, i64, i64)>) {
    let root = parse_specctra(ses).unwrap();
    let numbers = |s: &Sexp<'_>, from: usize| -> Vec<i64> {
        s.args()[from..]
            .iter()
            .filter_map(Sexp::as_atom)
            .map(|a| a.parse().unwrap())
            .collect()
    };
    let (mut wires, mut vias) = (Vec::new(), Vec::new());
    let out = root.child("routes").unwrap().child("network_out").unwrap();
    for net in out.children("net") {
        for wire in net.children("wire") {
            let path = wire.child("path").unwrap();
            let n = numbers(path, 1);
            let points = n[1..].chunks(2).map(|c| (c[0], c[1])).collect();
            wires.push((path.arg(0).unwrap().to_string(), n[0], points));
        }
        for via in net.children("via") {
            let n = numbers(via, 1);
            vias.push((via.arg(0).unwrap().to_string(), n[0], n[1]));
        }
    }
    (wires, vias)
}

#[test]
fn sessions_hold_the_model_exactly() {
    let b = read(DESIGN).unwrap();
    let layers = &b.rules.spec().layers;
    let mut s = Session::new("test.dsn", layers, &b.nets, &b.board.tracks, &b.board.vias);
    s.components = &b.components;
    let types: Vec<WireType> = b.tracks.iter().map(|t| t.kind).collect();
    s.track_types = &types;
    let ses = write_session(&s).unwrap();
    let root = parse_specctra(&ses).unwrap();
    assert!(root.is("session"));
    assert_eq!(root.child("base_design").unwrap().arg(0), Some("test.dsn"));

    // The placement, by image.
    let placement = root.child("placement").unwrap();
    let components: Vec<&Sexp<'_>> = placement.children("component").collect();
    assert_eq!(components.len(), 2);
    let r2 = components[0].children("place").nth(1).unwrap();
    let args: Vec<&str> = r2.args().iter().filter_map(Sexp::as_atom).collect();
    assert_eq!(args, ["R2", "15240000", "5080000", "back", "90"]);

    // Straight tracks exactly, in nanometres, and the arc as a polyline from its start to
    // its end, within the tolerance of the circle.
    let (wires, vias) = routes(&ses);
    assert_eq!(wires.len(), 4);
    // Net by net, starting with the net with no name.
    assert_eq!(wires[0].2, [(0, 0), (254_000, 0)]);
    let w = |i: usize| (wires[i].0.as_str(), wires[i].1, wires[i].2.clone());
    assert_eq!(
        w(1),
        (
            "Top",
            254_000,
            vec![(7_112_000, 5_080_000), (2_540_000, 5_080_000)]
        )
    );
    assert!(ses.contains("(type protect)"));
    let arc = &wires
        .iter()
        .find(|w| w.2.len() > 2)
        .expect("the arc is a polyline")
        .2;
    assert_eq!(arc[0], (0, 2_540_000));
    assert_eq!(arc[arc.len() - 1], (2_540_000, 0));
    for &(x, y) in arc {
        let r = ((x * x + y * y) as f64).sqrt();
        assert!(
            (r - 2_540_000.0).abs() <= (DEFAULT_ARC_TOLERANCE + 1) as f64,
            "{r}"
        );
    }
    // The arc turns the long way round, through the third quadrant.
    assert!(arc.iter().any(|&(x, y)| x < 0 && y < 0));

    // Vias by KiCad's names, which give their drills back.
    assert_eq!(vias.len(), 2);
    assert_eq!(vias[0].0, "Via[0-2]_609.6:300_um");
    assert_eq!(read::kicad_via_drill(&vias[0].0), Some(300_000));
    assert_eq!(vias[1].0, "Via[0-2]_508:254_um");
    let library = root.child("routes").unwrap().child("library_out").unwrap();
    let first = library.child("padstack").unwrap();
    assert_eq!(first.children("shape").count(), 3);
}

#[test]
fn sessions_round_in_coarser_steps() {
    let layers = ["A".to_string()];
    let nets = [String::new(), "N".to_string()];
    let tracks = [Track {
        start: Point::new(150, -150),
        end: Point::new(149, -149),
        mid: None,
        width: 1_000,
        layer: LayerId(0),
        net: None,
    }];
    let mut s = Session::new("x", &layers, &nets, &tracks, &[]);
    s.resolution = Resolution {
        unit: Unit::Um,
        steps: 10,
    };
    let ses = write_session(&s).unwrap();
    assert!(ses.contains("(resolution um 10)"), "{ses}");
    let (wires, _) = routes(&ses);
    assert_eq!(wires[0].1, 10);
    assert_eq!(wires[0].2, [(2, -2), (1, -1)]);
    assert!(ses.contains("(net \"\""));
    // An empty library, which KiCad wants.
    assert!(ses.contains("(library_out\n    )"));
}

#[test]
fn sessions_refuse_what_they_cant_hold() {
    let layers = ["A".to_string()];
    let nets = ["say \"hi\"".to_string()];
    let tracks = [Track {
        start: Point::new(0, 0),
        end: Point::new(1, 0),
        mid: None,
        width: 1,
        layer: LayerId(0),
        net: Some(crate::id::NetId(0)),
    }];
    assert!(write_session(&Session::new("x", &layers, &nets, &tracks, &[])).is_err());
    let mut bad = tracks;
    bad[0].layer = LayerId(3);
    let nets = ["N".to_string()];
    assert!(write_session(&Session::new("x", &layers, &nets, &bad, &[])).is_err());
    bad[0].layer = LayerId(0);
    bad[0].net = Some(crate::id::NetId(5));
    assert!(write_session(&Session::new("x", &layers, &nets, &bad, &[])).is_err());
}

#[test]
fn kicad_via_names_give_drills() {
    assert_eq!(read::kicad_via_drill("Via[0-1]_800:400_um"), Some(400_000));
    assert_eq!(
        read::kicad_via_drill("Via[0-3]_600:317.5_um"),
        Some(317_500)
    );
    assert_eq!(read::kicad_via_drill("Via[0-1]_32:16_mil"), Some(16 * MIL));
    assert_eq!(read::kicad_via_drill("via"), None);
    assert_eq!(read::kicad_via_drill("Via[0-1]_800:x_um"), None);
}
