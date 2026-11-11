//! Writing a Specctra session, a `.ses` file, from tracks and vias.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::{Component, DsnError, Resolution, Side, WireType};
use crate::geom::drc::{Track, Via};
use crate::geom::{DEFAULT_ARC_TOLERANCE, LayerId, Point, Shape};

/// What a session holds: the placement, if any, and the routes.
///
/// Everything is borrowed: the layers and nets name the model's [`LayerId`]s and
/// [`NetId`](crate::id::NetId)s, as `DsnBoard::rules` and `DsnBoard::nets`, or a
/// `KicadBoard`'s, do.
#[derive(Clone, Debug)]
pub struct Session<'a> {
    /// The session's name.
    pub name: &'a str,
    /// The design it is for, usually the name of the `.dsn` file.
    pub base_design: &'a str,
    /// The copper layers' names, by [`LayerId`], as the design names them: KiCad's import
    /// finds layers by the names it exported, which are the board's own, such as `Front`,
    /// where `kicad::read` gives KiCad's canonical `F.Cu`.
    pub layers: &'a [String],
    /// The nets' names, by [`NetId`](crate::id::NetId).
    pub nets: &'a [String],
    /// Where the components are. Empty for no placement, which leaves them where they are:
    /// KiCad moves every component a session places.
    pub components: &'a [Component],
    /// The tracks. Arcs become polylines within [`arc_tolerance`](Self::arc_tolerance),
    /// as sessions have no arcs that KiCad reads.
    pub tracks: &'a [Track],
    /// The vias.
    pub vias: &'a [Via],
    /// The tracks' types, by position. Missing ones are [`WireType::Normal`].
    pub track_types: &'a [WireType],
    /// The vias' types, by position.
    pub via_types: &'a [WireType],
    /// The resolution to write in. Coordinates are written in its steps, rounded to the
    /// nearest with halves away from zero, which is exact for [`Resolution::NANOMETRE`],
    /// the default.
    pub resolution: Resolution,
    /// How far an arc's polyline may be from the arc, in nanometres: see
    /// [`Shape::arc_track`]. [`DEFAULT_ARC_TOLERANCE`] by default.
    pub arc_tolerance: i64,
}

impl<'a> Session<'a> {
    /// A session with these routes, no placement and no types, in nanometres.
    pub fn new(
        base_design: &'a str,
        layers: &'a [String],
        nets: &'a [String],
        tracks: &'a [Track],
        vias: &'a [Via],
    ) -> Session<'a> {
        Session {
            name: base_design,
            base_design,
            layers,
            nets,
            components: &[],
            tracks,
            vias,
            track_types: &[],
            via_types: &[],
            resolution: Resolution::NANOMETRE,
            arc_tolerance: DEFAULT_ARC_TOLERANCE,
        }
    }
}

