//! Routing the curated boards: each board's tracks, vias and zone fills are taken out, its
//! nets are routed again by [`crate::pcb::route`], and the result is checked by our DRC and
//! for connectivity, and with `GUIDE_KICAD=1`, by KiCad's own DRC on the board written back.
//!
//! The three smallest boards route in every test run. `GUIDE_ROUTE=1` routes all eleven, in a
//! release build, and prints the table that `docs/benchmarks.md` has.

use std::time::Instant;

use super::json::{self, Json};
use super::{KicadBoard, replace_routing_and_fills};
use crate::geom::drc::{self, Item, Violation};
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

/// The UUIDs that [`replace_routing_and_fills`] gives new items start with this.
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

/// The board with no routing: no tracks, vias or zone fills, but copper graphics and zones'
/// outlines kept, as [`replace_routing_and_fills`] keeps them.
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

/// The options to route with: the defaults, but for `GUIDE_PLANES=0`, which routes nets
/// with zones as any others, and `GUIDE_PLANE_COST`, for measuring them.
fn options() -> Options {
    let mut options = Options::default();
    if std::env::var("GUIDE_PLANES").is_ok_and(|v| v == "0") {
        options.planes = false;
    }
    if let Some(cost) = std::env::var("GUIDE_PLANE_COST")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        options.plane_cost = cost;
    }
    options
}

/// Routes a board, checks it, and returns what it did with the routed board's text.
fn route_board(dir: &str, name: &'static str) -> (Row, String, Routed, String) {
    let (row, text, routed, pro, _, _) = route_board_with(dir, name, options(), |_, _| ());
    (row, text, routed, pro)
}

