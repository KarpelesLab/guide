//! Architecture files: VPR's `arch.xml`, read for the delays inside blocks.
//!
//! [`read_architecture`] reads what [`build_timing`](super::build_timing) needs and skips
//! everything else: the device's layout, its routing and power are in the routing resource
//! graph or don't matter here.

use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;

use super::ImportError;
use super::timing::{LogicDelays, PrimitiveModel, SequentialDelays};
use super::xml::{Event, Tag, XmlReader};

/// What a primitive `pb_type` of an architecture is, by its `blif_model`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PrimitiveKind {
    /// `.input`: an input pad.
    InputPad,
    /// `.output`: an output pad.
    OutputPad,
    /// `.names`, or a `.subckt` whose model has no clocked port.
    Combinational,
    /// `.latch`, or a `.subckt` whose model has a clocked port.
    Sequential,
}

/// A primitive `pb_type` of an architecture.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ArchPrimitive {
    /// The name that a `.net` file gives it, as in `lut` or `ff`.
    pub pb_type: String,
    /// What it is.
    pub kind: PrimitiveKind,
}

/// What [`read_architecture`] took from an architecture file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Architecture {
    delays: LogicDelays,
    primitives: Vec<ArchPrimitive>,
    interconnects: u32,
}

impl Architecture {
    /// Returns the delays inside blocks, and what each primitive does, for
    /// [`build_timing`](super::build_timing).
    pub fn delays(&self) -> &LogicDelays {
        &self.delays
    }

    /// Returns the delays, to change or add to them.
    pub fn delays_mut(&mut self) -> &mut LogicDelays {
        &mut self.delays
    }

    /// Takes the delays.
    pub fn into_delays(self) -> LogicDelays {
        self.delays
    }

    /// Returns the primitives, in file order, with the ones VPR makes up for a LUT or memory
    /// `class` right after the `pb_type` they come from.
    pub fn primitives(&self) -> &[ArchPrimitive] {
        &self.primitives
    }

    /// Returns how many interconnects were read, counting those that VPR makes up for a
    /// `class`.
    pub fn interconnects(&self) -> u32 {
        self.interconnects
    }
}

/// A port of a model.
#[derive(Clone, Debug, Default)]
struct ModelPort {
    name: String,
    /// Clocked by a clock port, as `clock="clk"` says.
    clocked: bool,
    sinks: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct Model {
    inputs: Vec<ModelPort>,
    outputs: Vec<ModelPort>,
}

/// A port of a `pb_type`.
#[derive(Clone, Debug)]
struct PbPort {
    name: String,
    pins: u32,
    class: Option<String>,
}

/// A timing annotation on a primitive `pb_type`, in picoseconds.
#[derive(Clone, Debug)]
enum Annotation {
    Constant {
        delay: u32,
        inputs: String,
        outputs: String,
        line: u64,
    },
    Matrix {
        delays: Vec<u32>,
        inputs: String,
        outputs: String,
        line: u64,
    },
    Setup(u32),
    Hold(u32),
    ClockToQ(u32),
}

#[derive(Clone, Debug)]
struct PbType {
    name: String,
    blif_model: Option<String>,
    class: Option<String>,
    ports: Vec<PbPort>,
    annotations: Vec<Annotation>,
    line: u64,
}

/// An `<interconnect>` child being read.
#[derive(Clone, Debug)]
struct PendingInterconnect {
    container: String,
    name: String,
    input: String,
    output: String,
}

/// A `<delay_matrix>` being read, whose text is the matrix.
#[derive(Clone, Debug)]
struct PendingMatrix {
    inputs: String,
    outputs: String,
    text: String,
    line: u64,
    /// `type="min"`, which is skipped.
    skip: bool,
}

/// Where the reader is.
#[derive(Clone, Debug)]
enum Context {
    Root,
    Models,
    Model(String),
    ModelPorts {
        output: bool,
    },
    BlockList,
    PbType,
    Mode,
    Interconnects,
    Interconnect,
    Matrix,
    /// Anything else, and everything inside it.
    Skip,
}

fn invalid(line: u64, what: &'static str, value: &str) -> ImportError {
    ImportError::InvalidValue {
        line,
        what,
        value: value.chars().take(64).collect(),
    }
}

fn required(tag: &Tag<'_>, attribute: &'static str) -> Result<String, ImportError> {
    tag.attr(attribute)
        .map(str::to_owned)
        .ok_or_else(|| ImportError::MissingAttribute {
            line: tag.line(),
            element: tag.name().to_owned(),
            attribute,
        })
}

/// Converts a time in seconds, as an architecture file writes it, to whole picoseconds,
/// rounded to the nearest and halves away from zero.
///
/// This works on the decimal digits as written, never through a float, so it is exact: `4.243e-11`
/// is 42.43 ps and becomes 42, `1.25e-12` becomes 1, and `2.5e-12` becomes 3. It takes an
/// optional sign, digits with an optional point, and an optional exponent. A negative time
/// other than zero, or one above `u32::MAX` picoseconds (about 4.3 ms), is `None`.
pub(crate) fn picoseconds(text: &str) -> Option<u32> {
    let text = text.trim();
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(at) => (&text[..at], text[at + 1..].parse::<i64>().ok()?),
        None => (text, 0),
    };
    let (negative, mantissa) = match mantissa.as_bytes().first() {
        Some(b'-') => (true, &mantissa[1..]),
        Some(b'+') => (false, &mantissa[1..]),
        _ => (false, mantissa),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole
        .bytes()
        .chain(fraction.bytes())
        .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let digits: String = whole.chars().chain(fraction.chars()).collect();
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(0);
    }
    if negative {
        return None;
    }
    // The value is `digits * 10^scale` picoseconds.
    let scale = exponent
        .checked_add(12)?
        .checked_sub(i64::try_from(fraction.len()).ok()?)?;
    if scale >= 0 {
        // More than 10 digits is above `u32::MAX`, whatever they are.
        if digits.len() as i64 + scale > 10 {
            return None;
        }
        let value = digits.parse::<u64>().ok()? * 10u64.pow(scale as u32);
        return u32::try_from(value).ok();
    }
    let kept = digits.len() as i64 + scale;
    if kept < 0 {
        // Below a tenth of a picosecond.
        return Some(0);
    }
    let kept = kept as usize;
    if kept > 10 {
        return None;
    }
    let whole: u64 = if kept == 0 {
        0
    } else {
        digits[..kept].parse().ok()?
    };
    let round_up = digits.as_bytes()[kept] >= b'5';
    u32::try_from(whole + u64::from(round_up)).ok()
}

