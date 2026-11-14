// Boards are built up a field at a time, each test changing what it is about.
#![allow(clippy::field_reassign_with_default)]

use core::cmp::Ordering;

use proptest::prelude::*;

use super::clearance::{Filter, exact_below, filter};
use super::drc::{self, Board, DrcError, Item, Keepout, Pad, Track, Via, Violation};
use super::*;
use crate::cost::Cost;
use crate::id::NetId;

fn p(x: i64, y: i64) -> Point {
    Point::new(x, y)
}

fn circle(x: i64, y: i64, d: i64) -> Shape {
    Shape::circle(p(x, y), d).unwrap()
}

fn track(a: (i64, i64), b: (i64, i64), w: i64) -> Shape {
    Shape::track(p(a.0, a.1), p(b.0, b.1), w).unwrap()
}

fn rect(x0: i64, y0: i64, x1: i64, y1: i64) -> Shape {
    Shape::rect(Rect::new(p(x0, y0), p(x1, y1))).unwrap()
}

/// The smallest clearance a pair violates: just above their gap.
fn first_violated(a: &Shape, b: &Shape) -> i64 {
    (1..10_000).find(|&c| a.violates(b, c).unwrap()).unwrap()
}

// Hand-checked distances.

#[test]
fn tangent_circles_touch_and_violate_any_clearance() {
    // Centres 5 apart, radii 2 and 3.
    let (a, b) = (circle(0, 0, 4), circle(3, 4, 6));
    assert!(a.touches(&b));
    assert!(a.violates(&b, 1).unwrap());
    assert!(!a.violates(&b, 0).unwrap());
    let gap = a.distance(&b);
    assert!(gap.is_zero() && !gap.is_overlap() && gap.is_exact());
    assert_eq!(gap.cmp_to(0), Ordering::Equal);
    // One unit further and they don't touch.
    let c = circle(3, 4, 4);
    assert!(!a.touches(&c));
    assert_eq!(a.distance(&c).cmp_to(1), Ordering::Equal);
    assert!(!a.violates(&c, 1).unwrap());
    assert!(a.violates(&c, 2).unwrap());
}

#[test]
fn parallel_tracks_at_exactly_the_clearance() {
    let a = track((0, 0), (1000, 0), 200);
    for (y, expect) in [(399, true), (400, false), (401, false)] {
        let b = track((0, y), (1000, y), 200);
        assert_eq!(a.violates(&b, 200).unwrap(), expect, "y = {y}");
    }
    let b = track((0, 400), (1000, 400), 200);
    assert_eq!(a.distance(&b).cmp_to(200), Ordering::Equal);
    assert_eq!(first_violated(&a, &b), 201);
}

#[test]
fn odd_widths_give_half_unit_radii() {
    // Radii 100.5 and 99.5 add to 200, which leaves 200 of the 400.
    let a = track((0, 0), (1000, 0), 201);
    let b = track((0, 400), (1000, 400), 199);
    assert!(!a.violates(&b, 200).unwrap());
    assert!(a.violates(&b, 201).unwrap());
    // Radii 100.5 and 100 leave 199.5, between two whole clearances.
    let c = track((0, 400), (1000, 400), 200);
    assert!(!a.violates(&c, 199).unwrap());
    assert!(a.violates(&c, 200).unwrap());
    let gap = a.distance(&c);
    assert_eq!((gap.floor(), gap.ceil()), (199, 200));
    assert_eq!(gap.to_f64(), 199.5);
}

#[test]
fn diagonal_track_to_circle() {
    // (10, 0) is 10 / sqrt(2) = 7.07 from the line y = x, less radii of 1 and 1.
    let a = track((0, 0), (10, 10), 2);
    let b = circle(10, 0, 2);
    let gap = a.distance(&b);
    assert_eq!((gap.floor(), gap.ceil()), (5, 6));
    assert!(!a.violates(&b, 5).unwrap());
    assert!(a.violates(&b, 6).unwrap());
    // Past the end of the track, the distance is to its end.
    let c = circle(13, 14, 2);
    assert_eq!(a.distance(&c).cmp_to(3), Ordering::Equal);
}

#[test]
fn rectangle_pad_and_via() {
    let pad = rect(0, 0, 100, 100);
    assert!(pad.touches(&circle(150, 50, 100)));
    let via = circle(151, 50, 100);
    assert!(!pad.touches(&via));
    assert_eq!(pad.distance(&via).cmp_to(1), Ordering::Equal);
    assert!(!pad.violates(&via, 1).unwrap());
    assert!(pad.violates(&via, 2).unwrap());
    // Off a corner: (130, 140) is 50 from (100, 100), less a radius of 10.
    let corner = circle(130, 140, 20);
    assert_eq!(pad.distance(&corner).cmp_to(40), Ordering::Equal);
}

#[test]
fn rounded_corners_are_round() {
    // The corner's centre is (80, 80), with radius 20, and (140, 160) is 100 from it.
    let pad = Shape::round_rect(Rect::new(p(0, 0), p(100, 100)), 20).unwrap();
    let via = circle(140, 160, 20);
    assert_eq!(pad.distance(&via).cmp_to(70), Ordering::Equal);
    assert!(!pad.violates(&via, 70).unwrap());
    assert!(pad.violates(&via, 71).unwrap());
    // A square corner would be 72.1 from the centre, less 10.
    assert!(rect(0, 0, 100, 100).violates(&via, 70).unwrap());
    // A radius of half the shorter side makes an oval.
    let oval = Shape::round_rect(Rect::new(p(0, 0), p(100, 40)), 20).unwrap();
    assert_eq!(oval, track((20, 20), (80, 20), 40));
    assert!(Shape::round_rect(Rect::new(p(0, 0), p(100, 40)), 21).is_err());
}

#[test]
fn polygon_insides_count() {
    let square = [p(0, 0), p(1000, 0), p(1000, 1000), p(0, 1000)];
    let pad = Shape::polygon(&square, &[]).unwrap();
    let inside = circle(500, 500, 100);
    assert!(pad.touches(&inside));
    assert!(pad.distance(&inside).is_overlap());
    assert!(pad.violates(&inside, 1).unwrap());
    // Inside a hole, 100 from its edge, less a radius of 50.
    let hole = [p(400, 400), p(600, 400), p(600, 600), p(400, 600)];
    let ring = Shape::polygon(&square, &[&hole]).unwrap();
    assert!(!ring.touches(&inside));
    assert!(!ring.violates(&inside, 50).unwrap());
    assert!(ring.violates(&inside, 51).unwrap());
    // A polygon grown by 10 reaches 10 further.
    let grown = Shape::rounded_polygon(&square, &[&hole], 10).unwrap();
    assert!(!grown.violates(&inside, 40).unwrap());
    assert!(grown.violates(&inside, 41).unwrap());
}

#[test]
fn crossing_and_overlapping_tracks_touch() {
    let a = track((0, 0), (100, 100), 1);
    assert!(a.touches(&track((0, 100), (100, 0), 1)));
    assert!(a.touches(&track((50, 50), (200, 200), 1)));
    // A point track is a dot.
    let dot = track((50, 60), (50, 60), 2);
    assert!(!a.touches(&dot));
    assert!(a.touches(&track((50, 51), (50, 51), 2)));
}

#[test]
fn clearance_zero_is_never_violated() {
    let (a, b) = (circle(0, 0, 10), circle(0, 0, 10));
    assert!(!a.violates(&b, 0).unwrap());
    assert!(a.touches(&b));
}

// Arcs.

#[test]
fn arcs_are_conservative_by_their_error() {
    // A quarter circle of radius 10 000 about the origin, 100 wide.
    let arc = Shape::arc_track(p(10_000, 0), p(7071, 7071), p(0, 10_000), 100, 10).unwrap();
    assert!(!arc.is_exact());
    // A via at the origin, 100 across: the true gap is 10 000 - 50 - 50 = 9 900.
    let via = circle(0, 0, 100);
    let gap = arc.distance(&via);
    assert!(!gap.is_exact());
    // A true violation is never missed.
    assert!(arc.violates(&via, 9901).unwrap());
    // A gap within the error, 2 * (10 + 1), may be reported. Beyond it, it isn't.
    assert!(!arc.violates(&via, 9900 - 23).unwrap());
    // A full circle.
    let ring = Shape::arc_track(p(1000, 0), p(-1000, 0), p(1000, 0), 10, 1).unwrap();
    // Its outer edge is at 1005, so a true gap of 1 from a dot at 1007.
    assert!(ring.violates(&circle(0, 1007, 2), 2).unwrap());
    assert!(!ring.violates(&circle(0, 0, 2), 900).unwrap());
    // Collinear points make a straight track.
    let line = Shape::arc_track(p(0, 0), p(50, 0), p(100, 0), 10, 1).unwrap();
    assert!(line.violates(&circle(50, 20, 2), 15).unwrap());
}

