//! reticle's front end, as `reticle fpga` runs it: Verilog to a netlist of the device's
//! primitives, with the constraints read beside it.
//!
//! This repeats what reticle's command-line tool does before placement, since that part of
//! it is in the binary and not the library: parsing and elaborating the sources, pulling in
//! the device's cell library for primitives the design names, reading the constraints, and
//! `fpga::synthesize_for`.

use std::path::{Path, PathBuf};

use reticle::diag::Diagnostics;
use reticle::fpga::{Constraints, Device, FpgaOptions, synthesize_for};
use reticle::ir::{Design, ModuleId};
use reticle::source::{SourceId, SourceMap};

/// Reads files for `$readmemh` and for chip databases, from the sources' directories.
pub struct DiskFiles {
    roots: Vec<PathBuf>,
}

impl DiskFiles {
    /// A provider rooted at the directories of `sources`, then the working directory.
    pub fn for_sources(sources: &[String]) -> DiskFiles {
        let mut roots: Vec<PathBuf> = Vec::new();
        for source in sources {
            let dir = Path::new(source)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf();
            // And the directory above, a project's, where `sw/` and the like are.
            let up = dir.parent().map(Path::to_path_buf);
            for d in std::iter::once(dir).chain(up) {
                if !roots.contains(&d) {
                    roots.push(d);
                }
            }
        }
        roots.push(PathBuf::from("."));
        DiskFiles { roots }
    }
}

impl reticle::ir::memfile::FileProvider for DiskFiles {
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

/// The modules a Verilog text declares, by a plain reading of `module <name>` lines.
fn declared_modules(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("module ") {
            let name: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.push(name);
            }
        }
    }
    out
}

/// Verilog files that declare modules a design may name, as `reticle build`'s library
/// finds them: every `.v` under the directories given that is not a testbench.
struct Library {
    /// Each file and the modules it declares.
    files: Vec<(String, Vec<String>)>,
}

impl Library {
    fn new(dirs: Option<&str>) -> Library {
        let mut paths = Vec::new();
        for dir in dirs.into_iter().flat_map(|d| d.split(',')) {
            let mut stack = vec![PathBuf::from(dir)];
            while let Some(d) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&d) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if path.is_dir() {
                        if !matches!(name, "tb" | "sim" | "test" | "tests" | "board" | "sw") {
                            stack.push(path);
                        }
                    } else if name.ends_with(".v") && !name.ends_with("_tb.v") {
                        paths.push(path);
                    }
                }
            }
        }
        paths.sort();
        let files = paths
            .into_iter()
            .filter_map(|path| {
                let text = std::fs::read_to_string(&path).ok()?;
                Some((path.to_string_lossy().into_owned(), declared_modules(&text)))
            })
            .collect();
        Library { files }
    }

    /// The first file that declares `name` and nothing already declared.
    fn find(&self, name: &str, declared: &[String]) -> Option<&(String, Vec<String>)> {
        self.files.iter().find(|(_, modules)| {
            modules.iter().any(|m| m == name) && !modules.iter().any(|m| declared.contains(m))
        })
    }
}

/// A design synthesised for a device, with its constraints.
pub struct Synthesised {
    /// The design, mapped to the device's primitives.
    pub design: Design,
    /// Its top module.
    pub top: ModuleId,
    /// The constraints, from the files and the sources' attributes.
    pub constraints: Constraints,
}

fn render(diags: &Diagnostics) -> String {
    diags
        .iter()
        .filter(|d| d.severity == reticle::diag::Severity::Error)
        .map(|d| d.message.clone())
        .take(8)
        .collect::<Vec<_>>()
        .join("; ")
}

fn load(map: &mut SourceMap, path: &str) -> Result<SourceId, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read `{path}`: {e}"))?;
    map.add(path.to_owned(), text)
        .map_err(|e| format!("cannot load `{path}`: {e}"))
}

