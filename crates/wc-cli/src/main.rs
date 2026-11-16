//! `wiringcraft`: every project operation as a command, for scripts, agents and the app.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use wc_core::geom::{Point, Rotation};
use wc_core::lock::ItemKind;
use wc_core::project::LibrarySource;
use wc_core::{DeviceTag, Endpoint, Page, PageSize, PlacedSymbol, Project, Wire, store};

type Res<T> = Result<T, String>;

#[derive(Parser)]
#[command(name = "wiringcraft", version, about = "Electrical schematic and wiring design")]
struct Cli {
    /// Project folder.
    #[arg(short = 'C', long = "project", global = true, default_value = ".")]
    project: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new project folder (also a git repository).
    New {
        dir: PathBuf,
        #[arg(long)]
        name: Option<String>,
        /// Do not make it a git repository.
        #[arg(long)]
        no_git: bool,
    },
    /// Pages.
    #[command(subcommand)]
    Page(PageCmd),
    /// Place a library symbol on a page.
    Place {
        page: String,
        symbol: String,
        /// Position in mm, `x,y`.
        #[arg(long, value_parser = parse_point)]
        at: Point,
        #[arg(long)]
        tag: Option<String>,
        /// 0, 90, 180 or 270.
        #[arg(long, default_value_t = 0)]
        rotate: i64,
        #[arg(long)]
        mirror: bool,
    },
    /// Draw a wire. Ends are `item:pin` (e.g. `s1:13`) or a point `x,y`.
    Wire {
        page: String,
        from: String,
        to: String,
        /// Bend points, `x,y` each.
        #[arg(long = "via", value_parser = parse_point)]
        via: Vec<Point>,
        #[arg(long)]
        number: Option<String>,
        #[arg(long)]
        potential: Option<String>,
        /// The cable this wire is a core of, e.g. `=PUMP-W1` (use `--cable=-W1` for a tag that
        /// starts with `-`).
        #[arg(long)]
        cable: Option<String>,
        /// The core's name in the cable, e.g. `1` or `BN`.
        #[arg(long)]
        core: Option<String>,
    },
    /// Set a device's part and description.
    Device {
        tag: String,
        #[arg(long)]
        part: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Library sources, versions and updates.
    #[command(subcommand)]
    Lib(LibCmd),
    /// Import drawings, symbols, parts and schematics.
    #[command(subcommand)]
    Import(ImportCmd),
    /// Export to PDF.
    Pdf {
        out: PathBuf,
        /// Page ids to export (default: all).
        #[arg(long = "page")]
        pages: Vec<String>,
        #[arg(long, default_value = "")]
        date: String,
        /// Append report pages: parts, devices, wires, nets, terminals, or all.
        #[arg(long = "report")]
        reports: Vec<String>,
    },
    /// Export pages as drawings for other tools: one SVG or DXF file per page in a folder,
    /// drawn like the PDF (frame, title block, symbols, wires, cross-references).
    Drawing {
        /// Output folder; files are named `<page number>.<format>`.
        out: PathBuf,
        /// `svg` or `dxf`.
        #[arg(long, default_value = "svg")]
        format: String,
        /// Page ids to export (default: all).
        #[arg(long = "page")]
        pages: Vec<String>,
        #[arg(long, default_value = "")]
        date: String,
    },
    /// Print a report (parts, devices, wires, nets, terminals) as a table, CSV or Markdown.
    Report {
        kind: String,
        #[arg(long, default_value = "text")]
        format: String,
        /// Write to a file instead of standard output.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// List the potentials (connected pins) across all pages.
    Nets,
    /// Check the project for mistakes: devices drawn twice, contacts without a coil, shorted
    /// potentials, open pins, wires ending in the air, devices without a part. Exits with an
    /// error when an error is found, so it can guard a commit or a CI job.
    Check {
        /// Also list notes (open pins, devices without a part).
        #[arg(long)]
        all: bool,
    },
    /// Show what is inside a project archive from another tool (.tewzip, .zw1, .elk, .zip).
    Inspect { file: PathBuf },
    /// Version control: commit, branches, merge, history.
    #[command(subcommand)]
    Vcs(VcsCmd),
}

#[derive(Subcommand)]
enum PageCmd {
    Add {
        id: String,
        #[arg(long)]
        number: String,
        #[arg(long, default_value = "")]
        title: String,
        #[arg(long, default_value = "A3")]
        size: String,
    },
    List,
    Remove {
        id: String,
    },
}

#[derive(Subcommand)]
enum LibCmd {
    /// Add a source. Earlier sources win over later ones.
    Add {
        name: String,
        /// `folder` or `git`.
        kind: String,
        location: String,
        #[arg(long)]
        reference: Option<String>,
    },
    /// Clone or refresh git sources.
    Sync,
    /// Compare the project's pinned items with the library.
    Status,
    /// Take newer library versions (all changed items, or the ids given).
    Update {
        ids: Vec<String>,
    },
    /// Pin every used item that is not pinned yet.
    Lock,
    Search {
        query: String,
    },
    /// Offer the project's copy of an item back to a source for review.
    Propose {
        source: String,
        id: String,
        #[arg(long)]
        part: bool,
    },
}

#[derive(Subcommand)]
enum ImportCmd {
    /// A symbol from a DXF drawing. Pins: POINT entities, or entities on a layer named PIN.
    SymbolDxf {
        file: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        lib: PathBuf,
        #[arg(long, default_value = "")]
        category: String,
        #[arg(long, default_value = "")]
        letter_code: String,
    },
    /// A symbol from an SVG drawing. Pins: elements with `id="pin:NAME"` or `data-pin="NAME"`.
    SymbolSvg {
        file: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        lib: PathBuf,
        #[arg(long, default_value = "")]
        category: String,
        #[arg(long, default_value = "")]
        letter_code: String,
    },
    /// Parts from a CSV or semicolon-separated export (Excel "Save as CSV").
    PartsCsv {
        file: PathBuf,
        #[arg(long)]
        lib: PathBuf,
    },
    /// The open-format parts of another tool's project archive (PDF pages as trace pages,
    /// DXF drawings as pages, AutomationML devices and connections).
    Archive {
        file: PathBuf,
        #[arg(long, default_value_t = 300)]
        first_page: usize,
    },
    /// A DWG drawing onto a page, converted to DXF by an installed converter (ODA File
    /// Converter, `dwg2dxf`, or the command in WIRINGCRAFT_DWG2DXF with {in} and {out}).
    PageDwg {
        file: PathBuf,
        #[arg(long)]
        page: String,
        #[arg(long, default_value = "1.0")]
        scale: f64,
    },
    /// PDF pages as trace graphics on new pages (vector content; scanned pages have none).
    Pdf {
        file: PathBuf,
        #[arg(long, default_value_t = 200)]
        first_page: usize,
    },
    /// An Eplan Electric P8 project (.elk, its .edb folder, or a zip of the folder): its pages as
    /// drawn, with devices at their original positions and wires traced, plus devices, parts and
    /// connections not on a drawn page as overview pages.
    Eplan {
        path: PathBuf,
        #[arg(long, default_value_t = 400)]
        first_page: usize,
    },
    /// AutomationML (CAEX) devices, properties and connections, laid out as overview pages.
    Aml {
        file: PathBuf,
        #[arg(long, default_value_t = 100)]
        first_page: usize,
    },
    /// A DXF drawing onto a page as graphics; blocks named like library symbols become symbols.
    PageDxf {
        file: PathBuf,
        #[arg(long)]
        page: String,
        #[arg(long, default_value = "1.0")]
        scale: f64,
    },
}

#[derive(Subcommand)]
enum VcsCmd {
    Init,
    Commit {
        #[arg(short, long)]
        message: String,
    },
    Branch {
        name: Option<String>,
    },
    Switch {
        name: String,
    },
    Merge {
        name: String,
    },
    /// History as a subway map (terminal), or as SVG with --svg.
    Log {
        #[arg(long)]
        svg: Option<PathBuf>,
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },
    /// What changed, in schematic terms. Defaults to uncommitted changes.
    Changes {
        #[arg(default_value = "HEAD")]
        from: String,
        to: Option<String>,
    },
}

fn parse_point(s: &str) -> Result<Point, String> {
    let (x, y) = s.split_once(',').ok_or("expected x,y")?;
    let x: f64 = x.trim().parse().map_err(|_| format!("bad x in {s:?}"))?;
    let y: f64 = y.trim().parse().map_err(|_| format!("bad y in {s:?}"))?;
    if x.is_finite() && y.is_finite() {
        Ok(Point::new(x, y))
    } else {
        Err("coordinates must be finite".into())
    }
}

fn parse_end(page: &Page, s: &str) -> Res<Endpoint> {
    if let Ok(p) = parse_point(s) {
        return Ok(Endpoint::Point { at: p });
    }
    let (item, pin) = s
        .split_once(':')
        .ok_or_else(|| format!("{s:?}: expected item:pin or x,y"))?;
    if page.symbol(item).is_none() {
        return Err(format!("no symbol {item:?} on page {}", page.id));
    }
    Ok(Endpoint::Pin {
        item: item.into(),
        pin: pin.into(),
    })
}

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

fn load(dir: &Path) -> Res<Project> {
    store::load(dir).map_err(e)
}

fn save(dir: &Path, p: &Project) -> Res<()> {
    store::save(dir, p).map(|_| ()).map_err(e)
}

fn page_mut<'a>(p: &'a mut Project, id: &str) -> Res<&'a mut Page> {
    p.page_mut(id).ok_or_else(|| format!("no page {id:?}"))
}