/// The lowest and highest pin of a range, as in `[3:0]`.
type PinRange = (u32, u32);

/// One reference to ports in an annotation, such as `fle[9:0].out` or `lut6.in[2:0]`: the
/// type, the port, and the pins if they are given, lowest first.
fn parse_port_ref(text: &str) -> Option<(&str, &str, Option<PinRange>)> {
    let (instance, port) = text.split_once('.')?;
    let pb_type = instance.split_once('[').map_or(instance, |(name, _)| name);
    let (port, range) = match port.split_once('[') {
        Some((name, rest)) => {
            let rest = rest.strip_suffix(']')?;
            let (high, low) = match rest.split_once(':') {
                Some((high, low)) => (high.trim().parse().ok()?, low.trim().parse().ok()?),
                None => {
                    let bit = rest.trim().parse().ok()?;
                    (bit, bit)
                }
            };
            let (high, low): (u32, u32) = (high, low);
            (name, Some((high.min(low), high.max(low))))
        }
        None => (port, None),
    };
    (!pb_type.is_empty() && !port.is_empty()).then_some((pb_type, port, range))
}

/// Lists the pins that the references in `text` name, as port and position, in order.
fn expand_pins(text: &str, ports: &[PbPort], line: u64) -> Result<Vec<(String, u32)>, ImportError> {
    let mut pins = Vec::new();
    for token in text.split_ascii_whitespace() {
        let (_, port, range) = parse_port_ref(token).ok_or_else(|| invalid(line, "port", token))?;
        let declared = ports
            .iter()
            .find(|declared| declared.name == port)
            .ok_or_else(|| invalid(line, "port", token))?;
        let (low, high) = match range {
            Some((low, high)) if high < declared.pins => (low, high),
            Some(_) => return Err(invalid(line, "port", token)),
            None if declared.pins == 0 => continue,
            None => (0, declared.pins - 1),
        };
        for bit in low..=high {
            pins.push((port.to_owned(), bit));
        }
    }
    Ok(pins)
}

/// The `type.port` of each reference in `text`, without indices.
fn port_names(text: &str, line: u64) -> Result<Vec<String>, ImportError> {
    text.split_ascii_whitespace()
        .map(|token| {
            parse_port_ref(token)
                .map(|(pb_type, port, _)| format!("{pb_type}.{port}"))
                .ok_or_else(|| invalid(line, "port", token))
        })
        .collect()
}

/// Reads one delay attribute, in seconds, as picoseconds.
fn delay_attr(tag: &Tag<'_>, attribute: &'static str) -> Result<Option<u32>, ImportError> {
    match tag.attr(attribute) {
        Some(value) => picoseconds(value)
            .map(Some)
            .ok_or_else(|| invalid(tag.line(), "delay", value)),
        None => Ok(None),
    }
}

/// Builds what [`read_architecture`] returns, as it goes.
#[derive(Default)]
struct Reader {
    arch: Architecture,
    models: BTreeMap<String, Model>,
    /// The `pb_type`s open, outermost first.
    pb_types: Vec<PbType>,
    interconnect: Option<PendingInterconnect>,
    matrix: Option<PendingMatrix>,
    /// Interconnect delays already set, to keep the largest when names repeat across modes.
    seen_interconnects: BTreeSet<(String, String)>,
}

impl Reader {
    fn add_interconnect(&mut self, container: &str, name: &str) {
        if self
            .seen_interconnects
            .insert((container.to_owned(), name.to_owned()))
        {
            self.arch.interconnects += 1;
            // VPR gives no delay to an edge that no annotation covers.
            self.arch.delays.set_interconnect(container, name, 0);
        }
    }

