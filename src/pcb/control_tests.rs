//! Tests of run control, of what a run reports of the connections it leaves open, and of
//! the options that came with them.

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::tests::{MM, design, outline, route_checked, smd};
use super::{
    Blocker, Cause, CopperKind, Design, DesignItem, Phase, Progress, RouteError, RouteOptions,
    Routed, RunControl, Stopped, route, route_with, unconnected,
};
use crate::geom::drc::{self, Keepout, Pad, Track};
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::NetId;

/// Six nets across and six down on two layers, which all cross.
fn weave() -> Design {
    let mut pads = Vec::new();
    for k in 0..6 {
        let at = (2 * k + 2) * MM;
        pads.push(smd(0, at, 1 + k as u32));
        pads.push(smd(14 * MM, at, 1 + k as u32));
        pads.push(smd(at - MM, 0, 7 + k as u32));
        pads.push(smd(at - MM, 14 * MM, 7 + k as u32));
    }
    let mut d = design(2, pads, 13);
    d.outline = outline(Rect::new(
        Point::new(-MM, -MM),
        Point::new(15 * MM, 15 * MM),
    ));
    d
}

/// Five pads of one net in a row on one layer, the last walled in by four keepouts.
fn walled_in() -> Design {
    let pads: Vec<Pad> = (0..5).map(|k| smd(k * 2 * MM, 0, 1)).collect();
    let mut d = design(1, pads, 2);
    let x = 8 * MM;
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
        Point::new(11 * MM, 3 * MM),
    ));
    d
}

/// What must hold of any result, stopped or not: no rule broken, the nets in pieces are
/// the nets reported, and the open connections are the pieces less one.
fn check(design: &Design, routed: &Routed) {
    let board = routed.board(design);
    let found = drc::check(&board, &design.rules).unwrap();
    assert!(found.is_empty(), "{found:?}");
    assert_eq!(unconnected(&board, &design.rules), routed.unrouted);
    let (needed, made) = routed.stats.islands;
    assert_eq!(routed.open.len(), needed - made);
    let mut nets: Vec<NetId> = routed.open.iter().map(|o| o.net).collect();
    nets.dedup();
    assert_eq!(nets, routed.unrouted);
    assert_eq!(routed.track_kinds.len(), routed.tracks.len());
    assert_eq!(routed.via_kinds.len(), routed.vias.len());
}

/// Everything of a result that a caller can see.
fn same(a: &Routed, b: &Routed) {
    assert_eq!(a.tracks, b.tracks);
    assert_eq!(a.vias, b.vias);
    assert_eq!(a.unrouted, b.unrouted);
    assert_eq!(a.stats, b.stats);
    assert_eq!(a.stopped, b.stopped);
    assert_eq!(a.open, b.open);
    assert_eq!(a.track_kinds, b.track_kinds);
}

#[test]
fn a_run_under_control_that_nothing_stops_is_the_run_without() {
    let d = weave();
    let plain = route_checked(&d, &RouteOptions::default());
    assert!(plain.unrouted.is_empty() && plain.stopped.is_none() && plain.open.is_empty());
    assert_eq!(plain.stats.islands, (12, 12));
    assert!(plain.stats.work >= plain.stats.expanded && plain.stats.work > 0);
    assert!(plain.track_kinds.iter().all(|k| *k == CopperKind::Plain));
    check(&d, &plain);

    // A flag that is never set and a progress that hears of everything change nothing.
    let cancel = AtomicBool::new(false);
    let heard = |_: &Progress| {};
    let control = RunControl {
        cancel: Some(&cancel),
        progress: Some(&heard),
        progress_interval: Duration::ZERO,
        ..RunControl::default()
    };
    same(
        &plain,
        &route_with(&d, &RouteOptions::default(), &control).unwrap(),
    );
    // A run with a deadline or a budget makes the nets that find no way in their corridors
    // wait, and takes only stubs out of nets with planes: here no net does, and there is
    // no plane, so a deadline that is not reached changes nothing either.
    let control = RunControl {
        deadline: Some(Instant::now() + Duration::from_secs(3600)),
        ..control
    };
    same(
        &plain,
        &route_with(&d, &RouteOptions::default(), &control).unwrap(),
    );

    // Nor does a budget that is not spent.
    let options = RouteOptions {
        work_budget: Some(plain.stats.work + 1),
        ..RouteOptions::default()
    };
    same(&plain, &route(&d, &options).unwrap());
}

