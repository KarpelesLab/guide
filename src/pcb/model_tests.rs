//! Tests of what a host says of its board beyond pads and nets: where vias may be, zones
//! filled by the host, zones that are planes or not, keepouts' exceptions, layers, pad
//! stacks, net ties and rules by area.

use super::tests::{MM, design, route_checked, smd};
use super::zone_tests::{check_fills, plane_board, zone};
use super::{Design, Fill, Islands, RouteOptions, Routed, fill_zones, unconnected};
use crate::geom::drc::{self, Pad, Track};
use crate::geom::{LayerId, LayerSet, Point, Shape};
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
