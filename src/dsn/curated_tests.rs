//! The curated boards, read from KiCad's DSN export and from KiCad's own files.
//!
//! Each board in `testdata/kicad` has a `.dsn` beside it, exported by KiCad 10.0.3. The
//! tests here read both, match the items of one to those of the other, and compare what
//! [`drc::check`] finds on each, finding by finding. The differences must each be one
//! that `Readings::explain` accounts for: something that DSN doesn't carry, or rounding.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::*;
use crate::geom::drc::{self, Item, KICAD_CLEARANCE_SLACK, Violation, check};
use crate::geom::{LayerId, LayerSet, RuleSpec, Rules, Shape};
use crate::id::NetId;
use crate::kicad::curated_tests::{ARC_TOLERANCE, BOARDS, load, raise_clearances, testdata};
use crate::kicad::{self, KicadBoard, Source};

/// How far apart, in nanometres, an item's two readings may be and still be matched. KiCad
/// writes six significant digits in micrometres, which rounds by up to 0.5 µm beyond
/// 100 mm, and by more for the polygons it makes of rounded pads.
const MATCH: i64 = 3_000;

fn load_dsn(dir: &str, board: &str) -> String {
    std::fs::read_to_string(testdata().join(dir).join(board).with_extension("dsn")).unwrap()
}

fn center(r: crate::geom::Rect) -> Point {
    Point::new((r.min().x + r.max().x) / 2, (r.min().y + r.max().y) / 2)
}

fn near(a: Point, b: Point, d: i64) -> bool {
    (a.x - b.x).abs() <= d && (a.y - b.y).abs() <= d
}

/// For each item of the DSN board, the KiCad item it is.
struct Matching {
    tracks: Vec<Option<usize>>,
    vias: Vec<Option<usize>>,
    pads: Vec<Option<usize>>,
}

/// A pad's component and number, from a KiCad pad's description, `pad 3 of U1`.
fn kicad_pad_name(description: &str) -> Option<(&str, &str)> {
    if let Some(reference) = description.strip_prefix("pad of ") {
        return Some((reference, ""));
    }
    let rest = description.strip_prefix("pad ")?;
    let (number, reference) = rest.rsplit_once(" of ")?;
    Some((reference, number))
}

fn match_items(k: &KicadBoard, d: &DsnBoard) -> Matching {
    // The first KiCad item that fits and is not taken yet, or else the first that fits:
    // boards can have the same track twice.
    fn pick(n: usize, taken: &mut BTreeSet<usize>, fits: impl Fn(usize) -> bool) -> Option<usize> {
        let found = (0..n)
            .filter(|&i| fits(i))
            .find(|i| !taken.contains(i))
            .or_else(|| (0..n).find(|&i| fits(i)))?;
        taken.insert(found);
        Some(found)
    }
    let mut taken = BTreeSet::new();
    let tracks = d
        .board
        .tracks
        .iter()
        .map(|t| {
            pick(k.board.tracks.len(), &mut taken, |i| {
                let kt = &k.board.tracks[i];
                kt.layer == t.layer
                    && ((near(kt.start, t.start, MATCH) && near(kt.end, t.end, MATCH))
                        || (near(kt.start, t.end, MATCH) && near(kt.end, t.start, MATCH)))
            })
        })
        .collect();
    let mut taken = BTreeSet::new();
    let vias = d
        .board
        .vias
        .iter()
        .map(|v| {
            pick(k.board.vias.len(), &mut taken, |i| {
                near(k.board.vias[i].at, v.at, MATCH)
            })
        })
        .collect();
    // KiCad's pads by name, each as its first part, the box of all its parts, and its
    // layers. A custom pad is several parts in a row, with one source.
    type Part = (usize, crate::geom::Rect, LayerSet);
    let mut by_name: BTreeMap<(&str, &str), Vec<Part>> = BTreeMap::new();
    let mut i = 0;
    while i < k.pads.len() {
        let mut j = i + 1;
        let mut bbox = k.board.pads[i].shape.bbox();
        while j < k.pads.len() && k.pads[j] == k.pads[i] {
            bbox = bbox.union(&k.board.pads[j].shape.bbox());
            j += 1;
        }
        if let Some(name) = kicad_pad_name(&k.pads[i].description) {
            by_name
                .entry(name)
                .or_default()
                .push((i, bbox, k.board.pads[i].layers));
        }
        i = j;
    }
    let pads = d
        .board
        .pads
        .iter()
        .zip(&d.pads)
        .map(|(p, s)| {
            let reference = d.components[s.component].reference.as_str();
            // KiCad's exporter renames footprints of one reference, and pins of one number.
            let reference = reference.split_once("_").map_or(reference, |(r, _)| r);
            let pin = s.pin.split_once('@').map_or(s.pin.as_str(), |(p, _)| p);
            let c = center(p.shape.bbox());
            by_name
                .get(&(reference, pin))?
                .iter()
                .filter(|&&(_, _, layers)| layers == p.layers)
                .map(|&(i, bbox, _)| {
                    let kc = center(bbox);
                    ((kc.x - c.x).abs().max((kc.y - c.y).abs()), i)
                })
                .filter(|&(dist, _)| dist <= MATCH)
                .min()
                .map(|(_, i)| i)
        })
        .collect();
    Matching { tracks, vias, pads }
}

