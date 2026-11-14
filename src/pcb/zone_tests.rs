//! Tests of zone filling, and of routing nets through their zones, on small boards.

use super::tests::{MM, design, outline, route_checked, smd, through};
use super::{
    CopperZone, Design, Islands, PadConnection, RouteOptions, ZoneConnection, fill_zones,
    unconnected,
};
use crate::geom::drc::{self, Board};
use crate::geom::{Area, LayerId, LayerSet, Point, Rect};
use crate::id::NetId;

/// A square zone on `layer`, of net `net`, from `0` to `size` on each side.
pub(super) fn zone(net: u32, layer: u8, size: i64) -> CopperZone {
    CopperZone {
        outline: vec![
            Point::new(0, 0),
            Point::new(size, 0),
            Point::new(size, size),
            Point::new(0, size),
        ],
        holes: Vec::new(),
        layers: LayerSet::EMPTY.with(LayerId(layer)),
        net: Some(NetId(net)),
        priority: 0,
        clearance: 300_000,
        min_width: 200_000,
        connection: ZoneConnection::Thermal,
        thermal_gap: 400_000,
        spoke_width: 300_000,
        islands: Islands::Remove,
        ..CopperZone::default()
    }
}

fn area_mm2(design: &Design, fills: &[super::Fill]) -> f64 {
    let areas: Vec<Area> = fills
        .iter()
        .map(|f| Area::of_shape(&f.shape, 0, 1, crate::geom::ArcSide::Inside).unwrap())
        .collect();
    let _ = design;
    Area::union_all(&areas).unwrap().area2() as f64 / 2e12
}

/// Checks a design's board with its fills: no violation, and the fills of each net touch
/// its pads.
pub(super) fn check_fills(design: &Design, board: &Board, fills: &[super::Fill]) -> Board {
    let mut b = board.clone();
    b.zones.extend(fills.iter().map(|f| f.to_zone(design)));
    let found = drc::check(&b, &design.rules).unwrap();
    assert!(found.is_empty(), "{found:?}");
    b
}

#[test]
fn a_fill_keeps_clear_of_other_nets_and_joins_its_own_through_spokes() {
    // A pad of the zone's net, and one of another, in a 10 mm square.
    let mut d = design(1, vec![smd(3 * MM, 5 * MM, 1), smd(7 * MM, 5 * MM, 2)], 3);
    d.zones.zones.push(zone(1, 0, 10 * MM));
    let fills = fill_zones(&d.board, &d).unwrap();
    assert_eq!(fills.len(), 1);
    let board = check_fills(&d, &d.board, &fills);
    // The other pad is kept clear of by the clearance, and the thermal gap is around the
    // pad of the net, with four spokes across it, so the pad is connected.
    assert!(unconnected(&board, &d.rules).is_empty());
    let other = &d.board.pads[1].shape;
    assert!(!fills[0].shape.violates(other, 300_000).unwrap());
    let own = &d.board.pads[0].shape;
    assert!(fills[0].shape.touches(own));
    // The square less two rounded squares: the pad grown by the gap, 1.82 mm², and the
    // other grown by the clearance, 1.36 mm²; and the spokes, across the gap and the pad,
    // add some 0.75 mm².
    let a = area_mm2(&d, &fills);
    assert!((97.3..97.8).contains(&a), "{a}");
}

#[test]
fn spokes_that_reach_no_fill_are_left_out_and_islands_go() {
    // The pad of the zone's net is walled in by another net's track, so its spokes reach
    // nothing beyond the gap, and the fill inside the wall is an island.
    let mut d = design(1, vec![smd(5 * MM, 5 * MM, 1), smd(MM, MM, 1)], 3);
    let r = Rect::new(Point::new(3 * MM, 3 * MM), Point::new(7 * MM, 7 * MM));
    for wall in outline(r) {
        let core = wall.bbox();
        let (a, b) = (core.min(), core.max());
        d.board.tracks.push(drc::Track {
            start: Point::new(a.x, a.y),
            end: Point::new(b.x, b.y),
            mid: None,
            width: 200_000,
            layer: LayerId(0),
            net: Some(NetId(2)),
        });
    }
    d.zones.zones.push(zone(1, 0, 10 * MM));
    let fills = fill_zones(&d.board, &d).unwrap();
    let board = check_fills(&d, &d.board, &fills);
    // Outside the wall is joined to the second pad; inside, the fill around the first pad
    // reaches it through its spokes, so both stay, but don't join.
    assert_eq!(fills.len(), 2);
    assert_eq!(unconnected(&board, &d.rules), [NetId(1)]);
    // With the pad's connection none, the inside reaches nothing and is an island.
    d.zones.pads = vec![
        PadConnection {
            connection: Some(ZoneConnection::None),
            ..PadConnection::default()
        };
        1
    ];
    let fills = fill_zones(&d.board, &d).unwrap();
    check_fills(&d, &d.board, &fills);
    assert_eq!(fills.len(), 1);
    // Kept, it stays.
    d.zones.zones[0].islands = Islands::Keep;
    assert_eq!(fill_zones(&d.board, &d).unwrap().len(), 2);
}

