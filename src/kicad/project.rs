//! Design rules from a `.kicad_pro`, or from a board's own net classes.

use std::collections::BTreeMap;

use super::KicadError;
use super::json::{self, Json};
use super::units::mm_to_nm;

/// A net class: its name, its clearance and the gap within its differential pairs if it
/// sets them, and its priority, lower first, where KiCad 9 and later give one. The track
/// width and via sizes are what new routes of the class get, not limits, so they are only
/// for routing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClassDef {
    pub(crate) name: String,
    pub(crate) clearance: Option<i64>,
    pub(crate) pair_gap: Option<i64>,
    pub(crate) priority: Option<i64>,
    pub(crate) track_width: Option<i64>,
    pub(crate) via_diameter: Option<i64>,
    pub(crate) via_drill: Option<i64>,
}

/// The rules, as KiCad states them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectRules {
    pub(crate) min_clearance: i64,
    pub(crate) hole_clearance: i64,
    pub(crate) hole_to_hole: i64,
    pub(crate) min_track_width: i64,
    /// The clearance between copper and the board's edge.
    pub(crate) edge_clearance: i64,
    /// The classes, `Default` among them, in file order.
    pub(crate) classes: Vec<ClassDef>,
    /// Nets assigned to classes by name, in file order.
    pub(crate) assignments: Vec<(String, Vec<String>)>,
    /// Patterns, each with its class, in file order.
    pub(crate) patterns: Vec<(String, String)>,
    /// Checks that the project turns off, by KiCad's name for them.
    pub(crate) ignored: Vec<String>,
}

/// KiCad's own defaults, for a board with no project.
pub(crate) fn defaults() -> ProjectRules {
    ProjectRules {
        min_clearance: 0,
        hole_clearance: 0,
        hole_to_hole: 250_000,
        min_track_width: 0,
        edge_clearance: 0,
        classes: vec![ClassDef {
            name: "Default".into(),
            clearance: Some(200_000),
            pair_gap: Some(250_000),
            priority: None,
            track_width: Some(200_000),
            via_diameter: Some(600_000),
            via_drill: Some(300_000),
        }],
        assignments: Vec::new(),
        patterns: Vec::new(),
        ignored: Vec::new(),
    }
}

fn length(v: Option<&Json>) -> Option<i64> {
    v.and_then(Json::as_number).and_then(mm_to_nm)
}

/// Reads the rules of a `.kicad_pro`.
pub(crate) fn parse(text: &str) -> Result<ProjectRules, KicadError> {
    let root = json::parse(text)?;
    let mut rules = defaults();
    let board_rules = root.at(&["board", "design_settings", "rules"]);
    let get = |key: &str| board_rules.and_then(|r| length(r.get(key)));
    rules.min_clearance = get("min_clearance").unwrap_or(rules.min_clearance);
    rules.hole_clearance = get("min_hole_clearance").unwrap_or(rules.hole_clearance);
    rules.hole_to_hole = get("min_hole_to_hole").unwrap_or(rules.hole_to_hole);
    rules.min_track_width = get("min_track_width").unwrap_or(rules.min_track_width);
    rules.edge_clearance = get("min_copper_edge_clearance").unwrap_or(rules.edge_clearance);
    if let Some(severities) = root.at(&["board", "design_settings", "rule_severities"]) {
        for (check, severity) in severities.as_object() {
            if severity.as_str() == Some("ignore") {
                rules.ignored.push(check.clone());
            }
        }
    }

    let settings = root.get("net_settings");
    let classes = settings.and_then(|s| s.get("classes"));
    if let Some(classes) = classes {
        rules.classes.clear();
        for class in classes.as_array() {
            let Some(name) = class.get("name").and_then(Json::as_str) else {
                continue;
            };
            rules.classes.push(ClassDef {
                name: name.to_string(),
                clearance: length(class.get("clearance")),
                pair_gap: length(class.get("diff_pair_gap")),
                priority: class
                    .get("priority")
                    .and_then(Json::as_number)
                    .and_then(|p| p.parse().ok()),
                track_width: length(class.get("track_width")),
                via_diameter: length(class.get("via_diameter")),
                via_drill: length(class.get("via_drill")),
            });
            // KiCad 6 lists a class's nets in the class.
            for net in class.get("nets").map_or(&[][..], Json::as_array) {
                if let Some(net) = net.as_str() {
                    rules
                        .assignments
                        .push((net.to_string(), vec![name.to_string()]));
                }
            }
        }
        if !rules.classes.iter().any(|c| c.name == "Default") {
            rules.classes.insert(0, defaults().classes.remove(0));
        }
    }
    if let Some(assigned) = settings.and_then(|s| s.get("netclass_assignments")) {
        for (net, class) in assigned.as_object() {
            // A class's name, or from KiCad 9, a list of them.
            let names: Vec<String> = match class {
                Json::String(name) => vec![name.clone()],
                other => other
                    .as_array()
                    .iter()
                    .filter_map(|c| c.as_str().map(str::to_string))
                    .collect(),
            };
            rules.assignments.push((net.clone(), names));
        }
    }
    if let Some(patterns) = settings.and_then(|s| s.get("netclass_patterns")) {
        for p in patterns.as_array() {
            if let (Some(pattern), Some(class)) = (
                p.get("pattern").and_then(Json::as_str),
                p.get("netclass").and_then(Json::as_str),
            ) {
                rules
                    .patterns
                    .push((pattern.to_string(), class.to_string()));
            }
        }
    }
    Ok(rules)
}

