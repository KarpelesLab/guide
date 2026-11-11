//! A small command-line parser, so that the tool needs no dependency: each command declares
//! its options in a table, which gives both the parsing and the `--help` text.

use std::collections::BTreeMap;

/// Why a command did not run to the end.
#[derive(Debug, PartialEq)]
pub enum Failure {
    /// `--help` was asked for: the text to print, after which the tool succeeds.
    Help(String),
    /// The command line is wrong.
    Usage(String),
    /// An input could not be read or understood, or an output written.
    Input(String),
}

impl Failure {
    /// A failure to read or write `path`.
    pub fn file(path: &str, error: impl std::fmt::Display) -> Failure {
        Failure::Input(format!("{path}: {error}"))
    }
}

/// An option of a command.
#[derive(Clone, Copy, Debug)]
pub struct Opt {
    /// Its long name, without the dashes, which also names it in [`Matches`].
    pub long: &'static str,
    /// Its short form, if any, such as `o` for `-o`.
    pub short: Option<char>,
    /// Other spellings, with their dashes, such as `-de`.
    pub aliases: &'static [&'static str],
    /// The name of its value, such as `FILE`, or `None` for a switch.
    pub value: Option<&'static str>,
    /// One line on what it does.
    pub help: &'static str,
}

impl Opt {
    /// A switch.
    pub const fn flag(long: &'static str, help: &'static str) -> Opt {
        Opt {
            long,
            short: None,
            aliases: &[],
            value: None,
            help,
        }
    }

    /// An option with a value.
    pub const fn value(long: &'static str, value: &'static str, help: &'static str) -> Opt {
        Opt {
            long,
            short: None,
            aliases: &[],
            value: Some(value),
            help,
        }
    }

    /// The same, with a short form.
    pub const fn short(mut self, short: char) -> Opt {
        self.short = Some(short);
        self
    }

    /// The same, with other spellings.
    pub const fn aliases(mut self, aliases: &'static [&'static str]) -> Opt {
        self.aliases = aliases;
        self
    }
}

/// `-o FILE`.
pub const fn output(help: &'static str) -> Opt {
    Opt::value("output", "FILE", help).short('o')
}

/// `--json FILE`.
pub const JSON: Opt = Opt::value(
    "json",
    "FILE",
    "Write a machine-readable report, as JSON, to FILE (`-` for standard output)",
);
/// `-q`.
pub const QUIET: Opt = Opt::flag("quiet", "Print nothing but errors").short('q');
/// `-v`.
pub const VERBOSE: Opt = Opt::flag(
    "verbose",
    "Print more: warnings about the inputs, and each iteration or round",
)
.short('v');
/// `--threads N`.
pub const THREADS: Opt = Opt::value(
    "threads",
    "N",
    "Threads to search with; results are the same for any number [default: 1]",
)
.short('j');
/// `--time-limit SECONDS`.
pub const TIME_LIMIT: Opt = Opt::value(
    "time-limit",
    "SECONDS",
    "Stop routing after this long and keep the best result so far; a run that hits it may not be reproducible",
);

/// A command: what it is called, what it takes, and what it does.
pub struct Command {
    /// Its words, such as `pcb route`.
    pub name: &'static str,
    /// One line on what it does.
    pub about: &'static str,
    /// Its positional arguments, as in the usage line, such as `BOARD.kicad_pcb`.
    pub operands: &'static str,
    /// How many positional arguments it takes, at least and at most.
    pub count: (usize, usize),
    /// Its options, other than `--help`.
    pub options: Vec<Opt>,
    /// More text for its help, after the options.
    pub details: &'static str,
}

impl Command {
    /// Its help text.
    pub fn help(&self) -> String {
        let mut text = format!(
            "{}\n\nUsage: guide {} [OPTIONS] {}\n\nOptions:\n",
            self.about, self.name, self.operands
        );
        let mut rows: Vec<(String, &str)> = Vec::new();
        for o in &self.options {
            let mut names = match o.short {
                Some(s) => format!("-{s}, --{}", o.long),
                None => format!("    --{}", o.long),
            };
            if let Some(v) = o.value {
                names.push(' ');
                names.push_str(v);
            }
            for a in o.aliases {
                names.push_str(", ");
                names.push_str(a);
            }
            rows.push((names, o.help));
        }
        rows.push(("-h, --help".to_owned(), "Print this help"));
        let width = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        for (names, help) in rows {
            text.push_str(&format!("  {names:width$}  {help}\n"));
        }
        if !self.details.is_empty() {
            text.push('\n');
            text.push_str(self.details.trim_end());
            text.push('\n');
        }
        text
    }

    fn usage_error(&self, message: impl std::fmt::Display) -> Failure {
        Failure::Usage(format!(
            "{message}\nUsage: guide {} [OPTIONS] {}\nFor more, try `guide {} --help`.",
            self.name, self.operands, self.name
        ))
    }

    fn find(&self, spelled: &str) -> Option<&Opt> {
        self.options.iter().find(|o| {
            o.aliases.contains(&spelled)
                || spelled.strip_prefix("--") == Some(o.long)
                || o.short.is_some_and(|s| {
                    let mut chars = spelled.chars();
                    chars.next() == Some('-') && chars.next() == Some(s) && chars.next().is_none()
                })
        })
    }

