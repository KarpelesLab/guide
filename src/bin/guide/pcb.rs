//! `guide pcb` and `guide dsn`: KiCad boards and Specctra designs.

use std::path::Path;

use guide::dsn::{self, DsnBoard, Session, WireType, write_session};
use guide::geom::drc::{self, Board, Item, Track, Via, Violation};
use guide::geom::{LayerId, Rules};
use guide::id::NetId;
use guide::kicad::{self, FlowOptions, KicadBoard};
use guide::pcb::place::{PlaceOptions, Side, outline_overlaps, place_components};
use guide::pcb::{self, Design, Options, Routed};

use crate::Outcome;
use crate::args::{self, Command, Failure, Matches, Opt};
use crate::report::{Map, Ui, Value, read_text, write_output};

const PROJECT: Opt = Opt::value(
    "project",
    "FILE",
    "The project, for its rules [default: the board's .kicad_pro, if there is one]",
);
const NO_PROJECT: Opt = Opt::flag(
    "no-project",
    "Take the rules from the board alone, even with a .kicad_pro beside it",
);
const STRIP: Opt = Opt::flag(
    "strip",
    "Take out the tracks and vias already there and route everything again; by default they \
     stay, and only what is missing is routed",
);
const CHECK: Opt = Opt::flag(
    "check",
    "Check the result for clearance after routing, and report what the new copper breaks",
);

/// The routing options, which `pcb route`, `pcb flow` and `dsn route` share.
fn route_opts() -> Vec<Opt> {
    vec![
        Opt::value(
            "via-cost",
            "MM",
            "What a via costs, in millimetres of track [default: 1]",
        ),
        Opt::value(
            "pitch",
            "MM",
            "The lattice tracks move on [default: half the smallest track width plus clearance]",
        ),
        Opt::value(
            "rip-ups",
            "N",
            "Times a net may be ripped up for others; 0 turns rip-up off [default: 4]",
        ),
        Opt::value(
            "max-expansions",
            "N",
            "Search nodes one net may expand [default: 2000000]",
        ),
        Opt::flag(
            "no-planes",
            "Route nets with zones by tracks between their pads, not to their zones",
        ),
        Opt::flag(
            "pairs-first",
            "Route differential pairs side by side before everything else, not last",
        ),
        Opt::flag("no-fine-pass", "Skip the second pass on a finer lattice"),
        Opt::flag("no-cleanup", "Skip pulling tracks tight and removing vias"),
    ]
}

fn route_options(m: &Matches) -> Result<Options, Failure> {
    let mut o = Options::default();
    if let Some(v) = m.factor("via-cost")? {
        o.via_cost = v;
    }
    if let Some(p) = m.millimetres("pitch")? {
        if p == 0 {
            return Err(Failure::Usage("--pitch must be above 0".to_owned()));
        }
        o.pitch = Some(p);
    }
    if let Some(n) = m.number("rip-ups")? {
        o.rip_ups = n;
    }
    if let Some(n) = m.number("max-expansions")? {
        o.max_expansions = n;
    }
    o.planes = !m.flag("no-planes");
    o.pairs_first = m.flag("pairs-first");
    o.fine_pass = !m.flag("no-fine-pass");
    o.cleanup = !m.flag("no-cleanup");
    Ok(o)
}

fn with(mut options: Vec<Opt>, more: Vec<Opt>) -> Vec<Opt> {
    options.extend(more);
    options
}

fn common() -> Vec<Opt> {
    vec![args::JSON, args::QUIET, args::VERBOSE]
}

/// A KiCad board's text, its project's, and the board as read.
struct Loaded {
    path: String,
    pcb: String,
    project: Option<String>,
    board: KicadBoard,
}