/// The true distance from a point to the arc through three points, in `f64`, if the
/// points make an arc of moderate radius.
fn point_to_arc(pt: (f64, f64), a: Point, m: Point, b: Point) -> Option<f64> {
    let f = |q: Point| (q.x as f64, q.y as f64);
    let ((ax, ay), (mx, my), (bx, by)) = (f(a), f(m), f(b));
    let d = 2.0 * (ax * (my - by) + mx * (by - ay) + bx * (ay - my));
    if d.abs() < 1e3 {
        return None;
    }
    let sq = |x: f64, y: f64| x * x + y * y;
    let ux = (sq(ax, ay) * (my - by) + sq(mx, my) * (by - ay) + sq(bx, by) * (ay - my)) / d;
    let uy = (sq(ax, ay) * (bx - mx) + sq(mx, my) * (ax - bx) + sq(bx, by) * (mx - ax)) / d;
    let r = (ax - ux).hypot(ay - uy);
    if r > 1e6 {
        return None;
    }
    let angle = |x: f64, y: f64| (y - uy).atan2(x - ux);
    let tau = core::f64::consts::TAU;
    let ccw_from = |from: f64, to: f64| (to - from).rem_euclid(tau);
    let ccw = (mx - ax) * (by - ay) - (my - ay) * (bx - ax) > 0.0;
    let (lo, hi) = if ccw { (a, b) } else { (b, a) };
    let (lo, hi) = (f(lo), f(hi));
    let sweep = ccw_from(angle(lo.0, lo.1), angle(hi.0, hi.1));
    let on_arc = ccw_from(angle(lo.0, lo.1), angle(pt.0, pt.1)) <= sweep;
    let ends = (pt.0 - ax)
        .hypot(pt.1 - ay)
        .min((pt.0 - bx).hypot(pt.1 - by));
    Some(if on_arc {
        ((pt.0 - ux).hypot(pt.1 - uy) - r).abs().min(ends)
    } else {
        ends
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// Checks arcs against the true arc: violations are never missed, and false ones are
    /// within the error bound.
    #[test]
    fn arcs_never_miss_a_violation(
        a in (-20_000i64..20_000, -20_000i64..20_000),
        m in (-20_000i64..20_000, -20_000i64..20_000),
        b in (-20_000i64..20_000, -20_000i64..20_000),
        c in (-30_000i64..30_000, -30_000i64..30_000),
        width in 1i64..400,
        diameter in 1i64..400,
        tolerance in 1i64..50,
        clearance in 1i64..3000,
    ) {
        let (a, m, b) = (p(a.0, a.1), p(m.0, m.1), p(b.0, b.1));
        let Some(true_core) = point_to_arc((c.0 as f64, c.1 as f64), a, m, b) else {
            return Ok(());
        };
        let arc = Shape::arc_track(a, m, b, width, tolerance).unwrap();
        let via = circle(c.0, c.1, diameter);
        let gap = true_core - (width + diameter) as f64 / 2.0;
        let reported = arc.violates(&via, clearance).unwrap();
        if gap < clearance as f64 - 1e-3 {
            prop_assert!(reported, "missed: gap {gap}, clearance {clearance}");
        }
        if reported {
            let slack = 2.0 * (tolerance + 1) as f64;
            prop_assert!(gap < clearance as f64 + slack + 1e-3, "gap {gap} reported");
        }
        // The gap is a lower bound.
        prop_assert!(arc.distance(&via).to_f64() <= gap.max(0.0) + 1e-3);
    }
}

// Range and validity.

#[test]
fn out_of_range_input_is_an_error() {
    let far = p(MAX_COORD + 1, 0);
    let edge = p(MAX_COORD, -MAX_COORD);
    assert!(Point::checked(MAX_COORD, MAX_COORD).is_ok());
    assert_eq!(
        Point::checked(0, -MAX_COORD - 1),
        Err(GeomError::CoordinateOutOfRange {
            x: 0,
            y: -MAX_COORD - 1
        })
    );
    assert!(Shape::circle(far, 10).is_err());
    assert!(Shape::circle(edge, 10).is_ok());
    assert!(Shape::track(edge, far, 10).is_err());
    assert!(Shape::track(p(i64::MIN, i64::MAX), edge, 10).is_err());
    assert!(Shape::arc_track(p(0, 0), far, p(10, 0), 10, 1).is_err());
    assert!(Shape::polygon(&[p(0, 0), far, p(0, 10)], &[]).is_err());
    assert!(Shape::rect(Rect::new(p(0, 0), far)).is_err());
    assert!(Shape::rect(Rect::new(p(i64::MIN, i64::MIN), p(i64::MAX, i64::MAX))).is_err());
    // Dimensions.
    assert!(Shape::circle(edge, 0).is_err());
    assert!(Shape::circle(edge, -5).is_err());
    assert!(Shape::circle(edge, MAX_COORD + 1).is_err());
    assert!(Shape::track(edge, edge, 0).is_err());
    assert!(Shape::arc_track(p(0, 0), p(5, 5), p(10, 0), 10, 0).is_err());
    assert!(Shape::rect(Rect::new(p(0, 0), p(0, 10))).is_err());
    assert!(Shape::rounded_polygon(&[p(0, 0), p(9, 0), p(0, 9)], &[], -1).is_err());
    // Clearances.
    let a = circle(0, 0, 10);
    assert!(a.violates(&a, -1).is_err());
    assert!(a.violates(&a, MAX_COORD + 1).is_err());
    assert!(a.violates(&a, MAX_COORD).unwrap());
    assert!(ShapeIndex::new().within(&a, -1, &mut Vec::new()).is_err());
    // An arc that bulges far beyond the range.
    // A full circle whose points are all in range, but which reaches 1.2 times as far.
    let bulge = Shape::arc_track(p(0, 0), p(MAX_COORD, MAX_COORD), p(0, 0), 1, 1000);
    assert!(
        matches!(bulge, Err(GeomError::InvalidArc { .. })),
        "{bulge:?}"
    );
}

#[test]
fn invalid_polygons_are_rejected() {
    let bowtie = [p(0, 0), p(10, 10), p(10, 0), p(0, 10)];
    assert!(matches!(
        Shape::polygon(&bowtie, &[]),
        Err(GeomError::InvalidPolygon { .. })
    ));
    let square = [p(0, 0), p(10, 0), p(10, 10), p(0, 10)];
    let outside = [p(20, 20), p(30, 20), p(30, 30)];
    assert!(Shape::polygon(&square, &[&outside]).is_err());
    assert!(Shape::polygon(&[p(0, 0), p(1, 1)], &[]).is_err());
    // Clockwise is as good as counter-clockwise.
    let mut cw = square;
    cw.reverse();
    assert!(Shape::polygon(&cw, &[]).is_ok());
}

#[test]
fn extreme_coordinates_are_handled() {
    let a = circle(-MAX_COORD, -MAX_COORD, MAX_COORD);
    let b = circle(MAX_COORD, MAX_COORD, MAX_COORD);
    let gap = a.distance(&b);
    // 2 sqrt(2) 2^40 - 2^40.
    let expect = 2.0 * 2f64.sqrt() * MAX_COORD as f64 - MAX_COORD as f64;
    assert!((gap.to_f64() - expect).abs() < 1.0);
    assert_eq!(gap.floor(), expect.floor() as i64);
    assert!(!a.violates(&b, MAX_COORD).unwrap());
    let t = track((-MAX_COORD, -MAX_COORD), (MAX_COORD, MAX_COORD), 1);
    assert!(t.touches(&a) && t.touches(&b));
    // The other diagonal is sqrt(2) 2^40 from each centre, less 2^40 / 2 and a half.
    let u = track((-MAX_COORD, MAX_COORD), (MAX_COORD, -MAX_COORD), 1);
    let expect = 2f64.sqrt() * MAX_COORD as f64 - (MAX_COORD as f64 + 1.0) / 2.0;
    assert_eq!(u.distance(&a).floor(), expect.floor() as i64);
    assert!(gap.cmp_to(i64::MAX) == Ordering::Less);
}

// Filter against the exact answer, and both against brute force.

fn any_shape(span: i64) -> impl Strategy<Value = Shape> {
    let pt = move || (-span..span, -span..span).prop_map(|(x, y)| p(x, y));
    let size = move || 1..span.max(2);
    prop_oneof![
        (pt(), size()).prop_map(|(c, d)| Shape::circle(c, d).unwrap()),
        (pt(), pt(), size()).prop_map(|(a, b, w)| Shape::track(a, b, w).unwrap()),
        (pt(), size(), size(), 0..span.max(2) / 4).prop_map(|(c, w, h, r)| {
            let r = r.min(w.min(h) / 2);
            Shape::round_rect(Rect::new(c, p(c.x + w, c.y + h)), r).unwrap()
        }),
        (pt(), pt(), pt(), 0..3i64).prop_filter_map("degenerate triangle", |(a, b, c, r)| {
            Shape::rounded_polygon(&[a, b, c], &[], r).ok()
        }),
        (pt(), pt(), pt(), size()).prop_filter_map("bad arc", move |(a, m, b, w)| {
            Shape::arc_track(a, m, b, w, 1 + span / 50).ok()
        }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// The filter, where it answers, agrees with polyclip, and `violates`, `touches` and
    /// `distance` all agree, the last computed by brute force over every pair of edges.
    #[test]
    fn filter_agrees_with_exact(
        (a, b, span) in prop_oneof![Just(30i64), Just(3000), Just(MAX_COORD / 2)]
            .prop_flat_map(|span| (any_shape(span), any_shape(span), Just(span))),
        clearance_frac in 0.0..1.5f64,
    ) {
        let clearance = ((span as f64) * clearance_frac) as i64;
        let m = 2 * clearance + a.radius2 + b.radius2 + a.error2 + b.error2;
        let exact = exact_below(&a, &b, m);
        match filter(&a, &b, m) {
            Filter::Below => prop_assert!(exact),
            Filter::NotBelow => prop_assert!(!exact),
            Filter::Unknown => {}
        }
        let violates = a.violates(&b, clearance).unwrap();
        prop_assert_eq!(violates, clearance > 0 && exact);
        prop_assert_eq!(violates, b.violates(&a, clearance).unwrap());
        let gap = a.distance(&b);
        prop_assert_eq!(violates, clearance > 0 && gap.cmp_to(clearance) == Ordering::Less);
        prop_assert_eq!(a.touches(&b), gap.is_zero());
        prop_assert!(gap.floor() <= gap.ceil() && gap.ceil() - gap.floor() <= 1);
        prop_assert!(gap.cmp_to(gap.floor()) != Ordering::Less);
        prop_assert!((gap.to_f64() - gap.floor() as f64).abs() <= 1.0 + gap.to_f64() * 1e-12);
    }

    /// Near the boundary, where the filter must step aside, `violates` is exact: one unit
    /// either way of a gap changes the answer.
    #[test]
    fn exact_at_every_clearance(
        a in any_shape(200),
        b in any_shape(200),
    ) {
        let gap = a.distance(&b);
        let floor = gap.floor();
        for c in floor.saturating_sub(2).max(1)..floor + 3 {
            let expect = gap.cmp_to(c) == Ordering::Less;
            prop_assert_eq!(a.violates(&b, c).unwrap(), expect, "clearance {}", c);
        }
    }
}

// The index.

fn any_rect(span: i64) -> impl Strategy<Value = Rect> {
    (-span..span, -span..span, 0..span / 4, 0..span / 4)
        .prop_map(|(x, y, w, h)| Rect::new(p(x, y), p(x + w, y + h)))
}

#[derive(Clone, Debug)]
enum Op {
    Insert(Rect),
    Remove(usize),
    Query(Rect),
}

fn any_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => any_rect(1000).prop_map(Op::Insert),
        2 => any::<usize>().prop_map(Op::Remove),
        2 => any_rect(1000).prop_map(Op::Query),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Checks every query against a linear scan, through bulk loads, inserts and removes.
    #[test]
    fn index_matches_brute_force(
        initial in proptest::collection::vec(any_rect(1000), 0..200),
        ops in proptest::collection::vec(any_op(), 0..400),
    ) {
        let mut index = SpatialIndex::bulk_load(initial.iter().copied());
        let mut live: Vec<(EntryId, Rect)> = index.ids().zip(initial.iter().copied()).collect();
        prop_assert_eq!(live.len(), initial.len());
        let mut out = Vec::new();
        let check = |index: &SpatialIndex, live: &[(EntryId, Rect)], q: &Rect, out: &mut Vec<EntryId>| {
            index.query(q, out);
            let mut expect: Vec<EntryId> =
                live.iter().filter(|(_, r)| r.intersects(q)).map(|(id, _)| *id).collect();
            expect.sort_unstable();
            *out == expect
        };
        for op in ops {
            match op {
                Op::Insert(r) => {
                    let id = index.insert(r);
                    prop_assert!(live.iter().all(|(other, _)| *other != id));
                    live.push((id, r));
                }
                Op::Remove(k) if !live.is_empty() => {
                    let (id, _) = live.swap_remove(k % live.len());
                    prop_assert!(index.remove(id));
                    prop_assert!(!index.remove(id));
                    prop_assert_eq!(index.rect(id), None);
                }
                Op::Remove(_) => {}
                Op::Query(q) => prop_assert!(check(&index, &live, &q, &mut out)),
            }
            prop_assert_eq!(index.len(), live.len());
        }
        // Everything, and each entry by its own box.
        let all = Rect::new(p(-MAX_COORD, -MAX_COORD), p(MAX_COORD, MAX_COORD));
        prop_assert!(check(&index, &live, &all, &mut out));
        for (id, r) in &live {
            prop_assert_eq!(index.rect(*id), Some(*r));
            index.query(r, &mut out);
            prop_assert!(out.contains(id));
        }
        let mut ids: Vec<EntryId> = live.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        prop_assert_eq!(index.ids().collect::<Vec<_>>(), ids);
    }

    /// Checks clearance queries against every shape.
    #[test]
    fn shape_queries_match_brute_force(
        shapes in proptest::collection::vec(any_shape(2000), 1..60),
        probe in any_shape(2000),
        remove in proptest::collection::vec(any::<usize>(), 0..20),
        clearance in 0i64..500,
    ) {
        let mut index = ShapeIndex::bulk_load(shapes.iter().cloned());
        for k in remove {
            let ids: Vec<EntryId> = index.ids().collect();
            if !ids.is_empty() {
                prop_assert!(index.remove(ids[k % ids.len()]).is_some());
            }
        }
        let mut out = Vec::new();
        index.within(&probe, clearance, &mut out).unwrap();
        let expect: Vec<EntryId> = index
            .ids()
            .filter(|&id| probe.violates(index.get(id).unwrap(), clearance).unwrap())
            .collect();
        prop_assert_eq!(out, expect);
    }
}

#[test]
fn index_identifiers_are_deterministic_and_reused() {
    let r = |x| Rect::new(p(x, 0), p(x + 5, 5));
    let run = || {
        let mut index = SpatialIndex::new();
        let ids: Vec<EntryId> = (0..100).map(|i| index.insert(r(i * 3))).collect();
        for id in ids.iter().step_by(3) {
            index.remove(*id);
        }
        let again = index.insert(r(1000));
        let mut out = Vec::new();
        index.query(&Rect::new(p(0, 0), p(2000, 5)), &mut out);
        (again, out)
    };
    let (again, out) = run();
    assert_eq!(run(), (again, out.clone()));
    // The last slot freed is the first reused, with a new generation.
    assert_eq!(again.slot(), 99);
    assert_eq!(out.len(), 67);
    assert!(out.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn index_takes_any_rectangle() {
    let huge = Rect::new(p(i64::MIN, i64::MIN), p(i64::MAX, i64::MAX));
    let mut index = SpatialIndex::new();
    let mut ids: Vec<EntryId> = (0..100).map(|_| index.insert(huge)).collect();
    ids.extend((0..100).map(|i| index.insert(Rect::new(p(i, 0), p(i, 0)))));
    let mut out = Vec::new();
    index.query(&huge, &mut out);
    assert_eq!(out.len(), 200);
    index.query(&Rect::new(p(5, 0), p(5, 0)), &mut out);
    assert_eq!(out.len(), 101);
    for id in ids {
        assert!(index.remove(id));
    }
    assert!(SpatialIndex::bulk_load([huge; 50]).len() == 50);
}

#[test]
fn index_survives_emptying_and_refilling() {
    let mut index = SpatialIndex::bulk_load((0..500).map(|i| Rect::new(p(i, i), p(i + 2, i + 2))));
    let ids: Vec<EntryId> = index.ids().collect();
    for id in &ids {
        assert!(index.remove(*id));
    }
    assert!(index.is_empty());
    let mut out = Vec::new();
    index.query(&Rect::new(p(-10, -10), p(1000, 1000)), &mut out);
    assert!(out.is_empty());
    for i in 0..500 {
        index.insert(Rect::new(p(-i, 0), p(-i, 0)));
    }
    index.query(&Rect::new(p(-10, 0), p(0, 0)), &mut out);
    assert_eq!(out.len(), 11);
}

// Rules.

fn via_def(name: &str, drill: i64, ring: i64, from: u8, to: u8) -> ViaDef {
    ViaDef {
        name: name.into(),
        drill,
        annular_ring: ring,
        from: LayerId(from),
        to: LayerId(to),
        cost: Cost::new(10.0).unwrap(),
    }
}

fn four_layers() -> RuleSpec {
    let mut spec = RuleSpec::new(&["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"], 150, 150);
    spec.vias = vec![
        via_def("through", 300, 150, 0, 3),
        via_def("micro", 100, 75, 0, 1),
    ];
    spec.default_via = Some(ViaId(0));
    spec
}

fn overrides(track_width: Option<i64>, clearance: Option<i64>) -> Overrides {
    Overrides {
        track_width,
        clearance,
    }
}

#[test]
fn rule_precedence() {
    let mut spec = four_layers();
    spec.layer_overrides = vec![
        (LayerId(1), overrides(Some(100), Some(100))),
        (LayerId(2), overrides(None, Some(120))),
    ];
    spec.classes = vec![
        NetClass {
            name: "power".into(),
            nets: vec![NetId(1), NetId(2)],
            overrides: overrides(Some(500), None),
            layers: vec![(LayerId(1), overrides(Some(400), None))],
            via: None,
        },
        NetClass {
            name: "hv".into(),
            nets: vec![NetId(3)],
            overrides: overrides(None, Some(800)),
            layers: vec![(LayerId(3), overrides(None, Some(900)))],
            via: Some(ViaId(1)),
        },
    ];
    let rules = Rules::new(spec).unwrap();
    let (power, hv, plain) = (Some(NetId(2)), Some(NetId(3)), Some(NetId(9)));
    let w = |net, l| rules.track_width(net, LayerId(l));
    let c = |net, l| rules.clearance(net, LayerId(l));
    // Class on the layer, class, layer, default.
    assert_eq!(
        [w(power, 1), w(power, 0), w(plain, 1), w(plain, 0)],
        [400, 500, 100, 150]
    );
    assert_eq!(
        [c(hv, 3), c(hv, 1), c(plain, 1), c(plain, 0)],
        [900, 800, 100, 150]
    );
    // A field a class leaves alone falls through to the layer.
    assert_eq!([c(power, 1), c(power, 2), w(hv, 1)], [100, 120, 100]);
    // No net, or no class, is the board's.
    assert_eq!([c(None, 2), w(None, 0)], [120, 150]);
    assert_eq!(rules.clearance_between(hv, power, LayerId(1)), 800);
    assert_eq!(rules.clearance_between(None, power, LayerId(2)), 120);
    assert_eq!(rules.max_clearance(), 900);
    assert_eq!(rules.class_of(NetId(3)), Some(ClassId(1)));
    assert_eq!(rules.class_of(NetId(9)), None);
    assert_eq!(rules.via(hv).unwrap().name, "micro");
    assert_eq!(rules.via(power).unwrap().name, "through");
    assert_eq!(rules.via(power).unwrap().diameter(), 600);
    assert_eq!(rules.via(None).unwrap().layers(), LayerSet::first(4));
    assert_eq!(rules.layer("In2.Cu"), Some(LayerId(2)));
    assert_eq!(rules.layer("nope"), None);
}

#[test]
fn rules_are_validated() {
    let bad = |edit: &dyn Fn(&mut RuleSpec)| {
        let mut spec = four_layers();
        edit(&mut spec);
        Rules::new(spec).unwrap_err()
    };
    assert_eq!(bad(&|s| s.layers.clear()), RuleError::LayerCount(0));
    assert_eq!(
        bad(&|s| s.layers = (0..65).map(|i| i.to_string()).collect()),
        RuleError::LayerCount(65)
    );
    assert!(matches!(
        bad(&|s| s.layers[1] = "F.Cu".into()),
        RuleError::DuplicateName { what: "layer", .. }
    ));
    assert!(matches!(
        bad(&|s| s.track_width = 0),
        RuleError::InvalidValue { .. }
    ));
    assert!(matches!(
        bad(&|s| s.clearance = -1),
        RuleError::InvalidValue { .. }
    ));
    assert!(matches!(
        bad(&|s| s.hole_to_hole = MAX_COORD + 1),
        RuleError::InvalidValue { .. }
    ));
    assert!(matches!(
        bad(&|s| s.vias[0].drill = 0),
        RuleError::InvalidValue { .. }
    ));
    assert!(matches!(
        bad(&|s| s.vias[0].annular_ring = i64::MAX),
        RuleError::InvalidValue { .. }
    ));
    assert!(matches!(
        bad(&|s| s.vias[1].name = "through".into()),
        RuleError::DuplicateName { what: "via", .. }
    ));
    assert!(matches!(
        bad(&|s| s.vias[0].to = LayerId(4)),
        RuleError::InvalidSpan { .. }
    ));
    assert!(matches!(
        bad(&|s| s.vias[0].from = LayerId(3)),
        RuleError::InvalidSpan { .. }
    ));
    assert!(matches!(
        bad(&|s| s.default_via = Some(ViaId(2))),
        RuleError::UnknownVia { .. }
    ));
    assert!(matches!(
        bad(&|s| s.layer_overrides = vec![(LayerId(4), Overrides::default())]),
        RuleError::UnknownLayer { .. }
    ));
    assert!(matches!(
        bad(&|s| s.layer_overrides = vec![(LayerId(1), Overrides::default()); 2]),
        RuleError::DuplicateLayer { .. }
    ));
    assert!(matches!(
        bad(&|s| s.layer_overrides = vec![(LayerId(1), overrides(Some(-3), None))]),
        RuleError::InvalidValue { .. }
    ));
    let class = |name: &str, net| NetClass {
        name: name.into(),
        nets: vec![NetId(net)],
        ..NetClass::default()
    };
    assert!(matches!(
        bad(&|s| s.classes = vec![class("a", 1), class("b", 1)]),
        RuleError::NetInTwoClasses { net: NetId(1), .. }
    ));
    assert!(matches!(
        bad(&|s| s.classes = vec![class("a", 1), class("a", 2)]),
        RuleError::DuplicateName { what: "class", .. }
    ));
    assert!(matches!(
        bad(&|s| s.classes = vec![NetClass {
            via: Some(ViaId(7)),
            ..class("a", 1)
        }]),
        RuleError::UnknownVia { .. }
    ));
    assert!(matches!(
        bad(&|s| s.classes = vec![NetClass {
            layers: vec![(LayerId(9), Overrides::default())],
            ..class("a", 1)
        }]),
        RuleError::UnknownLayer { .. }
    ));
    // A net listed twice in one class is fine.
    let mut spec = four_layers();
    spec.classes = vec![NetClass {
        nets: vec![NetId(1), NetId(1)],
        ..class("a", 1)
    }];
    assert!(Rules::new(spec).is_ok());
    // Every error displays.
    assert!(!bad(&|s| s.layers.clear()).to_string().is_empty());
}

// The DRC.

fn two_layer_rules() -> Rules {
    let mut spec = RuleSpec::new(&["F.Cu", "B.Cu"], 100, 100);
    spec.layer_overrides = vec![(LayerId(1), overrides(None, Some(300)))];
    spec.classes = vec![NetClass {
        name: "wide".into(),
        nets: vec![NetId(5)],
        overrides: overrides(Some(400), None),
        ..NetClass::default()
    }];
    spec.min_drill = 200;
    spec.min_annular_ring = 100;
    spec.hole_to_hole = 250;
    Rules::new(spec).unwrap()
}

fn t(a: (i64, i64), b: (i64, i64), width: i64, layer: u8, net: Option<u32>) -> Track {
    Track {
        start: p(a.0, a.1),
        end: p(b.0, b.1),
        mid: None,
        width,
        layer: LayerId(layer),
        net: net.map(NetId),
    }
}

fn v(x: i64, y: i64, drill: i64, diameter: i64, net: Option<u32>) -> Via {
    Via {
        at: p(x, y),
        drill,
        diameter,
        from: LayerId(0),
        to: LayerId(1),
        net: net.map(NetId),
    }
}

#[test]
fn drc_reports_every_kind() {
    let rules = two_layer_rules();
    let mut board = Board::default();
    board.tracks = vec![
        // 0 and 1 cross: a short.
        t((0, 0), (1000, 0), 100, 0, Some(1)),
        t((500, -500), (500, 500), 100, 0, Some(2)),
        // 2 is 50 from 0, under the clearance of 100, and 10 from 1.
        t((0, 150), (390, 150), 100, 0, Some(3)),
        // 3 is of net 1, overlapping 0, and exactly 100 from 2 and from 1.
        t((0, -50), (300, -50), 100, 0, Some(1)),
        // 4 is too narrow for its class, which wants 400.
        t((0, 5000), (100, 5000), 300, 0, Some(5)),
        // 5 is in the keepout.
        t((10_000, 0), (10_100, 0), 100, 1, Some(6)),
    ];
    board.vias = vec![
        // A small drill, and a ring of only 50.
        v(20_000, 0, 100, 200, Some(7)),
        // Holes 150 apart, under the hole-to-hole of 250, of one net.
        v(30_000, 0, 300, 500, Some(8)),
        v(30_450, 0, 300, 500, Some(8)),
    ];
    board.pads = vec![Pad {
        shape: rect(40_000, 0, 40_100, 100),
        layers: LayerSet::first(2),
        hole: None,
        net: Some(NetId(9)),
        clearance: None,
        stack: Vec::new(),
    }];
    board.keepouts = vec![Keepout {
        shape: rect(9_000, -1000, 11_000, 1000),
        layers: LayerSet::EMPTY.with(LayerId(1)),
        tracks: true,
        vias: false,
        pads: true,
        except: Vec::new(),
    }];
    let found = drc::check(&board, &rules).unwrap();
    let mut it = found.iter();
    assert!(matches!(
        it.next(),
        Some(Violation::Short {
            a: Item::Track(0),
            b: Item::Track(1),
            layer: LayerId(0)
        })
    ));
    match it.next() {
        Some(Violation::Clearance {
            a: Item::Track(0),
            b: Item::Track(2),
            layer: LayerId(0),
            required: 100,
            gap,
        }) => {
            assert_eq!(gap.cmp_to(50), Ordering::Equal);
        }
        other => panic!("{other:?}"),
    }
    match it.next() {
        Some(Violation::Clearance {
            a: Item::Track(1),
            b: Item::Track(2),
            gap,
            ..
        }) => {
            assert_eq!(gap.cmp_to(10), Ordering::Equal);
        }
        other => panic!("{other:?}"),
    }
    let next = it.next();
    assert!(
        matches!(
            next,
            Some(Violation::TrackWidth {
                track: 4,
                required: 400
            })
        ),
        "{next:?}"
    );
    assert!(matches!(
        it.next(),
        Some(Violation::Drill {
            via: 0,
            required: 200
        })
    ));
    assert!(matches!(
        it.next(),
        Some(Violation::AnnularRing {
            via: 0,
            required: 100
        })
    ));
    match it.next() {
        Some(Violation::HoleToHole {
            a: Item::Via(1),
            b: Item::Via(2),
            required: 250,
            gap,
        }) => {
            assert_eq!(gap.cmp_to(150), Ordering::Equal);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        it.next(),
        Some(Violation::Keepout {
            keepout: 0,
            item: Item::Track(5),
            layer: LayerId(1)
        })
    ));
    assert_eq!(it.next(), None, "{found:#?}");
}

#[test]
fn drc_pairs_are_reported_once_on_the_first_layer() {
    let rules = two_layer_rules();
    let mut board = Board::default();
    // A via on both layers, a track 150 from it on each: a violation only of the 300 on
    // the bottom layer.
    board.vias = vec![v(0, 0, 300, 600, Some(1))];
    board.tracks = vec![
        t((-1000, 500), (1000, 500), 100, 0, Some(2)),
        t((-1000, 500), (1000, 500), 100, 1, Some(2)),
    ];
    let found = drc::check(&board, &rules).unwrap();
    assert!(matches!(
        found[..],
        [Violation::Clearance {
            a: Item::Track(1),
            b: Item::Via(0),
            layer: LayerId(1),
            required: 300,
            ..
        }]
    ));
    // A pad on both layers, with a clearance violation on top and a short below: one
    // short.
    board.vias.clear();
    board.pads = vec![Pad {
        shape: rect(-100, 0, 100, 400),
        layers: LayerSet::first(2),
        hole: None,
        net: Some(NetId(1)),
        clearance: None,
        stack: Vec::new(),
    }];
    board.tracks[0].start.y = 520;
    board.tracks[0].end.y = 520;
    board.tracks[1].start.y = 450;
    board.tracks[1].end.y = 450;
    let found = drc::check(&board, &rules).unwrap();
    assert!(
        matches!(
            found[..],
            [
                Violation::Short {
                    a: Item::Track(1),
                    b: Item::Pad(0),
                    layer: LayerId(1)
                },
                Violation::Clearance {
                    a: Item::Track(0),
                    b: Item::Pad(0),
                    layer: LayerId(0),
                    ..
                },
            ]
        ),
        "{found:#?}"
    );
    // A short below replaces a clearance violation above.
    board.tracks.truncate(1);
    board.tracks[0].start.y = 450;
    board.tracks[0].end.y = 450;
    board
        .tracks
        .push(t((-1000, 520), (1000, 520), 100, 0, Some(2)));
    board.tracks[0].layer = LayerId(1);
    board.pads[0].shape = rect(-100, 0, 100, 400);
    let found = drc::check(&board, &rules).unwrap();
    assert!(
        matches!(
            found[..],
            [
                Violation::Short {
                    a: Item::Track(0),
                    b: Item::Pad(0),
                    layer: LayerId(1)
                },
                Violation::Clearance {
                    a: Item::Track(1),
                    b: Item::Pad(0),
                    layer: LayerId(0),
                    ..
                },
            ]
        ),
        "{found:#?}"
    );
    // One via through both layers, a clearance violation on top and touching below.
    let mut board = Board::default();
    board.vias = vec![v(0, 0, 300, 600, Some(1))];
    board.tracks = vec![];
    board.pads = vec![Pad {
        shape: rect(320, -100, 400, 100),
        layers: LayerSet::first(1),
        hole: None,
        net: Some(NetId(2)),
        clearance: None,
        stack: Vec::new(),
    }];
    board.pads.push(Pad {
        shape: rect(300, -100, 400, 100),
        layers: LayerSet::EMPTY.with(LayerId(1)),
        hole: None,
        net: Some(NetId(2)),
        clearance: None,
        stack: Vec::new(),
    });
    let found = drc::check(&board, &rules).unwrap();
    assert!(
        matches!(
            found[..],
            [
                Violation::Short {
                    a: Item::Via(0),
                    b: Item::Pad(1),
                    layer: LayerId(1)
                },
                Violation::Clearance {
                    a: Item::Via(0),
                    b: Item::Pad(0),
                    layer: LayerId(0),
                    ..
                },
            ]
        ),
        "{found:#?}"
    );
}

#[test]
fn drc_nets() {
    let rules = two_layer_rules();
    let mut board = Board::default();
    // Overlapping copper of one net is fine. Copper of no net is connected to nothing.
    board.tracks = vec![
        t((0, 0), (100, 0), 100, 0, Some(1)),
        t((50, 0), (150, 0), 100, 0, Some(1)),
        t((0, 1000), (100, 1000), 100, 0, None),
        t((50, 1000), (150, 1000), 100, 0, None),
    ];
    let found = drc::check(&board, &rules).unwrap();
    assert!(matches!(
        found[..],
        [Violation::Short {
            a: Item::Track(2),
            b: Item::Track(3),
            ..
        }]
    ));
}

#[test]
fn drc_keepouts_by_kind_and_boundary() {
    let rules = two_layer_rules();
    let mut board = Board::default();
    board.keepouts = vec![Keepout {
        shape: rect(0, 0, 1000, 1000),
        layers: LayerSet::first(2),
        tracks: false,
        vias: true,
        pads: false,
        except: Vec::new(),
    }];
    // A track and a pad inside don't count, nor a via 1 away. A via touching does.
    board.tracks = vec![t((100, 100), (200, 200), 100, 0, Some(1))];
    board.pads = vec![Pad {
        shape: rect(500, 500, 600, 600),
        layers: LayerSet::first(1),
        hole: None,
        net: Some(NetId(2)),
        clearance: None,
        stack: Vec::new(),
    }];
    board.vias = vec![
        v(1301, 500, 300, 600, Some(3)),
        v(-300, 500, 300, 600, Some(4)),
    ];
    let found = drc::check(&board, &rules).unwrap();
    assert!(
        matches!(
            found[..],
            [Violation::Keepout {
                keepout: 0,
                item: Item::Via(1),
                layer: LayerId(0)
            }]
        ),
        "{found:#?}"
    );
}

#[test]
fn drc_mask_bridges_of_a_pads_opening() {
    let rules = two_layer_rules();
    let mut board = Board::default();
    board.pads = vec![Pad {
        shape: rect(0, 0, 1000, 1000),
        layers: LayerSet::first(1),
        hole: None,
        net: Some(NetId(1)),
        clearance: None,
        stack: Vec::new(),
    }];
    // The pad's opening reaches 500 past it on top.
    board.masks = vec![drc::MaskOpening {
        shape: rect(0, 0, 1000, 1000),
        layer: LayerId(0),
        margin: 500,
        net: Some(NetId(1)),
    }];
    board.tracks = vec![
        // Its own net's track, in the opening.
        t((500, 500), (500, 3000), 100, 0, Some(1)),
        // Another net's, 400 off the pad, in the opening, and one 600 off, out of it.
        t((-450, -2000), (-450, 3000), 100, 0, Some(2)),
        t((1650, -2000), (1650, 3000), 100, 0, Some(3)),
        // Another net's under the opening, on the bottom.
        t((500, -2000), (500, 3000), 100, 1, Some(4)),
    ];
    let found: Vec<Violation> = drc::check(&board, &rules)
        .unwrap()
        .into_iter()
        .filter(|v| matches!(v, Violation::MaskBridge { .. }))
        .collect();
    assert_eq!(
        found,
        [Violation::MaskBridge {
            opening: 0,
            item: Item::Track(1),
            layer: LayerId(0)
        }]
    );
}

#[test]
fn drc_mask_bridges_of_an_opening_of_no_net() {
    let rules = two_layer_rules();
    let mut board = Board::default();
    board.masks = vec![drc::MaskOpening {
        shape: rect(0, 0, 10_000, 10_000),
        layer: LayerId(0),
        margin: 0,
        net: None,
    }];
    let pad = |x: i64, net: Option<u32>| Pad {
        shape: rect(x, 1000, x + 1000, 2000),
        layers: LayerSet::first(1),
        hole: None,
        net: net.map(NetId),
        clearance: None,
        stack: Vec::new(),
    };
    // One net, and copper of none, bridge nothing alone.
    board.pads = vec![pad(1000, Some(1)), pad(3000, Some(1))];
    let bridges = |board: &Board| -> Vec<Item> {
        drc::check(board, &rules)
            .unwrap()
            .into_iter()
            .filter_map(|v| match v {
                Violation::MaskBridge { item, .. } => Some(item),
                _ => None,
            })
            .collect()
    };
    assert!(bridges(&board).is_empty());
    board.pads = vec![pad(1000, None), pad(3000, None)];
    assert!(bridges(&board).is_empty());
    // A second net bridges every item with a net.
    board.pads = vec![pad(1000, Some(1)), pad(3000, Some(2)), pad(5000, None)];
    assert_eq!(bridges(&board), [Item::Pad(0), Item::Pad(1)]);
    // And so does a hole of no copper, of no net, with one net.
    board.pads = vec![
        pad(1000, Some(1)),
        Pad {
            shape: circle(6000, 6000, 500),
            layers: LayerSet::EMPTY,
            hole: Some(circle(6000, 6000, 500)),
            net: None,
            clearance: None,
            stack: Vec::new(),
        },
    ];
    assert_eq!(bridges(&board), [Item::Pad(0)]);
}

#[test]
fn drc_pad_holes_and_arcs() {
    let rules = two_layer_rules();
    let mut board = Board::default();
    board.pads = vec![
        Pad {
            shape: circle(0, 0, 1000),
            layers: LayerSet::first(2),
            hole: Some(circle(0, 0, 600)),
            net: Some(NetId(1)),
            clearance: None,
            stack: Vec::new(),
        },
        Pad {
            shape: circle(5000, 0, 1000),
            layers: LayerSet::first(2),
            hole: Some(track((5000, -100), (5000, 100), 400)),
            net: Some(NetId(2)),
            clearance: None,
            stack: Vec::new(),
        },
    ];
    // A via 249 from the round hole, and an arc track around the slotted pad.
    board.vias = vec![v(0, 699, 300, 500, Some(1))];
    board.arc_tolerance = 5;
    board.tracks = vec![Track {
        start: p(5000, 700),
        mid: Some(p(5700, 0)),
        end: p(5000, -700),
        width: 100,
        layer: LayerId(0),
        net: Some(NetId(3)),
    }];
    let found = drc::check(&board, &rules).unwrap();
    // The arc is 700 - 50 - 500 = 150 from the pad, more than 100 even less the arc's
    // error of 2 * (5 + 1).
    assert!(
        matches!(
            found[..],
            [Violation::HoleToHole {
                a: Item::Via(0),
                b: Item::Pad(0),
                required: 250,
                ..
            }]
        ),
        "{found:#?}"
    );
    board.tracks[0].width = 220;
    let found = drc::check(&board, &rules).unwrap();
    assert!(
        matches!(
            found[..2],
            [
                Violation::Clearance {
                    a: Item::Track(0),
                    b: Item::Pad(1),
                    ..
                },
                Violation::HoleToHole { .. }
            ]
        ),
        "{found:#?}"
    );
}

#[test]
fn drc_rejects_bad_input() {
    let rules = two_layer_rules();
    let mut board = Board::default();
    board.tracks = vec![t((0, 0), (MAX_COORD + 1, 0), 100, 0, None)];
    assert!(matches!(
        drc::check(&board, &rules),
        Err(drc::DrcError::Geometry {
            item: Item::Track(0),
            ..
        })
    ));
    board.tracks = vec![t((0, 0), (1, 0), 100, 2, None)];
    assert_eq!(
        drc::check(&board, &rules),
        Err(drc::DrcError::Layer {
            item: Item::Track(0)
        })
    );
    board.tracks.clear();
    board.vias = vec![Via {
        to: LayerId(2),
        ..v(0, 0, 100, 200, None)
    }];
    assert_eq!(
        drc::check(&board, &rules),
        Err(drc::DrcError::Layer { item: Item::Via(0) })
    );
    board.vias = vec![Via {
        drill: i64::MIN,
        ..v(0, 0, 100, 200, None)
    }];
    assert!(drc::check(&board, &rules).is_err());
    board.vias.clear();
    board.keepouts = vec![Keepout {
        shape: circle(0, 0, 10),
        layers: LayerSet::first(3),
        tracks: true,
        vias: true,
        pads: true,
        except: Vec::new(),
    }];
    let err = drc::check(&board, &rules).unwrap_err();
    assert_eq!(
        err,
        drc::DrcError::Layer {
            item: Item::Keepout(0)
        }
    );
    assert!(!err.to_string().is_empty());
}

/// A DRC of copper pairs without the index: every pair on every layer.
fn brute_force_pairs(board: &Board, rules: &Rules) -> Vec<Violation> {
    let mut items: Vec<(Item, Shape, LayerSet, Option<NetId>)> = Vec::new();
    for (i, t) in board.tracks.iter().enumerate() {
        let s = Shape::track(t.start, t.end, t.width).unwrap();
        items.push((Item::Track(i), s, LayerSet::EMPTY.with(t.layer), t.net));
    }
    for (i, v) in board.vias.iter().enumerate() {
        items.push((
            Item::Via(i),
            Shape::circle(v.at, v.diameter).unwrap(),
            LayerSet::span(v.from, v.to),
            v.net,
        ));
    }
    for (i, pad) in board.pads.iter().enumerate() {
        items.push((Item::Pad(i), pad.shape.clone(), pad.layers, pad.net));
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            let (a, b) = (&items[i], &items[j]);
            if a.3.is_some() && a.3 == b.3 {
                continue;
            }
            let layers = LayerSet(a.2.0 & b.2.0);
            let short = layers.iter().find(|_| a.1.distance(&b.1).is_zero());
            if let Some(layer) = short {
                out.push(Violation::Short {
                    a: a.0,
                    b: b.0,
                    layer,
                });
                continue;
            }
            for layer in layers.iter() {
                let required = rules.clearance_between(a.3, b.3, layer);
                let gap = a.1.distance(&b.1);
                if gap.cmp_to(required) == Ordering::Less {
                    out.push(Violation::Clearance {
                        a: a.0,
                        b: b.0,
                        layer,
                        required,
                        gap,
                    });
                    break;
                }
            }
        }
    }
    out.sort_by_key(|v| match v {
        Violation::Short { a, b, .. } => (0, *a, *b),
        Violation::Clearance { a, b, .. } => (1, *a, *b),
        _ => unreachable!(),
    });
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Checks the DRC's pairs against every pair, on random boards.
    #[test]
    fn drc_matches_brute_force(
        tracks in proptest::collection::vec(
            ((0i64..3000, 0i64..3000), (0i64..3000, 0i64..3000), 20i64..300, 0u8..2, proptest::option::of(0u32..4)),
            0..25,
        ),
        vias in proptest::collection::vec(((0i64..3000, 0i64..3000), 100i64..400, proptest::option::of(0u32..4)), 0..10),
        pads in proptest::collection::vec(((0i64..3000, 0i64..3000), 20i64..400, 20i64..400, 1u64..4, proptest::option::of(0u32..4)), 0..10),
    ) {
        let rules = two_layer_rules();
        let mut board = Board::default();
        board.tracks = tracks.iter().map(|&(a, b, w, l, n)| t(a, b, w, l, n)).collect();
        board.vias = vias.iter().map(|&((x, y), d, n)| v(x, y, d / 2, d, n)).collect();
        board.pads = pads.iter().map(|&((x, y), w, h, layers, n)| Pad {
            shape: rect(x, y, x + w, y + h),
            layers: LayerSet(layers),
            hole: None,
            net: n.map(NetId),
            clearance: None,
            stack: Vec::new(),
        }).collect();
        let found = drc::check(&board, &rules).unwrap();
        let pairs: Vec<Violation> = found
            .iter()
            .filter(|v| matches!(v, Violation::Short { .. } | Violation::Clearance { .. }))
            .cloned()
            .collect();
        prop_assert_eq!(&pairs, &brute_force_pairs(&board, &rules));
        // Determinism, and the order.
        prop_assert_eq!(&found, &drc::check(&board, &rules).unwrap());
        prop_assert!(found.windows(2).all(|w| w[0].key() <= w[1].key()));
    }
}

#[test]
fn polyclip_stays_private() {
    // Every file of the module, by name, so that a new one can't be missed.
    let files: [(&str, &str); 9] = [
        ("clearance.rs", include_str!("clearance.rs")),
        ("drc.rs", include_str!("drc.rs")),
        ("index.rs", include_str!("index.rs")),
        ("mod.rs", include_str!("mod.rs")),
        ("poly.rs", include_str!("poly.rs")),
        ("rules.rs", include_str!("rules.rs")),
        ("shape.rs", include_str!("shape.rs")),
        ("tests.rs", include_str!("tests.rs")),
        ("types.rs", include_str!("types.rs")),
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/geom");
    let mut on_disk: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    on_disk.sort();
    assert_eq!(on_disk, files.map(|(name, _)| name.to_string()));
    let needle = ["poly", "clip"].concat();
    for (name, text) in files {
        let code = text
            .lines()
            .map(str::trim_start)
            .filter(|l| !l.starts_with("//"));
        for line in code {
            if name == "poly.rs" {
                let public = line.starts_with("pub ") || line.contains(" pub ");
                assert!(!public, "poly.rs has a public item: {line}");
                assert!(
                    !(line.starts_with("impl") && line.contains(" for ")),
                    "poly.rs has a trait impl: {line}"
                );
            } else if name != "tests.rs" {
                assert!(!line.contains(&needle), "{name} names {needle}: {line}");
            }
        }
    }
}

// KiCad's slack, and agreement with KiCad.

/// Boards like the probe that measured [`drc::KICAD_CLEARANCE_SLACK`]: at a clearance of
/// 0.2 mm, a pair `deficit` nm short of it, as two tracks, a track and a via, or a track
/// at 45° and a via.
fn slack_case(kind: usize, deficit: i64) -> Board {
    const MM: i64 = 1_000_000;
    let gap = MM / 5 - deficit;
    let mut board = Board::default();
    match kind {
        0 => {
            let y = 10 * MM + MM / 5 + gap;
            board.tracks = vec![
                t((10 * MM, 10 * MM), (20 * MM, 10 * MM), MM / 5, 0, Some(1)),
                t((10 * MM, y), (20 * MM, y), MM / 5, 0, Some(2)),
            ];
        }
        1 => {
            let y = 10 * MM + MM / 10 + gap + 3 * MM / 10;
            board.tracks = vec![t(
                (10 * MM, 10 * MM),
                (20 * MM, 10 * MM),
                MM / 5,
                0,
                Some(1),
            )];
            board.vias = vec![v(15 * MM, y, 3 * MM / 10, 3 * MM / 5, Some(2))];
        }
        _ => {
            // Rounded up to whole nanometres, so that the gap is short by at most `deficit`
            // and by more than `deficit - 1`.
            let d = ((MM / 10 + gap + 3 * MM / 10) as f64 / 2f64.sqrt()).ceil() as i64;
            board.tracks = vec![t(
                (10 * MM, 10 * MM),
                (20 * MM, 20 * MM),
                MM / 5,
                0,
                Some(1),
            )];
            board.vias = vec![v(
                15 * MM + d,
                15 * MM - d,
                3 * MM / 10,
                3 * MM / 5,
                Some(2),
            )];
        }
    }
    board
}

#[test]
fn slack_decides_as_kicad_did() {
    let rules = Rules::new(RuleSpec::new(&["F.Cu", "B.Cu"], 1, 200_000)).unwrap();
    let cases = [
        (0, false, false),
        (1, true, false),
        (500, true, false),
        (501, true, true),
    ];
    for kind in 0..3 {
        for (deficit, exact, kicad) in cases {
            let mut board = slack_case(kind, deficit);
            let found = drc::check(&board, &rules).unwrap();
            let what = format!("case {kind}, deficit {deficit}: {found:?}");
            assert_eq!(found.len(), usize::from(exact), "{what}");
            board.clearance_slack = drc::KICAD_CLEARANCE_SLACK;
            let found = drc::check(&board, &rules).unwrap();
            assert_eq!(found.len(), usize::from(kicad), "{what}, with slack");
            // The rule's clearance is reported, not the one the slack lowered it to.
            if let [Violation::Clearance { required, .. }] = found[..] {
                assert_eq!(required, 200_000);
            }
        }
    }
    let mut board = slack_case(0, 0);
    board.clearance_slack = -1;
    assert_eq!(drc::check(&board, &rules), Err(drc::DrcError::Slack(-1)));
    // A slack as large as the clearance still finds shorts.
    board.clearance_slack = 200_000;
    board.tracks[1] = board.tracks[0].clone();
    board.tracks[1].net = Some(NetId(2));
    let found = drc::check(&board, &rules).unwrap();
    assert!(matches!(found[..], [Violation::Short { .. }]), "{found:?}");
}

/// Millimetres with six decimals, as KiCad writes them.
fn mm(v: i64) -> String {
    let sign = if v < 0 { "-" } else { "" };
    format!("{sign}{}.{:06}", v.abs() / 1_000_000, v.abs() % 1_000_000)
}

fn kicad_uuid(item: Item) -> String {
    let (kind, i) = match item {
        Item::Track(i) => (0, i),
        Item::Via(i) => (1, i),
        Item::Pad(i) => (2, i),
        Item::Zone(i) => (3, i),
        Item::Keepout(i) => (4, i),
    };
    format!("00000000-0000-0000-{kind:04x}-{:012x}", i + 1)
}

/// The board as a KiCad 8 file, on F.Cu and B.Cu. KiCad's Y runs down, which changes no
/// distance.
fn kicad_board(board: &Board) -> String {
    let mut text = String::from(
        "(kicad_pcb (version 20240108) (generator \"guide\") (generator_version \"8.0\")\n\
         (general (thickness 1.6) (legacy_teardrops no))\n(paper \"A4\")\n\
         (layers (0 \"F.Cu\" signal) (31 \"B.Cu\" signal) (44 \"Edge.Cuts\" user))\n\
         (setup (pad_to_mask_clearance 0))\n(net 0 \"\")\n",
    );
    for n in 1..=8 {
        text += &format!("(net {n} \"N{n}\")\n");
    }
    for (a, b, c, d) in [
        (0, 0, 60, 0),
        (60, 0, 60, 60),
        (60, 60, 0, 60),
        (0, 60, 0, 0),
    ] {
        text += &format!(
            "(gr_line (start {a} {b}) (end {c} {d}) (layer \"Edge.Cuts\") (width 0.05))\n"
        );
    }
    let net = |n: Option<NetId>| n.map_or(0, |n| n.0);
    let layer = |l: LayerId| if l.0 == 0 { "F.Cu" } else { "B.Cu" };
    for (i, t) in board.tracks.iter().enumerate() {
        text += &format!(
            "(segment (start {} {}) (end {} {}) (width {}) (layer \"{}\") (net {}) (uuid \"{}\"))\n",
            mm(t.start.x),
            mm(t.start.y),
            mm(t.end.x),
            mm(t.end.y),
            mm(t.width),
            layer(t.layer),
            net(t.net),
            kicad_uuid(Item::Track(i)),
        );
    }
    for (i, v) in board.vias.iter().enumerate() {
        text += &format!(
            "(via (at {} {}) (size {}) (drill {}) (layers \"F.Cu\" \"B.Cu\") (net {}) (uuid \"{}\"))\n",
            mm(v.at.x),
            mm(v.at.y),
            mm(v.diameter),
            mm(v.drill),
            net(v.net),
            kicad_uuid(Item::Via(i)),
        );
    }
    text + ")\n"
}

/// The string values of a key in a piece of JSON.
fn json_strings(text: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\": \"");
    text.match_indices(&needle)
        .filter_map(|(at, _)| {
            let rest = &text[at + needle.len()..];
            Some(rest[..rest.find('"')?].to_string())
        })
        .collect()
}

/// The clearance and short findings in a KiCad JSON report, as whether it is a short and
/// the pair of items, sorted. Each violation object is found by its braces, which is enough
/// for the reports `kicad-cli` writes.
fn kicad_findings(json: &str, board: &Board) -> Vec<(bool, Item, Item)> {
    let tracks = (0..board.tracks.len()).map(Item::Track);
    let items: Vec<Item> = tracks.chain((0..board.vias.len()).map(Item::Via)).collect();
    let start = json.find("\"violations\"").expect("a list of violations");
    let list = &json[start..];
    let mut found = Vec::new();
    let (mut depth, mut open) = (0, 0);
    for (i, ch) in list.char_indices() {
        match ch {
            '{' => {
                if depth == 0 {
                    open = i;
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth > 0 {
                    continue;
                }
                let object = &list[open..i];
                let short = match json_strings(object, "type").pop().as_deref() {
                    Some("clearance") => false,
                    Some("shorting_items" | "tracks_crossing") => true,
                    _ => continue,
                };
                let mut pair: Vec<Item> = json_strings(object, "uuid")
                    .iter()
                    .filter_map(|u| items.iter().copied().find(|&i| kicad_uuid(i) == *u))
                    .collect();
                pair.sort();
                if let [a, b] = pair[..] {
                    found.push((short, a, b));
                }
            }
            ']' if depth == 0 => break,
            _ => {}
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Compares the DRC with KiCad's on random boards of tracks and vias, finding by finding.
/// Copper of different nets never touches, as explained inside, so that only clearances
/// are compared, and not shorts.
///
/// Runs only with `GUIDE_KICAD=1`, as it needs `kicad-cli` on the path, and takes about
/// half a second a board. `GUIDE_KICAD_BOARDS` sets how many, 20 by default.
#[test]
fn agrees_with_kicad_on_random_boards() {
    if std::env::var_os("GUIDE_KICAD").is_none() {
        eprintln!("skipped: set GUIDE_KICAD=1 to compare with kicad-cli");
        return;
    }
    const MM: i64 = 1_000_000;
    let boards: usize = std::env::var("GUIDE_KICAD_BOARDS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(20);
    let rules = Rules::new(RuleSpec::new(&["F.Cu", "B.Cu"], 1, MM / 5)).unwrap();
    let dir = std::env::temp_dir().join(format!("guide-kicad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut rng = crate::rng::Rng::new(42);
    let mut pick = |lo: i64, hi: i64| lo + rng.below((hi - lo) as u64) as i64;
    let mut differences = Vec::new();
    let mut compared = 0;
    for case in 0..boards {
        let mut board = Board {
            clearance_slack: drc::KICAD_CLEARANCE_SLACK,
            ..Board::default()
        };
        let (lo, hi) = (20 * MM, 23 * MM);
        // Copper of different nets never touches: KiCad gives copper with no pad to
        // anchor its net the net of what it touches, which would hide the short.
        let mut placed: Vec<(Shape, LayerSet, u32)> = Vec::new();
        let mut fits = |shape: &Shape, layers: LayerSet, net: u32| {
            let clear = placed
                .iter()
                .all(|(other, on, n)| *n == net || on.0 & layers.0 == 0 || !shape.touches(other));
            if clear {
                placed.push((shape.clone(), layers, net));
            }
            clear
        };
        while board.tracks.len() < 12 {
            let a = (pick(lo, hi), pick(lo, hi));
            let b = (pick(lo, hi), pick(lo, hi));
            let (width, layer, net) = (pick(MM / 10, 3 * MM / 10), pick(0, 2), pick(1, 4));
            let track = t(a, b, width, layer as u8, Some(net as u32));
            let shape = Shape::track(track.start, track.end, width).unwrap();
            if fits(&shape, LayerSet::EMPTY.with(track.layer), net as u32) {
                board.tracks.push(track);
            }
        }
        while board.vias.len() < 4 {
            let (x, y, d, net) = (
                pick(lo, hi),
                pick(lo, hi),
                pick(4 * MM / 10, 8 * MM / 10),
                pick(1, 4),
            );
            if fits(
                &Shape::circle(p(x, y), d).unwrap(),
                LayerSet::first(2),
                net as u32,
            ) {
                board.vias.push(v(x, y, d / 2, d, Some(net as u32)));
            }
        }
        let pcb = dir.join(format!("case{case}.kicad_pcb"));
        let report = dir.join(format!("case{case}.json"));
        std::fs::write(&pcb, kicad_board(&board)).unwrap();
        let run = std::process::Command::new("kicad-cli")
            .args([
                "pcb",
                "drc",
                "--format",
                "json",
                "--severity-all",
                "--all-track-errors",
                "-o",
            ])
            .arg(&report)
            .arg(&pcb)
            .output()
            .expect("kicad-cli runs");
        assert!(report.exists(), "kicad-cli wrote no report: {run:?}");
        let theirs = kicad_findings(&std::fs::read_to_string(&report).unwrap(), &board);
        let mut ours: Vec<(bool, Item, Item)> = drc::check(&board, &rules)
            .unwrap()
            .into_iter()
            .filter_map(|v| match v {
                Violation::Short { a, b, .. } => Some((true, a, b)),
                Violation::Clearance { a, b, .. } => Some((false, a, b)),
                _ => None,
            })
            .collect();
        ours.sort();
        compared += ours.len().max(theirs.len());
        if ours != theirs {
            differences.push(format!(
                "{}:\n  ours:    {ours:?}\n  KiCad's: {theirs:?}",
                pcb.display()
            ));
        }
    }
    eprintln!("compared {compared} findings on {boards} boards");
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn rules_for_pairs_and_minimum_clearance() {
    let mut spec = RuleSpec::new(&["F.Cu"], 100, 200);
    spec.min_clearance = 150;
    spec.classes = vec![NetClass {
        name: "low".into(),
        nets: vec![NetId(3)],
        overrides: overrides(None, Some(100)),
        ..NetClass::default()
    }];
    spec.pairs = vec![(NetId(2), NetId(1), 120), (NetId(1), NetId(4), 400)];
    let rules = Rules::new(spec.clone()).unwrap();
    let (n1, n2, n3, n4) = (
        Some(NetId(1)),
        Some(NetId(2)),
        Some(NetId(3)),
        Some(NetId(4)),
    );
    let top = LayerId(0);
    // A class below the minimum is raised to it.
    assert_eq!(rules.clearance(n3, top), 150);
    // A pair's own clearance replaces the nets', larger or smaller, in either order, but
    // not below the minimum.
    assert_eq!(rules.clearance_between(n1, n2, top), 150);
    assert_eq!(rules.clearance_between(n4, n1, top), 400);
    assert_eq!(rules.clearance_between(n2, n4, top), 200);
    assert_eq!(rules.max_clearance(), 400);

    let mut twice = spec.clone();
    twice.pairs.push((NetId(1), NetId(2), 300));
    assert_eq!(
        Rules::new(twice).unwrap_err(),
        RuleError::InvalidPair {
            a: NetId(1),
            b: NetId(2)
        }
    );
    let mut itself = spec.clone();
    itself.pairs = vec![(NetId(1), NetId(1), 300)];
    assert!(matches!(
        Rules::new(itself),
        Err(RuleError::InvalidPair { .. })
    ));
    let mut negative = spec;
    negative.min_clearance = -1;
    assert!(matches!(
        Rules::new(negative),
        Err(RuleError::InvalidValue { .. })
    ));
}

#[test]
fn drc_zones_and_own_clearances() {
    let mut spec = RuleSpec::new(&["F.Cu", "B.Cu"], 100, 200);
    spec.min_clearance = 150;
    let rules = Rules::new(spec).unwrap();
    let pad = |x: i64, net: u32, clearance: Option<i64>| Pad {
        shape: rect(x, 0, x + 1000, 1000),
        layers: LayerSet::first(1),
        hole: None,
        net: Some(NetId(net)),
        clearance,
        stack: Vec::new(),
    };
    let mut board = Board::default();
    // Pads 160 apart: within the classes' 200, but a pad's own 100 replaces it, raised to
    // the minimum of 150, which 160 keeps.
    board.pads = vec![pad(0, 1, Some(100)), pad(1160, 2, None)];
    assert!(drc::check(&board, &rules).unwrap().is_empty());
    // The larger of two own clearances applies.
    board.pads[1].clearance = Some(300);
    let found = drc::check(&board, &rules).unwrap();
    assert!(
        matches!(found[..], [Violation::Clearance { required: 300, .. }]),
        "{found:?}"
    );
    // A zone of a third net, 100 below both pads: nothing on the other layer, and on
    // theirs, a violation of each pad's clearance.
    board.pads[1].clearance = None;
    board.pads[1].shape = rect(2000, 0, 3000, 1000);
    let zone = |layer: u8| drc::Zone {
        shape: rect(-500, -500, 3500, -100),
        layer: LayerId(layer),
        net: Some(NetId(3)),
        clearance: None,
    };
    board.zones = vec![zone(1)];
    assert!(drc::check(&board, &rules).unwrap().is_empty());
    board.zones = vec![zone(0)];
    let found = drc::check(&board, &rules).unwrap();
    assert!(
        matches!(
            found[..],
            [
                Violation::Clearance {
                    a: Item::Pad(0),
                    b: Item::Zone(0),
                    required: 150,
                    ..
                },
                Violation::Clearance {
                    a: Item::Pad(1),
                    b: Item::Zone(0),
                    required: 200,
                    ..
                },
            ]
        ),
        "{found:?}"
    );
    // A zone on a layer the stack doesn't have is an error, as is a negative clearance.
    board.zones[0].layer = LayerId(2);
    assert_eq!(
        drc::check(&board, &rules).unwrap_err(),
        DrcError::Layer {
            item: Item::Zone(0)
        }
    );
    board.zones.clear();
    board.pads[0].clearance = Some(-5);
    assert_eq!(
        drc::check(&board, &rules).unwrap_err(),
        DrcError::Clearance {
            item: Item::Pad(0),
            value: -5
        }
    );
}

#[test]
fn drc_hole_clearance() {
    let mut spec = RuleSpec::new(&["F.Cu", "B.Cu"], 100, 100);
    spec.hole_clearance = 300;
    let rules = Rules::new(spec).unwrap();
    let mut board = Board::default();
    // A mounting hole of no net, with no copper, 200 from a track of net 1 on the bottom.
    board.pads = vec![Pad {
        shape: circle(0, 0, 1000),
        layers: LayerSet::EMPTY,
        hole: Some(circle(0, 0, 1000)),
        net: None,
        clearance: None,
        stack: Vec::new(),
    }];
    // A via whose copper is 300 from that track, and its hole 500: both are allowed.
    board.vias = vec![v(5000, 0, 400, 800, Some(2))];
    board.tracks = vec![
        t((-2000, 800), (8000, 800), 200, 1, Some(1)),
        // A track of the via's own net, which its hole may meet.
        t((5000, 0), (5000, -3000), 200, 0, Some(2)),
    ];
    let found = drc::check(&board, &rules).unwrap();
    let holes: Vec<(Item, Item, LayerId, i64)> = found
        .iter()
        .filter_map(|v| match *v {
            Violation::HoleClearance {
                hole,
                item,
                layer,
                required,
                ..
            } => Some((hole, item, layer, required)),
            _ => None,
        })
        .collect();
    assert_eq!(
        holes,
        [(Item::Pad(0), Item::Track(0), LayerId(1), 300)],
        "{found:?}"
    );
    // Copper that already violates the clearance against a pad isn't reported against the
    // pad's hole as well.
    board.pads[0].layers = LayerSet::first(2);
    board.pads[0].net = Some(NetId(3));
    board.tracks[0] = t((-2000, 560), (8000, 560), 100, 1, Some(1));
    let found = drc::check(&board, &rules).unwrap();
    assert!(
        matches!(
            found[..],
            [Violation::Clearance {
                a: Item::Track(0),
                b: Item::Pad(0),
                ..
            }]
        ),
        "{found:?}"
    );
}

#[test]
fn drc_reports_a_track_over_a_hole_with_no_hole_clearance() {
    let rules = Rules::new(RuleSpec::new(&["F.Cu", "B.Cu"], 100, 100)).unwrap();
    let mut board = Board::default();
    board.pads = vec![Pad {
        shape: circle(0, 0, 1000),
        layers: LayerSet::EMPTY,
        hole: Some(circle(0, 0, 1000)),
        net: None,
        clearance: None,
        stack: Vec::new(),
    }];
    // Touching the hole is allowed, and crossing it is not.
    board.tracks = vec![t((-2000, 600), (2000, 600), 200, 0, Some(1))];
    assert!(drc::check(&board, &rules).unwrap().is_empty());
    board.tracks[0] = t((-2000, 599), (2000, 599), 200, 0, Some(1));
    let found = drc::check(&board, &rules).unwrap();
    assert!(
        matches!(
            found[..],
            [Violation::HoleClearance {
                hole: Item::Pad(0),
                item: Item::Track(0),
                required: 0,
                ..
            }]
        ),
        "{found:?}"
    );
}

#[test]
fn regions_unfracture_outlines() {
    // Two squares meeting at a corner, saved as one ring that visits it twice.
    let ring: Vec<Point> = [
        (0, 0),
        (10, 0),
        (10, 10),
        (20, 10),
        (20, 20),
        (10, 20),
        (10, 10),
        (0, 10),
    ]
    .iter()
    .map(|&(x, y)| p(x, y))
    .collect();
    let pieces = Shape::region(&[&ring]).unwrap();
    assert_eq!(pieces.len(), 2);
    assert!(pieces.iter().all(Shape::is_exact));
    assert!(pieces[0].touches(&pieces[1]));
    // Out of range is an error, not a wrong answer.
    let far = [p(0, 0), p(MAX_COORD + 1, 0), p(0, 1)];
    assert!(matches!(
        Shape::region(&[&far]),
        Err(GeomError::CoordinateOutOfRange { .. })
    ));
}
