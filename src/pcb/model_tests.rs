//! Tests of what a host says of its board beyond pads and nets: where vias may be, zones
//! filled by the host, zones that are planes or not, keepouts' exceptions, layers, pad
//! stacks, net ties and rules by area.

use super::tests::{MM, design, outline, route_checked, smd};
use super::zone_tests::{check_fills, plane_board, zone};
use super::{
    CopperZone, Design, Fill, Islands, MaskOpening, RouteOptions, Routed, ZoneConnection,
    fill_zones, unconnected,
};
use crate::geom::drc::{self, Item, Keepout, NetTie, Pad, Track, Via, Violation};
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::NetId;

/// The vias of a result that touch a pad with no hole.
fn vias_in_pads(design: &Design, routed: &Routed) -> usize {
    routed
        .vias
        .iter()
        .filter(|v| {
            let copper = Shape::circle(v.at, v.diameter).unwrap();
            design
                .board
                .pads
                .iter()
                .any(|p| p.hole.is_none() && p.shape.touches(&copper))
        })
        .count()
}

/// A 5 by 5 ball grid at a pitch of 1 mm, whose 8 balls round the centre each connect to a
/// pad outside it, as in `pads_trapped_in_an_area_array_escape_through_vias`.
fn ball_grid() -> Design {
    let ball = |x: i64, y: i64, net: u32| Pad {
        shape: Shape::circle(Point::new(x, y), 450_000).unwrap(),
        ..smd(x, y, net)
    };
    let mut pads = Vec::new();
    let (mut net, mut lone) = (1, 9);
    for i in 0..5 {
        for j in 0..5 {
            let (x, y) = (i * MM, j * MM);
            if (1..=3).contains(&i) && (1..=3).contains(&j) && (i, j) != (2, 2) {
                pads.push(ball(x, y, net));
                pads.push(smd(10 * MM, i64::from(net) * 1_500_000 - 4 * MM, net));
                net += 1;
            } else {
                lone += 1;
                pads.push(ball(x, y, lone));
            }
        }
    }
    design(2, pads, lone as usize + 1)
}

#[test]
fn without_vias_in_pads_a_pad_gets_its_via_beside_it() {
    // A pad on the top and one on the bottom, one above the other but for a millimetre.
    let mut d = design(2, vec![smd(0, 0, 1), smd(MM, 0, 1)], 2);
    d.board.pads[1].layers = LayerSet::EMPTY.with(LayerId(1));
    let with = route_checked(&d, &RouteOptions::default());
    assert!(with.unrouted.is_empty());
    assert_eq!(vias_in_pads(&d, &with), 1, "{:?}", with.vias);

    let options = RouteOptions {
        vias_in_pads: false,
        ..RouteOptions::default()
    };
    let without = route_checked(&d, &options);
    assert!(without.unrouted.is_empty());
    assert_eq!(without.vias.len(), 1);
    assert_eq!(vias_in_pads(&d, &without), 0, "{:?}", without.vias);
}

#[test]
fn without_vias_in_pads_an_area_array_escapes_through_dog_bones() {
    let d = ball_grid();
    let with = route_checked(&d, &RouteOptions::default());
    assert!(with.unrouted.is_empty());
    assert!(vias_in_pads(&d, &with) > 0);

    let options = RouteOptions {
        vias_in_pads: false,
        ..RouteOptions::default()
    };
    let without = route_checked(&d, &options);
    // As many nets routed, each of the 8 through a via that is in no pad.
    assert!(without.unrouted.is_empty(), "{:?}", without.unrouted);
    assert_eq!(vias_in_pads(&d, &without), 0);
    for n in 1..9 {
        assert!(
            without.vias.iter().any(|v| v.net == Some(NetId(n))),
            "net {n}"
        );
    }
}

/// A rectangle, in millimetres, as a ring.
fn ring(x0: i64, y0: i64, x1: i64, y1: i64) -> Vec<Point> {
    vec![
        Point::new(x0, y0),
        Point::new(x1, y0),
        Point::new(x1, y1),
        Point::new(x0, y1),
    ]
}

