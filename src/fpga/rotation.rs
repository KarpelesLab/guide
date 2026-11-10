//! Rotating the inputs of LUTs for timing: pin permutation inside clusters.
//!
//! A LUT's inputs are logically equivalent, since its truth table can be permuted with them,
//! but they are not equally fast: VTR's `k6_N10_40nm.xml` gives the six inputs of its LUT
//! delays from 82 ps to 398 ps. Where the inputs come from a `complete` crossbar, as a
//! cluster's inputs do in VTR's architectures, any signal can reach any input, so moving the
//! latest signals to the fastest inputs changes nothing outside the cluster: not the routing
//! between clusters, which reaches the cluster's equivalent inputs, nor what VPR checks.
//!
//! VPR's packed netlist records which input of the truth table each physical input carries,
//! in a `port_rotation_map`, and VPR's timing analysis reads it. VPR's packer chooses the
//! inputs for routability inside the cluster, not timing, and its router doesn't change them.
//!
//! [`rotatable_luts`] finds the LUTs whose inputs can be permuted, [`rotate_luts_for_timing`]
//! chooses a permutation for each from the arrival times of a timing analysis, and
//! [`write_rotated_netlist`] writes the packed netlist with them for VPR.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use super::ImportError;
use super::arch::Architecture;
use super::netlist::{InternalPin, PackedNetlist};
use super::timing::{DesignTiming, LogicDelays};
use super::xml::{Event, XmlReader};
use crate::timing::{ArcId, Constraints, PointId, TimingAnalysis};

/// A LUT whose inputs [`rotatable_luts`] found can be permuted.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RotatableLut {
    /// The type of the primitive, as in `lut`.
    pub pb_type: String,
    /// Its input port, which has a `port_rotation_map`.
    pub port: String,
    /// The pins of the port.
    pub width: u32,
}

/// For each LUT, by the name of the primitive, which physical input each one goes to.
///
/// Made by [`rotate_luts_for_timing`], and written by [`write_rotated_netlist`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LutRotation {
    /// By primitive name: for each physical input, the one it moves to.
    moves: BTreeMap<String, Vec<u32>>,
}

impl LutRotation {
    /// How many LUTs are rotated.
    pub fn len(&self) -> usize {
        self.moves.len()
    }

    /// Whether no LUT is rotated.
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    /// The physical input that each physical input of a LUT moves to, or `None` if it stays.
    pub fn permutation(&self, primitive: &str) -> Option<&[u32]> {
        self.moves.get(primitive).map(Vec::as_slice)
    }

    /// Rotates a LUT's inputs: physical input `i` moves to `permutation[i]`, which has to be a
    /// permutation of the inputs. An identity takes the LUT out.
    pub fn set(&mut self, primitive: &str, permutation: Vec<u32>) {
        if permutation.iter().enumerate().all(|(i, &p)| p == i as u32) {
            self.moves.remove(primitive);
        } else {
            self.moves.insert(primitive.to_owned(), permutation);
        }
    }
}