fn load_kicad(m: &Matches, ui: &Ui) -> Result<Loaded, Failure> {
    let path = m.operands[0].clone();
    let project_path = match (m.get("project"), m.flag("no-project")) {
        (Some(_), true) => {
            return Err(Failure::Usage(
                "--project and --no-project contradict each other".to_owned(),
            ));
        }
        (Some(p), false) => Some(p.to_owned()),
        (None, true) => None,
        (None, false) => {
            let beside = Path::new(&path).with_extension("kicad_pro");
            beside
                .is_file()
                .then(|| beside.to_string_lossy().into_owned())
        }
    };
    let pcb = read_text(&path)?;
    let project = project_path.as_deref().map(read_text).transpose()?;
    match &project_path {
        Some(p) => ui.progress(format!("reading {path}, with the rules of {p}")),
        None => ui.progress(format!("reading {path}, with no project")),
    }
    let board = kicad::read(&pcb, project.as_deref()).map_err(|e| Failure::file(&path, e))?;
    ui.warnings(&path, &board.warnings);
    Ok(Loaded {
        path,
        pcb,
        project,
        board,
    })
}

fn layer_name(rules: &Rules, layer: LayerId) -> String {
    rules
        .spec()
        .layers
        .get(usize::from(layer.0))
        .cloned()
        .unwrap_or_else(|| format!("layer {}", layer.0))
}

fn net_names(names: &[String], nets: &[NetId]) -> Vec<String> {
    nets.iter()
        .map(|n| {
            names
                .get(n.index())
                .cloned()
                .unwrap_or_else(|| format!("net {}", n.0))
        })
        .collect()
}

/// A violation for the report, with its items described.
fn violation(v: &Violation, rules: &Rules, describe: &dyn Fn(Item) -> String) -> Map {
    let mm = |nm: i64| nm as f64 / 1e6;
    let mut out = Map::default();
    let (kind, items, layer, required): (&str, Vec<Item>, Option<LayerId>, Option<i64>) = match *v {
        Violation::Short { a, b, layer } => ("short", vec![a, b], Some(layer), None),
        Violation::Clearance {
            a,
            b,
            layer,
            required,
            ..
        } => ("clearance", vec![a, b], Some(layer), Some(required)),
        Violation::TrackWidth { track, required } => (
            "track_width",
            vec![Item::Track(track)],
            None,
            Some(required),
        ),
        Violation::Drill { via, required } => ("drill", vec![Item::Via(via)], None, Some(required)),
        Violation::AnnularRing { via, required } => {
            ("annular_ring", vec![Item::Via(via)], None, Some(required))
        }
        Violation::HoleToHole { a, b, required, .. } => {
            ("hole_to_hole", vec![a, b], None, Some(required))
        }
        Violation::HoleClearance {
            hole,
            item,
            layer,
            required,
            ..
        } => (
            "hole_clearance",
            vec![hole, item],
            Some(layer),
            Some(required),
        ),
        Violation::Keepout {
            keepout,
            item,
            layer,
        } => (
            "keepout",
            vec![Item::Keepout(keepout), item],
            Some(layer),
            None,
        ),
        Violation::MaskBridge { item, layer, .. } => ("mask_bridge", vec![item], Some(layer), None),
        _ => ("other", Vec::new(), None, None),
    };
    out.set("kind", kind)
        .set(
            "items",
            items.into_iter().map(describe).collect::<Vec<String>>(),
        )
        .set("layer", layer.map(|l| layer_name(rules, l)))
        .set("required_mm", required.map(mm));
    out
}

fn describe_kicad(board: &KicadBoard) -> impl Fn(Item) -> String + '_ {
    move |item| match board.source(item) {
        Some(s) => s.description.clone(),
        None => match item {
            Item::Keepout(i) => format!("keepout {i}"),
            other => format!("{other:?}"),
        },
    }
}

/// What a routing did, for the report.
fn routing_report(report: &mut Map, routed: &Routed, names: &[String]) {
    let s = &routed.stats;
    report
        .set("complete", routed.unrouted.is_empty())
        .set("nets", s.nets)
        .set("nets_routed", s.routed)
        .set("connections", s.connections)
        .set("connections_made", s.connected)
        .set("unrouted", net_names(names, &routed.unrouted))
        .set("tracks_added", routed.tracks.len())
        .set("vias_added", routed.vias.len())
        .set("track_mm", s.after_cleanup.1 / 1e6)
        .set("pairs", s.pairs.0)
        .set("pairs_coupled", s.pairs.1)
        .set("length_rules", s.lengths.0)
        .set("length_rules_met", s.lengths.1)
        .set("planes_broken", s.planes_broken);
}

