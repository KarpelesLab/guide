//! The curated boards, checked by us and by KiCad.
//!
//! Each board in `testdata/kicad` is checked under its own rules, and under the same rules
//! with every net class's clearance raised, which turns the same geometry into many more
//! findings. KiCad's findings for each are recorded beside the board, so that
//! [`curated_boards_match_recorded_kicad_results`] runs without KiCad. With
//! `GUIDE_KICAD=1`, [`curated_boards_agree_with_kicad`] runs `kicad-cli` itself, and checks
//! the recordings too; `GUIDE_KICAD_UPDATE=1` rewrites them.
//!
//! A finding is a kind (`clearance`, `short`, `hole_clearance` or `hole_to_hole`) and the
//! UUIDs of the two items. Ours and KiCad's must be the same, except for the differences
//! that [`explain`] accounts for, each a known way in which KiCad 10.0.3 checks less, or
//! something we don't read. `docs/benchmarks.md` has the results and the reasons.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::json::{self, Json};
use super::sexpr::{self, Sexp};
use super::units::mm_to_nm;
use super::*;
use crate::geom::drc::{KICAD_CLEARANCE_SLACK, Violation};

/// The curated boards: their directory under `testdata/kicad`, their name, and by how much
/// each scenario raises every net class's clearance, in nanometres. Scenarios stop where
/// KiCad would reach its limit of 499 clearance violations, after which it stops looking.
const BOARDS: &[(&str, &str, &[i64])] = &[
    ("gekkio", "GB-BRK-LINK-A", &[0, 100_000, 300_000, 500_000]),
    ("gekkio", "GB-LIVE32", &[0, 50_000]),
    ("dotbots", "TS4231-BOB", &[0, 100_000, 300_000, 500_000]),
    ("tem-cell", "lisn", &[0, 100_000, 300_000, 500_000]),
    ("placebo", "placebo", &[0, 100_000, 200_000]),
    ("bugs", "flea", &[0, 50_000]),
    ("nitrokey", "nk3-nrf-mini", &[0, 50_000]),
    ("udb-s", "udb-s", &[0, 100_000, 300_000]),
    ("starshipraider", "isi-channel", &[0, 500_000]),
];

/// KiCad stops reporting a kind of violation once it has this many: 499 for clearances,
/// and 199 for the rest (`EXTENDED_ERROR_LIMIT` and `ERROR_LIMIT` in `drc_engine.cpp`).
fn kicad_limit(kind: &str) -> usize {
    if kind == "clearance" { 499 } else { 199 }
}

/// The arc tolerance for comparisons, 10 nm, which keeps our arcs' error far inside
/// KiCad's slack of 500 nm.
const ARC_TOLERANCE: i64 = 10;

fn scenario_name(extra: i64) -> String {
    if extra == 0 {
        "own".into()
    } else {
        format!("plus-{}mm", extra as f64 / 1e6)
    }
}

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/kicad")
}

/// A finding: its kind, and the UUIDs of its two items, sorted.
type Finding = (String, String, String);

fn finding(kind: &str, a: &str, b: &str) -> Finding {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    (kind.to_string(), a.to_string(), b.to_string())
}

/// Millimetres with six decimals.
fn mm(v: i64) -> String {
    let sign = if v < 0 { "-" } else { "" };
    format!("{sign}{}.{:06}", v.abs() / 1_000_000, v.abs() % 1_000_000)
}

/// The project with every net class's clearance raised by `extra`.
fn raise_clearances(project: &str, extra: i64) -> String {
    let mut root = json::parse(project).unwrap();
    if let Json::Object(members) = &mut root
        && let Some((_, Json::Object(settings))) =
            members.iter_mut().find(|(k, _)| k == "net_settings")
        && let Some((_, Json::Array(classes))) = settings.iter_mut().find(|(k, _)| k == "classes")
    {
        for class in classes {
            if let Json::Object(fields) = class
                && let Some((_, Json::Number(n))) =
                    fields.iter_mut().find(|(k, _)| k == "clearance")
            {
                *n = mm(mm_to_nm(n).unwrap() + extra);
            }
        }
    }
    let mut out = String::new();
    root.write(&mut out);
    out
}

fn load(dir: &str, board: &str) -> (String, String) {
    let base = testdata().join(dir).join(board);
    let pcb = std::fs::read_to_string(base.with_extension("kicad_pcb")).unwrap();
    let pro = std::fs::read_to_string(base.with_extension("kicad_pro")).unwrap();
    (pcb, pro)
}

