//! Placing a [`Model`] with guide, and writing the result back as reticle's placement.

use guide::place::{
    AnalyticOptions, AnnealOptions, Annealer, CostTerm, ItemId, NetView, anneal, check,
    place_analytic,
};
use reticle::fpga::place::Placement;

use crate::model::{Grid, Model};

/// Half-perimeter wirelength in whole tiles, as reticle counts it, leaving out global nets.
pub struct TileWirelength<'m> {
    /// By net, whether it is a global clock's.
    pub global: &'m [bool],
    /// Guide's columns and rows, and their tiles.
    pub grid: &'m Grid,
}

impl CostTerm for TileWirelength<'_> {
    fn net_cost(&self, net: &NetView<'_>) -> u64 {
        if self.global[net.net.index()] {
            return 0;
        }
        let (x0, y0) = self.grid.tile(net.bounds.min);
        let (x1, y1) = self.grid.tile(net.bounds.max);
        ((x1 - x0) + (y1 - y0)) as u64 * u64::from(net.weight)
    }
}

/// How guide places.
#[derive(Clone, Copy, Debug)]
pub struct GuideOptions {
    /// The seed.
    pub seed: u64,
    /// Moves per temperature of detailed placement, as a multiple of `units^(4/3)`.
    pub inner_num: f64,
    /// The first temperature of detailed placement, as a multiple of the standard deviation
    /// of the cost of its first moves.
    pub temperature: f64,
    /// The first range limit of detailed placement, in steps.
    pub range: u32,
    /// The share of the sites near them that a kind's cells fill as global placement
    /// spreads them, or 0 for all of the device.
    pub density: f64,
    /// Whether to anneal from a random placement instead of from the analytic one.
    pub anneal_only: bool,
}

/// What guide did.
pub struct GuideReport {
    /// The wirelength after legalisation, in guide's cost.
    pub legalized: u64,
    /// The cost after detailed placement.
    pub cost: u64,
    /// Annealing moves.
    pub moves: u64,
}

/// Places the model with guide's analytic placer and detailed annealing, and returns
/// reticle's placement.
pub fn place(
    model: &Model,
    sites: usize,
    options: GuideOptions,
) -> Result<(Placement, GuideReport), String> {
    let cost = TileWirelength {
        global: &model.global_net,
        grid: &model.grid,
    };
    let mut detailed = AnnealOptions::detailed();
    detailed.seed = options.seed;
    detailed.inner_num = options.inner_num;
    detailed.initial_temperature_factor = options.temperature;
    detailed.initial_range = Some(options.range);
    let (outcome, legalized) = if options.anneal_only {
        let mut annealing = AnnealOptions::default();
        annealing.seed = options.seed;
        annealing.inner_num = options.inner_num;
        let outcome = anneal(&model.problem, &cost, annealing).map_err(|e| e.to_string())?;
        (outcome, 0)
    } else {
        let mut analytic = AnalyticOptions::default();
        analytic.seed = options.seed;
        analytic.detailed = None;
        analytic.target_density = (options.density > 0.0).then_some(options.density);
        let (global, _) =
            place_analytic(&model.problem, &cost, analytic).map_err(|e| e.to_string())?;
        let annealer = Annealer::new(&model.problem, &cost, global.placement, detailed)
            .map_err(|e| e.to_string())?;
        let legalized = annealer.cost();
        (annealer.run(), legalized)
    };
    let violations = check(&model.problem, &outcome.placement);
    if !violations.is_empty() {
        return Err(format!(
            "guide's placement is not legal: {:?}",
            &violations[..violations.len().min(5)]
        ));
    }
    let mut placement = Placement::new(model.instance_of.len(), sites);
    for (item, &instance) in model.instance_of.iter().enumerate() {
        let site = outcome
            .placement
            .site(ItemId(item as u32))
            .ok_or("an item has no site")?;
        placement.place(instance, site.index());
    }
    Ok((
        placement,
        GuideReport {
            legalized,
            cost: outcome.cost,
            moves: outcome.moves,
        },
    ))
}
