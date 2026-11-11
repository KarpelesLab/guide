//! Constraints for routing from a board's custom rules, in its `.kicad_dru` file: lengths and
//! skews, track widths, and differential pairs' gaps and uncoupled lengths.

use super::KicadBoard;
use super::KicadError;
use super::project::{pair_partner, wildcard};
use crate::id::NetId;
use crate::pcb::{Design, LengthConstraint};
use crate::units::scaled;

/// What [`length_rules`] read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LengthRules {
    /// The constraints, in the order of the rules they come from: a rule's length, and
    /// then its skew, which are each a constraint of their own.
    pub constraints: Vec<LengthConstraint>,
    /// By net, the width of its tracks that a `track_width` constraint gives it: its `opt`,
    /// or failing that its `min`. In the order of the nets.
    pub widths: Vec<(NetId, i64)>,
    /// By differential pair, positive net first, the gap that a `diff_pair_gap` constraint
    /// gives it: its `opt`, or failing that its `min`. In the order of the board's pairs.
    pub pair_gaps: Vec<(NetId, NetId, i64)>,
    /// By differential pair, the longest that its nets may run apart, from a
    /// `diff_pair_uncoupled` constraint's `max`. In the order of the board's pairs.
    pub uncoupled: Vec<(NetId, NetId, i64)>,
    /// What was skipped, with why: rules whose conditions this doesn't understand, and
    /// constraints other than these.
    pub warnings: Vec<String>,
}

impl LengthRules {
    /// Gives a design what these rules ask of its routing: their lengths in
    /// [`lengths`](Design::lengths), their widths to nets' tracks and to the pairs they
    /// are in, and their gaps to pairs. Uncoupled lengths are only measured, not routed for.
    pub fn apply(&self, design: &mut Design) {
        design.lengths.extend(self.constraints.iter().cloned());
        for &(net, width) in &self.widths {
            if let Some(spec) = design.nets.get_mut(net.index()) {
                spec.track_width = width;
            }
            for pair in &mut design.pairs {
                if pair.positive == net || pair.negative == net {
                    pair.width = width;
                }
            }
        }
        for &(positive, negative, gap) in &self.pair_gaps {
            for pair in &mut design.pairs {
                if (pair.positive, pair.negative) == (positive, negative) {
                    pair.gap = gap;
                }
            }
        }
    }
}

/// A length in a rule, such as `1.5mm`, `20mil`, `0.1in` or `150um`, in nanometres. A
/// number alone is in millimetres.
fn length(text: &str) -> Option<i64> {
    let limit = crate::geom::MAX_COORD;
    let (number, factor, scale) = if let Some(n) = text.strip_suffix("mm") {
        (n, 1, 6)
    } else if let Some(n) = text.strip_suffix("mil") {
        (n, 254, 2)
    } else if let Some(n) = text.strip_suffix("in") {
        (n, 254, 5)
    } else if let Some(n) = text.strip_suffix("um") {
        (n, 1, 3)
    } else {
        (text, 1, 6)
    };
    scaled(number.trim(), factor, scale, limit)
}

/// One term of a condition, which a net matches or not.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Term {
    Name(String),
    Class(String),
    Pair(String),
}

/// A condition as terms joined by `||` of terms joined by `&&`, if it is one of those this
/// understands: `A.NetName == '…'`, `A.NetClass == '…'`, `A.hasNetclass('…')` and
/// `A.inDiffPair('…')`.
fn condition(text: &str) -> Option<Vec<Vec<Term>>> {
    let quoted = |s: &str| -> Option<String> {
        let s = s.trim();
        let inner = s
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .or_else(|| s.strip_prefix('"').and_then(|s| s.strip_suffix('"')))?;
        Some(inner.to_string())
    };
    let call = |s: &str, name: &str| -> Option<String> {
        let rest = s.strip_prefix(name)?.trim();
        quoted(rest.strip_prefix('(')?.strip_suffix(')')?)
    };
    let term = |s: &str| -> Option<Term> {
        let s = s.trim();
        if let Some((left, right)) = s.split_once("==") {
            return match left.trim() {
                "A.NetName" => Some(Term::Name(quoted(right)?)),
                "A.NetClass" => Some(Term::Class(quoted(right)?)),
                _ => None,
            };
        }
        call(s, "A.inDiffPair")
            .map(Term::Pair)
            .or_else(|| call(s, "A.hasNetclass").map(Term::Class))
    };
    text.split("||")
        .map(|any| any.split("&&").map(term).collect::<Option<Vec<Term>>>())
        .collect()
}

/// Whether KiCad's string comparison matches: as a wildcard, whatever the case.
fn matches(pattern: &str, value: &str) -> bool {
    wildcard(&pattern.to_lowercase(), &value.to_lowercase())
}

