//! Writing routes in VPR's `.route` format.
//!
//! The layout comes from `print_route` in VPR's `read_route.cpp`, and what's checked is what
//! the same file's reader checks when it loads a route for `--analysis`:
//!
//! - Each net's lines walk its tree depth first. A line's switch is the one leading on to the
//!   next line, and a `SINK` ends a branch with `Switch: -1`. The next line then repeats an
//!   earlier node, which is where the next branch starts, with the switch of that branch.
//! - Nets come in VPR's own order, with its numbers and names.
//! - A `SINK` line ends in `Net_pin_index`, which says which pin of the net it serves.
//! - An `IPIN` or `OPIN` on anything but an I/O tile names the pin. VPR checks that name
//!   against the block placed at that location, whatever the tile's called.
//!
//! The first line has a digest of the placement file, which VPR compares to its own, as
//! digests are checked by default. This module can't work that out, so it's up to the caller.

use core::fmt;
use std::collections::HashMap;
use std::error::Error;
use std::io::{self, Write};

use super::design::{Design, NetStatus};
use super::rr_graph::{NodeKind, RrGraph};
use crate::id::{NetId, NodeId};
use crate::net::{Net, Route, RouteError, RouteSet};

/// Why routes could not be written.
#[derive(Debug)]
#[non_exhaustive]
pub enum RouteFileError {
    /// Writing to the destination failed.
    Io(io::Error),
    /// A header field would break the line-based format, as it's empty or has whitespace in it.
    BadHeader {
        /// The field's contents.
        field: String,
    },
    /// A route isn't a tree, or misses one of its net's terminals.
    InvalidRoute {
        /// The net.
        net: NetId,
        /// What's wrong with the route.
        fault: RouteError<NodeId>,
    },
    /// A route isn't rooted on its net's `SOURCE`, or on one of the `OPIN`s of a driver that
    /// [`Design::driver_source`] has a `SOURCE` for.
    WrongRoot {
        /// The net.
        net: NetId,
        /// The route's root.
        node: NodeId,
    },
    /// A route uses an edge that the graph doesn't have.
    MissingEdge {
        /// The net.
        net: NetId,
        /// The start of the edge.
        from: NodeId,
        /// The end of the edge.
        to: NodeId,
    },
    /// A route uses a node that the graph doesn't have.
    UnknownNode {
        /// The net.
        net: NetId,
        /// The node.
        node: NodeId,
    },
    /// A branch of a route ends somewhere other than a `SINK` of its net, reached through
    /// a configurable switch. VPR only accepts branches that stop at a wire if a
    /// non-configurable switch forced them to go there.
    DanglingBranch {
        /// The net.
        net: NetId,
        /// Where the branch ends.
        node: NodeId,
    },
    /// Two pins of a net need the same `SINK`, so a route, being a tree, can't serve both.
    /// See [`Design::shared_sink_nets`].
    SharedSink {
        /// The net.
        net: NetId,
        /// The `SINK`.
        node: NodeId,
    },
    /// An `IPIN` or `OPIN` in a route is on a position of a tile with no block on it, or has no
    /// usable name in the graph, and VPR would need the name.
    UnnamedPin {
        /// The net.
        net: NetId,
        /// The node.
        node: NodeId,
    },
}

impl fmt::Display for RouteFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteFileError::Io(source) => write!(f, "write failed: {source}"),
            RouteFileError::BadHeader { field } => {
                write!(f, "{field:?} can't be a field of a route file's header")
            }
            RouteFileError::InvalidRoute { net, fault } => {
                write!(f, "net {}: {fault}", net.0)
            }
            RouteFileError::WrongRoot { net, node } => {
                write!(
                    f,
                    "net {}: route starts at node {}, which isn't its SOURCE",
                    net.0, node.0
                )
            }
            RouteFileError::MissingEdge { net, from, to } => write!(
                f,
                "net {}: the graph has no edge from node {} to node {}",
                net.0, from.0, to.0
            ),
            RouteFileError::UnknownNode { net, node } => {
                write!(f, "net {}: the graph has no node {}", net.0, node.0)
            }
            RouteFileError::DanglingBranch { net, node } => write!(
                f,
                "net {}: a branch ends at node {}, which isn't a SINK of the net",
                net.0, node.0
            ),
            RouteFileError::SharedSink { net, node } => write!(
                f,
                "net {}: more than one of its pins needs SINK node {}",
                net.0, node.0
            ),
            RouteFileError::UnnamedPin { net, node } => write!(
                f,
                "net {}: no block or name for the pin at node {}",
                net.0, node.0
            ),
        }
    }
}