/// What we know of an item, for explaining differences.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Arc,
    Track,
    Via,
    Pad,
    /// A zone's fill, with whether the zone is a teardrop.
    Fill(bool),
    Graphic,
}

/// Our check of a board: our findings, with the clearance each needed, and what kind of
/// item each UUID is.
struct Ours {
    findings: BTreeMap<Finding, i64>,
    kinds: BTreeMap<String, Kind>,
}

/// The kind of one of our findings, as KiCad would report it, and its items.
///
/// KiCad reports touching copper as a short only where it can name two nets, so not
/// between two pads one of which has no net (`testPadAgainstItem`), nor between a zone
/// and anything (`testItemAgainstZone`); it calls those clearance violations, as it does
/// copper of no net touching other copper of no net.
fn kind_of(
    read: &KicadBoard,
    kinds: &BTreeMap<String, Kind>,
    v: &Violation,
) -> Option<(&'static str, Item, Item)> {
    let net = |item: Item| match item {
        Item::Track(i) => read.board.tracks[i].net,
        Item::Via(i) => read.board.vias[i].net,
        Item::Pad(i) => read.board.pads[i].net,
        Item::Zone(i) => read.board.zones[i].net,
        Item::Keepout(_) => None,
    };
    let netless = |item: Item| net(item).is_none_or(|n| read.nets[n.index()].is_empty());
    let kind = |item: Item| {
        read.source(item)
            .and_then(|s| s.uuid.as_ref())
            .and_then(|u| kinds.get(u))
            .cloned()
    };
    Some(match *v {
        Violation::Short { a, b, .. } => {
            let fill =
                matches!(kind(a), Some(Kind::Fill(_))) || matches!(kind(b), Some(Kind::Fill(_)));
            let pads = matches!(a, Item::Pad(_)) && matches!(b, Item::Pad(_));
            if fill || (pads && (netless(a) || netless(b))) || (netless(a) && netless(b)) {
                ("clearance", a, b)
            } else {
                ("short", a, b)
            }
        }
        Violation::Clearance { a, b, .. } => ("clearance", a, b),
        Violation::HoleClearance { hole, item, .. } => ("hole_clearance", hole, item),
        Violation::HoleToHole { a, b, .. } => ("hole_to_hole", a, b),
        _ => return None,
    })
}

/// Checks a board under the rules of `project`, as KiCad would decide.
fn ours(pcb: &str, project: &str) -> Ours {
    let mut read = read(pcb, Some(project)).unwrap();
    read.board.clearance_slack = KICAD_CLEARANCE_SLACK;
    read.board.arc_tolerance = ARC_TOLERANCE;
    let mut kinds = BTreeMap::new();
    let teardrops = teardrop_zones(pcb);
    for (item, source) in (0..read.board.tracks.len())
        .map(Item::Track)
        .chain((0..read.board.vias.len()).map(Item::Via))
        .chain((0..read.board.pads.len()).map(Item::Pad))
        .chain((0..read.board.zones.len()).map(Item::Zone))
        .filter_map(|i| Some((i, read.source(i)?)))
    {
        let Some(uuid) = &source.uuid else { continue };
        let kind = match item {
            Item::Track(i) if read.board.tracks[i].mid.is_some() => Kind::Arc,
            Item::Track(_) => Kind::Track,
            Item::Via(_) => Kind::Via,
            Item::Pad(_) => Kind::Pad,
            Item::Zone(_) if source.description.starts_with("zone") => {
                Kind::Fill(teardrops.contains(uuid))
            }
            _ => Kind::Graphic,
        };
        kinds.insert(uuid.clone(), kind);
    }
    let found = drc::check(&read.board, &read.rules).unwrap();
    let uuid = |item: Item| {
        let source = read.source(item).unwrap();
        source
            .uuid
            .clone()
            .unwrap_or_else(|| format!("?{}", source.description))
    };
    let mut findings = BTreeMap::new();
    for v in &found {
        let Some((kind, a, b)) = kind_of(&read, &kinds, v) else {
            continue;
        };
        let required = match v {
            Violation::Clearance { required, .. }
            | Violation::HoleClearance { required, .. }
            | Violation::HoleToHole { required, .. } => *required,
            _ => 0,
        };
        let (a, b) = (uuid(a), uuid(b));
        if a != b {
            findings.entry(finding(kind, &a, &b)).or_insert(required);
        }
    }
    Ours { findings, kinds }
}

