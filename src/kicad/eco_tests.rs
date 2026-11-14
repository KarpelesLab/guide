//! Engineering changes on the curated boards: each board as placed by hand, routed, then
//! changed a little, a part turned or moved, a net removed or added, a part added, and routed
//! again by [`eco::reroute`], keeping what the change leaves alone, against routing the
//! changed board from nothing. Both are checked by our DRC and for connectivity.
//!
//! `TS4231-BOB` is changed in every test run. `GUIDE_ECO=1` does the boards of
//! [`ECO_BOARDS`], in a release build, and prints the table that `docs/benchmarks.md` has.
//! `GUIDE_BOARDS` names the boards to do, separated by commas.

use std::collections::BTreeSet;
use std::time::Instant;

use super::placement_tests::{BOARDS, load, stripped};
use super::{fixed_because, footprint_outline, placement_design, replace_placement};
use crate::geom::drc::{self, Item, Pad, Track, Via};
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::NetId;
use crate::pcb::eco::{self, Change};
use crate::pcb::place::{Placed, Side, edge_segments, near_segments, outline_overlaps};
use crate::pcb::{self, Design, RouteOptions, Routed};

/// The boards changed with `GUIDE_ECO=1`: those with parts to move.
const ECO_BOARDS: [&str; 9] = [
    "TS4231-BOB",
    "lisn",
    "placebo",
    "udb-s",
    "flea",
    "GB-LIVE32",
    "nk3-nrf-mini",
    "lpddr4-testbed",
    "ddr5-testbed",
];

/// A change to a board: what it is, the board after it, and what changed.
struct Eco {
    what: String,
    design: Design,
    change: Change,
}

/// Where every footprint of a board is.
fn where_they_are(read: &super::KicadBoard) -> Vec<Placed> {
    read.footprints
        .iter()
        .map(|f| Placed {
            at: f.at,
            rotation: f.angle,
            side: if f.back { Side::Back } else { Side::Front },
        })
        .collect()
}

/// The nets a footprint's pads are on, but those that zones connect.
fn signal_nets(read: &super::KicadBoard, design: &Design, f: usize) -> Vec<NetId> {
    let planes: BTreeSet<NetId> = design.zones.zones.iter().filter_map(|z| z.net).collect();
    let mut nets: Vec<NetId> = read.footprints[f]
        .pads
        .iter()
        .filter_map(|p| p.net)
        .filter(|n| !planes.contains(n))
        .collect();
    nets.sort_unstable();
    nets.dedup();
    nets
}

/// A footprint moved or turned, as KiCad would write it.
fn moved_part(
    pcb: &str,
    pro: &str,
    read: &super::KicadBoard,
    f: usize,
    to: Placed,
    what: String,
) -> Eco {
    let mut placed = where_they_are(read);
    placed[f] = to;
    let text = replace_placement(pcb, read, &placed);
    let again = super::read(&text, Some(pro)).unwrap();
    let design = stripped(&again);
    let nets = signal_nets(&again, &design, f);
    let areas = [footprint_outline(read, f), footprint_outline(&again, f)]
        .into_iter()
        .flatten()
        .collect();
    Eco {
        what,
        design,
        change: Change::new(nets, areas),
    }
}