fn run(cli: Cli) -> Res<()> {
    let dir = cli.project.as_path();
    match cli.cmd {
        Cmd::New {
            dir: new_dir,
            name,
            no_git,
        } => {
            let name = name.unwrap_or_else(|| {
                new_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Project".into())
            });
            store::create(&new_dir, &name).map_err(e)?;
            if !no_git {
                wc_vcs::Git::new(&new_dir).init().map_err(e)?;
            }
            println!("created {}", new_dir.display());
        }
        Cmd::Page(pc) => {
            let mut p = load(dir)?;
            match pc {
                PageCmd::Add {
                    id,
                    number,
                    title,
                    size,
                } => {
                    store::check_id(&id).map_err(e)?;
                    if p.page(&id).is_some() {
                        return Err(format!("page {id:?} exists"));
                    }
                    let mut page = Page::new(id, number, title);
                    page.size = match size.to_uppercase().as_str() {
                        "A4" => PageSize::A4,
                        "A3" => PageSize::A3,
                        "A2" => PageSize::A2,
                        other => return Err(format!("unknown size {other}")),
                    };
                    p.pages.push(page);
                    p.sort_pages();
                }
                PageCmd::List => {
                    for pg in &p.pages {
                        println!(
                            "{:<10} {:<6} {} ({} symbols, {} wires)",
                            pg.id,
                            pg.number,
                            pg.title,
                            pg.symbols.len(),
                            pg.wires.len()
                        );
                    }
                    return Ok(());
                }
                PageCmd::Remove { id } => {
                    let before = p.pages.len();
                    p.pages.retain(|pg| pg.id != id);
                    if p.pages.len() == before {
                        return Err(format!("no page {id:?}"));
                    }
                }
            }
            save(dir, &p)?;
        }
        Cmd::Place {
            page,
            symbol,
            at,
            tag,
            rotate,
            mirror,
        } => {
            let mut p = load(dir)?;
            if store::read_vendored_symbol(dir, &symbol).is_err() {
                let lib = wc_library::Library::open(dir, &p).map_err(e)?;
                wc_library::take(dir, &mut p, &lib, ItemKind::Symbol, &symbol).map_err(e)?;
            }
            let tag = tag.map(|t| DeviceTag::parse(&t)).transpose().map_err(e)?;
            let rotation = Rotation::from_degrees(rotate).ok_or("rotation must be 0, 90, 180 or 270")?;
            if let Some(t) = &tag {
                p.devices.entry(t.clone()).or_default();
            }
            let pg = page_mut(&mut p, &page)?;
            let id = pg.next_id("s");
            pg.symbols.push(PlacedSymbol {
                id: id.clone(),
                symbol,
                at,
                rotation,
                mirror,
                tag,
            });
            save(dir, &p)?;
            println!("{id}");
        }
        Cmd::Wire {
            page,
            from,
            to,
            via,
            number,
            potential,
            cable,
            core,
        } => {
            let mut p = load(dir)?;
            let cable = match cable {
                Some(c) => {
                    let tag = DeviceTag::parse(&c).map_err(e)?;
                    p.devices.entry(tag.clone()).or_default();
                    Some(wc_core::CableCore {
                        cable: tag,
                        core: core.unwrap_or_default(),
                    })
                }
                None if core.is_some() => return Err("--core needs --cable".into()),
                None => None,
            };
            let pg = page_mut(&mut p, &page)?;
            let from = parse_end(pg, &from)?;
            let to = parse_end(pg, &to)?;
            let id = pg.next_id("w");
            pg.wires.push(Wire {
                id: id.clone(),
                from,
                to,
                via,
                number,
                potential,
                cable,
            });
            save(dir, &p)?;
            println!("{id}");
        }
        Cmd::Device { tag, part, description } => {
            let mut p = load(dir)?;
            let t = DeviceTag::parse(&tag).map_err(e)?;
            if let Some(part) = &part
                && store::read_vendored_part(dir, part).is_err()
            {
                let lib = wc_library::Library::open(dir, &p).map_err(e)?;
                wc_library::take(dir, &mut p, &lib, ItemKind::Part, part).map_err(e)?;
            }
            let d = p.devices.entry(t).or_default();
            if part.is_some() {
                d.part = part;
            }
            if let Some(desc) = description {
                d.description = desc;
            }
            save(dir, &p)?;
        }
        Cmd::Lib(lc) => lib_cmd(dir, lc)?,
        Cmd::Import(ic) => import_cmd(dir, ic)?,
        Cmd::Pdf {
            out,
            pages,
            date,
            reports,
        } => {
            let p = load(dir)?;
            let (symbols, missing) = store::vendored_symbols(dir, &p);
            for m in missing {
                eprintln!("warning: symbol {m} has no vendored copy (run `wiringcraft lib lock`)");
            }
            let parts = vendored_parts(dir, &p);
            let inputs = wc_report::Inputs {
                project: &p,
                symbols: &symbols,
                parts: &parts,
            };
            let tables = report_kinds(&reports)?
                .into_iter()
                .map(|k| wc_report::build(k, &inputs))
                .collect();
            let (bytes, report) = wc_pdf::export(
                &p,
                &symbols,
                &wc_pdf::Options {
                    pages,
                    date,
                    wire_labels: true,
                    reports: tables,
                },
            );
            std::fs::write(&out, bytes).map_err(e)?;
            for w in &report.warnings {
                eprintln!("warning: {w}");
            }
            println!("wrote {} ({} pages)", out.display(), report.pages);
        }
        Cmd::Drawing {
            out,
            format,
            pages,
            date,
        } => {
            let render: fn(&wc_pdf::PageDrawing) -> String = match format.to_ascii_lowercase().as_str() {
                "svg" => wc_pdf::svg,
                "dxf" => wc_pdf::dxf,
                other => return Err(format!("unknown drawing format {other:?}: use svg or dxf")),
            };
            let ext = format.to_ascii_lowercase();
            let p = load(dir)?;
            let (symbols, missing) = store::vendored_symbols(dir, &p);
            for m in missing {
                eprintln!("warning: symbol {m} has no vendored copy (run `wiringcraft lib lock`)");
            }
            let (drawings, report) = wc_pdf::page_drawings(
                &p,
                &symbols,
                &wc_pdf::Options {
                    pages,
                    date,
                    ..Default::default()
                },
            );
            std::fs::create_dir_all(&out).map_err(e)?;
            let mut used = std::collections::BTreeSet::new();
            for d in &drawings {
                let mut stem = file_stem(&d.number, &d.id);
                while !used.insert(stem.clone()) {
                    stem.push('_');
                }
                std::fs::write(out.join(format!("{stem}.{ext}")), render(d)).map_err(e)?;
            }
            for w in &report.warnings {
                eprintln!("warning: {w}");
            }
            println!("wrote {} {ext} file(s) to {}", drawings.len(), out.display());
        }
        Cmd::Check { all } => {
            let p = load(dir)?;
            let (symbols, _) = store::vendored_symbols(dir, &p);
            let parts = vendored_parts(dir, &p);
            let inputs = wc_report::Inputs {
                project: &p,
                symbols: &symbols,
                parts: &parts,
            };
            let found = wc_report::check(&inputs);
            let count = |s: wc_report::Severity| found.iter().filter(|f| f.severity == s).count();
            for f in found.iter().filter(|f| all || f.severity != wc_report::Severity::Note) {
                let page = f
                    .page_id
                    .as_deref()
                    .and_then(|id| p.page(id))
                    .map(|pg| format!("page {}: ", pg.number))
                    .unwrap_or_default();
                println!("{:<7} {page}{}", f.severity.label(), f.message);
            }
            let (errors, warnings, notes) = (
                count(wc_report::Severity::Error),
                count(wc_report::Severity::Warning),
                count(wc_report::Severity::Note),
            );
            println!("{errors} errors, {warnings} warnings, {notes} notes");
            if errors > 0 {
                return Err(format!("the project check found {errors} errors"));
            }
        }
        Cmd::Report { kind, format, out } => {
            let p = load(dir)?;
            let (symbols, _) = store::vendored_symbols(dir, &p);
            let parts = vendored_parts(dir, &p);
            let inputs = wc_report::Inputs {
                project: &p,
                symbols: &symbols,
                parts: &parts,
            };
            let mut text = String::new();
            for k in report_kinds(std::slice::from_ref(&kind))? {
                let t = wc_report::build(k, &inputs);
                text.push_str(&match format.as_str() {
                    "csv" => t.to_csv(),
                    "md" | "markdown" => t.to_markdown(),
                    "text" => plain_table(&t),
                    other => return Err(format!("unknown format {other:?} (text, csv, md)")),
                });
                text.push('\n');
            }
            match out {
                Some(path) => std::fs::write(&path, text).map_err(e)?,
                None => print!("{text}"),
            }
        }
        Cmd::Nets => {
            let p = load(dir)?;
            let (symbols, _) = store::vendored_symbols(dir, &p);
            let linked = wc_core::connectivity::nets_linked(&p, |id| {
                symbols
                    .get(id)
                    .is_some_and(|s| s.role == wc_core::symbol::SymbolRole::Interruption)
            });
            for (i, n) in linked.iter().enumerate() {
                let pins: Vec<String> = n
                    .pins
                    .iter()
                    .map(|(pg, item, pin)| {
                        let tag = p
                            .page(pg)
                            .and_then(|x| x.symbol(item))
                            .and_then(|s| s.tag.as_ref())
                            .map(|t| t.to_string())
                            .unwrap_or_else(|| item.clone());
                        format!("{tag}:{pin}")
                    })
                    .collect();
                let name = if n.names.is_empty() {
                    format!("net{}", i + 1)
                } else {
                    n.names.join("/")
                };
                println!("{name}: {}", pins.join(" "));
            }
        }
        Cmd::Vcs(vc) => vcs_cmd(dir, vc)?,
        Cmd::Inspect { file } => {
            let bytes = std::fs::read(&file).map_err(e)?;
            let r = wc_import::archive::inspect(&bytes).map_err(e)?;
            println!("container: {}", r.container);
            let mut last = None;
            for en in &r.entries {
                if last != Some(en.kind) {
                    println!("{}:", en.kind.label());
                    last = Some(en.kind);
                }
                println!("  {:>12}  {}", en.size, en.name);
            }
            let importable = r.entries.iter().filter(|x| x.kind.importable()).count();
            println!(
                "{importable} of {} entries can be imported with `wiringcraft import archive`",
                r.entries.len()
            );
        }
    }
    Ok(())
}