/// Whether a point is in a fill.
fn filled(fills: &[Fill], x: i64, y: i64) -> bool {
    let dot = Shape::circle(Point::new(x, y), 2).unwrap();
    fills.iter().any(|f| f.shape.touches(&dot))
}

#[test]
fn a_host_s_fill_is_the_plane_that_routing_plans_on() {
    // The plane of `plane_board`, as a host filled it: with a hole of 4 mm round the pad in
    // the middle, which the fill made here joins through its spokes.
    let mut d = plane_board();
    let host = Shape::polygon(
        &ring(300_000, 300_000, 19_700_000, 19_700_000),
        &[&ring(8 * MM, 8 * MM, 12 * MM, 12 * MM)],
    )
    .unwrap();
    let own = route_checked(&d, &RouteOptions::default());
    assert!(filled(&own.fills, 10 * MM + 900_000, 10 * MM));
    let reaches_middle = |r: &Routed| {
        let pad = &d.board.pads[6].shape;
        r.tracks.iter().any(|t| {
            t.net == Some(NetId(1)) && Shape::track(t.start, t.end, t.width).unwrap().touches(pad)
        })
    };
    assert!(!reaches_middle(&own));

    d.zones.fills.push(Fill {
        zone: 0,
        layer: LayerId(1),
        shape: host.clone(),
    });
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    // The fill is the host's, as given, and the pad in the middle has a track to it.
    assert_eq!(routed.fills.len(), 1);
    assert_eq!(routed.fills[0].shape, host);
    assert_eq!(routed.fills[0].shape.rings().unwrap().len(), 2);
    assert!(reaches_middle(&routed));
    assert_eq!(routed.stats.connections, 4 + 1);
    assert_eq!(routed.stats.connected, 4 + 1);
    // On the board with the host's fill as its copper, every net is one piece, and without
    // what was routed the pad in the middle is not.
    let zone = drc::Zone {
        shape: host,
        layer: LayerId(1),
        net: Some(NetId(1)),
        clearance: None,
    };
    let mut board = d.board.clone();
    board.zones.push(zone);
    assert_eq!(unconnected(&board, &d.rules), [NetId(1), NetId(2)]);
    board.tracks.extend(routed.tracks.iter().cloned());
    board.vias.extend(routed.vias.iter().cloned());
    assert!(unconnected(&board, &d.rules).is_empty());
    assert!(drc::check(&board, &d.rules).unwrap().is_empty());
}

