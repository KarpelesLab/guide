//! Placing the curated boards: each board's movable footprints are placed again by
//! [`place_components`], from nothing, and the board is written back with
//! [`replace_placement`], read again, stripped of its routing, routed by [`pcb::route`] with its
//! zones, and checked by our DRC, and with `GUIDE_KICAD=1`, by KiCad's, courtyards included.
//! The same board as it was placed is routed too, to compare.
//!
//! Two small boards are placed and routed in every test run. `GUIDE_PLACE=1` does all eleven,
//! in a release build, and prints the table that `docs/benchmarks.md` has.

use std::collections::BTreeSet;
use std::time::Instant;

use super::json::{self, Json};
use super::{KicadBoard, fixed_because, placement_design, replace_placement};
use crate::geom::drc::{self, Item, Violation};
use crate::pcb::place::{PlaceOptions, PlaceReport, outline_overlaps, place_components};
use crate::pcb::{self, Design, Options, Routed};

const BOARDS: [(&str, &str); 11] = [
    ("gekkio", "GB-BRK-LINK-A"),
    ("dotbots", "TS4231-BOB"),
    ("tem-cell", "lisn"),
    ("starshipraider", "isi-channel"),
    ("placebo", "placebo"),
    ("udb-s", "udb-s"),
    ("bugs", "flea"),
    ("gekkio", "GB-LIVE32"),
    ("nitrokey", "nk3-nrf-mini"),
    ("lpddr4-testbed", "lpddr4-testbed"),
    ("ddr5-testbed", "ddr5-testbed"),
];

/// The UUIDs of routed items start with this.
const OURS: &str = "67756964-";

fn load(dir: &str, board: &str) -> (String, String) {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/kicad")
        .join(dir)
        .join(board);
    let pcb = std::fs::read_to_string(base.with_extension("kicad_pcb")).unwrap();
    let pro = std::fs::read_to_string(base.with_extension("kicad_pro")).unwrap();
    (pcb, pro)
}

/// The board with no routing, zones' outlines kept, as in the routing tests.
fn stripped(read: &KicadBoard) -> Design {
    let mut design = Design::from_kicad(read);
    design.board.tracks.clear();
    design.board.vias.clear();
    let zones = std::mem::take(&mut design.board.zones);
    design.board.zones = zones
        .into_iter()
        .zip(&read.zones)
        .filter(|(_, source)| source.description.starts_with("graphic"))
        .map(|(z, _)| z)
        .collect();
    design
}

/// What routing a board did, and what our DRC found wrong with routed items or with pads of
/// footprints in `moved`.
struct Outcome {
    routed: Routed,
    seconds: f64,
    violations: Vec<String>,
    text: String,
}

/// What tells a violation apart: its kind and its items.
fn key(v: &Violation) -> String {
    match *v {
        Violation::Short { a, b, .. } => format!("short {a:?} {b:?}"),
        Violation::Clearance { a, b, .. } => format!("clearance {a:?} {b:?}"),
        Violation::HoleToHole { a, b, .. } => format!("hole to hole {a:?} {b:?}"),
        Violation::HoleClearance { hole, item, .. } => format!("hole {hole:?} {item:?}"),
        Violation::MaskBridge { opening, item, .. } => format!("mask {opening} {item:?}"),
        _ => format!("{v:?}"),
    }
}

/// What our DRC finds on a board as it was, stripped of its routing: between its pads and
/// mask openings, which footprints bring with them wherever they are, such as a footprint's
/// own pads under one opening.
fn baseline(pcb: &str, pro: &str) -> BTreeSet<String> {
    let read = super::read(pcb, Some(pro)).unwrap();
    let design = stripped(&read);
    drc::check(&design.board, &design.rules)
        .unwrap()
        .iter()
        .map(key)
        .collect()
}