/// Checks the routed board, and reports what breaks a rule with the new copper: of tracks
/// and vias from `first_track` and `first_via` on, and of zones from `first_zone` on.
fn check_routed(
    report: &mut Map,
    board: &Board,
    rules: &Rules,
    new: (usize, usize, usize),
    describe: &dyn Fn(Item) -> String,
) -> Result<usize, Failure> {
    let ours = |item: Item| match item {
        Item::Track(i) => i >= new.0,
        Item::Via(i) => i >= new.1,
        Item::Zone(i) => i >= new.2,
        _ => false,
    };
    let found: Vec<Value> = drc::check(board, rules)
        .map_err(|e| Failure::Input(format!("checking the result: {e}")))?
        .iter()
        .filter(|v| match **v {
            Violation::Short { a, b, .. }
            | Violation::Clearance { a, b, .. }
            | Violation::HoleToHole { a, b, .. } => ours(a) || ours(b),
            Violation::HoleClearance { hole, item, .. } => ours(hole) || ours(item),
            Violation::Keepout { item, .. } | Violation::MaskBridge { item, .. } => ours(item),
            Violation::TrackWidth { track, .. } => ours(Item::Track(track)),
            Violation::Drill { via, .. } | Violation::AnnularRing { via, .. } => {
                ours(Item::Via(via))
            }
            _ => true,
        })
        .map(|v| violation(v, rules, describe).into())
        .collect();
    let count = found.len();
    report.set("violations", Value::List(found));
    Ok(count)
}

fn pcb_route_command() -> Command {
    Command {
        name: "pcb route",
        about: "Routes a KiCad board: the connections its tracks don't make yet, or all of them \
                with --strip, with its zones filled again around everything.",
        operands: "BOARD.kicad_pcb",
        count: (1, 1),
        options: with(
            with(
                vec![
                    args::output("Where to write the routed board (`-` for standard output)"),
                    PROJECT,
                    NO_PROJECT,
                    STRIP,
                    CHECK,
                ],
                route_opts(),
            ),
            common(),
        ),
        details: "Example:\n  guide pcb route board.kicad_pcb -o routed.kicad_pcb\n\n\
                  Tracks and vias already on the board are written back with new UUIDs, and \
                  locks and teardrops are not kept. Without -o, nothing is written. Routing \
                  is deterministic and has no time limit; --max-expansions bounds each net's \
                  search.",
    }
}

/// `guide pcb route`.
pub fn route(argv: &[String]) -> Outcome {
    let m = pcb_route_command().parse(argv)?;
    let ui = Ui::new(&m);
    let options = route_options(&m)?;
    let loaded = load_kicad(&m, &ui)?;
    let read = &loaded.board;
    let mut design = Design::from_kicad(read);
    // Zones' fills are routing's to make again; filled graphics stay.
    let zones = std::mem::take(&mut design.board.zones);
    design.board.zones = zones
        .into_iter()
        .zip(&read.zones)
        .filter(|(_, source)| source.description.starts_with("graphic"))
        .map(|(z, _)| z)
        .collect();
    if m.flag("strip") {
        design.board.tracks.clear();
        design.board.vias.clear();
    }
    let (kept_tracks, kept_vias) = (design.board.tracks.clone(), design.board.vias.clone());
    ui.progress(format!(
        "routing {} nets, keeping {} tracks and {} vias",
        design.nets.iter().filter(|n| n.route).count(),
        kept_tracks.len(),
        kept_vias.len()
    ));
    let routed = pcb::route(&design, &options)
        .map_err(|e| Failure::Input(format!("{}: {e}", loaded.path)))?;
    let mut report = Map::default();
    routing_report(&mut report, &routed, &read.nets);
    let mut complete = routed.unrouted.is_empty();
    if m.flag("check") {
        ui.progress("checking");
        let new = (
            design.board.tracks.len(),
            design.board.vias.len(),
            design.board.zones.len(),
        );
        let board = routed.board(&design);
        let describe = |item: Item| match item {
            Item::Track(i) if i >= new.0 => format!("new track {}", i - new.0),
            Item::Via(i) if i >= new.1 => format!("new via {}", i - new.1),
            Item::Zone(i) if i >= new.2 => format!("zone fill {}", i - new.2),
            other => describe_kicad(read)(other),
        };
        complete &= check_routed(&mut report, &board, &design.rules, new, &describe)? == 0;
    }
    let mut written = None;
    if let Some(path) = m.get("output") {
        let mut tracks: Vec<Track> = kept_tracks;
        tracks.extend(routed.tracks.iter().cloned());
        let mut vias: Vec<Via> = kept_vias;
        vias.extend(routed.vias.iter().cloned());
        let text =
            kicad::replace_routing_and_fills(&loaded.pcb, &tracks, &vias, &routed.fills, read);
        write_output(path, text.as_bytes())?;
        written = Some(path);
    }
    report.set("written", written);
    ui.finish("pcb route", report)?;
    Ok(complete)
}