/// The DSN reading, with each class's clearance and the default raised by `extra`, as
/// `raise_clearances` does to a KiCad project.
fn raised(d: &DsnBoard, extra: i64) -> Rules {
    let mut spec: RuleSpec = d.rules.spec().clone();
    spec.clearance += extra;
    for class in &mut spec.classes {
        if let Some(c) = &mut class.overrides.clearance {
            *c += extra;
        }
    }
    Rules::new(spec).unwrap()
}

/// The key of a KiCad item: its UUID, or its description and position if it has none.
fn kicad_key(k: &KicadBoard, item: Item) -> String {
    match k.source(item) {
        Some(Source { uuid: Some(u), .. }) => u.clone(),
        Some(s) => format!("{}#{item:?}", s.description),
        None => format!("{item:?}"),
    }
}

/// An item's shape, net and own clearance, for measuring gaps.
fn copper(board: &drc::Board, item: Item) -> (Shape, Option<NetId>, Option<i64>) {
    match item {
        Item::Track(i) => {
            let t = &board.tracks[i];
            let shape = match t.mid {
                None => Shape::track(t.start, t.end, t.width),
                Some(mid) => Shape::arc_track(t.start, mid, t.end, t.width, ARC_TOLERANCE),
            };
            (shape.unwrap(), t.net, None)
        }
        Item::Via(i) => {
            let v = &board.vias[i];
            (Shape::circle(v.at, v.diameter).unwrap(), v.net, None)
        }
        Item::Pad(i) => {
            let p = &board.pads[i];
            (p.shape.clone(), p.net, p.clearance)
        }
        Item::Zone(i) => {
            let z = &board.zones[i];
            (z.shape.clone(), z.net, z.clearance)
        }
        Item::Keepout(i) => (board.keepouts[i].shape.clone(), None, None),
    }
}