/// The UUIDs of the zones that are teardrops.
fn teardrop_zones(pcb: &str) -> BTreeSet<String> {
    let root = sexpr::parse(pcb).unwrap();
    zones(&root)
        .filter(|z| {
            z.child("attr").and_then(|a| a.child("teardrop")).is_some()
                || z.child("name")
                    .and_then(|n| n.arg(0))
                    .is_some_and(|n| n.starts_with("$teardrop"))
        })
        .filter_map(zone_uuid)
        .collect()
}

/// The zones of a board, as KiCad lists them for its DRC: the board's, then footprints'.
fn zones<'s, 'a>(root: &'s Sexp<'a>) -> impl Iterator<Item = &'s Sexp<'a>> {
    let board = root.children("zone");
    let footprints = root
        .args()
        .iter()
        .filter(|s| s.is("footprint") || s.is("module"))
        .flat_map(|f| f.children("zone"));
    board.chain(footprints)
}

fn zone_uuid(zone: &Sexp<'_>) -> Option<String> {
    zone.child("uuid")
        .or_else(|| zone.child("tstamp"))
        .and_then(|u| u.arg(0))
        .map(str::to_string)
}

/// The kind of finding of a type of KiCad violation, if it is one we compare. KiCad calls
/// two straight tracks whose centrelines cross a crossing, and two holes in the same place
/// co-located, which are a short and holes too close to each other here.
fn kicad_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "clearance" => "clearance",
        "shorting_items" | "tracks_crossing" => "short",
        "hole_clearance" => "hole_clearance",
        "hole_to_hole" | "holes_co_located" => "hole_to_hole",
        _ => return None,
    })
}

/// The findings of a KiCad DRC report, and its descriptions of them, after checking that
/// KiCad didn't reach any limit if `complete`.
fn kicad_findings(report: &str, complete: bool) -> BTreeMap<Finding, String> {
    let root = json::parse(report).unwrap();
    let mut out = BTreeMap::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for v in root.get("violations").map_or(&[][..], Json::as_array) {
        let ty = v.get("type").and_then(Json::as_str).unwrap_or("");
        *counts.entry(ty).or_default() += 1;
        let Some(kind) = kicad_kind(ty) else { continue };
        let items = v.get("items").map_or(&[][..], Json::as_array);
        let uuids: Vec<&str> = items
            .iter()
            .filter_map(|i| i.get("uuid").and_then(Json::as_str))
            .collect();
        if let [a, b] = uuids[..] {
            let names: Vec<&str> = items
                .iter()
                .filter_map(|i| i.get("description").and_then(Json::as_str))
                .collect();
            let description = v.get("description").and_then(Json::as_str).unwrap_or("");
            out.insert(
                finding(kind, a, b),
                format!("{description}: {}", names.join(" / ")),
            );
        }
    }
    for (ty, n) in counts.into_iter().filter(|_| complete) {
        if let Some(kind) = kicad_kind(ty) {
            assert!(
                n < kicad_limit(kind),
                "KiCad reached its limit of {ty} violations: {n}"
            );
        }
    }
    out
}

/// Runs `kicad-cli pcb drc` on a board with a project, and returns its report.
fn run_kicad(pcb: &str, project: &str, name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("guide-kicad-boards-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let board = dir.join(format!("{name}.kicad_pcb"));
    let report = dir.join(format!("{name}.json"));
    std::fs::write(&board, pcb).unwrap();
    std::fs::write(board.with_extension("kicad_pro"), project).unwrap();
    let run = std::process::Command::new("kicad-cli")
        .args([
            "pcb",
            "drc",
            "--format",
            "json",
            "--severity-all",
            "--all-track-errors",
            "-o",
        ])
        .arg(&report)
        .arg(&board)
        .output()
        .expect("kicad-cli runs");
    let text = std::fs::read_to_string(&report);
    let _ = std::fs::remove_dir_all(&dir);
    text.unwrap_or_else(|_| panic!("kicad-cli wrote no report: {run:?}"))
}

/// What is recorded of KiCad on one board and scenario: its findings, and those of ours
/// on arcs that KiCad misses but finds once the arcs are split into short segments.
#[derive(Default, PartialEq, Eq, Debug)]
struct Recorded {
    findings: BTreeSet<Finding>,
    arcs: BTreeSet<Finding>,
}

fn recorded_path(dir: &str, board: &str, extra: i64) -> PathBuf {
    testdata()
        .join(dir)
        .join(format!("{board}.{}.kicad-drc", scenario_name(extra)))
}

fn read_recorded(path: &Path) -> Recorded {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut recorded = Recorded::default();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words[..] {
            [k, a, b] => recorded.findings.insert(finding(k, a, b)),
            ["arc", k, a, b] => recorded.arcs.insert(finding(k, a, b)),
            _ => panic!("{}: bad line {line:?}", path.display()),
        };
    }
    recorded
}