#[test]
fn higher_priority_zones_of_other_nets_are_kept_clear_of() {
    let mut d = design(1, vec![smd(2 * MM, 2 * MM, 1), smd(8 * MM, 8 * MM, 2)], 3);
    d.zones.zones.push(zone(1, 0, 10 * MM));
    let mut high = zone(2, 0, 10 * MM);
    high.outline = vec![
        Point::new(5 * MM, 5 * MM),
        Point::new(10 * MM, 5 * MM),
        Point::new(10 * MM, 10 * MM),
        Point::new(5 * MM, 10 * MM),
    ];
    high.priority = 1;
    d.zones.zones.push(high);
    let fills = fill_zones(&d.board, &d).unwrap();
    // The higher first.
    assert_eq!(fills[0].zone, 1);
    check_fills(&d, &d.board, &fills);
    let low: Vec<_> = fills.iter().filter(|f| f.zone == 0).cloned().collect();
    let a = area_mm2(&d, &low);
    // The square less the other's quarter, grown by its clearance.
    assert!(a < 75.0 - 2.0 * 5.0 * 0.29, "{a}");
}

/// Four pads of a net with a plane on an inner layer, and of another crossing it.
pub(super) fn plane_board() -> Design {
    let mut d = design(
        4,
        vec![
            smd(2 * MM, 2 * MM, 1),
            smd(18 * MM, 2 * MM, 1),
            smd(2 * MM, 18 * MM, 1),
            smd(18 * MM, 18 * MM, 1),
            smd(10 * MM, MM, 2),
            smd(10 * MM, 19 * MM, 2),
            through(10 * MM, 10 * MM, 1, 4),
        ],
        3,
    );
    d.outline = outline(Rect::new(Point::new(0, 0), Point::new(20 * MM, 20 * MM)));
    d.edge_clearance = 300_000;
    d.zones.zones.push(zone(1, 1, 20 * MM));
    d
}

#[test]
fn a_net_with_a_plane_connects_to_it_with_stubs() {
    let d = plane_board();
    let routed = route_checked(&d, &RouteOptions::default());
    assert!(routed.unrouted.is_empty());
    assert!(!routed.fills.is_empty());
    // Each surface pad has a via of its own to the plane, close by, and the net has no
    // track across the board.
    let net = Some(NetId(1));
    let vias = routed.vias.iter().filter(|v| v.net == net).count();
    assert_eq!(vias, 4);
    let longest = routed
        .tracks
        .iter()
        .filter(|t| t.net == net)
        .map(|t| (t.end.x - t.start.x).abs().max((t.end.y - t.start.y).abs()))
        .max()
        .unwrap_or(0);
    assert!(longest < 2 * MM, "{longest}");
    // The other net crosses the board, on an outer layer.
    assert!(
        routed
            .tracks
            .iter()
            .filter(|t| t.net == Some(NetId(2)))
            .all(|t| t.layer != LayerId(1))
    );
    assert_eq!(routed.stats.connections, 4 + 1);
    assert_eq!(routed.stats.connected, 4 + 1);
}

#[test]
fn without_planes_nets_with_zones_are_routed_as_tracks_and_filled_after() {
    let d = plane_board();
    let options = RouteOptions {
        planes: false,
        ..RouteOptions::default()
    };
    let routed = route_checked(&d, &options);
    assert!(routed.unrouted.is_empty());
    assert!(!routed.fills.is_empty());
    let length: i64 = routed
        .tracks
        .iter()
        .filter(|t| t.net == Some(NetId(1)))
        .map(|t| (t.end.x - t.start.x).abs() + (t.end.y - t.start.y).abs())
        .sum();
    assert!(length > 20 * MM, "{length}");
}

#[test]
fn plane_routing_is_deterministic() {
    let d = plane_board();
    let a = route_checked(&d, &RouteOptions::default());
    let b = route_checked(&d, &RouteOptions::default());
    assert_eq!(a.tracks, b.tracks);
    assert_eq!(a.vias, b.vias);
    assert_eq!(a.fills, b.fills);
}
