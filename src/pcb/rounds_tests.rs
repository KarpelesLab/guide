//! Tests of the rounds of cadlab's own router as the negotiation for every net at once.

use super::tests::{MM, design, outline, route_checked, smd};
use super::{Design, Rounds, RouteOptions, rounds};
use crate::geom::drc::Pad;
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::NetId;

/// Nets across each other, each from the left edge to the right at another height, so
/// that every one crosses every other.
fn crossed(nets: i64) -> Design {
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

fn native() -> RouteOptions {
    RouteOptions {
        whole: true,
        rounds: Rounds::Native,
        ..RouteOptions::default()
    }
}

#[test]
fn nets_across_each_other_are_all_routed_at_once() {
    let d = crossed(6);
    let routed = route_checked(&d, &native());
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
    let d = crossed(12);
    let one = route_checked(&d, &native());
    assert!(one.unrouted.is_empty());
    for threads in [2, 5] {
        let options = RouteOptions {
            threads,
            ..native()
        };
        let more = route_checked(&d, &options);
        assert_eq!(one.tracks, more.tracks);
        assert_eq!(one.vias, more.vias);
        assert_eq!(one.stats.work, more.stats.work);
    }
    // And the same twice over.
    let again = route_checked(&d, &native());
    assert_eq!(one.tracks, again.tracks);
}

#[test]
fn a_budget_that_ends_it_leaves_a_legal_board_and_the_same_on_any_number_of_threads() {
    let d = crossed(8);
    let all = route_checked(
        &d,
        &RouteOptions {
            work_budget: Some(u64::MAX / 4),
            ..native()
        },
    );
    assert!(all.unrouted.is_empty());
    for part in [1, 8, 64, 512, 4096] {
        let budget = RouteOptions {
            work_budget: Some(all.stats.work * part / 8192 + 1),
            ..native()
        };
        // Whatever it is stopped in, what is returned breaks no rule.
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
fn another_seed_is_another_order_and_still_routes_every_net() {
    let d = crossed(10);
    for seed in [None, Some(2), Some(3)] {
        let routed = route_checked(&d, &RouteOptions { seed, ..native() });
        assert!(routed.unrouted.is_empty(), "seed {seed:?}");
    }
}

/// A row of eight pads 0.25 mm wide at a pitch of 0.5 mm, each of a net of its own, with a
/// pad of each net 6 mm above it: a track of 0.3 mm out of one clears the pads beside it
/// by 0.2 mm, and two side by side out of two pads do not clear each other.
fn row() -> Design {
    let mut pads = Vec::new();
    for i in 0..8i64 {
        let x = i * 500_000;
        let pad = |y: i64, w: i64, h: i64| {
            Pad::new(
                Shape::rect(Rect::new(
                    Point::new(x - w / 2, y - h / 2),
                    Point::new(x + w / 2, y + h / 2),
                ))
                .unwrap(),
                LayerSet::EMPTY.with(LayerId(0)),
                Some(NetId(i as u32 + 1)),
            )
        };
        pads.push(pad(0, 250_000, 1_500_000));
        pads.push(pad(6 * MM + (i % 2) * 2 * MM, 250_000, 600_000));
    }
    let mut d = design(2, pads, 9);
    for spec in &mut d.nets {
        spec.track_width = 300_000;
    }
    d.outline = outline(Rect::new(
        Point::new(-3 * MM, -3 * MM),
        Point::new(7 * MM, 11 * MM),
    ));
    d
}

#[test]
fn the_nets_of_a_row_of_fine_pitch_are_narrower_where_the_parts_are_told() {
    let mut d = row();
    // With no parts told there are no rows.
    assert!(rounds::necks(&d).iter().all(Option::is_none));
    // The row is one part and each pad above it one of its own.
    d.parts = (0..16)
        .map(|p| if p % 2 == 0 { 0 } else { 1 + p as u32 })
        .collect();
    let narrow = rounds::necks(&d);
    let least = d.rules.spec().track_width;
    for (net, w) in narrow.iter().enumerate().skip(1) {
        let w = w.expect("narrower");
        assert!(w < 300_000 && w >= least, "{w}");
        // Two of them side by side out of two pads of the row clear each other.
        assert!(w + d.rules.clearance(Some(NetId(net as u32)), LayerId(0)) <= 500_000);
    }
    assert!(narrow[0].is_none());
}

#[test]
fn a_row_of_fine_pitch_is_routed_with_its_ways_out() {
    let mut d = row();
    d.parts = (0..16)
        .map(|p| if p % 2 == 0 { 0 } else { 1 + p as u32 })
        .collect();
    let options = RouteOptions {
        work_budget: Some(50_000_000),
        ..native()
    };
    let routed = route_checked(&d, &options);
    assert!(routed.unrouted.is_empty());
    let (rounds, shared, _, nets) = routed.stats.whole;
    assert!(rounds >= 1);
    assert_eq!((shared, nets), (0, 8));
}