    /// Sets an interconnect's delay between ports, keeping the largest if a mode with an
    /// interconnect of the same name already set one.
    fn set_interconnect_delay(
        &mut self,
        interconnect: &PendingInterconnect,
        (inputs, outputs): (&str, &str),
        delay: u32,
        line: u64,
    ) -> Result<(), ImportError> {
        let (container, name) = (&interconnect.container, &interconnect.name);
        let inputs = if inputs.is_empty() {
            &interconnect.input
        } else {
            inputs
        };
        let outputs = if outputs.is_empty() {
            &interconnect.output
        } else {
            outputs
        };
        for from in port_names(inputs, line)? {
            for to in port_names(outputs, line)? {
                let delays = &mut self.arch.delays;
                let known = delays
                    .interconnect_ports(container, name, &from, &to)
                    .unwrap_or(0);
                delays.set_interconnect_ports(container, name, &from, &to, known.max(delay));
            }
        }
        Ok(())
    }

    fn start(&mut self, context: &Context, tag: &Tag<'_>) -> Result<Context, ImportError> {
        let line = tag.line();
        Ok(match (context, tag.name()) {
            (Context::Root, "models") => Context::Models,
            (Context::Root, "complexblocklist") => Context::BlockList,
            (Context::Models, "model") => {
                let name = required(tag, "name")?;
                self.models.insert(name.clone(), Model::default());
                Context::Model(name)
            }
            (Context::Model(_), "input_ports") => Context::ModelPorts { output: false },
            (Context::Model(_), "output_ports") => Context::ModelPorts { output: true },
            (Context::ModelPorts { .. }, "port") => {
                // Handled by the caller, which knows the model.
                Context::Skip
            }
            (Context::BlockList | Context::PbType | Context::Mode, "pb_type") => {
                self.pb_types.push(PbType {
                    name: required(tag, "name")?,
                    blif_model: tag.attr("blif_model").map(str::to_owned),
                    class: tag.attr("class").map(str::to_owned),
                    ports: Vec::new(),
                    annotations: Vec::new(),
                    line,
                });
                Context::PbType
            }
            (Context::PbType, "input" | "output" | "clock") => {
                let pins = required(tag, "num_pins")?;
                let pins = pins.parse().map_err(|_| invalid(line, "num_pins", &pins))?;
                let port = PbPort {
                    name: required(tag, "name")?,
                    pins,
                    class: tag.attr("port_class").map(str::to_owned),
                };
                if let Some(pb_type) = self.pb_types.last_mut() {
                    pb_type.ports.push(port);
                }
                Context::Skip
            }
            (Context::PbType, "mode") => Context::Mode,
            (Context::PbType | Context::Mode, "interconnect") => Context::Interconnects,
            (Context::Interconnects, "direct" | "mux" | "complete") => {
                let container = self
                    .pb_types
                    .last()
                    .map(|pb_type| pb_type.name.clone())
                    .unwrap_or_default();
                let interconnect = PendingInterconnect {
                    name: required(tag, "name")?,
                    input: required(tag, "input")?,
                    output: required(tag, "output")?,
                    container,
                };
                self.add_interconnect(&interconnect.container, &interconnect.name);
                self.interconnect = Some(interconnect);
                Context::Interconnect
            }
            (Context::Interconnect, "delay_constant") => {
                if let (Some(delay), Some(interconnect)) =
                    (delay_attr(tag, "max")?, self.interconnect.clone())
                {
                    let inputs = tag.attr("in_port").unwrap_or_default();
                    let outputs = tag.attr("out_port").unwrap_or_default();
                    self.set_interconnect_delay(&interconnect, (inputs, outputs), delay, line)?;
                }
                Context::Skip
            }
            (Context::PbType, "delay_constant") => {
                if let (Some(delay), Some(pb_type)) =
                    (delay_attr(tag, "max")?, self.pb_types.last_mut())
                {
                    pb_type.annotations.push(Annotation::Constant {
                        delay,
                        inputs: required(tag, "in_port")?,
                        outputs: required(tag, "out_port")?,
                        line,
                    });
                }
                Context::Skip
            }
            (Context::PbType | Context::Interconnect, "delay_matrix") => {
                self.matrix = Some(PendingMatrix {
                    inputs: tag.attr("in_port").unwrap_or_default().to_owned(),
                    outputs: tag.attr("out_port").unwrap_or_default().to_owned(),
                    text: String::new(),
                    line,
                    skip: tag.attr("type") != Some("max"),
                });
                Context::Matrix
            }
            (Context::PbType, "T_setup" | "T_hold" | "T_clock_to_Q") => {
                let attribute = if tag.name() == "T_clock_to_Q" {
                    "max"
                } else {
                    "value"
                };
                if let (Some(delay), Some(pb_type)) =
                    (delay_attr(tag, attribute)?, self.pb_types.last_mut())
                {
                    pb_type.annotations.push(match tag.name() {
                        "T_setup" => Annotation::Setup(delay),
                        "T_hold" => Annotation::Hold(delay),
                        _ => Annotation::ClockToQ(delay),
                    });
                }
                Context::Skip
            }
            _ => Context::Skip,
        })
    }

