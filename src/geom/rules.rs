//! Design rules: widths, clearances, vias, net classes and the layer stack.

use std::collections::BTreeMap;
use std::fmt;

use super::types::MAX_COORD;
use crate::cost::Cost;
use crate::id::NetId;

/// The most copper layers a stack may have, as [`LayerSet`] is a 64-bit mask.
pub const MAX_LAYERS: usize = 64;

/// A copper layer, by its position in the stack, counted from the top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerId(pub u8);

/// A set of copper layers.
///
/// ```
/// use guide::geom::{LayerId, LayerSet};
///
/// let outer = LayerSet::EMPTY.with(LayerId(0)).with(LayerId(3));
/// assert!(outer.contains(LayerId(3)));
/// assert!(!outer.contains(LayerId(1)));
/// assert_eq!(outer.iter().collect::<Vec<_>>(), [LayerId(0), LayerId(3)]);
/// assert_eq!(LayerSet::span(LayerId(1), LayerId(2)), LayerSet::EMPTY.with(LayerId(1)).with(LayerId(2)));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerSet(pub u64);

impl LayerSet {
    /// No layers.
    pub const EMPTY: LayerSet = LayerSet(0);

    /// The layers from `from` to `to`, both included, in either order. Layers beyond
    /// [`MAX_LAYERS`] are left out.
    pub fn span(from: LayerId, to: LayerId) -> LayerSet {
        let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
        (lo.0..=hi.0).fold(LayerSet::EMPTY, |set, l| set.with(LayerId(l)))
    }

    /// The first `count` layers.
    pub fn first(count: usize) -> LayerSet {
        match count {
            0 => LayerSet::EMPTY,
            n if n >= MAX_LAYERS => LayerSet(u64::MAX),
            n => LayerSet((1u64 << n) - 1),
        }
    }

    /// This set with `layer` added. A layer beyond [`MAX_LAYERS`] is ignored.
    #[must_use]
    pub fn with(self, layer: LayerId) -> LayerSet {
        LayerSet(self.0 | 1u64.checked_shl(u32::from(layer.0)).unwrap_or(0))
    }

    /// Whether the set holds `layer`.
    pub fn contains(self, layer: LayerId) -> bool {
        1u64.checked_shl(u32::from(layer.0))
            .is_some_and(|bit| self.0 & bit != 0)
    }

    /// Whether every layer of this set is in `other`.
    pub fn is_subset(self, other: LayerSet) -> bool {
        self.0 & !other.0 == 0
    }

    /// The layers, from the top.
    pub fn iter(self) -> impl Iterator<Item = LayerId> {
        (0..MAX_LAYERS as u8)
            .filter(move |&l| self.contains(LayerId(l)))
            .map(LayerId)
    }
}

/// Values that override the board's defaults, field by field. `None` leaves a value to
/// the next rule in [order of precedence](Rules#precedence).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Overrides {
    /// The width tracks must have at least.
    pub track_width: Option<i64>,
    /// The gap copper of other nets must keep at least.
    pub clearance: Option<i64>,
}

/// A kind of via.
#[derive(Clone, Debug, PartialEq)]
pub struct ViaDef {
    /// The name, unique among vias.
    pub name: String,
    /// The finished hole's diameter.
    pub drill: i64,
    /// The width of copper around the hole, so that the pad's diameter is
    /// `drill + 2 * annular_ring`.
    pub annular_ring: i64,
    /// The top layer it connects.
    pub from: LayerId,
    /// The bottom layer it connects, below `from`.
    pub to: LayerId,
    /// What a router should charge for using one.
    pub cost: Cost,
}

impl ViaDef {
    /// The diameter of its copper.
    pub fn diameter(&self) -> i64 {
        // Saturating, as a spec that hasn't been validated may hold anything.
        self.drill
            .saturating_add(self.annular_ring.saturating_mul(2))
    }

    /// The layers it has copper on.
    pub fn layers(&self) -> LayerSet {
        LayerSet::span(self.from, self.to)
    }
}

/// Identifies a [`NetClass`] by its position in [`RuleSpec::classes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClassId(pub u32);

/// Identifies a [`ViaDef`] by its position in [`RuleSpec::vias`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ViaId(pub u32);