/// An element of a `.net` file, held whole.
#[derive(Clone, Debug, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The type of a block, its `instance` up to the `[`.
    fn pb_type(&self) -> &str {
        let instance = self.attr("instance").unwrap_or_default();
        instance.split('[').next().unwrap_or_default()
    }

    fn is_block(&self) -> bool {
        self.name == "block" && self.attr("name").is_some_and(|n| n != "open")
    }

    /// The `port` of this name in the block's `inputs`, as an index in `inputs`' children.
    fn input_port(&self, port: &str) -> Option<(usize, usize)> {
        let inputs = self.children.iter().position(|c| c.name == "inputs")?;
        let index = self.children[inputs]
            .children
            .iter()
            .position(|c| c.name == "port" && c.attr("name") == Some(port))?;
        Some((inputs, index))
    }

    fn input_tokens(&self, port: &str) -> Option<Vec<&str>> {
        let (inputs, index) = self.input_port(port)?;
        Some(
            self.children[inputs].children[index]
                .text
                .split_whitespace()
                .collect(),
        )
    }

    fn set_input_text(&mut self, port: &str, text: String) {
        if let Some((inputs, index)) = self.input_port(port) {
            self.children[inputs].children[index].text = text;
        }
    }

    /// The `port_rotation_map`s of the block, by port.
    fn rotation_maps(&self) -> Vec<(String, Vec<String>)> {
        let Some(inputs) = self.children.iter().find(|c| c.name == "inputs") else {
            return Vec::new();
        };
        inputs
            .children
            .iter()
            .filter(|c| c.name == "port_rotation_map")
            .filter_map(|c| {
                let name = c.attr("name")?.to_owned();
                Some((name, c.text.split_whitespace().map(str::to_owned).collect()))
            })
            .collect()
    }

    fn set_rotation_map(&mut self, port: &str, text: String) {
        if let Some(inputs) = self.children.iter_mut().find(|c| c.name == "inputs")
            && let Some(map) = inputs
                .children
                .iter_mut()
                .find(|c| c.name == "port_rotation_map" && c.attr("name") == Some(port))
        {
            map.text = text;
        }
    }

    /// Whether any port of this block mentions `prefix`, as in `ble6.in[`.
    fn mentions(&self, prefix: &str) -> bool {
        self.children.iter().any(|group| {
            matches!(group.name.as_str(), "inputs" | "outputs" | "clocks")
                && group
                    .children
                    .iter()
                    .any(|port| port.name == "port" && port.text.contains(prefix))
        })
    }

    fn at(&self, path: &[usize]) -> &Node {
        path.iter().fold(self, |node, &i| &node.children[i])
    }

    fn at_mut(&mut self, path: &[usize]) -> &mut Node {
        path.iter().fold(self, |node, &i| &mut node.children[i])
    }
}

/// A source of an input in a `.net` file, `from->interconnect`, split into the block type,
/// the port and the pin of `from`, and the interconnect.
fn split_source(token: &str) -> Option<(&str, &str, u32, &str)> {
    let (from, interconnect) = token.split_once("->")?;
    let (block, pin) = from.split_once('.')?;
    let block = block.split('[').next()?;
    let (port, rest) = pin.split_once('[')?;
    let bit = rest.strip_suffix(']')?.parse().ok()?;
    Some((block, port, bit, interconnect))
}

/// The chain of blocks, from a LUT up to the block whose inputs come from a `complete`
/// crossbar, with the port at each, if the LUT's inputs can be permuted. Each block but the
/// last takes its inputs straight from the same pins of its parent's port, which nothing else
/// in the parent uses.
fn chain(
    top: &Node,
    path: &[usize],
    port: &str,
    arch: &Architecture,
) -> Option<Vec<(usize, String)>> {
    let mut levels = vec![(path.len(), port.to_owned())];
    let width = top.at(path).input_tokens(port)?.len();
    loop {
        let &(depth, ref port) = levels.last()?;
        if depth == 0 {
            return None;
        }
        let node = top.at(&path[..depth]);
        let parent = top.at(&path[..depth - 1]);
        let tokens = node.input_tokens(port)?;
        if tokens.len() != width {
            return None;
        }
        let parent_type = parent.pb_type();
        let used: Vec<_> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| **t != "open")
            .collect();
        if used.is_empty() {
            return None;
        }
        let sources: Option<Vec<_>> = used.iter().map(|(_, t)| split_source(t)).collect();
        let sources = sources?;
        if sources
            .iter()
            .all(|&(_, _, _, interconnect)| arch.is_complete(parent_type, interconnect))
        {
            return Some(levels);
        }
        // Straight from the parent's port, pin for pin.
        let parent_port = sources[0].1;
        let straight = sources
            .iter()
            .zip(&used)
            .all(|(&(block, p, bit, _), &(i, _))| {
                block == parent_type && p == parent_port && bit as usize == i
            });
        if !straight {
            return None;
        }
        let parent_tokens = parent.input_tokens(parent_port)?;
        if parent_tokens.len() != width
            || parent_tokens
                .iter()
                .zip(&tokens)
                .any(|(a, b)| (*a == "open") != (*b == "open"))
        {
            return None;
        }
        // Nothing else in the parent may use its port.
        let prefix = format!("{parent_type}.{parent_port}[");
        let child = path[depth - 1];
        let others = parent
            .children
            .iter()
            .enumerate()
            .any(|(i, c)| i != child && c.name == "block" && c.mentions(&prefix));
        let own_outputs = parent.children.iter().any(|group| {
            group.name == "outputs" && group.children.iter().any(|p| p.text.contains(&prefix))
        });
        if others || own_outputs {
            return None;
        }
        levels.push((depth - 1, parent_port.to_owned()));
    }
}