fn write_recorded(path: &Path, extra: i64, recorded: &Recorded) {
    let mut text = format!(
        "# The clearance findings of kicad-cli pcb drc 10.0.3 on this board, with every net\n\
         # class's clearance raised by {} mm: the kind of finding, and the UUIDs of its items.\n\
         # Lines starting with `arc` are findings that KiCad misses on arcs, but makes once\n\
         # those arcs are split into segments within 5 nm of them.\n",
        extra as f64 / 1e6
    );
    for (k, a, b) in &recorded.findings {
        text += &format!("{k} {a} {b}\n");
    }
    for (k, a, b) in &recorded.arcs {
        text += &format!("arc {k} {a} {b}\n");
    }
    std::fs::write(path, text).unwrap();
}

/// Why a finding is ours only or KiCad's only, if it is one of the known differences.
fn explain(
    finding: &Finding,
    ours_only: bool,
    ours: &Ours,
    recorded: &Recorded,
    pcb: &str,
) -> Option<&'static str> {
    let (kind, a, b) = (finding.0.as_str(), &finding.1, &finding.2);
    let (ka, kb) = (ours.kinds.get(a), ours.kinds.get(b));
    if !ours_only {
        // KiCad checks text on copper, which isn't read here.
        return (ka.is_none() || kb.is_none()).then_some("text");
    }
    let arc = |k: Option<&Kind>| k == Some(&Kind::Arc);
    if (arc(ka) || arc(kb)) && recorded.arcs.contains(finding) {
        return Some("arc");
    }
    if kind == "hole_clearance" && ka == Some(&Kind::Pad) && kb == Some(&Kind::Pad) {
        return Some("pad holes");
    }
    if kind == "clearance"
        && ka == Some(&Kind::Fill(false))
        && kb == Some(&Kind::Fill(false))
        && !kicad_zones_violate(pcb, a, b, ours.findings[finding])
    {
        return Some("zone sweep");
    }
    None
}

/// Whether KiCad's `testZonesToZones` finds two zones' fills closer than `clearance`.
///
/// It compares two fills on a layer only if their bounding boxes meet, with no margin for
/// the clearance, so fills side by side across a gap are never compared. Then it takes
/// the edges of each fill, each from left to right, sorted by their left ends. For each
/// edge of the first zone, it measures the edges of the second in order, and stops at the
/// first whose left end is to the right of this edge's right end, which skips the edges
/// beyond it that are within the clearance.
fn kicad_zones_violate(pcb: &str, a: &str, b: &str, clearance: i64) -> bool {
    type Seg = ((i64, i64), (i64, i64));
    let root = sexpr::parse(pcb).unwrap();
    let all: Vec<&Sexp<'_>> = zones(&root).collect();
    let position = |u: &str| all.iter().position(|z| zone_uuid(z).as_deref() == Some(u));
    let (Some(ia), Some(ib)) = (position(a), position(b)) else {
        return true;
    };
    // KiCad takes the zone that comes first as the first.
    let (first, second) = if ia < ib {
        (all[ia], all[ib])
    } else {
        (all[ib], all[ia])
    };
    let edges = |zone: &Sexp<'_>, layer: &str| -> Vec<Seg> {
        let mut out = Vec::new();
        for fill in zone.children("filled_polygon") {
            if fill.child("layer").and_then(|l| l.arg(0)) != Some(layer) {
                continue;
            }
            let points: Vec<(i64, i64)> = fill
                .child("pts")
                .map_or(&[][..], Sexp::args)
                .iter()
                .filter(|p| p.is("xy"))
                .map(|p| {
                    (
                        mm_to_nm(p.arg(0).unwrap()).unwrap(),
                        mm_to_nm(p.arg(1).unwrap()).unwrap(),
                    )
                })
                .collect();
            for (i, &p) in points.iter().enumerate() {
                let q = points[(i + 1) % points.len()];
                out.push(if p.0 > q.0 { (q, p) } else { (p, q) });
            }
        }
        out.sort();
        out
    };
    let layers: BTreeSet<&str> = first
        .children("filled_polygon")
        .filter_map(|f| f.child("layer").and_then(|l| l.arg(0)))
        .collect();
    let bbox = |edges: &[Seg]| {
        edges
            .iter()
            .fold((i64::MAX, i64::MAX, i64::MIN, i64::MIN), |b, &(p, q)| {
                (
                    b.0.min(p.0),
                    b.1.min(p.1.min(q.1)),
                    b.2.max(q.0),
                    b.3.max(p.1.max(q.1)),
                )
            })
    };
    for layer in layers {
        let (refs, tests) = (edges(first, layer), edges(second, layer));
        // Fills whose boxes don't meet are never compared, however close they are.
        let (a, b) = (bbox(&refs), bbox(&tests));
        if refs.is_empty() || tests.is_empty() || a.0 > b.2 || b.0 > a.2 || a.1 > b.3 || b.1 > a.3 {
            continue;
        }
        for r in &refs {
            for t in &tests {
                if r.1.0 < t.0.0 {
                    break;
                }
                if (segment_distance(*r, *t) + 0.5).floor() < clearance as f64 {
                    return true;
                }
            }
        }
    }
    false
}