/// The changes tried on a board, those it has room and nets for.
fn changes(pcb: &str, pro: &str, read: &super::KicadBoard, d0: &Design, r0: &Routed) -> Vec<Eco> {
    let mut out = Vec::new();
    out.push(Eco {
        what: "nothing".into(),
        design: d0.clone(),
        change: Change::default(),
    });
    let pd = placement_design(read);
    let movable: Vec<usize> = (0..read.footprints.len())
        .filter(|&f| fixed_because(read, f).is_none())
        .collect();
    let routed_nets: BTreeSet<NetId> = r0.tracks.iter().filter_map(|t| t.net).collect();
    // A part of two pins, both nets routed, turned half a turn: its pads change places.
    if let Some(&f) = movable.iter().find(|&&f| {
        let nets = signal_nets(read, d0, f);
        read.footprints[f].pads.len() == 2
            && nets.len() == 2
            && nets.iter().all(|n| routed_nets.contains(n))
    }) {
        let fp = &read.footprints[f];
        let to = Placed {
            at: fp.at,
            rotation: fp.angle + 180_000,
            side: if fp.back { Side::Back } else { Side::Front },
        };
        let mut e = moved_part(pcb, pro, read, f, to, format!("{} turned", fp.reference));
        e.what = format!("{} turned 180°", fp.reference);
        out.push(e);
    }
    // The part of most pins, up to 16, moved by 1 mm where it fits.
    let segments = edge_segments(&read.routing.outline);
    let mut by_pins: Vec<usize> = movable
        .iter()
        .copied()
        .filter(|&f| read.footprints[f].pads.len() <= 16)
        .collect();
    by_pins.sort_by_key(|&f| (core::cmp::Reverse(read.footprints[f].pads.len()), f));
    'parts: for &f in &by_pins {
        let fp = &read.footprints[f];
        let here = where_they_are(read);
        let before: BTreeSet<(usize, usize)> = outline_overlaps(&pd, &here).into_iter().collect();
        for (dx, dy) in [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (-1, -1),
            (1, -1),
            (-1, 1),
        ] {
            let mut placed = here.clone();
            placed[f].at = Point::new(fp.at.x + dx * 1_000_000, fp.at.y + dy * 1_000_000);
            let new = outline_overlaps(&pd, &placed)
                .into_iter()
                .any(|o| !before.contains(&o) && (o.0 == f || o.1 == f));
            let outline = footprint_outline(read, f).map(|o| {
                Rect::new(
                    Point::new(o.min().x + dx * 1_000_000, o.min().y + dy * 1_000_000),
                    Point::new(o.max().x + dx * 1_000_000, o.max().y + dy * 1_000_000),
                )
            });
            let near_edge = outline.is_none_or(|o| {
                near_segments(&segments, &o, read.routing.edge_clearance + 250_000)
            });
            if !new && !near_edge {
                let what = format!("{} moved 1 mm", fp.reference);
                out.push(moved_part(pcb, pro, read, f, placed[f], what));
                break 'parts;
            }
        }
    }
    // A net of two pads, routed, removed: its pads have no net.
    let planes: BTreeSet<NetId> = d0.zones.zones.iter().filter_map(|z| z.net).collect();
    let pads_of = |d: &Design, n: NetId| -> Vec<usize> {
        (0..d.board.pads.len())
            .filter(|&p| d.board.pads[p].net == Some(n))
            .collect()
    };
    if let Some(&net) = routed_nets
        .iter()
        .find(|&&n| pads_of(d0, n).len() == 2 && !planes.contains(&n))
    {
        let mut design = d0.clone();
        for p in pads_of(d0, net) {
            design.board.pads[p].net = None;
        }
        out.push(Eco {
            what: format!("net {} removed", read.nets[net.index()]),
            design,
            change: Change::new(vec![net], Vec::new()),
        });
    }
    // A net added between the two nearest pads of nets of one pad each, on different
    // footprints and the outer layers.
    let bottom = LayerId((d0.rules.layer_count() - 1) as u8);
    let outer = LayerSet::EMPTY.with(LayerId(0)).with(bottom);
    let mut count = vec![0usize; d0.nets.len()];
    for p in &d0.board.pads {
        if let Some(n) = p.net.filter(|n| n.index() < count.len()) {
            count[n.index()] += 1;
        }
    }
    let owner = |p: usize| {
        read.footprints
            .iter()
            .position(|f| f.board_pads.contains(&p))
    };
    let lonely: Vec<usize> = (0..d0.board.pads.len())
        .filter(|&p| {
            let pad = &d0.board.pads[p];
            pad.net
                .is_some_and(|n| count[n.index()] == 1 && d0.nets[n.index()].route)
                && pad.layers.0 & outer.0 != 0
        })
        .collect();
    let centre = |p: usize| {
        let b = d0.board.pads[p].shape.bbox();
        Point::new((b.min().x + b.max().x) / 2, (b.min().y + b.max().y) / 2)
    };
    let mut pair: Option<(i64, usize, usize)> = None;
    for (k, &a) in lonely.iter().enumerate() {
        for &b in &lonely[k + 1..] {
            if owner(a) == owner(b) {
                continue;
            }
            let (pa, pb) = (centre(a), centre(b));
            let d = (pa.x - pb.x).abs() + (pa.y - pb.y).abs();
            if d <= 15_000_000 && pair.is_none_or(|(e, _, _)| d < e) {
                pair = Some((d, a, b));
            }
        }
    }
    if let Some((_, a, b)) = pair {
        let mut design = d0.clone();
        let net = NetId(design.nets.len() as u32);
        let spec = design.nets[design.board.pads[a].net.unwrap().index()];
        design.nets.push(spec);
        design.board.pads[a].net = Some(net);
        design.board.pads[b].net = Some(net);
        out.push(Eco {
            what: format!(
                "net added, {} to {}",
                read.pads[a].description, read.pads[b].description
            ),
            design,
            change: Change::new(vec![net], Vec::new()),
        });
    }
    // A part added: two pads of 0.6 by 0.5 mm, 1 mm apart, on the front, between a routed
    // net of two pads or more and ground, or another such net, near the first's first pad.
    let ground = (0..read.nets.len())
        .map(|i| NetId(i as u32))
        .find(|n| read.nets[n.index()].to_ascii_uppercase().contains("GND"));
    let signals: Vec<NetId> = routed_nets
        .iter()
        .copied()
        .filter(|&n| pads_of(d0, n).len() >= 2 && !planes.contains(&n))
        .collect();
    if let (Some(&a), Some(b)) = (signals.first(), ground.or_else(|| signals.get(1).copied())) {
        let rooms: Vec<Rect> = (0..read.footprints.len())
            .filter(|&f| !read.footprints[f].back)
            .filter_map(|f| footprint_outline(read, f))
            .collect();
        let start = centre(pads_of(d0, a)[0]);
        let mut spots: Vec<(i64, Point)> = Vec::new();
        for i in -32..=32i64 {
            for j in -32..=32i64 {
                let p = Point::new(start.x + i * 250_000, start.y + j * 250_000);
                spots.push((i * i + j * j, p));
            }
        }
        spots.sort_unstable();
        let pad_at = |c: Point, net| Pad {
            shape: Shape::rect(Rect::new(
                Point::new(c.x - 300_000, c.y - 250_000),
                Point::new(c.x + 300_000, c.y + 250_000),
            ))
            .unwrap(),
            layers: LayerSet::EMPTY.with(LayerId(0)),
            hole: None,
            net: Some(net),
            clearance: None,
            stack: Vec::new(),
        };
        let free = |p: Point| {
            let body = Rect::new(
                Point::new(p.x - 1_100_000, p.y - 600_000),
                Point::new(p.x + 1_100_000, p.y + 600_000),
            );
            !near_segments(&segments, &body, read.routing.edge_clearance + 250_000)
                && !rooms.iter().any(|r| r.intersects(&body))
                && !d0
                    .board
                    .pads
                    .iter()
                    .any(|q| q.shape.bbox().intersects(&body.expand(200_000)))
                && !d0
                    .board
                    .masks
                    .iter()
                    .any(|m| m.shape.bbox().intersects(&body.expand(200_000)))
                && !d0
                    .board
                    .keepouts
                    .iter()
                    .any(|k| k.shape.bbox().intersects(&body))
                && segments
                    .iter()
                    .map(|&(a, b)| Rect::new(a, b))
                    .reduce(|a, b| a.union(&b))
                    .is_some_and(|board| {
                        board.intersects(&body) && {
                            let inner = Rect::new(
                                Point::new(board.min().x + 2_000_000, board.min().y + 2_000_000),
                                Point::new(board.max().x - 2_000_000, board.max().y - 2_000_000),
                            );
                            inner.min().x <= body.min().x
                                && body.max().x <= inner.max().x
                                && inner.min().y <= body.min().y
                                && body.max().y <= inner.max().y
                        }
                    })
        };
        if let Some(&(_, p)) = spots.iter().find(|&&(_, p)| free(p)) {
            let mut design = d0.clone();
            design
                .board
                .pads
                .push(pad_at(Point::new(p.x - 500_000, p.y), a));
            design
                .board
                .pads
                .push(pad_at(Point::new(p.x + 500_000, p.y), b));
            let mut nets = vec![a];
            if !planes.contains(&b) {
                nets.push(b);
            }
            let body = Rect::new(
                Point::new(p.x - 800_000, p.y - 250_000),
                Point::new(p.x + 800_000, p.y + 250_000),
            );
            out.push(Eco {
                what: format!(
                    "part added, {} to {}",
                    read.nets[a.index()],
                    read.nets[b.index()]
                ),
                design,
                change: Change::new(nets, vec![body]),
            });
        }
    }
    out
}