#[test]
fn a_host_s_fill_is_cut_around_copper_it_does_not_clear() {
    // Two squares joined by a neck of 0.15 mm, narrower than the zone's minimum width, as a
    // host's spoke may be.
    let mut d = design(2, vec![smd(20 * MM, 20 * MM, 2)], 3);
    let mut z = zone(1, 0, 15 * MM);
    z.islands = Islands::Keep;
    d.zones.zones.push(z);
    let given = [
        ring(0, 0, 10 * MM, 10 * MM),
        ring(10 * MM, 4_900_000, 11 * MM, 5_050_000),
        ring(11 * MM, 0, 15 * MM, 10 * MM),
    ];
    let host = Shape::region(&[&given[0], &given[1], &given[2]]).unwrap();
    assert_eq!(host.len(), 1);
    d.zones.fills.push(Fill {
        zone: 0,
        layer: LayerId(0),
        shape: host[0].clone(),
    });
    // Nothing near it: as given.
    let fills = fill_zones(&d.board, &d).unwrap();
    assert_eq!(fills, d.zones.fills);
    // A track of another net on the other layer changes nothing either.
    let track = |x: i64, layer: u8| Track {
        start: Point::new(x, -MM),
        end: Point::new(x, 11 * MM),
        mid: None,
        width: 200_000,
        layer: LayerId(layer),
        net: Some(NetId(2)),
    };
    let mut board = d.board.clone();
    board.tracks.push(track(3 * MM, 1));
    assert_eq!(fill_zones(&board, &d).unwrap(), d.zones.fills);
    // On its layer, the track cuts the first square in two, 0.3 mm from the track on
    // either side, and the neck stays.
    board.tracks[0] = track(3 * MM, 0);
    let fills = fill_zones(&board, &d).unwrap();
    check_fills(&d, &board, &fills);
    assert_eq!(fills.len(), 2);
    assert!(filled(&fills[..1], MM, 5 * MM) != filled(&fills[1..], MM, 5 * MM));
    let right = if filled(&fills[..1], MM, 5 * MM) {
        &fills[1..]
    } else {
        &fills[..1]
    };
    assert!(filled(right, 5 * MM, 5 * MM));
    assert!(filled(right, 10_500_000, 5 * MM));
    assert!(filled(right, 13 * MM, 5 * MM));
    assert!(!filled(&fills, 3 * MM + 390_000, 5 * MM));
    assert!(filled(&fills, 3 * MM + 410_000, 5 * MM));
    // A track that leaves a strip of 0.05 mm at the square's edge: the strip goes.
    board.tracks[0] = track(450_000, 0);
    let fills = fill_zones(&board, &d).unwrap();
    check_fills(&d, &board, &fills);
    assert_eq!(fills.len(), 1);
    assert!(!filled(&fills, 25_000, 5 * MM));
    assert!(filled(&fills, 13 * MM, 5 * MM));
    // And with islands taken away, a piece that no copper of the net touches goes too.
    d.zones.zones[0].islands = Islands::Remove;
    d.board.pads.push(smd(13 * MM, 5 * MM, 1));
    board.pads.push(smd(13 * MM, 5 * MM, 1));
    board.tracks[0] = track(3 * MM, 0);
    let fills = fill_zones(&board, &d).unwrap();
    assert_eq!(fills.len(), 1);
    assert!(!filled(&fills, MM, 5 * MM));
}

#[test]
fn a_zone_sets_how_many_spokes_its_reliefs_have_and_where_the_first_is() {
    let mut d = design(1, vec![smd(5 * MM, 5 * MM, 1)], 2);
    d.zones.zones.push(zone(1, 0, 10 * MM));
    // In the gap of 0.4 mm round the pad of 0.6 mm: to the right, above, and off a corner.
    let (c, mid) = (5 * MM, 500_000);
    let spokes = |d: &Design| {
        let fills = fill_zones(&d.board, d).unwrap();
        check_fills(d, &d.board, &fills);
        [
            filled(&fills, c + mid, c),
            filled(&fills, c - mid, c),
            filled(&fills, c, c + mid),
            filled(&fills, c + 440_000, c + 440_000),
        ]
    };
    assert_eq!(spokes(&d), [true, true, true, false]);
    d.zones.zones[0].spokes = 4;
    assert_eq!(spokes(&d), [true, true, true, false]);
    d.zones.zones[0].spokes = 2;
    assert_eq!(spokes(&d), [true, true, false, false]);
    d.zones.zones[0].spoke_direction = Some((0, 3));
    assert_eq!(spokes(&d), [false, false, true, false]);
    d.zones.zones[0].spokes = 1;
    d.zones.zones[0].spoke_direction = Some((-1, 0));
    assert_eq!(spokes(&d), [false, true, false, false]);
    d.zones.zones[0].spokes = 8;
    assert_eq!(spokes(&d), [true, true, true, true]);
    // A pad's own direction comes before its zone's.
    d.zones.zones[0].spokes = 2;
    d.zones.pads.push(super::PadConnection {
        spoke_direction: Some((0, 1)),
        ..Default::default()
    });
    assert_eq!(spokes(&d), [false, false, true, false]);
}

/// On one layer: two pads of net 1 in a zone of theirs across the middle of the board, which
/// joins them solidly, and two pads of net 2 on either side of it.
fn wall_board() -> Design {
    let mut d = design(
        1,
        vec![
            smd(10 * MM, 0, 1),
            smd(10 * MM, 10 * MM, 1),
            smd(0, 5 * MM, 2),
            smd(20 * MM, 5 * MM, 2),
        ],
        4,
    );
    d.zones.zones.push(CopperZone {
        outline: ring(8 * MM, -2 * MM, 12 * MM, 12 * MM),
        connection: ZoneConnection::Solid,
        ..zone(1, 0, 0)
    });
    d
}

