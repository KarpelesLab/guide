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

/// On one layer, a net of six pads in a line from one edge of the board to the other, and two
/// nets that each have a pad on either side of it: no board has all three.
fn line_and_two_across() -> Design {
    let mut pads: Vec<Pad> = (0..6).map(|i| smd(i * 2 * MM, 0, 1)).collect();
    for (k, x) in [(2u32, 3 * MM), (3, 7 * MM)] {
        // At the board's edges, with no room for a track behind them: the line cannot go
        // round a pad of theirs either.
        pads.push(smd(x, -5_500_000, k));
        pads.push(smd(x, 5_500_000, k));
    }
    let mut d = design(1, pads, 4);
    d.outline = outline(Rect::new(
        Point::new(-2 * MM, -6 * MM),
        Point::new(12 * MM, 6 * MM),
    ));
    // No way round its ends: from each to the board's edge is something that no net is
    // routed over.
    for (from, to) in [(-2 * MM, -600_000), (10_600_000, 12 * MM)] {
        d.obstacles.push((
            Shape::rect(Rect::new(
                Point::new(from, -300_000),
                Point::new(to, 300_000),
            ))
            .unwrap(),
            LayerId(0),
        ));
    }
    d
}

#[test]
fn routes_that_still_share_a_point_are_made_legal_a_wire_at_a_time() {
    let d = line_and_two_across();
    let routed = route_checked(&d, &native());
    // The two nets across have to cross the line, each between two other pads of it, and the
    // rounds end with points shared: by two wires of the line and the two wires across them.
    // Each wire across shares as many points as the wire of the line that it crosses, and of
    // two that share as many the longer goes, which has more other ways: the two wires
    // across are taken out, one and then the other, and the line of five wires is whole.
    // (cadlab's own router takes out the net that shares the most, all of it: the line.
    // And with a point that stays shared costing a quarter as much more in each round, as
    // it did until 0.0.37, the rounds ended with both nets across on one wire of the line,
    // which was taken out alone, and the passes after them left the line with all five of
    // its connections open.)
    let (_, shared, taken_out, nets) = routed.stats.whole;
    assert!(shared >= 2, "{:?}", routed.stats.whole);
    assert_eq!((taken_out, nets), (2, 1));
    assert_eq!(routed.unrouted, [NetId(2), NetId(3)]);
    assert_eq!(routed.open.len(), 2);
    // The same on any number of threads.
    let more = route_checked(
        &d,
        &RouteOptions {
            threads: 3,
            ..native()
        },
    );
    assert_eq!(routed.tracks, more.tracks);
    assert_eq!(routed.stats.work, more.stats.work);
}

/// On one layer, a net of two pads, one of them walled in by pads of eight other nets that
/// no track passes between.
fn walled_in() -> Design {
    let mut pads = vec![smd(0, 0, 1), smd(10 * MM, 0, 1)];
    let mut net = 2;
    for dx in [-900_000, 0, 900_000] {
        for dy in [-900_000, 0, 900_000] {
            if (dx, dy) != (0, 0) {
                pads.push(smd(10 * MM + dx, dy, net));
                net += 1;
            }
        }
    }
    let mut d = design(1, pads, 10);
    d.outline = outline(Rect::new(
        Point::new(-2 * MM, -4 * MM),
        Point::new(14 * MM, 4 * MM),
    ));
    d
}

#[test]
fn a_board_given_up_has_the_share_of_the_time_that_it_is_told() {
    use super::{RunControl, Stopped, route_with};
    use std::time::{Duration, Instant};
    let hour = || RunControl {
        deadline: Some(Instant::now() + Duration::from_secs(3600)),
        ..RunControl::default()
    };
    // Given up at the look, with none of the time to its deadline: the run that begins
    // again is at its deadline as it begins, and says so.
    let d = walled_in();
    let none = RouteOptions {
        given_up_share: 0.0,
        ..native()
    };
    let routed = route_with(&d, &none, &hour()).unwrap();
    assert!(routed.stats.rounds_given_up);
    assert_eq!(routed.stopped, Some(Stopped::Deadline));
    // With all of it, which is the default, nothing stops it.
    assert_eq!(native().given_up_share, 1.0);
    let routed = route_with(&d, &native(), &hour()).unwrap();
    assert!(routed.stats.rounds_given_up);
    assert_eq!(routed.stopped, None);
    // And a board that the rounds take has its deadline whatever the share.
    let d = crossed(6);
    let routed = route_with(&d, &none, &hour()).unwrap();
    assert!(!routed.stats.rounds_given_up);
    assert_eq!(routed.stopped, None);
    assert!(routed.unrouted.is_empty());
    // Nor is this crate's own rounds' run told anything by it.
    let own = RouteOptions {
        rounds: Rounds::Own,
        ..none
    };
    let routed = route_with(&walled_in(), &own, &hour()).unwrap();
    assert_eq!(routed.stopped, None);
}