/// What our DRC finds wrong with routed copper, and the nets left apart that the routing
/// doesn't say are unrouted.
fn check(design: &Design, routed: &Routed) -> (usize, usize) {
    let board = routed.board(design);
    let ours = |i: Item| {
        matches!(i, Item::Track(t) if t >= design.board.tracks.len())
            || matches!(i, Item::Via(v) if v >= design.board.vias.len())
    };
    let found = drc::check(&board, &design.rules)
        .unwrap()
        .into_iter()
        .filter(|v| match *v {
            drc::Violation::Short { a, b, .. }
            | drc::Violation::Clearance { a, b, .. }
            | drc::Violation::HoleToHole { a, b, .. } => ours(a) || ours(b),
            drc::Violation::HoleClearance { hole, item, .. } => ours(hole) || ours(item),
            drc::Violation::Keepout { item, .. } | drc::Violation::MaskBridge { item, .. } => {
                ours(item)
            }
            _ => true,
        })
        .count();
    let apart = pcb::unconnected(&board, &design.rules)
        .into_iter()
        .filter(|n| !routed.unrouted.contains(n))
        .count();
    (found, apart)
}

/// The whole board's counts of a full routing, as [`eco::reroute`] counts them.
fn recount(design: &Design, routed: &mut Routed) {
    let board = routed.board(design);
    let (nets, done, needed, made, _) = pcb::connectivity(design, &board);
    let s = &mut routed.stats;
    (s.nets, s.routed, s.connections, s.connected) = (nets, done, needed, made);
}

