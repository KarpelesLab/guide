//! Tests of differential pairs and lengths on small boards.

use super::tests::{MM, design, outline, route_checked};
use super::{Design, DiffPair, Options, Routed, coupling};
use crate::geom::drc::{Keepout, Pad};
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::NetId;

/// A pad of 0.3 by 0.6 mm, as a connector's.
fn pin(x: i64, y: i64, net: u32) -> Pad {
    Pad {
        shape: Shape::rect(Rect::new(
            Point::new(x - 150_000, y - 300_000),
            Point::new(x + 150_000, y + 300_000),
        ))
        .unwrap(),
        layers: LayerSet::EMPTY.with(LayerId(0)),
        hole: None,
        net: Some(NetId(net)),
        clearance: None,
    }
}

/// A pair, nets 1 and 2, from two pins 0.65 mm apart to two others at `(x, y)`.
fn pair_design(layers: usize, x: i64, y: i64) -> Design {
    let mut d = design(
        layers,
        vec![
            pin(0, 0, 1),
            pin(650_000, 0, 2),
            pin(x, y, 1),
            pin(x + 650_000, y, 2),
        ],
        3,
    );
    d.pairs.push(DiffPair {
        positive: NetId(1),
        negative: NetId(2),
        width: 200_000,
        gap: 200_000,
    });
    d.outline = outline(Rect::new(
        Point::new(-5 * MM, -5 * MM),
        Point::new(x + 5 * MM, y + 5 * MM),
    ));
    d
}

/// The fraction of the positive net's length at the pair's gap, to within a micrometre.
fn coupled(r: &Routed) -> f64 {
    let (c, total) = coupling(&r.tracks, NetId(1), NetId(2), 200_000, 1000);
    c / total
}

#[test]
fn a_pair_routes_side_by_side() {
    let d = pair_design(2, 20 * MM, 15 * MM);
    let r = route_checked(&d, &Options::default());
    assert!(r.unrouted.is_empty());
    assert_eq!(r.stats.pairs, (1, 1));
    let f = coupled(&r);
    assert!(f > 0.8, "coupled {f}");
}

/// A band across the board on the top layer that tracks can't cross.
fn band(d: &mut Design, y0: i64, y1: i64) {
    d.board.keepouts.push(Keepout {
        shape: Shape::rect(Rect::new(Point::new(-10 * MM, y0), Point::new(40 * MM, y1))).unwrap(),
        layers: LayerSet::EMPTY.with(LayerId(0)),
        tracks: true,
        vias: false,
        pads: false,
    });
}

#[test]
fn a_pair_changes_layers_through_two_vias_side_by_side() {
    let mut d = pair_design(2, 20 * MM, 15 * MM);
    band(&mut d, 6 * MM, 8 * MM);
    let r = route_checked(&d, &Options::default());
    assert!(r.unrouted.is_empty());
    assert_eq!(r.stats.pairs, (1, 1));
    assert_eq!(r.vias.len(), 4);
    let f = coupled(&r);
    assert!(f > 0.7, "coupled {f}");
}

#[test]
fn a_pair_whose_nets_swap_sides_is_still_routed() {
    let mut d = pair_design(2, 20 * MM, 15 * MM);
    // The positive net's second pin is to the right of the negative's, not the left.
    d.board.pads[2] = pin(20 * MM + 650_000, 15 * MM, 1);
    d.board.pads[3] = pin(20 * MM, 15 * MM, 2);
    let r = route_checked(&d, &Options::default());
    assert!(r.unrouted.is_empty());
}

#[test]
fn pairs_far_apart_at_their_ends_are_routed_apart() {
    let mut d = pair_design(2, 20 * MM, 15 * MM);
    d.board.pads[1] = pin(8 * MM, 0, 2);
    let r = route_checked(&d, &Options::default());
    assert!(r.unrouted.is_empty());
    assert_eq!(r.stats.pairs, (1, 0));
}

#[test]
fn without_coupling_pairs_route_as_other_nets() {
    let d = pair_design(2, 20 * MM, 15 * MM);
    let options = Options {
        couple_pairs: false,
        ..Options::default()
    };
    let r = route_checked(&d, &options);
    assert!(r.unrouted.is_empty());
    assert_eq!(r.stats.pairs, (1, 0));
}

#[test]
fn terminals_beyond_a_pairs_ends_are_joined_to_it() {
    let mut d = pair_design(2, 20 * MM, 15 * MM);
    d.board.pads.push(pin(23 * MM, 15 * MM, 1));
    d.board.pads.push(pin(23 * MM + 650_000, 15 * MM, 2));
    let r = route_checked(&d, &Options::default());
    assert!(r.unrouted.is_empty());
    assert_eq!(r.stats.pairs, (1, 1));
    assert!(coupled(&r) > 0.6);
}

#[test]
fn pairs_route_the_same_every_time() {
    let mut d = pair_design(2, 20 * MM, 15 * MM);
    band(&mut d, 6 * MM, 8 * MM);
    let a = route_checked(&d, &Options::default());
    let b = route_checked(&d, &Options::default());
    assert_eq!(a.tracks, b.tracks);
    assert_eq!(a.vias, b.vias);
}