const SEED: Opt = Opt::value("seed", "N", "The placer's seed [default: 1]");
const EFFORT: Opt = Opt::value(
    "effort",
    "F",
    "Moves of the annealing after each placement, per part [default: 3]",
);

fn place_options(m: &Matches) -> Result<PlaceOptions, Failure> {
    let mut o = PlaceOptions::default();
    if let Some(seed) = m.number("seed")? {
        o.seed = seed;
    }
    if let Some(effort) = m.factor("effort")? {
        o.effort = effort;
    }
    o.rotate = !m.flag("no-rotate");
    o.flip = !m.flag("no-flip");
    Ok(o)
}

fn place_opts() -> Vec<Opt> {
    vec![
        SEED,
        EFFORT,
        Opt::flag("no-rotate", "Keep parts at their angles"),
        Opt::flag("no-flip", "Keep parts on their sides"),
    ]
}

fn pcb_place_command() -> Command {
    Command {
        name: "pcb place",
        about: "Places a KiCad board's movable footprints, those that aren't locked or fixed by \
                what they are, such as connectors and mounting holes.",
        operands: "BOARD.kicad_pcb",
        count: (1, 1),
        options: with(
            with(
                vec![
                    args::output("Where to write the placed board (`-` for standard output)"),
                    PROJECT,
                    NO_PROJECT,
                    Opt::flag(
                        "keep-routing",
                        "Keep the tracks and vias, which no longer fit the placement; by \
                         default they are taken out",
                    ),
                ],
                place_opts(),
            ),
            common(),
        ),
        details: "Example:\n  guide pcb place board.kicad_pcb -o placed.kicad_pcb\n\n\
                  Then `guide pcb route placed.kicad_pcb`, or `guide pcb flow` for both in a \
                  loop.",
    }
}

/// `guide pcb place`.
pub fn place(argv: &[String]) -> Outcome {
    let m = pcb_place_command().parse(argv)?;
    let ui = Ui::new(&m);
    let options = place_options(&m)?;
    let loaded = load_kicad(&m, &ui)?;
    let read = &loaded.board;
    let design = kicad::placement_design(read);
    ui.progress(format!(
        "placing {} of {} footprints",
        design.components.iter().filter(|c| !c.fixed).count(),
        design.components.len()
    ));
    let placed = place_components(&design, &options)
        .map_err(|e| Failure::Input(format!("{}: placing: {e}", loaded.path)))?;
    let overlaps: Vec<String> = outline_overlaps(&design, &placed.placed)
        .into_iter()
        .filter(|&(a, b)| !design.components[a].fixed || !design.components[b].fixed)
        .map(|(a, b)| {
            format!(
                "{} and {}",
                design.components[a].name, design.components[b].name
            )
        })
        .collect();
    let legal = overlaps.is_empty();
    let mut report = Map::default();
    report
        .set("legal", legal)
        .set("footprints", design.components.len())
        .set("movable", placed.movable)
        .set(
            "on_back",
            placed
                .placed
                .iter()
                .filter(|p| p.side == Side::Back)
                .count(),
        )
        .set("wirelength_mm_before", placed.wirelength.0 as f64 / 1e6)
        .set("wirelength_mm_after", placed.wirelength.1 as f64 / 1e6)
        .set("spacing_mm", placed.spacing as f64 / 1e6)
        .set("overlaps", overlaps);
    let mut written = None;
    if let Some(path) = m.get("output") {
        let mut text = kicad::replace_placement(&loaded.pcb, read, &placed.placed);
        if !m.flag("keep-routing") {
            text = kicad::replace_routing(&text, &[], &[], &read.nets, &read.rules);
        }
        write_output(path, text.as_bytes())?;
        written = Some(path);
    }
    report.set("written", written);
    ui.finish("pcb place", report)?;
    Ok(legal)
}