impl Error for RouteFileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RouteFileError::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for RouteFileError {
    fn from(source: io::Error) -> RouteFileError {
        RouteFileError::Io(source)
    }
}

/// The header of a route file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteHeader<'a> {
    /// The name of the placement file, for the record.
    pub placement_file: &'a str,
    /// The digest of the placement file, as VPR writes it: `SHA256:` and the lowercase hex of
    /// the SHA-256 of the file's bytes. VPR rejects a route whose digest doesn't match the
    /// placement it loads, unless given `--verify_file_digests off`.
    pub placement_id: &'a str,
}

/// How many nets got each treatment in a route file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RouteSummary {
    /// Nets written with a route.
    pub routed: u32,
    /// Nets that need a route but have none in the [`RouteSet`]. They're written like VPR
    /// does, with their heading alone. VPR will find that the routing isn't complete.
    pub unrouted: u32,
    /// Global nets, written with the blocks that they connect.
    pub global: u32,
    /// Nets with no sinks.
    pub no_sinks: u32,
    /// Nets with no driver, which VPR won't be able to match, since it would have removed them.
    pub undriven: u32,
}

/// Writes routes in the format of VPR's `.route` files.
///
/// This goes through all of `design`'s nets, in order, whether they have a route or not.
/// Routes have to have been found on `device`'s graph.
///
/// VPR 9.0.0 accepts what this writes, as the [module documentation](super) says.
///
/// # Errors
///
/// Fails if a write fails, if the header has a field that doesn't suit the format, or if
/// a route can't be written as it stands, for which see [`RouteFileError`]. Some output
/// may have been written by then.
///
/// # Examples
///
/// See the [module documentation](super).
pub fn write_route<W: Write>(
    mut out: W,
    device: &RrGraph,
    design: &Design,
    routes: &RouteSet<NodeId>,
    header: &RouteHeader<'_>,
) -> Result<RouteSummary, RouteFileError> {
    for field in [header.placement_file, header.placement_id] {
        if field.is_empty() || field.contains(char::is_whitespace) {
            return Err(RouteFileError::BadHeader {
                field: field.to_owned(),
            });
        }
    }
    let (width, height) = device.grid_size();
    write!(
        out,
        "Placement_File: {} Placement_ID: {}\nArray size: {width} x {height} logic blocks.\n\nRouting:",
        header.placement_file, header.placement_id,
    )?;

    let mut writer = NetWriter {
        device,
        design,
        steps: Vec::new(),
    };
    let nets: HashMap<NetId, &Net<NodeId>> =
        design.nets().iter().map(|net| (net.id, net)).collect();
    let mut summary = RouteSummary::default();
    for (index, info) in design.net_infos().iter().enumerate() {
        let id = NetId(index as u32);
        match info.status {
            NetStatus::Global => {
                write!(
                    out,
                    "\n\nNet {index} ({}): global net connecting:\n\n",
                    info.name
                )?;
                for pin in &info.pins {
                    if let Some(block) = design.block(pin.pin.block) {
                        writeln!(
                            out,
                            "Block {} (#{}) at ({},{},{}), Pin class {}.",
                            block.name, pin.pin.block, block.x, block.y, block.layer, pin.class,
                        )?;
                    }
                }
                summary.global += 1;
            }
            NetStatus::NoSinks => {
                write!(out, "\n\nNet {index} ({})\n\n", info.name)?;
                write!(
                    out,
                    "\n\nUsed in local cluster only, reserved one CLB pin\n\n"
                )?;
                summary.no_sinks += 1;
            }
            NetStatus::Undriven => {
                write!(out, "\n\nNet {index} ({})\n\n", info.name)?;
                summary.undriven += 1;
            }
            NetStatus::Routable => {
                write!(out, "\n\nNet {index} ({})\n\n", info.name)?;
                match (routes.route(id), nets.get(&id)) {
                    (Some(route), Some(net)) if !route.is_empty() => {
                        writer.write_net(&mut out, net, route)?;
                        summary.routed += 1;
                    }
                    _ => summary.unrouted += 1,
                }
            }
        }
    }
    out.flush()?;
    Ok(summary)
}