/// The distance between two segments, in `f64`.
fn segment_distance(a: ((i64, i64), (i64, i64)), b: ((i64, i64), (i64, i64))) -> f64 {
    let to = |p: (i64, i64)| (p.0 as f64, p.1 as f64);
    let point_seg = |p: (f64, f64), s: ((f64, f64), (f64, f64))| {
        let (d, e) = ((s.1.0 - s.0.0, s.1.1 - s.0.1), (p.0 - s.0.0, p.1 - s.0.1));
        let len = d.0 * d.0 + d.1 * d.1;
        let t = if len == 0.0 {
            0.0
        } else {
            ((e.0 * d.0 + e.1 * d.1) / len).clamp(0.0, 1.0)
        };
        (e.0 - t * d.0).hypot(e.1 - t * d.1)
    };
    let (a, b) = ((to(a.0), to(a.1)), (to(b.0), to(b.1)));
    let cross = |o: (f64, f64), p: (f64, f64), q: (f64, f64)| {
        (p.0 - o.0) * (q.1 - o.1) - (p.1 - o.1) * (q.0 - o.0)
    };
    let crossing = cross(a.0, a.1, b.0) * cross(a.0, a.1, b.1) < 0.0
        && cross(b.0, b.1, a.0) * cross(b.0, b.1, a.1) < 0.0;
    if crossing {
        return 0.0;
    }
    [
        point_seg(a.0, b),
        point_seg(a.1, b),
        point_seg(b.0, a),
        point_seg(b.1, a),
    ]
    .into_iter()
    .fold(f64::INFINITY, f64::min)
}

/// The comparison of one board and scenario.
#[derive(Default)]
struct Comparison {
    kicad: usize,
    ours: usize,
    common: usize,
    explained: BTreeMap<&'static str, usize>,
    unexplained: Vec<String>,
}

fn compare(
    label: &str,
    pcb: &str,
    ours: &Ours,
    kicad: &BTreeMap<Finding, String>,
    recorded: &Recorded,
) -> Comparison {
    let mut c = Comparison {
        kicad: kicad.len(),
        ours: ours.findings.len(),
        ..Comparison::default()
    };
    for f in ours.findings.keys() {
        if kicad.contains_key(f) {
            c.common += 1;
        } else if let Some(reason) = explain(f, true, ours, recorded, pcb) {
            *c.explained.entry(reason).or_default() += 1;
        } else {
            c.unexplained
                .push(format!("{label}: ours only: {} {} {}", f.0, f.1, f.2));
        }
    }
    for (f, description) in kicad {
        if ours.findings.contains_key(f) {
            continue;
        }
        if let Some(reason) = explain(f, false, ours, recorded, pcb) {
            *c.explained.entry(reason).or_default() += 1;
        } else {
            c.unexplained.push(format!(
                "{label}: KiCad's only: {} {} {}: {description}",
                f.0, f.1, f.2
            ));
        }
    }
    c
}

/// A row of the table in `docs/benchmarks.md`.
fn row(board: &str, extra: i64, c: &Comparison) -> String {
    let explained: Vec<String> = c
        .explained
        .iter()
        .map(|(r, n)| format!("{n} {r}"))
        .collect();
    format!(
        "| {board} | {} | {} | {} | {} | {} | {} |",
        if extra == 0 {
            "own".into()
        } else {
            format!("+{} mm", extra as f64 / 1e6)
        },
        c.kicad,
        c.ours,
        c.common,
        if explained.is_empty() {
            "-".into()
        } else {
            explained.join(", ")
        },
        c.unexplained.len()
    )
}