/// A finding: its kind, and the KiCad keys of its two items, sorted.
type Key = (&'static str, String, String);

/// A finding on one reading: its items on that reading, in the order of their keys, its
/// layer and the clearance it needed.
struct Found {
    items: (Item, Item),
    layer: LayerId,
}

/// The kind of a violation, its items, and its layer, for those compared. A short and a
/// clearance violation are one kind: which of them a pair is can turn on rounding.
fn kind(v: &Violation) -> Option<(&'static str, Item, Item, LayerId)> {
    Some(match *v {
        Violation::Short { a, b, layer } | Violation::Clearance { a, b, layer, .. } => {
            ("copper", a, b, layer)
        }
        Violation::HoleClearance {
            hole, item, layer, ..
        } => ("hole clearance", hole, item, layer),
        Violation::HoleToHole { a, b, .. } => ("hole to hole", a, b, LayerId(0)),
        Violation::Keepout {
            keepout,
            item,
            layer,
        } => ("keepout", Item::Keepout(keepout), item, layer),
        Violation::TrackWidth { track, .. } => {
            ("width", Item::Track(track), Item::Track(track), LayerId(0))
        }
        _ => return None,
    })
}

fn findings(found: &[Violation], key: &dyn Fn(Item) -> String) -> BTreeMap<Key, Found> {
    let mut out = BTreeMap::new();
    for v in found {
        let Some((kind, a, b, layer)) = kind(v) else {
            continue;
        };
        let (ka, kb) = (key(a), key(b));
        // The parts of a custom pad are one pad.
        if ka == kb && kind == "copper" {
            continue;
        }
        let (ka, kb, items) = if ka <= kb {
            (ka, kb, (a, b))
        } else {
            (kb, ka, (b, a))
        };
        out.entry((kind, ka, kb)).or_insert(Found { items, layer });
    }
    out
}

/// How far the gaps between a pair of items on the two readings may differ for rounding to
/// explain a difference. KiCad writes every coordinate to six significant digits in
/// micrometres, which moves a point by up to 0.5 µm on each axis beyond 100 mm, and so a
/// gap by up to 1.5 µm.
const ROUNDING: i64 = 1_500;

/// The same where a rounded pad is involved, which KiCad writes as a polygon within 5 µm
/// of its true outline.
const ROUNDING_POLYGONS: i64 = 6_500;

/// What one board and scenario gives: the findings on each reading, how many are on both,
/// and the differences, by reason.
#[derive(Default)]
struct Comparison {
    kicad: usize,
    dsn: usize,
    common: usize,
    kicad_only: BTreeMap<&'static str, usize>,
    dsn_only: BTreeMap<&'static str, usize>,
    unexplained: Vec<String>,
}

/// The two readings of a board, and how their items match.
struct Readings {
    k: KicadBoard,
    d: DsnBoard,
    m: Matching,
}

impl Readings {
    fn to_kicad(&self, item: Item) -> Option<Item> {
        match item {
            Item::Track(i) => self.m.tracks[i].map(Item::Track),
            Item::Via(i) => self.m.vias[i].map(Item::Via),
            Item::Pad(i) => self.m.pads[i].map(Item::Pad),
            Item::Zone(_) | Item::Keepout(_) => None,
        }
    }

    /// The DSN items of a KiCad item: every part of a custom pad finds its DSN pad.
    fn to_dsn(&self, item: Item) -> Vec<Item> {
        let key = kicad_key(&self.k, item);
        let mut out = Vec::new();
        let n = self.d.board.tracks.len() + self.d.board.vias.len() + self.d.board.pads.len();
        for j in 0..n {
            let (nt, nv) = (self.d.board.tracks.len(), self.d.board.vias.len());
            let dsn = if j < nt {
                Item::Track(j)
            } else if j < nt + nv {
                Item::Via(j - nt)
            } else {
                Item::Pad(j - nt - nv)
            };
            if self
                .to_kicad(dsn)
                .is_some_and(|ki| kicad_key(&self.k, ki) == key)
            {
                out.push(dsn);
            }
        }
        out
    }

    /// Why a finding is on one reading and not the other, if it can say.
    fn explain(&self, key: &Key, found: &Found, kicad_side: bool) -> Option<&'static str> {
        let (k, d) = (&self.k, &self.d);
        match key.0 {
            "hole clearance" | "hole to hole" => return Some("holes"),
            "keepout" => return Some("holes as keepouts"),
            "width" => return Some("routing widths"),
            _ => {}
        }
        let (a, b) = found.items;
        let (kicad_pair, dsn_a, dsn_b) = if kicad_side {
            ((a, b), self.to_dsn(a), self.to_dsn(b))
        } else {
            match (self.to_kicad(a), self.to_kicad(b)) {
                (Some(ka), Some(kb)) => ((ka, kb), vec![a], vec![b]),
                _ => return Some("unmatched"),
            }
        };
        let zone = |i: Item| matches!(i, Item::Zone(_));
        if zone(kicad_pair.0) || zone(kicad_pair.1) {
            let graphic = |i: Item| {
                zone(i)
                    && !k
                        .source(i)
                        .is_some_and(|s| s.description.starts_with("zone"))
            };
            return Some(if graphic(kicad_pair.0) || graphic(kicad_pair.1) {
                "copper graphics"
            } else {
                "zone fills"
            });
        }
        if dsn_a.is_empty() || dsn_b.is_empty() {
            return Some("unmatched");
        }
        let (ka, na, ca) = copper(&k.board, kicad_pair.0);
        let (kb, nb, cb) = copper(&k.board, kicad_pair.1);
        let kicad_required = drc::clearance(&k.rules, (na, ca), (nb, cb), found.layer);
        let dsn_shapes = |items: &[Item]| -> Vec<(Shape, Option<NetId>)> {
            items
                .iter()
                .map(|&i| {
                    let (s, n, _) = copper(&d.board, i);
                    (s, n)
                })
                .collect()
        };
        let (da, db) = (dsn_shapes(&dsn_a), dsn_shapes(&dsn_b));
        let dsn_required = drc::clearance(&d.rules, (da[0].1, None), (db[0].1, None), found.layer);
        if kicad_required != dsn_required {
            let pair =
                k.rules.spec().pairs.iter().any(|&(p, q, _)| {
                    (Some(p), Some(q)) == (na, nb) || (Some(q), Some(p)) == (na, nb)
                });
            if ca.is_some() || cb.is_some() {
                return Some("pads' own clearances");
            }
            if pair {
                return Some("pairs' gaps");
            }
            if kicad_required == k.rules.spec().min_clearance {
                return Some("minimum clearance");
            }
            return None;
        }
        // The gaps on the two readings, either side of what decides, which is the
        // clearance less KiCad's slack.
        let kicad_gap = ka.distance(&kb);
        let dsn_gap = da
            .iter()
            .flat_map(|(x, _)| db.iter().map(move |(y, _)| x.distance(y)))
            .min_by_key(|g| g.floor())?;
        let lo = kicad_gap.floor().min(dsn_gap.floor());
        let hi = kicad_gap.ceil().max(dsn_gap.ceil());
        let threshold = (kicad_required - KICAD_CLEARANCE_SLACK).max(0);
        if !(lo <= threshold && threshold <= hi) {
            return None;
        }
        // KiCad exports a custom pad as its convex hull, which can only be larger.
        let custom = |items: &[Item]| {
            items
                .iter()
                .any(|&i| matches!(i, Item::Pad(p) if d.pads[p].padstack.starts_with("Cust")))
        };
        if (custom(&dsn_a) || custom(&dsn_b)) && dsn_gap.floor() <= kicad_gap.ceil() {
            return Some("custom pads as hulls");
        }
        // An arc's chord is at most its sagitta from it, and the gap between two arcs moves
        // by both.
        let sagitta = |i: Item| match i {
            Item::Track(t) => k.board.tracks[t].mid.map_or(0, |mid| {
                let t = &k.board.tracks[t];
                sagitta(t.start, mid, t.end)
            }),
            _ => 0,
        };
        let polygon = |items: &[Item]| {
            items
                .iter()
                .any(|&i| matches!(i, Item::Pad(p) if d.pads[p].padstack.starts_with("RoundRect")))
        };
        let rounding = if polygon(&dsn_a) || polygon(&dsn_b) {
            ROUNDING_POLYGONS
        } else {
            ROUNDING
        };
        let arcs = sagitta(kicad_pair.0) + sagitta(kicad_pair.1);
        if arcs > 0 && hi - lo <= arcs + rounding {
            return Some("arcs as chords");
        }
        (hi - lo <= rounding).then_some("rounding")
    }
}

