//! Handmade cases for the reader, the scoring and the router.
//!
//! The expected scores of the scoring tests are what `eval2008.pl`, the ISPD 2008 contest's
//! script, printed for the same files, except where a test says the script stops.

use super::*;

/// A grid of 4 by 3 cells of 10 by 10, from (100, 200), on three layers: horizontal on 1 and 3,
/// vertical on 2. Wires are 1 wide with a spacing of 1, so each net takes 2 from each edge.
const GRID: &str = "grid 4 3 3
vertical capacity 0 4 0
horizontal capacity 4 0 2
minimum width 1 1 1
minimum spacing 1 1 1
via spacing 1 1 1
100 200 10 10
";

fn design(nets: &str, adjustments: &str) -> Design {
    let text = format!("{GRID}\n{nets}\n{adjustments}");
    read_design(text.as_bytes()).unwrap()
}

fn score(design: &Design, routes: &str, via_cost: u64) -> Evaluation {
    evaluate(design, &read_routes(routes.as_bytes()).unwrap(), via_cost)
}

const THREE_NETS: &str = "num net 3
a 0 2 1
105 205 1
135 205 1
b 1 2 1
101 209 1
139 201 1
c 2 3 1
105 215 1
105 225 1
125 225 1
";

#[test]
fn reads_the_grid_nets_and_adjustments() {
    let d = design(THREE_NETS, "2\n1 0 1 2 0 1 0\n0 1 2 0 2 2 1\n");
    assert_eq!((d.cols, d.rows, d.layers()), (4, 3, 3));
    assert_eq!(d.vertical_capacity, [0, 4, 0]);
    assert_eq!(d.horizontal_capacity, [4, 0, 2]);
    assert_eq!((d.origin, d.tile), ((100, 200), (10, 10)));
    assert_eq!(d.nets.len(), 3);
    assert_eq!(d.nets[2].name, "c");
    assert_eq!(d.nets[2].number, 2);
    assert_eq!(
        d.nets[2].pins[2],
        Pin {
            x: 125,
            y: 225,
            layer: 0
        }
    );
    assert_eq!(d.cell_of(139, 201), Some((3, 0)));
    assert_eq!(d.cell_of(140, 201), None);
    assert_eq!(d.center(3, 0), (135, 205));
    let caps = d.capacities();
    let h = |layer, x, y| caps.get(EdgeAt::Horizontal { layer, x, y });
    let v = |layer, x, y| caps.get(EdgeAt::Vertical { layer, x, y });
    assert_eq!(
        (h(0, 0, 0), h(0, 1, 0), h(2, 1, 0), h(1, 1, 0)),
        (4, 0, 2, 0)
    );
    assert_eq!((v(1, 0, 0), v(1, 0, 1), v(0, 0, 1)), (4, 1, 0));
}

#[test]
fn reading_reports_the_line_at_fault() {
    let text = format!("{GRID}num net 1\na 0 2 1\n105 205 1\n105 205 0\n");
    match read_design(text.as_bytes()) {
        Err(ReadError::Syntax { line: 11, .. }) => {}
        other => panic!("{other:?}"),
    }
    let text = format!("{GRID}num net 2\na 0 1 1\n105 205 1\n");
    assert!(matches!(
        read_design(text.as_bytes()),
        Err(ReadError::UnexpectedEnd { .. })
    ));
    assert!(matches!(
        read_routes("a 0\n(1,2,1)-(3,2)\n!\n".as_bytes()),
        Err(ReadError::Syntax { line: 2, .. })
    ));
}