/// Writes a session: the placement, if there is one, then the routes, net by net, with a
/// padstack for each kind of via.
///
/// A via's padstack is named as KiCad names them, `Via[0-1]_800:400_um` for a via from
/// layer 0 to layer 1 of 800 µm with a 400 µm drill, since that name is where KiCad's
/// session import finds the drill. It has a circle on every layer the via spans. KiCad
/// reads the drill from the name in micrometres through a `double`, and truncates it to
/// nanometres, which is exact for any drill of a whole number of nanometres that `f64`
/// holds closely enough, as every real one does.
///
/// Tracks and vias with no net, or with the net named `""`, are written in the net `""`.
///
/// # Errors
///
/// [`DsnError`] if a name holds a `"`, which a session can't quote, if an item names a layer
/// or net that isn't given, or if an arc can't be approximated.
///
/// ```
/// use guide::dsn::{Session, write_session};
/// use guide::geom::drc::{Track, Via};
/// use guide::geom::{LayerId, Point};
/// use guide::id::NetId;
///
/// let layers = ["F.Cu".to_string(), "B.Cu".to_string()];
/// let nets = [String::new(), "GND".to_string()];
/// let tracks = [Track {
///     start: Point::new(0, 0),
///     end: Point::new(1_000_000, 0),
///     mid: None,
///     width: 250_000,
///     layer: LayerId(0),
///     net: Some(NetId(1)),
/// }];
/// let vias = [Via {
///     at: Point::new(1_000_000, 0),
///     drill: 300_000,
///     diameter: 600_000,
///     from: LayerId(0),
///     to: LayerId(1),
///     net: Some(NetId(1)),
/// }];
/// let ses = write_session(&Session::new("demo.dsn", &layers, &nets, &tracks, &vias))?;
/// assert!(ses.contains(r#"(wire (path "F.Cu" 250000 0 0 1000000 0))"#));
/// assert!(ses.contains(r#"(via "Via[0-1]_600:300_um" 1000000 0)"#));
/// # Ok::<(), guide::dsn::DsnError>(())
/// ```
pub fn write_session(s: &Session<'_>) -> Result<String, DsnError> {
    let w = Writer {
        resolution: s.resolution,
    };
    let mut out = String::new();
    let _ = writeln!(out, "(session {}", quote(s.name)?);
    let _ = writeln!(out, "  (base_design {})", quote(s.base_design)?);
    if !s.components.is_empty() {
        out.push_str("  (placement\n");
        let _ = writeln!(out, "    {}", w.resolution_text());
        // Components by image, each image where it first comes.
        let mut images: Vec<(&str, Vec<&Component>)> = Vec::new();
        for c in s.components {
            match images.iter_mut().find(|(name, _)| *name == c.image) {
                Some((_, list)) => list.push(c),
                None => images.push((&c.image, vec![c])),
            }
        }
        for (image, list) in images {
            let _ = writeln!(out, "    (component {}", quote(image)?);
            for c in list {
                let side = match c.side {
                    Side::Front => "front",
                    Side::Back => "back",
                };
                let _ = writeln!(
                    out,
                    "      (place {} {} {} {side} {})",
                    quote(&c.reference)?,
                    w.coord(c.at.x),
                    w.coord(c.at.y),
                    millidegrees(c.rotation),
                );
            }
            out.push_str("    )\n");
        }
        out.push_str("  )\n");
    }
    out.push_str("  (was_is\n  )\n");
    out.push_str("  (routes\n");
    let _ = writeln!(out, "    {}", w.resolution_text());
    out.push_str("    (parser\n");
    out.push_str("      (host_cad \"guide\")\n");
    let _ = writeln!(
        out,
        "      (host_version \"{}\")",
        env!("CARGO_PKG_VERSION")
    );
    out.push_str("    )\n");

    // The vias' padstacks, by span, diameter and drill, in the order they first come.
    let layer = |l: LayerId| -> Result<&str, DsnError> {
        s.layers
            .get(usize::from(l.0))
            .map(String::as_str)
            .ok_or_else(|| DsnError::other(format!("no layer {} is given", l.0)))
    };
    let mut padstacks: Vec<(String, &Via)> = Vec::new();
    let mut via_names = Vec::with_capacity(s.vias.len());
    for via in s.vias {
        if via.from > via.to || via.drill <= 0 || via.diameter <= 0 {
            return Err(DsnError::other(
                "a via must have a drill, a diameter and a span",
            ));
        }
        let name = format!(
            "Via[{}-{}]_{}:{}_um",
            via.from.0,
            via.to.0,
            micrometres(via.diameter),
            micrometres(via.drill)
        );
        if !padstacks.iter().any(|(n, _)| *n == name) {
            padstacks.push((name.clone(), via));
        }
        via_names.push(name);
    }
    // KiCad refuses a session without a library, even an empty one.
    out.push_str("    (library_out\n");
    for (name, via) in &padstacks {
        let _ = writeln!(out, "      (padstack {}", quote(name)?);
        for l in via.from.0..=via.to.0 {
            let _ = writeln!(
                out,
                "        (shape (circle {} {} 0 0))",
                quote(layer(LayerId(l))?)?,
                w.coord(via.diameter)
            );
        }
        out.push_str("        (attach off)\n      )\n");
    }
    out.push_str("    )\n");

    // The routes, by net, in the order of the nets.
    let net_of = |net: Option<crate::id::NetId>| -> Result<usize, DsnError> {
        let n = net.map_or(0, |n| n.index());
        if n == 0 && s.nets.is_empty() {
            return Ok(0);
        }
        if n < s.nets.len() {
            Ok(n)
        } else {
            Err(DsnError::other(format!("no net {n} is given")))
        }
    };
    let mut by_net: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (i, t) in s.tracks.iter().enumerate() {
        let points = w.centerline(t, s.arc_tolerance)?;
        let mut line = format!(
            "(wire (path {} {}",
            quote(layer(t.layer)?)?,
            w.coord(t.width)
        );
        for p in points {
            let _ = write!(line, " {} {}", w.coord(p.x), w.coord(p.y));
        }
        line.push(')');
        if let Some(kind) = s.track_types.get(i).and_then(|k| k.name()) {
            let _ = write!(line, " (type {kind})");
        }
        line.push(')');
        by_net.entry(net_of(t.net)?).or_default().push(line);
    }
    for (i, (v, name)) in s.vias.iter().zip(&via_names).enumerate() {
        let mut line = format!(
            "(via {} {} {}",
            quote(name)?,
            w.coord(v.at.x),
            w.coord(v.at.y)
        );
        if let Some(kind) = s.via_types.get(i).and_then(|k| k.name()) {
            let _ = write!(line, " (type {kind})");
        }
        line.push(')');
        by_net.entry(net_of(v.net)?).or_default().push(line);
    }
    out.push_str("    (network_out\n");
    for (net, lines) in by_net {
        let name = s.nets.get(net).map_or("", String::as_str);
        let _ = writeln!(out, "      (net {}", quote(name)?);
        for line in lines {
            let _ = writeln!(out, "        {line}");
        }
        out.push_str("      )\n");
    }
    out.push_str("    )\n  )\n)\n");
    Ok(out)
}

