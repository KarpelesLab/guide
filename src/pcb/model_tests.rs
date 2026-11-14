//! Tests of what a host says of its board beyond pads and nets: where vias may be, zones
//! filled by the host, zones that are planes or not, keepouts' exceptions, layers, pad
//! stacks, net ties and rules by area.

use super::tests::{MM, design, route_checked, smd};
use super::{Design, RouteOptions, Routed};
use crate::geom::drc::Pad;
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
