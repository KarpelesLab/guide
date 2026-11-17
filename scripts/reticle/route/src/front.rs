//! Reticle's flow up to a netlist of the device's primitives: what `reticle fpga` does
//! before it hands over to a family's place and route, as `src/bin/reticle/main.rs` has it
//! (`load_design_with`, the start of `fpga`), with the command line's defaults.

use std::path::Path;

use reticle::diag::Diagnostics;
use reticle::fpga::{Constraints, Device, FpgaOptions, synthesize_for, target};
use reticle::ir::{Design, ModuleId};
use reticle::source::{SourceId, SourceMap};

/// Files a design reads, `$readmemh` and the chip databases, from the directories of its
/// sources and then the working directory: the command line's `DiskFiles`.
pub struct DiskFiles {
    roots: Vec<std::path::PathBuf>,
}

impl DiskFiles {
    pub fn for_sources(sources: &[String]) -> DiskFiles {
        let mut roots: Vec<std::path::PathBuf> = Vec::new();
        for source in sources {
            let dir = Path::new(source)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf();
            if !roots.contains(&dir) {
                roots.push(dir);
            }
        }
        // And the directories above them: an example's `rtl/` reads `sw/` beside it, as the
        // command line does when it is run from the example's own directory.
        for dir in roots.clone() {
            if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty())
                && !roots.contains(&parent.to_path_buf())
            {
                roots.push(parent.to_path_buf());
            }
        }
        roots.push(std::path::PathBuf::from("."));
        DiskFiles { roots }
    }
}

impl reticle::sim::FileProvider for DiskFiles {
    fn read_file(&self, path: &str) -> Option<String> {
        let wanted = Path::new(path);
        if wanted.is_absolute() {
            return std::fs::read_to_string(wanted).ok();
        }
        self.roots
            .iter()
            .find_map(|root| std::fs::read_to_string(root.join(wanted)).ok())
    }

    fn read_bytes(&self, path: &str) -> Option<Vec<u8>> {
        let wanted = Path::new(path);
        if wanted.is_absolute() {
            return std::fs::read(wanted).ok();
        }
        self.roots
            .iter()
            .find_map(|root| std::fs::read(root.join(wanted)).ok())
    }
}

struct IncludesFrom(String);

impl reticle::verilog::IncludeResolver for IncludesFrom {
    fn resolve(&mut self, path: &str, _from: SourceId) -> Option<(String, String)> {
        let base = Path::new(&self.0).parent().unwrap_or(Path::new("."));
        let full = base.join(path);
        let text = std::fs::read_to_string(&full).ok()?;
        Some((full.to_string_lossy().into_owned(), text))
    }
}

/// A synthesised design, ready for a family's place and route.
pub struct Synthesised {
    pub design: Design,
    pub top: ModuleId,
    pub device: Device,
    pub constraints: Constraints,
}

fn report(diags: &Diagnostics, map: &SourceMap) -> Result<(), String> {
    if diags.has_errors() {
        return Err(diags.render(map));
    }
    Ok(())
}