fn lib_cmd(dir: &Path, lc: LibCmd) -> Res<()> {
    let mut p = load(dir)?;
    match lc {
        LibCmd::Add {
            name,
            kind,
            location,
            reference,
        } => {
            store::check_id(&name).map_err(e)?;
            if p.meta.libraries.iter().any(|l| l.name == name) {
                return Err(format!("source {name:?} exists"));
            }
            if kind != "folder" && kind != "git" {
                return Err("kind must be folder or git".into());
            }
            p.meta.libraries.push(LibrarySource {
                name,
                kind,
                location,
                reference,
            });
            save(dir, &p)?;
        }
        LibCmd::Sync => {
            for line in wc_library::sync(dir, &p).map_err(e)? {
                println!("{line}");
            }
        }
        LibCmd::Status => {
            let lib = wc_library::Library::open(dir, &p).map_err(e)?;
            for s in wc_library::status(dir, &p, &lib) {
                let state = match s.state {
                    wc_library::State::UpToDate => "up to date",
                    wc_library::State::Changed => "CHANGED in library",
                    wc_library::State::NotLocked => "not pinned",
                    wc_library::State::GoneFromLibrary => "gone from library (using project copy)",
                };
                let kind = if s.kind == ItemKind::Symbol { "symbol" } else { "part" };
                println!(
                    "{kind} {}: {state}; used by {}",
                    s.id,
                    if s.used_by.is_empty() {
                        "-".into()
                    } else {
                        s.used_by.join(", ")
                    }
                );
                for c in s.changes {
                    println!("    {c}");
                }
            }
        }
        LibCmd::Update { ids } => {
            let lib = wc_library::Library::open(dir, &p).map_err(e)?;
            let only = if ids.is_empty() { None } else { Some(ids.as_slice()) };
            let done = wc_library::update(dir, &mut p, &lib, only).map_err(e)?;
            save(dir, &p)?;
            println!("updated {} item(s)", done.len());
        }
        LibCmd::Lock => {
            let lib = wc_library::Library::open(dir, &p).map_err(e)?;
            let missing = wc_library::lock_missing(dir, &mut p, &lib).map_err(e)?;
            save(dir, &p)?;
            for (k, id) in missing {
                eprintln!("warning: {k:?} {id} is in no library source");
            }
        }
        LibCmd::Search { query } => {
            let lib = wc_library::Library::open(dir, &p).map_err(e)?;
            for h in lib.search(&query) {
                println!(
                    "{:<7} {:<32} {:<12} {}",
                    if h.kind == ItemKind::Symbol { "symbol" } else { "part" },
                    h.id,
                    h.source,
                    h.title
                );
            }
        }
        LibCmd::Propose { source, id, part } => {
            let lib = wc_library::Library::open(dir, &p).map_err(e)?;
            let kind = if part { ItemKind::Part } else { ItemKind::Symbol };
            let path = wc_library::propose(dir, &lib, &source, kind, &id).map_err(e)?;
            println!("proposed {}", path.display());
        }
    }
    Ok(())
}