#[test]
fn a_board_whose_connections_have_no_way_on_the_grid_is_given_up_before_a_round() {
    let d = walled_in();
    let routed = route_checked(&d, &native());
    // Its one connection has no way whatever the other nets do: the rounds say so before
    // they route anything, and the run is one of this crate's own.
    assert!(routed.stats.rounds_given_up, "{:?}", routed.stats);
    assert_eq!(routed.unrouted, [NetId(1)]);
    // Told before the first round, it has cost the run no search.
    let own = route_checked(
        &d,
        &RouteOptions {
            rounds: Rounds::Own,
            ..native()
        },
    );
    assert!(!own.stats.rounds_given_up);
    assert_eq!(routed.stats.work, own.stats.work);
    // And it is that run, to the track.
    assert_eq!(routed.tracks, own.tracks);
    assert_eq!(routed.vias, own.vias);
}

/// What the look at a board is given of its fills and its spokes, on a board with none.
fn no_pours<'a>(
    d: &Design,
    counted: &'a dyn Fn(Vec<super::Fill>) -> Vec<super::Fill>,
) -> rounds::Pours<'a> {
    rounds::Pours {
        fills: &[],
        own_zones: d.board.zones.len(),
        is_plane: &[],
        counted,
        spoke: &|_, _| (0, 0.0),
    }
}

#[test]
fn the_look_at_a_board_counts_its_connections_and_those_with_no_way() {
    let counted = |fills: Vec<super::Fill>| fills;
    // Four nets across each other all have a way, and a first round takes some work.
    let d = crossed(4);
    let look = rounds::look(&d, &no_pours(&d, &counted)).expect("a board to route");
    assert_eq!((look.conns, look.no_way), (4, 0));
    assert!(look.work > 0.0 && look.secs >= 0.0, "{look:?}");
    assert!(!look.gives_up(None, None, 1));
    // The look is the same each time: it has no order of its own, and no clock in its count.
    let again = rounds::look(&d, &no_pours(&d, &counted)).expect("a board to route");
    assert_eq!(
        (look.conns, look.no_way, look.work),
        (again.conns, again.no_way, again.work)
    );
    // A pad walled in has no way, and that is the board's one connection.
    let d = walled_in();
    let look = rounds::look(&d, &no_pours(&d, &counted)).expect("a board to route");
    assert_eq!((look.conns, look.no_way), (1, 1));
    assert!(look.gives_up(None, None, 1));
    // And nothing is searched for on a board that is given up for that.
    assert_eq!((look.work, look.secs), (0.0, 0.0));
}

#[test]
fn a_board_is_given_up_for_its_connections_with_no_way_or_for_the_room_of_eight_first_rounds() {
    let look = rounds::Look {
        conns: 100,
        no_way: 2,
        work: 10.0,
        secs: 1.0,
    };
    // One connection in fifty with no way is not too many; more is.
    assert!(!look.gives_up(None, None, 1));
    assert!(rounds::Look { no_way: 3, ..look }.gives_up(None, None, 1));
    // Eight first rounds are to fit what the rounds have of the work,
    assert!(look.gives_up(Some(79.0), None, 1));
    assert!(!look.gives_up(Some(80.0), None, 1));
    // and of the time, where three threads do the work of two.
    assert!(look.gives_up(None, Some(7.9), 1));
    assert!(!look.gives_up(None, Some(8.0), 1));
    assert!(look.gives_up(None, Some(3.9), 3));
    assert!(!look.gives_up(None, Some(4.0), 3));
}