/// The greatest distance between the arc through three points and its chord, rounded up.
fn sagitta(s: Point, m: Point, e: Point) -> i64 {
    let (ax, ay) = ((s.x - m.x) as f64, (s.y - m.y) as f64);
    let (bx, by) = ((e.x - m.x) as f64, (e.y - m.y) as f64);
    let (cx, cy) = ((e.x - s.x) as f64, (e.y - s.y) as f64);
    let cross = (ax * by - ay * bx).abs();
    if cross == 0.0 {
        return 0;
    }
    let (a, b, c) = (ax.hypot(ay), bx.hypot(by), cx.hypot(cy));
    let r = a * b * c / (2.0 * cross);
    // The middle is on the far side of the chord from the centre if the arc is more than
    // half a circle.
    let half = c / 2.0;
    let h = (r * r - half * half).max(0.0).sqrt();
    let major = ax * bx + ay * by > 0.0;
    (if major { r + h } else { r - h }).ceil() as i64
}

fn compare(dir: &str, name: &str, extra: i64) -> Comparison {
    let (pcb, pro) = load(dir, name);
    let mut k = kicad::read(&pcb, Some(&raise_clearances(&pro, extra))).unwrap();
    // Both decide as KiCad does, as the comparisons with KiCad itself do.
    k.board.arc_tolerance = ARC_TOLERANCE;
    k.board.clearance_slack = KICAD_CLEARANCE_SLACK;
    let mut d = read(&load_dsn(dir, name)).unwrap();
    d.rules = raised(&d, extra);
    d.board.arc_tolerance = ARC_TOLERANCE;
    d.board.clearance_slack = KICAD_CLEARANCE_SLACK;
    let m = match_items(&k, &d);
    let r = Readings { k, d, m };
    let dsn_key = |item: Item| match r.to_kicad(item) {
        Some(ki) => kicad_key(&r.k, ki),
        None => format!("dsn {}", r.d.describe(item).unwrap_or_default()),
    };
    let on_kicad = findings(&check(&r.k.board, &r.k.rules).unwrap(), &|i| {
        kicad_key(&r.k, i)
    });
    let on_dsn = findings(&check(&r.d.board, &r.d.rules).unwrap(), &dsn_key);
    let mut c = Comparison {
        kicad: on_kicad.len(),
        dsn: on_dsn.len(),
        ..Comparison::default()
    };
    for (key, found) in &on_kicad {
        if on_dsn.contains_key(key) {
            c.common += 1;
            continue;
        }
        match r.explain(key, found, true) {
            Some(why) => *c.kicad_only.entry(why).or_default() += 1,
            None => c
                .unexplained
                .push(format!("only on the KiCad reading: {key:?}")),
        }
    }
    for (key, found) in &on_dsn {
        if on_kicad.contains_key(key) {
            continue;
        }
        match r.explain(key, found, false) {
            Some(why) => *c.dsn_only.entry(why).or_default() += 1,
            None => c.unexplained.push(format!(
                "only on the DSN reading: {key:?}: {} / {}",
                r.d.describe(found.items.0).unwrap_or_default(),
                r.d.describe(found.items.1).unwrap_or_default()
            )),
        }
    }
    c
}