    /// The closest long option to a misspelled one, if any is close.
    fn suggest(&self, spelled: &str) -> Option<&'static str> {
        let word = spelled.trim_start_matches('-');
        self.options
            .iter()
            .map(|o| (distance(word, o.long), o.long))
            .filter(|&(d, long)| d <= 2.max(long.len() / 4) && d < long.len())
            .min()
            .map(|(_, long)| long)
    }

    /// Parses the arguments after the command's words.
    pub fn parse(&self, args: &[String]) -> Result<Matches, Failure> {
        let mut m = Matches {
            command: self.name,
            values: BTreeMap::new(),
            operands: Vec::new(),
        };
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            if arg == "--" {
                m.operands.extend(rest.by_ref().cloned());
                break;
            }
            if arg == "-h" || arg == "--help" {
                return Err(Failure::Help(self.help()));
            }
            if arg.len() < 2 || !arg.starts_with('-') {
                m.operands.push(arg.clone());
                continue;
            }
            let (spelled, inline) = match arg.split_once('=') {
                Some((name, value)) if arg.starts_with("--") => (name, Some(value)),
                _ => (arg.as_str(), None),
            };
            let Some(opt) = self.find(spelled) else {
                let hint = self
                    .suggest(spelled)
                    .map(|s| format!(" (did you mean --{s}?)"))
                    .unwrap_or_default();
                return Err(self.usage_error(format!("unknown option {spelled}{hint}")));
            };
            let value = match (opt.value, inline) {
                (None, None) => String::new(),
                (None, Some(_)) => {
                    return Err(self.usage_error(format!("--{} takes no value", opt.long)));
                }
                (Some(_), Some(v)) => v.to_owned(),
                (Some(name), None) => match rest.next() {
                    Some(v) => v.clone(),
                    None => {
                        return Err(self.usage_error(format!("{spelled} needs a value, {name}")));
                    }
                },
            };
            if m.values.insert(opt.long, value).is_some() {
                return Err(self.usage_error(format!("--{} is given more than once", opt.long)));
            }
        }
        let (least, most) = self.count;
        if m.operands.len() < least {
            return Err(self.usage_error(format!("missing {}", self.operands)));
        }
        if m.operands.len() > most {
            return Err(self.usage_error(format!("unexpected argument {:?}", m.operands[most])));
        }
        Ok(m)
    }
}

/// The edit distance between two words, for suggestions.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let next = (previous + usize::from(ca != cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            previous = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// What a command line gave.
#[derive(Debug)]
pub struct Matches {
    command: &'static str,
    values: BTreeMap<&'static str, String>,
    /// The positional arguments, in order.
    pub operands: Vec<String>,
}

impl Matches {
    fn bad(&self, name: &str, what: &str, value: &str) -> Failure {
        Failure::Usage(format!(
            "--{name} needs {what}, not {value:?}\nFor more, try `guide {} --help`.",
            self.command
        ))
    }

    /// Whether a switch was given.
    pub fn flag(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    /// An option's value.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// A required option's value.
    pub fn required(&self, name: &str) -> Result<&str, Failure> {
        self.get(name).ok_or_else(|| {
            Failure::Usage(format!(
                "--{name} is required\nFor more, try `guide {} --help`.",
                self.command
            ))
        })
    }

    /// A whole number.
    pub fn number<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, Failure> {
        self.get(name)
            .map(|v| {
                v.parse::<T>()
                    .map_err(|_| self.bad(name, "a whole number", v))
            })
            .transpose()
    }

    /// A finite number, at least 0.
    pub fn factor(&self, name: &str) -> Result<Option<f64>, Failure> {
        self.get(name)
            .map(|v| {
                v.parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite() && *f >= 0.0)
                    .ok_or_else(|| self.bad(name, "a number of at least 0", v))
            })
            .transpose()
    }