#[test]
fn a_work_budget_stops_a_run_the_same_way_every_time_and_on_any_thread() {
    let d = weave();
    let whole = route(&d, &RouteOptions::default()).unwrap();
    let mut options = RouteOptions::default();
    let mut seen_partial = false;
    for share in [0, 1, 2, 3, 5, 7] {
        let budget = whole.stats.work * share / 8;
        options.work_budget = Some(budget);
        let first = route(&d, &options).unwrap();
        check(&d, &first);
        assert_eq!(first.stopped, Some(Stopped::WorkBudget), "budget {budget}");
        assert!(first.stats.work <= budget);
        // The last of the work is cleanup's: a budget that ends there leaves nothing open.
        seen_partial |= !first.tracks.is_empty() && !first.open.is_empty();
        // Again here, and on four threads at once, which is as many thread counts as a
        // router with no threads of its own can be run under.
        same(&first, &route(&d, &options).unwrap());
        std::thread::scope(|scope| {
            let runs: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| route(&d, &options).unwrap()))
                .collect();
            for run in runs {
                same(&first, &run.join().unwrap());
            }
        });
    }
    assert!(seen_partial, "no budget left a part of the board routed");
    // Nothing spent, nothing routed, and every connection is reported as stopped.
    options.work_budget = Some(0);
    let none = route(&d, &options).unwrap();
    assert!(none.tracks.is_empty() && none.vias.is_empty());
    assert_eq!(none.open.len(), 12);
    assert!(none.open.iter().all(|o| o.cause == Cause::Stopped));
    assert_eq!(none.stats.work, 0);
}