fn pcb_flow_command() -> Command {
    Command {
        name: "pcb flow",
        about: "Places and routes a KiCad board, then places again the parts around what routed \
                worst, rerouting only what moved, while that pays.",
        operands: "BOARD.kicad_pcb",
        count: (1, 1),
        options: with(
            with(
                with(
                    vec![
                        args::output(
                            "Where to write the placed and routed board (`-` for standard output)",
                        ),
                        PROJECT,
                        NO_PROJECT,
                        Opt::value(
                            "rounds",
                            "N",
                            "Rounds of placing again at most; 0 is placement then routing \
                             [default: 6]",
                        ),
                    ],
                    place_opts(),
                ),
                route_opts(),
            ),
            common(),
        ),
        details: "Example:\n  guide pcb flow board.kicad_pcb -o done.kicad_pcb\n\n\
                  Every track and via is routed again: the board's own are taken out.",
    }
}

fn score(s: &kicad::Score) -> Map {
    let mut out = Map::default();
    out.set("connections", s.connections.0)
        .set("connections_made", s.connections.1)
        .set("vias", s.vias)
        .set("track_mm", s.track / 1e6)
        .set("starved_thermals", s.starved);
    out
}

/// `guide pcb flow`.
pub fn flow(argv: &[String]) -> Outcome {
    let m = pcb_flow_command().parse(argv)?;
    let ui = Ui::new(&m);
    let mut options = FlowOptions::default();
    options.place = place_options(&m)?;
    options.route = route_options(&m)?;
    if let Some(rounds) = m.number("rounds")? {
        options.rounds = rounds;
    }
    let loaded = load_kicad(&m, &ui)?;
    ui.progress("placing and routing");
    let flow = kicad::place_and_route(&loaded.pcb, loaded.project.as_deref(), &options)
        .map_err(|e| Failure::Input(format!("{}: {e}", loaded.path)))?;
    for (i, round) in flow.rounds.iter().enumerate() {
        ui.detail(format!(
            "round {i}: {} parts placed again, {} nets rerouted, kept: {}",
            round.window.0, round.rerouted, round.kept
        ));
    }
    let mut report = Map::default();
    routing_report(&mut report, &flow.routed, &loaded.board.nets);
    report
        .set("first", score(&flow.first))
        .set("rounds", flow.rounds.len())
        .set("rounds_kept", flow.rounds.iter().filter(|r| r.kept).count());
    let complete = flow.routed.unrouted.is_empty();
    let mut written = None;
    if let Some(path) = m.get("output") {
        write_output(path, flow.text.as_bytes())?;
        written = Some(path);
    }
    report.set("written", written);
    ui.finish("pcb flow", report)?;
    Ok(complete)
}

fn drc_command() -> Command {
    Command {
        name: "pcb drc",
        about: "Checks a KiCad board, or a Specctra DSN design, for clearances, shorts, holes, \
                keepouts and solder mask bridges, and for nets left unconnected.",
        operands: "BOARD.kicad_pcb|DESIGN.dsn",
        count: (1, 1),
        options: with(
            vec![
                PROJECT,
                NO_PROJECT,
                Opt::flag(
                    "kicad-tolerance",
                    "Decide clearances as KiCad does, with 0.5 µm of slack; by default checks \
                     are exact",
                ),
                Opt::flag(
                    "ignore-unconnected",
                    "Don't count unconnected nets as failures",
                ),
            ],
            common(),
        ),
        details: "Example:\n  guide pcb drc board.kicad_pcb --json drc.json\n\n\
                  The exit status is 1 if anything is found. A DSN file has no zone fills, so \
                  nets that only their zones join show as unconnected there: \
                  --ignore-unconnected leaves them out of the status.",
    }
}