fn length_of(routed: &Routed, net: u32) -> i64 {
    routed
        .tracks
        .iter()
        .filter(|t| t.net == Some(NetId(net)))
        .map(|t| {
            let (dx, dy) = ((t.end.x - t.start.x) as f64, (t.end.y - t.start.y) as f64);
            (dx * dx + dy * dy).sqrt() as i64
        })
        .sum()
}

#[test]
fn a_zone_that_is_not_a_plane_is_copper_that_routes_keep_clear_of() {
    let mut d = wall_board();
    // As a plane, the zone gives way to the other net, which goes straight through it.
    let through = super::route(&d, &RouteOptions::default()).unwrap();
    assert!(length_of(&through, 2) < 21 * MM);

    d.zones.zones[0].plane = Some(false);
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    // The zone's pads need no track, and it is filled as before routing, in one piece.
    assert_eq!(length_of(&routed, 1), 0);
    assert_eq!(routed.stats.stubs, 0);
    assert_eq!(routed.fills.len(), 1);
    assert!(filled(&routed.fills, 10 * MM, -MM) && filled(&routed.fills, 10 * MM, 11 * MM));
    // The other net goes round it.
    assert!(length_of(&routed, 2) > 24 * MM, "{}", length_of(&routed, 2));
    // One connection routed, and one that the zone makes.
    assert_eq!(routed.stats.connections, 2);
    assert_eq!(routed.stats.connected, 2);
}

#[test]
fn zones_are_planes_by_how_much_of_their_nets_they_cover() {
    use super::router::plane_zones;
    let mut d = wall_board();
    // A net across the board with a pour of a millimetre round one of its pads.
    d.board.pads.push(smd(0, -20 * MM, 3));
    d.board.pads.push(smd(30 * MM, -20 * MM, 3));
    d.zones.zones.push(CopperZone {
        outline: ring(-500_000, -20 * MM - 500_000, 500_000, -20 * MM + 500_000),
        connection: ZoneConnection::Solid,
        ..zone(3, 0, 0)
    });
    let planes = |d: &Design, coverage: f64| {
        let options = RouteOptions {
            plane_coverage: coverage,
            ..RouteOptions::default()
        };
        plane_zones(d, &options).unwrap()
    };
    assert_eq!(planes(&d, 0.0), [true, true]);
    assert_eq!(planes(&d, 0.25), [true, false]);
    // A zone says so itself, whatever it covers.
    d.zones.zones[1].plane = Some(true);
    d.zones.zones[0].plane = Some(false);
    assert_eq!(planes(&d, 0.25), [false, true]);
    assert_eq!(planes(&d, 0.0), [false, true]);

    // Routed so, the pour is copper of its net, and the net a track from one pad to the
    // other, with no stub to the pour.
    d.zones.zones[1].plane = None;
    d.zones.zones[0].plane = None;
    let options = RouteOptions {
        plane_coverage: 0.25,
        ..RouteOptions::default()
    };
    let routed = route_checked(&d, &options);
    assert_eq!(routed.unrouted, [NetId(1)]);
    assert!(length_of(&routed, 3) > 29 * MM);
}

