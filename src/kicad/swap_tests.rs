//! Pin swaps on the curated boards: DQ bits within their byte lanes on the two DRAM boards,
//! as placed by hand, routed with and without them; and the two pins of passives on every
//! board, after placement.
//!
//! `GUIDE_SWAP=1` runs them, in a release build, and prints the tables that
//! `docs/benchmarks.md` has; with `GUIDE_KICAD=1`, KiCad's DRC checks the DRAM boards with
//! their swaps, routed. `GUIDE_BOARDS` names the boards to do, separated by commas.

use std::collections::BTreeSet;

use super::placement_tests::{BOARDS, baseline, kicad_check, load, route};
use super::{
    FixedBecause, KicadBoard, PadNetChange, fixed_because, pad_changes, placement_design,
    replace_pad_nets, swap_groups,
};
use crate::pcb::place::{PlaceOptions, Placed, Side, place_components};
use crate::pcb::swap::{SwapGroup, apply_swaps, optimize_swaps};

/// The DQ bits of each byte lane of a DRAM: on the footprint, not a connector, with the most
/// pads on nets named `DQ` and a number, such as `DQ3` or `DQ12_A`, a group of the pins whose
/// numbers divided by eight are the same.
fn dq_lanes(board: &KicadBoard) -> Vec<SwapGroup> {
    let bit = |name: &str| -> Option<u32> {
        let rest = name.rsplit('/').next()?.strip_prefix("DQ")?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let tail = &rest[digits.len()..];
        (!digits.is_empty() && (tail.is_empty() || tail.starts_with('_')))
            .then(|| digits.parse().ok())
            .flatten()
    };
    let Some(f) = (0..board.footprints.len())
        .filter(|&f| fixed_because(board, f) != Some(FixedBecause::Connector))
        .max_by_key(|&f| {
            board.footprints[f]
                .pads
                .iter()
                .filter(|p| p.net.is_some_and(|n| bit(&board.nets[n.index()]).is_some()))
                .count()
        })
    else {
        return Vec::new();
    };
    let mut lanes: std::collections::BTreeMap<u32, Vec<usize>> = Default::default();
    for (k, pad) in board.footprints[f]
        .pads
        .iter()
        .filter(|p| p.net.is_some())
        .enumerate()
    {
        if let Some(b) = pad.net.and_then(|n| bit(&board.nets[n.index()])) {
            lanes.entry(b / 8).or_default().push(k);
        }
    }
    lanes
        .into_values()
        .filter(|pins| pins.len() >= 2)
        .map(|pins| SwapGroup { component: f, pins })
        .collect()
}

fn hand(read: &KicadBoard) -> Vec<Placed> {
    read.footprints
        .iter()
        .map(|f| Placed {
            at: f.at,
            rotation: f.angle,
            side: if f.back { Side::Back } else { Side::Front },
        })
        .collect()
}

fn edits(changes: &[PadNetChange]) -> Vec<(usize, usize, Option<crate::id::NetId>)> {
    changes.iter().map(PadNetChange::edit).collect()
}

#[test]
fn swapped_pads_are_written_and_read_back() {
    let (dir, name) = BOARDS[1];
    let (pcb, pro) = load(dir, name);
    let read = super::read(&pcb, Some(&pro)).unwrap();
    let pd = placement_design(&read);
    let groups = swap_groups(&read, &pd);
    assert!(!groups.is_empty());
    // Every passive swapped, whether it pays or not.
    let mut swaps = Vec::new();
    for g in &groups {
        let c = &pd.components[g.component];
        for (a, b) in [(0, 1), (1, 0)] {
            swaps.push(crate::pcb::swap::PinSwap {
                component: g.component,
                pin: a,
                from: c.pins[a].net,
                to: c.pins[b].net,
            });
        }
    }
    let changes = pad_changes(&read, &swaps);
    assert_eq!(changes.len(), swaps.len());
    let text = replace_pad_nets(&pcb, &read, &edits(&changes));
    let again = super::read(&text, Some(&pro)).unwrap();
    for ch in &changes {
        let pad = &again.footprints[ch.footprint].pads[ch.pad_index];
        assert_eq!(again.nets[pad.net.unwrap().index()], ch.to, "{ch:?}");
    }
    // Nothing else changed.
    assert_eq!(again.board.pads.len(), read.board.pads.len());
    assert_eq!(again.board.tracks.len(), read.board.tracks.len());
}

