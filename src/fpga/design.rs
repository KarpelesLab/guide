//! Turning a netlist and a placement into nets on a device's graph.
//!
//! # From a pin to a node
//!
//! Each pin of a placed block has to become a node in the graph, and this takes less than
//! one might expect: no architecture file is needed. The graph's `block_types` section lists
//! each tile type's pins by name and class, and `SOURCE` and `SINK` nodes have the class
//! index as their `ptc`. So a pin `I[3]` of a block at `(x, y)` and position `sub` becomes:
//!
//! 1. the tile type at `(x, y)` from the graph's grid,
//! 2. the pin named `I[3]` at position `sub` of that tile, which gives its class, and
//! 3. the `SINK` at `(x, y)` with that class as its `ptc`.
//!
//! # Pin equivalence
//!
//! VPR's graph already represents equivalent pins. A pin class is one `SOURCE` or `SINK` node
//! with an edge from or to each of its pins, so reaching the class node through any of them
//! is enough. A terminal therefore normally has just one access point. It gets more than one
//! only when the graph has several nodes for one class at one location, which is treated as
//! the same choice, and a router can then reach whichever it likes.

use std::collections::{HashMap, HashSet};

use super::ImportError;
use super::netlist::{PackedNetlist, PackedPin};
use super::place::Placement;
use super::rr_graph::{NodeKind, PinClassKind, RrGraph};
use crate::cost::Cost;
use crate::id::{NetId, NodeId};
use crate::net::{AccessPoint, Net, Terminal};

/// A port that VPR's architecture file marks as a global, and that isn't a clock.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GlobalPort {
    /// The block type, as in `clb`, which is the [`PackedBlock::pb_type`](super::PackedBlock).
    pub pb_type: String,
    /// The name of the port.
    pub port: String,
}

/// How to treat nets when resolving them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NetOptions {
    /// Routes global nets like any others, as with `--clock_modeling route` in VPR.
    ///
    /// By default they're reported and skipped, as with VPR's default of `ideal`. Note that
    /// a graph built with VPR's default gives clock pins no way in, so routing them on such
    /// a graph won't get anywhere.
    pub route_global_nets: bool,
    /// Ports to treat as global besides those in a block's `<clocks>`.
    ///
    /// These are ports with `is_non_clock_global` in the architecture file, which neither
    /// the netlist nor the graph can reveal.
    pub global_ports: Vec<GlobalPort>,
}

/// What becomes of a net.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NetStatus {
    /// It needs routing, and is in [`Design::nets`].
    Routable,
    /// It's global: it reaches a clock port or one of [`NetOptions::global_ports`]. It isn't
    /// in [`Design::nets`], and VPR would not route it either. A net with any global pin is
    /// global as a whole, as it is in VPR.
    Global,
    /// It has a driver but no sinks outside of its block. It's in [`Design::nets`], with
    /// one terminal and nothing to route.
    NoSinks,
    /// It has sinks but no driver, which is only found in broken netlists. It isn't in
    /// [`Design::nets`].
    Undriven,
}

/// A pin of a net, resolved to the device.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NetPin {
    /// The pin in the netlist.
    pub pin: PackedPin,
    /// The index in [`RrGraph::block_types`] of the type of tile it's on.
    pub tile_type: u16,
    /// The pin's class within that type.
    pub class: u32,
}

/// What's known about a net, whether it's being routed or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetInfo {
    /// The net's name.
    pub name: String,
    /// What becomes of it.
    pub status: NetStatus,
    /// Its pins: the driver first, if it has one, and then its sinks in VPR's order. So,
    /// for a net with a driver, the position here is the pin's `Net_pin_index`.
    pub pins: Vec<NetPin>,
}

/// A design resolved against a device, ready for a router.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Design {
    nets: Vec<Net<NodeId>>,
    infos: Vec<NetInfo>,
    blocks: Vec<DesignBlock>,
    /// Blocks by location, which a route file has to name for the pins it passes through.
    by_location: HashMap<(u32, u32, u32, u32), u32>,
    shared_sink_nets: Vec<NetId>,
}

/// What a route file needs to say about a block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DesignBlock {
    pub name: String,
    pub pb_type: String,
    pub x: u32,
    pub y: u32,
    pub layer: u32,
}

impl Design {
    /// Returns the nets to route, in order of ID.
    ///
    /// A net's ID is its number in VPR, including all nets that aren't here, so that it
    /// can name the same net in a route file. So the [`NetId`] of an entry is not necessarily
    /// its position in this list. The driver is the first terminal of each.
    pub fn nets(&self) -> &[Net<NodeId>] {
        &self.nets
    }

