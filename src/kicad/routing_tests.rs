//! Routing the curated boards: each board's tracks, vias and zone fills are taken out, its
//! nets are routed again by [`crate::pcb::route`], and the result is checked by our DRC and
//! for connectivity, and with `GUIDE_KICAD=1`, by KiCad's own DRC on the board written back.
//!
//! The three smallest boards route in every test run. `GUIDE_ROUTE=1` routes all nine, in a
//! release build, and prints the table that `docs/benchmarks.md` has.

use std::time::Instant;

use super::json::{self, Json};
use super::{KicadBoard, replace_routing};
use crate::geom::drc::{self, Item, Violation};
use crate::pcb::{self, Design, Options, Routed};

const BOARDS: [(&str, &str); 9] = [
    ("gekkio", "GB-BRK-LINK-A"),
    ("dotbots", "TS4231-BOB"),
    ("tem-cell", "lisn"),
    ("starshipraider", "isi-channel"),
    ("placebo", "placebo"),
    ("udb-s", "udb-s"),
    ("bugs", "flea"),
    ("gekkio", "GB-LIVE32"),
    ("nitrokey", "nk3-nrf-mini"),
];

/// The UUIDs that [`replace_routing`] gives new items start with this.
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

/// The board with no routing: no tracks, vias or zone fills, but copper graphics kept, as
/// [`replace_routing`] keeps them.
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

/// The length of a board's tracks in millimetres, arcs as two chords through their middle.
pub(super) fn length(tracks: &[drc::Track]) -> f64 {
    let d = |a: crate::geom::Point, b: crate::geom::Point| {
        let (x, y) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
        (x * x + y * y).sqrt()
    };
    tracks
        .iter()
        .map(|t| match t.mid {
            None => d(t.start, t.end),
            Some(m) => d(t.start, m) + d(m, t.end),
        })
        .sum::<f64>()
        / 1e6
}

/// What routing one board did.
struct Row {
    name: &'static str,
    nets: (usize, usize),
    connections: (usize, usize),
    /// Violations of our DRC that involve a routed track or via.
    violations: usize,
    vias: (usize, usize, usize),
    length: (f64, f64, f64),
    seconds: f64,
}

impl Row {
    fn markdown(&self) -> String {
        format!(
            "| `{}` | {}/{} | {}/{} ({:.0}%) | {} | {} → {} ({}) | {:.0} → {:.0} ({:.0}) | {:.2} |",
            self.name,
            self.nets.0,
            self.nets.1,
            self.connections.0,
            self.connections.1,
            100.0 * self.connections.0 as f64 / self.connections.1.max(1) as f64,
            self.violations,
            self.vias.0,
            self.vias.1,
            self.vias.2,
            self.length.0,
            self.length.1,
            self.length.2,
            self.seconds,
        )
    }
}

/// Routes a board, checks it, and returns what it did with the routed board's text.
fn route_board(dir: &str, name: &'static str) -> (Row, String, Routed, String) {
    let (row, text, routed, pro, _, _) = route_board_with(dir, name, |_, _| ());
    (row, text, routed, pro)
}

/// [`route_board`], with the design changed by `change` first, and also returning the
/// design and the board as read.
fn route_board_with(
    dir: &str,
    name: &'static str,
    change: impl FnOnce(&KicadBoard, &mut Design),
) -> (Row, String, Routed, String, Design, KicadBoard) {
    let (pcb, pro) = load(dir, name);
    let read = super::read(&pcb, Some(&pro)).unwrap();
    let mut design = stripped(&read);
    change(&read, &mut design);
    let start = Instant::now();
    let routed = pcb::route(&design, &Options::default()).unwrap();
    let seconds = start.elapsed().as_secs_f64();
    let board = routed.board(&design);
    let ours = |item: Item| matches!(item, Item::Track(_) | Item::Via(_));
    let violations: Vec<Violation> = drc::check(&board, &design.rules)
        .unwrap()
        .into_iter()
        .filter(|v| match *v {
            Violation::Short { a, b, .. }
            | Violation::Clearance { a, b, .. }
            | Violation::HoleToHole { a, b, .. } => ours(a) || ours(b),
            Violation::HoleClearance { hole, item, .. } => ours(hole) || ours(item),
            Violation::Keepout { item, .. } => ours(item),
            _ => true,
        })
        .collect();
    assert!(violations.is_empty(), "{name}: {violations:?}");
    // Every net routed is connected, and no other.
    let unconnected = pcb::unconnected(&board, &design.rules);
    let missing: Vec<_> = unconnected
        .iter()
        .filter(|n| !routed.unrouted.contains(n))
        .collect();
    assert!(
        missing.is_empty(),
        "{name}: routed but not connected: {missing:?}"
    );
    if !routed.unrouted.is_empty() {
        let names: Vec<&str> = routed
            .unrouted
            .iter()
            .map(|n| read.nets[n.index()].as_str())
            .collect();
        eprintln!("{name}: unrouted {names:?}");
    }
    let s = routed.stats;
    let row = Row {
        name,
        nets: (s.routed, s.nets),
        connections: (s.connected, s.connections),
        violations: violations.len(),
        vias: (s.before_cleanup.0, s.after_cleanup.0, read.board.vias.len()),
        length: (
            s.before_cleanup.1 / 1e6,
            s.after_cleanup.1 / 1e6,
            length(&read.board.tracks),
        ),
        seconds,
    };
    let text = replace_routing(&pcb, &routed.tracks, &routed.vias, &read.nets, &read.rules);
    (row, text, routed, pro, design, read)
}