fn route(pcb: &str, pro: &str, moved: &BTreeSet<String>, baseline: &BTreeSet<String>) -> Outcome {
    let read = super::read(pcb, Some(pro)).unwrap();
    let design = stripped(&read);
    let start = Instant::now();
    let routed = pcb::route(&design, &Options::default()).unwrap();
    let seconds = start.elapsed().as_secs_f64();
    let board = routed.board(&design);
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
    let violations: Vec<Violation> = drc::check(&board, &design.rules)
        .unwrap()
        .into_iter()
        .filter(|v| !baseline.contains(&key(v)))
        .filter(|v| match *v {
            Violation::Short { a, b, .. }
            | Violation::Clearance { a, b, .. }
            | Violation::HoleToHole { a, b, .. } => ours(a) || ours(b),
            Violation::HoleClearance { hole, item, .. } => ours(hole) || ours(item),
            Violation::Keepout { item, .. } => ours(item),
            Violation::MaskBridge { item, .. } => ours(item) || matches!(item, Item::Zone(_)),
            _ => true,
        })
        .collect();
    let name = |i: Item| match i {
        Item::Track(_) | Item::Via(_) => format!("{i:?}"),
        _ => read
            .source(i)
            .map_or_else(|| format!("{i:?}"), |s| s.description.clone()),
    };
    let violations: Vec<String> = violations
        .iter()
        .map(|v| match *v {
            Violation::Short { a, b, .. } => format!("short: {} / {}", name(a), name(b)),
            Violation::Clearance { a, b, required, .. } => {
                format!("clearance {required}: {} / {}", name(a), name(b))
            }
            Violation::HoleToHole { a, b, .. } => {
                format!("hole to hole: {} / {}", name(a), name(b))
            }
            Violation::HoleClearance { hole, item, .. } => {
                format!("hole clearance: {} / {}", name(hole), name(item))
            }
            Violation::MaskBridge { item, .. } => format!("mask bridge: {}", name(item)),
            _ => format!("{v:?}"),
        })
        .collect();
    let unconnected = pcb::unconnected(&board, &design.rules);
    assert!(
        unconnected.iter().all(|n| routed.unrouted.contains(n)),
        "routed but not connected: {unconnected:?}"
    );
    let text =
        super::replace_routing_and_fills(pcb, &routed.tracks, &routed.vias, &routed.fills, &read);
    Outcome {
        routed,
        seconds,
        violations,
        text,
    }
}

/// Places a board's movable footprints, and returns the report, the placed board's text, the
/// references of what moved, and the seconds placement took.
fn place(pcb: &str, pro: &str, name: &str) -> (PlaceReport, String, BTreeSet<String>, f64) {
    let mut read = super::read(pcb, Some(pro)).unwrap();
    if std::env::var_os("GUIDE_NO_SILK").is_some() {
        read.silk_is_error = false;
    }
    let design = placement_design(&read);
    if std::env::var_os("GUIDE_VERBOSE").is_some() {
        for (i, fp) in read.footprints.iter().enumerate() {
            eprintln!(
                "{name}: {} {:?} {}",
                fp.reference,
                fixed_because(&read, i),
                fp.library
            );
        }
    }
    if std::env::var_os("GUIDE_VERBOSE").is_some() {
        let mm2 = |r: &crate::geom::Rect| {
            (r.max().x - r.min().x) as f64 * (r.max().y - r.min().y) as f64 / 1e12
        };
        for c in &design.components {
            eprintln!(
                "{name}: {} fixed {} outline {:?}",
                c.name,
                c.fixed,
                c.outline.as_ref().map(mm2)
            );
        }
        for k in &design.keepouts {
            eprintln!("{name}: keepout {:?} {:.2} mm2", k.1, mm2(&k.0));
        }
    }
    let start = Instant::now();
    let mut options = PlaceOptions::default();
    // For measuring other settings.
    if let Some(fill) = std::env::var("GUIDE_FILL")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        options.fill = fill;
    }
    if let Some(spacing) = std::env::var("GUIDE_SPACING")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        options.spacing = spacing;
    }
    let report = place_components(&design, &options).unwrap();
    let seconds = start.elapsed().as_secs_f64();
    let overlaps = outline_overlaps(&design, &report.placed);
    // Fixed parts may overlap each other as the designer left them; nothing moved may.
    let moved_overlaps: Vec<_> = overlaps
        .iter()
        .filter(|&&(a, b)| !design.components[a].fixed || !design.components[b].fixed)
        .map(|&(a, b)| (&design.components[a].name, &design.components[b].name))
        .collect();
    assert!(moved_overlaps.is_empty(), "{name}: {moved_overlaps:?}");
    let moved: BTreeSet<String> = design
        .components
        .iter()
        .filter(|c| !c.fixed)
        .map(|c| c.name.clone())
        .collect();
    let text = replace_placement(pcb, &read, &report.placed);
    // What was written is what was placed.
    let again = super::read(&text, Some(pro)).unwrap();
    for (c, (fp, p)) in again.footprints.iter().zip(&report.placed).enumerate() {
        assert_eq!(
            (fp.at, fp.angle, fp.back),
            (p.at, p.rotation, p.side == crate::pcb::place::Side::Back),
            "{name}: {}",
            fp.reference
        );
        let pins: Vec<_> = fp
            .pads
            .iter()
            .filter(|p| p.net.is_some())
            .map(|p| p.at)
            .collect();
        assert_eq!(
            pins,
            crate::pcb::place::pin_positions(&design, c, p),
            "{name}: {}",
            fp.reference
        );
    }
    (report, text, moved, seconds)
}

