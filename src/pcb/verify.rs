//! Whether every net's pads are connected by its copper: the check of completion, which
//! shares nothing with the router but the geometry.

use std::collections::BTreeMap;

use crate::geom::drc::Board;
use crate::geom::{LayerSet, Rules, Shape, SpatialIndex};
use crate::id::NetId;

/// The nets with two pads or more that their copper doesn't all connect, in order.
///
/// Copper of a net is connected where two of its items touch on a layer they share:
/// pads, tracks, vias and zones. A pad's or a via's copper joins its layers, and so do the
/// shapes of a pad's [stack](crate::geom::drc::Pad::stack). Arcs are
/// taken as their approximations, which can only make them reach further.
///
/// ```
/// use guide::geom::drc::{Board, Pad, Track};
/// use guide::geom::{LayerId, LayerSet, Point, RuleSpec, Rules, Shape};
/// use guide::id::NetId;
/// use guide::pcb::unconnected;
///
/// let rules = Rules::new(RuleSpec::new(&["F.Cu"], 100, 100))?;
/// let pad = |x| Pad {
///     shape: Shape::circle(Point::new(x, 0), 500).unwrap(),
///     layers: LayerSet::EMPTY.with(LayerId(0)),
///     hole: None,
///     net: Some(NetId(1)),
///     clearance: None,
///     stack: Vec::new(),
/// };
/// let mut board = Board::default();
/// board.pads = vec![pad(0), pad(5000)];
/// assert_eq!(unconnected(&board, &rules), [NetId(1)]);
/// board.tracks.push(Track {
///     start: Point::new(0, 0),
///     end: Point::new(5000, 0),
///     mid: None,
///     width: 100,
///     layer: LayerId(0),
///     net: Some(NetId(1)),
/// });
/// assert!(unconnected(&board, &rules).is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn unconnected(board: &Board, rules: &Rules) -> Vec<NetId> {
    // By net: each item's shape and layers, pads first.
    let mut items: BTreeMap<NetId, (usize, Vec<(Shape, LayerSet)>)> = BTreeMap::new();
    for p in &board.pads {
        if let Some(n) = p.net {
            let entry = items.entry(n).or_default();
            entry.0 += 1;
            entry.1.push((p.shape.clone(), p.layers));
        }
    }
    // By net, the other shapes of its pads' stacks, each with its pad's position among the
    // net's: they go after everything else, and are one piece with their pad.
    let mut stacks: BTreeMap<NetId, Vec<(usize, Shape, LayerSet)>> = BTreeMap::new();
    let mut count: BTreeMap<NetId, usize> = BTreeMap::new();
    for p in &board.pads {
        if let Some(n) = p.net {
            let k = count.entry(n).or_default();
            for (layers, shape) in &p.stack {
                stacks
                    .entry(n)
                    .or_default()
                    .push((*k, shape.clone(), *layers));
            }
            *k += 1;
        }
    }
    let mut add = |net: Option<NetId>, shape: Option<Shape>, layers: LayerSet| {
        if let (Some(n), Some(s)) = (net, shape)
            && let Some(entry) = items.get_mut(&n)
        {
            entry.1.push((s, layers));
        }
    };
    for t in &board.tracks {
        let shape = match t.mid {
            None => Shape::track(t.start, t.end, t.width),
            Some(mid) => Shape::arc_track(t.start, mid, t.end, t.width, board.arc_tolerance),
        };
        add(t.net, shape.ok(), LayerSet::EMPTY.with(t.layer));
    }
    for v in &board.vias {
        add(
            v.net,
            Shape::circle(v.at, v.diameter).ok(),
            LayerSet::span(v.from, v.to),
        );
    }
    for z in &board.zones {
        add(z.net, Some(z.shape.clone()), LayerSet::EMPTY.with(z.layer));
    }
    let mut joined: BTreeMap<NetId, Vec<(usize, usize)>> = BTreeMap::new();
    for (net, parts) in stacks {
        if let Some(entry) = items.get_mut(&net) {
            for (pad, shape, layers) in parts {
                joined.entry(net).or_default().push((pad, entry.1.len()));
                entry.1.push((shape, layers));
            }
        }
    }
    let stack = rules.layers();
    let mut out = Vec::new();
    let mut near = Vec::new();
    for (net, (pads, list)) in &items {
        if *pads < 2 {
            continue;
        }
        let index = SpatialIndex::bulk_load(list.iter().map(|(s, _)| s.bbox()));
        let mut parent: Vec<usize> = (0..list.len()).collect();
        for &(pad, part) in joined.get(net).map_or(&[][..], Vec::as_slice) {
            parent[part] = pad;
        }
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for (i, (shape, layers)) in list.iter().enumerate() {
            index.query(&shape.bbox(), &mut near);
            for id in &near {
                let j = id.slot() as usize;
                if j <= i {
                    continue;
                }
                let (other, other_layers) = &list[j];
                if layers.0 & other_layers.0 & stack.0 == 0 {
                    continue;
                }
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b && shape.touches(other) {
                    parent[a.max(b)] = a.min(b);
                }
            }
        }
        let root = find(&mut parent, 0);
        if (1..*pads).any(|i| find(&mut parent, i) != root) {
            out.push(*net);
        }
    }
    out
}