fn import_cmd(dir: &Path, ic: ImportCmd) -> Res<()> {
    match ic {
        ImportCmd::SymbolDxf {
            file,
            id,
            name,
            lib,
            category,
            letter_code,
        } => {
            let text = std::fs::read(&file).map_err(e)?;
            let mut s = wc_import::dxf::symbol(&String::from_utf8_lossy(&text), &id, &name).map_err(e)?;
            s.category = category;
            s.letter_code = letter_code;
            let path = wc_library::write_symbol(&lib, &s).map_err(e)?;
            println!(
                "wrote {} ({} pins, {} shapes)",
                path.display(),
                s.pins.len(),
                s.graphics.len()
            );
        }
        ImportCmd::SymbolSvg {
            file,
            id,
            name,
            lib,
            category,
            letter_code,
        } => {
            let text = std::fs::read_to_string(&file).map_err(e)?;
            let mut s = wc_import::svg::symbol(&text, &id, &name).map_err(e)?;
            s.category = category;
            s.letter_code = letter_code;
            let path = wc_library::write_symbol(&lib, &s).map_err(e)?;
            println!(
                "wrote {} ({} pins, {} shapes)",
                path.display(),
                s.pins.len(),
                s.graphics.len()
            );
        }
        ImportCmd::PartsCsv { file, lib } => {
            let bytes = std::fs::read(&file).map_err(e)?;
            let (parts, warnings) = wc_import::csv::parts(&wc_import::csv::decode(&bytes)).map_err(e)?;
            for p in &parts {
                wc_library::write_part(&lib, p).map_err(e)?;
            }
            for w in warnings {
                eprintln!("warning: {w}");
            }
            println!("imported {} part(s) into {}", parts.len(), lib.display());
        }
        ImportCmd::Pdf { file, first_page } => {
            let mut p = load(dir)?;
            let bytes = std::fs::read(&file).map_err(e)?;
            let traced = wc_import::pdf::pages(&bytes).map_err(e)?;
            let stem = file
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            for (i, t) in traced.into_iter().enumerate() {
                let id = format!("pdf-{}", first_page + i);
                if p.page(&id).is_some() {
                    return Err(format!("page {id} exists; pick another --first-page"));
                }
                if t.graphics.is_empty() {
                    eprintln!(
                        "warning: page {} of {stem} has no vector content ({} images); scanned pages need OCR/tracing",
                        i + 1,
                        t.images
                    );
                }
                let mut page = Page::new(id, (first_page + i).to_string(), format!("{stem} p.{}", i + 1));
                page.size = if t.size_mm.0 > 500.0 {
                    PageSize::A2
                } else if t.size_mm.0 > 350.0 {
                    PageSize::A3
                } else {
                    PageSize::A4
                };
                println!("page {}: {} shapes", page.number, t.graphics.len());
                page.graphics = t.graphics;
                p.pages.push(page);
            }
            p.sort_pages();
            save(dir, &p)?;
        }
        ImportCmd::Eplan { path, first_page } => {
            let mut p = load(dir)?;
            let tables = eplan_tables(&path)?;
            let ep = wc_import::eplan::read(&tables).map_err(e)?;
            let (built, warnings) = wc_import::eplan::build(&ep, first_page);
            for w in ep.warnings.iter().chain(&warnings) {
                eprintln!("warning: {w}");
            }
            let source = format!(
                "import:{}",
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            // Customer and project description fill empty title block fields.
            for (id, key) in [(10015u32, "customer"), (10011, "description")] {
                if let Some((_, _, v)) = ep.properties.iter().find(|(i, _, _)| *i == id)
                    && !v.is_empty()
                {
                    p.meta.title_block.entry(key.to_owned()).or_insert_with(|| v.clone());
                }
            }
            let (n_dev, n_pages) = (built.devices.len(), built.pages.len());
            let n_drawn = built.pages.iter().filter(|pg| pg.id.starts_with("eplan-page-")).count();
            add_built(dir, &mut p, built, &source)?;
            save(dir, &p)?;
            println!(
                "imported {} Eplan pages as {n_pages} page(s), {n_drawn} of them as drawn: {n_dev} devices, {} \
                 connections",
                ep.pages.len(),
                ep.connections.len()
            );
        }
        ImportCmd::Aml { file, first_page } => {
            let mut p = load(dir)?;
            let text = std::fs::read(&file).map_err(e)?;
            let imp = wc_import::aml::parse(&String::from_utf8_lossy(&text)).map_err(e)?;
            for w in &imp.warnings {
                eprintln!("warning: {w}");
            }
            let built = wc_import::aml::build(&imp, first_page);
            let source = format!(
                "import:{}",
                file.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            for s in &built.symbols {
                store::write_vendored_symbol(dir, s).map_err(e)?;
                p.lock.symbols.insert(
                    s.id.clone(),
                    wc_core::LockEntry {
                        source: source.clone(),
                        hash: s.hash(),
                    },
                );
            }
            for part in &built.parts {
                store::write_vendored_part(dir, part).map_err(e)?;
                p.lock.parts.insert(
                    part.id.clone(),
                    wc_core::LockEntry {
                        source: source.clone(),
                        hash: part.hash(),
                    },
                );
            }
            let n_pages = built.pages.len();
            for page in built.pages {
                if p.page(&page.id).is_some() {
                    return Err(format!(
                        "page {} exists; remove it or import into a fresh project",
                        page.id
                    ));
                }
                p.pages.push(page);
            }
            p.sort_pages();
            let n_dev = built.devices.len();
            for (t, d) in built.devices {
                p.devices.insert(t, d);
            }
            save(dir, &p)?;
            println!(
                "imported {n_dev} devices, {} connections on {n_pages} page(s)",
                imp.links.len()
            );
        }
        ImportCmd::Archive { file, first_page } => {
            use wc_import::archive::Kind;
            let bytes = std::fs::read(&file).map_err(e)?;
            let entries =
                wc_import::archive::open_entries(&bytes, &[Kind::Pdf, Kind::Dxf, Kind::AutomationMl]).map_err(e)?;
            if entries.is_empty() {
                return Err(
                    "no importable PDF, DXF or AutomationML inside; run `wiringcraft inspect` to see what is there"
                        .into(),
                );
            }
            let tmp = std::env::temp_dir().join(format!("wiringcraft-archive-{}", std::process::id()));
            std::fs::create_dir_all(&tmp).map_err(e)?;
            let mut next = first_page;
            for (i, (name, kind, data)) in entries.into_iter().enumerate() {
                let ext = match kind {
                    Kind::Pdf => "pdf",
                    Kind::Dxf => "dxf",
                    _ => "aml",
                };
                let path = tmp.join(format!("{i}.{ext}"));
                std::fs::write(&path, &data).map_err(e)?;
                println!("{name}:");
                let r = match kind {
                    Kind::Pdf => {
                        let n = wc_import::pdf::pages(&data).map(|p| p.len()).unwrap_or(0);
                        let r = import_cmd(
                            dir,
                            ImportCmd::Pdf {
                                file: path.clone(),
                                first_page: next,
                            },
                        );
                        next += n.max(1);
                        r
                    }
                    Kind::Dxf => {
                        let mut p = load(dir)?;
                        let id = format!("dxf-{next}");
                        p.pages.push(Page::new(id.clone(), next.to_string(), name.clone()));
                        p.sort_pages();
                        save(dir, &p)?;
                        next += 1;
                        import_cmd(
                            dir,
                            ImportCmd::PageDxf {
                                file: path.clone(),
                                page: id,
                                scale: 1.0,
                            },
                        )
                    }
                    _ => {
                        let r = import_cmd(
                            dir,
                            ImportCmd::Aml {
                                file: path.clone(),
                                first_page: next,
                            },
                        );
                        next += 10;
                        r
                    }
                };
                if let Err(err) = r {
                    eprintln!("warning: {name}: {err}");
                }
            }
            let _ = std::fs::remove_dir_all(&tmp);
        }
        ImportCmd::PageDwg { file, page, scale } => {
            let dxf = dwg_to_dxf(&file)?;
            let r = import_cmd(
                dir,
                ImportCmd::PageDxf {
                    file: dxf.clone(),
                    page,
                    scale,
                },
            );
            let _ = std::fs::remove_file(&dxf);
            r?;
        }
        ImportCmd::PageDxf { file, page, scale } => {
            let mut p = load(dir)?;
            let lib = wc_library::Library::open(dir, &p).map_err(e)?;
            let known: std::collections::BTreeSet<String> = lib.index(ItemKind::Symbol).into_keys().collect();
            let text = std::fs::read(&file).map_err(e)?;
            let imported = wc_import::dxf::page(&String::from_utf8_lossy(&text), scale, &known).map_err(e)?;
            for s in &imported.symbols {
                if store::read_vendored_symbol(dir, &s.symbol).is_err() {
                    wc_library::take(dir, &mut p, &lib, ItemKind::Symbol, &s.symbol).map_err(e)?;
                }
            }
            let pg = page_mut(&mut p, &page)?;
            let n_g = imported.graphics.len();
            let n_s = imported.symbols.len();
            pg.graphics.extend(imported.graphics);
            for mut s in imported.symbols {
                s.id = pg.next_id("s");
                pg.symbols.push(s);
            }
            save(dir, &p)?;
            println!("imported {n_g} shapes and {n_s} symbols onto page {page}");
        }
    }
    Ok(())
}

/// Convert DWG to a temporary DXF with whatever converter is installed. DWG is a closed format;
/// WiringCraft does not read it itself.
fn dwg_to_dxf(file: &Path) -> Res<PathBuf> {
    use std::process::Command;
    let tmp = std::env::temp_dir().join(format!("wiringcraft-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(e)?;
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "drawing".into());
    let out = tmp.join(format!("{stem}.dxf"));
    let input = file.to_string_lossy().into_owned();
    let output = out.to_string_lossy().into_owned();
    let ran = |mut c: Command| c.output().map(|o| o.status.success()).unwrap_or(false);
    if let Ok(template) = std::env::var("WIRINGCRAFT_DWG2DXF") {
        let parts: Vec<String> = template
            .split_whitespace()
            .map(|p| p.replace("{in}", &input).replace("{out}", &output))
            .collect();
        if let Some((prog, args)) = parts.split_first() {
            let mut c = Command::new(prog);
            c.args(args);
            if ran(c) && out.exists() {
                return Ok(out);
            }
        }
        return Err("WIRINGCRAFT_DWG2DXF did not produce a DXF".into());
    }
    let mut c = Command::new("dwg2dxf");
    c.args(["-y", "-o", &output, &input]);
    if ran(c) && out.exists() {
        return Ok(out);
    }
    // ODA File Converter works on folders: <in dir> <out dir> <version> <type> <recurse> <audit> <filter>.
    let in_dir = file
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into());
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for prog in ["ODAFileConverter", "ODAFileConverter.exe"] {
        let mut c = Command::new(prog);
        c.args([
            in_dir.as_str(),
            tmp.to_string_lossy().as_ref(),
            "ACAD2018",
            "DXF",
            "0",
            "1",
            name.as_str(),
        ]);
        if ran(c) && out.exists() {
            return Ok(out);
        }
    }
    Err("no DWG converter found: install ODA File Converter or dwg2dxf, or set WIRINGCRAFT_DWG2DXF".into())
}

fn vcs_cmd(dir: &Path, vc: VcsCmd) -> Res<()> {
    let git = wc_vcs::Git::new(dir);
    match vc {
        VcsCmd::Init => git.init().map_err(e)?,
        VcsCmd::Commit { message } => match git.commit(&message).map_err(e)? {
            Some(h) => println!("{}", &h[..h.len().min(10)]),
            None => println!("nothing to record"),
        },
        VcsCmd::Branch { name: Some(n) } => git.create_branch(&n).map_err(e)?,
        VcsCmd::Branch { name: None } => {
            let cur = git.current_branch().unwrap_or_default();
            for b in git.branches().map_err(e)? {
                println!("{} {b}", if b == cur { "*" } else { " " });
            }
        }
        VcsCmd::Switch { name } => git.switch(&name).map_err(e)?,
        VcsCmd::Merge { name } => match git.merge(&name).map_err(e)? {
            wc_vcs::MergeOutcome::UpToDate => println!("already up to date"),
            wc_vcs::MergeOutcome::Merged => println!("merged {name}"),
            wc_vcs::MergeOutcome::Conflicts(files) => {
                println!("conflicts in:");
                for f in files {
                    println!("  {f}");
                }
                return Err("resolve the files, then `wiringcraft vcs commit`".into());
            }
        },
        VcsCmd::Log { svg, limit } => {
            let rows = wc_vcs::history(&git, limit).map_err(e)?;
            match svg {
                Some(path) => {
                    std::fs::write(&path, wc_vcs::subway_svg(&rows)).map_err(e)?;
                    println!("wrote {}", path.display());
                }
                None => print!("{}", wc_vcs::subway_text(&rows)),
            }
        }
        VcsCmd::Changes { from, to } => {
            for c in wc_vcs::changes(&git, &from, to.as_deref()).map_err(e)? {
                println!("{c}");
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Parse report names; `all` means every report.
fn report_kinds(names: &[String]) -> Result<Vec<wc_report::Kind>, String> {
    let mut out = Vec::new();
    for n in names {
        if n == "all" {
            out.extend(wc_report::Kind::ALL);
            continue;
        }
        let k = wc_report::Kind::parse(n)
            .ok_or_else(|| format!("unknown report {n:?} (parts, devices, wires, nets, terminals, all)"))?;
        out.push(k);
    }
    Ok(out)
}

/// The project's vendored parts by id (parts referenced by devices).
fn vendored_parts(dir: &Path, p: &wc_core::Project) -> std::collections::BTreeMap<String, wc_core::Part> {
    p.devices
        .values()
        .filter_map(|d| d.part.as_ref())
        .filter_map(|id| store::read_vendored_part(dir, id).ok().map(|part| (id.clone(), part)))
        .collect()
}

/// A report as aligned plain text for the terminal.
fn plain_table(t: &wc_report::Table) -> String {
    let mut w: Vec<usize> = t.columns.iter().map(|c| c.chars().count()).collect();
    for r in &t.rows {
        for (i, c) in r.iter().enumerate() {
            if let Some(x) = w.get_mut(i) {
                *x = (*x).max(c.chars().count());
            }
        }
    }
    let line = |cells: &[String]| {
        let parts: Vec<String> = cells.iter().zip(&w).map(|(c, n)| format!("{c:<n$}")).collect();
        parts.join("  ").trim_end().to_owned()
    };
    let mut out = format!("{}\n{}\n", t.title, line(&t.columns));
    out.push_str(&w.iter().map(|n| "-".repeat(*n)).collect::<Vec<_>>().join("  "));
    out.push('\n');
    for r in &t.rows {
        out.push_str(&line(r));
        out.push('\n');
    }
    out
}

/// The tables of an Eplan project given as an .elk file, an .edb folder or a zip of the folder.
fn eplan_tables(path: &Path) -> Result<wc_import::eplan::Tables, String> {
    let e = |x: std::io::Error| format!("{}: {x}", path.display());
    let folder = if path.is_dir() {
        path.to_path_buf()
    } else {
        let bytes = std::fs::read(path).map_err(e)?;
        if bytes.starts_with(b"PK") {
            return wc_import::eplan::tables_from_zip(&bytes).map_err(|x| x.to_string());
        }
        let name = wc_import::eplan::elk_target(&bytes)
            .ok_or_else(|| format!("{} is not an Eplan .elk file, .edb folder or zip", path.display()))?;
        path.with_file_name(name)
    };
    let mut tables = wc_import::eplan::Tables::new();
    for entry in std::fs::read_dir(&folder)
        .map_err(|x| format!("{}: {x}", folder.display()))?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if wc_import::eplan::wanted(&name)
            && let Ok(bytes) = std::fs::read(entry.path())
        {
            tables.insert(name, bytes);
        }
    }
    Ok(tables)
}

/// Add imported symbols, parts, pages and devices to a project, pinning symbols and parts in
/// the lockfile under `source`.
fn add_built(dir: &Path, p: &mut Project, built: wc_import::aml::Built, source: &str) -> Result<(), String> {
    let e = |x: wc_core::Error| x.to_string();
    for s in &built.symbols {
        store::write_vendored_symbol(dir, s).map_err(e)?;
        p.lock.symbols.insert(
            s.id.clone(),
            wc_core::LockEntry {
                source: source.to_owned(),
                hash: s.hash(),
            },
        );
    }
    for part in &built.parts {
        store::write_vendored_part(dir, part).map_err(e)?;
        p.lock.parts.insert(
            part.id.clone(),
            wc_core::LockEntry {
                source: source.to_owned(),
                hash: part.hash(),
            },
        );
    }
    for page in built.pages {
        if p.page(&page.id).is_some() {
            return Err(format!(
                "page {} exists; remove it or import into a fresh project",
                page.id
            ));
        }
        p.pages.push(page);
    }
    p.sort_pages();
    for (t, d) in built.devices {
        p.devices.insert(t, d);
    }
    Ok(())
}

/// A file name for a page: its number, or its id when the number is empty, with anything a file
/// system might refuse replaced.
fn file_stem(number: &str, id: &str) -> String {
    let base = if number.trim().is_empty() { id } else { number.trim() };
    let stem: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = stem.trim_start_matches('.').to_owned();
    if stem.is_empty() { "page".into() } else { stem }
}
