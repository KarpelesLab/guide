use super::*;
use crate::geom::drc::Violation;
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::NetId;

const MM: i64 = 1_000_000;

/// A board with two copper layers, the nets given, and the items given.
fn board(nets: &[&str], items: &str) -> String {
    let mut text = String::from(
        "(kicad_pcb (version 20240108) (generator \"pcbnew\")\n\
         (layers (0 \"F.Cu\" signal) (31 \"B.Cu\" signal) (44 \"Edge.Cuts\" user))\n(net 0 \"\")\n",
    );
    for (i, n) in nets.iter().enumerate() {
        text += &format!("(net {} \"{n}\")\n", i + 1);
    }
    text + items + ")\n"
}

fn rect(x0: i64, y0: i64, x1: i64, y1: i64) -> Rect {
    Rect::new(Point::new(x0, y0), Point::new(x1, y1))
}

/// KiCad's Y runs down and ours up: a box from KiCad's coordinates.
fn kicad_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    let n = |v: f64| (v * MM as f64).round() as i64;
    rect(n(x0), -n(y1), n(x1), -n(y0))
}

/// The gap between a shape and a point, in nanometres, rounded.
fn gap_to(shape: &Shape, x: f64, y: f64) -> f64 {
    let point = Point::new((x * 1e6).round() as i64, (-y * 1e6).round() as i64);
    let probe = Shape::track(point, point, 1).unwrap();
    // The probe's radius is half a unit.
    let d = shape.distance(&probe).to_f64();
    if d > 0.0 { (d + 0.5).round() } else { 0.0 }
}

/// Whether two boxes are the same to within `slack` on every side.
fn near(a: Rect, b: Rect, slack: i64) -> bool {
    let close = |p: Point, q: Point| (p.x - q.x).abs() <= slack && (p.y - q.y).abs() <= slack;
    close(a.min(), b.min()) && close(a.max(), b.max())
}

#[test]
fn reads_tracks_vias_and_nets() {
    let text = board(
        &["A", "B"],
        "(segment (start 1 2) (end 3 2) (width 0.25) (layer \"F.Cu\") (net 1) (uuid \"t\"))\n\
         (arc (start 0 0) (mid 1 1) (end 2 0) (width 0.2) (layer \"B.Cu\") (net 2) (uuid \"a\"))\n\
         (via (at 5 5) (size 0.6) (drill 0.3) (layers \"F.Cu\" \"B.Cu\") (net 2) (uuid \"v\"))\n\
         (segment (start 1 2) (end 3 2) (width 0.25) (layer \"Dwgs.User\") (net 1))\n\
         (future_thing (with \"anything\" (in it)))\n",
    );
    let read = read(&text, None).unwrap();
    assert_eq!(read.nets, ["", "A", "B"]);
    assert_eq!(read.rules.spec().layers, ["F.Cu", "B.Cu"]);
    let t = &read.board.tracks[0];
    assert_eq!(
        (t.start, t.end, t.width),
        (Point::new(MM, -2 * MM), Point::new(3 * MM, -2 * MM), MM / 4)
    );
    assert_eq!((t.layer, t.net), (LayerId(0), Some(NetId(1))));
    let a = &read.board.tracks[1];
    assert_eq!((a.mid, a.layer), (Some(Point::new(MM, -MM)), LayerId(1)));
    let v = &read.board.vias[0];
    assert_eq!(
        (v.at, v.diameter, v.drill, v.from, v.to),
        (
            Point::new(5 * MM, -5 * MM),
            600_000,
            300_000,
            LayerId(0),
            LayerId(1)
        )
    );
    // The track on a drawing layer isn't copper.
    assert_eq!(read.board.tracks.len(), 2);
    assert_eq!(read.tracks[1].uuid.as_deref(), Some("a"));
    assert_eq!(read.source(Item::Via(0)).unwrap().description, "via");
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    // KiCad's default clearance applies without a project.
    assert_eq!(read.rules.spec().clearance, 200_000);
}