/// KiCad's findings on a board, at error severity: each violation's type, description and the
/// UUIDs of its items, and how many connections are missing.
fn kicad_drc(text: &str, project: &str, name: &str) -> (Vec<(String, Vec<String>)>, usize) {
    let dir = std::env::temp_dir().join(format!("guide-placed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let board = dir.join(format!("{name}.kicad_pcb"));
    let report = dir.join(format!("{name}.json"));
    std::fs::write(&board, text).unwrap();
    std::fs::write(board.with_extension("kicad_pro"), project).unwrap();
    let run = std::process::Command::new("kicad-cli")
        .args(["pcb", "drc", "--format", "json", "--severity-error"])
        .arg("-o")
        .arg(&report)
        .arg(&board)
        .output()
        .expect("kicad-cli runs");
    let report = std::fs::read_to_string(&report)
        .unwrap_or_else(|_| panic!("kicad-cli wrote no report: {run:?}"));
    let _ = std::fs::remove_dir_all(&dir);
    let root = json::parse(&report).unwrap();
    let mut found = Vec::new();
    for v in root.get("violations").map_or(&[][..], Json::as_array) {
        let items = v.get("items").map_or(&[][..], Json::as_array);
        let mut uuids: Vec<String> = items
            .iter()
            .filter_map(|i| i.get("uuid").and_then(Json::as_str).map(str::to_string))
            .collect();
        uuids.sort();
        // Between items of one footprint, such as its own pads under its own opening in the
        // mask, a violation is the footprint's, wherever it is.
        let owners: BTreeSet<Option<&str>> = items
            .iter()
            .map(|i| {
                let d = i.get("description").and_then(Json::as_str).unwrap_or("");
                d.split_once(" of ")
                    .and_then(|(_, r)| r.split_whitespace().next())
            })
            .collect();
        if owners.len() == 1 && owners.first().is_some_and(Option::is_some) {
            continue;
        }
        let ty = v.get("type").and_then(Json::as_str).unwrap_or("");
        let what = v.get("description").and_then(Json::as_str).unwrap_or("");
        found.push((format!("{ty}: {what}"), uuids));
    }
    let unconnected = root
        .get("unconnected_items")
        .map_or(0, |u| u.as_array().len());
    (found, unconnected)
}

/// KiCad's findings on a placed and routed board that involve a routed item, a footprint that
/// moved or one of its pads, and that KiCad doesn't find on the board as it was placed,
/// routed alike, where they are the footprints' own; and how many connections are missing.
fn kicad_check(
    text: &str,
    original: &str,
    project: &str,
    name: &str,
    moved: &BTreeSet<String>,
) -> (Vec<String>, usize) {
    let read = super::read(text, Some(project)).unwrap();
    let mut uuids: BTreeSet<String> = BTreeSet::new();
    for fp in &read.footprints {
        if moved.contains(&fp.reference) {
            uuids.extend(fp.uuid.clone());
            for p in fp.board_pads.clone() {
                uuids.extend(read.pads[p].uuid.clone());
            }
        }
    }
    let (before, _) = kicad_drc(original, project, name);
    let before: BTreeSet<(String, Vec<String>)> = before.into_iter().collect();
    let (after, unconnected) = kicad_drc(text, project, name);
    let found = after
        .into_iter()
        .filter(|(_, items)| {
            items
                .iter()
                .any(|u| u.starts_with(OURS) || uuids.contains(u))
        })
        .filter(|v| !before.contains(v))
        .map(|(what, _)| what)
        .collect();
    (found, unconnected)
}

fn summary(o: &Outcome) -> String {
    let s = &o.routed.stats;
    format!(
        "{}/{} | {} | {:.0} | {:.1}",
        s.connected,
        s.connections,
        s.after_cleanup.0,
        s.after_cleanup.1 / 1e6,
        o.seconds
    )
}

#[test]
fn small_curated_boards_place_and_route() {
    for &(dir, name) in &BOARDS[1..2] {
        let (pcb, pro) = load(dir, name);
        let (report, text, moved, _) = place(&pcb, &pro, name);
        assert!(report.movable > 0);
        let outcome = route(&text, &pro, &moved, &baseline(&pcb, &pro));
        assert!(outcome.routed.unrouted.is_empty(), "{name}");
        assert!(
            outcome.violations.is_empty(),
            "{name}: {:?}",
            outcome.violations
        );
    }
}

/// Places and routes all eleven boards, and routes them as they were placed, and prints the
/// table of `docs/benchmarks.md`. Runs only with `GUIDE_PLACE=1`, and is meant for a release
/// build. With `GUIDE_KICAD=1` as well, KiCad's DRC checks each placed and routed board, and
/// must find nothing wrong with what moved or was routed. `GUIDE_BOARDS` names the boards to
/// do, separated by commas, and `GUIDE_WRITE`, a directory, gets each placed board, routed.
#[test]
fn curated_boards_place() {
    if std::env::var_os("GUIDE_PLACE").is_none() {
        eprintln!("skipped: set GUIDE_PLACE=1 to place the curated boards");
        return;
    }
    let kicad = std::env::var_os("GUIDE_KICAD").is_some();
    let only = std::env::var("GUIDE_BOARDS").ok();
    println!(
        "| Board | Movable | Placing, s | HPWL, mm: was → placed | Connections: original, placed | Vias | Track, mm | Routing, s | DRC | KiCad |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
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
        let original = route(&pcb, &pro, &BTreeSet::new(), &base);
        let (report, text, moved, place_seconds) = place(&pcb, &pro, name);
        let placed = route(&text, &pro, &moved, &base);
        if let Some(out) = std::env::var_os("GUIDE_WRITE") {
            let base = std::path::Path::new(&out).join(name);
            std::fs::write(base.with_extension("kicad_pcb"), &placed.text).unwrap();
            std::fs::write(base.with_extension("kicad_pro"), &pro).unwrap();
        }
        let (o, p) = (&original.routed.stats, &placed.routed.stats);
        print!(
            "| `{name}` | {}/{} | {place_seconds:.2} | {:.0} → {:.0} | {}/{}, {}/{} | {} → {} | {:.0} → {:.0} | {:.1} → {:.1} | {} |",
            report.movable,
            report.placed.len(),
            report.wirelength.0 as f64 / 1e6,
            report.wirelength.1 as f64 / 1e6,
            o.connected,
            o.connections,
            p.connected,
            p.connections,
            o.after_cleanup.0,
            p.after_cleanup.0,
            o.after_cleanup.1 / 1e6,
            p.after_cleanup.1 / 1e6,
            original.seconds,
            placed.seconds,
            placed.violations.len(),
        );
        if std::env::var_os("GUIDE_VERBOSE").is_some() && !placed.violations.is_empty() {
            eprintln!("{name}: our DRC found {:#?}", placed.violations);
        }
        if !placed.violations.is_empty() {
            failures.push(format!("{name}: {:?}", placed.violations));
        }
        if kicad {
            let (found, unconnected) =
                kicad_check(&placed.text, &original.text, &pro, name, &moved);
            print!(" {} violations, {unconnected} unconnected |", found.len());
            if std::env::var_os("GUIDE_VERBOSE").is_some() {
                eprintln!("{name}: KiCad found {found:#?}");
            }
            if !found.is_empty() {
                failures.push(format!("{name}, KiCad: {found:?}"));
            }
            if placed.routed.unrouted.is_empty() && unconnected > 0 {
                failures.push(format!("{name}, KiCad: {unconnected} unconnected"));
            }
        } else {
            print!(" - |");
        }
        println!();
        if std::env::var_os("GUIDE_VERBOSE").is_some() {
            eprintln!(
                "{name}: original {}; placed {}; spacing {}, congestion {:.2}",
                summary(&original),
                summary(&placed),
                report.spacing,
                report.peak_congestion
            );
        }
        if !placed.routed.unrouted.is_empty() {
            let read = super::read(&placed.text, Some(&pro)).unwrap();
            let names: Vec<&str> = placed
                .routed
                .unrouted
                .iter()
                .map(|n| read.nets[n.index()].as_str())
                .collect();
            eprintln!("{name}: unrouted after placement {names:?}");
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