/// [`route_board`], with these options, and the design changed by `change` first, and also
/// returning the design and the board as read.
fn route_board_with(
    dir: &str,
    name: &'static str,
    options: Options,
    change: impl FnOnce(&KicadBoard, &mut Design),
) -> (Row, String, Routed, String, Design, KicadBoard) {
    let (pcb, pro) = load(dir, name);
    let read = super::read(&pcb, Some(&pro)).unwrap();
    let mut design = stripped(&read);
    change(&read, &mut design);
    let start = Instant::now();
    let routed = pcb::route(&design, &options).unwrap();
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
            // Zones' fills are the router's too.
            Violation::MaskBridge { item, .. } => ours(item) || matches!(item, Item::Zone(_)),
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
    let text = replace_routing_and_fills(&pcb, &routed.tracks, &routed.vias, &routed.fills, &read);
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
/// many connections are missing. With `refill`, KiCad fills the zones again itself first,
/// in place of the fills written.
fn kicad_check(text: &str, project: &str, name: &str, refill: bool) -> (Vec<String>, usize) {
    let dir = std::env::temp_dir().join(format!("guide-routed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let board = dir.join(format!("{name}.kicad_pcb"));
    let report = dir.join(format!("{name}.json"));
    std::fs::write(&board, text).unwrap();
    std::fs::write(board.with_extension("kicad_pro"), project).unwrap();
    let mut cmd = std::process::Command::new("kicad-cli");
    cmd.args(["pcb", "drc", "--format", "json", "--severity-error"]);
    if refill {
        cmd.arg("--refill-zones");
    }
    let run = cmd
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

/// Routes all eleven boards, and prints the table of `docs/benchmarks.md`. Runs only with
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
            for refill in [false, true] {
                let (found, unconnected) = kicad_check(&text, &pro, name, refill);
                let what = if refill { "refilled by KiCad" } else { "KiCad" };
                print!(
                    " {what}: {} violations, {unconnected} unconnected",
                    found.len()
                );
                if !found.is_empty() {
                    failures.push(format!("{name}, {what}: {found:?}"));
                }
                if routed.unrouted.is_empty() && unconnected > 0 {
                    failures.push(format!("{name}, {what}: {unconnected} unconnected"));
                }
            }
        }
        println!();
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The boards with differential pairs or lengths to match, with custom rules for them. The
/// boards have none of their own: these ask for what their designers did by hand.
/// `isi-channel` has three channels of two nets on separate connectors, matched in length
/// within each and of three lengths, about 55, 150 and 300 mm, and each of the next three
/// has a USB pair. The DRAM boards have their data and their command and address buses
/// each matched in length, the strobes and clocks with them, which their designers met to
/// within 0.05 mm but for a few nets of `ddr5-testbed`; and on that board, the pairs are
/// at the width and gap of its hand routing, 0.1016 and 0.25 mm, not its net class's.
const PAIRED: [(&str, &str, &str); 6] = [
    (
        "starshipraider",
        "isi-channel",
        "(version 1)
         (rule ch1 (condition \"A.inDiffPair('/CH1_')\")
           (constraint length (min 54.85mm) (opt 55.1mm) (max 55.35mm))
           (constraint skew (max 0.1mm) (within_diff_pairs)))
         (rule ch2 (condition \"A.inDiffPair('/CH2_')\")
           (constraint length (min 149.6mm) (opt 149.85mm) (max 150.1mm))
           (constraint skew (max 0.1mm) (within_diff_pairs)))
         (rule ch3 (condition \"A.inDiffPair('/CH3_')\")
           (constraint length (min 299.75mm) (opt 300mm) (max 300.25mm))
           (constraint skew (max 0.1mm) (within_diff_pairs)))",
    ),
    ("udb-s", "udb-s", USB_SKEW),
    ("gekkio", "GB-LIVE32", USB_SKEW),
    ("nitrokey", "nk3-nrf-mini", USB_SKEW),
    (
        "lpddr4-testbed",
        "lpddr4-testbed",
        "(version 1)
         (rule dq (condition \"A.NetName == 'DQ??_A' || A.NetName == 'DQ_S?_?A' || A.NetName == 'DMI_?A'\")
           (constraint skew (max 0.1mm)))
         (rule ca (condition \"A.NetName == 'CA?_A' || A.NetName == 'CKE0_A' || A.NetName == 'CS0_A' || A.NetName == 'CK_?_A'\")
           (constraint skew (max 0.1mm)))",
    ),
    (
        "ddr5-testbed",
        "ddr5-testbed",
        "(version 1)
         (rule dq (condition \"A.NetName == 'DQ?' || A.inDiffPair('DQS_') || A.inDiffPair('TDQS_')\")
           (constraint skew (max 0.1mm)))
         (rule ca (condition \"A.NetName == 'CA0' || A.NetName == 'CA1' || A.NetName == 'CA2' || A.NetName == 'CA3' || A.NetName == 'CA4' || A.NetName == 'CA5' || A.NetName == 'CA6' || A.NetName == 'CA7' || A.NetName == 'CA8' || A.NetName == 'CA9' || A.NetName == 'CA1?' || A.NetName == 'CS_N' || A.inDiffPair('CLK_')\")
           (constraint skew (max 0.1mm)))
         (rule pairs (condition \"A.inDiffPair('*')\")
           (constraint track_width (opt 0.1016mm))
           (constraint diff_pair_gap (opt 0.25mm)))",
    ),
];

/// The boards whose pairs are routed first, [`Options::pairs_first`]: on the DRAM boards,
/// routed last, they find no room through the buses around their BGAs.
const PAIRS_FIRST: [&str; 2] = ["lpddr4-testbed", "ddr5-testbed"];

/// A skew of 0.1 mm at most within each differential pair.
const USB_SKEW: &str = "(version 1)
    (rule usb (condition \"A.inDiffPair('*')\")
      (constraint skew (max 0.1mm) (within_diff_pairs)))";

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

/// How far a constraint's nets are from it, in millimetres: the most any is outside its
/// length, and the skew past its own, or zero.
fn length_error(c: &pcb::LengthConstraint, tracks: &[drc::Track]) -> (Vec<f64>, f64, f64) {
    let lengths: Vec<f64> = c
        .nets
        .iter()
        .map(|&n| pcb::net_length(tracks, n) / 1e6)
        .collect();
    let out = lengths
        .iter()
        .map(|&l| {
            let below = c.min.map_or(0.0, |m| m as f64 / 1e6 - l);
            let above = c.max.map_or(0.0, |m| l - m as f64 / 1e6);
            below.max(above).max(0.0)
        })
        .fold(0.0, f64::max);
    let skew = lengths.iter().copied().fold(0.0, f64::max)
        - lengths.iter().copied().fold(f64::MAX, f64::min);
    (lengths, out, skew)
}

/// Routes the boards with differential pairs, with lengths from [`PAIRED`]'s rules, and
/// prints how their pairs and lengths come out against the original hand routing. Runs
/// only with `GUIDE_ROUTE=1`, as [`curated_boards_route`] does, and with `GUIDE_KICAD=1`
/// checks the results with KiCad's DRC too.
#[test]
fn curated_pairs() {
    if std::env::var_os("GUIDE_ROUTE").is_none() {
        eprintln!("skipped: set GUIDE_ROUTE=1 to route the curated boards");
        return;
    }
    let kicad = std::env::var_os("GUIDE_KICAD").is_some();
    let only = std::env::var("GUIDE_BOARDS").ok();
    let mut failures = Vec::new();
    let mut pairs = Vec::new();
    let mut lengths = Vec::new();
    for &(dir, name, dru) in &PAIRED {
        if only
            .as_ref()
            .is_some_and(|o| !o.split(',').any(|b| b == name))
        {
            continue;
        }
        let options = Options {
            pairs_first: PAIRS_FIRST.contains(&name),
            ..options()
        };
        let (row, text, routed, pro, design, read) =
            route_board_with(dir, name, options, |read, d| {
                super::length_rules(dru, read).unwrap().apply(d);
            });
        // `GUIDE_APART=0` leaves out routing the nets apart, which takes as long again.
        let apart = if std::env::var("GUIDE_APART").is_ok_and(|v| v == "0") {
            routed.clone()
        } else {
            pcb::route(
                &design,
                &Options {
                    couple_pairs: false,
                    tune_lengths: false,
                    ..Options::default()
                },
            )
            .unwrap()
        };
        for pair in &design.pairs {
            let ours = pair_measures(&routed.tracks, &routed.vias, &design, pair);
            let plain = pair_measures(&apart.tracks, &apart.vias, &design, pair);
            let theirs = pair_measures(&read.board.tracks, &read.board.vias, &design, pair);
            let cell = |m: (f64, f64, f64, usize)| {
                format!(
                    "{:.0}% | {:.2} | {:.4} | {}",
                    100.0 * m.0,
                    m.1.max(m.2),
                    (m.1 - m.2).abs(),
                    m.3
                )
            };
            pairs.push(format!(
                "| `{name}` | `{}`, `{}` | {:.2}, {:.2} | {} | {} | {} |",
                read.nets[pair.positive.index()],
                read.nets[pair.negative.index()],
                pair.width as f64 / 1e6,
                pair.gap as f64 / 1e6,
                cell(ours),
                cell(plain),
                cell(theirs),
            ));
        }
        for c in &design.lengths {
            let names: Vec<&str> = c
                .nets
                .iter()
                .map(|n| read.nets[n.index()].as_str())
                .collect();
            let window = match (c.min, c.max, c.max_skew) {
                (Some(a), Some(b), _) => format!("{:.2} to {:.2}", a as f64 / 1e6, b as f64 / 1e6),
                (_, _, Some(s)) => format!("skew {:.2}", s as f64 / 1e6),
                _ => String::from("-"),
            };
            let cell = |(l, out, skew): (Vec<f64>, f64, f64)| {
                let l: Vec<String> = l.iter().map(|x| format!("{x:.2}")).collect();
                format!("{} | {:.4} | {:.4}", l.join(", "), out, skew)
            };
            lengths.push(format!(
                "| `{name}` | {} | {window} | {} | {} | {} |",
                names.join(", "),
                cell(length_error(c, &routed.tracks)),
                cell(length_error(c, &apart.tracks)),
                cell(length_error(c, &read.board.tracks)),
            ));
        }
        println!(
            "{} pairs side by side {:?}, lengths met {:?}, meanders {:.1} mm",
            row.markdown(),
            routed.stats.pairs,
            routed.stats.lengths,
            routed.stats.meanders / 1e6,
        );
        if let Some(out) = std::env::var_os("GUIDE_WRITE") {
            let base = std::path::Path::new(&out).join(name);
            std::fs::write(base.with_extension("kicad_pcb"), &text).unwrap();
            std::fs::write(base.with_extension("kicad_pro"), &pro).unwrap();
        }
        if kicad {
            let (found, unconnected) = kicad_check(&text, &pro, name, false);
            println!(
                "{name}: KiCad: {} violations, {unconnected} unconnected",
                found.len()
            );
            if !found.is_empty() {
                failures.push(format!("{name}: {found:?}"));
            }
        }
    }
    println!();
    println!(
        "| Board | Pair | Width, gap | Coupled | Longer, mm | Skew | Vias | Apart, no meanders: coupled | Longer | Skew | Vias | Original: coupled | Longer | Skew | Vias |"
    );
    println!(
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    );
    for p in pairs {
        println!("{p}");
    }
    println!();
    println!(
        "| Board | Nets | Constraint, mm | Lengths | Outside | Skew | Apart, no meanders: lengths | Outside | Skew | Original: lengths | Outside | Skew |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for l in lengths {
        println!("{l}");
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