/// A group of nets with rules of their own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetClass {
    /// The name, unique among classes.
    pub name: String,
    /// The nets in the class. A net may be in one class at most.
    pub nets: Vec<NetId>,
    /// The class's values on every layer.
    pub overrides: Overrides,
    /// The class's values on particular layers, at most one entry for each layer.
    pub layers: Vec<(LayerId, Overrides)>,
    /// The via a router should use for these nets, in place of the board's.
    pub via: Option<ViaId>,
}

/// Everything [`Rules`] are made from, as plain data.
///
/// All distances are in database units. [`RuleSpec::new`] gives the minimal set, with no
/// vias, classes or overrides and no manufacturing limits.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleSpec {
    /// The names of the copper layers, from the top, unique and at most [`MAX_LAYERS`].
    pub layers: Vec<String>,
    /// The default width tracks must have at least.
    pub track_width: i64,
    /// The default gap between copper of different nets.
    pub clearance: i64,
    /// The board's values on particular layers, at most one entry for each layer.
    pub layer_overrides: Vec<(LayerId, Overrides)>,
    /// The kinds of via.
    pub vias: Vec<ViaDef>,
    /// The via a router should use for nets whose class names none.
    pub default_via: Option<ViaId>,
    /// The net classes.
    pub classes: Vec<NetClass>,
    /// The smallest drill a via may have. Zero for no limit.
    pub min_drill: i64,
    /// The narrowest annular ring a via may have. Zero for no limit.
    pub min_annular_ring: i64,
    /// The gap that drilled holes must keep from each other, whatever their nets. Zero for
    /// no limit.
    pub hole_to_hole: i64,
    /// The smallest clearance there is: every clearance between copper of two nets is at
    /// least this, whatever the classes and layers say, and so are the clearances that
    /// [pads and zones of their own](super::drc::Pad::clearance) set. Zero for none. This
    /// is KiCad's minimum clearance, from its board setup.
    pub min_clearance: i64,
    /// The gap that copper must keep from the holes of other nets, and from holes with no
    /// net, such as mounting holes. Zero for no limit.
    pub hole_clearance: i64,
    /// Clearances between particular pairs of nets, which replace whatever their classes
    /// and layers say, larger or smaller, though not [`min_clearance`](Self::min_clearance).
    /// Each pair at most once, in either order. KiCad uses this for the two nets of a
    /// differential pair, kept apart by their class's gap.
    pub pairs: Vec<(NetId, NetId, i64)>,
}

impl RuleSpec {
    /// Rules with these layers and defaults, and nothing else.
    pub fn new(layers: &[&str], track_width: i64, clearance: i64) -> RuleSpec {
        RuleSpec {
            layers: layers.iter().map(|s| s.to_string()).collect(),
            track_width,
            clearance,
            layer_overrides: Vec::new(),
            vias: Vec::new(),
            default_via: None,
            classes: Vec::new(),
            min_drill: 0,
            min_annular_ring: 0,
            hole_to_hole: 0,
            min_clearance: 0,
            hole_clearance: 0,
            pairs: Vec::new(),
        }
    }
}

/// Why a [`RuleSpec`] was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuleError {
    /// There are no layers, or more than [`MAX_LAYERS`].
    LayerCount(usize),
    /// Two layers, classes or vias have the same name.
    DuplicateName {
        /// What has the name: "layer", "class" or "via".
        what: &'static str,
        /// The name.
        name: String,
    },
    /// A value is out of range. Widths, drills and rings must be positive, and the rest at
    /// least zero, and all at most [`MAX_COORD`](super::MAX_COORD).
    InvalidValue {
        /// Where the value is.
        what: String,
        /// The value.
        value: i64,
    },
    /// A layer is not in the stack.
    UnknownLayer {
        /// Where it is mentioned.
        what: String,
        /// The layer.
        layer: LayerId,
    },
    /// The same layer has two sets of overrides in one place.
    DuplicateLayer {
        /// Where.
        what: String,
        /// The layer.
        layer: LayerId,
    },
    /// A via doesn't run downward from one layer to another.
    InvalidSpan {
        /// The via's name.
        via: String,
    },
    /// A via is mentioned that isn't defined.
    UnknownVia {
        /// Where it is mentioned.
        what: String,
        /// The via.
        via: ViaId,
    },
    /// A pair of nets has two clearances of its own, or is one net twice.
    InvalidPair {
        /// One net.
        a: NetId,
        /// The other.
        b: NetId,
    },
    /// A net is in two classes.
    NetInTwoClasses {
        /// The net.
        net: NetId,
        /// The first class.
        first: String,
        /// The second class.
        second: String,
    },
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuleError::LayerCount(n) => {
                write!(
                    f,
                    "a stack needs between 1 and {MAX_LAYERS} layers, not {n}"
                )
            }
            RuleError::DuplicateName { what, name } => write!(f, "two of {what} {name:?}"),
            RuleError::InvalidValue { what, value } => write!(f, "invalid {what}: {value}"),
            RuleError::UnknownLayer { what, layer } => {
                write!(
                    f,
                    "{what} names layer {}, which is not in the stack",
                    layer.0
                )
            }
            RuleError::DuplicateLayer { what, layer } => {
                write!(f, "{what} has two sets of overrides for layer {}", layer.0)
            }
            RuleError::InvalidSpan { via } => {
                write!(f, "via {via:?} must run down from one layer to another")
            }
            RuleError::UnknownVia { what, via } => {
                write!(f, "{what} names via {}, which is not defined", via.0)
            }
            RuleError::InvalidPair { a, b } => {
                write!(
                    f,
                    "nets {} and {} are not a valid pair, or are given twice",
                    a.0, b.0
                )
            }
            RuleError::NetInTwoClasses { net, first, second } => {
                write!(f, "net {} is in classes {first:?} and {second:?}", net.0)
            }
        }
    }
}

