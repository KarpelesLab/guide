//! What every command shares: its report, printed as text and written as JSON, messages on
//! progress, reading and writing files, and the time limit.

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::args::{Failure, Matches};

/// A value in a report.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Nothing, `null`.
    Null,
    /// A yes or no.
    Bool(bool),
    /// A whole number.
    Int(i128),
    /// A number, `null` in JSON if not finite.
    Float(f64),
    /// Text.
    Str(String),
    /// A list.
    List(Vec<Value>),
    /// Keys and values, in order.
    Map(Vec<(String, Value)>),
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Value {
                Value::Int(v as i128)
            }
        }
    )*};
}
from_int!(u8, u16, u32, u64, usize, i32, i64, i128);

impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Float(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::Str(v.to_owned())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::Str(v)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Value {
        v.map_or(Value::Null, Into::into)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Value {
        Value::List(v.into_iter().map(Into::into).collect())
    }
}

/// Keys and values, built in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map(pub Vec<(String, Value)>);

impl Map {
    /// Adds a key and its value.
    pub fn set(&mut self, key: &str, value: impl Into<Value>) -> &mut Map {
        self.0.push((key.to_owned(), value.into()));
        self
    }
}

impl From<Map> for Value {
    fn from(v: Map) -> Value {
        Value::Map(v.0)
    }
}

/// Writes a string as JSON.
fn json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl Value {
    /// The value as JSON, indented by `depth` levels after the first line.
    pub fn json(&self, out: &mut String, depth: usize) {
        let pad = |out: &mut String, depth: usize| {
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
        };
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Value::Float(f) if f.is_finite() => {
                // Debug always has a decimal point or an exponent, both valid JSON.
                let _ = write!(out, "{f:?}");
            }
            Value::Float(_) => out.push_str("null"),
            Value::Str(s) => json_string(out, s),
            Value::List(items) if items.is_empty() => out.push_str("[]"),
            Value::List(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    pad(out, depth + 1);
                    item.json(out, depth + 1);
                }
                pad(out, depth);
                out.push(']');
            }
            Value::Map(entries) if entries.is_empty() => out.push_str("{}"),
            Value::Map(entries) => {
                out.push('{');
                for (i, (key, value)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    pad(out, depth + 1);
                    json_string(out, key);
                    out.push_str(": ");
                    value.json(out, depth + 1);
                }
                pad(out, depth);
                out.push('}');
            }
        }
    }

    /// The value on one line, for text.
    fn text(&self) -> String {
        match self {
            Value::Null => "none".to_owned(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => {
                if f.abs() >= 100.0 || *f == 0.0 {
                    format!("{f:.0}")
                } else {
                    format!("{f:.3}")
                }
            }
            Value::Str(s) => s.clone(),
            Value::List(items) if items.is_empty() => "none".to_owned(),
            Value::List(items) => items.iter().map(Value::text).collect::<Vec<_>>().join(", "),
            Value::Map(entries) => entries
                .iter()
                .map(|(k, v)| format!("{k}={}", v.text()))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// How much to say on standard error, and where the summary goes.
pub struct Ui {
    /// Print nothing but errors.
    pub quiet: bool,
    /// Print more.
    pub verbose: bool,
    /// Where the JSON report goes, if anywhere.
    json: Option<String>,
    /// Whether standard output carries data, so that the summary goes to standard error.
    stdout_taken: bool,
    started: Instant,
}

impl Ui {
    /// From the common options.
    pub fn new(m: &Matches) -> Ui {
        let json = m.get("json").map(str::to_owned);
        let stdout_taken = json.as_deref() == Some("-") || m.get("output") == Some("-");
        Ui {
            quiet: m.flag("quiet"),
            verbose: m.flag("verbose") && !m.flag("quiet"),
            json,
            stdout_taken,
            started: Instant::now(),
        }
    }

    /// Says what is happening, unless quiet.
    pub fn progress(&self, message: impl std::fmt::Display) {
        if !self.quiet {
            eprintln!("[{:7.2}s] {message}", self.started.elapsed().as_secs_f64());
        }
    }

    /// Says more, if verbose.
    pub fn detail(&self, message: impl std::fmt::Display) {
        if self.verbose {
            eprintln!("          {message}");
        }
    }

    /// Lists what a reader warned of, if verbose, or how many warnings there were.
    pub fn warnings(&self, what: &str, warnings: &[(String, usize)]) {
        if warnings.is_empty() {
            return;
        }
        if self.verbose {
            for (message, count) in warnings {
                self.detail(format!("{what}: {message} ({count}x)"));
            }
        } else {
            let total: usize = warnings.iter().map(|w| w.1).sum();
            self.progress(format!(
                "{what}: {total} warnings of {} kinds (-v lists them)",
                warnings.len()
            ));
        }
    }

    /// Prints the summary, unless quiet, and writes the JSON report, if asked.
    pub fn finish(&self, command: &str, report: Map) -> Result<(), Failure> {
        let mut root = Map::default();
        root.set("command", command);
        root.set("version", env!("CARGO_PKG_VERSION"));
        root.0.extend(report.0);
        root.set("seconds", self.started.elapsed().as_secs_f64());
        if !self.quiet {
            let mut text = String::new();
            let width = root.0.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
            for (key, value) in &root.0 {
                match value {
                    Value::List(items) if items.iter().any(|v| matches!(v, Value::Map(_))) => {
                        let _ = writeln!(text, "{key:width$}: {}", items.len());
                        let shown = if self.verbose { items.len() } else { 20 };
                        for item in items.iter().take(shown) {
                            let _ = writeln!(text, "  {}", item.text());
                        }
                        if items.len() > shown {
                            let _ = writeln!(
                                text,
                                "  ... {} more (-v or --json lists all)",
                                items.len() - shown
                            );
                        }
                    }
                    _ => {
                        let _ = writeln!(text, "{key:width$}: {}", value.text());
                    }
                }
            }
            if self.stdout_taken {
                eprint!("{text}");
            } else {
                print!("{text}");
            }
        }
        if let Some(path) = &self.json {
            let mut json = String::new();
            Value::from(root).json(&mut json, 0);
            json.push('\n');
            write_output(path, json.as_bytes())?;
        }
        Ok(())
    }
}

/// Reads a file as text.
pub fn read_text(path: &str) -> Result<String, Failure> {
    std::fs::read_to_string(path).map_err(|e| Failure::file(path, e))
}

/// Opens a file for reading, buffered.
pub fn open(path: &str) -> Result<std::io::BufReader<std::fs::File>, Failure> {
    std::fs::File::open(path)
        .map(|f| std::io::BufReader::with_capacity(1 << 20, f))
        .map_err(|e| Failure::file(path, e))
}

/// Writes a file, or standard output for `-`.
pub fn write_output(path: &str, bytes: &[u8]) -> Result<(), Failure> {
    if path == "-" {
        let mut out = std::io::stdout().lock();
        out.write_all(bytes)
            .and_then(|()| out.flush())
            .map_err(|e| Failure::file("standard output", e))
    } else {
        std::fs::write(path, bytes).map_err(|e| Failure::file(path, e))
    }
}

/// A time limit: a flag that a timer sets once it runs out, for the engine's cancel flag.
/// Ctrl-C can't set it, as the tool has no signal handler: it ends the process at once.
pub struct Deadline {
    flag: Arc<AtomicBool>,
    done: Arc<(Mutex<bool>, Condvar)>,
    timer: Option<JoinHandle<()>>,
}

impl Deadline {
    /// Starts the timer, if there is a limit.
    pub fn start(limit: Option<Duration>) -> Deadline {
        let flag = Arc::new(AtomicBool::new(false));
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let timer = limit.map(|limit| {
            let (flag, done) = (Arc::clone(&flag), Arc::clone(&done));
            std::thread::spawn(move || {
                let (lock, signal) = &*done;
                let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                let (guard, _) = signal
                    .wait_timeout_while(guard, limit, |finished| !*finished)
                    .unwrap_or_else(|e| e.into_inner());
                if !*guard {
                    flag.store(true, Ordering::Relaxed);
                }
            })
        });
        Deadline { flag, done, timer }
    }

    /// The flag, for [`EngineOptions::cancel`](guide::engine::EngineOptions::cancel), if there
    /// is a limit.
    pub fn flag(&self) -> Option<&AtomicBool> {
        self.timer.as_ref().map(|_| &*self.flag)
    }

    /// Whether time ran out.
    pub fn expired(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        let (lock, signal) = &*self.done;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        signal.notify_all();
        if let Some(timer) = self.timer.take() {
            let _ = timer.join();
        }
    }
}

/// The SHA-256 digest of some bytes, in hex: what VPR puts in its files' headers, as
/// `SHA256:` and the digest of the file they refer to.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for block in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:08x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_digests() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256_hex(&million),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn json_is_escaped_and_nested() {
        let mut m = Map::default();
        m.set("a", "q\"\\\n\u{1}é")
            .set("n", Value::Null)
            .set("f", f64::NAN)
            .set("x", 1.5)
            .set("list", vec![1u32, 2])
            .set("empty", Vec::<u32>::new());
        let mut out = String::new();
        Value::from(m).json(&mut out, 0);
        assert_eq!(
            out,
            "{\n  \"a\": \"q\\\"\\\\\\n\\u0001é\",\n  \"n\": null,\n  \"f\": null,\n  \"x\": 1.5,\n  \
             \"list\": [\n    1,\n    2\n  ],\n  \"empty\": []\n}"
        );
    }

    #[test]
    fn a_deadline_sets_its_flag() {
        let d = Deadline::start(Some(Duration::from_millis(10)));
        assert!(d.flag().is_some());
        let started = Instant::now();
        while !d.expired() && started.elapsed() < Duration::from_secs(10) {
            std::thread::yield_now();
        }
        assert!(d.expired());
        let none = Deadline::start(None);
        assert!(none.flag().is_none());
        assert!(!none.expired());
    }
}