    /// Returns what's known about every net in the netlist, by [`NetId`].
    pub fn net_infos(&self) -> &[NetInfo] {
        &self.infos
    }

    /// Returns what's known about a net.
    pub fn net_info(&self, net: NetId) -> Option<&NetInfo> {
        self.infos.get(net.0 as usize)
    }

    /// Returns the global nets, which by default were skipped.
    pub fn global_nets(&self) -> impl Iterator<Item = (NetId, &NetInfo)> {
        self.with_status(NetStatus::Global)
    }

    /// Returns the nets with a sink but no driver, which were skipped.
    pub fn undriven_nets(&self) -> impl Iterator<Item = (NetId, &NetInfo)> {
        self.with_status(NetStatus::Undriven)
    }

    /// Returns the nets with two pins that resolve to one `SINK` node.
    ///
    /// VPR can route to such a node once per pin, if it has the capacity. A
    /// [`Route`](crate::net::Route) can't do that, as it's a tree, so one of these can be
    /// routed, but [`write_route`](super::write_route) won't write it.
    pub fn shared_sink_nets(&self) -> &[NetId] {
        &self.shared_sink_nets
    }

    fn with_status(&self, status: NetStatus) -> impl Iterator<Item = (NetId, &NetInfo)> {
        self.infos
            .iter()
            .enumerate()
            .filter(move |(_, info)| info.status == status)
            .map(|(index, info)| (NetId(index as u32), info))
    }

    /// Returns the name and type of the block at a tile's position.
    pub(crate) fn block_at(
        &self,
        x: u32,
        y: u32,
        sub_block: u32,
        layer: u32,
    ) -> Option<&DesignBlock> {
        let &index = self.by_location.get(&(x, y, sub_block, layer))?;
        self.blocks.get(index as usize)
    }

    /// Returns a block by its number in the netlist.
    pub(crate) fn block(&self, index: u32) -> Option<&DesignBlock> {
        self.blocks.get(index as usize)
    }
}

/// Where a block's tile is, and what's on it.
struct Located<'a> {
    name: &'a str,
    x: u32,
    y: u32,
    root_x: u32,
    root_y: u32,
    sub_block: u32,
    layer: u32,
    tile_type: u16,
}