/// Whether `name` matches a KiCad wildcard pattern as a whole: `*` is any run of
/// characters, and `?` any one.
pub(crate) fn wildcard(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    // The classic greedy match with one backtracking point.
    let (mut i, mut j) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while j < n.len() {
        if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some((i, j));
            i += 1;
        } else if let Some((si, sj)) = star {
            i = si + 1;
            j = sj + 1;
            star = Some((si, sj + 1));
        } else {
            return false;
        }
    }
    p[i..].iter().all(|&c| c == '*')
}

/// The other net of a differential pair, as KiCad finds it: past any digits and
/// underscores at the end of the name, a `+` pairs with `-`, and `P` with `N`.
pub(crate) fn pair_partner(net: &str) -> Option<String> {
    let chars: Vec<char> = net.chars().collect();
    let i = chars
        .iter()
        .rposition(|&c| !(c.is_ascii_digit() || c == '_'))?;
    let other = match chars[i] {
        '+' => '-',
        '-' => '+',
        'P' => 'N',
        'N' => 'P',
        _ => return None,
    };
    let mut partner = chars;
    partner[i] = other;
    Some(partner.into_iter().collect())
}

/// Whether a net class pattern matches a net's name: as the name itself, or as a wildcard.
fn matches(pattern: &str, net: &str) -> bool {
    pattern == net || wildcard(pattern, net)
}

/// Whether a pattern uses what only a regular expression would.
pub(crate) fn looks_like_regex(pattern: &str) -> bool {
    pattern.contains(['[', ']', '(', ')', '|', '+', '^', '$', '\\', '{', '}'])
        || pattern.contains(".*")
}

impl ProjectRules {
    /// The class a net is in, if it isn't `Default`, with its clearance. KiCad 8 and
    /// earlier take the first class that an assignment or a pattern gives a net. KiCad 9
    /// gives classes priorities and combines every class that applies: of those that set a
    /// clearance, the one first by priority sets the net's.
    pub(crate) fn class_of(&self, net: &str) -> Option<usize> {
        let index: BTreeMap<&str, usize> = self
            .classes
            .iter()
            .enumerate()
            .map(|(i, c)| (c.name.as_str(), i))
            .collect();
        let mut matched: Vec<usize> = Vec::new();
        for (name, classes) in &self.assignments {
            if name == net {
                matched.extend(
                    classes
                        .iter()
                        .filter_map(|c| index.get(c.as_str()).copied()),
                );
            }
        }
        for (pattern, class) in &self.patterns {
            if matches(pattern, net) {
                matched.extend(index.get(class.as_str()).copied());
            }
        }
        let prioritised = self.classes.iter().any(|c| c.priority.is_some());
        let chosen = if prioritised {
            matched
                .iter()
                .copied()
                .filter(|&c| self.classes[c].clearance.is_some())
                .min_by_key(|&c| (self.classes[c].priority.unwrap_or(i64::MAX), c))
        } else {
            matched.first().copied()
        };
        chosen.filter(|&c| self.classes[c].name != "Default")
    }

