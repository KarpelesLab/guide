//! Pin and gate swapping: giving pins that are equivalent each other's nets, where that makes
//! the board shorter to route.
//!
//! A board file says nothing of which pins are equivalent, so the caller says, with
//! [`SwapGroup`]s: the two pins of a resistor or a capacitor without polarity, the elements of
//! a resistor network, a connector's pins that the design leaves free to assign, a
//! microcontroller's GPIOs that its pin multiplexer can route anywhere, or the DQ bits of one
//! byte lane of a DRAM, whose order the controller doesn't care about. [`two_pin_groups`]
//! finds the safe automatic ones: the two pins of resistors, capacitors, inductors and ferrite
//! beads, by their references.
//!
//! [`optimize_swaps`] swaps the nets of pins within a group while that makes the nets
//! shorter: two-pin nets by the distance between their pins, which uncrosses them, and others
//! by their half-perimeter. Nets that a zone connects don't count, as they go to their plane by
//! a via wherever they are. It returns the swaps, which [`apply_swaps`] makes in a
//! [`PlacementDesign`], and `kicad::replace_pad_nets` in a KiCad board, for the schematic to
//! be annotated back from.

use std::collections::BTreeMap;

use super::place::{Placed, PlacementDesign};
use crate::geom::Point;
use crate::id::NetId;

/// Pins of one component whose nets may be exchanged among them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapGroup {
    /// The component, by index in [`PlacementDesign::components`].
    pub component: usize,
    /// Its pins, by index in [`Component::pins`](super::place::Component::pins).
    pub pins: Vec<usize>,
}

/// A pin given another net.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinSwap {
    /// The component, by index in [`PlacementDesign::components`].
    pub component: usize,
    /// The pin, by index in [`Component::pins`](super::place::Component::pins).
    pub pin: usize,
    /// The net it had.
    pub from: NetId,
    /// The net it has now.
    pub to: NetId,
}

/// The two pins of each resistor, capacitor, inductor and ferrite bead, as their references
/// say (`R`, `C`, `L`, `FB`), that has two pins of two different nets. The caller leaves out
/// polarized parts, such as electrolytic capacitors, which a reference doesn't tell apart.
pub fn two_pin_groups(design: &PlacementDesign) -> Vec<SwapGroup> {
    design
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            let prefix: String = c
                .name
                .chars()
                .take_while(char::is_ascii_alphabetic)
                .collect();
            matches!(prefix.as_str(), "R" | "C" | "L" | "FB")
                && c.pins.len() == 2
                && c.pins[0].net != c.pins[1].net
        })
        .map(|(component, _)| SwapGroup {
            component,
            pins: vec![0, 1],
        })
        .collect()
}

/// How long a net is, for swapping: the distance between its pins if it has two, its
/// half-perimeter otherwise.
fn net_cost(pins: &[Point]) -> f64 {
    match pins {
        [] | [_] => 0.0,
        [a, b] => {
            let (dx, dy) = ((a.x - b.x) as f64, (a.y - b.y) as f64);
            (dx * dx + dy * dy).sqrt()
        }
        _ => {
            let (mut lo, mut hi) = (pins[0], pins[0]);
            for p in pins {
                lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
                hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
            }
            ((hi.x - lo.x) + (hi.y - lo.y)) as f64
        }
    }
}

/// How many times every pair of every group is tried, at most.
const PASSES: usize = 16;

