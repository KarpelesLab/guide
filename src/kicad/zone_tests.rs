//! Zone fills of the curated boards: `guide`'s against KiCad's.
//!
//! Each board keeps its tracks and vias, and its zones are filled again by
//! [`fill_zones`](crate::pcb::fill_zones), to compare with the fills saved in the file,
//! which the board's designer had KiCad make, zone by zone and layer by layer: the area of
//! each, and the area that only one of the two covers. Our DRC must find nothing wrong
//! with our fills. With `GUIDE_KICAD=1`, KiCad 10 refills the same zones itself, through
//! `kicad-cli pcb drc --refill-zones --save-board`, to compare with too, and checks the board
//! with our fills written into it, with its own DRC.

use std::collections::BTreeMap;

use super::json::{self, Json};
use super::sexpr::{self, Sexp};
use super::{KicadBoard, replace_routing_and_fills};
use crate::geom::drc::{self, Item, Violation};
use crate::geom::{Area, LayerId};
use crate::pcb::{Design, Fill, fill_zones};

/// The nine curated boards. `flea` has no zone.
const ALL: [(&str, &str); 9] = [
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

pub(super) fn load(dir: &str, board: &str) -> (String, String) {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/kicad")
        .join(dir)
        .join(board);
    let pcb = std::fs::read_to_string(base.with_extension("kicad_pcb")).unwrap();
    let pro = std::fs::read_to_string(base.with_extension("kicad_pro")).unwrap();
    (pcb, pro)
}

/// The UUID of each zone list of a board, in order, and whether it is a teardrop's.
fn zone_lists(pcb: &str) -> Vec<(Option<String>, bool)> {
    fn walk(node: &Sexp<'_>, out: &mut Vec<(Option<String>, bool)>) {
        for item in node.args() {
            match item.name() {
                Some("zone") => {
                    let uuid = item
                        .child("uuid")
                        .or_else(|| item.child("tstamp"))
                        .and_then(|u| u.arg(0))
                        .map(str::to_string);
                    let teardrop = item
                        .child("name")
                        .and_then(|n| n.arg(0))
                        .is_some_and(|n| n.starts_with("$teardrop"));
                    out.push((uuid, teardrop));
                }
                Some("footprint" | "module") => walk(item, out),
                _ => {}
            }
        }
    }
    let root = sexpr::parse(pcb).unwrap();
    let mut out = Vec::new();
    walk(&root, &mut out);
    out
}

/// The board as read, with its zones' fills taken out but for teardrops', which belong to
/// its tracks; filled graphics stay. And by zone of the design, the fills that were saved,
/// by layer.
fn without_fills(read: &KicadBoard, pcb: &str) -> (Design, BTreeMap<(String, LayerId), Area>) {
    let lists = zone_lists(pcb);
    let mut design = Design::from_kicad(read);
    let mut saved: BTreeMap<(String, LayerId), Vec<Area>> = BTreeMap::new();
    let mut zones = Vec::new();
    for (z, source) in design.board.zones.iter().zip(&read.zones) {
        let list = lists
            .iter()
            .position(|(uuid, _)| uuid.is_some() && *uuid == source.uuid);
        let Some(list) = list else {
            // A filled graphic.
            zones.push(z.clone());
            continue;
        };
        if lists[list].1 {
            zones.push(z.clone());
            continue;
        }
        if read.routing.zone_lists.contains(&list) {
            let area = Area::union_all(
                &z.shape
                    .clone()
                    .pieces()
                    .iter()
                    .map(|s| Area::of_shape(s, 0, 1000, crate::geom::ArcSide::Outside).unwrap())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let uuid = lists[list].0.clone().unwrap_or_default();
            saved.entry((uuid, z.layer)).or_default().push(area);
        }
    }
    design.board.zones = zones;
    let saved = saved
        .into_iter()
        .map(|(k, v)| (k, Area::union_all(&v).unwrap()))
        .collect();
    (design, saved)
}

/// Our fills, by zone's UUID and layer.
fn by_zone(fills: &[Fill], read: &KicadBoard, pcb: &str) -> BTreeMap<(String, LayerId), Area> {
    let lists = zone_lists(pcb);
    let mut out: BTreeMap<(String, LayerId), Vec<Area>> = BTreeMap::new();
    for f in fills {
        let uuid = lists[read.routing.zone_lists[f.zone]]
            .0
            .clone()
            .unwrap_or_default();
        out.entry((uuid, f.layer))
            .or_default()
            .push(Area::of_shape(&f.shape, 0, 1000, crate::geom::ArcSide::Outside).unwrap());
    }
    out.into_iter()
        .map(|(k, v)| (k, Area::union_all(&v).unwrap()))
        .collect()
}

fn mm2(area: &Area) -> f64 {
    area.area2() as f64 / 2e12
}

/// What the fills of one board come to against another's: the area of each, in square
/// millimetres, and the area that only one of the two covers.
fn compare(
    ours: &BTreeMap<(String, LayerId), Area>,
    theirs: &BTreeMap<(String, LayerId), Area>,
) -> (f64, f64, f64) {
    let empty = Area::empty();
    let keys: std::collections::BTreeSet<_> = ours.keys().chain(theirs.keys()).collect();
    let (mut a, mut b, mut x) = (0.0, 0.0, 0.0);
    for k in keys {
        let o = ours.get(k).unwrap_or(&empty);
        let t = theirs.get(k).unwrap_or(&empty);
        a += mm2(o);
        b += mm2(t);
        x += mm2(&o.xor(t).unwrap());
    }
    (a, b, x)
}

/// Violations of our DRC that involve a fill.
fn fill_violations(board: &drc::Board, design: &Design, first_fill: usize) -> Vec<Violation> {
    let ours = |i: Item| matches!(i, Item::Zone(z) if z >= first_fill);
    drc::check(board, &design.rules)
        .unwrap()
        .into_iter()
        .filter(|v| match *v {
            Violation::Short { a, b, .. } | Violation::Clearance { a, b, .. } => ours(a) || ours(b),
            Violation::HoleClearance { hole, item, .. } => ours(hole) || ours(item),
            _ => false,
        })
        .collect()
}

/// Fills a board's zones around its own tracks and vias, checks our fills with our DRC, and
/// returns the design, the board as read, our fills, and the fills saved in the file.
#[allow(clippy::type_complexity)]
fn fill_board(
    dir: &str,
    name: &str,
) -> (
    Design,
    KicadBoard,
    Vec<Fill>,
    BTreeMap<(String, LayerId), Area>,
    String,
    String,
) {
    let (pcb, pro) = load(dir, name);
    let read = super::read(&pcb, Some(&pro)).unwrap();
    let (design, saved) = without_fills(&read, &pcb);
    let fills = fill_zones(&design.board, &design).unwrap();
    let mut board = design.board.clone();
    let first = board.zones.len();
    board.zones.extend(fills.iter().map(|f| f.to_zone(&design)));
    let found = fill_violations(&board, &design, first);
    assert!(found.is_empty(), "{name}: {found:?}");
    (design, read, fills, saved, pcb, pro)
}

/// Fills every curated board's zones, which our DRC must find nothing wrong with, and
/// prints how they compare with the fills saved in the files, which older versions of KiCad
/// made, under rules of their own: to the board's edge and around holes, KiCad 7 kept the
/// zone's clearance, where KiCad 10 keeps the edge clearance and the hole clearance.
#[test]
fn curated_fills_are_clean() {
    println!("| Board | Zones | Saved, mm² | Ours, mm² | Only one, mm² |");
    for (dir, name) in ALL {
        let (design, read, fills, saved, pcb, _) = fill_board(dir, name);
        let (ours, theirs, only) = compare(&by_zone(&fills, &read, &pcb), &saved);
        println!(
            "| `{name}` | {} | {theirs:.1} | {ours:.1} | {only:.2} ({:.2}%) |",
            design.zones.zones.len(),
            100.0 * only / theirs.max(1e-9)
        );
    }
}

/// Runs `kicad-cli pcb drc` on a board, refilling its zones first if `refill` says, and
/// returns its report, and the board as saved after refilling.
fn kicad_drc(text: &str, project: &str, name: &str, refill: bool) -> (Json, Option<String>) {
    let dir = std::env::temp_dir().join(format!("guide-zones-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let board = dir.join(format!("{name}.kicad_pcb"));
    let report = dir.join(format!("{name}.json"));
    std::fs::write(&board, text).unwrap();
    std::fs::write(board.with_extension("kicad_pro"), project).unwrap();
    let mut cmd = std::process::Command::new("kicad-cli");
    cmd.args(["pcb", "drc", "--format", "json", "--severity-error"]);
    if refill {
        cmd.args(["--refill-zones", "--save-board"]);
    }
    let run = cmd.arg("-o").arg(&report).arg(&board).output().unwrap();
    let report = std::fs::read_to_string(&report)
        .unwrap_or_else(|_| panic!("kicad-cli wrote no report: {run:?}"));
    let saved = refill.then(|| std::fs::read_to_string(&board).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    (json::parse(&report).unwrap(), saved)
}

/// KiCad's violations, by type, that involve a zone.
fn zone_findings(report: &Json) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for v in report.get("violations").map_or(&[][..], Json::as_array) {
        let items = v.get("items").map_or(&[][..], Json::as_array);
        let zone = items.iter().any(|i| {
            i.get("description")
                .and_then(Json::as_str)
                .is_some_and(|d| d.contains("Zone") || d.contains("zone"))
        });
        if zone {
            let ty = v
                .get("type")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string();
            *out.entry(ty).or_insert(0) += 1;
        }
    }
    out
}

/// With `GUIDE_KICAD=1`: KiCad refills each board, which is compared with our fills, and
/// checks each with our fills written in, with its own DRC.
#[test]
fn curated_fills_against_kicad() {
    if std::env::var_os("GUIDE_KICAD").is_none() {
        eprintln!("skipped: set GUIDE_KICAD=1 to compare with KiCad's fills");
        return;
    }
    println!(
        "| Board | Zones | KiCad's, mm² | Ours, mm² | Only one, mm² | KiCad's DRC of its fills | Of ours | Unconnected: KiCad's, ours |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut failures = Vec::new();
    for (dir, name) in ALL {
        let (design, read, fills, _, pcb, pro) = fill_board(dir, name);
        if design.zones.zones.is_empty() {
            continue;
        }
        let (theirs_report, saved) = kicad_drc(&pcb, &pro, name, true);
        let saved = saved.unwrap();
        let again = super::read(&saved, Some(&pro)).unwrap();
        let (_, kicad_fills) = without_fills(&again, &saved);
        let (ours, theirs, only) = compare(&by_zone(&fills, &read, &pcb), &kicad_fills);
        let text =
            replace_routing_and_fills(&pcb, &read.board.tracks, &read.board.vias, &fills, &read);
        let (ours_report, _) = kicad_drc(&text, &pro, name, false);
        let unconnected = |r: &Json| r.get("unconnected_items").map_or(0, |u| u.as_array().len());
        let (a, b) = (zone_findings(&theirs_report), zone_findings(&ours_report));
        println!(
            "| `{name}` | {} | {theirs:.1} | {ours:.1} | {only:.2} ({:.2}%) | {a:?} | {b:?} | {}, {} |",
            design.zones.zones.len(),
            100.0 * only / theirs.max(1e-9),
            unconnected(&theirs_report),
            unconnected(&ours_report),
        );
        if b.keys().any(|k| !a.contains_key(k)) {
            failures.push(format!("{name}: {b:?} against {a:?}"));
        }
        if only > 0.02 * theirs || unconnected(&ours_report) > unconnected(&theirs_report) {
            failures.push(format!("{name}: {only:.2} of {theirs:.1} mm² differ"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