/// The tables for writing a net.
struct NetWriter<'a> {
    device: &'a RrGraph,
    design: &'a Design,
    steps: Vec<crate::space::Step<NodeId>>,
}

/// A place on the walk down a route.
struct Visit {
    node: NodeId,
    /// The switch that this node was reached through, or `None` at the root.
    arrived_by: Option<u16>,
    /// How many of the node's children have been gone through already.
    next_child: usize,
}

impl NetWriter<'_> {
    /// Writes a net's lines.
    ///
    /// Children are in the order of the route's edges, so a route grown one path at a time
    /// comes out in the same order that VPR would have written it. This doesn't recurse, as
    /// a path through a device can be long.
    fn write_net<W: Write>(
        &mut self,
        out: &mut W,
        net: &Net<NodeId>,
        route: &Route<NodeId>,
    ) -> Result<(), RouteFileError> {
        let id = net.id;
        route
            .validate(net)
            .map_err(|fault| RouteFileError::InvalidRoute { net: id, fault })?;
        let Some(root) = route.root() else {
            return Ok(());
        };

        let on_driver = net.driver().is_some_and(|driver| {
            driver
                .access_points()
                .iter()
                .any(|point| point.node == root)
        });
        // A driver on its `OPIN`s has its `SOURCE` written first: see `Design::driver_source`.
        let source = self.design.driver_source(id);
        let root_kind = if source.is_some() {
            NodeKind::Opin
        } else {
            NodeKind::Source
        };
        if !on_driver || self.device.node_kind(root) != Some(root_kind) {
            return Err(RouteFileError::WrongRoot {
                net: id,
                node: root,
            });
        }

        let mut sinks: HashMap<NodeId, u32> = HashMap::with_capacity(net.terminals.len());
        for (position, terminal) in net.terminals.iter().enumerate().skip(1) {
            for point in terminal.access_points() {
                if sinks.insert(point.node, position as u32).is_some() {
                    return Err(RouteFileError::SharedSink {
                        net: id,
                        node: point.node,
                    });
                }
            }
        }

        let mut children: HashMap<NodeId, Vec<NodeId>> =
            HashMap::with_capacity(route.edges().len());
        for edge in route.edges() {
            children.entry(edge.from).or_default().push(edge.to);
        }

        if let Some(source) = source {
            let Some(switch) = self.device.edge_switch_with(source, root, &mut self.steps) else {
                return Err(RouteFileError::MissingEdge {
                    net: id,
                    from: source,
                    to: root,
                });
            };
            self.write_node(out, id, source, Some(switch), None)?;
        }

        let mut stack = vec![Visit {
            node: root,
            arrived_by: None,
            next_child: 0,
        }];
        while let Some(visit) = stack.last_mut() {
            let node = visit.node;
            let kids = children.get(&node).map_or(&[][..], Vec::as_slice);
            let Some(&child) = kids.get(visit.next_child) else {
                if kids.is_empty() {
                    // A leaf, which is the only way to get here without having printed the
                    // node, since a node with children prints once for each of them.
                    let allowed = sinks.contains_key(&node)
                        || visit.arrived_by.is_some_and(|switch| {
                            self.device
                                .switch(switch)
                                .is_some_and(|switch| !switch.kind.is_configurable())
                        });
                    if !allowed {
                        return Err(RouteFileError::DanglingBranch { net: id, node });
                    }
                    self.write_node(out, id, node, None, sinks.get(&node).copied())?;
                }
                stack.pop();
                continue;
            };
            visit.next_child += 1;
            let Some(switch) = self.device.edge_switch_with(node, child, &mut self.steps) else {
                return Err(RouteFileError::MissingEdge {
                    net: id,
                    from: node,
                    to: child,
                });
            };
            self.write_node(out, id, node, Some(switch), None)?;
            stack.push(Visit {
                node: child,
                arrived_by: Some(switch),
                next_child: 0,
            });
        }
        Ok(())
    }

    /// Writes the line for a node, in exactly the form of VPR's `print_route`.
    fn write_node<W: Write>(
        &self,
        out: &mut W,
        net: NetId,
        node: NodeId,
        switch: Option<u16>,
        net_pin_index: Option<u32>,
    ) -> Result<(), RouteFileError> {
        let unknown = || RouteFileError::UnknownNode { net, node };
        let kind = self.device.node_kind(node).ok_or_else(unknown)?;
        let location = self.device.node_location(node).ok_or_else(unknown)?;
        let ptc = self.device.node_ptc(node).ok_or_else(unknown)?;

        write!(
            out,
            "Node:\t{}\t{:>6} ({},{},{}) ",
            node.0,
            kind.name(),
            location.xlow,
            location.ylow,
            location.layer_low,
        )?;
        if location.xlow != location.xhigh
            || location.ylow != location.yhigh
            || location.layer_low != location.layer_high
        {
            write!(
                out,
                "to ({},{},{}) ",
                location.xhigh, location.yhigh, location.layer_high,
            )?;
        }

        let tile_type = self
            .device
            .tile(
                u32::from(location.xlow),
                u32::from(location.ylow),
                u32::from(location.layer_low),
            )
            .map(|tile| {
                (
                    tile,
                    self.device.block_types().get(usize::from(tile.block_type)),
                )
            });
        let is_io = matches!(tile_type, Some((_, Some(block))) if block.is_io());
        let label = match kind {
            NodeKind::Ipin | NodeKind::Opin if is_io => "Pad",
            NodeKind::Ipin | NodeKind::Opin => "Pin",
            NodeKind::ChanX | NodeKind::ChanY | NodeKind::ChanZ => "Track",
            NodeKind::Source | NodeKind::Sink if is_io => "Pad",
            NodeKind::Source | NodeKind::Sink => "Class",
            NodeKind::Mux => "Index",
        };
        write!(out, " {label}: {ptc}  ")?;

        if !is_io && matches!(kind, NodeKind::Ipin | NodeKind::Opin) {
            let unnamed = || RouteFileError::UnnamedPin { net, node };
            let (tile, block_type) = tile_type.ok_or_else(unnamed)?;
            let slot = block_type
                .and_then(|block| block.pin_slot(ptc))
                .ok_or_else(unnamed)?;
            let block = self
                .design
                .block_at(
                    u32::from(location.xlow).saturating_sub(u32::from(tile.width_offset)),
                    u32::from(location.ylow).saturating_sub(u32::from(tile.height_offset)),
                    slot.sub_block,
                    u32::from(location.layer_low),
                )
                .ok_or_else(unnamed)?;
            write!(out, " {}.{}[{}] ", block.pb_type, slot.port, slot.bit)?;
        }

        match switch {
            Some(switch) => write!(out, "Switch: {switch}")?,
            None => write!(out, "Switch: -1")?,
        }
        if kind == NodeKind::Sink {
            // -1 is what VPR writes for "no pin", and only a SINK that isn't one of the net's,
            // at the end of a non-configurable branch, can get here without one.
            write!(
                out,
                " Net_pin_index: {}",
                net_pin_index.map_or(-1, i64::from)
            )?;
        }
        writeln!(out)?;
        Ok(())
    }
}
