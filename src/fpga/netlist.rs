//! Packed netlists: VPR's `.net` format.
//!
//! A packed netlist is a tree of `block` elements: the root stands for the circuit, its
//! children are the blocks a placer places, and everything further down is how those are
//! filled in. Only the children of the root connect to the routing, so only they become part
//! of the result, but their outputs have to be traced through what's below them to find out
//! which nets they carry.
//!
//! What's below them is kept too, in a reduced form, for timing: the primitives at the leaves
//! of each placed block ([`PackedPrimitive`]), and for each primitive pin the chain of
//! interconnect inside the block that reaches it ([`InternalPath`]). That's what
//! [`build_timing`](super::build_timing) needs to time a path through a block.

use std::collections::HashMap;
use std::io::BufRead;

use super::ImportError;
use super::xml::{Event, XmlReader};

/// Why a block connects to a pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PinRole {
    Input,
    Output,
    Clock,
}

/// A pin of a block that connects to a net.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackedPin {
    /// The block, as an index in [`PackedNetlist::blocks`].
    pub block: u32,
    /// The port, as an index in [`PackedNetlist::ports`].
    pub port: u32,
    /// The pin within the port.
    pub bit: u32,
}

/// A net between blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedNet {
    /// The net's name.
    pub name: String,
    /// The pin that drives the net. Only a broken netlist has a net with none.
    pub driver: Option<PackedPin>,
    /// The pins that the net drives, in the order that VPR numbers them after the driver, in
    /// which a pin's position plus one is its `Net_pin_index` in a route file.
    pub sinks: Vec<PackedPin>,
    /// Whether any sink is on a port in a block's `<clocks>`. VPR's default is to leave these
    /// nets to an ideal clock network, and not to route them.
    pub on_clock_port: bool,
}

/// A block as a placer sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedBlock {
    /// The block's name, which the placement refers to it by.
    pub name: String,
    /// The type of the block, which is its `instance` up to the `[`.
    pub pb_type: String,
}

/// A pin of a primitive, by port and position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PortPin {
    /// The port, as an index in [`PackedNetlist::ports`].
    pub port: u32,
    /// The pin within the port.
    pub bit: u32,
}

/// A primitive: a leaf of the hierarchy inside a placed block, such as a LUT, a flip-flop or
/// an I/O pad.
///
/// Only pins that connect to something are listed. A `.net` file doesn't say what a
/// primitive does (the architecture file does), so this is only its name, its type and its
/// pins. [`build_timing`](super::build_timing) says how it tells registers, pads and logic
/// apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedPrimitive {
    /// The placed block it is in, as an index in [`PackedNetlist::blocks`].
    pub block: u32,
    /// Its name, which is the name of the atom that VPR packed into it, as in `n31` or
    /// `out:p6`.
    pub name: String,
    /// Its type, which is its `instance` up to the `[`, as in `lut` or `ff`.
    pub pb_type: String,
    /// Its connected input pins.
    pub inputs: Vec<PortPin>,
    /// Its connected output pins.
    pub outputs: Vec<PortPin>,
    /// Its connected clock pins.
    pub clocks: Vec<PortPin>,
}

/// One end of an [`InternalPath`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InternalPin {
    /// A pin of a placed block, which is where nets between blocks connect.
    Block(PackedPin),
    /// A pin of a primitive.
    Primitive {
        /// The primitive, as an index in [`PackedNetlist::primitives`].
        primitive: u32,
        /// The pin.
        pin: PortPin,
    },
}

/// A connection inside a placed block: from a pin of the block or the output of a primitive,
/// through interconnect, to the input of a primitive or an output pin of the block.
///
/// Every connected input and clock pin of a primitive has one of these, and so does every
/// connected output pin of a placed block, except for
/// [`route_through_outputs`](PackedNetlist::route_through_outputs) and
/// [`unresolved_pins`](PackedNetlist::unresolved_pins).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InternalPath {
    /// Where the path starts: a block's input or clock pin, or a primitive's output.
    pub from: InternalPin,
    /// Where it ends: a primitive's input or clock pin, or a block's output.
    pub to: InternalPin,
    /// Whether it ends on a primitive's clock pin.
    pub clock: bool,
    /// Its interconnect, as a range of `PackedNetlist::hops`.
    hops_start: u32,
    hops_end: u32,
}

/// A piece of interconnect inside a block, as `.net` files name it: `crossbar` in
/// `clb.I[5]->crossbar`.
///
/// Names are only unique within the `pb_type` that the architecture file defines them in,
/// so the type of the block that holds the interconnect is part of its identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Interconnect {
    /// The type of the block that holds it, as in `clb`.
    pub container: String,
    /// Its name, as in `crossbar`.
    pub name: String,
}

/// Where one hop through interconnect comes from and goes to, by the type of each block and
/// the port: `clb` and `I` to `fle` and `in` for `clb.I[5]->crossbar` on an input of an
/// `fle`. This is how VPR's `delay_constant` tells the edges of one interconnect apart, with
/// its `in_port` and `out_port`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HopEnds {
    /// The type of the block the hop comes from: the block that holds the interconnect, or
    /// one inside it.
    pub from_type: String,
    /// The port it comes from.
    pub from_port: String,
    /// The type of the block it goes to.
    pub to_type: String,
    /// The port it goes to.
    pub to_port: String,
}

/// A netlist read from a `.net` file.
///
/// Blocks and nets have the same numbers as in VPR: blocks are numbered in file order, and
/// nets by where each one first turns up, going through blocks in order, and through each
/// block's inputs, then its outputs, then its clocks. That's what makes it possible to write
/// a route file that VPR accepts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackedNetlist {
    name: String,
    architecture_id: Option<String>,
    atom_netlist_id: Option<String>,
    blocks: Vec<PackedBlock>,
    nets: Vec<PackedNet>,
    ports: Vec<String>,
    route_through_outputs: u64,
    primary_inputs: Vec<String>,
    primary_outputs: Vec<String>,
    clock_nets: Vec<String>,
    primitives: Vec<PackedPrimitive>,
    paths: Vec<InternalPath>,
    interconnects: Vec<Interconnect>,
    hops: Vec<u32>,
    /// By hop, an index in `hop_ends`.
    hop_end_of: Vec<u32>,
    hop_ends: Vec<HopEnds>,
    unresolved_pins: u64,
}

