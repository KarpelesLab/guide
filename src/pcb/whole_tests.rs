//! Tests of the negotiation for every net at once.

use super::tests::{MM, design, outline, route_checked, smd, through};
use super::{RouteOptions, route};
use crate::geom::drc::Keepout;
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::NetId;

/// Nets across each other, each from the left edge to the right at another height, so
/// that every one crosses every other.
fn crossed(nets: i64) -> super::Design {
    let pads = (0..nets)
        .flat_map(|i| {
            [
                smd(0, i * 2 * MM, i as u32 + 1),
                smd(12 * MM, (nets - 1 - i) * 2 * MM, i as u32 + 1),
            ]
        })
        .collect();
    let mut d = design(2, pads, nets as usize + 1);
    d.outline = outline(Rect::new(
        Point::new(-2 * MM, -2 * MM),
        Point::new(14 * MM, nets * 2 * MM),
    ));
    d
}

fn whole() -> RouteOptions {
    RouteOptions {
        whole: true,
        ..RouteOptions::default()
    }
}

#[test]
fn nets_across_each_other_are_all_routed_at_once() {
    let d = crossed(6);
    let routed = route_checked(&d, &whole());
    assert!(routed.unrouted.is_empty());
    // Every net began the passes with its route, and none was taken out for another.
    let (rounds, shared, taken_out, nets) = routed.stats.whole;
    assert!(rounds >= 1);
    assert_eq!((shared, taken_out, nets), (0, 0, 6));
    assert_eq!(routed.stats.whole_made.0, 6);
    assert_eq!(routed.stats.rip_ups, 0);
    assert!(!routed.vias.is_empty());
}

#[test]
fn the_routes_are_the_same_on_any_number_of_threads() {
    let d = crossed(8);
    let one = route_checked(&d, &whole());
    for threads in [2, 5] {
        let options = RouteOptions { threads, ..whole() };
        let more = route_checked(&d, &options);
        assert_eq!(one.tracks, more.tracks);
        assert_eq!(one.vias, more.vias);
        assert_eq!(one.stats.work, more.stats.work);
    }
    // And the same twice over.
    let again = route_checked(&d, &whole());
    assert_eq!(one.tracks, again.tracks);
}

#[test]
fn more_nets_than_are_routed_at_once_have_the_same_routes_on_any_number_of_threads() {
    // Twenty nets across each other: more than the few that are routed at once, and
    // each in the way of the others of its few, so that some are routed again at their
    // turn.
    let d = crossed(20);
    let one = route_checked(&d, &whole());
    assert!(one.unrouted.is_empty());
    for threads in [3, 16] {
        let options = RouteOptions { threads, ..whole() };
        let more = route_checked(&d, &options);
        assert_eq!(one.tracks, more.tracks);
        assert_eq!(one.vias, more.vias);
        assert_eq!(one.stats.work, more.stats.work);
    }
    // And whatever a budget stops it in.
    for part in [8, 64, 512, 4096] {
        let budget = RouteOptions {
            work_budget: Some(one.stats.work * part / 8192 + 1),
            ..whole()
        };
        let alone = route_checked(&d, &budget);
        let more = route_checked(
            &d,
            &RouteOptions {
                threads: 3,
                ..budget
            },
        );
        assert_eq!(alone.tracks, more.tracks);
        assert_eq!(alone.vias, more.vias);
        assert_eq!(alone.stats.work, more.stats.work);
    }
}

#[test]
fn a_budget_that_ends_it_leaves_a_legal_board_and_the_passes_go_on() {
    let d = crossed(8);
    let all = route_checked(&d, &whole());
    assert!(all.unrouted.is_empty());
    for part in [1, 8, 64, 512, 4096] {
        let options = RouteOptions {
            work_budget: Some(all.stats.work * part / 8192 + 1),
            ..whole()
        };
        // Whatever it is stopped in, what is returned breaks no rule and is counted
        // as it is.
        let routed = route_checked(&d, &options);
        let again = route(&d, &options).unwrap();
        assert_eq!(routed.tracks, again.tracks);
        assert_eq!(routed.vias, again.vias);
    }
}