struct Writer {
    resolution: Resolution,
}

impl Writer {
    fn resolution_text(&self) -> String {
        format!(
            "(resolution {} {})",
            self.resolution.unit.name(),
            self.resolution.steps
        )
    }

    /// Nanometres in steps of the resolution, rounded half away from zero.
    fn coord(&self, nm: i64) -> i64 {
        let num = i128::from(nm) * i128::from(self.resolution.steps.max(1));
        let den = i128::from(self.resolution.unit.nanometres());
        let (q, r) = (num / den, num % den);
        (q + i128::from(2 * r.abs() >= den) * num.signum()) as i64
    }

    /// A track's centerline from its start to its end: two points, or an arc's polyline.
    fn centerline(&self, t: &Track, tolerance: i64) -> Result<Vec<Point>, DsnError> {
        let Some(mid) = t.mid else {
            return Ok(vec![t.start, t.end]);
        };
        let shape = Shape::arc_track(t.start, mid, t.end, t.width, tolerance)
            .map_err(|e| DsnError::other(format!("an arc can't be written: {e}")))?;
        let mut points = shape.centerline().unwrap_or_else(|| vec![t.start, t.end]);
        let (first, last) = (points[0], points[points.len() - 1]);
        let d = |a: Point, b: Point| (a.x - b.x).unsigned_abs() + (a.y - b.y).unsigned_abs();
        if d(last, t.start) < d(first, t.start) {
            points.reverse();
        }
        // The ends are the track's own, whatever rounding did.
        let n = points.len();
        points[0] = t.start;
        points[n - 1] = t.end;
        Ok(points)
    }
}

/// A name in quotes. Sessions have no escapes, so a name can't hold a quote.
fn quote(name: &str) -> Result<String, DsnError> {
    if name.contains('"') {
        return Err(DsnError::other(format!(
            "the name {name:?} holds a quote, which a session can't hold"
        )));
    }
    Ok(format!("\"{name}\""))
}

/// Nanometres in micrometres, exactly, as decimal text.
fn micrometres(nm: i64) -> String {
    thousandths(nm)
}

/// Millidegrees in degrees, exactly, as decimal text.
fn millidegrees(m: i32) -> String {
    thousandths(i64::from(m))
}

/// `v / 1000` as exact decimal text with no trailing zeros.
fn thousandths(v: i64) -> String {
    let sign = if v < 0 { "-" } else { "" };
    let (whole, frac) = (v.unsigned_abs() / 1000, v.unsigned_abs() % 1000);
    if frac == 0 {
        return format!("{sign}{whole}");
    }
    let text = format!("{frac:03}");
    format!("{sign}{whole}.{}", text.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_are_exact() {
        assert_eq!(micrometres(800_000), "800");
        assert_eq!(micrometres(317_500), "317.5");
        assert_eq!(micrometres(1), "0.001");
        assert_eq!(millidegrees(-12_345), "-12.345");
        let w = Writer {
            resolution: Resolution {
                unit: super::super::Unit::Um,
                steps: 10,
            },
        };
        assert_eq!(w.coord(150), 2);
        assert_eq!(w.coord(-150), -2);
        assert_eq!(w.coord(149), 1);
    }
}
