//! A device graph in VPR's format for `mult_4x4`, for the tests that route it.

use core::fmt::Write as _;
use std::collections::HashMap;

/// The tile types of `k6_N10_40nm.xml` as VPR writes them in a graph: an `io` is 8
/// positions of `outpad`, `inpad` and `clock`, each its own class, and a `clb` has 40
/// equivalent inputs, 10 equivalent outputs and a clock, in classes 0, 1 and 2.
fn k6_block_types() -> String {
    let mut xml = String::from(
        "<block_types>\n<block_type id=\"0\" name=\"EMPTY\" width=\"1\" height=\"1\"/>\n\
         <block_type id=\"1\" name=\"io\" width=\"1\" height=\"1\">\n",
    );
    let mut ptc = 0;
    for instance in 0..8 {
        for (kind, port) in [("INPUT", "outpad"), ("OUTPUT", "inpad"), ("INPUT", "clock")] {
            writeln!(
                xml,
                "<pin_class type=\"{kind}\"><pin ptc=\"{ptc}\">io[{instance}].{port}[0]</pin></pin_class>"
            )
            .unwrap();
            ptc += 1;
        }
    }
    xml.push_str(
        "</block_type>\n<block_type id=\"2\" name=\"clb\" width=\"1\" height=\"1\">\n\
         <pin_class type=\"INPUT\">",
    );
    for bit in 0..40 {
        write!(xml, "<pin ptc=\"{bit}\">clb.I[{bit}]</pin>").unwrap();
    }
    xml.push_str("</pin_class>\n<pin_class type=\"OUTPUT\">");
    for bit in 0..10 {
        write!(xml, "<pin ptc=\"{}\">clb.O[{bit}]</pin>", bit + 40).unwrap();
    }
    xml.push_str(
        "</pin_class>\n<pin_class type=\"INPUT\"><pin ptc=\"50\">clb.clk[0]</pin></pin_class>\n\
         </block_type>\n</block_types>\n",
    );
    xml
}

/// Which way a pin faces, by the channel it reaches.
#[derive(Clone, Copy)]
enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