/// Two nets on the same row of layer 1, whose edges take one net, and a third net with a
/// bend. eval2008.pl: Tot OF 0, Max OF 0, WL 11, and with each route again but a's, Tot OF 6,
/// Max OF 2, WL 11.
#[test]
fn overflow_is_counted_in_capacity_units_on_every_edge() {
    let d = design(THREE_NETS, "");
    let routes = "a 0 1
(105,205,1)-(135,205,1)
!
b 1
(105,205,1)-(135,205,1)
!
c 2 4
(105,215,1)-(105,215,2)
(105,215,2)-(105,225,2)
(105,225,2)-(105,225,1)
(105,225,1)-(125,225,1)
!
";
    let s = score(&d, routes, 1);
    assert!(s.is_complete(), "{s:?}");
    // 2 + 2 on each of three edges of capacity 4 is no overflow; the capacity is 4 on layer 1,
    // so nothing overflows yet.
    assert_eq!((s.total_overflow, s.max_overflow), (0, 0));
    assert_eq!((s.wire_length, s.vias, s.wirelength), (9, 2, 11));
    // With a third net on the row, each of its three edges is 2 over.
    let routes = format!(
        "{routes}c 2\n(105,215,1)-(105,215,2)\n(105,215,2)-(105,225,2)\n(105,225,2)-(105,225,1)\n(105,225,1)-(125,225,1)\n!\nb 1\n(105,205,1)-(135,205,1)\n!\n"
    );
    let s = score(&d, &routes, 1);
    assert!(s.is_complete(), "{s:?}");
    // The row now has 3 nets on edges of capacity 4: 2 over on each of 3 edges. Net c's
    // second route is counted again too: its vertical edge has 4 of 4, and its horizontal ones
    // 4 of 4, so no more overflow there.
    assert_eq!(
        (s.total_overflow, s.max_overflow, s.overflowed_edges),
        (6, 2, 3)
    );
    // Lengths are of each net's last route: a 3, b 3, c 3 and 2 vias.
    assert_eq!((s.wire_length, s.vias, s.wirelength), (9, 2, 11));
    assert_eq!(s.overflowed_nets, 2);
    assert_eq!(score(&d, &routes, 3).wirelength, 15);
}

/// An edge adjusted to 0 overflows by all of its usage, and one adjusted to 1 by all but 1.
/// eval2008.pl: Tot OF 3, Max OF 2, WL 5.
#[test]
fn adjustments_replace_the_default_capacity() {
    let d = design(
        "num net 2\na 0 2 1\n105 205 1\n125 205 1\nb 1 2 1\n105 205 1\n105 215 1\n",
        "2\n1 0 1 2 0 1 0\n0 0 2 0 1 2 1\n",
    );
    let routes = "a 0
(105,205,1)-(125,205,1)
!
b 1
(105,205,1)-(105,205,2)
(105,205,2)-(105,215,2)
(105,215,2)-(105,215,1)
!
";
    let s = score(&d, routes, 1);
    assert!(s.is_complete(), "{s:?}");
    assert_eq!(
        (s.total_overflow, s.max_overflow, s.overflowed_edges),
        (3, 2, 2)
    );
    assert_eq!((s.wire_length, s.vias), (3, 2));
    assert_eq!(s.wirelength, 5);
}

/// A via over several layers counts each, and so does its share of the wirelength. A net
/// in one cell needs no route. eval2008.pl: Tot OF 0, Max OF 0, WL 6.
#[test]
fn vias_count_every_layer_they_span() {
    let d = design(
        "num net 2\na 0 2 1\n105 205 1\n125 205 1\nb 1 2 1\n105 205 1\n106 206 1\n",
        "",
    );
    let routes = "a 0
(105,205,1)-(105,205,3)
(105,205,3)-(125,205,3)
(125,205,3)-(125,205,1)
!
";
    let s = score(&d, routes, 1);
    assert!(s.is_complete(), "{s:?}");
    assert_eq!((s.wire_length, s.vias, s.wirelength), (2, 4, 6));
    assert_eq!(score(&d, routes, 3).wirelength, 14);
    assert_eq!(s.total_overflow, 0);
}