/// `guide pcb drc`.
pub fn drc(argv: &[String]) -> Outcome {
    let m = drc_command().parse(argv)?;
    let ui = Ui::new(&m);
    let path = m.operands[0].clone();
    let is_dsn = Path::new(&path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dsn"));
    type Describe = Box<dyn Fn(Item) -> String>;
    let (mut board, rules, names, describe): (Board, Rules, Vec<String>, Describe) = if is_dsn {
        ui.progress(format!("reading {path}"));
        let dsn = dsn::read(&read_text(&path)?).map_err(|e| Failure::file(&path, e))?;
        ui.warnings(&path, &dsn.warnings);
        let (board, rules, names) = (dsn.board.clone(), dsn.rules.clone(), dsn.nets.clone());
        let describe = move |item: Item| dsn.describe(item).unwrap_or_else(|| format!("{item:?}"));
        (board, rules, names, Box::new(describe))
    } else {
        let loaded = load_kicad(&m, &ui)?;
        let read = loaded.board;
        let (board, rules, names) = (read.board.clone(), read.rules.clone(), read.nets.clone());
        (
            board,
            rules,
            names,
            Box::new(move |item| describe_kicad(&read)(item)),
        )
    };
    if m.flag("kicad-tolerance") {
        board.clearance_slack = drc::KICAD_CLEARANCE_SLACK;
    }
    ui.progress("checking");
    let violations = drc::check(&board, &rules).map_err(|e| Failure::file(&path, e))?;
    let unconnected = pcb::unconnected(&board, &rules);
    let mut report = Map::default();
    let clean = violations.is_empty() && (unconnected.is_empty() || m.flag("ignore-unconnected"));
    report
        .set("clean", clean)
        .set("tracks", board.tracks.len())
        .set("vias", board.vias.len())
        .set("pads", board.pads.len())
        .set("unconnected_nets", net_names(&names, &unconnected))
        .set(
            "violations",
            Value::List(
                violations
                    .iter()
                    .map(|v| violation(v, &rules, &*describe).into())
                    .collect(),
            ),
        );
    ui.finish("pcb drc", report)?;
    Ok(clean)
}

fn dsn_route_command() -> Command {
    Command {
        name: "dsn route",
        about: "Routes a Specctra DSN design, such as KiCad's File > Export > Specctra DSN, and \
                writes a session, .ses, for KiCad's File > Import > Specctra Session: in place of \
                Freerouting.",
        operands: "[DESIGN.dsn]",
        count: (0, 1),
        options: with(
            with(
                vec![
                    Opt::value("input", "FILE", "The design, as Freerouting's -de")
                        .aliases(&["-de"]),
                    args::output("Where to write the session (`-` for standard output)")
                        .aliases(&["-do"]),
                    STRIP,
                    CHECK,
                    Opt::value(
                        "edge-clearance",
                        "MM",
                        "What copper keeps from the board's edge, which a DSN file doesn't say \
                         [default: the board's clearance]",
                    ),
                ],
                route_opts(),
            ),
            common(),
        ),
        details: "Examples:\n  guide dsn route board.dsn -o board.ses\n  \
                  guide dsn route -de board.dsn -do board.ses\n\n\
                  Wiring that the design marks protect or fix stays, and is not written to the \
                  session, as KiCad keeps its locked tracks on import. Other wiring stays too, \
                  unless --strip, and is written back with what is added. Planes with a net are \
                  routed to as zones: refill zones in KiCad after importing (B).",
    }
}

/// `guide dsn route`.
pub fn dsn_route(argv: &[String]) -> Outcome {
    let m = dsn_route_command().parse(argv)?;
    let ui = Ui::new(&m);
    let options = route_options(&m)?;
    let path = match (m.operands.first(), m.get("input")) {
        (Some(_), Some(_)) => {
            return Err(Failure::Usage(
                "give the design once, as an argument or with -de".to_owned(),
            ));
        }
        (Some(p), None) => p.clone(),
        (None, Some(p)) => p.to_owned(),
        (None, None) => {
            return Err(Failure::Usage(
                "missing DESIGN.dsn\nFor more, try `guide dsn route --help`.".to_owned(),
            ));
        }
    };
    ui.progress(format!("reading {path}"));
    let dsn: DsnBoard = dsn::read(&read_text(&path)?).map_err(|e| Failure::file(&path, e))?;
    ui.warnings(&path, &dsn.warnings);
    let mut design = Design::from_dsn(&dsn);
    if let Some(clearance) = m.millimetres("edge-clearance")? {
        design.edge_clearance = clearance;
    }
    // What stays in the design, and what of it goes back into the session, with its type.
    let mut session_tracks: Vec<Track> = Vec::new();
    let mut track_types: Vec<WireType> = Vec::new();
    let mut session_vias: Vec<Via> = Vec::new();
    let mut via_types: Vec<WireType> = Vec::new();
    let strip = m.flag("strip");
    let kind =
        |sources: &[dsn::WireSource], i: usize| sources.get(i).map_or(WireType::Normal, |s| s.kind);
    let tracks = std::mem::take(&mut design.board.tracks);
    for (i, t) in tracks.into_iter().enumerate() {
        let k = kind(&dsn.tracks, i);
        if k.is_fixed() {
            design.board.tracks.push(t);
        } else if !strip {
            session_tracks.push(t.clone());
            track_types.push(k);
            design.board.tracks.push(t);
        }
    }
    let vias = std::mem::take(&mut design.board.vias);
    for (i, v) in vias.into_iter().enumerate() {
        let k = kind(&dsn.vias, i);
        if k.is_fixed() {
            design.board.vias.push(v);
        } else if !strip {
            session_vias.push(v.clone());
            via_types.push(k);
            design.board.vias.push(v);
        }
    }
    ui.progress(format!(
        "routing {} nets, keeping {} tracks and {} vias",
        design.nets.iter().filter(|n| n.route).count(),
        design.board.tracks.len(),
        design.board.vias.len()
    ));
    let routed =
        pcb::route(&design, &options).map_err(|e| Failure::Input(format!("{path}: {e}")))?;
    let mut report = Map::default();
    routing_report(&mut report, &routed, &dsn.nets);
    let mut complete = routed.unrouted.is_empty();
    if m.flag("check") {
        ui.progress("checking");
        let new = (
            design.board.tracks.len(),
            design.board.vias.len(),
            design.board.zones.len(),
        );
        let board = routed.board(&design);
        let describe = |item: Item| match item {
            Item::Track(i) if i >= new.0 => format!("new track {}", i - new.0),
            Item::Via(i) if i >= new.1 => format!("new via {}", i - new.1),
            Item::Zone(i) => format!("zone fill {i}"),
            other => dsn.describe(other).unwrap_or_else(|| format!("{other:?}")),
        };
        complete &= check_routed(&mut report, &board, &design.rules, new, &describe)? == 0;
    }
    let mut written = None;
    if let Some(out) = m.get("output") {
        session_tracks.extend(routed.tracks.iter().cloned());
        track_types.resize(session_tracks.len(), WireType::Route);
        session_vias.extend(routed.vias.iter().cloned());
        via_types.resize(session_vias.len(), WireType::Route);
        let base = Path::new(&path)
            .file_name()
            .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
        let layers = &dsn.rules.spec().layers;
        let mut session = Session::new(&base, layers, &dsn.nets, &session_tracks, &session_vias);
        session.track_types = &track_types;
        session.via_types = &via_types;
        let text = write_session(&session).map_err(|e| Failure::file(out, e))?;
        write_output(out, text.as_bytes())?;
        written = Some(out);
    }
    report.set("written", written);
    ui.finish("dsn route", report)?;
    Ok(complete)
}