/// Our findings on the curated boards are KiCad's, as recorded, but for known differences.
#[test]
fn curated_boards_match_recorded_kicad_results() {
    let mut unexplained = Vec::new();
    for &(dir, board, extras) in BOARDS {
        let (pcb, pro) = load(dir, board);
        for &extra in extras {
            let recorded = read_recorded(&recorded_path(dir, board, extra));
            let ours = ours(&pcb, &raise_clearances(&pro, extra));
            let kicad = recorded
                .findings
                .iter()
                .map(|f| (f.clone(), String::new()))
                .collect();
            let label = format!("{board} {}", scenario_name(extra));
            let c = compare(&label, &pcb, &ours, &kicad, &recorded);
            eprintln!("{}", row(board, extra, &c));
            unexplained.extend(c.unexplained);
        }
    }
    assert!(unexplained.is_empty(), "{}", unexplained.join("\n"));
}

/// Compares our DRC with `kicad-cli`'s on the curated boards, live. Runs only with
/// `GUIDE_KICAD=1`, as it needs `kicad-cli` 10.0.3 on the path, and takes a minute or two.
/// With `GUIDE_KICAD_UPDATE=1` as well, it records what KiCad finds.
#[test]
fn curated_boards_agree_with_kicad() {
    if std::env::var_os("GUIDE_KICAD").is_none() {
        eprintln!("skipped: set GUIDE_KICAD=1 to compare with kicad-cli");
        return;
    }
    let update = std::env::var_os("GUIDE_KICAD_UPDATE").is_some();
    let mut problems = Vec::new();
    for &(dir, board, extras) in BOARDS {
        let (pcb, pro) = load(dir, board);
        for &extra in extras {
            let project = raise_clearances(&pro, extra);
            let kicad = kicad_findings(&run_kicad(&pcb, &project, board), true);
            let ours = ours(&pcb, &project);
            // The arcs that only we find, checked again by KiCad with every arc split.
            let arc = |u: &String| ours.kinds.get(u) == Some(&Kind::Arc);
            let suspects: BTreeSet<Finding> = ours
                .findings
                .keys()
                .filter(|f| !kicad.contains_key(*f) && (arc(&f.1) || arc(&f.2)))
                .cloned()
                .collect();
            let arcs = if suspects.is_empty() {
                BTreeSet::new()
            } else {
                let involved: BTreeSet<&String> = suspects
                    .iter()
                    .flat_map(|f| [&f.1, &f.2])
                    .filter(|u| arc(u))
                    .collect();
                let (split, owner) = split_arcs(&pcb, &involved);
                // Splitting multiplies findings, so KiCad may reach its limits, and miss
                // some: those would be left unexplained, never explained wrongly.
                let found: BTreeSet<Finding> =
                    kicad_findings(&run_kicad(&split, &project, board), false)
                        .into_keys()
                        .map(|(k, a, b)| {
                            let back = |u: String| owner.get(&u).cloned().unwrap_or(u);
                            finding(&k, &back(a), &back(b))
                        })
                        .collect();
                suspects.intersection(&found).cloned().collect()
            };
            let live = Recorded {
                findings: kicad.keys().cloned().collect(),
                arcs,
            };
            let path = recorded_path(dir, board, extra);
            if update {
                write_recorded(&path, extra, &live);
            } else if read_recorded(&path) != live {
                problems.push(format!("{}: KiCad's findings changed", path.display()));
            }
            let label = format!("{board} {}", scenario_name(extra));
            let c = compare(&label, &pcb, &ours, &kicad, &live);
            eprintln!("{}", row(board, extra, &c));
            problems.extend(c.unexplained);
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The board with the arc tracks of `arcs` split into segments within 5 nm of them, and
/// the arc that each segment's UUID comes from.
fn split_arcs(pcb: &str, arcs: &BTreeSet<&String>) -> (String, BTreeMap<String, String>) {
    let mut out = String::with_capacity(pcb.len());
    let mut owner = BTreeMap::new();
    let mut last = 0;
    for (start, end) in top_level_items(pcb) {
        let text = &pcb[start..end];
        if !text[1..].trim_start().starts_with("arc") {
            continue;
        }
        let item = sexpr::parse(text).unwrap();
        let point = |name: &str| {
            let c = item.child(name).unwrap();
            (
                mm_to_nm(c.arg(0).unwrap()).unwrap(),
                mm_to_nm(c.arg(1).unwrap()).unwrap(),
            )
        };
        let (s, m, e) = (point("start"), point("mid"), point("end"));
        let width = item.child("width").and_then(|w| w.arg(0)).unwrap();
        let layer = item.child("layer").and_then(|l| l.arg(0)).unwrap();
        let net = item.child("net").and_then(|n| n.arg(0)).unwrap_or("0");
        let net = if net.parse::<i64>().is_ok() {
            net.to_string()
        } else {
            format!("{net:?}")
        };
        let uuid = zone_uuid(&item).unwrap();
        if !arcs.contains(&uuid) {
            continue;
        }
        out += &pcb[last..start];
        last = end;
        let points = arc_polyline(s, m, e, 5.0);
        for w in points.windows(2) {
            let id = format!("ffffffff-ffff-ffff-ffff-{:012x}", owner.len());
            owner.insert(id.clone(), uuid.clone());
            out += &format!(
                "(segment (start {} {}) (end {} {}) (width {width}) (layer {layer:?}) (net {net}) (uuid {id:?}))\n",
                mm(w[0].0),
                mm(w[0].1),
                mm(w[1].0),
                mm(w[1].1)
            );
        }
    }
    out += &pcb[last..];
    (out, owner)
}

/// The byte ranges of the lists inside the file's list.
fn top_level_items(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut quoted, mut escaped) = (0, 0, false, false);
    for (i, c) in text.char_indices() {
        if quoted {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '(' => {
                depth += 1;
                if depth == 2 {
                    start = i;
                }
            }
            ')' => {
                if depth == 2 {
                    out.push((start, i + 1));
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    out
}

/// Points along the arc from `s` through `m` to `e`, with chords within `error` of it.
fn arc_polyline(s: (i64, i64), m: (i64, i64), e: (i64, i64), error: f64) -> Vec<(i64, i64)> {
    let f = |p: (i64, i64)| (p.0 as f64, p.1 as f64);
    let ((ax, ay), (bx, by), (cx, cy)) = (f(s), f(m), f(e));
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d == 0.0 {
        return vec![s, e];
    }
    let sq = |x: f64, y: f64| x * x + y * y;
    let ux = (sq(ax, ay) * (by - cy) + sq(bx, by) * (cy - ay) + sq(cx, cy) * (ay - by)) / d;
    let uy = (sq(ax, ay) * (cx - bx) + sq(bx, by) * (ax - cx) + sq(cx, cy) * (bx - ax)) / d;
    let r = (ax - ux).hypot(ay - uy);
    let angle = |x: f64, y: f64| (y - uy).atan2(x - ux);
    let tau = std::f64::consts::TAU;
    let (a0, am, a1) = (angle(ax, ay), angle(bx, by), angle(cx, cy));
    let mut sweep = (a1 - a0).rem_euclid(tau);
    if (am - a0).rem_euclid(tau) > sweep {
        sweep -= tau;
    }
    let step = 2.0 * (1.0 - error / r).clamp(-1.0, 1.0).acos();
    let n = ((sweep.abs() / step).ceil() as usize).max(1);
    let mut points: Vec<(i64, i64)> = (0..=n)
        .map(|i| {
            let t = a0 + sweep * i as f64 / n as f64;
            (
                (ux + r * t.cos()).round() as i64,
                (uy + r * t.sin()).round() as i64,
            )
        })
        .collect();
    points[0] = s;
    points[n] = e;
    points.dedup();
    points
}

/// Asserts that two lists are the same, showing the first difference.
fn same<T: PartialEq + std::fmt::Debug>(board: &str, what: &str, old: &[T], new: &[T]) {
    if let Some(i) = (0..old.len().min(new.len())).find(|&i| old[i] != new[i]) {
        panic!(
            "{board}: {what} {i} differ:\n  {:?}\n  {:?}",
            old[i], new[i]
        );
    }
    assert_eq!(old.len(), new.len(), "{board}: {what}");
}

/// Each curated board, saved again by `kicad-cli pcb upgrade` in KiCad 10's format, reads
/// as the same board: the same items, nets and rules. Runs only with `GUIDE_KICAD=1`.
#[test]
fn curated_boards_read_the_same_in_kicad_10_format() {
    if std::env::var_os("GUIDE_KICAD").is_none() {
        eprintln!("skipped: set GUIDE_KICAD=1 to compare with kicad-cli");
        return;
    }
    let dir = std::env::temp_dir().join(format!("guide-kicad-upgrade-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for &(subdir, board, _) in BOARDS {
        let (pcb, pro) = load(subdir, board);
        let path = dir.join(format!("{board}.kicad_pcb"));
        std::fs::write(&path, &pcb).unwrap();
        std::fs::write(path.with_extension("kicad_pro"), &pro).unwrap();
        let run = std::process::Command::new("kicad-cli")
            .args(["pcb", "upgrade", "--force"])
            .arg(&path)
            .output()
            .expect("kicad-cli runs");
        assert!(run.status.success(), "{run:?}");
        let upgraded = std::fs::read_to_string(&path).unwrap();
        let project = std::fs::read_to_string(path.with_extension("kicad_pro")).unwrap();
        let (old, new) = (
            read(&pcb, Some(&pro)).unwrap(),
            read(&upgraded, Some(&project)).unwrap(),
        );
        assert!(
            upgraded.contains("(generator_version \"10.0\")"),
            "{board} was not upgraded"
        );
        // Net IDs follow the order nets are met in, which the formats don't share: compare
        // names.
        let name =
            |r: &KicadBoard, n: Option<crate::id::NetId>| n.map(|n| r.nets[n.index()].clone());
        // KiCad sorts some items as it saves them: compare them in order of UUID, keeping
        // the order of the parts of one item.
        fn sorted<T>(sources: &[Source], items: impl Iterator<Item = T>) -> Vec<(Source, T)> {
            let mut v: Vec<(Source, T)> = sources.iter().cloned().zip(items).collect();
            v.sort_by(|a, b| a.0.uuid.cmp(&b.0.uuid));
            v
        }
        let tracks = |r: &KicadBoard| {
            let items = r
                .board
                .tracks
                .iter()
                .map(|t| (t.start, t.mid, t.end, t.width, t.layer, name(r, t.net)));
            sorted(&r.tracks, items)
        };
        let vias = |r: &KicadBoard| {
            let items = r
                .board
                .vias
                .iter()
                .map(|v| (v.at, v.drill, v.diameter, v.from, v.to, name(r, v.net)));
            sorted(&r.vias, items)
        };
        // KiCad saves pads' positions relative to their footprint, which it works out again
        // as it saves, so pads of footprints turned by other than right angles can move by
        // a nanometre: compare their boxes, to 2 nm.
        let pads = |r: &KicadBoard| {
            let corners = |s: &crate::geom::Shape| {
                let b = s.bbox();
                [b.min().x, b.min().y, b.max().x, b.max().y].map(|v| (v + 1).div_euclid(4))
            };
            let items = r.board.pads.iter().map(|p| {
                (
                    corners(&p.shape),
                    p.layers,
                    p.hole.as_ref().map(corners),
                    name(r, p.net),
                    p.clearance,
                )
            });
            sorted(&r.pads, items)
        };
        // KiCad 10 doesn't save teardrops' UUIDs, and may reorder zones: compare them as
        // they print, sorted.
        let zones = |r: &KicadBoard| -> Vec<String> {
            let mut v: Vec<String> = r
                .zones
                .iter()
                .zip(&r.board.zones)
                .map(|(s, z)| {
                    format!(
                        "{} {:?} {:?} {:?}",
                        s.description,
                        z.layer,
                        name(r, z.net),
                        z.shape
                    )
                })
                .collect();
            v.sort();
            v
        };
        same(board, "tracks", &tracks(&old), &tracks(&new));
        same(board, "vias", &vias(&old), &vias(&new));
        same(board, "pads", &pads(&old), &pads(&new));
        same(board, "zones", &zones(&old), &zones(&new));
        let rules = |r: &KicadBoard| {
            let s = r.rules.spec();
            let classes: Vec<_> = s
                .classes
                .iter()
                .map(|c| {
                    (
                        c.name.clone(),
                        c.nets
                            .iter()
                            .map(|&n| name(r, Some(n)))
                            .collect::<BTreeSet<_>>(),
                        c.overrides,
                    )
                })
                .collect();
            (
                s.layers.clone(),
                s.clearance,
                s.min_clearance,
                s.hole_clearance,
                s.hole_to_hole,
                classes,
            )
        };
        assert_eq!(rules(&old), rules(&new), "{board}: rules");
        eprintln!("{board}: the same in KiCad 10's format");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
