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
    /// Cabinet panel layouts: mounting plate, DIN rails, wire ducts and device footprints.
    /// Layouts print after the schematic pages in the PDF.
    #[command(subcommand)]
    Panel(PanelCmd),
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
    /// Number the wires by rule. Every wire of one connection gets the same number.
    /// Rules: `sequential` (1, 2, 3 in drawing order), `page-column` (`3.4`: page and column
    /// where the connection starts) or `potential` (the potential's name, else sequential).
    NumberWires {
        #[arg(long, default_value = "page-column")]
        rule: String,
        /// Renumber connections that already have a number too.
        #[arg(long)]
        overwrite: bool,
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
        /// Cross-section with its unit, e.g. `"1.5 mm²"` or `"18 AWG"`.
        #[arg(long)]
        section: Option<String>,
        /// Colour code, e.g. `BU`.
        #[arg(long)]
        colour: Option<String>,
        /// Wire type, e.g. `H07V-K`.
        #[arg(long = "type")]
        kind: Option<String>,
    },
    /// The project's locations (cabinets, rooms, panels): the `+` part of device tags.
    #[command(subcommand)]
    Location(LocationCmd),
    /// Jumpers (bridges) between terminals of one strip: jumpered terminals are one potential.
    #[command(subcommand)]
    Jumper(JumperCmd),
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
        /// Append report pages: contents, parts, devices, wires, nets, terminals, cables, plc, locations, or all.
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
    /// Print a report (contents, parts, devices, wires, nets, terminals, cables, plc, locations) as a table, CSV or Markdown.
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
    /// Export devices, their part data and connection points, and the connections between them
    /// as AutomationML (CAEX 3.0), for tools that take engineering data rather than drawings.
    Aml { out: PathBuf },
    /// Export data for other tools: marker lists for printing marking strips.
    #[command(subcommand)]
    Export(ExportCmd),
    /// Write the PLC inputs and outputs as an IEC 61131-3 global variable list (structured
    /// text) for TwinCAT, CODESYS and similar tools: one variable per point, named after its
    /// device tag and located at its address (`%I*`/`%Q*` when it has none).
    PlcVars { out: PathBuf },
    /// Check the project for mistakes: devices drawn twice, contacts without a coil, shorted
    /// potentials, open pins, wires ending in the air, devices without a part. Exits with an
    /// error when an error is found, so it can guard a commit or a CI job.
    Check {
        /// Also list notes (open pins, devices without a part).
        #[arg(long)]
        all: bool,
    },
    /// Find text in free texts, device tags, wire numbers, potentials and device descriptions
    /// on every page; one hit per line: page, kind, value. With --replace, replace it in every
    /// hit (a device tag renames the device everywhere it appears).
    Find {
        query: String,
        /// Replace the found text with this, in every hit.
        #[arg(long)]
        replace: Option<String>,
        /// Upper and lower case must match.
        #[arg(long)]
        match_case: bool,
    },
    /// Show what is inside a project archive from another tool (.tewzip, .zw1, .elk, .zip).
    Inspect { file: PathBuf },
    /// Version control: commit, branches, merge, history.
    #[command(subcommand)]
    Vcs(VcsCmd),
    /// Review comments pinned on pages: add, reply, resolve, list.
    #[command(subcommand)]
    Comment(CommentCmd),
}

#[derive(Subcommand)]
enum ExportCmd {
    /// Terminal, wire, device and cable markers for WAGO smartScript: a semicolon-separated
    /// CSV (UTF-8) with one row per marker (type, group, marker text, copies), in natural
    /// tag order. In smartScript, import it as Excel/CSV data and map the Marker column.
    WagoMarking {
        out: PathBuf,
        /// Only these marker kinds: terminals, wires, devices, cables (default: all).
        #[arg(long = "kind", value_delimiter = ',')]
        kinds: Vec<String>,
    },
}