fn reasons(r: &BTreeMap<&'static str, usize>) -> String {
    if r.is_empty() {
        return "none".into();
    }
    r.iter()
        .map(|(why, n)| format!("{n} {why}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every item of a DSN board is an item of its KiCad board, and every KiCad track and
/// via, and every pad with copper, is in the DSN.
#[test]
fn curated_dsn_items_match_kicad_items() {
    for &(dir, name, _) in BOARDS {
        let (pcb, pro) = load(dir, name);
        let k = kicad::read(&pcb, Some(&pro)).unwrap();
        let d = read(&load_dsn(dir, name)).unwrap();
        let m = match_items(&k, &d);
        for (what, list) in [("track", &m.tracks), ("via", &m.vias), ("pad", &m.pads)] {
            let missing = list.iter().filter(|x| x.is_none()).count();
            assert_eq!(
                missing, 0,
                "{name}: {missing} DSN {what}s match no KiCad one"
            );
        }
        assert_eq!(d.board.vias.len(), k.board.vias.len(), "{name}");
        let tracks: BTreeSet<usize> = m.tracks.iter().flatten().copied().collect();
        assert_eq!(tracks.len(), k.board.tracks.len(), "{name}");
        let exported: BTreeSet<String> = m
            .pads
            .iter()
            .flatten()
            .map(|&p| kicad_key(&k, Item::Pad(p)))
            .collect();
        let missing: Vec<&str> = (0..k.board.pads.len())
            .filter(|&p| k.board.pads[p].layers != LayerSet::EMPTY)
            .filter(|&p| !exported.contains(&kicad_key(&k, Item::Pad(p))))
            .map(|p| k.pads[p].description.as_str())
            .collect();
        eprintln!("{name}: KiCad pads not in the DSN: {missing:?}");
    }
}

/// The DSN reading of each curated board finds what the KiCad reading does, but for
/// differences that are each explained.
#[test]
fn curated_dsn_boards_check_as_their_kicad_boards() {
    let mut table = String::new();
    let mut unexplained = Vec::new();
    for &(dir, name, scenarios) in BOARDS {
        for &extra in scenarios {
            let c = compare(dir, name, extra);
            table.push_str(&format!(
                "| `{name}` | {} | {} | {} | {} | {} | {} |\n",
                crate::kicad::curated_tests::scenario_name(extra),
                c.kicad,
                c.dsn,
                c.common,
                reasons(&c.kicad_only),
                reasons(&c.dsn_only),
            ));
            unexplained.extend(
                c.unexplained
                    .into_iter()
                    .map(|u| format!("{name} {extra}: {u}")),
            );
        }
    }
    eprintln!("{table}");
    assert!(unexplained.is_empty(), "{}", unexplained.join("\n"));
}

/// Imports a session into a board with KiCad's own `ImportSpecctraSES`, which replaces the
/// board's tracks and vias with the session's, and saves the result.
const IMPORT_SES: &str = r#"
import sys, pcbnew
board = pcbnew.LoadBoard(sys.argv[1])
if not pcbnew.ImportSpecctraSES(board, sys.argv[2]):
    sys.exit("ImportSpecctraSES failed")
board.Save(sys.argv[3])
"#;

/// Exports a board to DSN with KiCad's own `ExportSpecctraDSN`, after renaming the second
/// and later footprints of one reference, which the exporter refuses, and cutting the
/// path in the first line to the file's name, as `testdata/kicad`'s DSN files were made.
const EXPORT_DSN: &str = r#"
import os, sys, pcbnew
board = pcbnew.LoadBoard(sys.argv[1])
seen = {}
for f in board.GetFootprints():
    ref = f.GetReference()
    seen[ref] = seen.get(ref, 0) + 1
    if seen[ref] > 1:
        f.SetReference("%s_%d" % (ref, seen[ref]))
if not pcbnew.ExportSpecctraDSN(board, sys.argv[2]):
    sys.exit("ExportSpecctraDSN failed")
with open(sys.argv[2]) as f:
    lines = f.read().split("\n")
lines[0] = '(pcb "%s"' % sys.argv[3]
with open(sys.argv[2], "w") as f:
    f.write("\n".join(lines))
"#;

/// Runs a Python script with KiCad's `pcbnew` module, isolated from the environment, from
/// a file in `scratch`.
fn run_python(scratch: &Path, script: &str, args: &[&Path], extra: &[&str]) {
    let path = scratch.join("guide-script.py");
    std::fs::write(&path, script).unwrap();
    let run = std::process::Command::new("python3")
        .arg("-I")
        .arg(&path)
        .args(args)
        .args(extra)
        .output()
        .expect("python3 runs");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

/// How many violations of each type a KiCad DRC report has, and how many unconnected items.
fn kicad_counts(report: &str) -> BTreeMap<String, usize> {
    let root = kicad::json::parse(report).unwrap();
    let mut counts = BTreeMap::new();
    for section in ["violations", "unconnected_items"] {
        for v in root.get(section).map_or(&[][..], |j| j.as_array()) {
            let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("?");
            *counts.entry(format!("{section}: {ty}")).or_insert(0) += 1;
        }
    }
    counts
}

/// How far KiCad's smallest gap between two items may move when an arc becomes a
/// polyline, in nanometres: the polyline's vertices are within 100 nm of the arc, plus one
/// for rounding them, and KiCad reports gaps to 100 nm.
const GAP_TOLERANCE: i64 = 300;

/// The pairs of items of each kind of violation in a KiCad DRC report, with the smallest
/// gap it reports between them, if it reports one. Items are their descriptions, with an
/// arc taken as a track and lengths dropped, so that an arc and the tracks it became are
/// one item, and so are tracks of one net on one layer.
///
/// Solder mask bridges are left out: KiCad reports one pair of the items that a mask
/// opening bridges, whichever its threads reach first, so the pairs change from run to run
/// on the same board, though not how many there are.
fn kicad_pairs(report: &str) -> BTreeMap<(String, Vec<String>), Option<i64>> {
    let root = kicad::json::parse(report).unwrap();
    let mut out: BTreeMap<(String, Vec<String>), Option<i64>> = BTreeMap::new();
    for v in root.get("violations").map_or(&[][..], |j| j.as_array()) {
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("?");
        if ty == "solder_mask_bridge" {
            continue;
        }
        let mut items: Vec<String> = v
            .get("items")
            .map_or(&[][..], |j| j.as_array())
            .iter()
            .filter_map(|i| i.get("description").and_then(|d| d.as_str()))
            .map(|d| {
                let d = d.replace("Track (arc)", "Track");
                d.split(", length ").next().unwrap_or("").to_string()
            })
            .collect();
        items.sort();
        let description = v.get("description").and_then(|d| d.as_str()).unwrap_or("");
        let gap = description
            .split("actual ")
            .nth(1)
            .and_then(|rest| rest.split(" mm").next())
            .and_then(|mm| crate::units::scaled(mm, 1, 6, i64::MAX));
        let entry = out.entry((ty.to_string(), items)).or_insert(gap);
        if let (Some(old), Some(new)) = (*entry, gap) {
            *entry = Some(old.min(new));
        }
    }
    out
}

/// A directory of its own for one test's files for a board, as tests run in parallel.
fn kicad_scratch(test: &str, name: &str) -> PathBuf {
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("guide-dsn-{pid}-{test}-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// KiCad's exporter still writes, from each curated board, the DSN file beside it. Runs
/// only with `GUIDE_KICAD=1`, as it needs KiCad 10.0.3's `pcbnew` module for Python.
/// With `GUIDE_KICAD_UPDATE=1` as well, it writes them again.
#[test]
fn curated_dsn_files_are_kicads_export() {
    if std::env::var_os("GUIDE_KICAD").is_none() {
        eprintln!("skipped: set GUIDE_KICAD=1 to export with KiCad");
        return;
    }
    let update = std::env::var_os("GUIDE_KICAD_UPDATE").is_some();
    let mut changed = Vec::new();
    for &(dir, name, _) in BOARDS {
        let scratch = kicad_scratch("export", name);
        let pcb = testdata().join(dir).join(name).with_extension("kicad_pcb");
        let out = scratch.join(format!("{name}.dsn"));
        run_python(
            &scratch,
            EXPORT_DSN,
            &[&pcb, &out],
            &[&format!("{name}.dsn")],
        );
        let fresh = std::fs::read_to_string(&out).unwrap();
        let _ = std::fs::remove_dir_all(&scratch);
        let path = testdata().join(dir).join(name).with_extension("dsn");
        if update {
            std::fs::write(&path, &fresh).unwrap();
        } else if std::fs::read_to_string(&path).unwrap() != fresh {
            changed.push(name);
        }
    }
    assert!(
        changed.is_empty(),
        "KiCad exports these differently now: {changed:?}"
    );
}

/// A session written from a board's own tracks and vias, imported into the board by KiCad,
/// gives back the same tracks and vias, and KiCad's DRC finds what it found before. Runs
/// only with `GUIDE_KICAD=1`, as it needs `kicad-cli` 10.0.3 and its `pcbnew` module.
///
/// Arcs are the exception: a session has no arcs that KiCad reads, so they come back as
/// polylines within [`DEFAULT_ARC_TOLERANCE`](crate::geom::DEFAULT_ARC_TOLERANCE) of them,
/// and only straight tracks are compared exactly.
#[test]
fn curated_sessions_import_into_kicad_unchanged() {
    if std::env::var_os("GUIDE_KICAD").is_none() {
        eprintln!("skipped: set GUIDE_KICAD=1 to import sessions into KiCad");
        return;
    }
    let mut problems = Vec::new();
    for &(dir, name, _) in BOARDS {
        let (pcb, pro) = load(dir, name);
        let k = kicad::read(&pcb, Some(&pro)).unwrap();
        // The design's names for the layers, which can be the board's own, such as
        // `Front`, where `kicad::read` gives KiCad's canonical `F.Cu`.
        let design = read(&load_dsn(dir, name)).unwrap();
        let layers = &design.rules.spec().layers;
        let ses = write_session(&Session::new(
            &format!("{name}.dsn"),
            layers,
            &k.nets,
            &k.board.tracks,
            &k.board.vias,
        ))
        .unwrap();
        let scratch = kicad_scratch("session", name);
        let (board, session, imported) = (
            testdata().join(dir).join(name).with_extension("kicad_pcb"),
            scratch.join(format!("{name}.ses")),
            scratch.join(format!("{name}.kicad_pcb")),
        );
        std::fs::write(&session, &ses).unwrap();
        run_python(&scratch, IMPORT_SES, &[&board, &session, &imported], &[]);
        let after = std::fs::read_to_string(&imported).unwrap();
        let _ = std::fs::remove_dir_all(&scratch);

        // The straight tracks and the vias come back exactly, nets and all.
        let back = kicad::read(&after, Some(&pro)).unwrap();
        type Seg = (Point, Point, i64, LayerId, String);
        let straight = |b: &KicadBoard| -> Vec<Seg> {
            let mut out: Vec<Seg> = b
                .board
                .tracks
                .iter()
                .filter(|t| t.mid.is_none())
                .map(|t| {
                    let (a, z) = if t.start <= t.end {
                        (t.start, t.end)
                    } else {
                        (t.end, t.start)
                    };
                    (
                        a,
                        z,
                        t.width,
                        t.layer,
                        b.nets[t.net.unwrap().index()].clone(),
                    )
                })
                .collect();
            out.sort();
            out
        };
        let vias = |b: &KicadBoard| -> Vec<(Point, i64, i64, LayerId, LayerId, String)> {
            let mut out: Vec<_> = b
                .board
                .vias
                .iter()
                .map(|v| {
                    let net = b.nets[v.net.unwrap().index()].clone();
                    (v.at, v.diameter, v.drill, v.from, v.to, net)
                })
                .collect();
            out.sort();
            out
        };
        let arcs = k.board.tracks.iter().filter(|t| t.mid.is_some()).count();
        let mut expected = straight(&k);
        if arcs > 0 {
            // The arcs' polylines are straight tracks now: keep only those that were.
            let now = straight(&back);
            let before: BTreeSet<&Seg> = expected.iter().collect();
            let missing = expected.iter().filter(|s| !now.contains(s)).count();
            if missing > 0 {
                problems.push(format!("{name}: {missing} straight tracks changed"));
            }
            let added = now.iter().filter(|s| !before.contains(s)).count();
            eprintln!("{name}: {arcs} arcs came back as {added} straight tracks");
            expected.clear();
        }
        if !expected.is_empty() && expected != straight(&back) {
            problems.push(format!("{name}: the tracks changed"));
        }
        if vias(&k) != vias(&back) {
            problems.push(format!("{name}: the vias changed"));
        }

        // KiCad's DRC finds the same: as many of each kind on a board with no arcs, and on
        // any board, the same pairs of items with the same smallest gaps, where an arc
        // and the tracks it became are one item.
        let before = kicad::curated_tests::run_kicad(&pcb, &pro, name);
        let now = kicad::curated_tests::run_kicad(&after, &pro, name);
        let (count_before, count_now) = (kicad_counts(&before), kicad_counts(&now));
        eprintln!("{name}: KiCad found {count_before:?} before, {count_now:?} after");
        if arcs == 0 && count_before != count_now {
            problems.push(format!(
                "{name}: KiCad's DRC changed: {count_before:?} to {count_now:?}"
            ));
        }
        // KiCad stops looking for a kind of violation once it has found its limit of them,
        // and an arc that violates a clearance is many tracks that each do. KiCad reports a
        // hole too close to copper only if it hasn't reported the copper, so the hole
        // clearances can't be compared either once the clearances can't.
        let reached = |kind: &str| {
            let n = |c: &BTreeMap<String, usize>| c.get(&format!("violations: {kind}")).copied();
            let limit = kicad::curated_tests::kicad_limit(kind);
            n(&count_before).unwrap_or(0) >= limit || n(&count_now).unwrap_or(0) >= limit
        };
        let full = |kind: &str| reached(kind) || (kind == "hole_clearance" && reached("clearance"));
        let (pairs_before, pairs_now) = (kicad_pairs(&before), kicad_pairs(&now));
        let keys: BTreeSet<_> = pairs_before.keys().chain(pairs_now.keys()).collect();
        let mut unchecked = BTreeSet::new();
        for key in keys {
            if full(&key.0) {
                unchecked.insert(key.0.clone());
                continue;
            }
            let (b, n) = (pairs_before.get(key), pairs_now.get(key));
            let same = match (b, n) {
                (Some(Some(b)), Some(Some(n))) => (b - n).abs() <= GAP_TOLERANCE,
                (b, n) => b == n,
            };
            if !same {
                problems.push(format!("{name}: {key:?} went from {b:?} to {n:?}"));
            }
        }
        if !unchecked.is_empty() {
            eprintln!("{name}: KiCad reached its limit, so these weren't compared: {unchecked:?}");
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
