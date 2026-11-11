//! `guide`, the command-line tool: placement and routing of FPGAs, PCBs and ASICs, and
//! multi-agent path finding, from the files of the tools of each field.
//!
//! ```text
//! guide <domain> <action> [options] <files>
//! ```
//!
//! Run `guide --help` for the commands, and `guide <domain> <action> --help` for each one's
//! options. Built with the `cli` feature: `cargo install guide --features cli`.
//!
//! Exit status: 0 if the result is complete and legal, 1 if it is not (nets left unrouted,
//! overuse, violations), and 2 if the command line or an input is wrong, or an output can't
//! be written.

mod args;
mod asic;
mod fpga;
mod mapf;
mod pcb;
mod report;

use std::process::ExitCode;

use args::Failure;

/// What a command came to: whether its result is complete and legal.
type Outcome = Result<bool, Failure>;

/// A command's words, what it does, and how to run it.
struct Entry {
    domain: &'static str,
    action: &'static str,
    about: &'static str,
    run: fn(&[String]) -> Outcome,
}

const COMMANDS: &[Entry] = &[
    Entry {
        domain: "fpga",
        action: "route",
        about: "Route a placed VPR design on a routing resource graph, to a .route file",
        run: fpga::route,
    },
    Entry {
        domain: "fpga",
        action: "place",
        about: "Place a packed VPR netlist on a device, to a .place file",
        run: fpga::place,
    },
    Entry {
        domain: "fpga",
        action: "flow",
        about: "Place, route, and place again where routing fails: .place and .route files",
        run: fpga::flow,
    },
    Entry {
        domain: "pcb",
        action: "route",
        about: "Route a KiCad board, filling its zones",
        run: pcb::route,
    },
    Entry {
        domain: "pcb",
        action: "place",
        about: "Place a KiCad board's movable footprints",
        run: pcb::place,
    },
    Entry {
        domain: "pcb",
        action: "flow",
        about: "Place and route a KiCad board, placing again where routing found trouble",
        run: pcb::flow,
    },
    Entry {
        domain: "pcb",
        action: "drc",
        about: "Check a KiCad board or a DSN design for clearance and connectivity",
        run: pcb::drc,
    },
    Entry {
        domain: "dsn",
        action: "route",
        about: "Route a Specctra DSN design to a session, as Freerouting does",
        run: pcb::dsn_route,
    },
    Entry {
        domain: "mapf",
        action: "solve",
        about: "Solve a MovingAI multi-agent path finding instance",
        run: mapf::solve,
    },
    Entry {
        domain: "asic",
        action: "route",
        about: "Route an ISPD 2007/2008 global routing design, and score the result",
        run: asic::route,
    },
    Entry {
        domain: "asic",
        action: "score",
        about: "Score ISPD routes as the 2008 contest's eval2008.pl does",
        run: asic::score,
    },
];

fn overview() -> String {
    let mut text = format!(
        "guide {}: placement and routing for PCBs, FPGAs, ASICs and multi-agent paths\n\n\
         Usage: guide <DOMAIN> <ACTION> [OPTIONS] <FILES>\n\nCommands:\n",
        env!("CARGO_PKG_VERSION")
    );
    for c in COMMANDS {
        let words = format!("{} {}", c.domain, c.action);
        text.push_str(&format!("  {words:12}  {}\n", c.about));
    }
    text.push_str(
        "\nOptions:\n  -h, --help     Print this help, or a command's with `guide <DOMAIN> <ACTION> --help`\n  \
         -V, --version  Print the version\n\n\
         Exit status: 0 for a complete and legal result, 1 for an incomplete or illegal one, and\n\
         2 for a wrong command line, an input that can't be read, or an output that can't be\n\
         written.\n",
    );
    text
}

fn domain_help(domain: &str) -> String {
    let mut text = format!("Usage: guide {domain} <ACTION> [OPTIONS] <FILES>\n\nActions:\n");
    for c in COMMANDS.iter().filter(|c| c.domain == domain) {
        text.push_str(&format!("  {:6}  {}\n", c.action, c.about));
    }
    text.push_str(&format!(
        "\nFor an action's options, try `guide {domain} <ACTION> --help`.\n"
    ));
    text
}

