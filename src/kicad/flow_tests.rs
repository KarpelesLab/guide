//! The placement–routing loop on the curated boards: each board placed from nothing, routed,
//! and placed again where routing found trouble, by [`place_and_route`], against the one-pass
//! flow it starts with and against the board as placed by hand, checked by our DRC and, with
//! `GUIDE_KICAD=1`, by KiCad's.
//!
//! `TS4231-BOB` goes round the loop in every test run. `GUIDE_LOOP=1` does all eleven, in a
//! release build, and prints the table that `docs/benchmarks.md` has. `GUIDE_BOARDS` names the
//! boards to do, separated by commas.

use std::collections::BTreeSet;
use std::time::Instant;

use super::placement_tests::{BOARDS, baseline, key, kicad_check, load, route};
use super::{FlowOptions, FlowReport, Score, place_and_route, placement_design};
use crate::geom::drc::{self, Item, Violation};

/// What our DRC finds wrong on a placed and routed board, as written, with routed items or
/// pads of footprints that moved, that the board as it was doesn't have; and the nets it
/// leaves apart.
fn check_text(
    text: &str,
    pro: &str,
    moved: &BTreeSet<String>,
    base: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let read = super::read(text, Some(pro)).unwrap();
    let ours = |item: Item| match item {
        Item::Track(_) | Item::Via(_) => true,
        Item::Pad(p) => read.pads.get(p).is_some_and(|s| {
            s.description
                .rsplit(" of ")
                .next()
                .is_some_and(|r| moved.contains(r))
        }),
        _ => false,
    };
    let found = drc::check(&read.board, &read.rules)
        .unwrap()
        .into_iter()
        .filter(|v| !base.contains(&key(v)))
        .filter(|v| match *v {
            Violation::Short { a, b, .. }
            | Violation::Clearance { a, b, .. }
            | Violation::HoleToHole { a, b, .. } => ours(a) || ours(b),
            Violation::HoleClearance { hole, item, .. } => ours(hole) || ours(item),
            Violation::Keepout { item, .. } => ours(item),
            Violation::MaskBridge { item, .. } => ours(item),
            _ => true,
        })
        .map(|v| format!("{v:?}"))
        .collect();
    let apart = crate::pcb::unconnected(&read.board, &read.rules)
        .into_iter()
        .map(|n| read.nets[n.index()].clone())
        .collect();
    (found, apart)
}

fn moved(pcb: &str, pro: &str) -> BTreeSet<String> {
    let read = super::read(pcb, Some(pro)).unwrap();
    placement_design(&read)
        .components
        .iter()
        .filter(|c| !c.fixed)
        .map(|c| c.name.clone())
        .collect()
}

fn cell(s: &Score) -> String {
    format!(
        "{}/{}, {}, {:.0}",
        s.connections.1,
        s.connections.0,
        s.vias,
        s.track / 1e6
    )
}

fn run(name: &str, pcb: &str, pro: &str) -> (FlowReport, f64) {
    let start = Instant::now();
    let report = place_and_route(pcb, Some(pro), &FlowOptions::default()).unwrap();
    let seconds = start.elapsed().as_secs_f64();
    if std::env::var_os("GUIDE_VERBOSE").is_some() {
        for (k, r) in report.rounds.iter().enumerate() {
            eprintln!("{name}: round {k}: {r:?}");
        }
    }
    (report, seconds)
}

#[test]
fn small_board_goes_round_the_loop() {
    let (dir, name) = BOARDS[1];
    let (pcb, pro) = load(dir, name);
    let (report, _) = run(name, &pcb, &pro);
    let base = baseline(&pcb, &pro);
    let moved = moved(&pcb, &pro);
    let (found, apart) = check_text(&report.text, &pro, &moved, &base);
    assert!(found.is_empty(), "{found:#?}");
    assert!(apart.is_empty(), "{apart:?}");
    let last = report
        .rounds
        .iter()
        .rev()
        .find(|r| r.kept)
        .map_or(report.first, |r| r.score);
    assert!(!report.first.better(&last, 3.0), "{report:?}");
    assert_eq!(last.connections.0, last.connections.1);
}