/// Maps a netlist onto a device, using a placement, to give the nets to route.
///
/// Skipped nets are reported in the result. See [the module documentation](super) for the
/// limits of this.
///
/// # Errors
///
/// Fails if a block with a connected pin has no location or isn't on a tile, if its pin
/// isn't on the tile it's placed on, if the graph has no node for its class, or if an output
/// connects to an input class or the opposite. Unused blocks don't need a location.
///
/// # Examples
///
/// See the [module documentation](super).
pub fn resolve_nets(
    device: &RrGraph,
    netlist: &PackedNetlist,
    placement: &Placement,
    options: &NetOptions,
) -> Result<Design, ImportError> {
    let globals: HashSet<(&str, &str)> = options
        .global_ports
        .iter()
        .map(|global| (global.pb_type.as_str(), global.port.as_str()))
        .collect();

    // Locations of the blocks, as far as they have one. Whether it's needed depends on
    // whether a block has any pins connected to a net.
    let mut located: Vec<Option<Located<'_>>> = Vec::with_capacity(netlist.blocks().len());
    let mut blocks = Vec::with_capacity(netlist.blocks().len());
    let mut by_location = HashMap::with_capacity(netlist.blocks().len());
    for (index, block) in netlist.blocks().iter().enumerate() {
        let place = placement.block(&block.name);
        located.push(match place {
            Some(place) => {
                let tile = device.tile(place.x, place.y, place.layer).ok_or_else(|| {
                    ImportError::BadLocation {
                        block: block.name.clone(),
                        x: place.x,
                        y: place.y,
                    }
                })?;
                by_location.insert(
                    (
                        place.x.saturating_sub(u32::from(tile.width_offset)),
                        place.y.saturating_sub(u32::from(tile.height_offset)),
                        place.sub_block,
                        place.layer,
                    ),
                    index as u32,
                );
                Some(Located {
                    name: &block.name,
                    x: place.x,
                    y: place.y,
                    root_x: place.x.saturating_sub(u32::from(tile.width_offset)),
                    root_y: place.y.saturating_sub(u32::from(tile.height_offset)),
                    sub_block: place.sub_block,
                    layer: place.layer,
                    tile_type: tile.block_type,
                })
            }
            None => None,
        });
        blocks.push(DesignBlock {
            name: block.name.clone(),
            pb_type: block.pb_type.clone(),
            x: place.map_or(0, |place| place.x),
            y: place.map_or(0, |place| place.y),
            layer: place.map_or(0, |place| place.layer),
        });
    }

    let resolve_pin = |pin: PackedPin, want: PinClassKind| -> Result<NetPin, ImportError> {
        let block = netlist
            .blocks()
            .get(pin.block as usize)
            .map_or("", |block| block.name.as_str());
        let Some(Some(place)) = located.get(pin.block as usize) else {
            return Err(ImportError::UnplacedBlock {
                block: block.to_owned(),
            });
        };
        let port = netlist
            .ports()
            .get(pin.port as usize)
            .map_or("", String::as_str);
        let tile = device.block_types().get(usize::from(place.tile_type));
        let not_found = || ImportError::PinNotFound {
            block: place.name.to_owned(),
            tile: tile.map_or_else(String::new, |tile| tile.name.clone()),
            port: port.to_owned(),
            bit: pin.bit,
            sub_block: place.sub_block,
        };
        let tile = tile.ok_or_else(not_found)?;
        let slot = tile
            .find_pin(place.sub_block, port, pin.bit)
            .ok_or_else(not_found)?;
        let class = tile
            .classes
            .get(slot.class as usize)
            .ok_or_else(not_found)?;
        if class.kind != want {
            return Err(ImportError::PinDirection {
                block: place.name.to_owned(),
                port: port.to_owned(),
            });
        }
        Ok(NetPin {
            pin,
            tile_type: place.tile_type,
            class: slot.class,
        })
    };

    let access_points = |pin: &NetPin, kind: NodeKind| -> Result<Terminal<NodeId>, ImportError> {
        let Some(Some(place)) = located.get(pin.pin.block as usize) else {
            return Err(ImportError::UnplacedBlock {
                block: String::new(),
            });
        };
        let points: Vec<AccessPoint<NodeId>> = device
            .find_nodes(kind, place.root_x, place.root_y, pin.class)
            .into_iter()
            .filter(|&node| {
                device.node_location(node).is_some_and(|location| {
                    (u32::from(location.layer_low)..=u32::from(location.layer_high))
                        .contains(&place.layer)
                })
            })
            .map(|node| AccessPoint {
                node,
                cost: Cost::ZERO,
            })
            .collect();
        Terminal::new(points).map_err(|_| ImportError::NodeNotFound {
            block: place.name.to_owned(),
            kind,
            x: place.x,
            y: place.y,
            class: pin.class,
        })
    };

    let mut nets = Vec::new();
    let mut infos = Vec::with_capacity(netlist.nets().len());
    let mut shared_sink_nets = Vec::new();
    for (index, net) in netlist.nets().iter().enumerate() {
        let id = NetId(index as u32);
        let global = net.on_clock_port
            || (!globals.is_empty()
                && net.sinks.iter().any(|pin| {
                    let block = netlist.blocks().get(pin.block as usize);
                    let port = netlist.ports().get(pin.port as usize);
                    match (block, port) {
                        (Some(block), Some(port)) => {
                            globals.contains(&(block.pb_type.as_str(), port.as_str()))
                        }
                        _ => false,
                    }
                }));

        let mut pins = Vec::with_capacity(net.sinks.len() + 1);
        if let Some(driver) = net.driver {
            pins.push(resolve_pin(driver, PinClassKind::Output)?);
        }
        for &sink in &net.sinks {
            pins.push(resolve_pin(sink, PinClassKind::Input)?);
        }

        let status = if global && !options.route_global_nets {
            NetStatus::Global
        } else if net.driver.is_none() {
            NetStatus::Undriven
        } else if net.sinks.is_empty() {
            NetStatus::NoSinks
        } else {
            NetStatus::Routable
        };

        if matches!(status, NetStatus::Routable | NetStatus::NoSinks) {
            let mut terminals = Vec::with_capacity(pins.len());
            for (position, pin) in pins.iter().enumerate() {
                let kind = if position == 0 {
                    NodeKind::Source
                } else {
                    NodeKind::Sink
                };
                terminals.push(access_points(pin, kind)?);
            }
            // Only sinks can be reached twice, and only a repeated one is any trouble.
            let mut sinks = HashSet::with_capacity(terminals.len());
            let shared = terminals.iter().skip(1).any(|terminal| {
                terminal
                    .access_points()
                    .iter()
                    .any(|point| !sinks.insert(point.node))
            });
            if shared {
                shared_sink_nets.push(id);
            }
            nets.push(Net { id, terminals });
        }

        infos.push(NetInfo {
            name: net.name.clone(),
            status,
            pins,
        });
    }

    Ok(Design {
        nets,
        infos,
        blocks,
        by_location,
        shared_sink_nets,
    })
}
