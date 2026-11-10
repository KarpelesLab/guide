//! The curated boards routed by `pcb::route` and by Freerouting, from the same stripped
//! board, measured by the same code here and checked by KiCad's own DRC.
//!
//! `scripts/freerouting/compare.sh` makes the stripped boards, their DSN files and
//! Freerouting's routed boards, then runs this test with `GUIDE_FREEROUTING` naming the
//! directory they are in. Without it, the test does nothing: nothing in a normal test run
//! needs Java or Freerouting.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use super::json::{self, Json};
use super::replace_routing;
use crate::geom::drc::{self, Board, Item, Violation};
use crate::geom::{LayerSet, Rules, Shape, SpatialIndex};
use crate::id::NetId;
use crate::pcb::{self, Design, Options};

/// The boards, in the order of `docs/benchmarks.md`.
const BOARDS: [&str; 11] = [
    "GB-BRK-LINK-A",
    "TS4231-BOB",
    "lisn",
    "isi-channel",
    "placebo",
    "udb-s",
    "flea",
    "GB-LIVE32",
    "nk3-nrf-mini",
    "lpddr4-testbed",
    "ddr5-testbed",
];

/// For each net with two pads or more, how many pads it has and how many islands of copper
/// they are on: pads that touch, or that its tracks, vias and zones join, are one island.
fn islands(board: &Board, rules: &Rules) -> BTreeMap<NetId, usize> {
    let mut items: BTreeMap<NetId, (usize, Vec<(Shape, LayerSet)>)> = BTreeMap::new();
    for p in &board.pads {
        if let Some(n) = p.net {
            let entry = items.entry(n).or_default();
            entry.0 += 1;
            entry.1.push((p.shape.clone(), p.layers));
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
        let shape = Shape::circle(v.at, v.diameter).ok();
        add(v.net, shape, LayerSet::span(v.from, v.to));
    }
    for z in &board.zones {
        add(z.net, Some(z.shape.clone()), LayerSet::EMPTY.with(z.layer));
    }
    let stack = rules.layers();
    let mut out = BTreeMap::new();
    let mut near = Vec::new();
    for (net, (pads, list)) in &items {
        if *pads < 2 {
            continue;
        }
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let index = SpatialIndex::bulk_load(list.iter().map(|(s, _)| s.bbox()));
        let mut parent: Vec<usize> = (0..list.len()).collect();
        for (i, (shape, layers)) in list.iter().enumerate() {
            index.query(&shape.bbox(), &mut near);
            for id in &near {
                let j = id.slot() as usize;
                if j <= i || layers.0 & list[j].1.0 & stack.0 == 0 {
                    continue;
                }
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b && shape.touches(&list[j].0) {
                    parent[a.max(b)] = a.min(b);
                }
            }
        }
        let mut roots: Vec<usize> = (0..*pads).map(|i| find(&mut parent, i)).collect();
        roots.sort_unstable();
        roots.dedup();
        out.insert(*net, roots.len());
    }
    out
}

/// What KiCad's DRC found on a routed board, at error severity.
struct KicadFound {
    /// Violations that involve a track or a via, by type. The stripped board has none, so
    /// every one is the router's.
    violations: BTreeMap<String, usize>,
    /// Violations of the stripped board itself, involving no track or via.
    others: usize,
    /// Connections KiCad finds missing.
    unconnected: usize,
}

fn kicad_drc(board: &Path, report: &Path) -> KicadFound {
    let run = std::process::Command::new("kicad-cli")
        .args(["pcb", "drc", "--format", "json", "--severity-error", "-o"])
        .arg(report)
        .arg(board)
        .output()
        .expect("kicad-cli runs");
    let text = std::fs::read_to_string(report)
        .unwrap_or_else(|_| panic!("kicad-cli wrote no report: {run:?}"));
    let root = json::parse(&text).unwrap();
    let mut found = KicadFound {
        violations: BTreeMap::new(),
        others: 0,
        unconnected: root
            .get("unconnected_items")
            .map_or(0, |u| u.as_array().len()),
    };
    for v in root.get("violations").map_or(&[][..], Json::as_array) {
        let items = v.get("items").map_or(&[][..], Json::as_array);
        let routed = items.iter().any(|i| {
            i.get("description")
                .and_then(Json::as_str)
                .is_some_and(|d| d.starts_with("Track") || d.starts_with("Via"))
        });
        if routed {
            let ty = v.get("type").and_then(Json::as_str).unwrap_or("?");
            *found.violations.entry(ty.to_string()).or_insert(0) += 1;
        } else {
            found.others += 1;
        }
    }
    found
}

/// One routed board, measured.
struct Measured {
    nets: (usize, usize),
    connections: (usize, usize),
    /// Violations of our DRC that involve a track or a via.
    ours: usize,
    kicad: KicadFound,
    vias: usize,
    length: f64,
    seconds: f64,
}

/// Measures a routed board against the stripped board it was routed from.
fn measure(stripped: &Design, routed: &Path, project: &str, seconds: f64) -> Measured {
    let text = std::fs::read_to_string(routed).unwrap();
    let read = super::read(&text, Some(project)).unwrap();
    let before = islands(&stripped.board, &stripped.rules);
    let after = islands(&read.board, &read.rules);
    let (mut nets, mut connections) = ((0, 0), (0, 0));
    for (net, &n) in &before {
        if n < 2 {
            continue;
        }
        let now = after.get(net).copied().unwrap_or(n);
        nets.1 += 1;
        nets.0 += usize::from(now == 1);
        connections.1 += n - 1;
        connections.0 += n - now;
    }
    let routed_item = |item: Item| matches!(item, Item::Track(_) | Item::Via(_));
    let ours = drc::check(&read.board, &read.rules)
        .unwrap()
        .into_iter()
        .filter(|v| match *v {
            Violation::Short { a, b, .. }
            | Violation::Clearance { a, b, .. }
            | Violation::HoleToHole { a, b, .. } => routed_item(a) || routed_item(b),
            Violation::HoleClearance { hole, item, .. } => routed_item(hole) || routed_item(item),
            Violation::Keepout { item, .. } | Violation::MaskBridge { item, .. } => {
                routed_item(item)
            }
            _ => true,
        })
        .count();
    std::fs::write(routed.with_extension("kicad_pro"), project).unwrap();
    let kicad = kicad_drc(routed, &routed.with_extension("drc.json"));
    Measured {
        nets,
        connections,
        ours,
        kicad,
        vias: read.board.vias.len(),
        length: super::routing_tests::length(&read.board.tracks),
        seconds,
    }
}

/// Freerouting's time for a run, from its log: the job's elapsed time, which leaves out
/// starting Java and reading the design. It reads as `2 minutes 0.03 seconds`, with hours
/// and minutes only when there are some.
fn freerouting_seconds(log: &str) -> f64 {
    let Some(elapsed) = log
        .split("(elapsed: ")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
    else {
        return f64::NAN;
    };
    let words: Vec<&str> = elapsed.split_whitespace().collect();
    let mut seconds = 0.0;
    for pair in words.chunks(2) {
        let (Some(n), Some(unit)) = (pair.first(), pair.get(1)) else {
            return f64::NAN;
        };
        let Ok(n) = n.parse::<f64>() else {
            return f64::NAN;
        };
        seconds += n * match unit.trim_end_matches('s') {
            "hour" => 3600.0,
            "minute" => 60.0,
            "second" => 1.0,
            _ => return f64::NAN,
        };
    }
    seconds
}

#[test]
fn freerouting_times_read() {
    let log = |e: &str| format!("Job 'X' finished with state: COMPLETED (elapsed: {e}, finished");
    assert_eq!(freerouting_seconds(&log("0.38 seconds")), 0.38);
    assert_eq!(freerouting_seconds(&log("2 minutes 0.5 seconds")), 120.5);
    assert_eq!(
        freerouting_seconds(&log("1 hour 1 minute 1 second")),
        3661.0
    );
    assert!(freerouting_seconds("no job").is_nan());
}

fn row(board: &str, tool: &str, m: &Measured) -> String {
    let kinds: Vec<String> = m
        .kicad
        .violations
        .iter()
        .map(|(k, n)| format!("{n} {k}"))
        .collect();
    let violations = m.kicad.violations.values().sum::<usize>();
    let kinds = if kinds.is_empty() {
        String::new()
    } else {
        format!(" ({})", kinds.join(", "))
    };
    format!(
        "| `{board}` | {tool} | {}/{} | {}/{} | {violations}{kinds} | {} | {} | {} | {:.0} | {:.2} |",
        m.nets.0,
        m.nets.1,
        m.connections.0,
        m.connections.1,
        m.kicad.unconnected,
        m.ours,
        m.vias,
        m.length,
        m.seconds,
    )
}

/// Routes each stripped board in `GUIDE_FREEROUTING` with `pcb::route`, writes it beside
/// Freerouting's, measures both and prints the table of `docs/benchmarks.md`. Freerouting's
/// runs are the tags in the directory's `runs` file, one per line, with each board's routed
/// board in `<board>.<tag>.kicad_pcb` and its log in `<board>.<tag>.log`.
#[test]
fn curated_boards_against_freerouting() {
    let Some(dir) = std::env::var_os("GUIDE_FREEROUTING") else {
        eprintln!("skipped: run scripts/freerouting/compare.sh to compare with Freerouting");
        return;
    };
    let dir = Path::new(&dir);
    let runs = std::fs::read_to_string(dir.join("runs")).unwrap_or_default();
    let runs: Vec<&str> = runs
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    println!(
        "| Board | Router | Nets | Connections | KiCad violations | KiCad unconnected | Our DRC | Vias | Track, mm | Seconds |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for name in BOARDS {
        let base = dir.join(name);
        let Ok(pcb) = std::fs::read_to_string(base.with_extension("kicad_pcb")) else {
            continue;
        };
        let pro = std::fs::read_to_string(base.with_extension("kicad_pro")).unwrap();
        let read = super::read(&pcb, Some(&pro)).unwrap();
        let design = Design::from_kicad(&read);
        assert!(
            design.board.tracks.is_empty() && design.board.vias.is_empty(),
            "{name}: the board isn't stripped"
        );

        let start = Instant::now();
        let routed = pcb::route(&design, &Options::default()).unwrap();
        let seconds = start.elapsed().as_secs_f64();
        let text = replace_routing(&pcb, &routed.tracks, &routed.vias, &read.nets, &read.rules);
        let ours = dir.join(format!("{name}.guide.kicad_pcb"));
        std::fs::write(&ours, text).unwrap();
        let m = measure(&design, &ours, &pro, seconds);
        println!("{}", row(name, "guide", &m));
        if m.kicad.others > 0 {
            eprintln!(
                "{name}: KiCad finds {} violations of no track or via",
                m.kicad.others
            );
        }

        for run in &runs {
            let board = dir.join(format!("{name}.{run}.kicad_pcb"));
            if !board.exists() {
                println!("| `{name}` | Freerouting, {run} | no result |");
                continue;
            }
            let log = std::fs::read_to_string(dir.join(format!("{name}.{run}.log")));
            let seconds = freerouting_seconds(&log.unwrap_or_default());
            let m = measure(&design, &board, &pro, seconds);
            println!("{}", row(name, &format!("Freerouting, {run}"), &m));
        }
    }
}
