//! Packed netlists: VPR's `.net` format.
//!
//! A packed netlist is a tree of `block` elements: the root stands for the circuit, its
//! children are the blocks a placer places, and everything further down is how those are
//! filled in. Only the children of the root connect to the routing, so only they become part
//! of the result, but their outputs have to be traced through what's below them to find out
//! which nets they carry.

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
}

impl Frame {
    fn new(instance: &str, placed: Option<u32>, is_leaf: bool) -> Frame {
        Frame {
            instance: instance.to_owned(),
            placed,
            is_leaf,
            inputs: Vec::new(),
            outputs: Vec::new(),
            clocks: Vec::new(),
            below: HashMap::new(),
        }
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
                            let pb_type = instance
                                .split_once('[')
                                .map_or(instance, |(pb_type, _)| pb_type);
                            placed = Some(netlist.blocks.len() as u32);
                            netlist.blocks.push(PackedBlock {
                                name: name.to_owned(),
                                pb_type: pb_type.to_owned(),
                            });
                        }
                        frames.push(Frame::new(instance, placed, tag.attr("mode").is_none()));
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
                                }
                            }
                            _ => {
                                if let Some(parent) = frames.last_mut() {
                                    pass_outputs_up(&frame, parent);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
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