#[test]
fn a_keepout_lets_the_nets_it_excepts_through() {
    // Two nets side by side, and a keepout from edge to edge of the board between their
    // pads, which excepts the first.
    let mut d = design(
        1,
        vec![
            smd(0, 0, 1),
            smd(10 * MM, 0, 1),
            smd(0, 2 * MM, 2),
            smd(10 * MM, 2 * MM, 2),
        ],
        3,
    );
    d.outline = outline(Rect::new(
        Point::new(-2 * MM, -2 * MM),
        Point::new(12 * MM, 4 * MM),
    ));
    d.board.keepouts.push(Keepout {
        shape: Shape::rect(Rect::new(
            Point::new(4 * MM, -3 * MM),
            Point::new(6 * MM, 5 * MM),
        ))
        .unwrap(),
        layers: LayerSet::first(1),
        tracks: true,
        vias: true,
        pads: false,
        except: vec![NetId(1)],
    });
    let routed = route_checked(&d, &RouteOptions::default());
    assert_eq!(routed.unrouted, [NetId(2)]);
    assert!(length_of(&routed, 1) >= 10 * MM - 1);

    // The check lets the excepted net's track be, and reports the same track of another.
    let mut board = routed.board(&d);
    assert!(drc::check(&board, &d.rules).unwrap().is_empty());
    for t in &mut board.tracks {
        t.net = Some(NetId(3));
    }
    let found = drc::check(&board, &d.rules).unwrap();
    assert!(
        found
            .iter()
            .any(|v| matches!(v, Violation::Keepout { keepout: 0, .. })),
        "{found:?}"
    );
    // With no exception, neither net is routed.
    d.board.keepouts[0].except.clear();
    let routed = route_checked(&d, &RouteOptions::default());
    assert_eq!(routed.unrouted, [NetId(1), NetId(2)]);
}

/// Whether every track of a net is on one of these layers.
fn tracks_on(routed: &Routed, net: u32, layers: LayerSet) -> bool {
    let mut of_net = routed.tracks.iter().filter(|t| t.net == Some(NetId(net)));
    of_net.all(|t| layers.contains(t.layer))
}

#[test]
fn a_run_and_a_net_keep_tracks_to_their_layers() {
    // Two nets that cross on a board too tight to go round a pad, as in
    // `crossing_nets_take_a_via_on_two_layers`.
    let mut d = design(
        2,
        vec![
            smd(0, 5 * MM, 1),
            smd(10 * MM, 5 * MM, 1),
            smd(5 * MM, 0, 2),
            smd(5 * MM, 10 * MM, 2),
        ],
        3,
    );
    d.outline = outline(Rect::new(
        Point::new(-600_000, -600_000),
        Point::new(10_600_000, 10_600_000),
    ));
    let top = LayerSet::EMPTY.with(LayerId(0));
    // Kept to the top, the run is as on one layer: a net is left.
    let options = RouteOptions {
        layers: Some(top),
        ..RouteOptions::default()
    };
    let routed = route_checked(&d, &options);
    assert_eq!(routed.unrouted.len(), 1);
    assert!(routed.tracks.iter().all(|t| t.layer == LayerId(0)));
    // With the second net alone kept to the top, the first is the one that changes layers.
    d.nets[2].layers = Some(top);
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    assert!(tracks_on(&routed, 2, top));
    assert!(!tracks_on(&routed, 1, top));
    assert!(routed.vias.iter().all(|v| v.net == Some(NetId(1))));
    // And the other way round.
    d.nets[2].layers = None;
    d.nets[1].layers = Some(top);
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    assert!(tracks_on(&routed, 1, top));
    assert!(!tracks_on(&routed, 2, top));
}

#[test]
fn a_net_kept_to_an_inner_layer_leaves_its_pads_through_vias_in_them() {
    let mut d = design(4, vec![smd(0, 0, 1), smd(10 * MM, 0, 1)], 2);
    let inner = LayerSet::EMPTY.with(LayerId(2));
    d.nets[1].layers = Some(inner);
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    assert!(!routed.tracks.is_empty());
    assert!(tracks_on(&routed, 1, inner));
    assert_eq!(routed.vias.len(), 2);
    assert_eq!(vias_in_pads(&d, &routed), 2);
    // With no via in a pad, no track may leave the pads, and the net is left.
    let options = RouteOptions {
        vias_in_pads: false,
        ..RouteOptions::default()
    };
    let routed = route_checked(&d, &options);
    assert_eq!(routed.unrouted, [NetId(1)]);
}

/// A through-hole pad at the origin of a board of four layers: 1.6 mm on the top, 0.9 mm on
/// the layers below, round a hole of 0.6 mm.
fn stacked(net: u32) -> Pad {
    let at = Point::new(0, 0);
    Pad {
        hole: Some(Shape::circle(at, 600_000).unwrap()),
        stack: vec![(
            LayerSet::span(LayerId(1), LayerId(3)),
            Shape::circle(at, 900_000).unwrap(),
        )],
        ..Pad::new(
            Shape::circle(at, 1_600_000).unwrap(),
            LayerSet::EMPTY.with(LayerId(0)),
            Some(NetId(net)),
        )
    }
}