/// Places and routes all eleven boards by the loop, and prints the table of
/// `docs/benchmarks.md`. Runs only with `GUIDE_LOOP=1`, and is meant for a release build.
#[test]
fn curated_boards_loop() {
    if std::env::var_os("GUIDE_LOOP").is_none() {
        eprintln!("skipped: set GUIDE_LOOP=1 to place and route the curated boards in a loop");
        return;
    }
    let kicad = std::env::var_os("GUIDE_KICAD").is_some();
    let only = std::env::var("GUIDE_BOARDS").ok();
    println!(
        "| Board | By hand: connections, vias, track | One pass | Loop | Rounds kept/run | Parts moved, mean and most, mm | Seconds: one pass, loop | Our DRC | KiCad: one pass, loop |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut failures = Vec::new();
    for &(dir, name) in &BOARDS {
        if only
            .as_ref()
            .is_some_and(|o| !o.split(',').any(|b| b == name))
        {
            continue;
        }
        let (pcb, pro) = load(dir, name);
        let base = baseline(&pcb, &pro);
        let hand = route(&pcb, &pro, &BTreeSet::new(), &base);
        let (report, seconds) = run(name, &pcb, &pro);
        let moved = moved(&pcb, &pro);
        if let Some(out) = std::env::var_os("GUIDE_WRITE") {
            let dir = std::path::Path::new(&out);
            for (suffix, text) in [("one-pass", &report.first_text), ("loop", &report.text)] {
                let base = dir.join(format!("{name}-{suffix}"));
                std::fs::write(base.with_extension("kicad_pcb"), text).unwrap();
                std::fs::write(base.with_extension("kicad_pro"), &pro).unwrap();
            }
        }
        let last = report
            .rounds
            .iter()
            .rev()
            .find(|r| r.kept)
            .map_or(report.first, |r| r.score);
        let h = &hand.routed.stats;
        let (vias, track) = crate::pcb::eco::measure(&hand.routed.tracks, &hand.routed.vias);
        let mut found_all = Vec::new();
        for text in [&report.first_text, &report.text] {
            let (found, apart) = check_text(text, &pro, &moved, &base);
            found_all.extend(found);
            if !apart.is_empty() {
                eprintln!("{name}: left apart {apart:?}");
            }
        }
        let one = report.first_seconds.0 + report.first_seconds.1;
        // How far the loop moved parts from the first placement, as place::placement_change
        // measures it: Manhattan distance of the origin, and moved if turned or flipped too.
        let (mut shifted, mut total, mut most) = (0, 0i64, 0i64);
        for (a, b) in report.first_placed.iter().zip(&report.placed) {
            let d = (a.at.x - b.at.x).abs() + (a.at.y - b.at.y).abs();
            if a != b {
                shifted += 1;
            }
            total += d;
            most = most.max(d);
        }
        let mean = if shifted > 0 {
            total as f64 / shifted as f64 / 1e6
        } else {
            0.0
        };
        print!(
            "| `{name}` | {}/{}, {vias}, {:.0} | {} | {} | {}/{} | {shifted}, {mean:.1}, {:.1} | {one:.1}, {seconds:.1} | {} |",
            h.connected,
            h.connections,
            track / 1e6,
            cell(&report.first),
            cell(&last),
            report.rounds.iter().filter(|r| r.kept).count(),
            report.rounds.len(),
            most as f64 / 1e6,
            found_all.len(),
        );
        if !found_all.is_empty() {
            failures.push(format!("{name}: {found_all:?}"));
        }
        if kicad {
            let mut cells = Vec::new();
            for text in [&report.first_text, &report.text] {
                let (found, unconnected) = kicad_check(text, &hand.text, &pro, name, &moved);
                cells.push(format!("{}, {unconnected}", found.len()));
                if !found.is_empty() {
                    failures.push(format!("{name}, KiCad: {found:?}"));
                }
            }
            print!(" {} |", cells.join("; "));
        } else {
            print!(" - |");
        }
        println!();
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