#[test]
fn reads_kicad_10_net_names_and_kicad_5_files() {
    let v10 = "(kicad_pcb (version 20260206) (layers (0 \"F.Cu\" signal) (2 \"B.Cu\" signal) (4 \"In1.Cu\" signal))\n\
               (segment (start 0 0) (end 1 0) (width 0.2) (layer \"In1.Cu\") (net \"GND\"))\n\
               (segment (start 0 1) (end 1 1) (width 0.2) (layer \"B.Cu\") (net \"3\")))";
    let read = read(v10, None).unwrap();
    assert_eq!(read.rules.spec().layers, ["F.Cu", "In1.Cu", "B.Cu"]);
    assert_eq!(read.nets, ["", "GND", "3"]);
    assert_eq!(read.board.tracks[0].layer, LayerId(1));

    let v5 = "(kicad_pcb (version 20171130) (host pcbnew 5.1)\n\
              (layers (0 F.Cu signal) (31 B.Cu signal))\n\
              (setup (hole_to_hole_min 0.3))\n\
              (net 0 \"\") (net 1 GND) (net 2 HV)\n\
              (net_class Default \"\" (clearance 0.15) (add_net GND))\n\
              (net_class Power \"\" (clearance 0.5) (add_net HV))\n\
              (module R (layer F.Cu) (tedit 0) (tstamp 1) (at 10 10 90)\n\
                (fp_text reference R1 (at 0 0) (layer F.SilkS))\n\
                (fp_arc (start 0 0) (end 1 0) (angle 90) (layer F.Cu) (width 0.1))\n\
                (pad 1 smd rect (at 1 0 90) (size 1 0.5) (layers F.Cu) (net 2 HV) (tstamp 2))))";
    let read = super::read(v5, None).unwrap();
    assert_eq!(read.rules.spec().clearance, 150_000);
    assert_eq!(read.rules.spec().hole_to_hole, 300_000);
    assert_eq!(read.rules.clearance(Some(NetId(2)), LayerId(0)), 500_000);
    // The pad is 1 mm right of the footprint, which is turned 90°: 1 mm up on screen.
    assert_eq!(
        read.board.pads[0].shape.bbox(),
        kicad_rect(9.75, 8.5, 10.25, 9.5)
    );
    assert_eq!(read.pads[0].description, "pad 1 of R1");
    // KiCad 5's arc: centre (0, 0), from (1, 0), 90° clockwise on screen, turned with the
    // footprint, which takes (1, 0) to (0, -1) and the arc's end to (1, 0).
    let arc = &read.board.zones[0].shape;
    assert!(
        near(arc.bbox(), kicad_rect(9.95, 8.95, 11.05, 10.05), 20),
        "{:?}",
        arc.bbox()
    );
}