#[test]
fn the_check_takes_a_pad_s_stack_layer_by_layer_and_reports_the_pad_once() {
    let mut d = design(4, vec![stacked(1)], 3);
    assert_eq!(d.board.pads[0].copper_layers(), LayerSet::first(4));
    // A track of another net 0.2 mm from the pad's copper below the top, which is inside
    // its copper on the top.
    let track = |layer: u8| Track {
        start: Point::new(-3 * MM, 750_000),
        end: Point::new(3 * MM, 750_000),
        mid: None,
        width: 200_000,
        layer: LayerId(layer),
        net: Some(NetId(2)),
    };
    d.board.tracks.push(track(1));
    assert!(drc::check(&d.board, &d.rules).unwrap().is_empty());
    d.board.tracks[0] = track(0);
    let found = drc::check(&d.board, &d.rules).unwrap();
    assert!(
        matches!(
            found[..],
            [Violation::Short {
                a: Item::Track(0),
                b: Item::Pad(0),
                layer: LayerId(0)
            }]
        ),
        "{found:?}"
    );
    // A via through every layer, too close to each of the pad's shapes: reported once, on
    // the first layer.
    d.board.tracks.clear();
    d.board.vias.push(Via {
        at: Point::new(1_250_000, 0),
        drill: 300_000,
        diameter: 600_000,
        from: LayerId(0),
        to: LayerId(3),
        net: Some(NetId(2)),
    });
    let found = drc::check(&d.board, &d.rules).unwrap();
    let of_pad = |v: &&Violation| {
        matches!(
            v,
            Violation::Clearance {
                b: Item::Pad(0),
                ..
            } | Violation::Short {
                b: Item::Pad(0),
                ..
            }
        )
    };
    assert_eq!(found.iter().filter(of_pad).count(), 1, "{found:?}");
    assert!(matches!(
        found.iter().find(of_pad),
        Some(Violation::Clearance {
            layer: LayerId(0),
            ..
        })
    ));
}

#[test]
fn a_pad_s_stack_is_one_terminal_that_joins_its_layers() {
    // The pad, a pad of its net on the bottom, and two pads of another net on the first
    // inner layer, either side of it, on a line that only the smaller copper there leaves
    // room for.
    let on = |pad: Pad, layer: u8| Pad {
        layers: LayerSet::EMPTY.with(LayerId(layer)),
        ..pad
    };
    let d = design(
        4,
        vec![
            stacked(1),
            on(smd(5 * MM, 0, 1), 3),
            on(smd(-3 * MM, 750_000, 2), 1),
            on(smd(3 * MM, 750_000, 2), 1),
        ],
        3,
    );
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    // The pad's hole takes its net to the bottom: no via.
    assert!(routed.vias.is_empty(), "{:?}", routed.vias);
    assert!(tracks_on(&routed, 1, LayerSet::EMPTY.with(LayerId(3))));
    // The other net goes straight by, under the pad's copper on the top.
    assert!(tracks_on(&routed, 2, LayerSet::EMPTY.with(LayerId(1))));
    assert!(length_of(&routed, 2) <= 6 * MM, "{}", length_of(&routed, 2));
}

#[test]
fn a_zone_joins_a_pad_s_stack_by_its_copper_on_the_zone_s_layer() {
    let mut pad = stacked(1);
    let c = 5 * MM;
    let at = Point::new(c, c);
    pad.shape = Shape::circle(at, 1_600_000).unwrap();
    pad.hole = Some(Shape::circle(at, 600_000).unwrap());
    pad.stack[0].1 = Shape::circle(at, 900_000).unwrap();
    let mut d = design(4, vec![pad], 2);
    d.zones.zones.push(zone(1, 1, 10 * MM));
    let fills = fill_zones(&d.board, &d).unwrap();
    check_fills(&d, &d.board, &fills);
    // Off the spokes: the gap of 0.4 mm is round the 0.9 mm of the inner layers, so the
    // fill is back at 1 mm from the centre, where the gap round the top's copper would be.
    let diagonal = |r: i64| c + r * 7071 / 10_000;
    assert!(!filled(&fills, diagonal(650_000), diagonal(650_000)));
    assert!(filled(&fills, diagonal(MM), diagonal(MM)));
    // And the spokes start at that copper.
    assert!(filled(&fills, c + 700_000, c));
    d.zones.zones[0].layers = LayerSet::EMPTY.with(LayerId(0));
    let fills = fill_zones(&d.board, &d).unwrap();
    assert!(!filled(&fills, diagonal(MM), diagonal(MM)));
}