    fn read_model_port(
        &mut self,
        model: &str,
        output: bool,
        tag: &Tag<'_>,
    ) -> Result<(), ImportError> {
        let port = ModelPort {
            name: required(tag, "name")?,
            // A clock port is clocked by nothing, and has an ideal clock on it.
            clocked: tag.attr("clock").is_some_and(|clock| !clock.is_empty())
                && tag.attr("is_clock") != Some("1"),
            sinks: tag
                .attr("combinational_sink_ports")
                .unwrap_or_default()
                .split_ascii_whitespace()
                .map(str::to_owned)
                .collect(),
        };
        if let Some(model) = self.models.get_mut(model) {
            if output {
                model.outputs.push(port);
            } else {
                model.inputs.push(port);
            }
        }
        Ok(())
    }

    fn end(&mut self, context: Context) -> Result<(), ImportError> {
        match context {
            Context::PbType => {
                if let Some(pb_type) = self.pb_types.pop() {
                    self.finish_pb_type(pb_type)?;
                }
            }
            Context::Interconnect => self.interconnect = None,
            Context::Matrix => {
                if let Some(matrix) = self.matrix.take() {
                    self.finish_matrix(matrix)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish_matrix(&mut self, matrix: PendingMatrix) -> Result<(), ImportError> {
        if matrix.skip {
            return Ok(());
        }
        let mut delays = Vec::new();
        for value in matrix.text.split_ascii_whitespace() {
            delays.push(picoseconds(value).ok_or_else(|| invalid(matrix.line, "delay", value))?);
        }
        if let Some(interconnect) = self.interconnect.clone() {
            // By port, not by pin: the largest of the matrix.
            let delay = delays.iter().copied().max().unwrap_or(0);
            let ports = (matrix.inputs.as_str(), matrix.outputs.as_str());
            return self.set_interconnect_delay(&interconnect, ports, delay, matrix.line);
        }
        if let Some(pb_type) = self.pb_types.last_mut() {
            pb_type.annotations.push(Annotation::Matrix {
                delays,
                inputs: matrix.inputs,
                outputs: matrix.outputs,
                line: matrix.line,
            });
        }
        Ok(())
    }

    /// Works out what a primitive is from its `blif_model`.
    fn classify(
        &self,
        pb_type: &PbType,
        blif_model: &str,
    ) -> Result<(PrimitiveKind, Option<PrimitiveModel>), ImportError> {
        Ok(match blif_model {
            ".input" => (PrimitiveKind::InputPad, None),
            ".output" => (PrimitiveKind::OutputPad, None),
            ".names" => (
                PrimitiveKind::Combinational,
                Some(PrimitiveModel::combinational()),
            ),
            ".latch" => (
                PrimitiveKind::Sequential,
                Some(PrimitiveModel::new().with_clocked("D").with_clocked("Q")),
            ),
            _ => {
                let name = blif_model
                    .strip_prefix(".subckt")
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| invalid(pb_type.line, "blif_model", blif_model))?;
                let model = self
                    .models
                    .get(name)
                    .ok_or_else(|| ImportError::Malformed {
                        line: pb_type.line,
                        message: format!(
                            "pb_type {} uses the undefined model {name}",
                            pb_type.name
                        ),
                    })?;
                let mut primitive = PrimitiveModel::new();
                for port in model.inputs.iter().chain(&model.outputs) {
                    if port.clocked {
                        primitive = primitive.with_clocked(&port.name);
                    }
                }
                for port in &model.inputs {
                    for sink in &port.sinks {
                        primitive = primitive.with_path(&port.name, sink);
                    }
                }
                let kind = if primitive.is_sequential() {
                    PrimitiveKind::Sequential
                } else {
                    PrimitiveKind::Combinational
                };
                (kind, Some(primitive))
            }
        })
    }

    fn finish_pb_type(&mut self, pb_type: PbType) -> Result<(), ImportError> {
        let Some(blif_model) = pb_type.blif_model.clone() else {
            return Ok(());
        };
        let (kind, model) = self.classify(&pb_type, &blif_model)?;
        // What VPR makes of a `class`: see `ProcessLutClass` and `ProcessMemoryClass` in
        // `libarchfpga/src/arch_util.cpp`. The annotations move to a child of a made-up name,
        // which is the primitive that a `.net` file names.
        let child = match pb_type.class.as_deref() {
            Some("lut") if pb_type.name == "lut" => Some("lut_child"),
            Some("lut") => Some("lut"),
            Some("memory") if pb_type.name == "memory_slice" => Some("memory_slice_1bit"),
            Some("memory") => Some("memory_slice"),
            _ => None,
        };
        let mut names = vec![pb_type.name.clone()];
        names.extend(child.map(str::to_owned));
        for name in &names {
            self.arch.primitives.push(ArchPrimitive {
                pb_type: name.clone(),
                kind,
            });
            if let Some(model) = &model {
                self.arch.delays.set_model(name, model.clone());
            }
            self.apply_annotations(name, &pb_type)?;
        }

        match pb_type.class.as_deref() {
            Some("lut") => {
                // The `wire` mode's interconnect, which carries the LUT's own delays, and
                // the `lut` mode's, which has none.
                let interconnect = PendingInterconnect {
                    container: pb_type.name.clone(),
                    name: format!("complete:{}", pb_type.name),
                    input: String::new(),
                    output: String::new(),
                };
                self.add_interconnect(&interconnect.container, &interconnect.name);
                let direct = format!("direct:{}", pb_type.name);
                self.add_interconnect(&pb_type.name, &direct);
                for annotation in &pb_type.annotations {
                    let (delay, inputs, outputs, line) = match annotation {
                        Annotation::Constant {
                            delay,
                            inputs,
                            outputs,
                            line,
                        } => (*delay, inputs, outputs, *line),
                        Annotation::Matrix {
                            delays,
                            inputs,
                            outputs,
                            line,
                        } => (
                            delays.iter().copied().max().unwrap_or(0),
                            inputs,
                            outputs,
                            *line,
                        ),
                        _ => continue,
                    };
                    let ports = (inputs.as_str(), outputs.as_str());
                    self.set_interconnect_delay(&interconnect, ports, delay, line)?;
                }
            }
            Some("memory") => {
                let data_pins = pb_type
                    .ports
                    .iter()
                    .find(|port| {
                        port.class
                            .as_deref()
                            .is_some_and(|class| class.starts_with("data"))
                    })
                    .map_or(1, |port| port.pins);
                let count = (pb_type.ports.len() as u32).saturating_mul(data_pins);
                for index in 0..count {
                    self.add_interconnect(&pb_type.name, &format!("direct:{index}"));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Puts a primitive's annotations into the delays under `name`.
    fn apply_annotations(&mut self, name: &str, pb_type: &PbType) -> Result<(), ImportError> {
        let delays = &mut self.arch.delays;
        let mut sequential: Option<SequentialDelays> = None;
        let mut hold: Option<u32> = None;
        for annotation in &pb_type.annotations {
            match annotation {
                Annotation::Constant {
                    delay,
                    inputs,
                    outputs,
                    line,
                } => {
                    for input in port_names(inputs, *line)? {
                        for output in port_names(outputs, *line)? {
                            let input = input.split_once('.').map_or("", |(_, port)| port);
                            let output = output.split_once('.').map_or("", |(_, port)| port);
                            delays.set_combinational_ports(name, input, output, *delay);
                        }
                    }
                }
                Annotation::Matrix {
                    delays: matrix,
                    inputs,
                    outputs,
                    line,
                } => {
                    let rows = expand_pins(inputs, &pb_type.ports, *line)?;
                    let columns = expand_pins(outputs, &pb_type.ports, *line)?;
                    if rows.len().checked_mul(columns.len()) != Some(matrix.len()) {
                        return Err(ImportError::Malformed {
                            line: *line,
                            message: format!(
                                "delay_matrix has {} values for {} inputs and {} outputs",
                                matrix.len(),
                                rows.len(),
                                columns.len()
                            ),
                        });
                    }
                    // Rows are inputs and columns outputs. A pin the matrix doesn't cover gets
                    // the largest of its port's.
                    let mut largest: BTreeMap<(&str, &str), u32> = BTreeMap::new();
                    for (row, (input, input_bit)) in rows.iter().enumerate() {
                        for (column, (output, output_bit)) in columns.iter().enumerate() {
                            let delay = matrix[row * columns.len() + column];
                            delays.set_combinational_pins(
                                name,
                                (input, *input_bit),
                                (output, *output_bit),
                                delay,
                            );
                            let entry = largest.entry((input, output)).or_insert(0);
                            *entry = (*entry).max(delay);
                        }
                    }
                    for ((input, output), delay) in largest {
                        let known = delays.combinational(name, input, output).unwrap_or(0);
                        delays.set_combinational_ports(name, input, output, known.max(delay));
                    }
                }
                Annotation::Setup(delay) => {
                    let entry = sequential.get_or_insert_default();
                    entry.setup = entry.setup.max(*delay);
                }
                Annotation::ClockToQ(delay) => {
                    let entry = sequential.get_or_insert_default();
                    entry.clock_to_q = entry.clock_to_q.max(*delay);
                }
                Annotation::Hold(delay) => hold = Some(hold.unwrap_or(0).max(*delay)),
            }
        }
        if let Some(sequential) = sequential {
            delays.set_sequential(name, sequential);
        }
        if let Some(hold) = hold {
            delays.set_hold(name, hold);
        }
        Ok(())
    }
}

/// Reads the delays inside blocks from a VPR architecture file, and what each primitive is.
///
/// # What is read
///
/// | Element | Becomes |
/// | --- | --- |
/// | `<model>`, with `clock` and `combinational_sink_ports` on its ports | A [`PrimitiveModel`] for each primitive with `blif_model=".subckt name"` |
/// | `blif_model` `.names` and `.latch` | [`PrimitiveModel::combinational`], and a register clocked on `D` and `Q` |
/// | `delay_constant` (`max`) on an interconnect | [`LogicDelays::set_interconnect_ports`], for every `in_port` and `out_port` named |
/// | `delay_matrix` (`type="max"`) on an interconnect | The same, with the largest delay of the matrix |
/// | `delay_constant` (`max`) on a primitive | [`LogicDelays::set_combinational_ports`] |
/// | `delay_matrix` (`type="max"`) on a primitive | [`LogicDelays::set_combinational_pins`], a pin per row and column, and the largest of each pair of ports for [`LogicDelays::set_combinational_ports`] |
/// | `T_setup` (`value`) and `T_clock_to_Q` (`max`) | [`LogicDelays::set_sequential`], the largest over the primitive's ports |
/// | `T_hold` (`value`) | [`LogicDelays::set_hold`] |
///
/// Every interconnect is given a delay of 0 between ports that no annotation covers, as VPR
/// does, so none falls back on [`LogicDelays::default_interconnect`]. Everything else in the
/// file is skipped, unknown elements included. `min` delays are skipped too, as there is no
/// hold analysis.
///
/// # Units
///
/// The file gives times in seconds, as decimal text such as `4.243e-11`. They become whole
/// picoseconds, rounded to the nearest and halves up, from the digits as written and never
/// through a float, so the result is exact and the same everywhere: `4.243e-11` is 42.43 ps,
/// and becomes 42. A file's delays are usually whole picoseconds, or close; `k6_N10_40nm`
/// has `4.243e-11` and `1.394e-11` on its pads, which lose 0.43 ps and gain 0.06 ps.
///
/// # What VPR does with a `class`
///
/// VPR rewrites a `pb_type` of `class="lut"` into a block with a `wire` mode, whose
/// interconnect `complete:name` carries the LUT's delays, and a mode with a primitive called
/// `lut` that has them (or `lut_child` if the `pb_type` is itself `lut`), joined by
/// interconnect `direct:name`. A `.net` file names those, so they are read the same way. The
/// same goes for `class="memory"`, whose primitive is `memory_slice`.
///
/// # Differences from VPR
///
/// - **Interconnect delays are by port, not by pin or instance.** An annotation that names
///   `fle[9:0].in` applies to every `fle`, and a `delay_matrix` on an interconnect counts as
///   its largest value. A `wire`-mode LUT's delay is the largest of its inputs. So these are
///   never optimistic.
/// - **Names of interconnect only have to be unique within a `pb_type`**, while VPR allows
///   the same name in two modes of one `pb_type`. Where those have different delays between
///   the same ports, the larger is kept.
/// - **A register has one setup and one clock-to-Q time**, the largest over its ports, since
///   that is what [`build_timing`](super::build_timing) has. Each port may differ in VPR.
/// - **A pin range in a primitive's `delay_matrix`**, such as `lut.in[3:0]`, is read from the
///   lowest pin up.
///
/// # Errors
///
/// Fails if the XML is malformed or a read fails, if a delay is negative or above `u32::MAX`
/// picoseconds, if a primitive uses a model that isn't defined, or if a `delay_matrix` names
/// ports that the `pb_type` doesn't have or doesn't have as many values as they have pins.
///
/// # Examples
///
/// ```
/// use guide::fpga::{PrimitiveKind, read_architecture};
///
/// let text = r#"<architecture>
///   <models/>
///   <layout><auto_layout aspect_ratio="1.0"/></layout>
///   <complexblocklist>
///     <pb_type name="clb">
///       <input name="I" num_pins="4"/><output name="O" num_pins="1"/>
///       <pb_type name="lut4" blif_model=".names" class="lut">
///         <input name="in" num_pins="4" port_class="lut_in"/>
///         <output name="out" num_pins="1" port_class="lut_out"/>
///         <delay_matrix type="max" in_port="lut4.in" out_port="lut4.out">
///           1e-10 2e-10 3e-10 2.5e-10
///         </delay_matrix>
///       </pb_type>
///       <interconnect>
///         <complete name="crossbar" input="clb.I" output="lut4.in">
///           <delay_constant max="9.5e-11" in_port="clb.I" out_port="lut4.in"/>
///         </complete>
///         <direct name="out" input="lut4.out" output="clb.O"/>
///       </interconnect>
///     </pb_type>
///   </complexblocklist>
/// </architecture>"#;
/// let arch = read_architecture(text.as_bytes())?;
/// let delays = arch.delays();
/// // The primitive of a `.net` file is the `lut` that VPR makes up inside `lut4`.
/// assert_eq!(delays.combinational_pins("lut", ("in", 3), ("out", 0)), Some(250));
/// assert_eq!(delays.combinational("lut", "in", "out"), Some(300));
/// assert_eq!(delays.interconnect_ports("clb", "crossbar", "clb.I", "lut4.in"), Some(95));
/// assert_eq!(delays.interconnect_ports("clb", "out", "lut4.out", "clb.O"), Some(0));
/// assert_eq!(arch.primitives()[1].pb_type, "lut");
/// assert_eq!(arch.primitives()[1].kind, PrimitiveKind::Combinational);
/// # Ok::<(), guide::fpga::ImportError>(())
/// ```
pub fn read_architecture<R: BufRead>(reader: R) -> Result<Architecture, ImportError> {
    let mut xml = XmlReader::new(reader);
    let mut reader = Reader::default();
    let mut stack: Vec<Context> = Vec::new();
    while let Some(event) = xml.next_event()? {
        match event {
            Event::Start(tag) => {
                let context = match stack.last() {
                    None => {
                        if tag.name() != "architecture" {
                            return Err(ImportError::Malformed {
                                line: tag.line(),
                                message: format!("expected <architecture>, found <{}>", tag.name()),
                            });
                        }
                        Context::Root
                    }
                    Some(Context::Skip | Context::Matrix) => Context::Skip,
                    Some(Context::ModelPorts { output }) if tag.name() == "port" => {
                        let output = *output;
                        let model = stack.iter().rev().find_map(|context| match context {
                            Context::Model(name) => Some(name.clone()),
                            _ => None,
                        });
                        if let Some(model) = model {
                            reader.read_model_port(&model, output, &tag)?;
                        }
                        Context::Skip
                    }
                    Some(context) => reader.start(context, &tag)?,
                };
                stack.push(context);
            }
            Event::Text(text) => {
                if let (Some(Context::Matrix), Some(matrix)) = (stack.last(), &mut reader.matrix) {
                    matrix.text.push(' ');
                    matrix.text.push_str(text);
                }
            }
            Event::End => {
                if let Some(context) = stack.pop() {
                    reader.end(context)?;
                }
            }
        }
    }
    Ok(reader.arch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_become_picoseconds_exactly() {
        for (text, expected) in [
            ("0", Some(0)),
            ("0.", Some(0)),
            ("-0", Some(0)),
            ("58e-12", Some(58)),
            ("7.247000e-11", Some(72)),
            ("4.243e-11", Some(42)),
            ("1.394e-11", Some(14)),
            ("2.5e-12", Some(3)),
            ("2.4999e-12", Some(2)),
            ("1.25e-12", Some(1)),
            ("0.5e-12", Some(1)),
            ("0.04e-12", Some(0)),
            ("1e-15", Some(0)),
            ("+1E-9", Some(1000)),
            ("261e-12", Some(261)),
            ("  82e-12 ", Some(82)),
            ("4.294967295e-3", Some(u32::MAX)),
            ("4.2949672955e-3", None),
            ("4.294967296e-3", None),
            ("1", None),
            ("1e300", None),
            ("1e-300", Some(0)),
            ("0e300", Some(0)),
            ("-1e-12", None),
            ("", None),
            (".", None),
            ("e-12", None),
            ("1.2.3", None),
            ("1e", None),
            ("NaN", None),
            ("inf", None),
            ("0x10", None),
        ] {
            assert_eq!(picoseconds(text), expected, "{text:?}");
        }
    }

    #[test]
    fn port_references_are_taken_apart() {
        assert_eq!(parse_port_ref("clb.I"), Some(("clb", "I", None)));
        assert_eq!(parse_port_ref("fle[9:0].out"), Some(("fle", "out", None)));
        assert_eq!(
            parse_port_ref("lut.in[3:1]"),
            Some(("lut", "in", Some((1, 3))))
        );
        assert_eq!(
            parse_port_ref("lut.in[2]"),
            Some(("lut", "in", Some((2, 2))))
        );
        assert_eq!(parse_port_ref("lut"), None);
        assert_eq!(parse_port_ref("lut.in[x]"), None);
    }

    const MODELS: &str = r#"<architecture>
      <models>
        <model name="ram">
          <input_ports>
            <port name="addr" clock="clk" combinational_sink_ports="out"/>
            <port name="data" clock="clk"/>
            <port name="clk" is_clock="1"/>
          </input_ports>
          <output_ports><port name="out"/></output_ports>
        </model>
        <model name="adder">
          <input_ports><port name="a" combinational_sink_ports="sum cout"/></input_ports>
          <output_ports><port name="sum"/><port name="cout"/></output_ports>
        </model>
      </models>
      <unknown><pb_type name="ignored" blif_model=".names"/></unknown>
      <complexblocklist>
        <pb_type name="blk">
          <mode name="one">
            <pb_type name="ram" blif_model=".subckt ram">
              <input name="addr" num_pins="2"/><input name="data" num_pins="1"/>
              <clock name="clk" num_pins="1"/><output name="out" num_pins="1"/>
              <T_setup value="5e-11" port="ram.addr" clock="clk"/>
              <T_setup value="6e-11" port="ram.data" clock="clk"/>
              <T_hold value="1e-11" port="ram.data" clock="clk"/>
              <delay_constant max="3e-10" in_port="ram.addr" out_port="ram.out"/>
              <delay_constant min="1e-10" in_port="ram.data" out_port="ram.out"/>
              <some_future_annotation value="1"/>
            </pb_type>
            <pb_type name="add" blif_model=".subckt adder">
              <input name="a" num_pins="2"/><output name="sum" num_pins="1"/>
              <output name="cout" num_pins="1"/>
              <delay_matrix type="max" in_port="add.a" out_port="add.sum add.cout">
                1e-11 2e-11
                3e-11 4e-11
              </delay_matrix>
              <delay_matrix type="min" in_port="add.a" out_port="add.sum">x</delay_matrix>
            </pb_type>
            <interconnect>
              <mux name="m" input="blk.in ram.out" output="add.a">
                <delay_constant max="1e-11" in_port="blk.in" out_port="add.a"/>
                <delay_constant max="2e-11" in_port="ram.out" out_port="add.a"/>
              </mux>
            </interconnect>
          </mode>
          <mode name="two">
            <interconnect>
              <direct name="m" input="blk.in" output="add.a">
                <delay_matrix type="max" in_port="blk.in" out_port="add.a">5e-12 7e-12</delay_matrix>
              </direct>
            </interconnect>
          </mode>
        </pb_type>
      </complexblocklist>
    </architecture>"#;

    #[test]
    fn models_classify_primitives_and_annotations_fill_the_delays() {
        let arch = read_architecture(MODELS.as_bytes()).unwrap();
        let kinds: Vec<_> = arch
            .primitives()
            .iter()
            .map(|primitive| (primitive.pb_type.as_str(), primitive.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("ram", PrimitiveKind::Sequential),
                ("add", PrimitiveKind::Combinational)
            ]
        );
        assert_eq!(arch.interconnects(), 1);
        let delays = arch.delays();

        let ram = delays.model("ram").unwrap();
        assert!(ram.is_clocked("addr") && ram.is_clocked("data"));
        assert!(!ram.is_clocked("clk") && !ram.is_clocked("out"));
        assert!(ram.has_path("addr", "out") && !ram.has_path("data", "out"));
        assert_eq!(
            delays.sequential("ram"),
            Some(SequentialDelays {
                clock_to_q: 0,
                setup: 60
            })
        );
        assert_eq!(delays.hold("ram"), Some(10));
        assert_eq!(delays.combinational("ram", "addr", "out"), Some(300));
        assert_eq!(delays.combinational("ram", "data", "out"), None);

        let add = delays.model("add").unwrap();
        assert!(add.has_path("a", "sum") && add.has_path("a", "cout"));
        assert_eq!(
            delays.combinational_pins("add", ("a", 0), ("sum", 0)),
            Some(10)
        );
        assert_eq!(
            delays.combinational_pins("add", ("a", 0), ("cout", 0)),
            Some(20)
        );
        assert_eq!(
            delays.combinational_pins("add", ("a", 1), ("sum", 0)),
            Some(30)
        );
        assert_eq!(
            delays.combinational_pins("add", ("a", 1), ("cout", 0)),
            Some(40)
        );
        assert_eq!(delays.combinational("add", "a", "cout"), Some(40));

        // The same name in two modes: the larger delay is kept for the same ports.
        assert_eq!(
            delays.interconnect_ports("blk", "m", "blk.in", "add.a"),
            Some(10)
        );
        assert_eq!(
            delays.interconnect_ports("blk", "m", "ram.out", "add.a"),
            Some(20)
        );
        assert_eq!(
            delays.interconnect_ports("blk", "m", "x.y", "add.a"),
            Some(0)
        );
        assert_eq!(delays.interconnect("blk", "m"), Some(0));
        assert_eq!(delays.model("ignored"), None);
    }

    #[test]
    fn bad_files_are_refused() {
        let wrap = |inner: &str| {
            format!(
                "<architecture><complexblocklist><pb_type name=\"p\">{inner}</pb_type></complexblocklist></architecture>"
            )
        };
        let cases = [
            "<pb_type name=\"q\" blif_model=\".subckt nothing\"/>".to_owned(),
            "<pb_type name=\"q\" blif_model=\"weird\"/>".to_owned(),
            "<pb_type name=\"q\" blif_model=\".names\"><input name=\"in\" num_pins=\"2\"/><output name=\"out\" num_pins=\"1\"/><delay_matrix type=\"max\" in_port=\"q.in\" out_port=\"q.out\">1e-12</delay_matrix></pb_type>".to_owned(),
            "<pb_type name=\"q\" blif_model=\".names\"><input name=\"in\" num_pins=\"1\"/><output name=\"out\" num_pins=\"1\"/><delay_matrix type=\"max\" in_port=\"q.nope\" out_port=\"q.out\">1e-12</delay_matrix></pb_type>".to_owned(),
            "<pb_type name=\"q\" blif_model=\".names\"><delay_constant max=\"-1e-12\" in_port=\"q.in\" out_port=\"q.out\"/></pb_type>".to_owned(),
            "<pb_type name=\"q\" blif_model=\".names\"><input name=\"in\" num_pins=\"x\"/></pb_type>".to_owned(),
            "<interconnect><direct name=\"d\" input=\"p.a\"/></interconnect>".to_owned(),
        ];
        for case in cases {
            assert!(read_architecture(wrap(&case).as_bytes()).is_err(), "{case}");
        }
        assert!(read_architecture("<arch/>".as_bytes()).is_err());
        assert!(read_architecture("<architecture>".as_bytes()).is_err());
    }
}