#[test]
fn small_curated_boards_route_completely() {
    for &(dir, name) in &BOARDS[..3] {
        let (row, _, routed, _) = route_board(dir, name);
        assert!(routed.unrouted.is_empty(), "{name}: {:?}", routed.unrouted);
        assert_eq!(row.violations, 0);
    }
}

/// What KiCad found on a board: violations that involve routed items, by type, and how
/// many connections are missing.
fn kicad_check(text: &str, project: &str, name: &str) -> (Vec<String>, usize) {
    let dir = std::env::temp_dir().join(format!("guide-routed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let board = dir.join(format!("{name}.kicad_pcb"));
    let report = dir.join(format!("{name}.json"));
    std::fs::write(&board, text).unwrap();
    std::fs::write(board.with_extension("kicad_pro"), project).unwrap();
    let run = std::process::Command::new("kicad-cli")
        .args(["pcb", "drc", "--format", "json", "--severity-error", "-o"])
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
        if items.iter().any(|i| {
            i.get("uuid")
                .and_then(Json::as_str)
                .is_some_and(|u| u.starts_with(OURS))
        }) {
            let ty = v.get("type").and_then(Json::as_str).unwrap_or("");
            let what = v.get("description").and_then(Json::as_str).unwrap_or("");
            found.push(format!("{ty}: {what}"));
        }
    }
    let unconnected = root
        .get("unconnected_items")
        .map_or(0, |u| u.as_array().len());
    (found, unconnected)
}

/// Routes all nine boards, and prints the table of `docs/benchmarks.md`. Runs only with
/// `GUIDE_ROUTE=1`, and is meant for a release build. With `GUIDE_KICAD=1` as well, it writes
/// each routed board and checks that `kicad-cli pcb drc` finds no violation of a routed
/// item, and a missing connection only where a net could not be routed. `GUIDE_BOARDS`, a
/// list of board names separated by commas, routes only those, and `GUIDE_WRITE`, a
/// directory, writes each routed board there.
#[test]
fn curated_boards_route() {
    if std::env::var_os("GUIDE_ROUTE").is_none() {
        eprintln!("skipped: set GUIDE_ROUTE=1 to route the curated boards");
        return;
    }
    let kicad = std::env::var_os("GUIDE_KICAD").is_some();
    println!("| Board | Nets | Connections | DRC | Vias | Length, mm | Seconds |");
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    let only = std::env::var("GUIDE_BOARDS").ok();
    let mut failures = Vec::new();
    for &(dir, name) in &BOARDS {
        if only
            .as_ref()
            .is_some_and(|o| !o.split(',').any(|b| b == name))
        {
            continue;
        }
        let (row, text, routed, pro) = route_board(dir, name);
        print!("{}", row.markdown());
        if let Some(out) = std::env::var_os("GUIDE_WRITE") {
            let base = std::path::Path::new(&out).join(name);
            std::fs::write(base.with_extension("kicad_pcb"), &text).unwrap();
            std::fs::write(base.with_extension("kicad_pro"), &pro).unwrap();
        }
        if kicad {
            let (found, unconnected) = kicad_check(&text, &pro, name);
            print!(
                " KiCad: {} violations, {unconnected} unconnected",
                found.len()
            );
            if !found.is_empty() {
                failures.push(format!("{name}: {found:?}"));
            }
            if routed.unrouted.is_empty() && unconnected > 0 {
                failures.push(format!("{name}: {unconnected} unconnected"));
            }
        }
        println!();
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The boards with differential pairs or lengths to match.
const PAIRED: [(&str, &str); 4] = [
    ("starshipraider", "isi-channel"),
    ("udb-s", "udb-s"),
    ("gekkio", "GB-LIVE32"),
    ("nitrokey", "nk3-nrf-mini"),
];

/// How a pair's two nets are routed: the fraction of the positive net's length at the
/// pair's gap, to within 5 µm, the two lengths in millimetres, and the vias of both.
fn pair_measures(
    tracks: &[drc::Track],
    vias: &[drc::Via],
    design: &Design,
    pair: &pcb::DiffPair,
) -> (f64, f64, f64, usize) {
    let rules = &design.rules;
    let gap = (0..rules.layer_count())
        .map(|l| {
            rules.clearance_between(
                Some(pair.positive),
                Some(pair.negative),
                crate::geom::LayerId(l as u8),
            )
        })
        .fold(pair.gap, i64::max);
    let (coupled, total) = pcb::coupling(tracks, pair.positive, pair.negative, gap, 5000);
    let (p, n) = (
        pcb::net_length(tracks, pair.positive) / 1e6,
        pcb::net_length(tracks, pair.negative) / 1e6,
    );
    let v = vias
        .iter()
        .filter(|v| v.net == Some(pair.positive) || v.net == Some(pair.negative))
        .count();
    (coupled / total.max(1.0), p, n, v)
}

/// Routes the boards with differential pairs, and prints how their pairs are routed
/// against the original hand routing. Runs only with `GUIDE_ROUTE=1`, as
/// [`curated_boards_route`] does, with `GUIDE_KICAD=1` too for KiCad's DRC.
#[test]
fn curated_pairs() {
    if std::env::var_os("GUIDE_ROUTE").is_none() {
        eprintln!("skipped: set GUIDE_ROUTE=1 to route the curated boards");
        return;
    }
    let kicad = std::env::var_os("GUIDE_KICAD").is_some();
    println!(
        "| Board | Pair | Gap, mm | Coupled | P, N length, mm | Skew, mm | Vias | Original: coupled | P, N length | Skew | Vias |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut failures = Vec::new();
    for &(dir, name) in &PAIRED {
        let (row, text, routed, pro, design, read) = route_board_with(dir, name, |_, _| ());
        for pair in &design.pairs {
            let ours = pair_measures(&routed.tracks, &routed.vias, &design, pair);
            let theirs = pair_measures(&read.board.tracks, &read.board.vias, &design, pair);
            println!(
                "| `{name}` | `{}`, `{}` | {:.3} | {:.0}% | {:.2}, {:.2} | {:.3} | {} | {:.0}% | {:.2}, {:.2} | {:.3} | {} |",
                read.nets[pair.positive.index()],
                read.nets[pair.negative.index()],
                pair.gap as f64 / 1e6,
                100.0 * ours.0,
                ours.1,
                ours.2,
                (ours.1 - ours.2).abs(),
                ours.3,
                100.0 * theirs.0,
                theirs.1,
                theirs.2,
                (theirs.1 - theirs.2).abs(),
                theirs.3,
            );
        }
        println!(
            "{} pairs side by side: {:?}",
            row.markdown(),
            routed.stats.pairs
        );
        if kicad {
            let (found, unconnected) = kicad_check(&text, &pro, name);
            println!(
                "{name}: KiCad: {} violations, {unconnected} unconnected",
                found.len()
            );
            if !found.is_empty() {
                failures.push(format!("{name}: {found:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn differential_pairs_are_read_with_their_class() {
    for (dir, name, expected) in [
        ("udb-s", "udb-s", vec![("D+", "D-", 250_000, 200_000)]),
        (
            "starshipraider",
            "isi-channel",
            vec![
                ("/CH1_P", "/CH1_N", 200_000, 250_000),
                ("/CH2_P", "/CH2_N", 200_000, 250_000),
                ("/CH3_P", "/CH3_N", 200_000, 250_000),
            ],
        ),
    ] {
        let (pcb, pro) = load(dir, name);
        let read = super::read(&pcb, Some(&pro)).unwrap();
        let pairs: Vec<(&str, &str, i64, i64)> = read
            .routing
            .pairs
            .iter()
            .map(|p| {
                (
                    read.nets[p.positive.index()].as_str(),
                    read.nets[p.negative.index()].as_str(),
                    p.width,
                    p.gap,
                )
            })
            .collect();
        assert_eq!(pairs, expected, "{name}");
        assert_eq!(Design::from_kicad(&read).pairs, read.routing.pairs);
    }
}