fn run(args: &[String]) -> Outcome {
    let is_help = |a: &str| a == "-h" || a == "--help";
    let (domain, rest) = match args {
        [] => {
            return Err(Failure::Usage(format!(
                "missing a command\n\n{}",
                overview()
            )));
        }
        [first, ..] if is_help(first) => return Err(Failure::Help(overview())),
        [first, ..] if first == "-V" || first == "--version" => {
            return Err(Failure::Help(format!(
                "guide {}\n",
                env!("CARGO_PKG_VERSION")
            )));
        }
        [first, rest @ ..] if first == "help" => {
            // `guide help`, `guide help pcb`, `guide help pcb route`.
            let mut again: Vec<String> = rest.to_vec();
            again.push("--help".to_owned());
            return run(&again);
        }
        [first, rest @ ..] => (first.as_str(), rest),
    };
    if !COMMANDS.iter().any(|c| c.domain == domain) {
        return Err(Failure::Usage(format!(
            "unknown domain {domain:?}: try fpga, pcb, dsn, mapf or asic\n\
             For more, try `guide --help`."
        )));
    }
    let (action, rest) = match rest {
        [] => {
            return Err(Failure::Usage(format!(
                "missing an action\n\n{}",
                domain_help(domain)
            )));
        }
        [first, ..] if is_help(first) => return Err(Failure::Help(domain_help(domain))),
        [first, rest @ ..] => (first.as_str(), rest),
    };
    match COMMANDS
        .iter()
        .find(|c| c.domain == domain && c.action == action)
    {
        Some(c) => (c.run)(rest),
        None => Err(Failure::Usage(format!(
            "unknown action {action:?} for {domain}\n\n{}",
            domain_help(domain)
        ))),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = match std::env::args_os()
        .skip(1)
        .map(|a| a.into_string())
        .collect::<Result<_, _>>()
    {
        Ok(args) => args,
        Err(bad) => {
            eprintln!("guide: error: an argument is not valid Unicode: {bad:?}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(Failure::Help(text)) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(Failure::Usage(message) | Failure::Input(message)) => {
            eprintln!("guide: error: {message}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn every_command_has_help() {
        for c in COMMANDS {
            let Err(Failure::Help(text)) = run(&args(&[c.domain, c.action, "--help"])) else {
                panic!("{} {} --help", c.domain, c.action);
            };
            assert!(text.contains(&format!("guide {} {}", c.domain, c.action)));
            assert_eq!(
                run(&args(&["help", c.domain, c.action])),
                Err(Failure::Help(text))
            );
        }
    }

    #[test]
    fn wrong_commands_are_usage_errors() {
        for line in [
            &[][..],
            &["nope"],
            &["pcb"],
            &["pcb", "nope"],
            &["fpga", "route"],
            &["pcb", "route"],
            &["mapf", "solve", "only-a-map"],
            &["asic", "route", "a", "b"],
        ] {
            assert!(
                matches!(run(&args(line)), Err(Failure::Usage(_))),
                "{line:?}"
            );
        }
    }

    #[test]
    fn missing_files_are_input_errors() {
        for line in [
            &["pcb", "route", "/nonexistent/x.kicad_pcb"][..],
            &["pcb", "drc", "/nonexistent/x.dsn"],
            &["dsn", "route", "/nonexistent/x.dsn"],
            &["mapf", "solve", "/nonexistent/m", "/nonexistent/s"],
            &["asic", "route", "/nonexistent/x.gr"],
            &[
                "fpga",
                "route",
                "--rr-graph",
                "/nonexistent/r",
                "--net",
                "/n",
                "--place",
                "/p",
                "--no-timing",
            ],
        ] {
            assert!(
                matches!(run(&args(line)), Err(Failure::Input(_))),
                "{line:?}"
            );
        }
    }
}
