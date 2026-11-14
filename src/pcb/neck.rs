//! Neck-down: a net whose tracks are too wide to leave one of its pads is routed narrower.
//!
//! A net's width is its class's, which is a default more than a rule: between the pads of a
//! 0.4 mm QFN, or of a fine-pitch connector, a track of 0.25 mm does not clear the pads on
//! either side, and boards at that pitch are routed narrower there by hand. A pad that no
//! straight track of its net's width leaves, from its centre in any of the eight directions
//! on any of its layers, past the other nets' pads and everything else that is fixed, has
//! its net routed at the widest width that does leave it, in steps of 5 µm, and never under
//! the least width the rules allow. The net has that width everywhere: one width a net.

use super::Design;
use super::detail::DIRS;
use super::router::pad_center;
use super::world::{Verdict, World};
use crate::geom::{LayerSet, Point, Shape};
use crate::id::NetId;

/// The step in which a narrower width is looked for.
const STEP: i64 = 5_000;

/// Whether a track of `net`, `width` wide, leaves the pad straight: from `center` to a
/// width past the pad's box, in one of the eight directions, on one of `layers`.
fn leaves(
    world: &World<'_>,
    net: NetId,
    shape: &Shape,
    center: Point,
    layers: LayerSet,
    width: i64,
) -> bool {
    let b = shape.bbox();
    DIRS.iter().any(|&(dx, dy)| {
        // How far the box's edge is in this direction, and a track's width more.
        let reach = |d: i64, lo: i64, hi: i64, at: i64| match d {
            1 => hi - at,
            -1 => at - lo,
            _ => i64::MAX,
        };
        let out = reach(dx, b.min().x, b.max().x, center.x).min(reach(
            dy,
            b.min().y,
            b.max().y,
            center.y,
        )) + width;
        let to = Point::new(center.x + dx * out, center.y + dy * out);
        layers.iter().any(|l| {
            Shape::track(center, to, width)
                .is_ok_and(|t| world.track(net, l, &t, None) == Verdict::Free)
        })
    })
}

/// By net, the width to route it at if that is narrower than its own: the widest at which
/// every pad of its that its own width does not leave is left, if there is one.
pub(crate) fn plan(design: &Design, world: &World<'_>) -> Vec<Option<i64>> {
    let rules = &design.rules;
    let stack = rules.layers();
    let mut out = vec![None; design.nets.len()];
    // By net, its pads with no hole, which are the ones a track has to leave on a layer.
    let mut pads: Vec<Vec<usize>> = vec![Vec::new(); design.nets.len()];
    for (i, p) in design.board.pads.iter().enumerate() {
        if let Some(n) = p.net
            && n.index() < pads.len()
            && p.hole.is_none()
        {
            pads[n.index()].push(i);
        }
    }
    for (i, spec) in design.nets.iter().enumerate() {
        if !spec.route || pads[i].is_empty() {
            continue;
        }
        let net = NetId(i as u32);
        // The least the rules allow on any layer, and the width the net has.
        let least = stack
            .iter()
            .map(|l| rules.track_width(Some(net), l))
            .max()
            .unwrap_or(spec.track_width);
        let full = spec.track_width.max(least);
        if least >= full {
            continue;
        }
        let at = |p: usize| {
            let pad = &design.board.pads[p];
            let layers = LayerSet(pad.layers.0 & stack.0);
            pad_center(&pad.shape).map(|c| (&pad.shape, c, layers))
        };
        let stuck: Vec<usize> = pads[i]
            .iter()
            .copied()
            .filter(|&p| {
                at(p).is_some_and(|(shape, c, layers)| !leaves(world, net, shape, c, layers, full))
            })
            .collect();
        if stuck.is_empty() {
            continue;
        }
        // The widest width that gets every pad out that any width gets out.
        let mut width = full;
        for &p in &stuck {
            let Some((shape, c, layers)) = at(p) else {
                continue;
            };
            let mut w = width;
            while w > least && !leaves(world, net, shape, c, layers, w) {
                w = (w - STEP).max(least);
            }
            if leaves(world, net, shape, c, layers, w) {
                width = w;
            }
        }
        if width < full {
            out[i] = Some(width);
        }
    }
    out
}