fn diff(before: &Routed, tracks: &[Track], vias: &[Via]) -> String {
    let (gone, added) = eco::difference((&before.tracks, &before.vias), (tracks, vias));
    let total = eco::measure(&before.tracks, &before.vias).1.max(1.0);
    format!(
        "{:.0}% ({} vias)",
        100.0 * (gone.2 + added.2) / total,
        gone.1 + added.1
    )
}

fn row(routed: &Routed, seconds: f64) -> String {
    let s = &routed.stats;
    let (vias, track) = eco::measure(&routed.tracks, &routed.vias);
    format!(
        "{}/{}, {vias}, {:.0}, {seconds:.2}",
        s.connected,
        s.connections,
        track / 1e6
    )
}

/// Changes a board and routes it again both ways; returns the rows of the table, and what
/// went wrong.
fn board_ecos(name: &str, full: bool) -> (Vec<String>, Vec<String>) {
    let &(dir, _) = BOARDS.iter().find(|(_, n)| *n == name).unwrap();
    let (pcb, pro) = load(dir, name);
    let read = super::read(&pcb, Some(&pro)).unwrap();
    let d0 = stripped(&read);
    let options = RouteOptions::default();
    let mut r0 = pcb::route(&d0, &options).unwrap();
    recount(&d0, &mut r0);
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for e in changes(&pcb, &pro, &read, &d0, &r0) {
        let start = Instant::now();
        let inc = eco::reroute(
            &e.design,
            &r0.tracks,
            &r0.vias,
            &e.change.clone().after(&r0.unrouted),
            &options,
        )
        .unwrap();
        let seconds = start.elapsed().as_secs_f64();
        let (found, apart) = check(&e.design, &inc.routed);
        if found > 0 || apart > 0 {
            failures.push(format!(
                "{name}, {}: {found} violations, {apart} apart",
                e.what
            ));
        }
        let mut cells = vec![
            format!("`{name}`"),
            e.what.clone(),
            row(&inc.routed, seconds),
            diff(&r0, &inc.routed.tracks, &inc.routed.vias),
        ];
        if full {
            let start = Instant::now();
            let mut again = pcb::route(&e.design, &options).unwrap();
            let seconds = start.elapsed().as_secs_f64();
            recount(&e.design, &mut again);
            let (found, apart) = check(&e.design, &again);
            if found > 0 || apart > 0 {
                failures.push(format!("{name}, {}, full: {found}, {apart}", e.what));
            }
            cells.push(row(&again, seconds));
            cells.push(diff(&r0, &again.tracks, &again.vias));
        }
        if e.what == "nothing" && (inc.routed.tracks != r0.tracks || inc.routed.vias != r0.vias) {
            let (gone, _) = eco::difference(
                (&r0.tracks, &r0.vias),
                (&inc.routed.tracks, &inc.routed.vias),
            );
            let added: Vec<String> = inc
                .routed
                .vias
                .iter()
                .filter(|v| !r0.vias.contains(v))
                .map(|v| format!("via of {}", read.nets[v.net.unwrap().index()]))
                .chain(
                    inc.routed
                        .tracks
                        .iter()
                        .filter(|t| !r0.tracks.contains(t))
                        .map(|t| format!("track of {}", read.nets[t.net.unwrap().index()])),
                )
                .collect();
            failures.push(format!(
                "{name}: nothing changed, yet the routing did: {gone:?} gone, {added:?} added, rerouted {:?}",
                inc.rerouted
            ));
        }
        rows.push(format!("| {} |", cells.join(" | ")));
    }
    (rows, failures)
}

#[test]
fn a_small_board_takes_small_changes() {
    let (rows, failures) = board_ecos("TS4231-BOB", false);
    assert!(rows.len() >= 4, "{rows:#?}");
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Changes the boards of [`ECO_BOARDS`] and routes them again, incrementally and from
/// nothing, and prints the table of `docs/benchmarks.md`. Runs only with `GUIDE_ECO=1`, and
/// is meant for a release build.
#[test]
fn curated_boards_eco() {
    if std::env::var_os("GUIDE_ECO").is_none() {
        eprintln!("skipped: set GUIDE_ECO=1 to change the curated boards and route them again");
        return;
    }
    let only = std::env::var("GUIDE_BOARDS").ok();
    println!(
        "| Board | Change | Incremental: connections, vias, track, s | Copper changed | From nothing: connections, vias, track, s | Copper changed |"
    );
    println!("| --- | --- | --- | --- | --- | --- |");
    let mut failures = Vec::new();
    for name in ECO_BOARDS {
        if only
            .as_ref()
            .is_some_and(|o| !o.split(',').any(|b| b == name))
        {
            continue;
        }
        let (rows, f) = board_ecos(name, true);
        for r in rows {
            println!("{r}");
        }
        failures.extend(f);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
