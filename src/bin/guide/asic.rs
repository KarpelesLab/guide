//! `guide asic`: global routing in the ISPD 2007 and 2008 contests' formats.

use guide::asic::{AsicOptions, Design, Evaluation, NetRoute, evaluate, read_design, read_routes};

use crate::Outcome;
use crate::args::{self, Command, Failure, Matches, Opt};
use crate::report::{Map, Ui, open, write_output};

const VIA_COST: Opt = Opt::value(
    "via-cost",
    "N",
    "Wirelength per layer a via spans, in scoring: 1 as in 2008, 3 as in 2007 [default: 1]",
);

fn route_command() -> Command {
    Command {
        name: "asic route",
        about: "Routes an ISPD 2007 or 2008 global routing design, writes the routes in the \
                contests' format, and scores them as the 2008 contest's eval2008.pl does.",
        operands: "DESIGN.gr",
        count: (1, 1),
        options: vec![
            args::output("Where to write the routes (`-` for standard output)"),
            VIA_COST,
            Opt::value(
                "batch",
                "N",
                "Nets routed at once against the same congestion, which changes results \
                 [default: 32]",
            ),
            Opt::value(
                "max-iterations",
                "N",
                "Iterations of negotiation on the plane at most [default: 100]",
            ),
            Opt::flag(
                "no-pattern",
                "Start every net from nothing, not from L-shaped pattern routes",
            ),
            args::THREADS,
            args::JSON,
            args::QUIET,
            args::VERBOSE,
        ],
        details: "Example:\n  guide asic route adaptec1.capo70.3d.35.50.90.gr -o adaptec1.route -j 8\n\n\
                  The exit status is 1 if a net is left unrouted or an edge overflows. Routing \
                  has no time limit: --max-iterations bounds it.",
    }
}

fn score_command() -> Command {
    Command {
        name: "asic score",
        about: "Scores routes in the ISPD contests' format, from any router, as the 2008 \
                contest's eval2008.pl does: overflow, wirelength and vias.",
        operands: "DESIGN.gr ROUTES",
        count: (2, 2),
        options: vec![VIA_COST, args::JSON, args::QUIET, args::VERBOSE],
        details: "Example:\n  guide asic score adaptec1.capo70.3d.35.50.90.gr adaptec1.route",
    }
}

fn read(path: &str) -> Result<Design, Failure> {
    read_design(open(path)?).map_err(|e| Failure::file(path, e))
}

fn via_cost(m: &Matches) -> Result<u64, Failure> {
    Ok(m.number("via-cost")?.unwrap_or(1))
}

/// The score, for the report, and whether it is complete with no overflow.
fn score_report(report: &mut Map, design: &Design, s: &Evaluation) -> bool {
    report
        .set("nets", design.nets.len())
        .set("total_overflow", s.total_overflow)
        .set("max_overflow", s.max_overflow)
        .set("overflowed_edges", s.overflowed_edges)
        .set("wirelength", s.wirelength)
        .set("wire_length", s.wire_length)
        .set("vias", s.vias)
        .set("unrouted_nets", s.unrouted.len())
        .set("unattached_pins", s.unattached_pins)
        .set("faults", s.faults.len())
        .set("nets_with_cycles", s.nets_with_cycles);
    s.is_complete() && s.total_overflow == 0
}

/// `guide asic route`.
pub fn route(argv: &[String]) -> Outcome {
    let m = route_command().parse(argv)?;
    let ui = Ui::new(&m);
    let path = &m.operands[0];
    let mut options = AsicOptions::default();
    options.threads = m.threads()?;
    if let Some(n) = m.number("batch")? {
        options.batch_size = n;
    }
    if let Some(n) = m.number("max-iterations")? {
        options.max_iterations = n;
    }
    options.pattern = !m.flag("no-pattern");
    let via_cost = via_cost(&m)?;

    ui.progress(format!("reading {path}"));
    let design = read(path)?;
    ui.progress(format!("routing {} nets", design.nets.len()));
    let routing = guide::asic::route_design(&design, &options);
    for r in &routing.iterations {
        ui.detail(format!(
            "iteration {}: rerouted {} overused {} overuse {}",
            r.iteration, r.nets_rerouted, r.overused_resources, r.total_overuse
        ));
    }
    let mut text = Vec::new();
    for (net, segments) in design.nets.iter().zip(&routing.routes) {
        if !segments.is_empty() {
            guide::asic::write_net_route(&mut text, &design, net, segments)
                .map_err(|e| Failure::Input(format!("writing routes: {e}")))?;
        }
    }
    let routes: Vec<NetRoute> = read_routes(text.as_slice())
        .map_err(|e| Failure::Input(format!("internal error: routes don't read back: {e}")))?;
    ui.progress("scoring");
    let s = evaluate(&design, &routes, via_cost);
    let mut report = Map::default();
    let good = score_report(&mut report, &design, &s);
    report
        .set("routed_nets", routing.routed_nets)
        .set("iterations", routing.iterations.len())
        .set("planar_overflow", routing.planar_overflow);
    let mut written = None;
    if let Some(out) = m.get("output") {
        write_output(out, &text)?;
        written = Some(out);
    }
    report.set("written", written);
    ui.finish("asic route", report)?;
    Ok(good)
}

/// `guide asic score`.
pub fn score(argv: &[String]) -> Outcome {
    let m = score_command().parse(argv)?;
    let ui = Ui::new(&m);
    let (path, routes_path) = (&m.operands[0], &m.operands[1]);
    let via_cost = via_cost(&m)?;
    ui.progress(format!("reading {path} and {routes_path}"));
    let design = read(path)?;
    let routes = read_routes(open(routes_path)?).map_err(|e| Failure::file(routes_path, e))?;
    let s = evaluate(&design, &routes, via_cost);
    let mut report = Map::default();
    let good = score_report(&mut report, &design, &s);
    ui.finish("asic score", report)?;
    Ok(good)
}