impl std::error::Error for RuleError {}

/// Validated design rules, and the answers to what applies where.
///
/// # Precedence
///
/// A width or clearance for a net on a layer is the first that is set of:
///
/// 1. the net's class, on that layer;
/// 2. the net's class;
/// 3. the board, on that layer;
/// 4. the board's default.
///
/// The more specific rule wins, and a class is more specific than a layer: a high-voltage
/// class keeps its clearance on every layer, however small inner layers' are. A net in no
/// class, or copper with no net, goes straight to 3. Between copper of two nets, the
/// clearance is the larger of the two nets', which is what KiCad does with net classes.
/// A clearance is never less than [`RuleSpec::min_clearance`].
///
/// ```
/// use guide::geom::{LayerId, NetClass, Overrides, RuleSpec, Rules};
/// use guide::id::NetId;
///
/// let mut spec = RuleSpec::new(&["F.Cu", "In1.Cu", "B.Cu"], 150_000, 150_000);
/// spec.layer_overrides.push((LayerId(1), Overrides { clearance: Some(100_000), ..Overrides::default() }));
/// spec.classes.push(NetClass {
///     name: "HV".into(),
///     nets: vec![NetId(7)],
///     overrides: Overrides { clearance: Some(500_000), ..Overrides::default() },
///     layers: vec![(LayerId(2), Overrides { clearance: Some(600_000), ..Overrides::default() })],
///     via: None,
/// });
/// let rules = Rules::new(spec)?;
///
/// let (hv, plain) = (Some(NetId(7)), Some(NetId(1)));
/// assert_eq!(rules.clearance(hv, LayerId(2)), 600_000); // class, on the layer
/// assert_eq!(rules.clearance(hv, LayerId(1)), 500_000); // class beats layer
/// assert_eq!(rules.clearance(plain, LayerId(1)), 100_000); // layer
/// assert_eq!(rules.clearance(plain, LayerId(0)), 150_000); // default
/// assert_eq!(rules.track_width(hv, LayerId(1)), 150_000); // nothing overrides widths
/// assert_eq!(rules.clearance_between(hv, plain, LayerId(1)), 500_000); // the larger
/// # Ok::<(), guide::geom::RuleError>(())
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Rules {
    spec: RuleSpec,
    class_of: BTreeMap<NetId, ClassId>,
    /// Overrides by layer, for the board and then each class, indexed by layer.
    board_layers: Vec<Overrides>,
    class_layers: Vec<Vec<Overrides>>,
    /// The clearances of pairs of nets, by the pair with the lesser net first.
    pairs: BTreeMap<(NetId, NetId), i64>,
    max_clearance: i64,
}

fn check_value(
    what: impl FnOnce() -> String,
    value: i64,
    positive: bool,
) -> Result<i64, RuleError> {
    let low = i64::from(positive);
    if (low..=MAX_COORD).contains(&value) {
        Ok(value)
    } else {
        Err(RuleError::InvalidValue {
            what: what(),
            value,
        })
    }
}

fn check_overrides(where_: &str, o: &Overrides) -> Result<(), RuleError> {
    if let Some(w) = o.track_width {
        check_value(|| format!("track width of {where_}"), w, true)?;
    }
    if let Some(c) = o.clearance {
        check_value(|| format!("clearance of {where_}"), c, false)?;
    }
    Ok(())
}

