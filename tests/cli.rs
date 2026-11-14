//! The `guide` command-line tool, run as a user would: on VTR's `mult_4x4`, a curated KiCad
//! board and its DSN export, a small MAPF map, and a small ISPD design, and on command lines
//! that are wrong.

#![cfg(feature = "cli")]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::island::island_device;

const GUIDE: &str = env!("CARGO_BIN_EXE_guide");

fn testdata(path: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(path)
        .to_string_lossy()
        .into_owned()
}

/// A fresh directory for one test's files.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("guide-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn path(dir: &Path, name: &str) -> String {
    dir.join(name).to_string_lossy().into_owned()
}

fn guide(args: &[&str]) -> Output {
    let out = Command::new(GUIDE).args(args).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
    out
}

/// Runs `guide`, and checks its exit status.
fn run(args: &[&str], status: i32) -> Output {
    let out = guide(args);
    assert_eq!(
        out.status.code(),
        Some(status),
        "{args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

/// A report field's text in JSON, as `"key": value`, which these tests look for.
fn has(json: &str, key: &str, value: &str) -> bool {
    json.contains(&format!("\"{key}\": {value}"))
}

#[test]
fn help_and_version() {
    let out = run(&["--help"], 0);
    for command in [
        "fpga route",
        "fpga place",
        "fpga flow",
        "pcb route",
        "pcb place",
        "pcb flow",
        "pcb drc",
        "dsn route",
        "mapf solve",
        "asic route",
        "asic score",
    ] {
        assert!(stdout(&out).contains(command), "{command}");
        let words: Vec<&str> = command.split(' ').collect();
        let help = run(&[words[0], words[1], "--help"], 0);
        assert!(stdout(&help).contains(&format!("Usage: guide {command}")));
    }
    assert!(stdout(&run(&["pcb", "--help"], 0)).contains("drc"));
    assert!(stdout(&run(&["help", "dsn", "route"], 0)).contains("-de"));
    assert!(stdout(&run(&["--version"], 0)).starts_with("guide "));
}

#[test]
fn wrong_command_lines_fail_clearly() {
    for (args, expected) in [
        (&[][..], "missing a command"),
        (&["routing"], "unknown domain"),
        (&["pcb"], "missing an action"),
        (&["pcb", "rout"], "unknown action"),
        (&["pcb", "route"], "missing BOARD.kicad_pcb"),
        (&["pcb", "route", "a", "b"], "unexpected argument"),
        (
            &["pcb", "route", "a", "--vias-cost", "1"],
            "did you mean --via-cost?",
        ),
        (&["pcb", "route", "a", "--via-cost"], "needs a value"),
        (
            &["pcb", "route", "a", "--via-cost", "-1"],
            "--via-cost needs",
        ),
        (&["pcb", "route", "a", "--strip=1"], "takes no value"),
        (
            &["pcb", "route", "a", "--project", "p", "--no-project"],
            "contradict",
        ),
        (
            &["fpga", "route", "--net", "n", "--place", "p"],
            "--rr-graph is required",
        ),
        (&["mapf", "solve", "m", "s", "--threads", "0"], "at least 1"),
        (
            &["mapf", "solve", "m", "s", "--time-limit", "0"],
            "seconds above 0",
        ),
        (&["asic", "route", "x.gr", "-j", "many"], "whole number"),
        (
            &["pcb", "route", "/nonexistent/board.kicad_pcb"],
            "/nonexistent/board.kicad_pcb",
        ),
    ] {
        let out = run(args, 2);
        assert!(
            stderr(&out).contains(expected),
            "{args:?}: {}",
            stderr(&out)
        );
        assert!(stderr(&out).starts_with("guide: error: "));
    }
}

/// Files that aren't what they should be are refused with the file's name, never a panic.
#[test]
fn bad_inputs_are_errors() {
    let dir = scratch("bad");
    let junk = path(&dir, "junk.txt");
    std::fs::write(&junk, "(this is not \"what anything wants\n\0\u{1}").unwrap();
    for args in [
        &["pcb", "route", &junk][..],
        &["pcb", "drc", &junk],
        &["pcb", "place", &junk],
        &["dsn", "route", &junk],
        &["mapf", "solve", &junk, &junk],
        &["asic", "route", &junk],
        &["asic", "score", &junk, &junk],
        &[
            "fpga",
            "route",
            "--rr-graph",
            &junk,
            "--net",
            &junk,
            "--place",
            &junk,
            "--no-timing",
        ],
        &[
            "fpga",
            "place",
            "--arch",
            &junk,
            "--rr-graph",
            &junk,
            "--net",
            &junk,
        ],
    ] {
        let out = run(args, 2);
        assert!(
            stderr(&out).contains("junk.txt"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
}

/// Random command lines, made of real options and junk, exit with 0, 1 or 2, and never panic.
#[test]
fn random_command_lines_never_panic() {
    let words = [
        "pcb",
        "route",
        "drc",
        "fpga",
        "flow",
        "mapf",
        "solve",
        "asic",
        "dsn",
        "--help",
        "-o",
        "--json",
        "-",
        "--seed",
        "--threads",
        "0",
        "-1",
        "1e9",
        "--time-limit",
        "nan",
        "--",
        "=",
        "--x=y",
        "-de",
        "-do",
        "/nonexistent",
        "--strip",
        "-q",
        "-v",
        "--agents",
        "",
        "--via-cost",
    ];
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..150 {
        let mut args = Vec::new();
        for _ in 0..1 + state % 6 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            args.push(words[(state % words.len() as u64) as usize]);
        }
        let out = guide(&args);
        assert!(
            matches!(out.status.code(), Some(0..=2)),
            "{args:?}: {:?}",
            out.status
        );
    }
}

/// `mult_4x4` on a generated 4 by 4 device: routed from VPR's placement, placed, and both.
#[test]
fn fpga_place_route_and_flow() {
    let dir = scratch("fpga");
    let rr = path(&dir, "rr.xml");
    std::fs::write(&rr, island_device(4, 12)).unwrap();
    let (arch, net, place) = (
        testdata("vtr/k6_N10_40nm.xml"),
        testdata("vtr/mult_4x4.net"),
        testdata("vtr/mult_4x4.place"),
    );
    let route = path(&dir, "out.route");
    let out = run(
        &[
            "fpga",
            "route",
            "--arch",
            &arch,
            "--rr-graph",
            &rr,
            "--net",
            &net,
            "--place",
            &place,
            "-o",
            &route,
            "--json",
            "-",
            "-q",
        ],
        0,
    );
    let json = stdout(&out);
    assert!(has(&json, "legal", "true"), "{json}");
    assert!(has(&json, "command", "\"fpga route\""));
    let text = std::fs::read_to_string(&route).unwrap();
    // The header names the placement by its SHA-256, which VPR checks.
    assert!(
        text.starts_with(&format!(
            "Placement_File: {place} Placement_ID: SHA256:b98f0e6368789460e510c392c3dd2c9bf4568045801bf6624f890886a41976c5"
        )),
        "{}",
        &text[..200]
    );

    // Without timing, the architecture isn't needed; with threads, results are the same.
    let a = path(&dir, "a.route");
    let b = path(&dir, "b.route");
    for (file, threads) in [(&a, "1"), (&b, "3")] {
        run(
            &[
                "fpga",
                "route",
                "--rr-graph",
                &rr,
                "--net",
                &net,
                "--place",
                &place,
                "--no-timing",
                "--batch",
                "4",
                "-j",
                threads,
                "-o",
                file,
                "-q",
            ],
            0,
        );
    }
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());

    // Too few iterations to route legally: nothing is written, and the status says so.
    let none = path(&dir, "none.route");
    let out = run(
        &[
            "fpga",
            "route",
            "--rr-graph",
            &rr,
            "--net",
            &net,
            "--place",
            &place,
            "--no-timing",
            "--max-expansions",
            "10",
            "-o",
            &none,
            "--json",
            "-",
            "-q",
        ],
        1,
    );
    assert!(has(&stdout(&out), "legal", "false"));
    assert!(!Path::new(&none).exists());

    let placed = path(&dir, "placed.place");
    let out = run(
        &[
            "fpga",
            "place",
            "--arch",
            &arch,
            "--rr-graph",
            &rr,
            "--net",
            &net,
            "-o",
            &placed,
            "--seed",
            "2",
            "--json",
            "-",
            "-q",
        ],
        0,
    );
    assert!(has(&stdout(&out), "legal", "true"));
    let text = std::fs::read_to_string(&placed).unwrap();
    // The netlist's ID is its SHA-256, as in VPR's own placement of it.
    assert!(text.contains(
        "Netlist_ID: SHA256:7c3af470d4d65989ad5784af5f36aa171cc20fb464a2e5b5ff70b5ec873d787b"
    ));
    run(
        &[
            "fpga",
            "route",
            "--arch",
            &arch,
            "--rr-graph",
            &rr,
            "--net",
            &net,
            "--place",
            &placed,
            "-q",
        ],
        0,
    );

    let (flow_place, flow_route) = (path(&dir, "flow.place"), path(&dir, "flow.route"));
    let out = run(
        &[
            "fpga",
            "flow",
            "--arch",
            &arch,
            "--rr-graph",
            &rr,
            "--net",
            &net,
            "-o",
            &flow_place,
            "--route-output",
            &flow_route,
            "--json",
            "-",
            "-q",
        ],
        0,
    );
    assert!(has(&stdout(&out), "legal", "true"));
    let route_text = std::fs::read_to_string(&flow_route).unwrap();
    assert!(route_text.starts_with(&format!(
        "Placement_File: {flow_place} Placement_ID: SHA256:"
    )));
    // The flow's placement routes on its own too.
    run(
        &[
            "fpga",
            "route",
            "--arch",
            &arch,
            "--rr-graph",
            &rr,
            "--net",
            &net,
            "--place",
            &flow_place,
            "-q",
        ],
        0,
    );
}

/// The smallest curated board, routed from KiCad's files and from its DSN export, checked,
/// placed, and placed and routed.
#[test]
fn pcb_and_dsn() {
    let dir = scratch("pcb");
    let board = testdata("kicad/gekkio/GB-BRK-LINK-A.kicad_pcb");
    let dsn = testdata("kicad/gekkio/GB-BRK-LINK-A.dsn");

    // The board as it is, routed by hand: clean.
    let out = run(&["pcb", "drc", &board, "--json", "-", "-q"], 0);
    assert!(has(&stdout(&out), "clean", "true"));

    // Stripped and routed again, with the project found beside the board.
    let routed = path(&dir, "routed.kicad_pcb");
    let out = run(
        &[
            "pcb", "route", &board, "--strip", "--check", "-o", &routed, "--json", "-",
        ],
        0,
    );
    let json = stdout(&out);
    assert!(has(&json, "complete", "true"), "{json}");
    assert!(has(&json, "violations", "[]"), "{json}");
    assert!(stderr(&out).contains("with the rules of"));
    run(&["pcb", "drc", &routed, "-q"], 0);
    // The same, written to standard output, with the summary moved to standard error.
    let out = run(&["pcb", "route", &board, "--strip", "-o", "-"], 0);
    assert!(stdout(&out).starts_with("(kicad_pcb"));
    assert!(stderr(&out).contains("complete"));

    // Kept as routed: nothing is missing, and the tracks are written back.
    let kept = path(&dir, "kept.kicad_pcb");
    run(&["pcb", "route", &board, "-o", &kept, "-q"], 0);
    run(&["pcb", "drc", &kept, "-q"], 0);

    // A work budget stops the run with what it has routed, legal and written, and says so:
    // the same board every time.
    let budgeted = [
        path(&dir, "budget-a.kicad_pcb"),
        path(&dir, "budget-b.kicad_pcb"),
    ];
    for to in &budgeted {
        let out = run(
            &[
                "pcb",
                "route",
                &board,
                "--strip",
                "--check",
                "--work-budget",
                "1000",
                "--seed",
                "3",
                "-o",
                to,
                "--json",
                "-",
                "-q",
            ],
            1,
        );
        let json = stdout(&out);
        assert!(has(&json, "stopped", "\"work_budget\""), "{json}");
        assert!(has(&json, "work", "1000"), "{json}");
        assert!(has(&json, "complete", "false"), "{json}");
        assert!(has(&json, "violations", "[]"), "{json}");
    }
    assert_eq!(
        std::fs::read(&budgeted[0]).unwrap(),
        std::fs::read(&budgeted[1]).unwrap()
    );
    // A time limit that the run doesn't reach changes nothing, and one that it does stops
    // it: with progress heard of on the way, if asked.
    let out = run(
        &[
            "pcb",
            "route",
            &board,
            "--strip",
            "--time-limit",
            "3600",
            "--json",
            "-",
            "-q",
        ],
        0,
    );
    assert!(has(&stdout(&out), "stopped", "null"));
    assert!(has(&stdout(&out), "time_limit_hit", "false"));
    let out = run(
        &[
            "pcb",
            "route",
            &board,
            "--strip",
            "--time-limit",
            "0.000001",
            "-v",
            "--json",
            "-",
        ],
        1,
    );
    assert!(has(&stdout(&out), "stopped", "\"time_limit\""));
    assert!(has(&stdout(&out), "time_limit_hit", "true"));
    assert!(stderr(&out).contains("Prepare: "), "{}", stderr(&out));

    // A DSN file has no zone fills, so the ground net, joined by its zone, is unconnected.
    let out = run(&["pcb", "drc", &dsn, "--json", "-", "-q"], 1);
    assert!(has(&stdout(&out), "clean", "false"));
    run(&["pcb", "drc", &dsn, "--ignore-unconnected", "-q"], 0);

    // As Freerouting is called.
    let ses = path(&dir, "board.ses");
    let out = run(
        &[
            "dsn", "route", "-de", &dsn, "-do", &ses, "--strip", "--check", "--json", "-",
        ],
        0,
    );
    assert!(has(&stdout(&out), "complete", "true"));
    let session = std::fs::read_to_string(&ses).unwrap();
    assert!(session.starts_with("(session \"GB-BRK-LINK-A.dsn\""));
    assert!(session.contains("(type route)"));

    let placed = path(&dir, "placed.kicad_pcb");
    let out = run(
        &["pcb", "place", &board, "-o", &placed, "--json", "-", "-q"],
        0,
    );
    assert!(has(&stdout(&out), "legal", "true"));
    run(&["pcb", "route", &placed, "-q"], 0);

    let flow = path(&dir, "flow.kicad_pcb");
    let out = run(
        &[
            "pcb", "flow", &board, "--rounds", "1", "-o", &flow, "--json", "-", "-q",
        ],
        0,
    );
    assert!(has(&stdout(&out), "complete", "true"));
    run(&["pcb", "drc", &flow, "-q"], 0);
}

/// Four agents crossing a small room with a pillar.
#[test]
fn mapf_solve() {
    let dir = scratch("mapf");
    let map = path(&dir, "room.map");
    let scen = path(&dir, "room.scen");
    std::fs::write(
        &map,
        "type octile\nheight 4\nwidth 5\nmap\n.....\n..@..\n.....\n.....\n",
    )
    .unwrap();
    let task = |s: (u32, u32), g: (u32, u32)| {
        format!("0\troom.map\t5\t4\t{}\t{}\t{}\t{}\t0\n", s.0, s.1, g.0, g.1)
    };
    let text = String::from("version 1\n")
        + &task((0, 0), (4, 3))
        + &task((4, 3), (0, 0))
        + &task((0, 3), (4, 0))
        + &task((4, 0), (0, 3));
    std::fs::write(&scen, text).unwrap();
    let plans = path(&dir, "plans.txt");
    let out = run(
        &[
            "mapf", "solve", &map, &scen, "-o", &plans, "--json", "-", "-q",
        ],
        0,
    );
    let json = stdout(&out);
    assert!(has(&json, "solved", "true"), "{json}");
    assert!(has(&json, "agents", "4"));
    let plans = std::fs::read_to_string(&plans).unwrap();
    assert!(plans.contains("starts=(0,0),(4,3),(0,3),(4,0),"), "{plans}");
    assert!(
        plans.contains("solution=\n0:(0,0),(4,3),(0,3),(4,0),\n"),
        "{plans}"
    );

    let out = run(&["mapf", "solve", &map, &scen, "--agents", "5"], 2);
    assert!(stderr(&out).contains("has 4 agents"));
    run(
        &[
            "mapf",
            "solve",
            &map,
            &scen,
            "--agents",
            "2",
            "-q",
            "--time-limit",
            "60",
        ],
        0,
    );
}

/// The ISPD design of `guide::asic`'s documentation: two nets that cross.
#[test]
fn asic_route_and_score() {
    let dir = scratch("asic");
    let design = path(&dir, "tiny.gr");
    std::fs::write(
        &design,
        "grid 3 3 2\nvertical capacity 0 2\nhorizontal capacity 2 0\nminimum width 1 1\n\
         minimum spacing 1 1\nvia spacing 1 1\n0 0 10 10\nnum net 2\n\
         A 0 2 1\n 5  5 1\n25 25 1\nB 1 2 1\n 5 25 1\n25  5 1\n",
    )
    .unwrap();
    let routes = path(&dir, "tiny.route");
    let out = run(
        &["asic", "route", &design, "-o", &routes, "--json", "-", "-q"],
        0,
    );
    let json = stdout(&out);
    assert!(has(&json, "total_overflow", "0"), "{json}");
    assert!(has(&json, "wire_length", "8"), "{json}");
    let out = run(&["asic", "score", &design, &routes, "--json", "-", "-q"], 0);
    assert!(has(&stdout(&out), "vias", "6"));
    // An empty route file leaves both nets unrouted.
    let empty = path(&dir, "empty.route");
    std::fs::write(&empty, "").unwrap();
    let out = run(&["asic", "score", &design, &empty, "--json", "-", "-q"], 1);
    assert!(has(&stdout(&out), "unrouted_nets", "2"));
}