/// Swaps the nets of pins within groups while that makes the nets shorter, with the
/// components where `placed` puts them, and returns the swaps, at most one for each pin, from
/// its net at first to its net at the end. See the [module](self).
///
/// Every pair of pins of a group is tried in order, and swapped if that shortens their two
/// nets by more than a nanometre, until a pass swaps nothing.
pub fn optimize_swaps(
    design: &PlacementDesign,
    placed: &[Placed],
    groups: &[SwapGroup],
) -> Vec<PinSwap> {
    let planes: Vec<NetId> = design.planes.iter().map(|&(n, _)| n).collect();
    // Every pin's place and net.
    let mut at: Vec<Vec<Point>> = Vec::new();
    let mut nets: Vec<Vec<NetId>> = Vec::new();
    for (c, comp) in design.components.iter().enumerate() {
        let p = placed.get(c).copied();
        at.push(
            comp.pins
                .iter()
                .map(|pin| {
                    p.and_then(|p| super::place::placed_point(comp, &p, pin.at))
                        .unwrap_or(pin.at)
                })
                .collect(),
        );
        nets.push(comp.pins.iter().map(|p| p.net).collect());
    }
    // By net, its pins.
    let mut members: BTreeMap<NetId, Vec<(usize, usize)>> = BTreeMap::new();
    for (c, list) in nets.iter().enumerate() {
        for (p, &n) in list.iter().enumerate() {
            members.entry(n).or_default().push((c, p));
        }
    }
    let cost = |members: &BTreeMap<NetId, Vec<(usize, usize)>>, n: NetId| {
        if planes.contains(&n) {
            return 0.0;
        }
        let pins: Vec<Point> = members
            .get(&n)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|&(c, p)| at[c][p])
            .collect();
        net_cost(&pins)
    };
    for _ in 0..PASSES {
        let mut swapped = false;
        for g in groups {
            let c = g.component;
            let Some(list) = nets.get(c) else { continue };
            let pins: Vec<usize> = g.pins.iter().copied().filter(|&p| p < list.len()).collect();
            for (k, &a) in pins.iter().enumerate() {
                for &b in &pins[k + 1..] {
                    let (na, nb) = (nets[c][a], nets[c][b]);
                    if na == nb {
                        continue;
                    }
                    let before = cost(&members, na) + cost(&members, nb);
                    let exchange =
                        |members: &mut BTreeMap<NetId, Vec<(usize, usize)>>, x: NetId, y: NetId| {
                            for (n, (from, to)) in [(x, (a, b)), (y, (b, a))] {
                                if let Some(m) = members.get_mut(&n)
                                    && let Some(e) = m.iter_mut().find(|e| **e == (c, from))
                                {
                                    *e = (c, to);
                                }
                            }
                        };
                    exchange(&mut members, na, nb);
                    let after = cost(&members, na) + cost(&members, nb);
                    if after < before - 1.0 {
                        nets[c][a] = nb;
                        nets[c][b] = na;
                        swapped = true;
                    } else {
                        // Back as it was: the pins exchanged back.
                        exchange(&mut members, na, nb);
                    }
                }
            }
        }
        if !swapped {
            break;
        }
    }
    let mut out = Vec::new();
    for (c, comp) in design.components.iter().enumerate() {
        for (p, pin) in comp.pins.iter().enumerate() {
            if nets[c][p] != pin.net {
                out.push(PinSwap {
                    component: c,
                    pin: p,
                    from: pin.net,
                    to: nets[c][p],
                });
            }
        }
    }
    out
}

/// Gives pins the nets that swaps give them.
pub fn apply_swaps(design: &mut PlacementDesign, swaps: &[PinSwap]) {
    for s in swaps {
        if let Some(pin) = design
            .components
            .get_mut(s.component)
            .and_then(|c| c.pins.get_mut(s.pin))
        {
            pin.net = s.to;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcb::place::{Component, ComponentPin, Side};

    #[test]
    fn crossed_connections_are_uncrossed() {
        let mut d = PlacementDesign::default();
        // A connector on the left, a part on the right whose pins are in the other order.
        let mut j = Component::new("J1", Point::new(0, 0), 0, Side::Front);
        let mut u = Component::new("U1", Point::new(10_000, 0), 0, Side::Front);
        for k in 0..4 {
            j.pins.push(ComponentPin {
                at: Point::new(0, k * 1000),
                net: NetId(k as u32 + 1),
            });
            u.pins.push(ComponentPin {
                at: Point::new(10_000, k * 1000),
                net: NetId(4 - k as u32),
            });
        }
        d.components = vec![j, u];
        let placed: Vec<Placed> = d
            .components
            .iter()
            .map(|c| Placed {
                at: c.at,
                rotation: c.rotation,
                side: c.side,
            })
            .collect();
        let group = SwapGroup {
            component: 1,
            pins: vec![0, 1, 2, 3],
        };
        let swaps = optimize_swaps(&d, &placed, &[group]);
        apply_swaps(&mut d, &swaps);
        for k in 0..4 {
            assert_eq!(d.components[1].pins[k].net, d.components[0].pins[k].net);
        }
        assert_eq!(swaps.len(), 4);
        // Nothing to gain the second time.
        assert!(optimize_swaps(&d, &placed, &two_pin_groups(&d)).is_empty());
    }
}