/// Checks overrides by layer, and lays them out by layer.
fn layer_table(
    where_: &str,
    layers: usize,
    entries: &[(LayerId, Overrides)],
) -> Result<Vec<Overrides>, RuleError> {
    let mut table = vec![None; layers];
    for (layer, o) in entries {
        let what = || format!("{where_} on layer {}", layer.0);
        let slot = table
            .get_mut(usize::from(layer.0))
            .ok_or_else(|| RuleError::UnknownLayer {
                what: where_.to_string(),
                layer: *layer,
            })?;
        if slot.is_some() {
            return Err(RuleError::DuplicateLayer {
                what: where_.to_string(),
                layer: *layer,
            });
        }
        check_overrides(&what(), o)?;
        *slot = Some(*o);
    }
    Ok(table.into_iter().map(Option::unwrap_or_default).collect())
}

fn check_unique<'a>(
    what: &'static str,
    names: impl Iterator<Item = &'a String>,
) -> Result<(), RuleError> {
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(RuleError::DuplicateName {
                what,
                name: name.clone(),
            });
        }
    }
    Ok(())
}

impl Rules {
    /// Validates a spec.
    ///
    /// # Errors
    ///
    /// [`RuleError`] for a spec with no layers or more than [`MAX_LAYERS`], or a value,
    /// name, net or layer that its variants describe as wrong.
    pub fn new(spec: RuleSpec) -> Result<Rules, RuleError> {
        let n = spec.layers.len();
        if n == 0 || n > MAX_LAYERS {
            return Err(RuleError::LayerCount(n));
        }
        check_unique("layer", spec.layers.iter())?;
        check_unique("class", spec.classes.iter().map(|c| &c.name))?;
        check_unique("via", spec.vias.iter().map(|v| &v.name))?;
        check_value(|| "default track width".into(), spec.track_width, true)?;
        check_value(|| "default clearance".into(), spec.clearance, false)?;
        check_value(|| "minimum drill".into(), spec.min_drill, false)?;
        check_value(
            || "minimum annular ring".into(),
            spec.min_annular_ring,
            false,
        )?;
        check_value(|| "hole-to-hole clearance".into(), spec.hole_to_hole, false)?;
        check_value(|| "minimum clearance".into(), spec.min_clearance, false)?;
        check_value(|| "hole clearance".into(), spec.hole_clearance, false)?;
        let board_layers = layer_table("the board", n, &spec.layer_overrides)?;

        for via in &spec.vias {
            let what = |field: &str| format!("{field} of via {:?}", via.name);
            check_value(|| what("drill"), via.drill, true)?;
            check_value(|| what("annular ring"), via.annular_ring, true)?;
            check_value(|| what("diameter"), via.diameter_checked(), true)?;
            if via.from >= via.to || usize::from(via.to.0) >= n {
                return Err(RuleError::InvalidSpan {
                    via: via.name.clone(),
                });
            }
        }
        let known_via = |what: String, via: Option<ViaId>| match via {
            Some(v) if v.0 as usize >= spec.vias.len() => {
                Err(RuleError::UnknownVia { what, via: v })
            }
            _ => Ok(()),
        };
        known_via("the board's default".into(), spec.default_via)?;

        let mut class_of = BTreeMap::new();
        let mut class_layers = Vec::with_capacity(spec.classes.len());
        for (i, class) in spec.classes.iter().enumerate() {
            let where_ = format!("class {:?}", class.name);
            check_overrides(&where_, &class.overrides)?;
            class_layers.push(layer_table(&where_, n, &class.layers)?);
            known_via(where_, class.via)?;
            for &net in &class.nets {
                if let Some(ClassId(first)) = class_of.insert(net, ClassId(i as u32))
                    && first as usize != i
                {
                    return Err(RuleError::NetInTwoClasses {
                        net,
                        first: spec.classes[first as usize].name.clone(),
                        second: class.name.clone(),
                    });
                }
            }
        }

        let mut pairs = BTreeMap::new();
        for &(a, b, clearance) in &spec.pairs {
            check_value(
                || format!("clearance of nets {} and {}", a.0, b.0),
                clearance,
                false,
            )?;
            if a == b || pairs.insert((a.min(b), a.max(b)), clearance).is_some() {
                return Err(RuleError::InvalidPair { a, b });
            }
        }
        let clearances = [spec.clearance, spec.min_clearance]
            .into_iter()
            .chain(spec.pairs.iter().map(|p| p.2))
            .chain(board_layers.iter().filter_map(|o| o.clearance))
            .chain(spec.classes.iter().filter_map(|c| c.overrides.clearance))
            .chain(class_layers.iter().flatten().filter_map(|o| o.clearance));
        let max_clearance = clearances.max().unwrap_or(0);
        Ok(Rules {
            spec,
            class_of,
            board_layers,
            class_layers,
            pairs,
            max_clearance,
        })
    }