#[test]
fn on_one_layer_what_cannot_cross_is_left_to_the_passes() {
    // Two nets that cross with no other layer: one of them has no way, and the board is
    // still legal.
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
    d.outline = outline(Rect::new(
        Point::new(-600_000, -600_000),
        Point::new(10_600_000, 10_600_000),
    ));
    let routed = route_checked(&d, &whole());
    assert_eq!(routed.unrouted.len(), 1);
    // Its negotiation ends with the crossing still shared at the pitch, so it is begun
    // again on the finer grids and in other orders, none of which ends otherwise: the
    // board is the same on any number of threads all the same, and the same twice over.
    assert!(routed.stats.whole.1 > 0);
    for threads in [1, 3] {
        let options = RouteOptions { threads, ..whole() };
        let again = route_checked(&d, &options);
        assert_eq!(routed.tracks, again.tracks);
        assert_eq!(routed.stats.work, again.stats.work);
    }
}

#[test]
fn on_more_than_two_layers_the_routes_are_legal_and_the_same_on_any_number_of_threads() {
    // The same nets across each other on four layers, where the grid is of twice the
    // pitch.
    let mut d = crossed(8);
    d.rules = super::tests::rules(4);
    let one = route_checked(&d, &whole());
    assert!(one.unrouted.is_empty());
    let (_, shared, taken_out, nets) = one.stats.whole;
    assert_eq!((shared, taken_out, nets), (0, 0, 8));
    let options = RouteOptions {
        threads: 3,
        ..whole()
    };
    let more = route_checked(&d, &options);
    assert_eq!(one.tracks, more.tracks);
    assert_eq!(one.vias, more.vias);
    // And whatever a budget stops it in.
    for part in [8, 64, 512, 4096] {
        let options = RouteOptions {
            work_budget: Some(one.stats.work * part / 8192 + 1),
            ..whole()
        };
        let routed = route_checked(&d, &options);
        let again = route(&d, &options).unwrap();
        assert_eq!(routed.tracks, again.tracks);
        assert_eq!(routed.vias, again.vias);
    }
}

#[test]
fn a_plane_of_another_net_costs_a_track_nothing_in_the_negotiation() {
    // Four layers, the second a plane of net 1, whose two pads are on one side of the
    // board, and net 2 between two pads with holes on the other side, 14 mm apart across
    // the direction of the top layer, with tracks kept off the two layers below the
    // plane. On the plane's layer, along its direction, the way costs its 35 steps; on
    // the top layer, in diagonals from side to side, half as much again. At twice its
    // length inside the plane, as the passes that route a net at a time pay, the top
    // layer was the cheaper of the two.
    let mut d = design(
        4,
        vec![
            through(3 * MM, 5 * MM, 1, 4),
            through(3 * MM, 15 * MM, 1, 4),
            through(14 * MM, 3 * MM, 2, 4),
            through(14 * MM, 17 * MM, 2, 4),
        ],
        3,
    );
    d.outline = outline(Rect::new(Point::new(0, 0), Point::new(20 * MM, 20 * MM)));
    d.edge_clearance = 300_000;
    d.zones.zones.push(super::zone_tests::zone(1, 1, 20 * MM));
    d.board.keepouts.push(Keepout {
        shape: Shape::rect(Rect::new(Point::new(0, 0), Point::new(20 * MM, 20 * MM))).unwrap(),
        layers: LayerSet::EMPTY.with(LayerId(2)).with(LayerId(3)),
        tracks: true,
        vias: false,
        pads: false,
        except: Vec::new(),
    });
    let options = RouteOptions {
        cleanup: false,
        ..whole()
    };
    let routed = route_checked(&d, &options);
    assert!(routed.unrouted.is_empty());
    assert!(routed.stats.whole.3 >= 1, "{:?}", routed.stats.whole);
    let net = Some(NetId(2));
    assert_eq!(routed.vias.iter().filter(|v| v.net == net).count(), 0);
    let tracks: Vec<_> = routed.tracks.iter().filter(|t| t.net == net).collect();
    assert!(!tracks.is_empty());
    assert!(tracks.iter().all(|t| t.layer == LayerId(1)), "{tracks:?}");
}