#[test]
fn places_pads_of_every_shape() {
    let text = board(
        &["A"],
        "(footprint \"X\" (layer \"F.Cu\") (at 10 20 90) (property \"Reference\" \"U1\")\n\
           (pad \"1\" smd rect (at 1 0 90) (size 2 1) (layers \"F.Cu\") (net 1 \"A\") (uuid \"p1\"))\n\
           (pad \"2\" smd circle (at 0 0) (size 1 1) (layers \"B.Cu\") (uuid \"p2\"))\n\
           (pad \"3\" smd oval (at 0 0 0) (size 3 1) (layers \"F.Cu\") (uuid \"p3\"))\n\
           (pad \"4\" smd roundrect (at 0 0 90) (size 2 1) (layers \"F.Cu\") (roundrect_rratio 0.25) (uuid \"p4\"))\n\
           (pad \"5\" smd trapezoid (at 0 0 90) (size 2 1) (rect_delta 0 0.4) (layers \"F.Cu\") (uuid \"p5\"))\n\
           (pad \"6\" smd roundrect (at 0 0 90) (size 2 2) (layers \"F.Cu\") (roundrect_rratio 0) (chamfer_ratio 0.25) (chamfer top_left) (uuid \"p6\"))\n\
           (pad \"7\" thru_hole circle (at 0 0 90) (size 2 2) (drill 1 (offset 0.5 0)) (layers \"*.Cu\") (uuid \"p7\"))\n\
           (pad \"8\" np_thru_hole circle (at 0 0) (size 1 1) (drill 1) (layers \"*.Cu\" \"*.Mask\") (uuid \"p8\"))\n\
           (pad \"9\" smd custom (at 0 0 90) (size 0.5 0.5) (layers \"F.Cu\") (options (clearance outline) (anchor circle))\n\
             (primitives (gr_poly (pts (xy 0 0) (xy 2 0) (xy 2 1)) (width 0) (fill yes)) (gr_line (start 0 0) (end 0 3) (width 0.2))) (uuid \"p9\"))\n\
           (pad \"\" thru_hole oval (at 0 0 90) (size 1 2) (drill oval 0.5 1.5) (layers \"*.Cu\") (uuid \"p10\")))\n",
    );
    let read = read(&text, None).unwrap();
    let pads = &read.board.pads;
    let by_uuid = |u: &str| -> Vec<&crate::geom::drc::Pad> {
        read.pads
            .iter()
            .zip(pads)
            .filter(|(s, _)| s.uuid.as_deref() == Some(u))
            .map(|(_, p)| p)
            .collect()
    };
    // At (1, 0) in the footprint, turned 90°: (10, 19). Its own 90° makes it 1 by 2.
    let p1 = by_uuid("p1")[0];
    assert_eq!(p1.shape.bbox(), kicad_rect(9.5, 18.0, 10.5, 20.0));
    assert_eq!(
        (p1.layers, p1.net, p1.hole.is_none()),
        (LayerSet(1), Some(NetId(1)), true)
    );
    // No net is no net at all, for a pad.
    let p2 = by_uuid("p2")[0];
    assert_eq!((p2.layers, p2.net), (LayerSet(2), None));
    assert_eq!(p2.shape.bbox(), kicad_rect(9.5, 19.5, 10.5, 20.5));
    assert_eq!(
        by_uuid("p3")[0].shape.bbox(),
        kicad_rect(8.5, 19.5, 11.5, 20.5)
    );
    // A rounded rectangle of 2 by 1, turned 90°, with corners of radius 0.25.
    let p4 = &by_uuid("p4")[0].shape;
    assert_eq!(p4.bbox(), kicad_rect(9.5, 19.0, 10.5, 21.0));
    let corner = (0.25f64 * 2f64.sqrt() - 0.25) * 1e6;
    assert_eq!(gap_to(p4, 9.5, 19.0), corner.round());
    // A trapezoid: KiCad's delta along Y widens one side along X and narrows the other.
    // Turned 90°, X is Y.
    let p5 = &by_uuid("p5")[0].shape;
    assert_eq!(p5.bbox(), kicad_rect(9.5, 18.8, 10.5, 21.2));
    // A square chamfered at its top left, by a quarter of its side.
    let p6 = &by_uuid("p6")[0].shape;
    assert_eq!(p6.bbox(), kicad_rect(9.0, 19.0, 11.0, 21.0));
    // Its top left, before it turns 90°, is at the bottom left after, on screen.
    let corners =
        [(9.0, 19.0), (11.0, 19.0), (9.0, 21.0), (11.0, 21.0)].map(|(x, y)| gap_to(p6, x, y));
    // The cut is 0.5 mm along each side, which leaves the corner 0.5 / sqrt(2) from it.
    assert_eq!(corners, [0.0, 0.0, 353_553.0, 0.0]);
    // The copper is offset from the hole, by 0.5 turned 90°: up.
    let p7 = by_uuid("p7")[0];
    assert_eq!(p7.shape.bbox(), kicad_rect(9.0, 18.5, 11.0, 20.5));
    assert_eq!(
        p7.hole.as_ref().unwrap().bbox(),
        kicad_rect(9.5, 19.5, 10.5, 20.5)
    );
    assert_eq!(p7.layers, LayerSet(3));
    // A hole as big as its pad leaves no copper.
    let p8 = by_uuid("p8")[0];
    assert_eq!((p8.layers, p8.hole.is_some()), (LayerSet::EMPTY, true));
    // A custom pad is its anchor and its primitives, turned with it.
    let p9 = by_uuid("p9");
    assert_eq!(p9.len(), 3);
    assert_eq!(p9[1].shape.bbox(), kicad_rect(10.0, 18.0, 11.0, 20.0));
    assert_eq!(p9[2].shape.bbox(), kicad_rect(9.9, 19.9, 13.1, 20.1));
    assert!(p9[0].hole.is_none() && p9[1].hole.is_none());
    // An oval slot, turned with its pad.
    let p10 = by_uuid("p10")[0];
    assert_eq!(
        p10.hole.as_ref().unwrap().bbox(),
        kicad_rect(9.25, 19.75, 10.75, 20.25)
    );
    assert_eq!(read.pads[pads.len() - 1].description, "pad of U1");
}