    /// The patterns that look like regular expressions, and match none of `nets` as
    /// wildcards: KiCad would try them as regular expressions too, which this doesn't.
    pub(crate) fn unmatched_regexes<'s>(&'s self, nets: &[String]) -> Vec<&'s str> {
        self.patterns
            .iter()
            .map(|(p, _)| p.as_str())
            .filter(|p| looks_like_regex(p) && !nets.iter().any(|n| matches(p, n)))
            .collect()
    }

    /// The gap a class keeps within its differential pairs, if it sets one.
    pub(crate) fn pair_gap(&self, class: Option<usize>) -> Option<i64> {
        match class {
            Some(c) => self.classes[c].pair_gap,
            None => self.classes.iter().find(|c| c.name == "Default")?.pair_gap,
        }
    }

    /// The class whose track width and via sizes new routes of a net take, `Default`
    /// included, by the same choice as [`class_of`](ProjectRules::class_of) but among the
    /// classes that set a track width.
    pub(crate) fn routing_class(&self, net: &str) -> Option<usize> {
        let default = self.classes.iter().position(|c| c.name == "Default");
        let mut matched: Vec<usize> = Vec::new();
        for (name, classes) in &self.assignments {
            if name == net {
                matched.extend(
                    classes
                        .iter()
                        .filter_map(|c| self.classes.iter().position(|d| &d.name == c)),
                );
            }
        }
        for (pattern, class) in &self.patterns {
            if matches(pattern, net) {
                matched.extend(self.classes.iter().position(|d| &d.name == class));
            }
        }
        let prioritised = self.classes.iter().any(|c| c.priority.is_some());
        let chosen = if prioritised {
            matched
                .iter()
                .copied()
                .filter(|&c| self.classes[c].track_width.is_some())
                .min_by_key(|&c| (self.classes[c].priority.unwrap_or(i64::MAX), c))
        } else {
            matched.first().copied()
        };
        chosen.or(default)
    }

    /// The `Default` class's clearance.
    pub(crate) fn default_clearance(&self) -> i64 {
        self.classes
            .iter()
            .find(|c| c.name == "Default")
            .and_then(|c| c.clearance)
            .unwrap_or(200_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_partners() {
        assert_eq!(pair_partner("D+").as_deref(), Some("D-"));
        assert_eq!(pair_partner("/USB_DN").as_deref(), Some("/USB_DP"));
        assert_eq!(pair_partner("CLK_N_1").as_deref(), Some("CLK_P_1"));
        assert_eq!(pair_partner("GND"), None);
        assert_eq!(pair_partner("12_"), None);
    }

    #[test]
    fn wildcards() {
        for (p, n, m) in [
            ("GND", "GND", true),
            ("GND", "GND2", false),
            ("/VCC*", "/VCC_3V3", true),
            ("*", "", true),
            ("a?c", "abc", true),
            ("a?c", "ac", false),
            ("*b*c", "abxbc", true),
            ("*b*c", "abxbd", false),
        ] {
            assert_eq!(wildcard(p, n), m, "{p} {n}");
        }
    }

    #[test]
    fn reads_classes_of_each_version() {
        let v6 = r#"{"board": {"design_settings": {"rules": {"min_clearance": 0.1, "min_hole_clearance": 0.25}}},
            "net_settings": {"classes": [{"name": "Default", "clearance": 0.2},
                                         {"name": "HV", "clearance": 0.5, "nets": ["/HV"]}]}}"#;
        let r = parse(v6).unwrap();
        assert_eq!(
            (r.min_clearance, r.hole_clearance, r.hole_to_hole),
            (100_000, 250_000, 250_000)
        );
        assert_eq!(r.class_of("/HV"), Some(1));
        assert_eq!(r.class_of("/LV"), None);

        let v9 = r#"{"net_settings": {"classes": [
                {"name": "Default", "clearance": 0.2, "priority": 2147483647},
                {"name": "A", "clearance": 0.3, "priority": 1},
                {"name": "B", "clearance": null, "priority": 0}],
            "netclass_assignments": {"x": ["B", "A"]},
            "netclass_patterns": [{"netclass": "B", "pattern": "y*"}, {"netclass": "A", "pattern": "[z]"}]}}"#;
        let r = parse(v9).unwrap();
        assert_eq!(r.class_of("x"), Some(1));
        assert_eq!(r.class_of("y1"), None);
        assert_eq!(r.class_of("z"), None);
        assert_eq!(r.unmatched_regexes(&["z".into(), "y".into()]), ["[z]"]);
    }
}