    /// A length in millimetres, as nanometres, at least 0 and at most a metre.
    pub fn millimetres(&self, name: &str) -> Result<Option<i64>, Failure> {
        self.get(name)
            .map(|v| {
                v.parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite() && (0.0..=1000.0).contains(f))
                    .map(|f| (f * 1e6).round() as i64)
                    .ok_or_else(|| self.bad(name, "a length in millimetres", v))
            })
            .transpose()
    }

    /// A duration in seconds.
    pub fn seconds(&self, name: &str) -> Result<Option<std::time::Duration>, Failure> {
        self.get(name)
            .map(|v| {
                v.parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite() && *f > 0.0 && *f < 1e9)
                    .map(std::time::Duration::from_secs_f64)
                    .ok_or_else(|| self.bad(name, "a number of seconds above 0", v))
            })
            .transpose()
    }

    /// One of some words, by position.
    pub fn choice(&self, name: &str, words: &[&str]) -> Result<Option<usize>, Failure> {
        self.get(name)
            .map(|v| {
                words
                    .iter()
                    .position(|w| *w == v)
                    .ok_or_else(|| self.bad(name, &format!("one of {}", words.join(", ")), v))
            })
            .transpose()
    }

    /// `--threads`, at least 1.
    pub fn threads(&self) -> Result<usize, Failure> {
        match self.number::<usize>("threads")? {
            Some(0) => Err(self.bad("threads", "at least 1", "0")),
            Some(n) => Ok(n),
            None => Ok(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> Command {
        Command {
            name: "test run",
            about: "Runs a test.",
            operands: "IN [IN2]",
            count: (1, 2),
            options: vec![
                output("Where to write"),
                Opt::value("seed", "N", "A seed"),
                Opt::flag("strip", "Strip it"),
                Opt::value("in", "FILE", "Input").aliases(&["-de"]),
                JSON,
                QUIET,
            ],
            details: "More.",
        }
    }

    fn args(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_options_and_operands() {
        let m = command()
            .parse(&args(&[
                "a", "-o", "out", "--seed=3", "--strip", "-de", "x", "--", "--b",
            ]))
            .unwrap();
        assert_eq!(m.operands, ["a", "--b"]);
        assert_eq!(m.get("output"), Some("out"));
        assert_eq!(m.number::<u64>("seed").unwrap(), Some(3));
        assert!(m.flag("strip"));
        assert_eq!(m.get("in"), Some("x"));
        assert!(!m.flag("quiet"));
    }

    #[test]
    fn rejects_what_is_wrong() {
        let c = command();
        let usage = |a: &[&str]| matches!(c.parse(&args(a)), Err(Failure::Usage(_)));
        assert!(usage(&[]));
        assert!(usage(&["a", "b", "c"]));
        assert!(usage(&["a", "--nope"]));
        assert!(usage(&["a", "--seed"]));
        assert!(usage(&["a", "--strip=yes"]));
        assert!(usage(&["a", "--seed", "1", "--seed", "2"]));
        assert!(usage(&["a", "-x"]));
        let m = c.parse(&args(&["a", "--seed", "x"])).unwrap();
        assert!(matches!(m.number::<u64>("seed"), Err(Failure::Usage(_))));
        assert!(matches!(m.factor("seed"), Err(Failure::Usage(_))));
        assert!(matches!(m.required("output"), Err(Failure::Usage(_))));
    }

    #[test]
    fn suggests_close_options() {
        let Err(Failure::Usage(text)) = command().parse(&args(&["a", "--sed", "1"])) else {
            panic!("an unknown option is refused");
        };
        assert!(text.contains("did you mean --seed?"), "{text}");
    }

    #[test]
    fn help_lists_every_option() {
        let Err(Failure::Help(text)) = command().parse(&args(&["a", "--help"])) else {
            panic!("--help asks for help");
        };
        for word in [
            "-o, --output FILE",
            "--seed N",
            "--in FILE, -de",
            "-h, --help",
            "More.",
        ] {
            assert!(text.contains(word), "{word} in {text}");
        }
    }

    #[test]
    fn numbers_are_checked() {
        let c = Command {
            options: vec![
                Opt::value("f", "F", ""),
                Opt::value("mm", "MM", ""),
                Opt::value("t", "S", ""),
                Opt::value("threads", "N", ""),
            ],
            ..command()
        };
        let parse = |a: &[&str]| c.parse(&args(a)).unwrap();
        for bad in ["-1", "nan", "inf", "", "1e400"] {
            let m = parse(&["a", "--f", bad, "--mm", bad, "--t", bad]);
            assert!(m.factor("f").is_err(), "{bad}");
            assert!(m.millimetres("mm").is_err(), "{bad}");
            assert!(m.seconds("t").is_err(), "{bad}");
        }
        let m = parse(&["a", "--mm", "0.1234567", "--t", "0.5", "--threads", "0"]);
        assert_eq!(m.millimetres("mm").unwrap(), Some(123_457));
        assert_eq!(
            m.seconds("t").unwrap(),
            Some(std::time::Duration::from_millis(500))
        );
        assert!(m.threads().is_err());
    }

    /// Arguments made of random pieces never panic the parser or the getters.
    #[test]
    fn random_arguments_never_panic() {
        let pieces = [
            "-",
            "--",
            "-o",
            "--output",
            "--seed",
            "--seed=",
            "--seed=-1",
            "--strip=",
            "--json",
            "-q",
            "-de",
            "x",
            "",
            "=",
            "--=",
            "-é",
            "--é",
            "1e999",
            "-1",
            "a=b",
            "--help=no",
            "\u{0}",
            "--in=--in",
        ];
        let c = command();
        let mut state = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..20_000 {
            let mut list = Vec::new();
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            for k in 0..(state % 7) {
                let i = (state.rotate_left(k as u32 * 9) % pieces.len() as u64) as usize;
                list.push(pieces[i].to_owned());
            }
            if let Ok(m) = c.parse(&list) {
                let _ = m.number::<u64>("seed");
                let _ = m.factor("seed");
                let _ = m.millimetres("seed");
                let _ = m.seconds("seed");
                let _ = m.choice("seed", &["a", "b"]);
                let _ = m.threads();
            }
        }
    }
}