#[test]
fn own_clearances_and_classes() {
    let text = board(
        &["A", "B", "D+", "D-"],
        "(footprint \"X\" (layer \"F.Cu\") (at 0 0) (clearance 0.3)\n\
           (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\") (net 1 \"A\"))\n\
           (pad \"2\" smd rect (at 2 0) (size 1 1) (layers \"F.Cu\") (net 2 \"B\") (clearance 0.4)))\n\
         (zone (net 1) (net_name \"A\") (layer \"F.Cu\") (connect_pads (clearance 0.5))\n\
           (filled_polygon (layer \"F.Cu\") (pts (xy 0 5) (xy 1 5) (xy 1 6))))\n",
    );
    let project = r#"{"board": {"design_settings": {"rules": {"min_clearance": 0.1, "min_hole_clearance": 0.25, "min_hole_to_hole": 0.3}}},
        "net_settings": {"classes": [{"name": "Default", "clearance": 0.2, "diff_pair_gap": 0.25},
                                     {"name": "USB", "clearance": 0.3, "diff_pair_gap": 0.15}],
                         "netclass_patterns": [{"netclass": "USB", "pattern": "D?"}]}}"#;
    let read = read(&text, Some(project)).unwrap();
    assert_eq!(read.board.pads[0].clearance, Some(300_000));
    assert_eq!(read.board.pads[1].clearance, Some(400_000));
    // A zone's own clearance is what it was filled with, not a rule.
    assert_eq!(read.board.zones[0].clearance, None);
    let spec = read.rules.spec();
    assert_eq!(
        (spec.min_clearance, spec.hole_clearance, spec.hole_to_hole),
        (100_000, 250_000, 300_000)
    );
    assert_eq!(spec.classes.len(), 1);
    assert_eq!(spec.classes[0].nets, [NetId(3), NetId(4)]);
    // The pair's gap is narrower than its class's clearance, so it applies between them.
    assert_eq!(spec.pairs, [(NetId(3), NetId(4), 150_000)]);
    let (dp, dm) = (Some(NetId(3)), Some(NetId(4)));
    assert_eq!(read.rules.clearance_between(dp, dm, LayerId(0)), 150_000);
    assert_eq!(
        read.rules.clearance_between(dp, Some(NetId(1)), LayerId(0)),
        300_000
    );
}

#[test]
fn zone_fills_are_unfractured() {
    // A square of 10 with a square hole of 4, joined to the outline by a cut along y = 3,
    // as KiCad saves fills, in KiCad's coordinates. The hole runs the other way round.
    let text = board(
        &["GND"],
        "(zone (net 1) (net_name \"GND\") (layers \"F.Cu\" \"B.Cu\") (uuid \"z\")\n\
           (polygon (pts (xy 0 0) (xy 10 0) (xy 10 10) (xy 0 10)))\n\
           (filled_polygon (layer \"B.Cu\") (pts (xy 0 0) (xy 10 0) (xy 10 3) (xy 7 3) (xy 3 3) (xy 3 7) (xy 7 7) (xy 7 3) (xy 10 3) (xy 10 10) (xy 0 10))))\n",
    );
    let read = read(&text, None).unwrap();
    assert_eq!(read.board.zones.len(), 1);
    let z = &read.board.zones[0];
    assert_eq!((z.layer, z.net), (LayerId(1), Some(NetId(1))));
    assert_eq!(read.zones[0].description, "zone of GND on B.Cu");
    assert_eq!(gap_to(&z.shape, 5.0, 5.0), 2_000_000.0);
    assert_eq!(gap_to(&z.shape, 1.0, 5.0), 0.0);
}

