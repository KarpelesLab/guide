//! Tests of PCB routing on small boards.

use super::{Design, MaskOpening, NetSpec, RouteOptions, Routed, ViaSize, route, unconnected};
use crate::geom::drc::{self, Board, Item, Keepout, Pad, Track, Violation};
use crate::geom::{LayerId, LayerSet, Point, Rect, RuleSpec, Rules, Shape};
use crate::id::NetId;

pub(super) const MM: i64 = 1_000_000;

pub(super) fn rules(layers: usize) -> Rules {
    let names: Vec<String> = (0..layers).map(|l| format!("L{l}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut spec = RuleSpec::new(&names, 150_000, 200_000);
    spec.hole_to_hole = 250_000;
    spec.hole_clearance = 200_000;
    Rules::new(spec).unwrap()
}

pub(super) fn smd(x: i64, y: i64, net: u32) -> Pad {
    Pad {
        shape: Shape::rect(Rect::new(
            Point::new(x - 300_000, y - 300_000),
            Point::new(x + 300_000, y + 300_000),
        ))
        .unwrap(),
        layers: LayerSet::EMPTY.with(LayerId(0)),
        hole: None,
        net: Some(NetId(net)),
        clearance: None,
    }
}

pub(super) fn through(x: i64, y: i64, net: u32, layers: usize) -> Pad {
    Pad {
        shape: Shape::circle(Point::new(x, y), 1_200_000).unwrap(),
        layers: LayerSet::first(layers),
        hole: Some(Shape::circle(Point::new(x, y), 600_000).unwrap()),
        net: Some(NetId(net)),
        clearance: None,
    }
}

pub(super) fn design(layers: usize, pads: Vec<Pad>, nets: usize) -> Design {
    let spec = NetSpec {
        route: true,
        track_width: 200_000,
        via: Some(ViaSize {
            diameter: 600_000,
            drill: 300_000,
        }),
        layers: None,
    };
    Design {
        board: Board {
            pads,
            ..Board::default()
        },
        rules: rules(layers),
        outline: Vec::new(),
        edge_clearance: 0,
        nets: vec![spec; nets],
        obstacles: Vec::new(),
        pairs: Vec::new(),
        lengths: Vec::new(),
        zones: Default::default(),
    }
}

/// Routes, and checks that what is routed breaks no rule and connects its nets.
pub(super) fn route_checked(design: &Design, options: &RouteOptions) -> Routed {
    let routed = route(design, options).unwrap();
    let board = routed.board(design);
    let found = drc::check(&board, &design.rules).unwrap();
    assert!(found.is_empty(), "{found:?}");
    assert_eq!(unconnected(&board, &design.rules), routed.unrouted);
    routed
}

/// A rectangle's outline as four thin lines.
pub(super) fn outline(r: Rect) -> Vec<Shape> {
    let (a, c) = (r.min(), r.max());
    let corners = [a, Point::new(c.x, a.y), c, Point::new(a.x, c.y)];
    (0..4)
        .map(|i| Shape::track(corners[i], corners[(i + 1) % 4], 1).unwrap())
        .collect()
}

#[test]
fn crossing_nets_take_a_via_on_two_layers() {
    let d = design(
        2,
        vec![
            smd(0, 5 * MM, 1),
            smd(10 * MM, 5 * MM, 1),
            smd(5 * MM, 0, 2),
            smd(5 * MM, 10 * MM, 2),
        ],
        3,
    );
    let mut d = d;
    // Too tight for a track to go round the end of a pad.
    d.outline = outline(Rect::new(
        Point::new(-600_000, -600_000),
        Point::new(10_600_000, 10_600_000),
    ));
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    assert_eq!(routed.stats.routed, 2);
    assert!(!routed.vias.is_empty());
    // Not far from straight across, for both.
    assert!(routed.stats.after_cleanup.1 < 23.0 * MM as f64);
}

#[test]
fn on_one_layer_one_crossing_net_is_left_unrouted() {
    let d = design(
        1,
        vec![
            smd(0, 5 * MM, 1),
            smd(10 * MM, 5 * MM, 1),
            smd(5 * MM, 0, 2),
            smd(5 * MM, 10 * MM, 2),
        ],
        3,
    );
    let mut d = d;
    // Tight enough that no track fits round the end of a pad.
    d.outline = outline(Rect::new(
        Point::new(-600_000, -600_000),
        Point::new(10_600_000, 10_600_000),
    ));
    let routed = route_checked(&d, &RouteOptions::default());
    assert_eq!(routed.unrouted.len(), 1);
    assert!(routed.vias.is_empty());
}

#[test]
fn routes_keep_out_of_keepouts_and_away_from_the_edge() {
    let mut d = design(1, vec![smd(0, 0, 1), smd(10 * MM, 0, 1)], 2);
    // A wall across the straight line, with a way round above it.
    d.board.keepouts.push(Keepout {
        shape: Shape::rect(Rect::new(
            Point::new(4 * MM, -3 * MM),
            Point::new(6 * MM, 2 * MM),
        ))
        .unwrap(),
        layers: LayerSet::first(1),
        tracks: true,
        vias: true,
        pads: false,
        except: Vec::new(),
    });
    d.outline = outline(Rect::new(
        Point::new(-2 * MM, -3 * MM),
        Point::new(12 * MM, 4 * MM),
    ));
    d.edge_clearance = 500_000;
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    for t in &routed.tracks {
        let shape = Shape::track(t.start, t.end, t.width).unwrap();
        for e in &d.outline {
            assert!(!shape.violates(e, d.edge_clearance).unwrap(), "{t:?}");
        }
    }
    // Round the wall, so longer than straight.
    assert!(routed.stats.after_cleanup.1 > 10.5 * MM as f64);
}

#[test]
fn a_net_with_no_way_out_is_unrouted_without_breaking_rules() {
    let mut d = design(1, vec![smd(0, 0, 1), smd(10 * MM, 0, 1)], 2);
    d.board.keepouts.push(Keepout {
        shape: Shape::rect(Rect::new(
            Point::new(4 * MM, -50 * MM),
            Point::new(6 * MM, 50 * MM),
        ))
        .unwrap(),
        layers: LayerSet::first(1),
        tracks: true,
        vias: true,
        pads: false,
        except: Vec::new(),
    });
    d.outline = outline(Rect::new(
        Point::new(-2 * MM, -10 * MM),
        Point::new(12 * MM, 10 * MM),
    ));
    let routed = route_checked(&d, &RouteOptions::default());
    assert_eq!(routed.unrouted, [NetId(1)]);
    assert!(routed.tracks.is_empty());
}

#[test]
fn fixed_routes_are_obstacles_or_part_of_their_net() {
    let mut d = design(1, vec![smd(0, 0, 1), smd(10 * MM, 0, 1)], 4);
    d.outline = outline(Rect::new(
        Point::new(-MM, -4 * MM),
        Point::new(11 * MM, 4 * MM),
    ));
    let track = |a: (i64, i64), b: (i64, i64), net| Track {
        start: Point::new(a.0, a.1),
        end: Point::new(b.0, b.1),
        mid: None,
        width: 200_000,
        layer: LayerId(0),
        net: Some(NetId(net)),
    };
    // Half of net 1 is routed already, and a track of net 3 is in the way.
    d.board.tracks.push(track((0, 0), (4 * MM, 0), 1));
    d.board
        .tracks
        .push(track((6 * MM, -3 * MM), (6 * MM, 3 * MM), 3));
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    // The route goes on from the end of the fixed half, and round net 3's track.
    assert!(
        routed
            .tracks
            .iter()
            .all(|t| t.start != Point::new(0, 0) && t.end != Point::new(0, 0)),
        "{:?}",
        routed.tracks
    );
    assert!(routed.stats.after_cleanup.1 > 6.0 * MM as f64);
}

#[test]
fn net_classes_set_widths_and_clearances() {
    let mut d = design(
        2,
        vec![
            smd(0, 0, 1),
            smd(10 * MM, 0, 1),
            smd(0, 2 * MM, 2),
            smd(10 * MM, 2 * MM, 2),
        ],
        3,
    );
    d.nets[2].track_width = 500_000;
    let mut spec = d.rules.spec().clone();
    spec.classes.push(crate::geom::NetClass {
        name: "wide".into(),
        nets: vec![NetId(2)],
        overrides: crate::geom::Overrides {
            clearance: Some(400_000),
            track_width: None,
        },
        ..Default::default()
    });
    d.rules = Rules::new(spec).unwrap();
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    for t in &routed.tracks {
        let want = if t.net == Some(NetId(2)) {
            500_000
        } else {
            200_000
        };
        assert_eq!(t.width, want);
    }
}

#[test]
fn through_hole_pads_connect_their_layers() {
    // Net 2 blocks the top layer between net 1's pads, which are through-hole.
    let mut pads = vec![through(0, 0, 1, 2), through(10 * MM, 0, 1, 2)];
    pads.push(Pad {
        shape: Shape::rect(Rect::new(
            Point::new(4 * MM, -20 * MM),
            Point::new(6 * MM, 20 * MM),
        ))
        .unwrap(),
        layers: LayerSet::EMPTY.with(LayerId(0)),
        hole: None,
        net: Some(NetId(2)),
        clearance: None,
    });
    let mut d = design(2, pads, 3);
    d.nets[2].route = false;
    d.outline = outline(Rect::new(
        Point::new(-3 * MM, -10 * MM),
        Point::new(13 * MM, 10 * MM),
    ));
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    assert!(routed.vias.is_empty());
    assert!(routed.tracks.iter().all(|t| t.layer == LayerId(1)));
}

#[test]
fn routes_stay_out_of_other_nets_mask_openings() {
    let mut d = design(
        1,
        vec![smd(0, 0, 1), smd(10 * MM, 0, 1), smd(5 * MM, 3 * MM, 2)],
        3,
    );
    d.nets[2].route = false;
    // Net 2's pad has a wide mask opening, reaching across the straight line.
    d.board.masks.push(MaskOpening {
        shape: d.board.pads[2].shape.clone(),
        layer: LayerId(0),
        margin: 3 * MM,
        net: Some(NetId(2)),
    });
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    for t in &routed.tracks {
        let shape = Shape::track(t.start, t.end, t.width).unwrap();
        assert!(!shape.violates(&d.board.pads[2].shape, 3 * MM).unwrap());
    }
}

#[test]
fn routing_is_deterministic() {
    let pads = (0..6)
        .flat_map(|i| {
            [
                smd(0, i * 2 * MM, i as u32 + 1),
                smd(12 * MM, (5 - i) * 2 * MM, i as u32 + 1),
            ]
        })
        .collect();
    let d = design(2, pads, 7);
    let a = route_checked(&d, &RouteOptions::default());
    let b = route_checked(&d, &RouteOptions::default());
    assert!(a.unrouted.is_empty());
    assert_eq!(a.tracks, b.tracks);
    assert_eq!(a.vias, b.vias);
    // And the same without global routing, which only changes where nets may go.
    let options = RouteOptions {
        global: false,
        ..RouteOptions::default()
    };
    let c = route_checked(&d, &options);
    assert!(c.unrouted.is_empty());
}

#[test]
fn cleanup_never_lengthens_tracks_or_adds_vias_and_breaks_nothing() {
    let pads = (0..6)
        .flat_map(|i| {
            [
                smd(0, i * 2 * MM, i as u32 + 1),
                smd(12 * MM, (5 - i) * 2 * MM, i as u32 + 1),
            ]
        })
        .collect();
    let d = design(2, pads, 7);
    let options = RouteOptions {
        cleanup: false,
        ..RouteOptions::default()
    };
    let raw = route_checked(&d, &options);
    let clean = route_checked(&d, &RouteOptions::default());
    assert_eq!(raw.stats.before_cleanup, clean.stats.before_cleanup);
    assert!(clean.stats.after_cleanup.0 <= raw.stats.after_cleanup.0);
    assert!(clean.vias.len() <= raw.vias.len());
}

#[test]
fn hole_to_hole_holds_between_a_nets_own_vias() {
    // Many nets crossing on two layers make vias, of one net and of several.
    let mut pads = Vec::new();
    for i in 0..4 {
        pads.push(smd(0, i * MM + MM, i as u32 + 1));
        pads.push(smd(8 * MM, i * MM + MM, i as u32 + 1));
        pads.push(smd(i * MM + 2 * MM, 0, i as u32 + 5));
        pads.push(smd(i * MM + 2 * MM, 6 * MM, i as u32 + 5));
    }
    let d = design(2, pads, 9);
    let routed = route_checked(&d, &RouteOptions::default());
    let board = routed.board(&d);
    let found = drc::check(&board, &d.rules).unwrap();
    assert!(
        !found.iter().any(|v| matches!(
            v,
            Violation::HoleToHole {
                a: Item::Via(_),
                ..
            }
        )),
        "{found:?}"
    );
}

#[test]
fn pads_trapped_in_an_area_array_escape_through_vias() {
    // A ball grid of 5 by 5 at a pitch of 1 mm, with balls of 0.45 mm: no track of 0.2 mm
    // with 0.2 mm on either side passes between them. The 8 balls round the centre each
    // connect to a pad outside it; the others are of nets with nothing else.
    let ball = |x: i64, y: i64, net: u32| Pad {
        shape: Shape::circle(Point::new(x, y), 450_000).unwrap(),
        ..smd(x, y, net)
    };
    let mut pads = Vec::new();
    let mut net = 1;
    let mut lone = 9;
    for i in 0..5 {
        for j in 0..5 {
            let (x, y) = (i * MM, j * MM);
            let ring = (1..=3).contains(&i) && (1..=3).contains(&j) && (i, j) != (2, 2);
            if ring {
                pads.push(ball(x, y, net));
                pads.push(smd(10 * MM, i64::from(net) * 1_500_000 - 4 * MM, net));
                net += 1;
            } else {
                lone += 1;
                pads.push(ball(x, y, lone));
            }
        }
    }
    let d = design(2, pads, lone as usize + 1);
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty(), "{:?}", routed.unrouted);
    // Each of the 8 needs a via to get out.
    for n in 1..9 {
        assert!(
            routed.vias.iter().any(|v| v.net == Some(NetId(n))),
            "net {n}"
        );
    }
}

#[test]
fn a_pad_whose_centre_is_blocked_is_reached_where_it_has_room() {
    let mut d = design(1, vec![smd(0, 0, 1), smd(5 * MM, 0, 1)], 2);
    // Copper of no net, as text can be, over the left edge of the first pad, too near its
    // centre for a track to end there.
    let text = Shape::rect(Rect::new(
        Point::new(-2 * MM, -2 * MM),
        Point::new(-250_000, 2 * MM),
    ))
    .unwrap();
    d.obstacles.push((text.clone(), LayerId(0)));
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    for t in &routed.tracks {
        let shape = Shape::track(t.start, t.end, t.width).unwrap();
        assert!(!shape.violates(&text, 200_000).unwrap());
    }
}

#[test]
fn a_net_of_many_pads_keeps_what_it_reaches() {
    // Twenty pads of one net in a row on one layer, the last walled in.
    let pads: Vec<Pad> = (0..20).map(|k| smd(k * 2 * MM, 0, 1)).collect();
    let mut d = design(1, pads, 2);
    let x = 38 * MM;
    let wall = |a: Point, b: Point| Keepout {
        shape: Shape::rect(Rect::new(a, b)).unwrap(),
        layers: LayerSet::first(1),
        tracks: true,
        vias: true,
        pads: false,
        except: Vec::new(),
    };
    d.board.keepouts.extend([
        wall(Point::new(x - MM, -MM), Point::new(x + MM, -MM / 2)),
        wall(Point::new(x - MM, MM / 2), Point::new(x + MM, MM)),
        wall(Point::new(x - MM, -MM), Point::new(x - MM / 2, MM)),
        wall(Point::new(x + MM / 2, -MM), Point::new(x + MM, MM)),
    ]);
    d.outline = outline(Rect::new(
        Point::new(-2 * MM, -3 * MM),
        Point::new(41 * MM, 3 * MM),
    ));
    let routed = route_checked(&d, &RouteOptions::default());
    assert_eq!(routed.unrouted, [NetId(1)]);
    assert_eq!(routed.stats.connections, 19);
    assert_eq!(routed.stats.connected, 18);
    assert!(!routed.tracks.is_empty());
}
