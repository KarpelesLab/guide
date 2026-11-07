//! Writing routes back into a `.kicad_pcb`.

use std::fmt::Write as _;

use crate::geom::Point;
use crate::geom::Rules;
use crate::geom::drc::{Track, Via};

/// What a list in the text is: its name and where it is.
struct Span {
    start: usize,
    end: usize,
}

/// The lists to drop from a board's text: tracks, arcs and vias at the top level, and every
/// zone's fill, wherever it is.
fn dropped(text: &str) -> Vec<Span> {
    let bytes = text.as_bytes();
    let mut out: Vec<Span> = Vec::new();
    // For each open list, where it starts and its name.
    let mut stack: Vec<(usize, &str)> = Vec::new();
    let (mut quoted, mut escaped) = (false, false);
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if quoted {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => quoted = false,
                _ => {}
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => quoted = true,
            b'(' => {
                let from = i + 1;
                let mut to = from;
                while to < bytes.len()
                    && !matches!(bytes[to], b' ' | b'\t' | b'\n' | b'\r' | b'(' | b')')
                {
                    to += 1;
                }
                stack.push((i, &text[from..to]));
            }
            b')' => {
                if let Some((start, name)) = stack.pop() {
                    let top = stack.len() == 1 && matches!(name, "segment" | "arc" | "via");
                    let fill = matches!(name, "filled_polygon" | "fill_segments");
                    let inside = out.last().is_some_and(|s| s.end > start);
                    if (top || fill) && !inside {
                        out.push(Span { start, end: i + 1 });
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Millimetres with up to six decimals, from nanometres, exactly.
fn mm(nm: i64) -> String {
    let sign = if nm < 0 { "-" } else { "" };
    let a = nm.unsigned_abs();
    let (whole, frac) = (a / 1_000_000, a % 1_000_000);
    if frac == 0 {
        format!("{sign}{whole}")
    } else {
        let f = format!("{frac:06}");
        format!("{sign}{whole}.{}", f.trim_end_matches('0'))
    }
}

/// A point in KiCad's coordinates, with Y down.
fn xy(p: Point) -> String {
    format!("{} {}", mm(p.x), mm(-p.y))
}

/// Replaces a board's routing with other tracks and vias, and returns the new text.
///
/// Every track, arc and via of `pcb` is taken out, and so is every zone's fill, which was
/// made for the old routing: KiCad fills zones again when asked. Everything else is kept as
/// it was, byte for byte, and the tracks and vias are added at the end of the board, in
/// the order given, each with a new UUID. `nets` names the nets by
/// [`NetId`](crate::id::NetId), as [`KicadBoard::nets`](super::KicadBoard::nets) does, and
/// `rules` names the layers. Tracks with an arc's middle point are written as arcs.
///
/// Nets are written by number, as in files up to KiCad 9, if the board declares them that
/// way, and by name otherwise, as in KiCad 10.
///
/// ```
/// use guide::geom::drc::Track;
/// use guide::geom::{LayerId, Point};
/// use guide::id::NetId;
///
/// let pcb = r#"(kicad_pcb (version 20240108) (generator "pcbnew")
///   (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
///   (net 0 "") (net 1 "A")
///   (segment (start 0 0) (end 10 0) (width 0.25) (layer "F.Cu") (net 1) (uuid "t1")))"#;
/// let read = guide::kicad::read(pcb, None)?;
/// let track = Track {
///     start: Point::new(0, 0),
///     end: Point::new(1_500_000, -2_000_000),
///     mid: None,
///     width: 200_000,
///     layer: LayerId(1),
///     net: Some(NetId(1)),
/// };
/// let text = guide::kicad::replace_routing(pcb, &[track], &[], &read.nets, &read.rules);
/// assert!(!text.contains("t1"));
/// assert!(text.contains("(segment (start 0 0) (end 1.5 2) (width 0.2) (layer \"B.Cu\") (net 1)"));
/// let again = guide::kicad::read(&text, None)?;
/// assert_eq!(again.board.tracks.len(), 1);
/// # Ok::<(), guide::kicad::KicadError>(())
/// ```
pub fn replace_routing(
    pcb: &str,
    tracks: &[Track],
    vias: &[Via],
    nets: &[String],
    rules: &Rules,
) -> String {
    // Net numbers, if the board declares them: `(net 3 "name")` at the top level.
    let mut numbers: Vec<Option<String>> = vec![None; nets.len()];
    if let Ok(root) = super::sexpr::parse(pcb) {
        for net in root.children("net") {
            if let (Some(n), Some(name)) = (net.arg(0), net.arg(1))
                && n.parse::<i64>().is_ok()
                && let Some(i) = nets.iter().position(|x| x == name)
            {
                numbers[i] = Some(n.to_string());
            }
        }
    }
    let by_number = numbers.iter().any(Option::is_some);
    let net = |id: Option<crate::id::NetId>| -> String {
        let i = id.map_or(0, |n| n.index());
        match (by_number, numbers.get(i).cloned().flatten()) {
            (true, Some(n)) => format!("(net {n})"),
            (true, None) => "(net 0)".to_string(),
            (false, _) => format!("(net \"{}\")", nets.get(i).map_or("", String::as_str)),
        }
    };
    let id = if pcb.contains("(tstamp ") && !pcb.contains("(uuid ") {
        "tstamp"
    } else {
        "uuid"
    };
    let layer = |l: crate::geom::LayerId| {
        rules
            .spec()
            .layers
            .get(usize::from(l.0))
            .cloned()
            .unwrap_or_default()
    };
    let mut added = String::new();
    let mut count: u64 = 0;
    let mut uuid = || {
        count += 1;
        // "guid" in ASCII, then a count.
        format!("67756964-0000-4000-8000-{count:012x}")
    };
    for t in tracks {
        let kind = if t.mid.is_some() { "arc" } else { "segment" };
        let _ = write!(added, "  ({kind} (start {})", xy(t.start));
        if let Some(m) = t.mid {
            let _ = write!(added, " (mid {})", xy(m));
        }
        let _ = writeln!(
            added,
            " (end {}) (width {}) (layer \"{}\") {} ({id} \"{}\"))",
            xy(t.end),
            mm(t.width),
            layer(t.layer),
            net(t.net),
            uuid()
        );
    }
    for v in vias {
        let _ = writeln!(
            added,
            "  (via (at {}) (size {}) (drill {}) (layers \"{}\" \"{}\") {} ({id} \"{}\"))",
            xy(v.at),
            mm(v.diameter),
            mm(v.drill),
            layer(v.from),
            layer(v.to),
            net(v.net),
            uuid()
        );
    }

    let mut out = String::with_capacity(pcb.len() + added.len());
    let mut at = 0;
    for span in dropped(pcb) {
        // Take the indentation before the list with it, and its line if nothing else is on it.
        let line_start = pcb[..span.start].rfind('\n').map_or(0, |i| i + 1);
        let start = if pcb[line_start..span.start].trim().is_empty() {
            line_start
        } else {
            span.start
        };
        let mut end = span.end;
        if start == line_start && pcb[end..].starts_with('\n') {
            end += 1;
        }
        out.push_str(&pcb[at..start.max(at)]);
        at = end.max(at);
    }
    out.push_str(&pcb[at..]);
    // Before the board's closing parenthesis.
    match out.rfind(')') {
        Some(close) => {
            let mut text = out[..close].to_string();
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&added);
            text.push_str(&out[close..]);
            text
        }
        None => out,
    }
}