#[test]
fn graphics_and_warnings() {
    let text = board(
        &[],
        "(gr_line (start 0 0) (end 1 0) (layer \"F.Cu\") (stroke (width 0.2) (type solid)) (uuid \"g1\"))\n\
         (gr_rect (start 0 2) (end 1 3) (layer \"B.Cu\") (stroke (width 0) (type solid)) (fill solid) (uuid \"g2\"))\n\
         (gr_circle (center 5 5) (end 6 5) (layer \"F.Cu\") (stroke (width 0.1) (type solid)) (fill none))\n\
         (gr_line (start 0 0) (end 1 0) (layer \"Edge.Cuts\") (stroke (width 0.1) (type solid)))\n\
         (gr_text \"hi\" (at 0 0) (layer \"F.Cu\"))\n\
         (gr_curve (pts (xy 0 0) (xy 1 1) (xy 2 1) (xy 3 0)) (layer \"F.Cu\") (stroke (width 0.1) (type solid)))\n",
    );
    let read = read(&text, None).unwrap();
    assert_eq!(read.board.zones.len(), 3);
    assert_eq!(read.zones[0].description, "graphic on F.Cu");
    assert_eq!(
        read.board.zones[1].shape.bbox(),
        kicad_rect(0.0, 2.0, 1.0, 3.0)
    );
    assert!(near(
        read.board.zones[2].shape.bbox(),
        kicad_rect(3.95, 3.95, 6.05, 6.05),
        20
    ));
    let warnings: Vec<&str> = read.warnings.iter().map(|(w, _)| w.as_str()).collect();
    assert_eq!(
        warnings,
        [
            "Bézier curve in a graphic, skipped",
            "text on copper, skipped"
        ]
    );
}

#[test]
fn nets_spread_through_copper_as_in_kicad() {
    // A track with no net from a pad of A, a track of B touching it, and a track touching
    // pads of A and B, which is left alone.
    let text = board(
        &["A", "B"],
        "(footprint \"X\" (layer \"F.Cu\") (at 0 0)\n\
           (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\") (net 1 \"A\"))\n\
           (pad \"2\" smd rect (at 10 0) (size 1 1) (layers \"F.Cu\") (net 2 \"B\")))\n\
         (segment (start 0 0) (end 3 0) (width 0.2) (layer \"F.Cu\") (net 0))\n\
         (segment (start 3 0) (end 3 3) (width 0.2) (layer \"F.Cu\") (net 2))\n\
         (segment (start 0 0) (end 10 0) (width 0.2) (layer \"B.Cu\") (net 0))\n\
         (segment (start 0 0) (end 10 0) (width 0.2) (layer \"F.Cu\") (net 0))\n",
    );
    let read = read(&text, None).unwrap();
    let nets: Vec<_> = read.board.tracks.iter().map(|t| t.net).collect();
    // The last track joins A and B, so all of its cluster keeps its nets.
    assert_eq!(
        nets,
        [
            Some(NetId(0)),
            Some(NetId(2)),
            Some(NetId(0)),
            Some(NetId(0))
        ]
    );
    let text = text.replace(
        "(segment (start 0 0) (end 10 0) (width 0.2) (layer \"F.Cu\") (net 0))\n",
        "",
    );
    let read = super::read(&text, None).unwrap();
    let nets: Vec<_> = read.board.tracks.iter().map(|t| t.net).collect();
    assert_eq!(nets, [Some(NetId(1)), Some(NetId(1)), Some(NetId(0))]);
    assert_eq!(read.warnings[0].1, 2);
}

#[test]
fn checks_a_board_end_to_end() {
    // Two pads of no net, 0.1 mm apart: KiCad checks those against each other. Two
    // tracks of no net, overlapping: KiCad doesn't.
    let text = board(
        &[],
        "(footprint \"X\" (layer \"F.Cu\") (at 0 0)\n\
           (pad \"\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\"))\n\
           (pad \"\" smd rect (at 1.1 0) (size 1 1) (layers \"F.Cu\")))\n\
         (segment (start 0 5) (end 3 5) (width 0.2) (layer \"F.Cu\") (net 0))\n\
         (segment (start 0 5.1) (end 3 5.1) (width 0.2) (layer \"F.Cu\") (net 0))\n",
    );
    let read = read(&text, None).unwrap();
    let found = drc::check(&read.board, &read.rules).unwrap();
    assert!(
        matches!(
            found[..],
            [Violation::Clearance {
                a: Item::Pad(0),
                b: Item::Pad(1),
                ..
            }]
        ),
        "{found:?}"
    );
}

#[test]
fn rejects_what_is_not_a_board() {
    assert!(read("(kicad_sch (version 1))", None).is_err());
    assert_eq!(
        read("(kicad_pcb (version", None).unwrap_err().line(),
        Some(1)
    );
    let no_copper = "(kicad_pcb (layers (44 \"Edge.Cuts\" user)))";
    assert!(read(no_copper, None).is_err());
    assert!(read("(kicad_pcb (layers (0 \"F.Cu\" signal)))", Some("{")).is_err());
}