/// eval2008.pl stops with "net b disjoint", after "net a pin (135,205,1) not attached" and
/// "net b pin (139,201,1) not attached". Without b's second piece, it stops with "net c
/// unrouted".
#[test]
fn faults_and_missing_routes_are_reported() {
    let d = design(THREE_NETS, "");
    // a: a pin not reached. b: a piece not connected to the first pin. c: no route at all.
    let routes = "a 0
(105,205,1)-(125,205,1)
!
b 1
(105,205,1)-(115,205,1)
(125,205,1)-(135,205,1)
!
zz 7
(105,205,1)-(115,205,1)
!
";
    let s = score(&d, routes, 1);
    assert!(!s.is_complete());
    assert_eq!(s.unrouted, [2]);
    // a misses its second pin, and b its second too.
    assert_eq!(s.unattached_pins, 2);
    assert_eq!(
        s.faults,
        [
            RouteFault::Disjoint { net: 1 },
            RouteFault::UnknownNet { name: "zz".into() }
        ]
    );
    let s = score(&d, "a 0\n(105,205,1)-(115,215,1)\n!\n", 1);
    assert_eq!(s.faults[0], RouteFault::Diagonal { net: 0 });
    let s = score(&d, "a 0\n(105,205,1)-(105,205,1)\n!\n", 1);
    assert_eq!(s.faults[0], RouteFault::Null { net: 0 });
    let s = score(&d, "a 0\n(105,205,1)-(145,205,1)\n!\n", 1);
    assert_eq!(s.faults[0], RouteFault::OffGrid { net: 0 });
}

#[test]
fn routes_are_written_at_cell_centres_and_read_back() {
    let d = design(THREE_NETS, "");
    let segs = [
        Segment {
            from: Gcell::new(0, 0, 0),
            to: Gcell::new(3, 0, 0),
        },
        Segment {
            from: Gcell::new(3, 0, 0),
            to: Gcell::new(3, 0, 1),
        },
    ];
    let mut out = Vec::new();
    write_net_route(&mut out, &d, &d.nets[0], &segs).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(
        text,
        "a 0 2\n(105,205,1)-(135,205,1)\n(135,205,1)-(135,205,2)\n!\n"
    );
    let back = read_routes(text.as_bytes()).unwrap();
    assert_eq!(back[0].segments[1], ((135, 205, 0), (135, 205, 1)));
}

/// The router on a design with more demand on layer 1 than it holds: every net is connected,
/// nothing overflows, and the result is the same on any number of threads.
#[test]
fn routing_a_small_design_is_complete_and_free_of_overflow() {
    let mut nets = String::from("num net 7\n");
    for i in 0..6 {
        nets.push_str(&format!(
            "n{i} {i} 2 1\n{} {} 1\n{} {} 1\n",
            101 + i,
            205 + 10 * (i % 3),
            131 + i,
            205 + 10 * ((i + 1) % 3)
        ));
    }
    nets.push_str("n6 6 3 1\n105 205 1\n135 225 1\n125 215 1\n");
    let d = design(&nets, "1\n0 0 1 1 0 1 2\n");
    let mut first = None;
    for threads in [1, 3] {
        let options = AsicOptions {
            threads,
            batch_size: 4,
            ..AsicOptions::default()
        };
        let r = route_design(&d, &options);
        assert_eq!(r.routed_nets, 7);
        assert_eq!(r.planar_overflow, 0);
        let mut file = Vec::new();
        for (net, segs) in d.nets.iter().zip(&r.routes) {
            write_net_route(&mut file, &d, net, segs).unwrap();
        }
        let s = evaluate(&d, &read_routes(file.as_slice()).unwrap(), 1);
        assert!(s.is_complete(), "{s:?}");
        assert_eq!(s.total_overflow, 0, "{s:?}");
        assert_eq!(s.nets_with_cycles, 0);
        match &first {
            None => first = Some(r.routes),
            Some(routes) => assert_eq!(routes, &r.routes),
        }
    }
}