/// Writes an island-style device of `size` by `size` tiles in VPR's `rr_graph` format, with
/// `tracks` wires of length 1 in each channel.
///
/// Wires meet in disjoint switch boxes, in both directions. A CLB's inputs each reach half
/// the tracks on one side, and its outputs half the tracks on every side. A pad reaches every
/// track of the channel beside it. Clock pins reach nothing, as with VPR's default ideal
/// clock. Delays: 60 ps through a switch box or out of a pin, 40 ps into a pin, and 5 ps for
/// each wire's own resistance and capacitance.
pub fn island_device(size: u32, tracks: u32) -> String {
    let mut nodes = String::new();
    let mut edges = String::new();
    let mut next = 0u32;
    // Every node, wires included, covers a single tile.
    let mut node = |nodes: &mut String,
                    kind: &str,
                    (x, y): (u32, u32),
                    ptc: u32,
                    capacity: u32,
                    wire: bool| {
        let id = next;
        next += 1;
        let extra = if wire {
            "<timing R=\"100\" C=\"1e-13\"/><segment segment_id=\"0\"/>"
        } else {
            ""
        };
        writeln!(
            nodes,
            "<node id=\"{id}\" type=\"{kind}\" capacity=\"{capacity}\"><loc xlow=\"{x}\" ylow=\"{y}\" xhigh=\"{x}\" yhigh=\"{y}\" ptc=\"{ptc}\"/>{extra}</node>"
        )
        .unwrap();
        id
    };
    let edge = |edges: &mut String, from: u32, to: u32, switch: u32| {
        writeln!(
            edges,
            "<edge src_node=\"{from}\" sink_node=\"{to}\" switch_id=\"{switch}\"/>"
        )
        .unwrap();
    };

    // Channels: CHANX at (x, y) runs above tile (x, y), and CHANY at (x, y) to its right.
    let last = size - 1;
    let mut chanx: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    let mut chany: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for y in 0..last {
        for x in 1..last {
            let wires = (0..tracks)
                .map(|t| node(&mut nodes, "CHANX", (x, y), t, 1, true))
                .collect();
            chanx.insert((x, y), wires);
        }
    }
    for x in 0..last {
        for y in 1..last {
            let wires = (0..tracks)
                .map(|t| node(&mut nodes, "CHANY", (x, y), t, 1, true))
                .collect();
            chany.insert((x, y), wires);
        }
    }

    // Switch boxes, at the corner above and to the right of each tile.
    for x in 0..last {
        for y in 0..last {
            let around: Vec<&Vec<u32>> = [
                chanx.get(&(x, y)),
                chanx.get(&(x + 1, y)),
                chany.get(&(x, y)),
                chany.get(&(x, y + 1)),
            ]
            .into_iter()
            .flatten()
            .collect();
            for a in &around {
                for b in &around {
                    if a[0] != b[0] {
                        for t in 0..tracks as usize {
                            edge(&mut edges, a[t], b[t], 1);
                        }
                    }
                }
            }
        }
    }
    let channel = |x: u32, y: u32, side: Side| -> Option<&Vec<u32>> {
        match side {
            Side::Top => chanx.get(&(x, y)),
            Side::Bottom => chanx.get(&(x, y.checked_sub(1)?)),
            Side::Right => chany.get(&(x, y)),
            Side::Left => chany.get(&(x.checked_sub(1)?, y)),
        }
    };
    const SIDES: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];

    let mut grid = String::from("<grid>\n");
    for y in 0..size {
        for x in 0..size {
            let on_edge_x = x == 0 || x == last;
            let on_edge_y = y == 0 || y == last;
            let tile = match (on_edge_x, on_edge_y) {
                (true, true) => 0,
                (false, false) => 2,
                _ => 1,
            };
            writeln!(
                grid,
                "<grid_loc x=\"{x}\" y=\"{y}\" block_type_id=\"{tile}\" width_offset=\"0\" height_offset=\"0\"/>"
            )
            .unwrap();
            if tile == 1 {
                let side = if x == 0 {
                    Side::Right
                } else if x == last {
                    Side::Left
                } else if y == 0 {
                    Side::Top
                } else {
                    Side::Bottom
                };
                let wires = channel(x, y, side).unwrap().clone();
                // Per position: outpad (in), inpad (out), clock (in), each a class.
                for class in 0..24 {
                    let input = class % 3 != 1;
                    let at = (x, y);
                    if input {
                        let sink = node(&mut nodes, "SINK", at, class, 1, false);
                        let ipin = node(&mut nodes, "IPIN", at, class, 1, false);
                        edge(&mut edges, ipin, sink, 0);
                        if class % 3 == 0 {
                            for &wire in &wires {
                                edge(&mut edges, wire, ipin, 2);
                            }
                        }
                    } else {
                        let source = node(&mut nodes, "SOURCE", at, class, 1, false);
                        let opin = node(&mut nodes, "OPIN", at, class, 1, false);
                        edge(&mut edges, source, opin, 0);
                        for &wire in &wires {
                            edge(&mut edges, opin, wire, 1);
                        }
                    }
                }
            } else if tile == 2 {
                let at = (x, y);
                let sink = node(&mut nodes, "SINK", at, 0, 40, false);
                let source = node(&mut nodes, "SOURCE", at, 1, 10, false);
                let clock = node(&mut nodes, "SINK", at, 2, 1, false);
                for pin in 0..40 {
                    let ipin = node(&mut nodes, "IPIN", at, pin, 1, false);
                    edge(&mut edges, ipin, sink, 0);
                    let side = SIDES[pin as usize % 4];
                    if let Some(wires) = channel(x, y, side) {
                        for (t, &wire) in wires.iter().enumerate() {
                            if t as u32 % 2 == (pin / 4) % 2 {
                                edge(&mut edges, wire, ipin, 2);
                            }
                        }
                    }
                }
                for pin in 40..50 {
                    let opin = node(&mut nodes, "OPIN", at, pin, 1, false);
                    edge(&mut edges, source, opin, 0);
                    for side in SIDES {
                        if let Some(wires) = channel(x, y, side) {
                            for (t, &wire) in wires.iter().enumerate() {
                                if t as u32 % 2 == pin % 2 {
                                    edge(&mut edges, opin, wire, 1);
                                }
                            }
                        }
                    }
                }
                let clock_pin = node(&mut nodes, "IPIN", at, 50, 1, false);
                edge(&mut edges, clock_pin, clock, 0);
            }
        }
    }
    grid.push_str("</grid>\n");
    let block_types = k6_block_types();

    format!(
        "<rr_graph tool_name=\"guide test\">\n<switches>\n\
         <switch id=\"0\" name=\"__vpr_delayless_switch__\" type=\"mux\"><timing Tdel=\"0\"/></switch>\n\
         <switch id=\"1\" name=\"sb\" type=\"mux\"><timing R=\"0\" Cin=\"0\" Cout=\"0\" Tdel=\"6e-11\"/></switch>\n\
         <switch id=\"2\" name=\"ipin_cblock\" type=\"mux\"><timing R=\"0\" Cin=\"0\" Cout=\"0\" Tdel=\"4e-11\"/></switch>\n\
         </switches>\n<segments><segment id=\"0\" name=\"L1\" length=\"1\"/></segments>\n\
         {block_types}{grid}<rr_nodes>\n{nodes}</rr_nodes>\n<rr_edges>\n{edges}</rr_edges>\n</rr_graph>\n"
    )
}