impl PackedNetlist {
    /// Returns the name of the netlist, which is the `name` of the root element.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the digest of the architecture file that VPR packed against, if the file
    /// records it. It's `SHA256:` and the hex of the digest of the file's bytes.
    pub fn architecture_id(&self) -> Option<&str> {
        self.architecture_id.as_deref()
    }

    /// Returns the digest of the logical netlist that VPR packed, if the file records it.
    pub fn atom_netlist_id(&self) -> Option<&str> {
        self.atom_netlist_id.as_deref()
    }

    /// Returns the blocks.
    pub fn blocks(&self) -> &[PackedBlock] {
        &self.blocks
    }

    /// Returns the nets.
    pub fn nets(&self) -> &[PackedNet] {
        &self.nets
    }

    /// Returns the names of the ports, which [`PackedPin::port`] is an index in.
    pub fn ports(&self) -> &[String] {
        &self.ports
    }

    /// Returns how many of a block's output pins are fed by one of its own inputs, and don't
    /// have a net of their own.
    ///
    /// VPR's packer doesn't produce these, so this should be 0. They're left out of the nets.
    pub fn route_through_outputs(&self) -> u64 {
        self.route_through_outputs
    }

    /// Returns the names of the circuit's primary inputs, as the root's `<inputs>` lists
    /// them. Each is the name of an input pad's primitive and of the net it drives.
    pub fn primary_inputs(&self) -> &[String] {
        &self.primary_inputs
    }

    /// Returns the names of the circuit's primary outputs, as the root's `<outputs>` lists
    /// them. Each is the name of an output pad's primitive, as in `out:p0`.
    pub fn primary_outputs(&self) -> &[String] {
        &self.primary_outputs
    }

    /// Returns the nets that the root's `<clocks>` lists.
    pub fn clock_nets(&self) -> &[String] {
        &self.clock_nets
    }

    /// Returns the primitives inside the placed blocks, block by block, and in file order
    /// within each block.
    pub fn primitives(&self) -> &[PackedPrimitive] {
        &self.primitives
    }

    /// Returns the connections inside the placed blocks, block by block.
    pub fn internal_paths(&self) -> &[InternalPath] {
        &self.paths
    }

    /// Returns the interconnect that [`hops`](PackedNetlist::hops) refers to.
    pub fn interconnects(&self) -> &[Interconnect] {
        &self.interconnects
    }

    /// Returns the interconnect that a path goes through, from its start to its end, as
    /// indices in [`interconnects`](PackedNetlist::interconnects).
    pub fn hops(&self, path: &InternalPath) -> &[u32] {
        self.hops
            .get(path.hops_start as usize..path.hops_end as usize)
            .unwrap_or(&[])
    }

    /// Returns where each hop of [`hops`](PackedNetlist::hops) comes from and goes to, as
    /// indices in [`hop_ends`](PackedNetlist::hop_ends), in the same order.
    pub fn path_hop_ends(&self, path: &InternalPath) -> &[u32] {
        self.hop_end_of
            .get(path.hops_start as usize..path.hops_end as usize)
            .unwrap_or(&[])
    }

    /// Returns the ends of hops that [`path_hop_ends`](PackedNetlist::path_hop_ends) refers
    /// to, each once, in the order they were first met.
    pub fn hop_ends(&self) -> &[HopEnds] {
        &self.hop_ends
    }

    /// Returns how many connected pins inside blocks could not be traced back to what drives
    /// them, because the file names a pin that doesn't exist. VPR doesn't write such files,
    /// so this should be 0. Such a pin has no [`InternalPath`], which cuts timing paths
    /// through it.
    pub fn unresolved_pins(&self) -> u64 {
        self.unresolved_pins
    }
}

/// Returns the type of an instance, which is the part before the `[`.
fn type_of(instance: &str) -> &str {
    instance
        .split_once('[')
        .map_or(instance, |(pb_type, _)| pb_type)
}

/// What kind of element an open one is, in the parts of the file that matter.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Element {
    Block,
    Pins(PinRole),
    Port,
    Other,
}

/// A port's name and its pins as written.
struct PortText {
    name: String,
    text: String,
}

/// An open `block`.
struct Frame {
    /// As in `fle[1]`.
    instance: String,
    /// The block's name, which is `open` for an unused place.
    name: String,
    /// The block's number in the netlist, if a placer sees it: if it's a child of the root
    /// and it isn't one of the empty places, which are all named `open`.
    placed: Option<u32>,
    /// True for a block with no `mode`, which is a primitive, whose outputs name nets.
    is_leaf: bool,
    inputs: Vec<PortText>,
    outputs: Vec<PortText>,
    clocks: Vec<PortText>,
    /// The nets on the output pins of the blocks directly below this one, by the name that
    /// this block's own pins use for them, as in `fle[1].out[0]`. This holds what one placed
    /// block needs, which is small, and is thrown away after it.
    below: HashMap<String, String>,
    /// The used blocks directly below this one, kept inside a placed block for
    /// [`trace_internals`].
    children: Vec<Frame>,
}

impl Frame {
    fn new(instance: &str, name: &str, placed: Option<u32>, is_leaf: bool) -> Frame {
        Frame {
            instance: instance.to_owned(),
            name: name.to_owned(),
            placed,
            is_leaf,
            inputs: Vec::new(),
            outputs: Vec::new(),
            clocks: Vec::new(),
            below: HashMap::new(),
            children: Vec::new(),
        }
    }

    /// Returns the block's type, which is its instance up to the `[`.
    fn pb_type(&self) -> &str {
        type_of(&self.instance)
    }

    /// Returns whether anything in the block is connected.
    fn is_used(&self) -> bool {
        !self.children.is_empty()
            || [&self.inputs, &self.outputs, &self.clocks]
                .into_iter()
                .flatten()
                .any(|port| port.text.split_ascii_whitespace().any(|pin| pin != "open"))
    }

    /// Finds the net on an output pin, written as this block wrote it.
    ///
    /// A primitive's output pin is named after the net. Other blocks say what drives the
    /// pin, as in `fle[1].out[0]->clbouts1`, which has to be looked up.
    fn output_net<'a>(&'a self, pin: &'a str) -> Option<&'a str> {
        if self.is_leaf {
            return Some(pin);
        }
        match pin.split_once("->") {
            Some((driver, _)) => self.below.get(driver).map(String::as_str),
            None => Some(pin),
        }
    }
}