/// Swaps DQ bits on the DRAM boards and passives' pins on every board, and prints the tables
/// of `docs/benchmarks.md`. Runs only with `GUIDE_SWAP=1`, and is meant for a release build.
#[test]
fn curated_boards_swap() {
    if std::env::var_os("GUIDE_SWAP").is_none() {
        eprintln!("skipped: set GUIDE_SWAP=1 to swap pins on the curated boards");
        return;
    }
    let only = std::env::var("GUIDE_BOARDS").ok();
    let kicad = std::env::var_os("GUIDE_KICAD").is_some();
    let wanted = |name: &str| {
        only.as_ref()
            .is_none_or(|o| o.split(',').any(|b| b == name))
    };
    let mut failures = Vec::new();
    println!("| Board | Two-pin passives | Swapped | HPWL, mm: placed → swapped |");
    println!("| --- | --- | --- | --- |");
    for &(dir, name) in &BOARDS {
        if !wanted(name) {
            continue;
        }
        let (pcb, pro) = load(dir, name);
        let read = super::read(&pcb, Some(&pro)).unwrap();
        let mut pd = placement_design(&read);
        let report = place_components(&pd, &PlaceOptions::default()).unwrap();
        let groups = swap_groups(&read, &pd);
        let swaps = optimize_swaps(&pd, &report.placed, &groups);
        let before = report.wirelength.1;
        apply_swaps(&mut pd, &swaps);
        let options = PlaceOptions::default();
        let after = crate::pcb::place::refine_components(
            &pd,
            &report.placed,
            report.spacing,
            &crate::pcb::place::Feedback::default(),
            &options,
        )
        .unwrap()
        .wirelength
        .0;
        println!(
            "| `{name}` | {} | {} | {:.1} → {:.1} |",
            groups.len(),
            swaps.len() / 2,
            before as f64 / 1e6,
            after as f64 / 1e6
        );
    }
    println!();
    println!(
        "| Board | DQ bits | Pads changed | Connections | Vias | Track, mm | Routing, s | Our DRC | KiCad |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for &(dir, name) in &BOARDS[9..] {
        if !wanted(name) {
            continue;
        }
        let (pcb, pro) = load(dir, name);
        let read = super::read(&pcb, Some(&pro)).unwrap();
        let pd = placement_design(&read);
        let lanes = dq_lanes(&read);
        let placed = hand(&read);
        // As the designer assigned them, and reversed within each lane, as a netlist that
        // nobody assigned for routing might have them; each swapped for length.
        let mut scrambled_swaps = Vec::new();
        for g in &lanes {
            let pins = &pd.components[g.component].pins;
            for (k, &p) in g.pins.iter().enumerate() {
                let q = g.pins[g.pins.len() - 1 - k];
                if pins[p].net != pins[q].net {
                    scrambled_swaps.push(crate::pcb::swap::PinSwap {
                        component: g.component,
                        pin: p,
                        from: pins[p].net,
                        to: pins[q].net,
                    });
                }
            }
        }
        let mut scrambled = pd.clone();
        apply_swaps(&mut scrambled, &scrambled_swaps);
        let mut variants: Vec<(&str, Vec<crate::pcb::swap::PinSwap>)> = Vec::new();
        variants.push(("as designed", Vec::new()));
        variants.push(("as designed, swapped", optimize_swaps(&pd, &placed, &lanes)));
        variants.push(("reversed", scrambled_swaps.clone()));
        let mut fixed = scrambled.clone();
        apply_swaps(&mut fixed, &optimize_swaps(&scrambled, &placed, &lanes));
        // Relative to the board as it is, the pins whose nets end up other than they were.
        let mut both = Vec::new();
        for g in &lanes {
            for &p in &g.pins {
                let (was, now) = (
                    pd.components[g.component].pins[p].net,
                    fixed.components[g.component].pins[p].net,
                );
                if was != now {
                    both.push(crate::pcb::swap::PinSwap {
                        component: g.component,
                        pin: p,
                        from: was,
                        to: now,
                    });
                }
            }
        }
        variants.push(("reversed, swapped", both));
        let moved: BTreeSet<String> = lanes
            .iter()
            .map(|g| read.footprints[g.component].reference.clone())
            .collect();
        let bits: usize = lanes.iter().map(|g| g.pins.len()).sum();
        let texts: Vec<String> = variants
            .iter()
            .map(|(_, swaps)| {
                let changes = pad_changes(&read, swaps);
                replace_pad_nets(&pcb, &read, &edits(&changes))
            })
            .collect();
        let outcomes: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = texts
                .iter()
                .enumerate()
                .map(|(k, text)| {
                    let (pro, moved) = (&pro, &moved);
                    s.spawn(move || {
                        let none = BTreeSet::new();
                        let m = if k == 0 { &none } else { moved };
                        route(text, pro, m, &baseline(text, pro))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for ((what, swaps), outcome) in variants.iter().zip(&outcomes) {
            let st = &outcome.routed.stats;
            let (v, t) = crate::pcb::eco::measure(&outcome.routed.tracks, &outcome.routed.vias);
            print!(
                "| `{name}`, {what} | {bits} | {} | {}/{} | {v} | {:.0} | {:.1} | {} |",
                swaps.len(),
                st.connected,
                st.connections,
                t / 1e6,
                outcome.seconds,
                outcome.violations.len()
            );
            if !outcome.violations.is_empty() {
                failures.push(format!("{name}, {what}: {:?}", outcome.violations));
            }
            if kicad {
                let (found, unconnected) =
                    kicad_check(&outcome.text, &outcomes[0].text, &pro, name, &moved);
                print!(" {} violations, {unconnected} unconnected |", found.len());
                if !found.is_empty() {
                    failures.push(format!("{name}, {what}, KiCad: {found:?}"));
                }
            } else {
                print!(" - |");
            }
            println!();
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