#[test]
fn a_cancel_flag_stops_a_run_with_what_it_has_routed() {
    let d = weave();
    let cancel = AtomicBool::new(false);
    // The flag is set from the callback, once three nets are routed, so that the test
    // doesn't depend on the clock.
    let heard = |p: &Progress| {
        if p.nets_routed >= 3 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let control = RunControl {
        cancel: Some(&cancel),
        progress: Some(&heard),
        progress_interval: Duration::ZERO,
        ..RunControl::default()
    };
    let routed = route_with(&d, &RouteOptions::default(), &control).unwrap();
    check(&d, &routed);
    assert_eq!(routed.stopped, Some(Stopped::Cancelled));
    assert_eq!(routed.stats.routed, 3);
    assert_eq!(routed.unrouted.len(), 9);
    assert_eq!(routed.open.len(), 9);
    assert!(routed.open.iter().all(|o| o.cause == Cause::Stopped));
    assert!(!routed.tracks.is_empty());
}

#[test]
fn a_deadline_stops_a_run() {
    let d = weave();
    let control = RunControl {
        deadline: Some(Instant::now()),
        ..RunControl::default()
    };
    let routed = route_with(&d, &RouteOptions::default(), &control).unwrap();
    check(&d, &routed);
    assert_eq!(routed.stopped, Some(Stopped::Deadline));
    assert!(routed.tracks.is_empty());
    assert_eq!(routed.open.len(), 12);
}

#[test]
fn progress_tells_of_each_phase_and_no_more_often_than_asked() {
    let d = weave();
    let log: RefCell<Vec<Progress>> = RefCell::new(Vec::new());
    let heard = |p: &Progress| log.borrow_mut().push(*p);
    let mut control = RunControl {
        progress: Some(&heard),
        progress_interval: Duration::ZERO,
        ..RunControl::default()
    };
    let routed = route_with(&d, &RouteOptions::default(), &control).unwrap();
    let all = log.take();
    let mut phases: Vec<Phase> = all.iter().map(|p| p.phase).collect();
    phases.dedup();
    assert_eq!(
        phases,
        [
            Phase::Prepare,
            Phase::Global,
            Phase::Detail,
            Phase::Cleanup,
            Phase::Report
        ]
    );
    assert!(all.windows(2).all(|w| w[0].work <= w[1].work));
    let last = all.last().unwrap();
    assert_eq!((last.nets, last.nets_routed), (12, 12));
    assert_eq!((last.connections, last.connected), (12, 12));
    assert_eq!(last.work, routed.stats.work);
    // Every net is heard of as it is taken from the queue.
    assert!(all.iter().filter(|p| p.phase == Phase::Detail).count() >= 12);

    // With an interval the run never reaches, only the changes of phase are told.
    control.progress_interval = Duration::from_secs(3600);
    route_with(&d, &RouteOptions::default(), &control).unwrap();
    assert_eq!(log.borrow().len(), phases.len());
}

#[test]
fn a_net_that_cannot_be_routed_whole_keeps_its_tree_if_asked() {
    let d = walled_in();
    let lost = route_checked(&d, &RouteOptions::default());
    let options = RouteOptions {
        keep_partial: true,
        ..RouteOptions::default()
    };
    let kept = route_checked(&d, &options);
    check(&d, &kept);
    assert_eq!(kept.unrouted, [NetId(1)]);
    assert_eq!((lost.stats.islands, kept.stats.islands), ((4, 0), (4, 3)));
    assert_eq!((kept.stats.connections, kept.stats.connected), (4, 3));
    assert!(lost.tracks.is_empty() && !kept.tracks.is_empty());
    // One connection is open: to the pad walled in, from its neighbour.
    assert_eq!(kept.open.len(), 1);
    let open = &kept.open[0];
    assert_eq!(
        (open.net, open.from.pad, open.to.pad),
        (NetId(1), Some(3), Some(4))
    );
    assert_eq!(open.cause, lost.open[3].cause);
}

#[test]
fn open_connections_say_why() {
    // A pad walled in: as the net is routed today it fails whole, and four connections
    // are open for the one pad, in the order of the pads they end at, each from the pad
    // before. The last is to the pad shut in, by the four keepouts.
    let d = walled_in();
    let routed = route_checked(&d, &RouteOptions::default());
    check(&d, &routed);
    assert_eq!(routed.unrouted, [NetId(1)]);
    assert!(routed.tracks.is_empty());
    assert_eq!(routed.stats.islands, (4, 0));
    let ends: Vec<_> = routed.open.iter().map(|o| (o.from.pad, o.to.pad)).collect();
    let pairs: Vec<_> = (0..4).map(|k| (Some(k), Some(k + 1))).collect();
    assert_eq!(ends, pairs);
    let last = &routed.open[3];
    assert_eq!(last.to.at, Point::new(8 * MM, 0));
    assert_eq!(last.to.layers, LayerSet::first(1));
    let walls: Vec<Blocker> = (0..4)
        .map(|k| Blocker::Item(DesignItem::Keepout(k)))
        .collect();
    assert_eq!(last.cause, Cause::Enclosed { by: walls });
    // The others are not shut in, and the search for the net found no path.
    assert!(routed.open[..3].iter().all(|o| o.cause == Cause::NoPath));

    // No via, and pads on different layers.
    let mut d = design(2, vec![smd(0, 0, 1), smd(4 * MM, 0, 1)], 2);
    d.board.pads[1].layers = LayerSet::EMPTY.with(LayerId(1));
    d.nets[1].via = None;
    let routed = route_checked(&d, &RouteOptions::default());
    check(&d, &routed);
    assert_eq!(routed.open.len(), 1);
    assert_eq!(routed.open[0].cause, Cause::NoLayer);

    // A search that runs out of nodes.
    let d = design(1, vec![smd(0, 0, 1), smd(30 * MM, 0, 1)], 2);
    let options = RouteOptions {
        max_expansions: 20,
        ..RouteOptions::default()
    };
    let routed = route_checked(&d, &options);
    check(&d, &routed);
    assert_eq!(routed.open[0].cause, Cause::OutOfBudget);
    assert_eq!(routed.stopped, None);

    // Another net's route in the way: on one layer, one of two nets that cross is shut
    // off by the other's track, from edge to edge.
    let mut d = design(
        1,
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
    let routed = route_checked(&d, &RouteOptions::default());
    check(&d, &routed);
    assert_eq!(routed.open.len(), 1);
    let open = &routed.open[0];
    let other = NetId(3 - open.net.0);
    assert!(routed.tracks.iter().all(|t| t.net == Some(other)));
    assert_eq!(open.cause, Cause::NoPath);
}

#[test]
fn an_error_names_the_item_at_fault() {
    let mut d = weave();
    let track = |width| Track {
        start: Point::new(0, 0),
        end: Point::new(MM, 0),
        mid: None,
        width,
        layer: LayerId(0),
        net: None,
    };
    d.board.tracks = vec![track(100_000), track(0)];
    let error = route(&d, &RouteOptions::default()).unwrap_err();
    assert!(
        matches!(
            error,
            RouteError::Item {
                item: DesignItem::Track(1),
                ..
            }
        ),
        "{error:?}"
    );
    assert!(error.to_string().contains("track 1"), "{error}");
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn a_seed_gives_another_order_and_the_same_one_every_time() {
    // On one layer, one of two nets that cross is routed: which depends on the order.
    let mut d = design(
        1,
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
    let mut options = RouteOptions {
        rip_ups: 0,
        ..RouteOptions::default()
    };
    let plain = route_checked(&d, &options);
    let mut others = 0;
    for seed in 0..16 {
        options.seed = Some(seed);
        let seeded = route_checked(&d, &options);
        check(&d, &seeded);
        same(&seeded, &route(&d, &options).unwrap());
        assert_eq!(seeded.unrouted.len(), 1);
        others += usize::from(seeded.unrouted != plain.unrouted);
    }
    assert!(
        others > 0 && others < 16,
        "{others} of 16 seeds changed the order"
    );
}

#[test]
fn a_reroute_tags_what_it_kept_and_stops_as_a_route_does() {
    use super::eco::{self, Change};
    let d = weave();
    let before = route(&d, &RouteOptions::default()).unwrap();
    let change = Change::new(vec![NetId(1)], Vec::new());
    let after = eco::reroute(
        &d,
        &before.tracks,
        &before.vias,
        &change,
        &RouteOptions::default(),
    )
    .unwrap();
    assert!(after.routed.unrouted.is_empty());
    let r = &after.routed;
    assert_eq!(r.track_kinds.len(), r.tracks.len());
    for (t, k) in r.tracks.iter().zip(&r.track_kinds) {
        let rerouted = t.net == Some(NetId(1));
        assert_eq!(*k == CopperKind::Plain, rerouted, "{t:?} {k:?}");
        assert_eq!(*k == CopperKind::Kept, !rerouted);
    }
    // Stopped at once, it keeps what it kept and leaves the changed net open.
    let cancel = AtomicBool::new(true);
    let calls = Cell::new(0);
    let heard = |_: &Progress| calls.set(calls.get() + 1);
    let control = RunControl {
        cancel: Some(&cancel),
        progress: Some(&heard),
        ..RunControl::default()
    };
    let stopped = eco::reroute_with(
        &d,
        &before.tracks,
        &before.vias,
        &change,
        &RouteOptions::default(),
        &control,
    )
    .unwrap();
    assert_eq!(stopped.routed.stopped, Some(Stopped::Cancelled));
    assert_eq!(stopped.routed.unrouted, [NetId(1)]);
    assert_eq!(stopped.routed.open.len(), 1);
    assert!(
        stopped
            .routed
            .track_kinds
            .iter()
            .all(|k| *k == CopperKind::Kept)
    );
    assert!(calls.get() > 0);
    assert!(
        drc::check(&stopped.routed.board(&d), &d.rules)
            .unwrap()
            .is_empty()
    );
}

/// Nets across and down on one layer and a second layer too narrow for all of them: they
/// are in each other's way, and rip-up takes them out again and again.
fn crowded() -> Design {
    let mut d = weave();
    // The second layer only in a band through the middle.
    let band = |a: Point, b: Point| Keepout {
        shape: Shape::rect(Rect::new(a, b)).unwrap(),
        layers: LayerSet::EMPTY.with(LayerId(1)),
        tracks: true,
        vias: true,
        pads: false,
        except: Vec::new(),
    };
    d.board.keepouts.extend([
        band(Point::new(-MM, -MM), Point::new(15 * MM, 5 * MM)),
        band(Point::new(-MM, 9 * MM), Point::new(15 * MM, 15 * MM)),
    ]);
    d
}

#[test]
fn a_stopped_run_keeps_the_most_connections_it_had() {
    let d = crowded();
    let whole = route(&d, &RouteOptions::default()).unwrap();
    assert!(whole.stats.rip_ups > 0, "nothing was ripped up");
    let mut options = RouteOptions::default();
    let mut most = 0;
    for step in 0..=64 {
        options.work_budget = Some(whole.stats.work * step / 64);
        let r = route(&d, &options).unwrap();
        check(&d, &r);
        // More work never leaves fewer connections: the nets that rip-up had out when
        // the budget ran out are as they were before it took them out.
        assert!(
            r.stats.connected >= most,
            "{} connections with {:?}, {most} with less",
            r.stats.connected,
            options.work_budget
        );
        most = r.stats.connected;
    }
}

#[test]
fn a_run_stopped_by_its_flag_fills_its_zones_unless_told_not_to() {
    let d = super::zone_tests::plane_board();
    let cancel = AtomicBool::new(true);
    let control = RunControl {
        cancel: Some(&cancel),
        ..RunControl::default()
    };
    let filled = route_with(&d, &RouteOptions::default(), &control).unwrap();
    assert_eq!(filled.stopped, Some(Stopped::Cancelled));
    assert!(!filled.fills.is_empty());

    let control = RunControl {
        fill_on_stop: false,
        ..control
    };
    let bare = route_with(&d, &RouteOptions::default(), &control).unwrap();
    check(&d, &bare);
    assert_eq!(bare.stopped, Some(Stopped::Cancelled));
    assert!(bare.fills.is_empty());
    // A run that is not stopped fills them, whatever it was told, and so does one that
    // its work budget stops.
    cancel.store(false, Ordering::Relaxed);
    let whole = route_with(&d, &RouteOptions::default(), &control).unwrap();
    assert!(whole.stopped.is_none() && !whole.fills.is_empty());
    let options = RouteOptions {
        work_budget: Some(0),
        ..RouteOptions::default()
    };
    let spent = route_with(&d, &options, &control).unwrap();
    assert_eq!(spent.stopped, Some(Stopped::WorkBudget));
    assert!(!spent.fills.is_empty());
}

/// One layer, a wall across the middle with a gap one track wide, and three kinds of nets:
/// net 1 through the gap, pad to pad; net 2 with a pad on either side of the wall, whose
/// only way is the gap that net 1 has taken; and eight nets of two pads with nothing in
/// their way, each longer than net 2, so that they come after it.
fn one_gap() -> Design {
    let mut pads = vec![
        smd(20 * MM, 19 * MM, 1),
        smd(20 * MM, 21 * MM, 1),
        smd(18 * MM, 17 * MM, 2),
        smd(18 * MM, 23 * MM, 2),
    ];
    for k in 0..8 {
        pads.push(smd(5 * MM, (30 + k) * MM, 3 + k as u32));
        pads.push(smd(13 * MM, (30 + k) * MM, 3 + k as u32));
    }
    let mut d = design(1, pads, 11);
    d.outline = outline(Rect::new(Point::new(0, 0), Point::new(40 * MM, 40 * MM)));
    let wall = |x0: i64, x1: i64| Keepout {
        shape: Shape::rect(Rect::new(
            Point::new(x0, 19_900_000),
            Point::new(x1, 20_100_000),
        ))
        .unwrap(),
        layers: LayerSet::first(1),
        tracks: true,
        vias: true,
        pads: false,
        except: Vec::new(),
    };
    d.board.keepouts.push(wall(-MM, 19_850_000));
    d.board.keepouts.push(wall(20_150_000, 41 * MM));
    d
}

#[test]
fn a_net_with_no_way_in_its_corridor_waits_for_the_others_in_the_first_pass() {
    // Net 2 finds no way in its corridor, and none over the board, which it takes tens of
    // thousands of nodes to find out. It waits for the eight nets after it to have their
    // turn first: a budget that the search over the board would have spent routes them.
    let d = one_gap();
    let options = RouteOptions {
        work_budget: Some(12_000),
        ..RouteOptions::default()
    };
    let routed = route(&d, &options).unwrap();
    check(&d, &routed);
    assert_eq!(routed.stopped, Some(Stopped::WorkBudget));
    assert_eq!(routed.unrouted, [NetId(2)]);
    assert_eq!(routed.open.len(), 1);
    assert_eq!(routed.open[0].cause, Cause::Stopped);
    // Left to finish, the run says that it has no way.
    let whole = route(&d, &RouteOptions::default()).unwrap();
    check(&d, &whole);
    assert_eq!(whole.stopped, None);
    assert_eq!(whole.unrouted.len(), 1);
    assert!(whole.stats.work > 12_000, "{}", whole.stats.work);
}

#[test]
fn a_net_that_waited_keeps_what_it_reaches_before_it_crosses_anything() {
    // The same board with a third pad of net 2 on the near side of the wall, and a net
    // that keeps what it reaches. When its turn comes again, net 2 joins the two pads on
    // one side and finds no way through the gap that net 1 has taken, with nothing moved:
    // that is kept, and the search that may cross net 1 waits once more. A budget that
    // ends before that search leaves net 1 routed and net 2 with the connection it
    // made; it used to end with net 1 taken out for net 2.
    let mut d = one_gap();
    d.board.pads.push(smd(16 * MM, 17 * MM, 2));
    let joined = |r: &Routed| {
        r.tracks.iter().any(|t| {
            t.net == Some(NetId(2)) && t.start.y.max(t.end.y) < 19 * MM && t.start.x != t.end.x
        })
    };
    let options = RouteOptions {
        keep_partial: true,
        work_budget: Some(60_000),
        ..RouteOptions::default()
    };
    let routed = route(&d, &options).unwrap();
    check(&d, &routed);
    assert_eq!(routed.stopped, Some(Stopped::WorkBudget));
    assert!(joined(&routed), "{:?}", routed.tracks);
    assert_eq!(routed.unrouted, [NetId(2)]);
    assert_eq!(routed.open.len(), 1);
    // Left to finish, the run ends with one of the two nets through the gap, as before.
    let whole = route(
        &d,
        &RouteOptions {
            work_budget: Some(u64::MAX / 2),
            ..options
        },
    )
    .unwrap();
    check(&d, &whole);
    assert_eq!(whole.stopped, None);
    assert_eq!(whole.unrouted.len(), 1);
    assert_eq!(whole.open.len(), 1);
}

#[test]
fn a_part_of_a_run_ends_at_its_share_and_the_run_is_then_one_that_was_stopped() {
    use super::control::Run;
    // Nine tenths of a budget of work for the first part, and the rest for the next.
    let control = RunControl::default();
    let run = Run::new(&control, Some(1000));
    run.part(1.0, 0.9);
    assert!(!run.poll());
    assert_eq!(run.limits(u64::MAX).max_expansions, 900);
    run.charge(900);
    assert!(run.poll() && run.ending());
    assert_eq!(run.stopped(), None);
    run.part(1.0, 1.0);
    assert!(!run.poll());
    assert_eq!(run.limits(u64::MAX).max_expansions, 100);
    run.charge(40);
    run.settle();
    assert_eq!(run.stopped(), Some(Stopped::WorkBudget));
    assert_eq!(run.work(), 940);
    // A part that ends before its share is spent cuts nothing short.
    let run = Run::new(&control, Some(1000));
    run.part(1.0, 0.9);
    run.charge(10);
    run.part(1.0, 1.0);
    run.settle();
    assert_eq!(run.stopped(), None);
    assert!(!run.poll());
    // A share of the time to a deadline: none of it ends the part at once, and the run
    // goes on with the next until the deadline itself.
    let control = RunControl {
        deadline: Some(Instant::now() + Duration::from_secs(3600)),
        ..RunControl::default()
    };
    let run = Run::new(&control, None);
    run.part(0.0, 1.0);
    assert!(run.poll());
    assert_eq!(run.stopped(), None);
    run.part(1.0, 1.0);
    assert!(!run.poll());
    assert!(!run.short_of(Duration::from_secs(60)));
    assert!(run.short_of(Duration::from_secs(7200)));
    run.settle();
    assert_eq!(run.stopped(), Some(Stopped::Deadline));
}

#[test]
fn a_result_says_which_nets_were_tried() {
    let d = weave();
    let whole = route(&d, &RouteOptions::default()).unwrap();
    let all: Vec<NetId> = (1..=12).map(NetId).collect();
    assert_eq!(whole.tried, all);
    // A run stopped after three nets has tried those, and no more than the one it was at.
    let cancel = AtomicBool::new(false);
    let heard = |p: &Progress| {
        if p.nets_routed >= 3 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let control = RunControl {
        cancel: Some(&cancel),
        progress: Some(&heard),
        progress_interval: Duration::ZERO,
        ..RunControl::default()
    };
    let routed = route_with(&d, &RouteOptions::default(), &control).unwrap();
    assert_eq!(routed.stats.routed, 3);
    assert!((3..=4).contains(&routed.tried.len()), "{:?}", routed.tried);
    assert!(routed.tried.windows(2).all(|w| w[0] < w[1]));
    // The nets left open that are not among them were not looked at.
    assert!(routed.unrouted.iter().any(|n| !routed.tried.contains(n)));
}

#[test]
fn a_pad_shut_in_is_said_to_be_after_a_run_that_was_stopped_too() {
    // The run is cancelled once routing is over, at the start of cleanup: the net that
    // found no way has its pads looked at all the same.
    let d = walled_in();
    let cancel = AtomicBool::new(false);
    let heard = |p: &Progress| {
        if p.phase == Phase::Cleanup {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let control = RunControl {
        cancel: Some(&cancel),
        progress: Some(&heard),
        progress_interval: Duration::ZERO,
        ..RunControl::default()
    };
    let routed = route_with(&d, &RouteOptions::default(), &control).unwrap();
    check(&d, &routed);
    assert_eq!(routed.stopped, Some(Stopped::Cancelled));
    assert_eq!(routed.tried, [NetId(1)]);
    assert_eq!(routed.open.len(), 4);
    assert!(matches!(routed.open[3].cause, Cause::Enclosed { .. }));
    assert!(routed.open[..3].iter().all(|o| o.cause == Cause::NoPath));
}

#[test]
fn a_run_stopped_while_it_plans_on_its_zones_comes_back_with_nothing() {
    // The deadline is over before the zones are filled to tell the planes: the fill is
    // given up, nothing is routed, and the run says that it was stopped.
    let d = super::zone_tests::plane_board();
    let options = RouteOptions {
        plane_coverage: 0.25,
        ..RouteOptions::default()
    };
    let control = RunControl {
        deadline: Some(Instant::now()),
        fill_on_stop: false,
        ..RunControl::default()
    };
    let routed = route_with(&d, &options, &control).unwrap();
    assert_eq!(routed.stopped, Some(Stopped::Deadline));
    assert!(routed.tracks.is_empty() && routed.vias.is_empty() && routed.fills.is_empty());
    assert!(routed.tried.is_empty());
    assert!(!routed.open.is_empty());
}

/// A net straight across a board of one layer, and a longer one that has to cross it:
/// nothing passes the ends of the first, but it has room to go round a pad of the second.
fn crossed() -> Design {
    let pads = vec![
        smd(500_000, 0, 1),
        smd(3_500_000, 0, 1),
        smd(2 * MM, -1_600_000, 2),
        smd(2 * MM, 1_600_000, 2),
    ];
    let mut d = design(1, pads, 3);
    d.outline = outline(Rect::new(
        Point::new(0, -3_500_000),
        Point::new(4 * MM, 3_500_000),
    ));
    d
}

#[test]
fn a_run_with_a_budget_moves_a_net_aside_where_another_has_to_cross_it() {
    let d = crossed();
    // With no budget the first net is taken out for the second, and routed again.
    let plain = route_checked(&d, &RouteOptions::default());
    assert!(plain.unrouted.is_empty(), "{:?}", plain.unrouted);
    assert_eq!((plain.stats.rip_ups, plain.stats.shoved), (1, 0));
    // With one it is cut where the second crosses it and mended round the second's pad,
    // and nothing is taken out.
    let options = RouteOptions {
        work_budget: Some(10_000_000),
        ..RouteOptions::default()
    };
    let routed = route_checked(&d, &options);
    assert!(routed.unrouted.is_empty(), "{:?}", routed.unrouted);
    assert!(routed.stopped.is_none());
    assert_eq!((routed.stats.rip_ups, routed.stats.shoved), (0, 1));
    // The second net is straight, and the first goes round.
    let length = |net: u32| -> i64 {
        let of = |t: &&Track| t.net == Some(NetId(net));
        let long = |t: &Track| (t.end.x - t.start.x).abs().max((t.end.y - t.start.y).abs());
        routed.tracks.iter().filter(of).map(long).sum()
    };
    assert_eq!(length(2), 3_200_000);
    assert!(length(1) > 3 * MM);
}

#[test]
fn a_net_that_can_not_be_moved_aside_is_taken_out_as_before() {
    // The same board with no room round the second net's pads: the first net can't be
    // mended, everything is put back, and the second takes its place as with no budget.
    let mut d = crossed();
    d.outline = outline(Rect::new(
        Point::new(0, -2 * MM),
        Point::new(4 * MM, 2 * MM),
    ));
    let options = RouteOptions {
        work_budget: Some(10_000_000),
        ..RouteOptions::default()
    };
    let routed = route_checked(&d, &options);
    assert_eq!(routed.stats.shoved, 0);
    assert_eq!(routed.unrouted.len(), 1);
    assert_eq!(routed.stats.connected, 1);
}

#[test]
fn a_work_budget_that_is_not_spent_stops_nothing_on_a_board_with_planes() {
    // Routing has nineteen twentieths of a budget and the joining of zones' pieces the
    // rest. Cleanup, between them, has the work that routing left of its own share: with
    // none at all, a run was stopped as it began cleaning up, for a budget spent, after
    // the few thousand nodes of its routing, and never joined its zones' pieces.
    let d = super::zone_tests::plane_board();
    let plain = route_checked(&d, &RouteOptions::default());
    assert!(plain.unrouted.is_empty());
    let options = RouteOptions {
        work_budget: Some(20_000_000),
        ..RouteOptions::default()
    };
    let routed = route_checked(&d, &options);
    assert_eq!(routed.stopped, None);
    assert!(routed.unrouted.is_empty(), "{:?}", routed.unrouted);
    assert!(routed.stats.work < 1_000_000, "{}", routed.stats.work);
    assert_eq!(routed.stats.islands.0, routed.stats.islands.1);
    // And one that is spent in routing still stops the run, with the joining's share kept.
    let options = RouteOptions {
        work_budget: Some(1),
        ..RouteOptions::default()
    };
    let cut = route(&d, &options).unwrap();
    assert_eq!(cut.stopped, Some(Stopped::WorkBudget));
}

#[test]
fn a_result_says_which_open_nets_had_a_search_that_failed() {
    // A net with a pad walled in is open because its own search found no way.
    let routed = route_checked(&walled_in(), &RouteOptions::default());
    assert_eq!(routed.unrouted, [NetId(1)]);
    assert_eq!(routed.failed, [NetId(1)]);
    // Nothing failed where everything is routed, nor where the run was stopped before a
    // net had a search: what is open then is for a run that goes on.
    assert!(
        route_checked(&weave(), &RouteOptions::default())
            .failed
            .is_empty()
    );
    let options = RouteOptions {
        work_budget: Some(0),
        ..RouteOptions::default()
    };
    let cut = route(&weave(), &options).unwrap();
    assert_eq!(cut.stopped, Some(Stopped::WorkBudget));
    assert!(!cut.unrouted.is_empty());
    assert!(cut.failed.is_empty(), "{:?}", cut.failed);
}

#[test]
fn a_run_with_a_budget_settles_what_its_passes_leave_open_and_one_with_none_does_not() {
    // Net 2 has no way but the gap that net 1 is in, and net 1 no other: the passes leave
    // net 2 open, and the settling tries a chain for it, which is undone. The routes are
    // those of a run that settles nothing.
    let d = one_gap();
    let options = RouteOptions {
        work_budget: Some(50_000_000),
        ..RouteOptions::default()
    };
    let routed = route(&d, &options).unwrap();
    check(&d, &routed);
    assert_eq!(routed.stopped, None);
    assert_eq!(routed.unrouted, [NetId(2)]);
    let (tried, kept, made) = routed.stats.settled;
    assert!(tried >= 1, "{:?}", routed.stats);
    assert_eq!((kept, made), (0, 0));
    let plain = RouteOptions {
        settle: false,
        ..options
    };
    let unsettled = route(&d, &plain).unwrap();
    assert_eq!(unsettled.stats.settled, (0, 0, 0));
    assert_eq!(unsettled.stats.negotiated, (0, 0, 0));
    // A negotiation comes before the chains, unless it is turned off.
    assert!(routed.stats.negotiated.0 >= 1, "{:?}", routed.stats);
    let chains = RouteOptions {
        negotiate: false,
        ..options
    };
    assert_eq!(route(&d, &chains).unwrap().stats.negotiated, (0, 0, 0));
    assert_eq!(unsettled.tracks, routed.tracks);
    assert_eq!(unsettled.vias, routed.vias);
    // With neither a budget nor a deadline nothing is settled.
    let whole = route(&d, &RouteOptions::default()).unwrap();
    assert_eq!(whole.stats.settled, (0, 0, 0));
}