#[test]
fn a_run_of_this_crate_s_own_rounds_is_the_run_of_the_board_without_the_zones_told_for_the_others()
{
    let plain = crossed(3);
    let mut told = plain.clone();
    // A pour of net 1 that the host's fill left with no copper, told after the others.
    told.zones.unfilled_from = Some(told.zones.zones.len());
    told.zones.zones.push(super::CopperZone {
        outline: vec![
            Point::new(-MM, -MM),
            Point::new(3 * MM, -MM),
            Point::new(3 * MM, MM),
            Point::new(-MM, MM),
        ],
        layers: LayerSet::EMPTY.with(LayerId(0)),
        net: Some(NetId(1)),
        ..super::CopperZone::default()
    });
    let own = RouteOptions {
        rounds: Rounds::Own,
        fills: true,
        ..native()
    };
    let (a, b) = (route_checked(&plain, &own), route_checked(&told, &own));
    assert_eq!(a.tracks, b.tracks);
    assert_eq!(a.vias, b.vias);
    assert_eq!(a.stats.work, b.stats.work);
    assert!(b.fills.is_empty());
    // Told as a zone like the others, it is one that the run plans on and fills.
    told.zones.unfilled_from = None;
    let c = route_checked(&told, &own);
    assert!(!c.fills.is_empty());
}

/// On one layer, a pad of net 1 in a pour of its net with spokes along the axes, and a net
/// of two pads whose straight way passes the pad through the room of its right spoke.
fn past_a_spoke() -> Design {
    let mut d = design(
        1,
        vec![
            smd(5 * MM, 5 * MM, 1),
            smd(5_600_000, 2 * MM, 2),
            smd(5_600_000, 8 * MM, 2),
        ],
        3,
    );
    d.zones.zones.push(super::CopperZone {
        outline: vec![
            Point::new(0, 0),
            Point::new(10 * MM, 0),
            Point::new(10 * MM, 10 * MM),
            Point::new(0, 10 * MM),
        ],
        layers: LayerSet::EMPTY.with(LayerId(0)),
        net: Some(NetId(1)),
        connection: super::ZoneConnection::Thermal,
        thermal_gap: 300_000,
        spoke_width: 300_000,
        min_width: 200_000,
        ..super::CopperZone::default()
    });
    d.outline = outline(Rect::new(
        Point::new(-MM, -MM),
        Point::new(11 * MM, 11 * MM),
    ));
    d
}

/// How many points along the tracks of `net`, a tenth of a millimetre apart, are in the
/// room of a spoke of another net's pad.
fn in_a_spoke_s_room(d: &Design, routed: &super::Routed, net: NetId) -> usize {
    let bounds = Rect::new(Point::new(-MM, -MM), Point::new(11 * MM, 11 * MM));
    let spokes = super::spokes::Spokes::plan(d, bounds, 100_000, 300_000, 4.0).unwrap();
    let mut n = 0;
    for t in routed.tracks.iter().filter(|t| t.net == Some(net)) {
        let (a, b) = (t.start, t.end);
        let steps = (((b.x - a.x).abs() + (b.y - a.y).abs()) / 100_000).max(1);
        for i in 0..=steps {
            let p = Point::new(a.x + (b.x - a.x) * i / steps, a.y + (b.y - a.y) * i / steps);
            n += usize::from(spokes.factor(net, 0, p) > 1.0);
        }
    }
    n
}

#[test]
fn the_room_of_a_spoke_has_its_price_in_these_rounds_too() {
    let d = past_a_spoke();
    // With no price for it the straight way is through the room of the pad's right spoke.
    // (The routes are looked at as the rounds leave them, with no cleanup after.)
    let rounds = RouteOptions {
        cleanup: false,
        ..native()
    };
    let free = route_checked(&d, &rounds);
    assert!(free.unrouted.is_empty());
    assert!(in_a_spoke_s_room(&d, &free, NetId(2)) > 0);
    // With the price the net goes round it, and the route is the one of the rounds: no
    // pass took it out or laid it again.
    let priced = route_checked(
        &d,
        &RouteOptions {
            spoke_cost: 4.0,
            ..rounds
        },
    );
    assert!(priced.unrouted.is_empty());
    assert_eq!(in_a_spoke_s_room(&d, &priced, NetId(2)), 0);
    assert_eq!(priced.stats.whole_made.0, 1);
    assert_eq!(priced.stats.rip_ups, 0);
}