/// Calls `f` with the path of every leaf block with a `port_rotation_map` under `node`.
fn leaves(node: &Node, path: &mut Vec<usize>, f: &mut impl FnMut(&[usize], &Node)) {
    let mut any = false;
    for (i, child) in node.children.iter().enumerate() {
        if child.is_block() {
            any = true;
            path.push(i);
            leaves(child, path, f);
            path.pop();
        }
    }
    if !any && node.is_block() && !node.rotation_maps().is_empty() {
        f(path, node);
    }
}

/// Reads a `.net` file a placed block at a time, calling `f` with each, and `outer` with
/// everything else, in order.
fn each_block<R: BufRead>(
    net: R,
    mut outer: impl FnMut(Outer<'_>) -> Result<(), ImportError>,
    mut f: impl FnMut(&mut Node) -> Result<(), ImportError>,
) -> Result<(), ImportError> {
    let mut xml = XmlReader::new(net);
    let mut stack: Vec<Node> = Vec::new();
    let mut root = false;
    while let Some(event) = xml.next_event()? {
        match event {
            Event::Start(tag) => {
                let node = Node {
                    name: tag.name().to_owned(),
                    attrs: tag
                        .attrs()
                        .map(|(n, v)| (n.to_owned(), v.to_owned()))
                        .collect(),
                    text: String::new(),
                    children: Vec::new(),
                };
                if root {
                    stack.push(node);
                } else {
                    root = true;
                    outer(Outer::Start(&node))?;
                }
            }
            Event::Text(text) => {
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(text);
                }
            }
            Event::End => match stack.pop() {
                Some(node) => match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => {
                        let mut node = node;
                        if node.is_block() {
                            f(&mut node)?;
                        }
                        outer(Outer::Child(&node))?;
                    }
                },
                None => outer(Outer::End)?,
            },
        }
    }
    Ok(())
}