/// Parses, elaborates and synthesises Verilog `sources` for `device`, as `reticle fpga
/// --bitstream` does, with `top` the top module if given and `params` its parameters.
pub fn synthesise(
    sources: &[String],
    constraint_files: &[String],
    device: &Device,
    top: Option<&str>,
    params: &[(String, String)],
    ip_dir: Option<&str>,
) -> Result<Synthesised, String> {
    let mut map = SourceMap::new();
    // A design already in reticle's own text form, as the golden tests keep them.
    if let [one] = sources
        && one.ends_with(".rtl")
    {
        let id = load(&mut map, one)?;
        let text = map.file(id).text().to_string();
        let design = Design::parse_text(&text, id).map_err(|d| format!("parse: {}", render(&d)))?;
        return finish(design, &mut map, sources, constraint_files, device);
    }
    let mut diags = Diagnostics::new();
    let mut pre = reticle::verilog::Preprocessor::new();
    let mut files = Vec::new();
    for path in sources {
        let id = load(&mut map, path)?;
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
    if diags.has_errors() {
        return Err(format!("parse: {}", render(&diags)));
    }
    let dialect = sources
        .first()
        .and_then(|p| Path::new(p).extension().and_then(|e| e.to_str()))
        .and_then(reticle::verilog::Dialect::for_extension)
        .unwrap_or_default();
    let mut options = reticle::verilog::ElabOptions::new(dialect);
    options.top = top.map(str::to_owned);
    options.params = params.to_vec();
    // The primitives a slice has itself stay primitives, as `reticle fpga` keeps them.
    let kept: Vec<String> = device
        .bels
        .iter()
        .filter(|b| {
            use reticle::fpga::BelRole::{
                Bscan, Capture, Dna, Efuse, FrameEcc, Icap, LutRam, Mux7, Mux8, Startup, UsrAccess,
            };
            matches!(b.role, Mux7 | Mux8)
                || (device.family == "xc7" && b.role == LutRam)
                || matches!(
                    b.role,
                    Bscan | Capture | Dna | Efuse | FrameEcc | Icap | Startup | UsrAccess
                )
        })
        .map(|b| b.name.clone())
        .collect();
    let family = device.family.clone();
    let library = Library::new(ip_dir);
    let mut declared: Vec<String> = sources
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .flat_map(|text| declared_modules(&text))
        .collect();
    let refs: Vec<&reticle::verilog::ast::SourceFile> = files.iter().collect();
    let mut diags = Diagnostics::new();
    let mut design = reticle::verilog::elaborate(&refs, &options, &mut diags);
    while let (Some(found), false) = (&design, diags.has_errors()) {
        let Some(top) = found.top else { break };
        let names: Vec<String> = found
            .undefined_instances(top, &[])
            .into_iter()
            .filter_map(|(module, instance)| {
                match &found.modules[module].instances[instance].module {
                    reticle::ir::ModuleRef::Unresolved(name)
                        if reticle::fpga::retarget(&family, name.as_str()).is_none() =>
                    {
                        Some(name.as_str().to_owned())
                    }
                    _ => None,
                }
            })
            .filter(|name| !kept.contains(name))
            .collect();
        // A module of reticle's IP library, as `reticle build` finds one by name.
        let mut wanted: Vec<String> = Vec::new();
        for name in &names {
            if let Some((path, modules)) = library.find(name, &declared) {
                declared.extend(modules.iter().cloned());
                wanted.push(path.clone());
            }
        }
        if !wanted.is_empty() {
            for path in &wanted {
                let id = load(&mut map, path)?;
                files.push(reticle::verilog::parse_source_in(
                    &mut map,
                    id,
                    reticle::verilog::Dialect::Verilog2005,
                    &mut pre,
                    &mut IncludesFrom(path.clone()),
                    &mut diags,
                ));
            }
            if diags.has_errors() {
                return Err(format!("library: {}", render(&diags)));
            }
            let refs: Vec<&reticle::verilog::ast::SourceFile> = files.iter().collect();
            diags = Diagnostics::new();
            design = reticle::verilog::elaborate(&refs, &options, &mut diags);
            continue;
        }
        let Some(source) = reticle::fpga::cell_source(&family, &names) else {
            break;
        };
        let id = map
            .add(format!("{family}_cells.v"), source)
            .map_err(|e| e.to_string())?;
        let mut cells = Diagnostics::new();
        files.push(reticle::verilog::parse_source_in(
            &mut map,
            id,
            reticle::verilog::Dialect::Verilog2005,
            &mut pre,
            &mut IncludesFrom(String::new()),
            &mut cells,
        ));
        if cells.has_errors() {
            return Err(format!("cell library: {}", render(&cells)));
        }
        let refs: Vec<&reticle::verilog::ast::SourceFile> = files.iter().collect();
        diags = Diagnostics::new();
        design = reticle::verilog::elaborate(&refs, &options, &mut diags);
    }
    let Some(design) = design.filter(|_| !diags.has_errors()) else {
        return Err(format!("elaborate: {}", render(&diags)));
    };
    finish(design, &mut map, sources, constraint_files, device)
}

/// Reads the constraints and synthesises an elaborated design.
fn finish(
    mut design: Design,
    map: &mut SourceMap,
    sources: &[String],
    constraint_files: &[String],
    device: &Device,
) -> Result<Synthesised, String> {
    let top = design.top.ok_or("the design names no top module")?;

    let mut diags = Diagnostics::new();
    let mut constraints = Constraints::new();
    for path in constraint_files {
        let id = load(map, path)?;
        let text = map.file(id).text().to_string();
        let read = if path.ends_with(".xdc") {
            Constraints::parse_xdc(&text, id, &design, top, &mut diags)
        } else {
            Constraints::parse(&text, id, &mut diags)
        };
        constraints.merge(read, &mut diags);
    }
    constraints.merge_attrs(&design, top, &mut diags);
    constraints.check(&design, device, &mut diags);
    if diags.has_errors() {
        return Err(format!("constraints: {}", render(&diags)));
    }

    let mut options = FpgaOptions {
        synth: reticle::synth::SynthOptions {
            files: Some(std::rc::Rc::new(DiskFiles::for_sources(sources))),
            ..reticle::synth::SynthOptions::default()
        },
        wide_muxes: true,
        ..FpgaOptions::default()
    };
    options.map.vivado_memories = device.family == "xc7";
    options.vivado_synthesis = device.family == "xc7";
    // As `reticle fpga --bitstream`: only the 7 series places a hard multiplier.
    if device.family != "xc7" {
        options.map.infer_dsp = false;
    }
    let mut diags = Diagnostics::new();
    synthesize_for(&mut design, top, device, &constraints, &options, &mut diags)
        .map_err(|e| format!("synthesis: {e}: {}", render(&diags)))?;
    Ok(Synthesised {
        design,
        top,
        constraints,
    })
}