/// Adds a net to the netlist's numbering by name, and returns the number.
fn net_number(netlist: &mut PackedNetlist, numbers: &mut HashMap<String, u32>, name: &str) -> u32 {
    if let Some(&number) = numbers.get(name) {
        return number;
    }
    let number = netlist.nets.len() as u32;
    numbers.insert(name.to_owned(), number);
    netlist.nets.push(PackedNet {
        name: name.to_owned(),
        driver: None,
        sinks: Vec::new(),
        on_clock_port: false,
    });
    number
}

/// Adds a port to the netlist's numbering by name, and returns the number.
fn port_number(netlist: &mut PackedNetlist, numbers: &mut HashMap<String, u32>, name: &str) -> u32 {
    if let Some(&number) = numbers.get(name) {
        return number;
    }
    let number = netlist.ports.len() as u32;
    numbers.insert(name.to_owned(), number);
    netlist.ports.push(name.to_owned());
    number
}

/// Reads a packed netlist.
///
/// This goes through the file once and holds what it needs for one placed block at a time.
///
/// # Errors
///
/// Fails if the XML is malformed or a read fails, if the root isn't a VPR netlist, or if
/// a net has more than one driver, with the line where it happened.
///
/// # Examples
///
/// ```
/// use guide::fpga::read_netlist;
///
/// let text = r#"<block name="top.net" instance="FPGA_packed_netlist[0]">
///   <block name="a" instance="io[0]" mode="inpad">
///     <outputs><port name="inpad">inpad[0].inpad[0]-&gt;x</port></outputs>
///     <block name="a" instance="inpad[0]"><outputs><port name="inpad">a</port></outputs></block>
///   </block>
///   <block name="out:b" instance="io[1]" mode="outpad">
///     <inputs><port name="outpad">a</port></inputs>
///   </block>
/// </block>"#;
/// let netlist = read_netlist(text.as_bytes())?;
/// assert_eq!(netlist.blocks()[1].pb_type, "io");
/// let net = &netlist.nets()[0];
/// assert_eq!(net.name, "a");
/// assert_eq!(net.driver.unwrap().block, 0);
/// assert_eq!(net.sinks[0].block, 1);
/// # Ok::<(), guide::fpga::ImportError>(())
/// ```
pub fn read_netlist<R: BufRead>(reader: R) -> Result<PackedNetlist, ImportError> {
    let mut xml = XmlReader::new(reader);
    let mut netlist = PackedNetlist::default();
    let mut net_numbers = HashMap::new();
    let mut port_numbers = HashMap::new();
    let mut interconnect_numbers = HashMap::new();
    let mut hop_end_numbers = HashMap::new();
    // The root's inputs, outputs and clocks, by `PinRole`.
    let mut root_lists = [String::new(), String::new(), String::new()];

    let mut frames: Vec<Frame> = Vec::new();
    let mut open: Vec<Element> = Vec::new();
    let mut port: Option<PortText> = None;
    let mut pin_role = PinRole::Input;
    let mut seen_root = false;

    while let Some(event) = xml.next_event()? {
        match event {
            Event::Start(tag) => {
                let line = tag.line();
                let parent = open.last().copied();
                let element = match tag.name() {
                    "block" => {
                        let Some(instance) = tag.attr("instance") else {
                            return Err(ImportError::MissingAttribute {
                                line,
                                element: "block".to_owned(),
                                attribute: "instance",
                            });
                        };
                        let name =
                            tag.attr("name")
                                .ok_or_else(|| ImportError::MissingAttribute {
                                    line,
                                    element: "block".to_owned(),
                                    attribute: "name",
                                })?;
                        let mut placed = None;
                        if !seen_root {
                            seen_root = true;
                            if instance != "FPGA_packed_netlist[0]" {
                                return Err(ImportError::Malformed {
                                    line,
                                    message: format!(
                                        "expected the root to be FPGA_packed_netlist[0], found {}",
                                        instance.chars().take(64).collect::<String>()
                                    ),
                                });
                            }
                            netlist.name = name.to_owned();
                            netlist.architecture_id =
                                tag.attr("architecture_id").map(str::to_owned);
                            netlist.atom_netlist_id =
                                tag.attr("atom_netlist_id").map(str::to_owned);
                        } else if frames.len() == 1 && name != "open" {
                            let pb_type = type_of(instance);
                            placed = Some(netlist.blocks.len() as u32);
                            netlist.blocks.push(PackedBlock {
                                name: name.to_owned(),
                                pb_type: pb_type.to_owned(),
                            });
                        }
                        frames.push(Frame::new(
                            instance,
                            name,
                            placed,
                            tag.attr("mode").is_none(),
                        ));
                        Element::Block
                    }
                    "inputs" if parent == Some(Element::Block) => Element::Pins(PinRole::Input),
                    "outputs" if parent == Some(Element::Block) => Element::Pins(PinRole::Output),
                    "clocks" if parent == Some(Element::Block) => Element::Pins(PinRole::Clock),
                    "port" if matches!(parent, Some(Element::Pins(_))) => {
                        if let Some(Element::Pins(role)) = parent {
                            pin_role = role;
                        }
                        let Some(name) = tag.attr("name") else {
                            return Err(ImportError::MissingAttribute {
                                line,
                                element: "port".to_owned(),
                                attribute: "name",
                            });
                        };
                        port = Some(PortText {
                            name: name.to_owned(),
                            text: String::new(),
                        });
                        Element::Port
                    }
                    _ => Element::Other,
                };
                open.push(element);
                if !seen_root {
                    return Err(ImportError::Malformed {
                        line,
                        message: format!(
                            "expected <block> as the root, found <{}>",
                            tag.name().chars().take(64).collect::<String>()
                        ),
                    });
                }
            }
            Event::Text(text) => {
                if let Some(port) = &mut port {
                    port.text.push_str(text);
                } else if let (1, Some(Element::Pins(role))) = (frames.len(), open.last()) {
                    // The root lists the circuit's own inputs, outputs and clocks as text.
                    let list = &mut root_lists[*role as usize];
                    list.push_str(text);
                    list.push(' ');
                }
            }
            Event::End => {
                let line = xml.line();
                match open.pop() {
                    Some(Element::Port) => {
                        if let (Some(finished), Some(frame)) = (port.take(), frames.last_mut()) {
                            match pin_role {
                                PinRole::Input => frame.inputs.push(finished),
                                PinRole::Output => frame.outputs.push(finished),
                                PinRole::Clock => frame.clocks.push(finished),
                            }
                        }
                    }
                    Some(Element::Block) => {
                        let Some(frame) = frames.pop() else {
                            continue;
                        };
                        match frames.len() {
                            0 => {}
                            1 => {
                                if let Some(block) = frame.placed {
                                    finish_placed_block(
                                        &frame,
                                        block,
                                        &mut netlist,
                                        &mut net_numbers,
                                        &mut port_numbers,
                                        line,
                                    )?;
                                    trace_internals(
                                        &frame,
                                        block,
                                        &mut netlist,
                                        &mut port_numbers,
                                        &mut interconnect_numbers,
                                        &mut hop_end_numbers,
                                    );
                                }
                            }
                            _ => {
                                // Only what's used inside a used placed block is kept. That
                                // isn't decided by the name: a LUT used as a wire is `open`.
                                let keep = frame.is_used()
                                    && frames.get(1).is_some_and(|placed| placed.placed.is_some());
                                if let Some(parent) = frames.last_mut() {
                                    pass_outputs_up(&frame, parent);
                                    if keep {
                                        parent.children.push(frame);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let [inputs, outputs, clocks] = root_lists;
    let names = |list: String| list.split_ascii_whitespace().map(str::to_owned).collect();
    netlist.primary_inputs = names(inputs);
    netlist.primary_outputs = names(outputs);
    netlist.clock_nets = names(clocks);
    Ok(netlist)
}

/// Tells a block what's on the output pins of a block directly inside it.
fn pass_outputs_up(frame: &Frame, parent: &mut Frame) {
    for port in &frame.outputs {
        for (bit, pin) in port.text.split_ascii_whitespace().enumerate() {
            if pin == "open" {
                continue;
            }
            if let Some(net) = frame.output_net(pin) {
                let key = format!("{}.{}[{bit}]", frame.instance, port.name);
                parent.below.insert(key, net.to_owned());
            }
        }
    }
}

/// A block of a placed block's hierarchy, flattened, with the placed block itself first.
struct FlatBlock<'a> {
    frame: &'a Frame,
    parent: Option<usize>,
    children: Vec<usize>,
    /// The primitive it is, if it's a leaf.
    primitive: Option<u32>,
}

fn flatten<'a>(frame: &'a Frame, parent: Option<usize>, out: &mut Vec<FlatBlock<'a>>) -> usize {
    let index = out.len();
    out.push(FlatBlock {
        frame,
        parent,
        children: Vec::new(),
        primitive: None,
    });
    for child in &frame.children {
        let child = flatten(child, Some(index), out);
        out[index].children.push(child);
    }
    index
}

/// A pin of a [`FlatBlock`]: the block, the port's name and the position.
type FlatPin<'a> = (usize, &'a str, u32);

/// Splits `fle[5].out[0]` into `fle[5]`, `out` and 0.
fn parse_pin_name(text: &str) -> Option<(&str, &str, u32)> {
    let (instance, pin) = text.split_once('.')?;
    let (port, rest) = pin.split_once('[')?;
    let bit = rest.strip_suffix(']')?.parse().ok()?;
    Some((instance, port, bit))
}

/// Adds an interconnect to the netlist's numbering, and returns the number.
fn interconnect_number(
    netlist: &mut PackedNetlist,
    numbers: &mut HashMap<(String, String), u32>,
    container: &str,
    name: &str,
) -> u32 {
    let key = (container.to_owned(), name.to_owned());
    if let Some(&number) = numbers.get(&key) {
        return number;
    }
    let number = netlist.interconnects.len() as u32;
    netlist.interconnects.push(Interconnect {
        container: key.0.clone(),
        name: key.1.clone(),
    });
    numbers.insert(key, number);
    number
}

/// Adds the ends of a hop to the netlist's numbering, and returns the number.
fn hop_end_number(
    netlist: &mut PackedNetlist,
    numbers: &mut HashMap<HopEnds, u32>,
    ends: HopEnds,
) -> u32 {
    if let Some(&number) = numbers.get(&ends) {
        return number;
    }
    let number = netlist.hop_ends.len() as u32;
    netlist.hop_ends.push(ends.clone());
    numbers.insert(ends, number);
    number
}

/// One hop of a walk back from a pin: the block holding the interconnect, its name, and the
/// block and port on either side.
#[derive(Clone)]
struct Hop<'a> {
    holder: usize,
    interconnect: &'a str,
    from: (usize, &'a str),
    to: (usize, &'a str),
}

/// Records the primitives inside a placed block and the paths between them and the block's
/// pins.
///
/// Inside a block, each pin is written as what drives it: `clb.I[5]->crossbar` for an input
/// of a block driven by an input of the block that holds it, through the interconnect
/// `crossbar` of that block, or `fle[5].out[0]->crossbar` for one driven by the output of a
/// sibling. An output of a block that isn't a primitive names a block inside it the same way.
/// So the type alone names the holder, and a type with an index one of its children. The
/// walk back from a pin ends at a pin of the placed block, which is named after its net, or
/// at the output of a primitive.
fn trace_internals(
    frame: &Frame,
    block: u32,
    netlist: &mut PackedNetlist,
    port_numbers: &mut HashMap<String, u32>,
    interconnect_numbers: &mut HashMap<(String, String), u32>,
    hop_end_numbers: &mut HashMap<HopEnds, u32>,
) {
    let mut blocks = Vec::new();
    flatten(frame, None, &mut blocks);

    // What drives each connected pin, as written, and whether it's an output.
    let mut drivers: HashMap<FlatPin<'_>, (&str, bool)> = HashMap::new();
    for (index, flat) in blocks.iter().enumerate() {
        for (ports, is_output) in [
            (&flat.frame.inputs, false),
            (&flat.frame.outputs, true),
            (&flat.frame.clocks, false),
        ] {
            for port in ports {
                for (bit, text) in port.text.split_ascii_whitespace().enumerate() {
                    if text != "open" {
                        drivers.insert((index, port.name.as_str(), bit as u32), (text, is_output));
                    }
                }
            }
        }
    }

    // The primitives, in file order.
    for flat in &mut blocks {
        if !flat.frame.is_leaf {
            continue;
        }
        let frame = flat.frame;
        let mut pins = |ports: &[PortText]| -> Vec<PortPin> {
            let mut pins = Vec::new();
            for port in ports {
                let port_id = port_number(netlist, port_numbers, &port.name);
                for (bit, text) in port.text.split_ascii_whitespace().enumerate() {
                    if text != "open" {
                        pins.push(PortPin {
                            port: port_id,
                            bit: bit as u32,
                        });
                    }
                }
            }
            pins
        };
        let primitive = PackedPrimitive {
            block,
            name: frame.name.clone(),
            pb_type: frame.pb_type().to_owned(),
            inputs: pins(&frame.inputs),
            outputs: pins(&frame.outputs),
            clocks: pins(&frame.clocks),
        };
        flat.primitive = Some(netlist.primitives.len() as u32);
        netlist.primitives.push(primitive);
    }

    // Walks back from a pin to its source, collecting the interconnect on the way, from the
    // sink end. A cycle in a damaged file can't loop forever: no walk is longer than there
    // are pins.
    fn walk_back<'a>(
        blocks: &[FlatBlock<'a>],
        drivers: &HashMap<FlatPin<'a>, (&'a str, bool)>,
        start: FlatPin<'a>,
        hops: &mut Vec<Hop<'a>>,
    ) -> Option<FlatPin<'a>> {
        let mut current = start;
        for _ in 0..=drivers.len() {
            let &(text, is_output) = drivers.get(&current)?;
            if current != start {
                if current.0 == 0 && !is_output {
                    return Some(current);
                }
                if is_output && blocks[current.0].frame.is_leaf {
                    return Some(current);
                }
            }
            let (name, interconnect) = text.split_once("->")?;
            let holder = if is_output {
                current.0
            } else {
                blocks[current.0].parent?
            };
            let (instance, port, bit) = parse_pin_name(name)?;
            // The holder is usually named by its type alone, but a block used as a wire, such
            // as a LUT in `wire` mode, names its own inputs with its index.
            let holder_frame = blocks[holder].frame;
            let child = blocks[holder]
                .children
                .iter()
                .find(|&&child| blocks[child].frame.instance == instance);
            let next = match child {
                Some(&child) => child,
                None if instance == holder_frame.pb_type() || instance == holder_frame.instance => {
                    holder
                }
                None => return None,
            };
            hops.push(Hop {
                holder,
                interconnect,
                from: (next, port),
                to: (current.0, current.1),
            });
            current = (next, port, bit);
        }
        None
    }

    let mut paths = Vec::new();
    let mut hops = Vec::new();
    let mut unresolved = 0;
    let top_is_leaf = blocks[0].frame.is_leaf;
    for (index, flat) in blocks.iter().enumerate() {
        let frame = flat.frame;
        let roles: &[(&Vec<PortText>, bool, bool)] = if top_is_leaf {
            // A placed block that is a primitive itself: its pins are the block's, and the
            // paths between the two go through nothing.
            &[]
        } else if index == 0 {
            &[(&frame.outputs, true, false)]
        } else if frame.is_leaf {
            &[(&frame.inputs, false, false), (&frame.clocks, false, true)]
        } else {
            &[]
        };
        for &(ports, is_output, clock) in roles {
            for port in ports {
                for (bit, text) in port.text.split_ascii_whitespace().enumerate() {
                    if text == "open" {
                        continue;
                    }
                    hops.clear();
                    let start = (index, port.name.as_str(), bit as u32);
                    match walk_back(&blocks, &drivers, start, &mut hops) {
                        // An output fed by one of the block's own inputs, which
                        // `finish_placed_block` has already counted.
                        Some(source) if is_output && source.0 == 0 => {}
                        Some(source) => paths.push((source, start, clock, hops.clone())),
                        None => unresolved += 1,
                    }
                }
            }
        }
    }

    let mut pin = |netlist: &mut PackedNetlist, (index, port, bit): FlatPin<'_>| {
        let port = port_number(netlist, port_numbers, port);
        match blocks[index].primitive {
            Some(primitive) => InternalPin::Primitive {
                primitive,
                pin: PortPin { port, bit },
            },
            None => InternalPin::Block(PackedPin { block, port, bit }),
        }
    };
    for (source, sink, clock, path_hops) in paths {
        let from = pin(netlist, source);
        let to = pin(netlist, sink);
        let hops_start = netlist.hops.len() as u32;
        for hop in path_hops.iter().rev() {
            let number = interconnect_number(
                netlist,
                interconnect_numbers,
                blocks[hop.holder].frame.pb_type(),
                hop.interconnect,
            );
            netlist.hops.push(number);
            let ends = HopEnds {
                from_type: blocks[hop.from.0].frame.pb_type().to_owned(),
                from_port: hop.from.1.to_owned(),
                to_type: blocks[hop.to.0].frame.pb_type().to_owned(),
                to_port: hop.to.1.to_owned(),
            };
            let ends = hop_end_number(netlist, hop_end_numbers, ends);
            netlist.hop_end_of.push(ends);
        }
        netlist.paths.push(InternalPath {
            from,
            to,
            clock,
            hops_start,
            hops_end: netlist.hops.len() as u32,
        });
    }

    if top_is_leaf && let Some(primitive) = blocks[0].primitive {
        // Each pin of the block is a pin of the primitive, with nothing in between.
        let record = netlist.primitives[primitive as usize].clone();
        for (pins, outward, clock) in [
            (&record.inputs, false, false),
            (&record.clocks, false, true),
            (&record.outputs, true, false),
        ] {
            for &pin in pins {
                let outer = InternalPin::Block(PackedPin {
                    block,
                    port: pin.port,
                    bit: pin.bit,
                });
                let inner = InternalPin::Primitive { primitive, pin };
                let (from, to) = if outward {
                    (inner, outer)
                } else {
                    (outer, inner)
                };
                let at = netlist.hops.len() as u32;
                netlist.paths.push(InternalPath {
                    from,
                    to,
                    clock,
                    hops_start: at,
                    hops_end: at,
                });
            }
        }
    }
    netlist.unresolved_pins += unresolved;
}

/// Adds the pins of a block that a placer can see to the netlist, in VPR's order.
fn finish_placed_block(
    frame: &Frame,
    block: u32,
    netlist: &mut PackedNetlist,
    net_numbers: &mut HashMap<String, u32>,
    port_numbers: &mut HashMap<String, u32>,
    line: u64,
) -> Result<(), ImportError> {
    for (ports, role) in [
        (&frame.inputs, PinRole::Input),
        (&frame.outputs, PinRole::Output),
        (&frame.clocks, PinRole::Clock),
    ] {
        for port in ports {
            let port_id = port_number(netlist, port_numbers, &port.name);
            for (bit, pin) in port.text.split_ascii_whitespace().enumerate() {
                if pin == "open" {
                    continue;
                }
                let name = match role {
                    PinRole::Output => match frame.output_net(pin) {
                        Some(net) => net,
                        None => {
                            netlist.route_through_outputs += 1;
                            continue;
                        }
                    },
                    // The input pins of a placed block are named after their nets.
                    PinRole::Input | PinRole::Clock => pin,
                };
                let number = net_number(netlist, net_numbers, name);
                let packed = PackedPin {
                    block,
                    port: port_id,
                    bit: bit as u32,
                };
                let net = &mut netlist.nets[number as usize];
                match role {
                    PinRole::Output => {
                        if net.driver.is_some() {
                            return Err(ImportError::MultipleDrivers {
                                line,
                                net: net.name.clone(),
                            });
                        }
                        net.driver = Some(packed);
                    }
                    PinRole::Input => net.sinks.push(packed),
                    PinRole::Clock => {
                        net.sinks.push(packed);
                        net.on_clock_port = true;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::io::BufReader;

    const GENUINE: &str = include_str!("../../testdata/vtr/mult_4x4.net");
    const MULTICLOCK: &str = include_str!("../../testdata/vtr/multiclock.net");
    const UNCONNECTED: &str = include_str!("../../testdata/vtr/unconnected.net");

    fn net<'a>(netlist: &'a PackedNetlist, name: &str) -> &'a PackedNet {
        netlist.nets().iter().find(|net| net.name == name).unwrap()
    }

    #[test]
    fn reads_a_genuine_netlist() {
        let netlist = read_netlist(GENUINE.as_bytes()).unwrap();
        assert_eq!(netlist.name(), "mult_4x4.net");
        assert_eq!(
            netlist.architecture_id(),
            Some("SHA256:5db51ec7ae1ca9eb9a6aac159b2b59db081121ebdc880deda954d3cdde5cb6c5")
        );
        assert_eq!(netlist.blocks().len(), 19);
        assert_eq!(netlist.blocks()[0].name, "n27");
        assert_eq!(netlist.blocks()[0].pb_type, "clb");
        assert_eq!(netlist.blocks()[3].pb_type, "io");
        assert_eq!(netlist.route_through_outputs(), 0);

        // Net numbers agree with the route file written from this netlist.
        let names: Vec<&str> = netlist.nets().iter().map(|net| net.name.as_str()).collect();
        assert_eq!(names[..7], ["y0", "y2", "x0", "n21", "n32", "n34", "x1"]);
        assert_eq!(names.len(), 25);
    }

    #[test]
    fn traces_outputs_through_the_hierarchy_to_their_nets() {
        let netlist = read_netlist(GENUINE.as_bytes()).unwrap();
        // `n32` drives out of the third block, which wrote it as `fle[9].out[0]->clbouts1`.
        let driver = net(&netlist, "n32").driver.unwrap();
        assert_eq!(netlist.blocks()[driver.block as usize].name, "n32");
        assert_eq!(netlist.ports()[driver.port as usize], "O");
        assert_eq!(driver.bit, 9);
        // An I/O pad's output comes straight from the primitive inside it.
        let pad = net(&netlist, "x0").driver.unwrap();
        assert_eq!(netlist.blocks()[pad.block as usize].name, "x0");
        assert_eq!(netlist.ports()[pad.port as usize], "inpad");
    }

    #[test]
    fn every_net_of_a_finished_netlist_has_a_driver() {
        let netlist = read_netlist(GENUINE.as_bytes()).unwrap();
        for net in netlist.nets() {
            assert!(net.driver.is_some(), "{} has no driver", net.name);
            assert!(!net.sinks.is_empty(), "{} has no sinks", net.name);
            assert!(!net.on_clock_port);
        }
    }

    #[test]
    fn finds_clocks_in_a_genuine_netlist() {
        let netlist = read_netlist(MULTICLOCK.as_bytes()).unwrap();
        let clocks: Vec<&str> = netlist
            .nets()
            .iter()
            .filter(|net| net.on_clock_port)
            .map(|net| net.name.as_str())
            .collect();
        assert_eq!(clocks, ["clk", "clk2"]);
        assert!(net(&netlist, "clk").driver.is_some());
        assert!(!net(&netlist, "in1").on_clock_port);
    }

    #[test]
    fn reads_a_genuine_netlist_with_multibit_ports_and_unconnected_outputs() {
        let netlist = read_netlist(UNCONNECTED.as_bytes()).unwrap();
        let block = netlist
            .blocks()
            .iter()
            .position(|block| block.name == "inst")
            .unwrap() as u32;
        // The port is written `a[2] open a[1] open open a[3] a[0] open`.
        let sink = net(&netlist, "a[3]").sinks[0];
        assert_eq!(sink.block, block);
        assert_eq!(netlist.ports()[sink.port as usize], "a");
        assert_eq!(sink.bit, 5);
    }

    #[test]
    fn nets_with_several_sinks_list_them_in_block_order() {
        let netlist = read_netlist(GENUINE.as_bytes()).unwrap();
        let sinks = &net(&netlist, "y0").sinks;
        let blocks: Vec<&str> = sinks
            .iter()
            .map(|sink| netlist.blocks()[sink.block as usize].name.as_str())
            .collect();
        assert_eq!(blocks, ["n27", "n22", "n32"]);
    }

    fn primitive<'a>(
        netlist: &'a PackedNetlist,
        name: &str,
        pb_type: &str,
    ) -> (u32, &'a PackedPrimitive) {
        let (index, primitive) = netlist
            .primitives()
            .iter()
            .enumerate()
            .find(|(_, primitive)| primitive.name == name && primitive.pb_type == pb_type)
            .unwrap();
        (index as u32, primitive)
    }

    fn port(netlist: &PackedNetlist, name: &str) -> u32 {
        netlist
            .ports()
            .iter()
            .position(|port| port == name)
            .unwrap() as u32
    }

    fn hop_names(netlist: &PackedNetlist, path: &InternalPath) -> Vec<String> {
        netlist
            .hops(path)
            .iter()
            .map(|&hop| {
                let interconnect = &netlist.interconnects()[hop as usize];
                format!("{}/{}", interconnect.container, interconnect.name)
            })
            .collect()
    }

    fn path_to(netlist: &PackedNetlist, to: InternalPin) -> &InternalPath {
        netlist
            .internal_paths()
            .iter()
            .find(|path| path.to == to)
            .unwrap()
    }

    #[test]
    fn keeps_the_primitives_and_the_root_lists() {
        let netlist = read_netlist(GENUINE.as_bytes()).unwrap();
        assert_eq!(netlist.primary_inputs()[..2], ["x0", "x1"]);
        assert_eq!(netlist.primary_outputs().len(), 8);
        assert_eq!(netlist.primary_outputs()[0], "out:p0");
        assert!(netlist.clock_nets().is_empty());
        assert_eq!(netlist.unresolved_pins(), 0);

        let count = |pb_type: &str| {
            netlist
                .primitives()
                .iter()
                .filter(|primitive| primitive.pb_type == pb_type)
                .count()
        };
        // Three CLBs, one of them with 10 LUTs, and the 16 pads.
        assert_eq!(count("inpad"), 8);
        assert_eq!(count("outpad"), 8);
        assert_eq!(count("ff"), 0);
        assert_eq!(
            count("lut"),
            netlist.primitives().len() - 16,
            "everything else is a LUT"
        );

        // `n31` uses five of its six inputs.
        let (_, n31) = primitive(&netlist, "n31", "lut");
        assert_eq!(n31.block, 0);
        assert_eq!(n31.inputs.len(), 5);
        assert_eq!(n31.outputs.len(), 1);
        assert!(n31.clocks.is_empty());
    }

    #[test]
    fn traces_each_primitive_pin_through_the_interconnect() {
        let netlist = read_netlist(GENUINE.as_bytes()).unwrap();
        let (n31, _) = primitive(&netlist, "n31", "lut");
        let input = |bit| InternalPin::Primitive {
            primitive: n31,
            pin: PortPin {
                port: port(&netlist, "in"),
                bit,
            },
        };

        // `lut[0].in[2]` comes from `lut6.in[2]`, `ble6.in[2]`, `fle.in[2]`, and `clb.I[7]`.
        let path = path_to(&netlist, input(2));
        assert_eq!(
            path.from,
            InternalPin::Block(PackedPin {
                block: 0,
                port: port(&netlist, "I"),
                bit: 7,
            })
        );
        assert!(!path.clock);
        assert_eq!(
            hop_names(&netlist, path),
            [
                "clb/crossbar",
                "fle/direct1",
                "ble6/direct1",
                "lut6/direct:lut6"
            ]
        );
        // The unused input has no path.
        assert!(
            !netlist
                .internal_paths()
                .iter()
                .any(|path| path.to == input(1))
        );

        // `p7`'s first input comes from inside the block, from the LUT in `fle[5]`.
        let (p7, _) = primitive(&netlist, "p7", "lut");
        let path = path_to(
            &netlist,
            InternalPin::Primitive {
                primitive: p7,
                pin: PortPin {
                    port: port(&netlist, "in"),
                    bit: 0,
                },
            },
        );
        let InternalPin::Primitive {
            primitive: from,
            pin,
        } = path.from
        else {
            panic!("{:?} should come from a primitive", path.from);
        };
        assert_eq!(netlist.primitives()[from as usize].block, 0);
        assert_eq!(netlist.ports()[pin.port as usize], "out");
        // Up out of `fle[5]`, through the crossbar, and down into `fle[1]`.
        assert_eq!(
            hop_names(&netlist, path),
            [
                "lut6/direct:lut6",
                "ble6/mux1",
                "fle/direct2",
                "clb/crossbar",
                "fle/direct1",
                "ble6/direct1",
                "lut6/direct:lut6"
            ]
        );

        // And `p7` leaves the block on `O[1]`.
        let path = path_to(
            &netlist,
            InternalPin::Block(PackedPin {
                block: 0,
                port: port(&netlist, "O"),
                bit: 1,
            }),
        );
        assert_eq!(
            path.from,
            InternalPin::Primitive {
                primitive: p7,
                pin: PortPin {
                    port: port(&netlist, "out"),
                    bit: 0,
                },
            }
        );
        assert_eq!(
            hop_names(&netlist, path),
            [
                "lut6/direct:lut6",
                "ble6/mux1",
                "fle/direct2",
                "clb/clbouts1"
            ]
        );
    }

    #[test]
    fn traces_clock_pins_to_their_clock() {
        let netlist = read_netlist(MULTICLOCK.as_bytes()).unwrap();
        assert_eq!(netlist.clock_nets(), ["clk", "clk2"]);
        let (ffa, record) = primitive(&netlist, "FFA", "ff");
        assert_eq!(record.clocks.len(), 1);
        let path = netlist
            .internal_paths()
            .iter()
            .find(|path| {
                path.clock
                    && matches!(path.to, InternalPin::Primitive { primitive, .. } if primitive == ffa)
            })
            .unwrap();
        let InternalPin::Block(pin) = path.from else {
            panic!("the clock should come from the block's pin");
        };
        assert_eq!(netlist.ports()[pin.port as usize], "clk");
        assert!(net(&netlist, "clk").sinks.contains(&pin));
        assert_eq!(
            hop_names(&netlist, path),
            ["clb/clks", "fle/direct3", "ble6/direct3"]
        );

        // `FFA`'s data comes through a LUT used as a wire, which the file calls `open`.
        let path = path_to(
            &netlist,
            InternalPin::Primitive {
                primitive: ffa,
                pin: PortPin {
                    port: port(&netlist, "D"),
                    bit: 0,
                },
            },
        );
        assert_eq!(
            hop_names(&netlist, path),
            [
                "clb/crossbar",
                "fle/direct1",
                "ble6/direct1",
                "lut6/complete:lut6",
                "ble6/direct2"
            ]
        );
        assert_eq!(netlist.unresolved_pins(), 0);
    }

    #[test]
    fn follows_scrambled_multibit_ports_and_skips_open_pins() {
        let netlist = read_netlist(UNCONNECTED.as_bytes()).unwrap();
        assert_eq!(netlist.unresolved_pins(), 0);
        let (bel, record) = primitive(&netlist, "inst", "dsp_bel");
        // Four of `a`, all eight of `b`, and none of `addsub`.
        assert_eq!(record.inputs.len(), 12);
        assert_eq!(record.outputs.len(), 8);
        // `dsp_bel.a[0]` is `dsp.a[6]`, which is the net `a[0]`.
        let path = path_to(
            &netlist,
            InternalPin::Primitive {
                primitive: bel,
                pin: PortPin {
                    port: port(&netlist, "a"),
                    bit: 0,
                },
            },
        );
        let InternalPin::Block(pin) = path.from else {
            panic!("should come from the block");
        };
        assert_eq!(pin.bit, 6);
        assert!(net(&netlist, "a[0]").sinks.contains(&pin));
        assert_eq!(hop_names(&netlist, path), ["dsp/xbar_a"]);
    }

    #[test]
    fn placed_blocks_that_are_primitives_connect_directly() {
        let text = "<block name='t' instance='FPGA_packed_netlist[0]'>\
            <block name='a' instance='lut[0]'>\
            <inputs><port name='in'>x open y</port></inputs>\
            <outputs><port name='out'>a</port></outputs></block>\
            </block>";
        let netlist = read_netlist(text.as_bytes()).unwrap();
        assert_eq!(netlist.primitives().len(), 1);
        assert_eq!(netlist.primitives()[0].inputs.len(), 2);
        assert_eq!(netlist.internal_paths().len(), 3);
        for path in netlist.internal_paths() {
            assert!(netlist.hops(path).is_empty());
            assert!(matches!(
                (path.from, path.to),
                (InternalPin::Block(_), InternalPin::Primitive { .. })
                    | (InternalPin::Primitive { .. }, InternalPin::Block(_))
            ));
        }
    }

    #[test]
    fn pins_that_name_nothing_are_counted() {
        let text = "<block name='t' instance='FPGA_packed_netlist[0]'>\
            <block name='a' instance='clb[0]' mode='m'>\
            <inputs><port name='I'>x</port></inputs>\
            <outputs><port name='O'>lut[0].out[0]-&gt;o</port></outputs>\
            <block name='a' instance='lut[0]'>\
            <inputs><port name='in'>clb.I[0]-&gt;i clb.I[3]-&gt;i ble.I[0]-&gt;i fle[2].out[0]-&gt;i nonsense</port></inputs>\
            <outputs><port name='out'>a</port></outputs></block>\
            </block></block>";
        let netlist = read_netlist(text.as_bytes()).unwrap();
        assert_eq!(netlist.unresolved_pins(), 4);
        assert_eq!(netlist.internal_paths().len(), 2);
    }

    #[test]
    fn empty_blocks_are_left_out() {
        let text = "<block name='t' instance='FPGA_packed_netlist[0]'>\
            <block name='open' instance='clb[0]'/>\
            <block name='a' instance='clb[1]' mode='m'>\
            <inputs><port name='I'>n open</port></inputs></block>\
            </block>";
        let netlist = read_netlist(text.as_bytes()).unwrap();
        assert_eq!(netlist.blocks().len(), 1);
        assert_eq!(netlist.nets()[0].sinks[0].block, 0);
    }

    #[test]
    fn output_from_own_input_is_counted_and_has_no_net() {
        let text = "<block name='t' instance='FPGA_packed_netlist[0]'>\
            <block name='a' instance='clb[0]' mode='m'>\
            <inputs><port name='I'>n</port></inputs>\
            <outputs><port name='O'>clb.I[0]-&gt;through</port></outputs></block>\
            </block>";
        let netlist = read_netlist(text.as_bytes()).unwrap();
        assert_eq!(netlist.route_through_outputs(), 1);
        assert_eq!(netlist.nets().len(), 1);
        assert_eq!(netlist.nets()[0].driver, None);
    }

    #[test]
    fn rejects_what_is_not_a_netlist() {
        for (text, line) in [
            ("<rr_graph/>", 1),
            ("\n<block name='t' instance='clb[0]'/>", 2),
            ("<block instance='FPGA_packed_netlist[0]'/>", 1),
            ("<block name='t'/>", 1),
            (
                "<block name='t' instance='FPGA_packed_netlist[0]'>\n\
                 <block name='a' instance='clb[0]'>\n<inputs><port>x</port></inputs></block></block>",
                3,
            ),
        ] {
            let error = read_netlist(text.as_bytes()).unwrap_err();
            assert_eq!(error.line(), Some(line), "for {text:?}: {error}");
        }
    }

    #[test]
    fn rejects_a_net_with_two_drivers() {
        let text = "<block name='t' instance='FPGA_packed_netlist[0]'>\
            <block name='a' instance='clb[0]'><outputs><port name='O'>n</port></outputs></block>\n\
            <block name='b' instance='clb[1]'><outputs><port name='O'>n</port></outputs></block>\
            </block>";
        let error = read_netlist(text.as_bytes()).unwrap_err();
        assert!(
            matches!(error, ImportError::MultipleDrivers { line: 2, .. }),
            "{error}"
        );
    }

    proptest! {
        #[test]
        fn random_input_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = read_netlist(bytes.as_slice());
        }

        #[test]
        fn truncated_genuine_files_are_errors(cut: prop::sample::Index) {
            let end = UNCONNECTED.trim_end().len();
            let at = cut.index(end);
            prop_assert!(read_netlist(&UNCONNECTED.as_bytes()[..at]).is_err());
        }

        #[test]
        fn damaged_genuine_files_never_panic(
            edits in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8),
        ) {
            let mut bytes = UNCONNECTED.as_bytes().to_vec();
            for (at, byte) in edits {
                let at = at.index(bytes.len());
                bytes[at] = byte;
            }
            let _ = read_netlist(bytes.as_slice());
        }

        #[test]
        fn buffer_size_does_not_matter(capacity in 1usize..64) {
            let whole = read_netlist(UNCONNECTED.as_bytes()).unwrap();
            let chunked =
                read_netlist(BufReader::with_capacity(capacity, UNCONNECTED.as_bytes())).unwrap();
            prop_assert_eq!(whole, chunked);
        }
    }
}