/// The base name of a differential pair a net is in, as `A.inDiffPair` matches it: the
/// name without its `P`, `N`, `+` or `-`, and what follows that.
fn pair_base(name: &str) -> Option<String> {
    pair_partner(name)?;
    let chars: Vec<char> = name.chars().collect();
    let i = chars
        .iter()
        .rposition(|&c| !(c.is_ascii_digit() || c == '_'))?;
    Some(chars[..i].iter().collect())
}

/// Reads the constraints for routing of a board's custom rules, the text of its `.kicad_dru`
/// file, which [`LengthRules::apply`] gives to a [`Design`].
///
/// A rule's `length` constraint, with its `min`, `opt` and `max`, applies to each net its
/// condition matches; its `skew` constraint, with its `max`, to all of them together, or
/// with `within_diff_pairs`, to the two nets of each differential pair among them. A
/// `track_width` constraint gives each net its `opt`, or its `min`, as the width of its
/// tracks; `diff_pair_gap` and `diff_pair_uncoupled` give each differential pair among
/// the nets a gap, its `opt` or `min`, and the longest it may run apart, its `max`. Where
/// several rules give a net one kind of constraint, the last in the file wins, as in KiCad.
///
/// Conditions are understood if they join, with `||` and `&&`, terms of these kinds, with
/// KiCad's wildcards and whatever the case: `A.NetName == '…'`, `A.NetClass == '…'`,
/// `A.hasNetclass('…')` and `A.inDiffPair('…')`, which matches the base name of a pair, as
/// `/USB_D` for `/USB_DP` and `/USB_DN`. A rule with any other condition is skipped, and
/// so are other constraints, each with a warning. The rules' own layers and severities
/// aren't read, nor are a `track_width`'s `max` and a `diff_pair_gap`'s `max`, which
/// KiCad's DRC checks.
///
/// # Errors
///
/// [`KicadError`] only if the text is not S-expressions. Rules it can't use are skipped
/// with a warning, as above.
///
/// ```
/// # let pcb = r#"(kicad_pcb (version 20240108) (generator test)
/// #   (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
/// #   (net 0 "") (net 1 "/USB_DP") (net 2 "/USB_DN"))"#;
/// use guide::kicad;
///
/// let board = kicad::read(pcb, None)?;
/// let rules = kicad::length_rules(
///     "(version 1)
///      (rule usb
///        (condition \"A.inDiffPair('/USB_D')\")
///        (constraint length (min 20mm) (max 30mm))
///        (constraint skew (max 0.1mm) (within_diff_pairs)))",
///     &board,
/// )?;
/// assert_eq!(rules.constraints.len(), 2);
/// assert_eq!(rules.constraints[0].min, Some(20_000_000));
/// assert_eq!(rules.constraints[1].max_skew, Some(100_000));
/// # Ok::<(), guide::kicad::KicadError>(())
/// ```
pub fn length_rules(text: &str, board: &KicadBoard) -> Result<LengthRules, KicadError> {
    // Comments are lines that start with `#`, which the reader doesn't know: blanked out,
    // which keeps offsets for errors.
    let mut clean = String::with_capacity(text.len() + 2);
    clean.push('(');
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with('#') {
            clean.extend(line.chars().map(|c| if c == '\n' { '\n' } else { ' ' }));
        } else {
            clean.push_str(line);
        }
    }
    clean.push_str("\n)");
    let root = crate::sexpr::parse(&clean)
        .map_err(|e| KicadError::syntax(text, e.offset.saturating_sub(1), e.message))?;
    let rules = &board.rules;
    let class_name = |net: NetId| -> &str {
        rules
            .class_of(net)
            .and_then(|c| rules.spec().classes.get(c.0 as usize))
            .map_or("Default", |c| c.name.as_str())
    };
    let pairs = &board.routing.pairs;
    let mut out = LengthRules::default();
    // Each rule's constraints, by kind: 0 for lengths, 1 for skews, 2 for widths, 3 for
    // pairs' gaps and 4 for their uncoupled lengths, these last three as a length's
    // `target`, `min` and `max`.
    let mut found: Vec<Vec<(usize, LengthConstraint, bool)>> = Vec::new();
    for rule in root.items().iter().filter(|r| r.is("rule")) {
        let name = rule.arg(0).unwrap_or("").to_string();
        let mut mine = Vec::new();
        let terms = match rule.child("condition").and_then(|c| c.arg(0)) {
            None => Some(vec![vec![]]),
            Some(text) => condition(text),
        };
        let Some(terms) = terms else {
            out.warnings.push(format!(
                "rule {name:?}: its condition isn't understood, so it is skipped"
            ));
            found.push(mine);
            continue;
        };
        let matched: Vec<NetId> = (0..board.nets.len())
            .map(|i| NetId(i as u32))
            .filter(|&n| !board.nets[n.index()].is_empty())
            .filter(|&n| {
                let name = &board.nets[n.index()];
                terms.iter().any(|all| {
                    all.iter().all(|t| match t {
                        Term::Name(p) => matches(p, name),
                        Term::Class(p) => matches(p, class_name(n)),
                        Term::Pair(p) => pair_base(name).is_some_and(|b| matches(p, &b)),
                    })
                })
            })
            .collect();
        for c in rule.children("constraint") {
            let value = |key: &str| c.child(key).and_then(|v| v.arg(0)).and_then(length);
            match c.arg(0) {
                Some("length") => mine.push((
                    0,
                    LengthConstraint {
                        nets: matched.clone(),
                        min: value("min"),
                        max: value("max"),
                        target: value("opt"),
                        max_skew: None,
                    },
                    false,
                )),
                Some("skew") => mine.push((
                    1,
                    LengthConstraint {
                        nets: matched.clone(),
                        max_skew: value("max"),
                        ..LengthConstraint::default()
                    },
                    c.items().iter().any(|i| i.is("within_diff_pairs")),
                )),
                Some(kind @ ("track_width" | "diff_pair_gap")) => {
                    if let Some(v) = value("opt").or_else(|| value("min")) {
                        mine.push((
                            if kind == "track_width" { 2 } else { 3 },
                            LengthConstraint {
                                nets: matched.clone(),
                                target: Some(v),
                                ..LengthConstraint::default()
                            },
                            false,
                        ));
                    }
                }
                Some("diff_pair_uncoupled") => {
                    if let Some(v) = value("max") {
                        mine.push((
                            4,
                            LengthConstraint {
                                nets: matched.clone(),
                                max: Some(v),
                                ..LengthConstraint::default()
                            },
                            false,
                        ));
                    }
                }
                Some(other) => out.warnings.push(format!(
                    "rule {name:?}: constraint {other:?} isn't read for routing"
                )),
                None => {}
            }
        }
        found.push(mine);
    }
    // The last rule to give a net a kind of constraint is the one that holds for it.
    let mut taken: [Vec<NetId>; 5] = Default::default();
    let mut constraints: Vec<Vec<LengthConstraint>> = vec![Vec::new(); found.len()];
    // By net, its width, and by pair, its gap and uncoupled length.
    let mut widths: Vec<Option<i64>> = vec![None; board.nets.len()];
    let mut gaps: Vec<[Option<i64>; 2]> = vec![[None; 2]; pairs.len()];
    for (r, mine) in found.into_iter().enumerate().rev() {
        let mut here = Vec::new();
        for (kind, mut c, within) in mine {
            c.nets.retain(|n| !taken[kind].contains(n));
            taken[kind].extend(c.nets.iter().copied());
            if c.nets.is_empty() {
                continue;
            }
            if kind == 2 {
                for n in &c.nets {
                    widths[n.index()] = c.target;
                }
                continue;
            }
            if kind >= 3 {
                for (k, p) in pairs.iter().enumerate() {
                    // A pair is given what is given to either of its nets.
                    if c.nets.contains(&p.positive) || c.nets.contains(&p.negative) {
                        let g = &mut gaps[k][kind - 3];
                        *g = g.or(if kind == 3 { c.target } else { c.max });
                    }
                }
                continue;
            }
            if within {
                for p in pairs {
                    if c.nets.contains(&p.positive) && c.nets.contains(&p.negative) {
                        here.push(LengthConstraint {
                            nets: vec![p.positive, p.negative],
                            ..c.clone()
                        });
                    }
                }
            } else {
                here.push(c);
            }
        }
        constraints[r] = here;
    }
    out.constraints = constraints.into_iter().flatten().collect();
    out.widths = widths
        .iter()
        .enumerate()
        .filter_map(|(i, w)| w.map(|w| (NetId(i as u32), w)))
        .collect();
    for (p, [gap, uncoupled]) in pairs.iter().zip(gaps) {
        if let Some(g) = gap {
            out.pair_gaps.push((p.positive, p.negative, g));
        }
        if let Some(u) = uncoupled {
            out.uncoupled.push((p.positive, p.negative, u));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_in_every_unit() {
        assert_eq!(length("1.5mm"), Some(1_500_000));
        assert_eq!(length("2"), Some(2_000_000));
        assert_eq!(length("10mil"), Some(254_000));
        assert_eq!(length("0.1in"), Some(2_540_000));
        assert_eq!(length("150um"), Some(150_000));
        assert_eq!(length("mm"), None);
    }

    #[test]
    fn conditions_that_are_understood() {
        assert_eq!(
            condition("A.NetName == '/CLK' || A.inDiffPair(\"/USB_D\")"),
            Some(vec![
                vec![Term::Name("/CLK".into())],
                vec![Term::Pair("/USB_D".into())]
            ])
        );
        assert_eq!(
            condition("A.NetClass == 'DDR' && A.hasNetclass('X')"),
            Some(vec![vec![
                Term::Class("DDR".into()),
                Term::Class("X".into())
            ]])
        );
        assert_eq!(condition("A.Type == 'Via'"), None);
        assert_eq!(condition("!A.inDiffPair('*')"), None);
    }

    #[test]
    fn later_rules_win_and_others_are_warned_of() {
        let pcb = r#"(kicad_pcb (version 20240108) (generator test)
            (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
            (net 0 "") (net 1 "/CH1_P") (net 2 "/CH1_N") (net 3 "/CH2_P") (net 4 "/CH2_N"))"#;
        let board = crate::kicad::read(pcb, None).unwrap();
        let dru = "# lengths\n(version 1)\n\
            (rule all (condition \"A.NetName == '/ch*'\") (constraint length (min 10mm)))\n\
            (rule ch1 (condition \"A.inDiffPair('/CH1_')\")\n\
              (constraint length (min 54.9mm) (opt 55mm) (max 55.1mm))\n\
              (constraint skew (max 0.1mm) (within_diff_pairs)))\n\
            (rule odd (condition \"A.Type == 'Via'\") (constraint length (max 1mm)))\n\
            (rule wide (constraint clearance (min 1mm)))\n";
        let rules = length_rules(dru, &board).unwrap();
        let n = |i: u32| NetId(i);
        assert_eq!(
            rules.constraints,
            [
                LengthConstraint {
                    nets: vec![n(3), n(4)],
                    min: Some(10_000_000),
                    ..LengthConstraint::default()
                },
                LengthConstraint {
                    nets: vec![n(1), n(2)],
                    min: Some(54_900_000),
                    max: Some(55_100_000),
                    target: Some(55_000_000),
                    max_skew: None,
                },
                LengthConstraint {
                    nets: vec![n(1), n(2)],
                    max_skew: Some(100_000),
                    ..LengthConstraint::default()
                },
            ]
        );
        assert_eq!(rules.warnings.len(), 2, "{:?}", rules.warnings);
        assert!(length_rules("(rule (", &board).is_err());
    }

    #[test]
    fn widths_gaps_and_uncoupled_lengths() {
        let pcb = r#"(kicad_pcb (version 20240108) (generator test)
            (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
            (net 0 "") (net 1 "CLK_P") (net 2 "CLK_N") (net 3 "DQ0") (net 4 "DQ1"))"#;
        let board = crate::kicad::read(pcb, None).unwrap();
        let dru = "(version 1)\n\
            (rule dq (condition \"A.NetName == 'DQ*'\")\n\
              (constraint track_width (min 0.1mm) (opt 0.12mm)))\n\
            (rule clk (condition \"A.inDiffPair('CLK_')\")\n\
              (constraint track_width (min 0.1016mm))\n\
              (constraint diff_pair_gap (min 0.2mm) (opt 0.25mm) (max 0.3mm))\n\
              (constraint diff_pair_uncoupled (max 2mm)))\n\
            (rule last (condition \"A.NetName == 'DQ1'\")\n\
              (constraint track_width (opt 0.15mm)))\n";
        let rules = length_rules(dru, &board).unwrap();
        let n = |i: u32| NetId(i);
        assert_eq!(
            rules.widths,
            [
                (n(1), 101_600),
                (n(2), 101_600),
                (n(3), 120_000),
                (n(4), 150_000)
            ]
        );
        assert_eq!(rules.pair_gaps, [(n(1), n(2), 250_000)]);
        assert_eq!(rules.uncoupled, [(n(1), n(2), 2_000_000)]);
        assert!(rules.warnings.is_empty(), "{:?}", rules.warnings);
        let mut design = Design::from_kicad(&board);
        rules.apply(&mut design);
        assert_eq!(design.nets[4].track_width, 150_000);
        assert_eq!(
            (design.pairs[0].width, design.pairs[0].gap),
            (101_600, 250_000)
        );
    }

    #[test]
    fn pair_base_names() {
        assert_eq!(pair_base("/USB_DP").as_deref(), Some("/USB_D"));
        assert_eq!(pair_base("D-").as_deref(), Some("D"));
        assert_eq!(pair_base("/CH1_N").as_deref(), Some("/CH1_"));
        assert_eq!(pair_base("GND"), None);
    }
}