/// What [`each_block`] gives outside the placed blocks.
enum Outer<'a> {
    Start(&'a Node),
    Child(&'a Node),
    End,
}

/// Finds the LUTs of a packed netlist whose inputs can be permuted, by the name of the
/// primitive.
///
/// A LUT can be rotated if it is a leaf with a `port_rotation_map`, and its inputs come
/// through blocks that each pass on the same pins of their parent's port, which nothing else in
/// the parent uses, from a block whose inputs come from a `complete` crossbar of
/// `arch`, such as the crossbar that takes a cluster's inputs to its LUTs in VTR's
/// architectures.
///
/// # Errors
///
/// [`ImportError`] if the file is not well-formed XML.
pub fn rotatable_luts<R: BufRead>(
    net: R,
    arch: &Architecture,
) -> Result<BTreeMap<String, RotatableLut>, ImportError> {
    let mut found = BTreeMap::new();
    each_block(
        net,
        |_| Ok(()),
        |top| {
            leaves(top, &mut Vec::new(), &mut |path, leaf| {
                for (port, map) in leaf.rotation_maps() {
                    if chain(top, path, &port, arch).is_some()
                        && let Some(name) = leaf.attr("name")
                    {
                        found.insert(
                            name.to_owned(),
                            RotatableLut {
                                pb_type: leaf.pb_type().to_owned(),
                                port,
                                width: map.len() as u32,
                            },
                        );
                    }
                }
            });
            Ok(())
        },
    )?;
    Ok(found)
}

/// One LUT that [`rotate_luts_for_timing`] works on.
struct Lut {
    primitive: String,
    /// The delay through each physical input.
    delays: Vec<u32>,
    /// Each connected input: its physical pin, its point and its arc to the output.
    inputs: Vec<(u32, PointId, ArcId)>,
    /// The current permutation.
    permutation: Vec<u32>,
}

/// Chooses a rotation of the inputs of every rotatable LUT for timing, and sets the delays of
/// the LUTs' arcs in `timing` to what they become.
///
/// With the arrival times of a timing analysis of `timing`'s graph as it is, connections'
/// delays included, each LUT's connected inputs, latest first, go to its physical inputs,
/// fastest first, by `delays`, which is what makes the LUT's output arrive earliest. That makes
/// arrivals downstream earlier too, so this goes round, a timing analysis each time, until no
/// LUT changes or `passes` have gone by. LUTs whose inputs are all as fast are left alone.
///
/// The delay of every connection, and so the routing, should be in the timing graph already,
/// with [`DesignTiming::set_connection_delays`]: rotating the inputs of a LUT does not change
/// the delays of the connections to it, as long as the crossbar's delay is the same to every
/// input, as it is in VTR's architectures.
pub fn rotate_luts_for_timing(
    timing: &mut DesignTiming,
    netlist: &PackedNetlist,
    delays: &LogicDelays,
    constraints: &Constraints,
    rotatable: &BTreeMap<String, RotatableLut>,
    passes: u32,
) -> LutRotation {
    // The point of each primitive pin, and the primitives to rotate.
    let graph = timing.graph();
    let mut point_of = BTreeMap::new();
    for point in 0..graph.num_points() {
        let point = PointId(point);
        if let Some(InternalPin::Primitive { primitive, pin }) = timing.pin(point) {
            point_of.insert((primitive, pin), point);
        }
    }
    let ports = netlist.ports();
    let mut luts = Vec::new();
    for (index, primitive) in netlist.primitives().iter().enumerate() {
        let Some(lut) = rotatable.get(&primitive.name) else {
            continue;
        };
        if lut.pb_type != primitive.pb_type {
            continue;
        }
        let Some(&output) = primitive.outputs.first() else {
            continue;
        };
        let output_port = &ports[output.port as usize];
        let pin_delays: Option<Vec<u32>> = (0..lut.width)
            .map(|bit| {
                delays.combinational_pins(&lut.pb_type, (&lut.port, bit), (output_port, output.bit))
            })
            .collect();
        let Some(pin_delays) = pin_delays else {
            continue;
        };
        if pin_delays.iter().all(|&d| d == pin_delays[0]) {
            continue;
        }
        let Some(&out_point) = point_of.get(&(index as u32, output)) else {
            continue;
        };
        let mut inputs = Vec::new();
        for &pin in &primitive.inputs {
            if ports[pin.port as usize] != lut.port || pin.bit >= lut.width {
                continue;
            }
            let Some(&point) = point_of.get(&(index as u32, pin)) else {
                continue;
            };
            let arc = graph
                .fanin(out_point)
                .iter()
                .copied()
                .find(|&arc| graph.arc(arc).is_some_and(|info| info.from == point));
            if let Some(arc) = arc {
                inputs.push((pin.bit, point, arc));
            }
        }
        if inputs.is_empty() {
            continue;
        }
        luts.push(Lut {
            primitive: primitive.name.clone(),
            delays: pin_delays,
            inputs,
            permutation: (0..lut.width).collect(),
        });
    }

    let mut analysis = TimingAnalysis::new(timing.graph());
    for _ in 0..passes {
        analysis.run(timing.graph(), constraints);
        let mut changed = false;
        for lut in &mut luts {
            let mut order: Vec<(u32, u64)> = lut
                .inputs
                .iter()
                .map(|&(bit, point, _)| (bit, analysis.arrival(point).unwrap_or(0)))
                .collect();
            order.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let mut fastest: Vec<u32> = (0..lut.delays.len() as u32).collect();
            fastest.sort_by_key(|&bit| (lut.delays[bit as usize], bit));
            let width = lut.delays.len();
            let mut permutation = vec![u32::MAX; width];
            let mut taken = vec![false; width];
            for (&(bit, _), &to) in order.iter().zip(&fastest) {
                permutation[bit as usize] = to;
                taken[to as usize] = true;
            }
            // Unconnected inputs fill what is left, in order.
            let mut free = (0..width as u32).filter(|&to| !taken[to as usize]);
            for slot in &mut permutation {
                if *slot == u32::MAX {
                    *slot = free.next().unwrap_or_default();
                }
            }
            if permutation != lut.permutation {
                changed = true;
                for &(bit, _, arc) in &lut.inputs {
                    let to = permutation[bit as usize] as usize;
                    let _ = timing.graph_mut().set_delay(arc, lut.delays[to]);
                }
                lut.permutation = permutation;
            }
        }
        if !changed {
            break;
        }
    }
    let mut rotation = LutRotation::default();
    for lut in luts {
        rotation.set(&lut.primitive, lut.permutation);
    }
    rotation
}

/// Rotates the inputs of the LUTs of one placed block.
fn rotate_block(top: &mut Node, arch: &Architecture, rotation: &LutRotation) -> usize {
    let mut work = Vec::new();
    leaves(top, &mut Vec::new(), &mut |path, leaf| {
        let Some(permutation) = leaf.attr("name").and_then(|n| rotation.permutation(n)) else {
            return;
        };
        for (port, map) in leaf.rotation_maps() {
            if map.len() == permutation.len() {
                work.push((path.to_vec(), port, permutation.to_vec()));
            }
        }
    });
    let mut rotated = 0;
    for (path, port, permutation) in work {
        let Some(levels) = chain(top, &path, &port, arch) else {
            continue;
        };
        let width = permutation.len();
        let last = levels.len() - 1;
        for (k, (depth, level_port)) in levels.iter().enumerate() {
            let node = top.at(&path[..*depth]);
            let Some(tokens) = node.input_tokens(level_port) else {
                continue;
            };
            let mut moved = vec!["open".to_owned(); width];
            for (i, token) in tokens.iter().enumerate() {
                let to = permutation[i] as usize;
                moved[to] = match split_source(token) {
                    // Below the crossbar, the parent's pin moves with the signal.
                    Some((block, p, _, interconnect)) if k != last => {
                        format!("{block}.{p}[{to}]->{interconnect}")
                    }
                    _ => (*token).to_owned(),
                };
            }
            top.at_mut(&path[..*depth])
                .set_input_text(level_port, moved.join(" "));
        }
        let leaf = top.at_mut(&path);
        if let Some((_, map)) = leaf.rotation_maps().into_iter().find(|(p, _)| *p == port) {
            let mut moved = vec!["open".to_owned(); width];
            for (i, entry) in map.into_iter().enumerate() {
                moved[permutation[i] as usize] = entry;
            }
            leaf.set_rotation_map(&port, moved.join(" "));
        }
        rotated += 1;
    }
    rotated
}

fn escape(text: &str, quotes: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quotes => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

fn start_tag(node: &Node) -> String {
    let mut tag = format!("<{}", node.name);
    for (name, value) in &node.attrs {
        tag.push_str(&format!(" {name}=\"{}\"", escape(value, true)));
    }
    tag
}

fn write_node<W: Write>(out: &mut W, node: &Node, depth: usize) -> std::io::Result<()> {
    let indent = "\t".repeat(depth);
    let tag = start_tag(node);
    if node.children.is_empty() {
        if node.text.is_empty() {
            writeln!(out, "{indent}{tag} />")
        } else {
            writeln!(
                out,
                "{indent}{tag}>{}</{}>",
                escape(&node.text, false),
                node.name
            )
        }
    } else {
        writeln!(out, "{indent}{tag}>")?;
        if !node.text.is_empty() {
            writeln!(out, "{indent}\t{}", escape(&node.text, false))?;
        }
        for child in &node.children {
            write_node(out, child, depth + 1)?;
        }
        writeln!(out, "{indent}</{}>", node.name)
    }
}

/// Writes a packed netlist with the inputs of its LUTs rotated, and returns how many LUTs it
/// rotated.
///
/// For each LUT that `rotation` moves and [`rotatable_luts`] would find, the signal on each
/// physical input moves to the input the rotation says, at every level from the crossbar to the
/// LUT, and the LUT's `port_rotation_map` moves with it, so that every input of the truth table
/// still gets the signal it had. Everything else is written as it was read, in the layout that
/// VPR writes, which VPR reads as the same packing. The netlist's digest changes, so a
/// placement of it has to name the new one.
///
/// # Errors
///
/// [`ImportError`] if the file is not well-formed XML, or [`ImportError::Io`] if writing fails.
pub fn write_rotated_netlist<R: BufRead, W: Write>(
    net: R,
    mut out: W,
    arch: &Architecture,
    rotation: &LutRotation,
) -> Result<usize, ImportError> {
    let io = |source| ImportError::Io { line: 0, source };
    writeln!(out, "<?xml version=\"1.0\"?>").map_err(io)?;
    let mut rotated = 0;
    each_block(
        net,
        |part| {
            match part {
                Outer::Start(node) => writeln!(out, "{}>", start_tag(node)),
                Outer::Child(node) => write_node(&mut out, node, 1),
                Outer::End => writeln!(out, "</block>"),
            }
            .map_err(io)
        },
        |top| {
            rotated += rotate_block(top, arch, rotation);
            Ok(())
        },
    )?;
    out.flush().map_err(io)?;
    Ok(rotated)
}