    /// The spec these rules were made from.
    pub fn spec(&self) -> &RuleSpec {
        &self.spec
    }

    /// How many copper layers the stack has.
    pub fn layer_count(&self) -> usize {
        self.spec.layers.len()
    }

    /// Every layer of the stack.
    pub fn layers(&self) -> LayerSet {
        LayerSet::first(self.layer_count())
    }

    /// The layer with this name.
    pub fn layer(&self, name: &str) -> Option<LayerId> {
        let i = self.spec.layers.iter().position(|l| l == name)?;
        Some(LayerId(i as u8))
    }

    /// The class of a net, if it has one.
    pub fn class_of(&self, net: NetId) -> Option<ClassId> {
        self.class_of.get(&net).copied()
    }

    /// The first value set, in order of [precedence](Rules#precedence).
    fn resolve(
        &self,
        net: Option<NetId>,
        layer: LayerId,
        field: fn(&Overrides) -> Option<i64>,
        default: i64,
    ) -> i64 {
        let l = usize::from(layer.0);
        let class = net.and_then(|n| self.class_of(n)).map(|c| c.0 as usize);
        let candidates = [
            class
                .and_then(|c| self.class_layers[c].get(l))
                .and_then(field),
            class.and_then(|c| field(&self.spec.classes[c].overrides)),
            self.board_layers.get(l).and_then(field),
        ];
        candidates.into_iter().flatten().next().unwrap_or(default)
    }

    /// The width tracks of `net` must have at least on `layer`. `None` is copper with no
    /// net.
    pub fn track_width(&self, net: Option<NetId>, layer: LayerId) -> i64 {
        self.resolve(net, layer, |o| o.track_width, self.spec.track_width)
    }

    /// The gap copper of `net` must keep from other nets' on `layer`, never less than
    /// [`RuleSpec::min_clearance`].
    pub fn clearance(&self, net: Option<NetId>, layer: LayerId) -> i64 {
        self.resolve(net, layer, |o| o.clearance, self.spec.clearance)
            .max(self.spec.min_clearance)
    }

    /// The gap copper of two different nets must keep on `layer`: the pair's own, if
    /// [`RuleSpec::pairs`] gives it one, or else the larger of the two nets'.
    pub fn clearance_between(&self, a: Option<NetId>, b: Option<NetId>, layer: LayerId) -> i64 {
        if let (Some(a), Some(b)) = (a, b)
            && let Some(&own) = self.pairs.get(&(a.min(b), a.max(b)))
        {
            return own.max(self.spec.min_clearance);
        }
        self.clearance(a, layer).max(self.clearance(b, layer))
    }

    /// The clearance of a pair of nets of its own, if [`RuleSpec::pairs`] gives it one, not
    /// yet raised to [`RuleSpec::min_clearance`].
    pub(crate) fn pair_clearance(&self, a: NetId, b: NetId) -> Option<i64> {
        if self.pairs.is_empty() {
            return None;
        }
        self.pairs.get(&(a.min(b), a.max(b))).copied()
    }

    /// The largest clearance anything may need.
    pub fn max_clearance(&self) -> i64 {
        self.max_clearance
    }

    /// The via a router should use for `net`: its class's, or else the board's default.
    pub fn via(&self, net: Option<NetId>) -> Option<&ViaDef> {
        let class = net.and_then(|n| self.class_of(n));
        let id = class
            .and_then(|c| self.spec.classes[c.0 as usize].via)
            .or(self.spec.default_via)?;
        self.spec.vias.get(id.0 as usize)
    }
}

impl ViaDef {
    /// The diameter, or a negative number standing for one too large to compute.
    fn diameter_checked(&self) -> i64 {
        self.annular_ring
            .checked_mul(2)
            .and_then(|r| r.checked_add(self.drill))
            .unwrap_or(-1)
    }
}