/// A net tie on one layer: a pad of net 1 and a pad of net 2 a millimetre apart, joined by
/// copper of net 1 over both, with an opening in the mask over each pad, and a pad of each
/// net further out to route to.
fn tie_board() -> Design {
    let mut d = design(
        1,
        vec![
            smd(0, 0, 1),
            smd(MM, 0, 2),
            smd(-5 * MM, 0, 1),
            smd(6 * MM, 0, 2),
        ],
        3,
    );
    d.board.zones.push(drc::Zone {
        shape: Shape::rect(Rect::new(
            Point::new(-100_000, -150_000),
            Point::new(MM + 100_000, 150_000),
        ))
        .unwrap(),
        layer: LayerId(0),
        net: Some(NetId(1)),
        clearance: None,
    });
    for (pad, net) in [(0, 1), (1, 2)] {
        d.board.masks.push(MaskOpening {
            shape: d.board.pads[pad].shape.clone(),
            layer: LayerId(0),
            margin: 50_000,
            net: Some(NetId(net)),
        });
    }
    d
}

#[test]
fn a_net_tie_lets_its_nets_touch_at_its_items() {
    let mut d = tie_board();
    // Without the tie, the copper between the pads shorts the second net's pad, and is
    // bare in its opening; and no track of the second net may end on that pad.
    let found = drc::check(&d.board, &d.rules).unwrap();
    assert!(found.iter().any(|v| matches!(
        v,
        Violation::Short {
            a: Item::Pad(1),
            b: Item::Zone(0),
            ..
        }
    )));
    assert!(
        found
            .iter()
            .any(|v| matches!(v, Violation::MaskBridge { opening: 1, .. }))
    );
    let routed = super::route(&d, &RouteOptions::default()).unwrap();
    assert_eq!(routed.unrouted, [NetId(2)]);

    d.board.ties.push(NetTie {
        nets: vec![NetId(1), NetId(2)],
        items: vec![Item::Pad(0), Item::Pad(1), Item::Zone(0)],
    });
    assert!(drc::check(&d.board, &d.rules).unwrap().is_empty());
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    assert!(length_of(&routed, 1) >= 5 * MM - 1 && length_of(&routed, 2) >= 5 * MM - 1);

    // The tie is for its nets alone: a track of a third net on its copper is a short.
    let mut board = routed.board(&d);
    board.tracks.push(Track {
        start: Point::new(500_000, -MM),
        end: Point::new(500_000, MM),
        mid: None,
        width: 200_000,
        layer: LayerId(0),
        net: Some(NetId(3)),
    });
    let found = drc::check(&board, &d.rules).unwrap();
    assert!(
        found.iter().any(|v| matches!(
            v,
            Violation::Short {
                b: Item::Zone(0),
                ..
            }
        )),
        "{found:?}"
    );
    // And away from its items, its two nets keep their clearance.
    let mut board = d.board.clone();
    let track = |y: i64, net: u32| Track {
        start: Point::new(-5 * MM, y),
        end: Point::new(5 * MM, y),
        mid: None,
        width: 200_000,
        layer: LayerId(0),
        net: Some(NetId(net)),
    };
    board
        .tracks
        .extend([track(3 * MM, 1), track(3 * MM + 300_000, 2)]);
    let found = drc::check(&board, &d.rules).unwrap();
    assert!(
        matches!(found[..], [Violation::Clearance { .. }]),
        "{found:?}"
    );
}