/// Reads, elaborates and synthesises a Verilog design for `device`, as `reticle fpga
/// --bitstream` does with no other option.
pub fn synthesise(
    sources: &[String],
    top: Option<&str>,
    device: &str,
    constraint_files: &[String],
) -> Result<Synthesised, String> {
    let device = target(device).ok_or_else(|| format!("unknown device `{device}`"))?;
    let bitstream = true;

    // As the command line: wide multiplexers on, a 7-series distributed RAM is the
    // device's own cell, and a configuration primitive is its site.
    let wide_muxes = true;
    let vivado_memories = true;
    let kept: Vec<String> = device
        .bels
        .iter()
        .filter(|b| {
            use reticle::fpga::BelRole::{
                Bscan, Capture, Dna, Efuse, FrameEcc, Icap, LutRam, Mux7, Mux8, Startup, UsrAccess,
            };
            (wide_muxes && matches!(b.role, Mux7 | Mux8))
                || (vivado_memories && device.family == "xc7" && b.role == LutRam)
                || matches!(
                    b.role,
                    Bscan | Capture | Dna | Efuse | FrameEcc | Icap | Startup | UsrAccess
                )
        })
        .map(|b| b.name.clone())
        .collect();

    let mut map = SourceMap::new();
    let mut diags = Diagnostics::new();
    let mut pre = reticle::verilog::Preprocessor::new();
    let mut files = Vec::new();
    for path in sources {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let id = map.add(path.clone(), &text).map_err(|e| e.to_string())?;
        let dialect = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .and_then(reticle::verilog::Dialect::for_extension)
            .unwrap_or_default();
        files.push(reticle::verilog::parse_source_in(
            &mut map,
            id,
            dialect,
            &mut pre,
            &mut IncludesFrom(path.clone()),
            &mut diags,
        ));
    }
    report(&diags, &map)?;
    let dialect = Path::new(&sources[0])
        .extension()
        .and_then(|e| e.to_str())
        .and_then(reticle::verilog::Dialect::for_extension)
        .unwrap_or_default();
    let mut options = reticle::verilog::ElabOptions::new(dialect);
    options.top = top.map(str::to_owned);
    let refs: Vec<&reticle::verilog::ast::SourceFile> = files.iter().collect();
    let mut diags = Diagnostics::new();
    let mut design = reticle::verilog::elaborate(&refs, &options, &mut diags);
    // The device's cell library, for what the design names and does not declare.
    let family = device.family.clone();
    let in_silicon = |name: &str| reticle::fpga::retarget(&family, name).is_some();
    while let (Some(found), false) = (&design, diags.has_errors()) {
        let Some(top) = found.top else { break };
        let names: Vec<String> = found
            .undefined_instances(top, &[])
            .into_iter()
            .filter_map(|(module, instance)| {
                match &found.modules[module].instances[instance].module {
                    reticle::ir::ModuleRef::Unresolved(name) if !in_silicon(name.as_str()) => {
                        Some(name.as_str().to_owned())
                    }
                    _ => None,
                }
            })
            .filter(|name| !kept.contains(name))
            .collect();
        let Some(source) = reticle::fpga::cell_source(&family, &names) else {
            break;
        };
        let id = map
            .add(format!("{family}_cells.v"), source)
            .map_err(|e| e.to_string())?;
        let mut library = Diagnostics::new();
        files.push(reticle::verilog::parse_source_in(
            &mut map,
            id,
            reticle::verilog::Dialect::Verilog2005,
            &mut pre,
            &mut IncludesFrom(String::new()),
            &mut library,
        ));
        report(&library, &map)?;
        let refs: Vec<&reticle::verilog::ast::SourceFile> = files.iter().collect();
        diags = Diagnostics::new();
        design = reticle::verilog::elaborate(&refs, &options, &mut diags);
    }
    report(&diags, &map)?;
    let mut design = design.ok_or("the design did not elaborate")?;
    let top = design.top.ok_or("the design names no top module")?;

    let mut diags = Diagnostics::new();
    let mut constraints = Constraints::new();
    for path in constraint_files {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let id = map.add(path.clone(), &text).map_err(|e| e.to_string())?;
        let read = if path.ends_with(".xdc") {
            Constraints::parse_xdc(&text, id, &design, top, &mut diags)
        } else {
            Constraints::parse(&text, id, &mut diags)
        };
        constraints.merge(read, &mut diags);
    }
    constraints.merge_attrs(&design, top, &mut diags);
    constraints.check(&design, device, &mut diags);
    report(&diags, &map)?;

    let mut options = FpgaOptions {
        synth: reticle::synth::SynthOptions {
            files: Some(std::rc::Rc::new(DiskFiles::for_sources(sources))),
            ..reticle::synth::SynthOptions::default()
        },
        wide_muxes,
        ..FpgaOptions::default()
    };
    options.map.vivado_memories = vivado_memories && device.family == "xc7";
    options.vivado_synthesis = device.family == "xc7";
    if bitstream && device.family != "xc7" {
        options.map.infer_dsp = false;
    }
    let mut diags = Diagnostics::new();
    let flow = synthesize_for(&mut design, top, device, &constraints, &options, &mut diags);
    report(&diags, &map)?;
    flow.map_err(|e| e.to_string())?;
    Ok(Synthesised {
        design,
        top,
        device: device.clone(),
        constraints,
    })
}

/// Where `reticle fetch` keeps a database: `~/.cache/reticle/<name>/<version>/`, or what
/// the variable `env` names.
pub fn database(env: &str, name: &str) -> Result<String, String> {
    if let Ok(path) = std::env::var(env) {
        return Ok(path);
    }
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_owned())?;
    let dir = Path::new(&home).join(".cache/reticle").join(name);
    let mut versions: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| {
            format!(
                "no {name} in {}: {e}; run `reticle fetch {name}`",
                dir.display()
            )
        })?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    versions.sort();
    versions
        .pop()
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| format!("no {name} in {}", dir.display()))
}