#[derive(Subcommand)]
enum CommentCmd {
    /// Pin a comment on a page. Prints its id.
    Add {
        page: String,
        text: String,
        /// Where on the page, in mm, `x,y`; without it the comment is about the whole page.
        #[arg(long, value_parser = parse_point)]
        at: Option<Point>,
        /// Who is writing (default: WIRINGCRAFT_USER, then git's user.name, then the login).
        #[arg(long)]
        author: Option<String>,
    },
    /// Answer a comment; this opens a resolved one again.
    Reply {
        id: String,
        text: String,
        #[arg(long)]
        author: Option<String>,
    },
    /// Mark a comment resolved.
    Resolve { id: String },
    /// Open a resolved comment again.
    Reopen { id: String },
    /// Delete a comment and its replies.
    Remove { id: String },
    /// List the comments, of one page or all; open ones only unless --all.
    List {
        page: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum PanelCmd {
    /// A new mounting plate with ducts around the edge and a DIN rail in every 125 mm row.
    New {
        id: String,
        #[arg(long, default_value = "")]
        name: String,
        /// Plate width in mm.
        #[arg(long, default_value_t = 600.0)]
        width: f64,
        /// Plate height in mm.
        #[arg(long, default_value_t = 800.0)]
        height: f64,
    },
    /// Put devices that have a part and are not on the plate yet onto its rails, sized by the
    /// part's `Width` and `Height` properties.
    Arrange {
        id: String,
        /// Only devices in this location, e.g. `+CAB1`.
        #[arg(long)]
        location: Option<String>,
    },
    /// Write a layout as a 3D STEP model (plate, rails, ducts and devices as solids, depths
    /// from the part's `Depth` property) for SolidCraft or any other CAD program.
    Step {
        id: String,
        out: PathBuf,
    },
    /// List the layouts and what is on them.
    List,
    Remove {
        id: String,
    },
}

#[derive(Subcommand)]
enum LocationCmd {
    /// List the locations with their descriptions.
    List,
    /// Add a location or change its description, e.g. `location set +L1.L3 "Cabinet 3"`.
    Set {
        /// The location, with or without its `+`.
        path: String,
        /// What it is; left out keeps the one it has, `""` clears it.
        description: Option<String>,
    },
    /// Remove a location from the list (devices keep their tags).
    Remove { path: String },
}

#[derive(Subcommand)]
enum JumperCmd {
    /// Jumper terminals of a strip, e.g. `jumper add -- -X1 3 4 5`. A jumper that shares a
    /// terminal with them becomes one with them.
    Add {
        /// The strip, e.g. `-X1` (after `--`, as it starts with `-`).
        strip: String,
        /// Terminal numbers, at least two.
        terminals: Vec<String>,
    },
    /// Take terminals out of their jumper, e.g. `jumper remove -- -X1 4`.
    Remove { strip: String, terminals: Vec<String> },
    /// List the project's jumpers.
    List,
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
        /// An overview (line diagram): its devices do not count as drawings in `check`.
        #[arg(long)]
        overview: bool,
    },
    List,
    Remove {
        id: String,
    },
    /// Mark a page as an overview (line diagram), or with `--off` as a normal page again.
    Overview {
        id: String,
        #[arg(long)]
        off: bool,
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
    /// PLC addresses from a CSV list (device and address columns, description optional), as a
    /// PLC tool or spreadsheet writes it. `report plc --format csv` writes one that reads back.
    PlcCsv { file: PathBuf },
    /// Every symbol of a KiCad symbol library (`.kicad_sym`, KiCad 6 or later). Ids are
    /// `<prefix>.<symbol name>`; the first pin becomes the origin.
    SymbolKicad {
        file: PathBuf,
        #[arg(long)]
        lib: PathBuf,
        /// Id prefix (default: `kicad.<file name>`).
        #[arg(long)]
        prefix: Option<String>,
        /// Multiply coordinates, e.g. 2 to put KiCad's 2.54 mm pin grid near a 5 mm grid.
        #[arg(long, default_value_t = 1.0)]
        scale: f64,
        #[arg(long, default_value = "")]
        category: String,
    },
    /// Parts from a BMEcat catalogue (XML, version 1.2 or 2005): order number, maker,
    /// description, the maker's part number, GTIN, and ECLASS class and features as properties.
    PartsBmecat {
        file: PathBuf,
        #[arg(long)]
        lib: PathBuf,
    },
    /// Parts from an EDZ file (EPLAN Data Exchange archive, 7-Zip or zip): part number, maker,
    /// description and the EPLAN part properties. Macros and pictures in it are counted, not
    /// imported.
    PartsEdz {
        file: PathBuf,
        #[arg(long)]
        lib: PathBuf,
    },
    /// QElectroTech elements (`.elmt`), one symbol per file. Ids are `<prefix>.<file name>`;
    /// the first terminal becomes the origin.
    SymbolQet {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        lib: PathBuf,
        /// Id prefix.
        #[arg(long, default_value = "qet")]
        prefix: String,
        /// Millimetres per QElectroTech pixel; 0.5 puts its 10 px grid on a 5 mm grid.
        #[arg(long, default_value_t = wc_import::qet::DEFAULT_SCALE)]
        scale: f64,
        #[arg(long, default_value = "")]
        category: String,
        /// Letter code for new devices, e.g. `K`.
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
    /// DXF drawings as pages, AutomationML devices and connections, PLC address lists onto the
    /// project's devices, KiCad symbol libraries and CSV parts lists into `--lib`).
    Archive {
        file: PathBuf,
        #[arg(long, default_value_t = 300)]
        first_page: usize,
        /// Library folder for the symbols of `.kicad_sym` entries and the parts of CSV parts
        /// lists; without it they are listed, not imported.
        #[arg(long)]
        lib: Option<PathBuf>,
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
    /// A SolidWorks Electrical project archive (.tewzip) or project database
    /// (tew_project_data.sqlite): its schematic and line-diagram pages, symbols as boxes with
    /// their connection points at the original positions, wires joined to them, and devices with
    /// part data, linked to the archive catalog's part when it has it. Symbol graphics (DWG) are
    /// not read.
    Swe {
        path: PathBuf,
        /// Which project of the archive, counted from 1 (archives can hold several).
        #[arg(long, default_value_t = 1)]
        pick: usize,
        /// Number the pages from here instead of keeping the source's page numbers.
        #[arg(long)]
        first_page: Option<usize>,
        /// Which library symbol to draw for which source block: a text file with one
        /// `block name = symbol.id` per line. Without it, blocks whose pins match a built-in
        /// symbol (coil A1/A2, contacts 13/14 and 11/12, lamp X1/X2, terminal 1/2) use it when
        /// the pins sit where the block's points are; the rest are drawn as boxes.
        #[arg(long)]
        symbol_map: Option<PathBuf>,
    },
    /// The libraries of a SolidWorks Electrical archive (.tewzip): catalog parts with their
    /// terminals, cable types with their cores, and symbol blocks as box symbols with their
    /// connection points. Into a library folder with --lib, else vendored into the project.
    SweLibrary {
        path: PathBuf,
        /// A library folder to write into instead of the project.
        #[arg(long)]
        lib: Option<PathBuf>,
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
    /// One page's history: who changed it, when, and what changed, newest first.
    PageLog {
        page: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
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

/// Save the project. Jumpers first follow their terminals: what the command renamed, moved or
/// deleted is found by comparing with the project as it is on disk.
fn save(dir: &Path, p: &Project) -> Res<()> {
    let mut p = p.clone();
    // A new project has nothing on disk to compare with; any other load failure is reported.
    let on_disk = dir.join(store::PROJECT_FILE).exists();
    if !p.meta.jumpers.is_empty() && on_disk {
        let old = store::load(dir).map_err(|err| format!("jumpers not updated: {}", e(err)))?;
        let (old_symbols, _) = store::vendored_symbols(dir, &old);
        let before = wc_core::project::terminal_numbers(&old, |id| old_symbols.get(id).map(|s| s.role));
        let (symbols, _) = store::vendored_symbols(dir, &p);
        wc_core::project::follow_terminals_in(&mut p, &before, |id| symbols.get(id).map(|s| s.role));
    }
    store::save(dir, &p).map(|_| ()).map_err(e)
}

/// The `find` output: one hit per line, the page number (`-` for a device not drawn), the
/// kind and the value, separated by tabs.
fn find_lines(p: &Project, hits: &[wc_core::find::Hit]) -> String {
    hits.iter()
        .map(|h| {
            let page = h
                .target
                .page()
                .and_then(|id| p.page(id))
                .map_or_else(|| "-".to_owned(), |pg| pg.number.clone());
            format!(
                "{}\t{}\t{}\n",
                escape_field(&page),
                h.kind.label(),
                escape_field(&h.value)
            )
        })
        .collect()
}

/// A value as one tab-separated field: backslash, tab and line breaks written as `\\`, `\t`,
/// `\n` and `\r`, so a multi-line text stays one hit per line.
fn escape_field(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// The `location list` output: one location per line, `+path`, then a tab and the description
/// when it has one.
fn write_locations(out: &mut impl std::io::Write, locations: &[wc_core::project::Location]) -> std::io::Result<()> {
    for l in locations {
        if l.description.is_empty() {
            writeln!(out, "+{}", l.path)?;
        } else {
            writeln!(out, "+{}\t{}", l.path, l.description)?;
        }
    }
    Ok(())
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
        Cmd::Panel(pc) => {
            let mut p = load(dir)?;
            match pc {
                PanelCmd::New {
                    id,
                    name,
                    width,
                    height,
                } => {
                    store::check_id(&id).map_err(e)?;
                    if p.panels.iter().any(|x| x.id == id) {
                        return Err(format!("panel {id:?} exists"));
                    }
                    if !(width.is_finite()
                        && height.is_finite()
                        && (200.0..=5000.0).contains(&width)
                        && (200.0..=5000.0).contains(&height))
                    {
                        return Err("plate width and height must be 200 to 5000 mm".into());
                    }
                    let name = if name.is_empty() { id.clone() } else { name };
                    let panel = wc_core::panel::Panel::with_rails(id, name, width, height);
                    let rails = panel
                        .items
                        .iter()
                        .filter(|i| matches!(i, wc_core::panel::PanelItem::Rail { .. }))
                        .count();
                    println!("panel {}: {width} x {height} mm, {rails} rail(s)", panel.id);
                    p.panels.push(panel);
                    p.panels.sort_by(|a, b| a.id.cmp(&b.id));
                }
                PanelCmd::Arrange { id, location } => {
                    let parts = vendored_parts(dir, &p);
                    let mut panels = std::mem::take(&mut p.panels);
                    let Some(panel) = panels.iter_mut().find(|x| x.id == id) else {
                        return Err(format!("no panel {id:?}"));
                    };
                    let done = wc_core::panel::arrange(panel, &p, &parts, location.as_deref());
                    p.panels = panels;
                    println!("placed {} device(s): {}", done.placed.len(), done.placed.join(", "));
                    if !done.estimated.is_empty() {
                        eprintln!(
                            "warning: no Width/Height on the part, drawn 18 x 90 mm: {}",
                            done.estimated.join(", ")
                        );
                    }
                    if !done.did_not_fit.is_empty() {
                        eprintln!("warning: no room on the rails for: {}", done.did_not_fit.join(", "));
                    }
                }
                PanelCmd::Step { id, out } => {
                    let Some(panel) = p.panels.iter().find(|x| x.id == id) else {
                        return Err(format!("no panel {id:?}"));
                    };
                    let parts = vendored_parts(dir, &p);
                    let step = wc_report::step::panel_step(&p, panel, &parts, &utc_now());
                    std::fs::write(&out, &step).map_err(e)?;
                    println!(
                        "wrote {} ({} solids)",
                        out.display(),
                        step.matches("MANIFOLD_SOLID_BREP(").count()
                    );
                    return Ok(());
                }
                PanelCmd::List => {
                    for x in &p.panels {
                        println!(
                            "{:<10} {} ({} x {} mm, {} device(s))",
                            x.id,
                            x.name,
                            x.width,
                            x.height,
                            x.device_tags().len()
                        );
                    }
                    return Ok(());
                }
                PanelCmd::Remove { id } => {
                    let before = p.panels.len();
                    p.panels.retain(|x| x.id != id);
                    if p.panels.len() == before {
                        return Err(format!("no panel {id:?}"));
                    }
                }
            }
            save(dir, &p)?;
        }
        Cmd::Page(pc) => {
            let mut p = load(dir)?;
            match pc {
                PageCmd::Add {
                    id,
                    number,
                    title,
                    size,
                    overview,
                } => {
                    store::check_id(&id).map_err(e)?;
                    if p.page(&id).is_some() {
                        return Err(format!("page {id:?} exists"));
                    }
                    let mut page = Page::new(id, number, title);
                    page.overview = overview;
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
                            "{:<10} {:<6} {} ({} symbols, {} wires){}",
                            pg.id,
                            pg.number,
                            pg.title,
                            pg.symbols.len(),
                            pg.wires.len(),
                            if pg.overview { ", overview" } else { "" }
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
                PageCmd::Overview { id, off } => {
                    let page = p
                        .pages
                        .iter_mut()
                        .find(|pg| pg.id == id)
                        .ok_or_else(|| format!("no page {id:?}"))?;
                    page.overview = !off;
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
            section,
            colour,
            kind,
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
                route: Default::default(),
                spec: wc_core::WireSpec {
                    section: section.as_deref().and_then(wc_core::WireSpec::value),
                    colour: colour.as_deref().and_then(wc_core::WireSpec::value),
                    kind: kind.as_deref().and_then(wc_core::WireSpec::value),
                },
            });
            save(dir, &p)?;
            println!("{id}");
        }
        Cmd::Location(lc) => {
            let mut p = load(dir)?;
            match lc {
                LocationCmd::List => {
                    write_locations(&mut std::io::stdout(), &p.meta.locations).map_err(e)?;
                }
                LocationCmd::Set { path, description } => {
                    let description = description.unwrap_or_else(|| {
                        let have = wc_core::project::location_path(&path).ok();
                        p.meta
                            .locations
                            .iter()
                            .find(|l| have.is_some() && wc_core::project::location_path(&l.path).ok() == have)
                            .map(|l| l.description.clone())
                            .unwrap_or_default()
                    });
                    if p.meta.set_location(&path, &description)? {
                        save(dir, &p)?;
                    }
                }
                LocationCmd::Remove { path } => {
                    p.meta.remove_location(&path)?;
                    save(dir, &p)?;
                }
            }
        }
        Cmd::Jumper(jc) => {
            let mut p = load(dir)?;
            match jc {
                JumperCmd::Add { strip, terminals } => {
                    let strip = DeviceTag::parse(&strip).map_err(e)?;
                    p.meta.add_jumper(&strip, &terminals)?;
                    save(dir, &p)?;
                }
                JumperCmd::Remove { strip, terminals } => {
                    let strip = DeviceTag::parse(&strip).map_err(e)?;
                    let mut changed = false;
                    for t in &terminals {
                        changed |= p.meta.remove_from_jumper(&strip, t.trim());
                    }
                    if !changed {
                        return Err(format!("no jumper of {strip} has terminal {}", terminals.join(", ")));
                    }
                    save(dir, &p)?;
                }
                JumperCmd::List => {
                    for j in &p.meta.jumpers {
                        println!("{} {}", j.strip, j.terminals.join(" "));
                    }
                }
            }
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
                    underlays: wc_pdf::read_underlays(dir, &p),
                    panels: p.panels.clone(),
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
        Cmd::NumberWires { rule, overwrite } => {
            let rule = wc_core::numbering::Rule::parse(&rule)
                .ok_or_else(|| format!("unknown rule {rule:?} (sequential, page-column, potential)"))?;
            let mut p = load(dir)?;
            let (symbols, _) = store::vendored_symbols(dir, &p);
            let done = wc_core::numbering::number_wires(
                &mut p,
                rule,
                overwrite,
                |id| symbols.get(id).map(|s| s.role),
                |page, end| match end {
                    wc_core::Endpoint::Point { at } => Some(*at),
                    wc_core::Endpoint::Pin { item, pin } => {
                        let placed = page.symbol(item)?;
                        wc_pdf::pin_position(placed, symbols.get(&placed.symbol)?, pin)
                    }
                },
            );
            save(dir, &p)?;
            println!(
                "numbered {} connection(s); {} wire(s) changed",
                done.connections, done.wires
            );
        }
        Cmd::Find {
            query,
            replace,
            match_case,
        } => {
            let mut p = load(dir)?;
            let opts = wc_core::find::FindOptions { match_case };
            let hits = wc_core::find::find(&p, &query, opts);
            print!("{}", find_lines(&p, &hits));
            if let Some(with) = replace {
                let n = wc_core::find::replace(&mut p, &hits, &query, &with, opts)?;
                if n > 0 {
                    save(dir, &p)?;
                }
                println!("replaced {n} value(s)");
            } else if hits.is_empty() {
                println!("nothing found");
            }
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
        Cmd::Export(ExportCmd::WagoMarking { out, kinds }) => {
            let done = export_wago_marking(dir, &out, &kinds)?;
            for w in &done.warnings {
                eprintln!("warning: {w}");
            }
            println!("wrote {} ({} markers)", out.display(), done.markers);
        }
        Cmd::Aml { out } => {
            let p = load(dir)?;
            let (symbols, _) = store::vendored_symbols(dir, &p);
            let parts = vendored_parts(dir, &p);
            let inputs = wc_report::Inputs {
                project: &p,
                symbols: &symbols,
                parts: &parts,
            };
            let (text, counts) = wc_report::automationml(&inputs, &utc_now());
            std::fs::write(&out, text).map_err(e)?;
            if counts.untagged_pins > 0 {
                eprintln!(
                    "warning: {} connected pin(s) on symbols without a tag were left out",
                    counts.untagged_pins
                );
            }
            println!(
                "wrote {}: {} devices, {} connections",
                out.display(),
                counts.devices,
                counts.links
            );
        }
        Cmd::PlcVars { out } => {
            let p = load(dir)?;
            let (symbols, _) = store::vendored_symbols(dir, &p);
            let parts = vendored_parts(dir, &p);
            let inputs = wc_report::Inputs {
                project: &p,
                symbols: &symbols,
                parts: &parts,
            };
            let v = wc_report::plc_variables(&inputs);
            std::fs::write(&out, &v.text).map_err(e)?;
            if v.unlocated > 0 {
                eprintln!(
                    "warning: {} point(s) have no PLC address and are declared at %I* or %Q*",
                    v.unlocated
                );
            }
            println!("wrote {}: {} variable(s)", out.display(), v.variables);
        }
        Cmd::Nets => {
            let p = load(dir)?;
            let (symbols, _) = store::vendored_symbols(dir, &p);
            let linked = wc_core::connectivity::nets_linked(&p, |id| symbols.get(id).map(|s| s.role));
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
        Cmd::Comment(cc) => comment_cmd(dir, cc)?,
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
            let (mut s, warnings) =
                wc_import::dxf::symbol_with_warnings(&String::from_utf8_lossy(&text), &id, &name).map_err(e)?;
            for w in &warnings {
                eprintln!("warning: {w}");
            }
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
        ImportCmd::SymbolKicad {
            file,
            lib,
            prefix,
            scale,
            category,
        } => {
            let text = std::fs::read_to_string(&file).map_err(e)?;
            let prefix = prefix.unwrap_or_else(|| kicad_prefix(&file.to_string_lossy()));
            import_kicad(&text, &lib, &prefix, scale, &category)?;
        }
        ImportCmd::SymbolQet {
            files,
            lib,
            prefix,
            scale,
            category,
            letter_code,
        } => {
            let (written, failed) = import_qet(&files, &lib, &prefix, scale, &category, &letter_code)?;
            if failed > 0 {
                return Err(format!(
                    "{failed} of {} element(s) could not be imported",
                    written + failed
                ));
            }
        }
        ImportCmd::PlcCsv { file } => {
            let mut p = load(dir)?;
            let bytes = std::fs::read(&file).map_err(e)?;
            let (rows, warnings) = wc_import::csv::plc_addresses(&wc_import::csv::decode(&bytes)).map_err(e)?;
            for w in warnings {
                eprintln!("warning: {w}");
            }
            let applied = wc_import::csv::apply_plc_addresses(&mut p, &rows);
            for t in &applied.unknown {
                eprintln!("warning: no symbol carries {t}, skipped");
            }
            if applied.changed > 0 {
                save(dir, &p)?;
            }
            println!(
                "PLC addresses: {} device(s) changed, {} already up to date, {} unknown",
                applied.changed,
                applied.unchanged,
                applied.unknown.len()
            );
        }
        ImportCmd::PartsBmecat { file, lib } => {
            let (n, warnings) = import_bmecat(&file, &lib)?;
            for w in warnings {
                eprintln!("warning: {w}");
            }
            println!("imported {n} part(s) into {}", lib.display());
        }
        ImportCmd::PartsEdz { file, lib } => {
            let (n, report, warnings) = import_edz(&file, &lib)?;
            for w in warnings {
                eprintln!("warning: {w}");
            }
            for line in report.lines() {
                println!("{line}");
            }
            println!("imported {n} part(s) into {}", lib.display());
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
                if t.graphics.is_empty() && t.scan.is_none() {
                    let why = t.scan_problem.as_deref().unwrap_or("no picture covers the page");
                    eprintln!(
                        "warning: page {} of {stem} has no vector content and no usable scan ({why})",
                        i + 1,
                    );
                }
                let underlay = match &t.scan {
                    Some(scan) => {
                        let file = format!("{}/{id}.{}", wc_core::Underlay::DIR, scan.raster.format.extension());
                        std::fs::create_dir_all(dir.join(wc_core::Underlay::DIR)).map_err(e)?;
                        std::fs::write(dir.join(&file), &scan.raster.bytes).map_err(e)?;
                        println!(
                            "page {}: scanned sheet saved as {file}, shown under the page",
                            first_page + i
                        );
                        Some(wc_core::Underlay {
                            file,
                            min: scan.min,
                            max: scan.max,
                        })
                    }
                    None => None,
                };
                let mut page = Page::new(id, (first_page + i).to_string(), format!("{stem} p.{}", i + 1));
                page.underlay = underlay;
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
        ImportCmd::Swe {
            path,
            pick,
            first_page,
            symbol_map,
        } => {
            let project = pick;
            let started = std::time::Instant::now();
            let mut p = load(dir)?;
            // A database file directly, or an archive read from disk: only the picked project's
            // database is unpacked, so a large archive costs little memory.
            let mut head = [0u8; 16];
            let mut file = std::fs::File::open(&path).map_err(e)?;
            let is_db = std::io::Read::read_exact(&mut file, &mut head).is_ok() && &head == b"SQLite format 3\0";
            let db = if is_db {
                // The same size cap as for a database taken out of an archive.
                let limit = wc_import::swe::MAX_DATABASE_BYTES;
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(
                    &mut std::io::Read::take(std::fs::File::open(&path).map_err(e)?, limit + 1),
                    &mut bytes,
                )
                .map_err(e)?;
                if bytes.len() as u64 > limit {
                    return Err(format!("{} is larger than {} MB", path.display(), limit >> 20));
                }
                (bytes, None)
            } else {
                let file = std::io::BufReader::new(std::fs::File::open(&path).map_err(e)?);
                let got =
                    wc_import::swe::database_from_archive(file, project).map_err(|x| format!("{x} (use --pick)"))?;
                if got.count > 1 {
                    eprintln!(
                        "the archive holds {} projects; importing number {project} ({})",
                        got.count, got.folder
                    );
                }
                let info_warnings = got.info.iter().flat_map(|i| &i.warnings);
                for w in got.info_warning.iter().chain(info_warnings) {
                    eprintln!("warning: {w}");
                }
                (got.bytes, got.info)
            };
            let (db, info) = db;
            let conn = wc_import::swe::open(&db).map_err(e)?;
            let mut sw = wc_import::swe::read(&conn).map_err(e)?;
            sw.info = info;
            let choice = swe_symbol_choice(dir, &p, symbol_map.as_deref())?;
            for w in &choice.2 {
                eprintln!("warning: {w}");
            }
            // The archive's catalog: devices that name one of its parts are linked to it, the part
            // comes along, and its size is used for layout footprints the source gives no box.
            let catalog_read = !is_db;
            if catalog_read {
                let file = std::io::BufReader::new(std::fs::File::open(&path).map_err(e)?);
                match wc_import::swe::library::from_archive(file) {
                    Ok(lib) => {
                        // Why a catalog row was left out, so a device it would have matched is
                        // not left unlinked without a reason.
                        for w in &lib.warnings {
                            eprintln!("warning: catalog: {w}");
                        }
                        sw.catalog = lib.parts;
                    }
                    Err(x) => eprintln!("warning: devices keep their part numbers as text: {x}"),
                }
            }
            let (built, warnings, report) = wc_import::swe::build_with(&sw, first_page, &choice.0);
            for w in sw.warnings.iter().chain(&warnings) {
                eprintln!("warning: {w}");
            }
            let linked = (catalog_read && !sw.catalog.is_empty())
                .then(|| built.devices.values().filter(|d| d.part.is_some()).count());
            let source = format!(
                "import:{}",
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            let n_sym = add_swe_built(dir, &mut p, built, &source, &choice.1)?;
            save(dir, &p)?;
            println!(
                "{:<18} {:>8} {:>9}  why the rest is not imported",
                "", "source", "imported"
            );
            for r in &report {
                println!("{:<18} {:>8} {:>9}  {}", r.what, r.source, r.imported, r.note);
            }
            if let Some(n) = linked {
                println!("{n} device(s) and cable(s) linked to a part from the archive's catalog");
            }
            println!(
                "{n_sym} symbol shape(s) for the boxes; done in {} ms",
                started.elapsed().as_millis()
            );
        }
        ImportCmd::SweLibrary { path, lib } => {
            let source = format!(
                "import:{}",
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            let (report, warnings) = import_swe_library(dir, &path, lib.as_deref(), &source)?;
            for w in warnings {
                eprintln!("warning: {w}");
            }
            for line in report.lines() {
                println!("{line}");
            }
            match lib {
                Some(lib) => println!("written to {}", lib.display()),
                None => println!("vendored into the project"),
            }
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
            // Through the shared preflight: conflicts are refused before anything is written.
            let (n_pages, n_dev) = (built.pages.len(), built.devices.len());
            add_built(dir, &mut p, built, &source)?;
            save(dir, &p)?;
            println!(
                "imported {n_dev} devices, {} connections on {n_pages} page(s)",
                imp.links.len()
            );
        }
        ImportCmd::Archive { file, first_page, lib } => {
            use wc_import::archive::Kind;
            let bytes = std::fs::read(&file).map_err(e)?;
            let entries = wc_import::archive::open_entries(
                &bytes,
                &[Kind::Pdf, Kind::Dxf, Kind::AutomationMl, Kind::KicadSymbols, Kind::Csv],
            )
            .map_err(e)?;
            if entries.is_empty() {
                return Err(
                    "no importable PDF, DXF, AutomationML, KiCad symbols or CSV lists inside; run `wiringcraft inspect` to see what is there"
                        .into(),
                );
            }
            // Symbol libraries go into a library folder, not the project's pages.
            let (symbols, entries): (Vec<_>, Vec<_>) = entries
                .into_iter()
                .partition(|(_, kind, _)| *kind == Kind::KicadSymbols);
            // CSV lists come last, so PLC addresses reach devices that pages in the same archive add.
            let (csvs, entries): (Vec<_>, Vec<_>) = entries.into_iter().partition(|(_, kind, _)| *kind == Kind::Csv);
            // Two entries with the same file name would write the same ids over each other.
            let mut prefixes = std::collections::BTreeSet::new();
            for (name, _, data) in symbols {
                println!("{name}:");
                let Some(lib) = &lib else {
                    eprintln!("warning: {name}: KiCad symbols, not imported: pass --lib <folder>");
                    continue;
                };
                let prefix = kicad_prefix(&name);
                if prefixes.contains(&prefix) {
                    eprintln!("warning: {name}: skipped, an earlier entry already imports as {prefix}");
                    continue;
                }
                // Read first: an entry that cannot be read writes nothing and keeps its prefix free.
                let read = match read_kicad(&String::from_utf8_lossy(&data), &prefix, 1.0) {
                    Ok(symbols) if symbols.is_empty() => continue,
                    Ok(symbols) => symbols,
                    Err(err) => {
                        eprintln!("warning: {name}: {err}");
                        continue;
                    }
                };
                prefixes.insert(prefix);
                if let Err(err) = write_kicad(read, lib, "") {
                    eprintln!("warning: {name}: {err}");
                }
            }
            if entries.is_empty() && csvs.is_empty() {
                return Ok(());
            }
            // Removed when it goes out of scope, also on an early return.
            let staging = staging_dir().map_err(e)?;
            let tmp = staging.path();
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
            for (i, (name, _, data)) in csvs.into_iter().enumerate() {
                println!("{name}:");
                // Told apart by header, the same column names `plc-csv` and `parts-csv` read.
                let path = tmp.join(format!("list-{i}.csv"));
                let cmd = match wc_import::csv::kind(&wc_import::csv::decode(&data)) {
                    Some(wc_import::csv::CsvKind::PlcAddresses) => ImportCmd::PlcCsv { file: path.clone() },
                    Some(wc_import::csv::CsvKind::Parts) => match &lib {
                        Some(lib) => ImportCmd::PartsCsv {
                            file: path.clone(),
                            lib: lib.clone(),
                        },
                        None => {
                            eprintln!("warning: {name}: parts list, not imported: pass --lib <folder>");
                            continue;
                        }
                    },
                    None => {
                        eprintln!(
                            "warning: {name}: not a parts list (order number column) or PLC address list (device and address columns), skipped"
                        );
                        continue;
                    }
                };
                std::fs::write(&path, &data).map_err(e)?;
                if let Err(err) = import_cmd(dir, cmd) {
                    eprintln!("warning: {name}: {err}");
                }
            }
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
            for w in &imported.warnings {
                eprintln!("warning: {w}");
            }
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
        VcsCmd::PageLog { page, limit } => {
            let p = load(dir)?;
            let page_id = p
                .pages
                .iter()
                .find(|pg| pg.id == page || pg.number == page)
                .map(|pg| pg.id.clone())
                .unwrap_or(page);
            let versions = wc_vcs::page_history(&git, &page_id, limit).map_err(e)?;
            if versions.is_empty() {
                println!("no recorded versions of page {page_id}");
            }
            for v in versions {
                println!(
                    "{}  {}  {}  {}",
                    &v.commit[..v.commit.len().min(10)],
                    &utc_date(v.time)[..10],
                    v.author,
                    v.subject
                );
                for c in &v.changes {
                    println!("    {c}");
                }
            }
        }
    }
    Ok(())
}

/// Who is writing a comment: the --author option, WIRINGCRAFT_USER, git's user.name, the login.
fn comment_author(given: Option<String>) -> String {
    given
        .or_else(|| std::env::var("WIRINGCRAFT_USER").ok())
        .filter(|a| !a.trim().is_empty())
        .or_else(|| {
            std::process::Command::new("git")
                .args(["config", "user.name"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .filter(|a| !a.is_empty())
        })
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("USERNAME").ok())
        .unwrap_or_default()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or(0)
}

fn comment_cmd(dir: &Path, cc: CommentCmd) -> Res<()> {
    let mut p = load(dir)?;
    let page_id = |p: &Project, page: &str| {
        p.pages
            .iter()
            .find(|pg| pg.id == page || pg.number == page)
            .map(|pg| pg.id.clone())
            .ok_or_else(|| format!("no page {page:?}"))
    };
    match cc {
        CommentCmd::Add { page, text, at, author } => {
            let page = page_id(&p, &page)?;
            let id = p
                .add_comment(&page, at, &comment_author(author), now_secs(), &text)
                .map_err(e)?;
            save(dir, &p)?;
            println!("{id}");
        }
        CommentCmd::Reply { id, text, author } => {
            p.reply_to_comment(&id, &comment_author(author), now_secs(), &text)
                .map_err(e)?;
            save(dir, &p)?;
        }
        CommentCmd::Resolve { id } => {
            p.resolve_comment(&id, true).map_err(e)?;
            save(dir, &p)?;
        }
        CommentCmd::Reopen { id } => {
            p.resolve_comment(&id, false).map_err(e)?;
            save(dir, &p)?;
        }
        CommentCmd::Remove { id } => {
            p.remove_comment(&id).map_err(e)?;
            save(dir, &p)?;
        }
        CommentCmd::List { page, all } => {
            let only = page.map(|pg| page_id(&p, &pg)).transpose()?;
            let (mut open, mut resolved) = (0, 0);
            for c in &p.comments {
                if only.as_ref().is_some_and(|o| *o != c.page) {
                    continue;
                }
                if c.resolved {
                    resolved += 1;
                } else {
                    open += 1;
                }
                if c.resolved && !all {
                    continue;
                }
                let number = p.page(&c.page).map(|pg| pg.number.as_str()).unwrap_or("?");
                let at = c.at.map(|a| format!(" at {:.0},{:.0}", a.x, a.y)).unwrap_or_default();
                println!(
                    "{} page {number}{at}  {}  {}{}",
                    c.id,
                    c.author,
                    &utc_date(c.time)[..10],
                    if c.resolved { "  (resolved)" } else { "" }
                );
                println!("    {}", c.text.replace('\n', "\n    "));
                for r in &c.replies {
                    println!(
                        "    > {} ({}): {}",
                        r.author,
                        &utc_date(r.time)[..10],
                        r.text.replace('\n', " ")
                    );
                }
            }
            println!("{open} open, {resolved} resolved");
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
        let k = wc_report::Kind::parse(n).ok_or_else(|| {
            format!(
                "unknown report {n:?} (contents, parts, devices, wires, nets, terminals, cables, plc, locations, all)"
            )
        })?;
        out.push(k);
    }
    Ok(out)
}

/// The project's vendored parts by id (parts referenced by devices).
/// What [`export_wago_marking`] wrote.
struct Exported {
    markers: usize,
    warnings: Vec<String>,
}

/// Write the WAGO smartScript marker list of the project in `dir` to `out`. Unknown kinds are
/// refused before anything is written. Symbols without a vendored copy are warned about: their
/// terminals cannot be recognised and are left out.
fn export_wago_marking(dir: &Path, out: &Path, kinds: &[String]) -> Res<Exported> {
    let kinds = wc_report::parse_kinds(kinds)?;
    let p = load(dir)?;
    let (symbols, missing) = store::vendored_symbols(dir, &p);
    let warnings = missing
        .iter()
        .map(|m| {
            format!("symbol {m} has no vendored copy (run `wiringcraft lib lock`); its terminals are not in the list")
        })
        .collect();
    let parts = vendored_parts(dir, &p);
    let inputs = wc_report::Inputs {
        project: &p,
        symbols: &symbols,
        parts: &parts,
    };
    let t = wc_report::marking_list(&inputs, &kinds);
    std::fs::write(out, wc_report::to_smartscript_csv(&t)).map_err(e)?;
    Ok(Exported {
        markers: t.rows.len(),
        warnings,
    })
}

/// Import a BMEcat catalogue's parts into a library folder. Files that are not UTF-8 are read
/// as Latin-1, which BMEcat 1.2 catalogues often declare. Returns the count and warnings.
fn import_bmecat(file: &Path, lib: &Path) -> Res<(usize, Vec<String>)> {
    let bytes = std::fs::read(file).map_err(e)?;
    let (parts, warnings) = wc_import::bmecat::parts(&wc_import::csv::decode(&bytes)).map_err(e)?;
    for p in &parts {
        wc_library::write_part(lib, p).map_err(e)?;
    }
    Ok((parts.len(), warnings))
}

/// Import an EDZ archive's parts into a library folder. Returns the count, what the archive
/// held and warnings.
fn import_edz(file: &Path, lib: &Path) -> Res<(usize, wc_import::edz::Report, Vec<String>)> {
    let bytes = std::fs::read(file).map_err(|x| format!("{}: {x}", file.display()))?;
    let (parts, report, warnings) = wc_import::edz::read(&bytes).map_err(|x| format!("{}: {x}", file.display()))?;
    for p in &parts {
        wc_library::write_part(lib, p).map_err(e)?;
    }
    Ok((parts.len(), report, warnings))
}

/// Import a SolidWorks Electrical archive's libraries into a library folder, or vendor them into
/// the project (pinned to `source`) when no folder is given. Returns the report and warnings.
fn import_swe_library(
    dir: &Path,
    file: &Path,
    lib: Option<&Path>,
    source: &str,
) -> Res<(wc_import::swe::library::LibraryReport, Vec<String>)> {
    let reader = std::io::BufReader::new(std::fs::File::open(file).map_err(|x| format!("{}: {x}", file.display()))?);
    let got = wc_import::swe::library::from_archive(reader).map_err(|x| format!("{}: {x}", file.display()))?;
    match lib {
        Some(lib) => {
            for p in &got.parts {
                wc_library::write_part(lib, p).map_err(e)?;
            }
            for s in &got.symbols {
                wc_library::write_symbol(lib, s).map_err(e)?;
            }
        }
        None => {
            let mut p = load(dir)?;
            let built = wc_import::aml::Built {
                symbols: got.symbols,
                parts: got.parts,
                ..Default::default()
            };
            add_built(dir, &mut p, built, source)?;
            save(dir, &p)?;
        }
    }
    Ok((got.report, got.warnings))
}

/// Import QElectroTech elements into a symbol library folder, one symbol per file. A file that
/// cannot be read is reported and skipped. Returns how many were written and how many failed.
/// The default id prefix for a KiCad library: `kicad.<file name without extension>`, lower
/// case, other characters as `_`. `name` may be a path or an archive entry name.
fn kicad_prefix(name: &str) -> String {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = leaf.rsplit_once('.').map_or(leaf, |(stem, _)| stem);
    format!("kicad.{}", wc_import::id_segment(stem))
}

/// Import every symbol (one per unit) of a KiCad library text into `lib`, printing each.
fn import_kicad(text: &str, lib: &Path, prefix: &str, scale: f64, category: &str) -> Res<usize> {
    write_kicad(read_kicad(text, prefix, scale)?, lib, category)
}

/// Read a KiCad library's symbols, printing its warnings; nothing is written yet.
fn read_kicad(text: &str, prefix: &str, scale: f64) -> Res<Vec<wc_import::kicad::Imported>> {
    let (symbols, warnings) = wc_import::kicad::symbols(text, prefix, scale).map_err(e)?;
    for w in warnings {
        eprintln!("warning: {w}");
    }
    Ok(symbols)
}

/// Write read KiCad symbols into `lib`, printing each.
fn write_kicad(symbols: Vec<wc_import::kicad::Imported>, lib: &Path, category: &str) -> Res<usize> {
    let n = symbols.len();
    for mut imp in symbols {
        imp.symbol.category = category.to_owned();
        let path = wc_library::write_symbol(lib, &imp.symbol).map_err(e)?;
        println!(
            "{} -> {} ({} pins, {} shapes)",
            imp.kicad_name,
            path.display(),
            imp.symbol.pins.len(),
            imp.symbol.graphics.len()
        );
    }
    Ok(n)
}

fn import_qet(
    files: &[PathBuf],
    lib: &Path,
    prefix: &str,
    scale: f64,
    category: &str,
    letter_code: &str,
) -> Res<(usize, usize)> {
    use std::io::Read;
    let (mut written, mut failed) = (0, 0);
    // Two files whose names give the same id would overwrite each other: the later one fails.
    let mut ids: std::collections::BTreeMap<String, PathBuf> = Default::default();
    for file in files {
        let stem = wc_import::id_segment(
            &file
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        let id = format!("{}.{stem}", prefix.trim_end_matches('.'));
        let read = match ids.get(&id) {
            Some(first) => Err(format!("gives the same id {id} as {}, skipped", first.display())),
            None => std::fs::File::open(file)
                .and_then(|f| {
                    // Read at most one byte past the limit, so a huge file is refused cheaply.
                    let mut text = String::new();
                    f.take(wc_import::qet::MAX_LEN as u64 + 1).read_to_string(&mut text)?;
                    Ok(text)
                })
                .map_err(e)
                .and_then(|text| wc_import::qet::element(&text, &id, scale).map_err(e)),
        };
        let (mut imp, warnings) = match read {
            Ok(r) => r,
            Err(err) => {
                eprintln!("error: {}: {err}", file.display());
                failed += 1;
                continue;
            }
        };
        ids.insert(id, file.clone());
        for w in warnings {
            eprintln!("warning: {}: {w}", file.display());
        }
        imp.symbol.category = category.to_owned();
        imp.symbol.letter_code = letter_code.to_owned();
        let path = wc_library::write_symbol(lib, &imp.symbol).map_err(e)?;
        println!(
            "{} -> {} ({} pins, {} shapes)",
            imp.qet_name,
            path.display(),
            imp.symbol.pins.len(),
            imp.symbol.graphics.len()
        );
        written += 1;
    }
    Ok((written, failed))
}

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
/// The library symbols a SolidWorks import may draw, with the library each comes from: the
/// project's own copy where it has one (so a repeated import matches it), otherwise the
/// project's libraries. With `map`, the user's block map from that file. Also returns the
/// warnings to show: copies or library symbols that cannot be read, mapped symbols no library has.
fn swe_symbol_choice(
    dir: &Path,
    p: &Project,
    map: Option<&Path>,
) -> Res<(
    wc_import::swe::SymbolChoice,
    std::collections::BTreeMap<String, String>,
    Vec<String>,
)> {
    let mut choice = wc_import::swe::SymbolChoice::default();
    if let Some(path) = map {
        let text = std::fs::read_to_string(path).map_err(|x| format!("{}: {x}", path.display()))?;
        choice.by_block = wc_import::swe::parse_symbol_map(&text).map_err(|x| format!("{}: {x}", path.display()))?;
    }
    let mut wanted: std::collections::BTreeSet<String> = choice.by_block.values().cloned().collect();
    wanted.extend(wc_import::swe::BUILTIN_BY_PINS.iter().map(|(_, id)| (*id).to_owned()));
    let lib = readable_libraries(dir, p);
    let mut sources = std::collections::BTreeMap::new();
    let mut warnings = Vec::new();
    for id in wanted {
        // The project's own copy first, used on a page or not.
        if p.lock.symbols.contains_key(&id) {
            match project_copy(dir, &id) {
                Ok(Some(s)) => {
                    let source = p.lock.symbols.get(&id).map(|l| l.source.clone()).unwrap_or_default();
                    sources.insert(id.clone(), source);
                    choice.library.insert(id, s);
                    continue;
                }
                Ok(None) => {}
                Err(why) => warnings.push(format!("{why}; trying the libraries")),
            }
        }
        match lib.as_ref().map(|l| l.symbol(&id)) {
            Some(Ok((s, source))) => {
                sources.insert(id.clone(), source);
                choice.library.insert(id, s);
            }
            // Only a symbol that is really missing is "no library has"; one that cannot be read
            // was reported with its reason.
            Some(Err(wc_library::Error::NotFound { .. })) | None => {
                if choice.by_block.values().any(|v| *v == id) {
                    warnings.push(format!(
                        "symbol map names {id}, which no library has; those blocks stay boxes"
                    ));
                }
            }
            Some(Err(why)) => warnings.push(format!(
                "library symbol {id} cannot be read, blocks it would match stay boxes: {why}"
            )),
        }
    }
    Ok((choice, sources, warnings))
}

/// The project's libraries that can be opened, in their order. A source that cannot be opened (a
/// git source not synced yet) only means fewer matches, and says so; the others stay usable.
fn readable_libraries(dir: &Path, p: &Project) -> Option<wc_library::Library> {
    let mut readable = p.clone();
    readable.meta.libraries.retain(|source| {
        let mut one = p.clone();
        one.meta.libraries = vec![source.clone()];
        wc_library::Library::open(dir, &one)
            .inspect_err(|x| eprintln!("warning: library {} not read, blocks may stay boxes: {x}", source.name))
            .is_ok()
    });
    wc_library::Library::open(dir, &readable)
        .inspect_err(|x| eprintln!("warning: libraries not read, blocks may stay boxes: {x}"))
        .ok()
}

/// The project's own copy of symbol `id`: `None` when it has none, the reason when the copy is
/// there but cannot be read.
fn project_copy(dir: &Path, id: &str) -> Res<Option<wc_core::Symbol>> {
    let path = dir
        .join(store::VENDOR_DIR)
        .join(ItemKind::Symbol.dir())
        .join(format!("{id}.json"));
    match std::fs::metadata(&path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(format!("project copy of symbol {id} is not a file: {}", path.display())),
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(x) => {
            return Err(format!(
                "project copy of symbol {id} cannot be read: {}: {x}",
                path.display()
            ));
        }
    }
    store::read_vendored_symbol(dir, id)
        .map(Some)
        .map_err(|why| format!("project copy of symbol {id} cannot be read: {why}"))
}

/// Adds a SolidWorks Electrical import to `p`, keeping the library symbols it used (named in
/// `sources`, new to the project or pinned before) pinned to their libraries. Returns how many
/// symbol shapes the import made itself.
fn add_swe_built(
    dir: &Path,
    p: &mut Project,
    built: wc_import::aml::Built,
    source: &str,
    sources: &std::collections::BTreeMap<String, String>,
) -> Res<usize> {
    let n_sym = built.symbols.iter().filter(|s| !sources.contains_key(&s.id)).count();
    let from_library: Vec<String> = built
        .symbols
        .iter()
        .filter(|s| sources.contains_key(&s.id))
        .map(|s| s.id.clone())
        .collect();
    add_built(dir, p, built, source)?;
    keep_library_sources(p, &from_library, sources);
    Ok(n_sym)
}

/// Library symbols an import used stay pinned to the library they came from (`add_built` pins
/// everything it adds to the import), whether they were pinned before or not.
fn keep_library_sources(p: &mut Project, ids: &[String], sources: &std::collections::BTreeMap<String, String>) {
    for id in ids {
        if let (Some(lib), Some(entry)) = (sources.get(id), p.lock.symbols.get_mut(id))
            && !lib.is_empty()
        {
            entry.source.clone_from(lib);
        }
    }
}

fn add_built(dir: &Path, p: &mut Project, built: wc_import::aml::Built, source: &str) -> Result<(), String> {
    let e = |x: wc_core::Error| x.to_string();
    // Refuse before anything is written: a repeated import must not overwrite the symbol files of
    // an earlier one and then fail half way.
    if let Some(page) = built.pages.iter().find(|pg| p.page(&pg.id).is_some()) {
        return Err(format!(
            "page {} exists; remove it or import into a fresh project",
            page.id
        ));
    }
    let numbers: std::collections::BTreeSet<&str> = p.pages.iter().map(|pg| pg.number.as_str()).collect();
    if let Some(page) = built.pages.iter().find(|pg| numbers.contains(pg.number.as_str())) {
        return Err(format!(
            "page number {} is taken; import with --first-page to number the pages from a free number",
            page.number
        ));
    }
    // A library item already in the project may come again only unchanged (same content hash).
    for s in &built.symbols {
        if let Some(entry) = p.lock.symbols.get(&s.id)
            && entry.hash != s.hash()
        {
            return Err(format!(
                "symbol {} exists with other content; import into a fresh project",
                s.id
            ));
        }
    }
    for part in &built.parts {
        if let Some(entry) = p.lock.parts.get(&part.id)
            && entry.hash != part.hash()
        {
            return Err(format!(
                "part {} exists with other content; import into a fresh project",
                part.id
            ));
        }
    }
    if let Some(panel) = built
        .panels
        .iter()
        .find(|n| p.panels.iter().any(|have| have.id == n.id))
    {
        return Err(format!(
            "panel {} exists; remove it or import into a fresh project",
            panel.id
        ));
    }
    // A device the project already has under the same tag may come again only unchanged.
    if let Some((tag, _)) = built
        .devices
        .iter()
        .find(|(t, d)| p.devices.get(*t).is_some_and(|have| have != *d))
    {
        return Err(format!(
            "device {tag} exists with other data; rename it or import into a fresh project"
        ));
    }
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
    for loc in built.locations {
        p.meta.merge_location(loc);
    }
    p.panels.extend(built.panels);
    p.panels.sort_by(|a, b| a.id.cmp(&b.id));
    // Title-block fields fill the ones the project has not set; nothing typed in is overwritten.
    p.meta.fill_title_block(built.title_block);
    Ok(())
}

/// The current time in UTC as `YYYY-MM-DDThh:mm:ssZ`.
fn utc_now() -> String {
    utc_date(now_secs())
}

/// Unix seconds in UTC as `YYYY-MM-DDThh:mm:ssZ`.
fn utc_date(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
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

/// A fresh folder for files taken out of an archive: unique, and on Unix readable only by
/// this user (mode 0700, set as it is created), so nobody can plant a file or link there.
/// Removed when the returned value is dropped.
fn staging_dir() -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("wiringcraft-archive-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_archive_staging_folder_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = staging_dir().unwrap();
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    fn cli(dir: &Path, args: &[&str]) -> Res<()> {
        let mut all = vec!["wiringcraft", "-C", dir.to_str().unwrap()];
        all.extend_from_slice(args);
        run(Cli::try_parse_from(all).map_err(|e| e.to_string())?)
    }

    #[test]
    fn find_lists_hits_and_replace_renames_the_device() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut p = load(&dir).unwrap();
        let tag = wc_core::DeviceTag::parse("-K1").unwrap();
        p.pages[0].symbols.push(wc_core::PlacedSymbol {
            id: "s1".into(),
            symbol: "wc.coil".into(),
            at: wc_core::Point::new(10.0, 10.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(tag.clone()),
        });
        p.devices.insert(tag, wc_core::Device::default());
        save(&dir, &p).unwrap();
        let hits = wc_core::find::find(&p, "k1", Default::default());
        let number = p.pages[0].number.clone();
        assert_eq!(find_lines(&p, &hits), format!("{number}\tDevice tag\t-K1\n"));
        cli(&dir, &["find", "K1", "--match-case", "--replace", "K2"]).unwrap();
        let p = load(&dir).unwrap();
        let k2 = wc_core::DeviceTag::parse("-K2").unwrap();
        assert_eq!(p.pages[0].symbols[0].tag.as_ref(), Some(&k2));
        assert!(p.devices.contains_key(&k2));
        assert!(cli(&dir, &["find", "K2", "--replace", "?"]).is_err(), "not a tag");
    }

    #[test]
    fn a_project_copy_that_cannot_be_read_says_why() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert_eq!(project_copy(dir, "wc.coil"), Ok(None), "no copy: no error");
        let folder = dir.join(store::VENDOR_DIR).join(ItemKind::Symbol.dir());
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("wc.coil.json"), "{ not json").unwrap();
        let reason = store::read_vendored_symbol(dir, "wc.coil").unwrap_err().to_string();
        let why = project_copy(dir, "wc.coil").unwrap_err();
        assert_eq!(why, format!("project copy of symbol wc.coil cannot be read: {reason}"));
        // A folder where the copy should be is not taken for a missing copy.
        std::fs::remove_file(folder.join("wc.coil.json")).unwrap();
        std::fs::create_dir(folder.join("wc.coil.json")).unwrap();
        let why = project_copy(dir, "wc.coil").unwrap_err();
        assert!(
            why.starts_with("project copy of symbol wc.coil is not a file: "),
            "{why}"
        );
    }

    #[test]
    fn an_import_uses_the_pinned_project_copy_even_when_no_page_uses_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut p = load(&dir).unwrap();
        let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
        p.meta.libraries.push(LibrarySource {
            name: "starter".into(),
            kind: "folder".into(),
            location: starter.to_string_lossy().into_owned(),
            reference: None,
        });
        let (mut coil, source) = wc_library::Library::open(&dir, &p).unwrap().symbol("wc.coil").unwrap();
        // The pinned copy differs from what the library has now (a changed library).
        coil.name = "Pinned coil".into();
        store::write_vendored_symbol(&dir, &coil).unwrap();
        p.lock.symbols.insert(
            "wc.coil".into(),
            wc_core::LockEntry {
                source: source.clone(),
                hash: "h".into(),
            },
        );
        assert!(
            p.pages
                .iter()
                .all(|pg| pg.symbols.iter().all(|s| s.symbol != "wc.coil")),
            "no page uses the pinned symbol"
        );
        let (choice, sources, _) = swe_symbol_choice(&dir, &p, None).unwrap();
        assert_eq!(choice.library["wc.coil"].name, "Pinned coil", "the project copy wins");
        assert_eq!(sources["wc.coil"], source, "it stays pinned to its library");
        // A library that is gone does not lose the pinned copy either.
        p.meta.libraries.clear();
        let (choice, _, _) = swe_symbol_choice(&dir, &p, None).unwrap();
        assert_eq!(choice.library["wc.coil"].name, "Pinned coil");
    }

    #[test]
    fn a_library_that_cannot_be_opened_leaves_the_others_usable() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut p = load(&dir).unwrap();
        let folder = |name: &str, location: String| LibrarySource {
            name: name.into(),
            kind: "folder".into(),
            location,
            reference: None,
        };
        p.meta.libraries = vec![
            LibrarySource {
                name: "broken".into(),
                kind: "ftp".into(),
                location: "x".into(),
                reference: None,
            },
            folder(
                "starter",
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../library")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ];
        assert!(
            wc_library::Library::open(&dir, &p).is_err(),
            "the source list as a whole fails"
        );
        let (choice, sources, _) = swe_symbol_choice(&dir, &p, None).unwrap();
        assert!(
            choice.library.contains_key("wc.coil"),
            "the readable library is still used"
        );
        assert_eq!(sources["wc.coil"], "starter");
    }

    #[test]
    fn an_unreadable_mapped_symbol_says_why_and_not_that_it_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut p = load(&dir).unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(lib.join("symbols")).unwrap();
        std::fs::write(lib.join("symbols").join("my.coil.json"), "{ not json").unwrap();
        p.meta.libraries = vec![LibrarySource {
            name: "mine".into(),
            kind: "folder".into(),
            location: lib.to_string_lossy().into_owned(),
            reference: None,
        }];
        let map = tmp.path().join("map.txt");
        std::fs::write(&map, "Relay = my.coil\nLamp = my.lamp\n").unwrap();
        let (choice, _, warnings) = swe_symbol_choice(&dir, &p, Some(&map)).unwrap();
        assert!(!choice.library.contains_key("my.coil"));
        assert!(
            warnings
                .iter()
                .any(|w| w.starts_with("library symbol my.coil cannot be read")),
            "the reason is given: {warnings:?}"
        );
        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("names my.coil, which no library has")),
            "an unreadable symbol is not called missing: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("names my.lamp, which no library has")),
            "a really missing one still is: {warnings:?}"
        );
    }

    #[test]
    fn a_second_import_keeps_a_used_library_symbol_pinned_to_its_library() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut p = load(&dir).unwrap();
        p.pages.clear();
        let coil: wc_core::Symbol =
            serde_json::from_str(r#"{"id":"wc.coil","name":"Coil","graphics":[],"pins":[]}"#).unwrap();
        let sources: std::collections::BTreeMap<String, String> = [("wc.coil".into(), "starter".into())].into();
        let import = |id: &str, number: &str| {
            let mut page = Page::new(id, number, "");
            page.symbols.push(PlacedSymbol {
                id: "s1".into(),
                symbol: "wc.coil".into(),
                at: Point::new(10.0, 10.0),
                rotation: Rotation::default(),
                mirror: false,
                tag: DeviceTag::parse("-K1").ok(),
            });
            wc_import::aml::Built {
                symbols: vec![coil.clone()],
                pages: vec![page],
                ..Default::default()
            }
        };
        add_swe_built(&dir, &mut p, import("i1", "1"), "import:first.db", &sources).unwrap();
        save(&dir, &p).unwrap();
        let mut p = load(&dir).unwrap();
        assert_eq!(p.lock.symbols["wc.coil"].source, "starter");
        assert!(
            p.pages
                .iter()
                .any(|pg| pg.symbols.iter().any(|s| s.symbol == "wc.coil"))
        );
        // The second import uses the symbol that is pinned and used already.
        add_swe_built(&dir, &mut p, import("i2", "2"), "import:second.db", &sources).unwrap();
        save(&dir, &p).unwrap();
        assert_eq!(load(&dir).unwrap().lock.symbols["wc.coil"].source, "starter");
    }

    #[test]
    fn an_import_keeps_library_symbols_pinned_to_their_library() {
        let mut p = Project::new("t");
        let entry = |source: &str| wc_core::LockEntry {
            source: source.into(),
            hash: "h".into(),
        };
        // wc.coil was pinned to the starter library before; add_built just re-pinned it, and a
        // new wc.lamp, to the import, and an import box to the import as well.
        for id in ["wc.coil", "wc.lamp", "swe.fuse.1"] {
            p.lock.symbols.insert(id.into(), entry("import:x.db"));
        }
        let sources: std::collections::BTreeMap<String, String> = [
            ("wc.coil".into(), "starter".into()),
            ("wc.lamp".into(), "starter".into()),
        ]
        .into();
        keep_library_sources(&mut p, &["wc.coil".into(), "wc.lamp".into()], &sources);
        let src = |id: &str| p.lock.symbols[id].source.clone();
        assert_eq!((src("wc.coil"), src("wc.lamp")), ("starter".into(), "starter".into()));
        assert_eq!(src("swe.fuse.1"), "import:x.db");
    }

    #[test]
    fn saving_from_the_command_line_keeps_jumpers_on_renamed_terminals() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut p = load(&dir).unwrap();
        let page = &mut p.pages[0];
        for (id, tag) in [("t3", "-X1.3"), ("t4", "-X1.4")] {
            page.symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: "wc.terminal".into(),
                at: Point::new(10.0, 10.0),
                rotation: Rotation::default(),
                mirror: false,
                tag: DeviceTag::parse(tag).ok(),
            });
        }
        let x1 = DeviceTag::parse("-X1").unwrap();
        p.meta.add_jumper(&x1, &["3".into(), "4".into()]).unwrap();
        save(&dir, &p).unwrap();
        // A command renames -X1.3 to -X1.7.
        let mut p = load(&dir).unwrap();
        p.pages[0].symbols[0].tag = DeviceTag::parse("-X1.7").ok();
        save(&dir, &p).unwrap();
        assert_eq!(load(&dir).unwrap().meta.jumpers[0].terminals, ["4", "7"]);
    }

    #[test]
    fn locations_are_set_listed_and_removed_from_the_command_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["location", "set", "+L1.L3", "Cabinet 3"]).unwrap();
        cli(&dir, &["location", "set", "L1"]).unwrap();
        cli(&dir, &["location", "set", "L1.L3"]).unwrap();
        let locs = |d: &Path| {
            load(d)
                .unwrap()
                .meta
                .locations
                .into_iter()
                .map(|l| (l.path, l.description))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            locs(&dir),
            [
                ("L1".to_owned(), String::new()),
                ("L1.L3".to_owned(), "Cabinet 3".to_owned())
            ]
        );
        cli(&dir, &["location", "list"]).unwrap();
        let mut listed = Vec::new();
        write_locations(&mut listed, &load(&dir).unwrap().meta.locations).unwrap();
        assert_eq!(String::from_utf8(listed).unwrap(), "+L1\n+L1.L3\tCabinet 3\n");
        cli(&dir, &["location", "set", "L1.L3", ""]).unwrap();
        assert_eq!(locs(&dir)[1].1, "", "an empty description clears it");
        cli(&dir, &["location", "remove", "+L1"]).unwrap();
        assert_eq!(locs(&dir).len(), 1);
        assert!(
            cli(&dir, &["location", "remove", "L1"])
                .unwrap_err()
                .contains("no location")
        );
        assert!(cli(&dir, &["location", "set", "+L1-K1", "x"]).is_err());
    }

    #[test]
    fn jumpers_are_added_listed_and_removed_from_the_command_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["jumper", "add", "--", "-X1", "3", "4"]).unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let saved = load(&dir).unwrap().meta.jumpers;
        assert_eq!(saved.len(), 1);
        assert_eq!(
            (&saved[0].strip, saved[0].terminals.as_slice()),
            (&x1, ["3", "4"].map(String::from).as_slice())
        );
        cli(&dir, &["jumper", "list"]).unwrap();
        cli(&dir, &["jumper", "add", "--", "-X1", "4", "5"]).unwrap();
        assert_eq!(load(&dir).unwrap().meta.jumpers[0].terminals, ["3", "4", "5"]);
        cli(&dir, &["jumper", "remove", "--", "-X1", "4"]).unwrap();
        assert_eq!(load(&dir).unwrap().meta.jumpers[0].terminals, ["3", "5"]);
        let err = cli(&dir, &["jumper", "remove", "--", "-X1", "9"]).unwrap_err();
        assert!(err.contains("no jumper of -X1 has terminal 9"), "{err}");
        assert!(cli(&dir, &["jumper", "add", "--", "-X1", "1.2", "3.4"]).is_err());
    }

    #[test]
    fn bmecat_parts_land_in_the_library_also_from_latin1_files() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("cat.xml");
        let mut bytes = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><BMECAT version=\"1.2\"><T_NEW_CATALOG><ARTICLE><SUPPLIER_AID>K1</SUPPLIER_AID><ARTICLE_DETAILS><DESCRIPTION_SHORT>Sch".to_vec();
        bytes.push(0xFC); // ü in Latin-1
        bytes.extend_from_slice(b"tz</DESCRIPTION_SHORT><MANUFACTURER_NAME>Acme</MANUFACTURER_NAME></ARTICLE_DETAILS></ARTICLE></T_NEW_CATALOG></BMECAT>");
        std::fs::write(&file, bytes).unwrap();
        let lib = tmp.path().join("lib");
        let (n, warnings) = import_bmecat(&file, &lib).unwrap();
        assert_eq!(n, 1, "{warnings:?}");
        let json = std::fs::read_to_string(lib.join("parts").join("acme.k1.json")).unwrap();
        let p: wc_core::Part = serde_json::from_str(&json).unwrap();
        assert_eq!(p.description, "Schütz");

        std::fs::write(&file, "<catalog/>").unwrap();
        assert!(import_bmecat(&file, &lib).is_err());
    }

    #[test]
    fn imported_panels_are_sorted_and_a_taken_panel_id_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let panel = |id: &str| wc_core::panel::Panel::new(id, id, 600.0, 800.0);
        let mut p = load(&dir).unwrap();
        let built = wc_import::aml::Built {
            panels: vec![panel("swe-layout-b"), panel("swe-layout-a")],
            ..Default::default()
        };
        add_built(&dir, &mut p, built, "import:a.tewzip").unwrap();
        let ids = |p: &Project| p.panels.iter().map(|n| n.id.clone()).collect::<Vec<_>>();
        // Sorted by the import itself, before saving (loading sorts too).
        assert_eq!(ids(&p), ["swe-layout-a", "swe-layout-b"]);
        save(&dir, &p).unwrap();

        // A second import with a taken id is refused before anything changes.
        let mut p = load(&dir).unwrap();
        let before = p.clone();
        let project_file = std::fs::read(dir.join("project.toml")).unwrap();
        let part = wc_core::Part {
            id: "maker.new-part".into(),
            ..Default::default()
        };
        let built = wc_import::aml::Built {
            panels: vec![panel("swe-layout-c"), panel("swe-layout-a")],
            parts: vec![part],
            ..Default::default()
        };
        let err = add_built(&dir, &mut p, built, "import:b.tewzip").unwrap_err();
        assert!(err.contains("panel swe-layout-a exists"), "{err}");
        assert_eq!(p, before);
        assert!(
            store::read_vendored_part(&dir, "maker.new-part").is_err(),
            "no part file written"
        );
        assert_eq!(std::fs::read(dir.join("project.toml")).unwrap(), project_file);
    }

    #[test]
    fn a_found_value_stays_on_one_line() {
        assert_eq!(escape_field("Motor\nStart\tA\\B\r"), "Motor\\nStart\\tA\\\\B\\r");
        assert_eq!(escape_field("-K1"), "-K1");
    }

    #[test]
    fn swe_libraries_land_in_a_library_folder_or_the_project() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let catalog = rusqlite::Connection::open_in_memory().unwrap();
        catalog
            .execute_batch(
                "CREATE TABLE tew_catreference (ref_id INTEGER, ref_reference TEXT, ref_manufacturer TEXT);
                 INSERT INTO tew_catreference VALUES (1, 'RLY-24', 'Maker A');",
            )
            .unwrap();
        let app = rusqlite::Connection::open_in_memory().unwrap();
        app.execute_batch(
            "CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT);
             CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
             INSERT INTO tew_block VALUES (1, 'Coil');
             INSERT INTO tew_blockpt VALUES (1, 'A1', 0, 5), (1, 'A2', 0, -5);",
        )
        .unwrap();
        let file = tmp.path().join("plant.tewzip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
            let opt = zip::write::SimpleFileOptions::default();
            for (name, c) in [
                ("Database/tew_catalog.sqlite", &catalog),
                ("Database/tew_app_data.sqlite", &app),
            ] {
                z.start_file(name, opt).unwrap();
                z.write_all(&c.serialize(rusqlite::MAIN_DB).unwrap()).unwrap();
            }
            z.finish().unwrap();
        }
        let lib = tmp.path().join("lib");
        let args = [
            "import",
            "swe-library",
            file.to_str().unwrap(),
            "--lib",
            lib.to_str().unwrap(),
        ];
        cli(tmp.path(), &args).unwrap();
        assert!(lib.join("parts").join("maker-a.rly-24.json").exists());
        assert!(lib.join("symbols").join("swe.lib.coil.json").exists());

        // Without --lib the items are vendored into the project and pinned to the import.
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["import", "swe-library", file.to_str().unwrap()]).unwrap();
        let p = load(&dir).unwrap();
        assert_eq!(p.lock.parts["maker-a.rly-24"].source, "import:plant.tewzip");
        assert!(p.lock.symbols.contains_key("swe.lib.coil"));
        assert!(store::read_vendored_part(&dir, "maker-a.rly-24").is_ok());

        std::fs::write(&file, "not an archive").unwrap();
        assert!(cli(&dir, &["import", "swe-library", file.to_str().unwrap()]).is_err());
    }

    #[test]
    fn a_zip_with_a_kicad_library_imports_every_unit_into_the_library() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        use std::io::Write;
        let file = tmp.path().join("vendor.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
            let opt = zip::write::SimpleFileOptions::default();
            z.start_file("symbols/My Relays.kicad_sym", opt).unwrap();
            z.write_all(
                br#"(kicad_symbol_lib
  (symbol "K" (property "Reference" "K" (at 0 0 0))
    (symbol "K_1_1" (pin passive line (at 0 3 270) (length 1) (name "~") (number "A1")))
    (symbol "K_2_1" (pin passive line (at 0 3 270) (length 1) (name "~") (number "11")))))"#,
            )
            .unwrap();
            // A broken library with the same name first: it writes nothing, so it takes nothing.
            z.start_file("a/My Relays.kicad_sym", opt).unwrap();
            z.write_all(b"(kicad_symbol_lib (symbol").unwrap();
            // Same file name elsewhere: it would write kicad.my_relays.k_a over the first one.
            z.start_file("zz/My Relays.kicad_sym", opt).unwrap();
            z.write_all(br#"(kicad_symbol_lib (symbol "K" (symbol "K_1_1" (pin passive line (at 0 3 270) (length 1) (name "~") (number "Z")))))"#)
                .unwrap();
            z.finish().unwrap();
        }
        let lib = tmp.path().join("lib");
        // Without --lib the library is listed, not imported, and nothing else is in the zip.
        cli(&dir, &["import", "archive", file.to_str().unwrap()]).unwrap();
        assert!(!lib.exists());
        cli(
            &dir,
            &[
                "import",
                "archive",
                file.to_str().unwrap(),
                "--lib",
                lib.to_str().unwrap(),
            ],
        )
        .unwrap();
        let symbols = lib.join("symbols");
        assert!(symbols.join("kicad.my_relays.k_a.json").exists());
        assert!(symbols.join("kicad.my_relays.k_b.json").exists());
        assert!(
            !symbols.join("kicad.my_relays.k.json").exists(),
            "the second entry was skipped"
        );
        let a = std::fs::read_to_string(symbols.join("kicad.my_relays.k_a.json")).unwrap();
        assert!(a.contains("\"A1\""), "{a}");
        assert_eq!(kicad_prefix("a/b\\Std Relays.v2.kicad_sym"), "kicad.std_relays_v2");
    }

    #[test]
    fn a_zip_with_csv_lists_imports_plc_addresses_and_parts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        use std::io::Write;
        let file = tmp.path().join("vendor.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
            let opt = zip::write::SimpleFileOptions::default();
            // The list comes first in the zip; -K1 exists only once the AutomationML entry is in.
            z.start_file("io/plc.csv", opt).unwrap();
            z.write_all(b"Device;Address;Comment\r\n-K1;%I0.0;Start\r\n").unwrap();
            z.start_file("parts.csv", opt).unwrap();
            z.write_all(b"Manufacturer;Order number;Description\r\nAcme;R-24;Relay\r\n")
                .unwrap();
            z.start_file("notes.csv", opt).unwrap();
            z.write_all(b"Name;Value\r\na;1\r\n").unwrap();
            z.start_file("plant.aml", opt).unwrap();
            z.write_all(
                br#"<CAEXFile FileName="plant.aml" SchemaVersion="3.0"><InstanceHierarchy Name="Plant">
  <InternalElement Name="K1" ID="e1"><ExternalInterface Name="A1" ID="i1"/></InternalElement>
</InstanceHierarchy></CAEXFile>"#,
            )
            .unwrap();
            z.finish().unwrap();
        }
        let lib = tmp.path().join("lib");
        // Without --lib the parts list is listed, not imported; the PLC list still applies.
        cli(&dir, &["import", "archive", file.to_str().unwrap()]).unwrap();
        assert!(!lib.exists());
        let k1 = DeviceTag::parse("-K1").unwrap();
        let dev = &load(&dir).unwrap().devices[&k1];
        assert_eq!(dev.properties.get("Address").map(String::as_str), Some("%I0.0"));
        assert_eq!(dev.description, "Start");
        cli(
            &dir,
            &[
                "import",
                "archive",
                file.to_str().unwrap(),
                "--lib",
                lib.to_str().unwrap(),
            ],
        )
        .unwrap();
        let parts: Vec<_> = std::fs::read_dir(lib.join("parts")).unwrap().collect();
        assert_eq!(parts.len(), 1, "one part from parts.csv, nothing from notes.csv");
        // A zip with only a list that neither reader takes still opens and imports nothing.
        let only = tmp.path().join("only.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&only).unwrap());
            z.start_file("notes.csv", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(b"Name;Value\r\na;1\r\n").unwrap();
            z.finish().unwrap();
        }
        cli(&dir, &["import", "archive", only.to_str().unwrap()]).unwrap();
    }

    #[test]
    fn edz_parts_land_in_the_library_and_macros_are_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("contactor.edz");
        let xml = r#"<partsmanagement><part P_ARTICLE_PARTNR="SIE.3RT2015" P_ARTICLE_MANUFACTURER="SIE" P_ARTICLE_DESCR1="de_DE@Schütz;en_US@Contactor;"/></partsmanagement>"#;
        let mut w = sevenz_rust2::ArchiveWriter::new(std::io::Cursor::new(Vec::new())).unwrap();
        w.push_archive_entry(sevenz_rust2::ArchiveEntry::new_file("parts.xml"), Some(xml.as_bytes()))
            .unwrap();
        w.push_archive_entry(sevenz_rust2::ArchiveEntry::new_file("SIE_3RT2015.ema"), Some(&b"m"[..]))
            .unwrap();
        std::fs::write(&file, w.finish().unwrap().into_inner()).unwrap();
        let lib = tmp.path().join("lib");
        cli(
            tmp.path(),
            &[
                "import",
                "parts-edz",
                file.to_str().unwrap(),
                "--lib",
                lib.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert!(lib.join("parts").join("sie.3rt2015.json").exists());
        std::fs::remove_dir_all(&lib).unwrap();
        let (n, report, warnings) = import_edz(&file, &lib).unwrap();
        assert_eq!(n, 1, "{warnings:?}");
        assert_eq!(report.macros, 1);
        let json = std::fs::read_to_string(lib.join("parts").join("sie.3rt2015.json")).unwrap();
        let p: wc_core::Part = serde_json::from_str(&json).unwrap();
        assert_eq!(p.description, "Contactor");

        std::fs::write(&file, "not an archive").unwrap();
        assert!(import_edz(&file, &lib).is_err());
        let err = cli(
            tmp.path(),
            &[
                "import",
                "parts-edz",
                file.to_str().unwrap(),
                "--lib",
                lib.to_str().unwrap(),
            ],
        )
        .unwrap_err();
        assert!(err.contains("not an EDZ archive"), "{err}");
    }

    #[test]
    fn qet_elements_land_in_the_library_and_bad_files_are_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let good = tmp.path().join("Lamp Red.elmt");
        std::fs::write(
            &good,
            r#"<definition type="element"><names><name lang="en">Lamp</name></names><description>
<circle x="-5" y="-5" diameter="10"/><terminal x="-10" y="0" orientation="w"/><terminal x="10" y="0" orientation="e"/>
</description></definition>"#,
        )
        .unwrap();
        let bad = tmp.path().join("bad.elmt");
        std::fs::write(&bad, "<svg/>").unwrap();
        // Same id as the good file once normalised: refused, the first one is kept.
        let same = tmp.path().join("lamp_red.elmt");
        std::fs::write(
            &same,
            "<definition type=\"element\"><description><terminal x=\"0\" y=\"0\"/></description></definition>",
        )
        .unwrap();
        // Over the size limit: refused without reading it all.
        let huge = tmp.path().join("huge.elmt");
        std::fs::write(&huge, vec![b' '; wc_import::qet::MAX_LEN + 10]).unwrap();
        let lib = tmp.path().join("lib");
        let (written, failed) = import_qet(&[good, bad, same, huge], &lib, "qet.", 0.5, "Lamps", "P").unwrap();
        assert_eq!((written, failed), (1, 3));
        let json = std::fs::read_to_string(lib.join("symbols").join("qet.lamp_red.json")).unwrap();
        let s: wc_core::Symbol = serde_json::from_str(&json).unwrap();
        assert_eq!(
            (s.name.as_str(), s.category.as_str(), s.letter_code.as_str()),
            ("Lamp", "Lamps", "P")
        );
        assert_eq!(s.pins.len(), 2);

        let cli =
            Cli::try_parse_from(["wiringcraft", "import", "symbol-qet", "a.elmt", "b.elmt", "--lib", "l"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Import(ImportCmd::SymbolQet { ref files, scale, .. }) if files.len() == 2 && scale == 0.5
        ));
        assert!(Cli::try_parse_from(["wiringcraft", "import", "symbol-qet", "--lib", "l"]).is_err());
    }

    #[test]
    fn wago_marking_kind_option_splits_on_commas_and_repeats() {
        let cli = Cli::try_parse_from([
            "wiringcraft",
            "-C",
            "proj",
            "export",
            "wago-marking",
            "out.csv",
            "--kind",
            "terminals,wires",
            "--kind",
            "cables",
        ])
        .unwrap();
        assert_eq!(cli.project, PathBuf::from("proj"));
        assert!(matches!(
            cli.cmd,
            Cmd::Export(ExportCmd::WagoMarking { ref out, ref kinds })
                if out == &PathBuf::from("out.csv") && kinds == &["terminals", "wires", "cables"]
        ));

        let cli = Cli::try_parse_from(["wiringcraft", "export", "wago-marking", "out.csv"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Export(ExportCmd::WagoMarking { ref kinds, .. }) if kinds.is_empty()
        ));
        assert!(Cli::try_parse_from(["wiringcraft", "export", "wago-marking"]).is_err());
    }

    #[test]
    fn wago_marking_export_writes_the_list_and_refuses_unknown_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        let mut p = store::create(&dir, "Test").unwrap();
        p.devices
            .insert(wc_core::DeviceTag::parse("-W1").unwrap(), Default::default());
        p.pages[0].symbols.push(wc_core::PlacedSymbol {
            id: "s1".into(),
            symbol: "no.such.symbol".into(),
            at: wc_core::geom::Point::new(50.0, 50.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(wc_core::DeviceTag::parse("-X1.1").unwrap()),
        });
        store::save(&dir, &p).unwrap();

        let out = tmp.path().join("bad.csv");
        let err = export_wago_marking(&dir, &out, &["rails".into()]).err().unwrap();
        assert!(err.contains("rails"), "{err}");
        assert!(!out.exists(), "nothing written for an unknown kind");

        let out = tmp.path().join("m.csv");
        let done = export_wago_marking(&dir, &out, &["cables".into(), "devices".into()]).unwrap();
        assert_eq!(done.markers, 2);
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "\u{feff}Type;Group;Marker;Count\r\nDevice;;-X1.1;1\r\nCable;;-W1;2\r\n"
        );
        assert!(
            done.warnings.iter().any(|w| w.contains("no.such.symbol")),
            "{:?}",
            done.warnings
        );
    }
}
