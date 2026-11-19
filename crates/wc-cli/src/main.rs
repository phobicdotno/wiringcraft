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
        /// Start from a base project: a project folder or a git repository URL. Its pages,
        /// settings, title block, library sources and pictures are copied (not its review
        /// comments or history); `{name}` and `{date}` in its title block and page titles
        /// become the new project's name and today's date.
        #[arg(long, value_name = "FOLDER_OR_URL")]
        template: Option<String>,
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
    /// Draw a black box (device box) for a device without a symbol: a rectangle with its
    /// top-left corner at `--at`, and connection points on its edges that wires attach to like
    /// symbol pins. It is stored as a symbol of the project (`box.<page>.<id>`), so it carries a
    /// device tag and part and shows in the reports and checks like any symbol. Prints its id.
    Box {
        page: String,
        /// Top-left corner in mm, `x,y`.
        #[arg(long, value_parser = parse_point)]
        at: Point,
        /// Width and height in mm, `w,h`.
        #[arg(long, value_parser = parse_point)]
        size: Point,
        /// Device tag, e.g. `-A1` (taken as a value, though it starts with `-`).
        #[arg(long, allow_hyphen_values = true)]
        tag: Option<String>,
        /// A connection point, `name:side:offset`: side `left`, `right`, `top` or `bottom`, and
        /// the offset in mm along that edge from its left or top end, e.g. `L1:top:10`. Repeat
        /// for each point.
        #[arg(long = "pin")]
        pins: Vec<String>,
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
    /// Terminal strip editor: the order of a strip's terminals on the rail, spare terminals and
    /// terminal levels. The terminal plan and the WAGO configurator export follow it.
    #[command(subcommand)]
    Strip(StripCmd),
    /// The project logo, shown on the cover page and in every title block.
    #[command(subcommand)]
    Logo(LogoCmd),
    /// Macros: saved groups of symbols, wires and drawings in the project's `macros` folder or a
    /// library source's. A macro can hold placeholders (`{{name}}` in its device texts, wire
    /// values and drawn texts) and named value sets that fill them, one per variant.
    #[command(subcommand)]
    Macro(MacroCmd),
    /// The project's revisions, the clouds that mark their changes on pages, and the table
    /// that lists them.
    #[command(subcommand)]
    Revision(RevisionCmd),
    /// The page template: the frame's reference columns and the title block's boxes.
    #[command(subcommand)]
    Template(TemplateCmd),
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
        /// Append report pages: contents, parts, devices, wires, nets, terminals, cables, plc, plc-cards, locations, networks, or all.
        #[arg(long = "report")]
        reports: Vec<String>,
        /// Print the project's texts in this language from `translations.toml`, such as `nb`,
        /// or in two (`nb,en`: each text with the second language after it). Texts without a
        /// translation are printed as drawn and listed as warnings.
        #[arg(long)]
        lang: Option<String>,
    },
    /// The project's translation dictionary (`translations.toml`): list the texts that have no
    /// translation into a language. Exits with an error when any is missing, so it can guard a
    /// release.
    Translations {
        /// The language to check, such as `nb`.
        #[arg(long)]
        lang: String,
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
    /// Print a report (contents, parts, devices, wires, nets, terminals, cables, plc, plc-cards, locations, networks) as a table, CSV or Markdown,
    /// or save it as an Excel workbook (`--format xlsx --out <file>`, one sheet per report).
    Report {
        kind: String,
        /// `text`, `csv`, `md` or `xlsx`.
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
    /// potentials, open pins, wires ending in the air, devices without a part, plus the
    /// project's own `[[rule]]` tables in `project.toml` (a cable on every motor, a tag
    /// pattern, at most N wires on a terminal) and the house rules in each library source's
    /// `rules.toml`, less the ones `rules_off` turns off. Exits with an error when an error is
    /// found, so it can guard a commit or a CI job.
    Check {
        /// Also list notes (open pins, devices without a part).
        #[arg(long)]
        all: bool,
    },
    /// Find text in free texts, device tags, wire numbers, potentials and device descriptions
    /// on every page; one hit per line: page, kind, value. With --replace, replace it in every
    /// hit (a device tag renames the device everywhere it appears). Status lines go to stderr,
    /// so stdout holds only hits.
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
    /// tag order. In smartScript, import it as Excel/CSV data and map the Marker column (or
    /// the Line columns, one per text line of the marker).
    WagoMarking {
        out: PathBuf,
        #[command(flatten)]
        args: MarkingArgs,
    },
    /// Terminal strips for the WAGO configurator (smartDESIGNER): a semicolon-separated CSV
    /// (UTF-8) with, for every strip in natural tag order, its plates and DIN rails, its
    /// terminals in strip order (part, level, wire cross-section and colour on each side), its
    /// jumpers and its accessories. Each row has a stable Index for importing the list back.
    WagoConfigurator { out: PathBuf },
}

/// Options of `export wago-marking`.
#[derive(clap::Args, Debug, Default, PartialEq)]
struct MarkingArgs {
    /// Only these marker kinds: terminals, wires, devices, cables (default: all).
    #[arg(long = "kind", value_delimiter = ',')]
    kinds: Vec<String>,
    /// The text lines of each marker, one column each: marker (the engraving text, a device's
    /// `Marker` property, or its tag), tag, description, location. Default: one marker line.
    #[arg(long = "line", value_delimiter = ',')]
    lines: Vec<String>,
    /// Language of the header row and the type column: en, de, nb.
    #[arg(long, default_value = "en")]
    lang: String,
    /// Only the terminals of this strip (as the terminal plan names it, e.g. -X1).
    #[arg(long, allow_hyphen_values = true)]
    strip: Option<String>,
    /// Written between the numbers of a multi-level terminal's levels on its one marker
    /// (default " / "). A terminal's level is its `Level` property.
    #[arg(long, allow_hyphen_values = true)]
    level_separator: Option<String>,
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
enum MacroCmd {
    /// List the macros, project first, with their value sets.
    List,
    /// Place a macro on a page with its top-left corner at `--at`. Its symbols get the next
    /// free tags and its devices the macro's data, placeholders filled from `--set`. Prints
    /// the new symbol and wire ids.
    Place {
        page: String,
        /// The macro's name or file name.
        name: String,
        /// Position in mm, `x,y`.
        #[arg(long, value_parser = parse_point)]
        at: Point,
        /// The value set (variant) to use; needed when the macro has value sets.
        #[arg(long)]
        set: Option<String>,
        /// A placeholder value, `name=value`, over the value set's. Repeatable.
        #[arg(long = "value", value_name = "NAME=VALUE")]
        values: Vec<String>,
    },
}

#[derive(Subcommand)]
enum LogoCmd {
    /// Show the logo's file, if the project has one.
    Show,
    /// Make a PNG or JPEG picture the logo: it is copied into the project's `pictures` folder
    /// (a picture already there is used as it is). Its place on the cover page is kept.
    Set { file: PathBuf },
    /// Take the logo off the cover page and the title blocks; its file stays in the project.
    Remove,
}

#[derive(Subcommand)]
enum RevisionCmd {
    /// List the revisions, oldest first, and the clouds on each page.
    List,
    /// Add a revision after the others.
    Add {
        letter: String,
        /// What changed.
        #[arg(long, default_value = "")]
        text: String,
        #[arg(long, default_value = "")]
        date: String,
        /// Who made it.
        #[arg(long, default_value = "")]
        by: String,
    },
    /// Change a revision's description, date and author.
    Edit {
        letter: String,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        by: Option<String>,
    },
    /// Remove a revision; refused while a cloud carries it.
    Remove { letter: String },
    /// Draw a cloud on a page around the rectangle from (x1, y1) to (x2, y2), in mm.
    Cloud {
        page: String,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        /// The revision in its tag; the latest when left out.
        #[arg(long)]
        rev: Option<String>,
    },
    /// Remove a page's clouds: the one at an index `revision list` shows, or with `--rev` every
    /// cloud of that revision.
    Uncloud {
        page: String,
        index: Option<usize>,
        #[arg(long)]
        rev: Option<String>,
    },
    /// Show the revision table on a page with its top-left corner at (x, y) in mm, or take it
    /// off with `--off`.
    Table {
        page: String,
        x: Option<f64>,
        y: Option<f64>,
        #[arg(long)]
        off: bool,
    },
}

#[derive(Subcommand)]
enum TemplateCmd {
    /// Print the template as TOML (the built-in one when the project has none).
    Show,
    /// Set the number of reference columns across the frame (1 to 40).
    Columns { count: usize },
    /// Go back to the built-in layout (removes `[template]` from project.toml).
    Reset,
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
    /// Give the jumper holding a terminal a part (article), e.g.
    /// `jumper part -- -X1 3 wago.2002-402`; leave the part out to clear it. A part the project
    /// does not have yet is taken from its libraries.
    Part {
        strip: String,
        terminal: String,
        part: Option<String>,
    },
    /// List the project's jumpers.
    List,
}

#[derive(Subcommand)]
enum StripCmd {
    /// List terminal strips with their terminals in rail order; spares are marked `(spare)`.
    /// Give a strip to list only that one, e.g. `strip list -- -X1`.
    List { strip: Option<String> },
    /// Put a strip's terminals in this rail order, e.g. `strip order -- -X1 3 1 2`. Terminals
    /// left out follow at the end; no terminals goes back to the order by number.
    Order { strip: String, terminals: Vec<String> },
    /// Move a terminal to a place on the rail (1 is first), e.g. `strip move -- -X1 3 1`.
    Move { strip: String, terminal: String, to: usize },
    /// Add a spare terminal (on the rail, not drawn), e.g. `strip spare --after 2 -- -X1 S1`;
    /// without `--after` it goes at the end.
    Spare {
        strip: String,
        terminal: String,
        /// The terminal it goes right after.
        #[arg(long)]
        after: Option<String>,
    },
    /// Take a spare terminal off the strip (and out of its jumper), e.g.
    /// `strip unspare -- -X1 S1`.
    Unspare { strip: String, terminal: String },
    /// Set a terminal's level on a multi-level terminal (1 is the bottom), e.g.
    /// `strip level -- -X1 4 2`; leave the level out to clear the terminal's own `Level`
    /// property. Levels are inferred in number order, not rail order: a terminal whose next
    /// number up has level 2 or more is still level 1.
    Level {
        strip: String,
        terminal: String,
        level: Option<u32>,
    },
}

#[derive(Subcommand)]
enum PageCmd {
    Add {
        id: String,
        #[arg(long)]
        number: String,
        #[arg(long, default_value = "")]
        title: String,
        /// A4, A3, A2, A1 or A0, landscape unless followed by `-portrait` (`A4-portrait`), or
        /// a sheet of its own as WIDTHxHEIGHT in millimetres (`500x700`).
        #[arg(long, default_value = "A3")]
        size: String,
        /// An overview (line diagram): its devices do not count as drawings in `check`.
        #[arg(long)]
        overview: bool,
        /// The project's cover page: shows the project's name and fields. A project has one
        /// cover page, so any other page loses the mark.
        #[arg(long)]
        cover: bool,
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
    /// Change a page's sheet size (as `page add --size` takes it). What is drawn stays where it
    /// is; a smaller sheet can leave some of it outside the frame.
    Size {
        id: String,
        size: String,
    },
    /// Make a page the project's cover page (any other page loses the mark), or with `--off` a
    /// normal page again.
    Cover {
        id: String,
        #[arg(long)]
        off: bool,
    },
    /// Show a page's title-block fields, or set the page's own with `key=value` (revision,
    /// date, drawn_by, checked_by, ...). The page's own values win over the project's on that
    /// page; `key=` clears one, so the project's shows again.
    Fields {
        id: String,
        /// `key=value` pairs to set; none lists the fields.
        set: Vec<String>,
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
    /// A maker's outline drawing (DXF, the front view, in mm at 1:1) as a part's footprint on
    /// cabinet layouts: stored in the library with the part, and drawn on layout pages instead
    /// of a box. A part without `Width` and `Height` takes its size from the drawing. When the
    /// project uses the part, its copy in the project is updated too.
    FootprintDxf {
        file: PathBuf,
        /// The part's library id.
        #[arg(long)]
        part: String,
        /// The library folder that holds the part (default: the project's library source that
        /// has it).
        #[arg(long)]
        lib: Option<PathBuf>,
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
    /// PLC addresses from a CSV list or an Excel workbook (device and address columns,
    /// description optional), as a PLC tool or spreadsheet writes it. `report plc --format csv`
    /// (or `xlsx`) writes one that reads back.
    PlcCsv { file: PathBuf },
    /// The WAGO configurator list back: a list written by `export wago-configurator`, with the
    /// articles and accessories chosen in the configurator. Each row is matched by its Index to
    /// the terminal or strip device it stands for, and its article to a part of the project or
    /// its libraries (by order number, or part id); matched articles become the devices' parts
    /// and accessories. Rows that cannot be matched are listed with the reason.
    Wago { file: PathBuf },
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
    /// description and the EPLAN part properties. Each macro's multi-line representation,
    /// variant A, becomes a library symbol (`eplan.<macro name>`) with its connection points as
    /// pins, and each part gets the symbol made from the macro it names; what the macros leave
    /// out is reported. Pictures are counted.
    PartsEdz {
        file: PathBuf,
        #[arg(long)]
        lib: PathBuf,
        /// Which library symbol to give parts by their macro: a text file with one
        /// `macro = symbol.id` per line. The macro is matched by its full name, its file name or
        /// its file name without `.ema`, ignoring case.
        #[arg(long)]
        symbol_map: Option<PathBuf>,
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
    /// Parts from a CSV or semicolon-separated export, or straight from an Excel workbook
    /// (`.xlsx`, its first visible sheet), with the same column names either way.
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
    /// (tew_project_data.sqlite): its schematic and line-diagram pages, symbols drawn from their
    /// DWG blocks (boxes when the block has no readable drawing, and always for a bare database)
    /// with their connection points at the original positions, the pages' own texts and frames,
    /// wires joined to the symbols, and devices with part data, linked to the archive catalog's
    /// part when it has it.
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
    /// terminals, cable types with their cores, and symbol blocks with their connection points,
    /// drawn from the archive's block DWG files (boxes where a block has none). A symbol or part
    /// the project already vendors is replaced by the new version (an earlier import's box by
    /// the drawn block) when its copy is still the one the project pins; a copy changed in the
    /// project is refused, not replaced. To take the new version into such a project, import
    /// with --lib into a library folder, add that folder as a source (`lib add`) ahead of any
    /// source with the same id, and run `lib update <id>`. Into a library folder with --lib,
    /// else vendored into the project.
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

/// A black box's connection point from `name:side:offset` (`L1:top:10`); the name may itself
/// hold a colon.
fn parse_box_pin(s: &str) -> Res<wc_core::BoxPin> {
    let mut parts = s.rsplitn(3, ':');
    let (Some(offset), Some(side), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!("{s:?}: expected name:side:offset, e.g. L1:top:10"));
    };
    let side =
        wc_core::BoxSide::parse(side).ok_or_else(|| format!("{s:?}: the side must be left, right, top or bottom"))?;
    let offset = offset
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("{s:?}: the offset must be a number of mm"))?;
    Ok(wc_core::BoxPin {
        name: name.to_owned(),
        side,
        offset,
    })
}

/// The project to keep output off while someone else holds one of its pages in a shared
/// session: the app names it ([`wc_core::guard::GUARD_ENV`]), else none.
fn output_guard() -> Option<&'static wc_core::guard::ProjectFiles> {
    static GUARD: std::sync::OnceLock<Option<wc_core::guard::ProjectFiles>> = std::sync::OnceLock::new();
    GUARD.get_or_init(wc_core::guard::ProjectFiles::from_env).as_ref()
}

/// Write an output file the user named. Under [`output_guard`] the file is written so that it
/// cannot land on a project file, a link swapped in after the app's own look included (#458).
fn write_out(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    wc_core::guard::write_output(output_guard(), path.as_ref(), bytes)
}

/// Make the output folder `out`; under [`output_guard`] a folder in the project is refused
/// before any folder is made.
fn make_out_dir(out: &Path) -> Res<()> {
    if let Some(g) = output_guard() {
        match g.holds(out) {
            Ok(false) => {}
            Ok(true) => {
                return Err(format!(
                    "{} is in the project folder, and someone else has a page of the project open in the shared session",
                    out.display()
                ));
            }
            Err(why) => {
                return Err(format!(
                    "{} could not be told apart from the project's files ({why})",
                    out.display()
                ));
            }
        }
    }
    std::fs::create_dir_all(out).map_err(e)
}

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

/// Says which pages stopped being the cover page when another one took the mark.
fn cover_moved(ids: Vec<String>) {
    for id in ids {
        eprintln!("page {id:?} is no longer the cover page");
    }
}

fn load(dir: &Path) -> Res<Project> {
    store::load(dir).map_err(e)
}

/// The project with its texts in `lang` (one language code, or two joined by `,`) from the
/// project's `translations.toml`, and the texts that have no translation.
fn translated(dir: &Path, p: &Project, lang: &str) -> Res<(Project, Vec<wc_core::translate::Missing>)> {
    use wc_core::translate::{Dictionary, parse_languages, translate};
    let langs = parse_languages(lang)
        .ok_or_else(|| format!("bad language {lang:?}: use a code such as nb, or two as nb,en"))?;
    let dict = Dictionary::load(dir).map_err(e)?;
    Ok(translate(p, &dict, &langs))
}

/// Save the project. Jumpers and strip layouts first follow their terminals: what the command
/// renamed, moved or deleted is found by comparing with the project as it is on disk.
fn save(dir: &Path, p: &Project) -> Res<()> {
    let mut p = p.clone();
    // A new project has nothing on disk to compare with; any other load failure is reported.
    let on_disk = dir.join(store::PROJECT_FILE).exists();
    if p.meta.follows_terminals() && on_disk {
        let old = store::load(dir).map_err(|err| format!("jumpers not updated: {}", e(err)))?;
        let (old_symbols, _) = store::vendored_symbols(dir, &old);
        let before = wc_core::project::terminal_numbers(&old, |id| old_symbols.get(id).map(|s| s.role));
        let (symbols, _) = store::vendored_symbols(dir, &p);
        wc_core::project::follow_terminals_in(&mut p, &before, |id| symbols.get(id).map(|s| s.role));
    }
    store::save(dir, &p).map(|_| ()).map_err(e)
}

/// Save a project whose jumpers already follow their terminals: after `add_built`, which
/// moves the project's own jumpers itself and adds the imported ones with their final numbers.
fn save_imported(dir: &Path, p: &Project) -> Res<()> {
    store::save(dir, p).map(|_| ()).map_err(e)
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
/// `\n` and `\r`, other control characters (such as ESC) as `\u{1b}`, so a multi-line text
/// stays one hit per line and a found value cannot send the terminal control sequences.
fn escape_field(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            // ESC and other control characters could steer the terminal.
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
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
            template,
        } => {
            let name = name.unwrap_or_else(|| {
                new_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Project".into())
            });
            match template {
                Some(t) => {
                    wc_vcs::base::create_from_base(&t, &new_dir, &name, &wc_core::base::today()).map_err(e)?;
                }
                None => {
                    store::create(&new_dir, &name).map_err(e)?;
                }
            }
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
                    write_out(&out, &step).map_err(e)?;
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
                    cover,
                } => {
                    store::check_id(&id).map_err(e)?;
                    if p.page(&id).is_some() {
                        return Err(format!("page {id:?} exists"));
                    }
                    let mut page = Page::new(id, number, title);
                    page.overview = overview;
                    page.size = PageSize::parse(&size)?;
                    p.pages.push(page);
                    if cover {
                        let i = p.pages.len() - 1;
                        cover_moved(p.set_cover(i, true).unwrap_or_default());
                    }
                    p.sort_pages();
                }
                PageCmd::List => {
                    for pg in &p.pages {
                        println!(
                            "{:<10} {:<6} {} ({} symbols, {} wires){}",
                            pg.id,
                            pg.number,
                            escape_field(&pg.title),
                            pg.symbols.len(),
                            pg.wires.len(),
                            match (pg.cover, pg.overview) {
                                (true, true) => ", cover, overview",
                                (true, false) => ", cover",
                                (false, true) => ", overview",
                                (false, false) => "",
                            }
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
                PageCmd::Size { id, size } => {
                    let size = PageSize::parse(&size)?;
                    let page = p
                        .pages
                        .iter_mut()
                        .find(|pg| pg.id == id)
                        .ok_or_else(|| format!("no page {id:?}"))?;
                    page.size = size;
                }
                PageCmd::Cover { id, off } => {
                    let i = p
                        .pages
                        .iter()
                        .position(|pg| pg.id == id)
                        .ok_or_else(|| format!("no page {id:?}"))?;
                    cover_moved(p.set_cover(i, !off).unwrap_or_default());
                }
                PageCmd::Fields { id, set } => {
                    let i = p
                        .pages
                        .iter()
                        .position(|pg| pg.id == id)
                        .ok_or_else(|| format!("no page {id:?}"))?;
                    if set.is_empty() {
                        print!("{}", page_fields(&p, i));
                        return Ok(());
                    }
                    let mut pairs = Vec::new();
                    for kv in &set {
                        let (k, v) = kv.split_once('=').ok_or_else(|| format!("{kv:?} is not key=value"))?;
                        let k = k.trim();
                        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                            return Err(format!("{k:?} is not a field name (letters, digits and _)"));
                        }
                        pairs.push((k.to_owned(), v.to_owned()));
                    }
                    if let Some(page) = p.pages.get_mut(i) {
                        for (k, v) in pairs {
                            page.set_title_field(&k, &v);
                        }
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
        Cmd::Box {
            page,
            at,
            size,
            tag,
            pins,
        } => {
            let mut p = load(dir)?;
            let tag = tag.map(|t| DeviceTag::parse(&t)).transpose().map_err(e)?;
            let pins = pins.iter().map(|s| parse_box_pin(s)).collect::<Res<Vec<_>>>()?;
            let lock = p.lock.symbols.clone();
            let pg = page_mut(&mut p, &page)?;
            // The first item id whose box symbol is free too: a box deleted from the page keeps
            // its symbol file and lock entry, and its id must not be taken over.
            let (id, symbol_id, symbol_path) = {
                let mut n = 1usize;
                loop {
                    let id = format!("s{n}");
                    n += 1;
                    if pg.symbol(&id).is_some() || pg.wires.iter().any(|w| w.id == id) {
                        continue;
                    }
                    let symbol_id = format!("box.{}.{id}", pg.id);
                    if symbol_id.len() > 128 {
                        return Err(format!(
                            "page id {:?} is too long to name a box symbol (box.<page>.<id> must stay within 128 characters)",
                            pg.id
                        ));
                    }
                    let path = store::vendor_path(dir, ItemKind::Symbol, &symbol_id).map_err(e)?;
                    if !lock.contains_key(&symbol_id) && !path.exists() {
                        break (id, symbol_id, path);
                    }
                }
            };
            let name = tag
                .as_ref()
                .map_or_else(|| "Black box".to_owned(), |t| format!("Black box {t}"));
            let symbol = wc_core::black_box(&symbol_id, &name, size.x, size.y, &pins).map_err(e)?;
            pg.symbols.push(PlacedSymbol {
                id: id.clone(),
                symbol: symbol_id.clone(),
                at,
                rotation: Rotation::R0,
                mirror: false,
                tag: tag.clone(),
            });
            if let Some(t) = &tag {
                p.devices.entry(t.clone()).or_default();
            }
            p.lock.symbols.insert(
                symbol_id.clone(),
                wc_core::LockEntry {
                    source: "project".into(),
                    hash: symbol.hash(),
                },
            );
            // Every project file the save may change, read first: a save that fails half way is
            // put back, so no lock entry or placement is left pointing at the symbol file.
            let before = snapshot_project(dir, &[], &[])?;
            store::write_vendored_symbol(dir, &symbol).map_err(e)?;
            if let Err(err) = save(dir, &p) {
                let failed = put_files_back(&before);
                if !failed.is_empty() {
                    // A page or the lock may still name the box: keep its symbol file.
                    return Err(format!(
                        "{err}; putting back the project files failed too, so the box's symbol {} is kept: {}",
                        symbol_path.display(),
                        failed.join("; ")
                    ));
                }
                return match std::fs::remove_file(&symbol_path) {
                    Ok(()) => Err(err),
                    Err(x) if x.kind() == std::io::ErrorKind::NotFound => Err(err),
                    Err(x) => Err(format!(
                        "{err}; removing the box's symbol {} failed too: {x}",
                        symbol_path.display()
                    )),
                };
            }
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
        Cmd::Macro(mc) => {
            let mut p = load(dir)?;
            let mut found: Vec<(String, String, PathBuf)> = wc_core::macros::list_macros(dir)
                .into_iter()
                .map(|(id, path)| ("project".to_owned(), id, path))
                .collect();
            // A library that cannot be opened is reported; the project's macros stay usable.
            match wc_library::Library::open(dir, &p) {
                Ok(lib) => found.extend(lib.macros()),
                Err(err) => eprintln!("warning: the library's macros are not listed: {err}"),
            }
            match mc {
                MacroCmd::List => {
                    for (source, id, path) in &found {
                        match wc_core::macros::read_macro(path) {
                            Ok(m) => {
                                let sets: Vec<&str> = m.value_sets.iter().map(|v| v.name.as_str()).collect();
                                let sets = if sets.is_empty() {
                                    String::new()
                                } else {
                                    format!("  [{}]", sets.join(", "))
                                };
                                let problem = m.check().err().map(|e| format!("  ({e})")).unwrap_or_default();
                                println!("{source}\t{id}\t{}{sets}{problem}", m.name);
                            }
                            Err(err) => println!("{source}\t{id}\t({err})"),
                        }
                    }
                }
                MacroCmd::Place {
                    page,
                    name,
                    at,
                    set,
                    values,
                } => {
                    let mut wanted = None;
                    let file = name.trim();
                    let file = file.strip_suffix(".json").unwrap_or(file);
                    for (_, id, path) in &found {
                        if id.eq_ignore_ascii_case(file) {
                            wanted = Some(path.clone());
                            break;
                        }
                        if wanted.is_none()
                            && wc_core::macros::read_macro(path).is_ok_and(|m| m.name.eq_ignore_ascii_case(name.trim()))
                        {
                            wanted = Some(path.clone());
                        }
                    }
                    let path = wanted.ok_or_else(|| format!("no macro {name:?}"))?;
                    let mut typed = std::collections::BTreeMap::new();
                    for v in &values {
                        let (k, val) = v
                            .split_once('=')
                            .ok_or_else(|| format!("--value {v:?}: write it as name=value"))?;
                        typed.insert(k.trim().to_owned(), val.to_owned());
                    }
                    let m = wc_core::macros::read_macro(&path)
                        .and_then(|m| m.apply(set.as_deref(), &typed))
                        .map_err(e)?;
                    // A pinned symbol keeps its pinned version: a missing copy is put back from
                    // a library that still has that version, never from a newer one. A symbol
                    // without a pin is taken from the library and pinned, also when a copy is
                    // left from an earlier attempt that failed before it was pinned.
                    let symbols: std::collections::BTreeSet<&str> =
                        m.symbols.iter().map(|s| s.symbol.as_str()).collect();
                    // The library is opened only when a symbol needs it, so a macro whose
                    // symbols all have their pinned copies works without it.
                    let mut lib: Option<wc_library::Library> = None;
                    for id in symbols {
                        let pin = p.lock.symbols.get(id).cloned();
                        if pin.is_some() {
                            let vendored = store::vendor_path(dir, ItemKind::Symbol, id).map_err(e)?;
                            if vendored.exists() {
                                // A copy that cannot be read is reported now, not after saving.
                                store::read_vendored_symbol(dir, id).map_err(e)?;
                                continue;
                            }
                        }
                        let lib = match &mut lib {
                            Some(lib) => &*lib,
                            None => &*lib.insert(wc_library::Library::open(dir, &p).map_err(e)?),
                        };
                        match pin {
                            Some(pin) => {
                                let fits = |s: &wc_core::Symbol| {
                                    if s.id != id {
                                        Err(format!("its file holds {}", s.id))
                                    } else if s.hash() != pin.hash {
                                        Err("another version".to_owned())
                                    } else {
                                        Ok(())
                                    }
                                };
                                let s = lib.find_version(ItemKind::Symbol, id, &pin.source, fits).map_err(|why| {
                                    format!(
                                        "symbol {id}: its copy in the project is missing and no library has the version the project pins ({why})"
                                    )
                                })?;
                                store::write_vendored_symbol(dir, &s).map_err(e)?;
                            }
                            None => {
                                wc_library::take(dir, &mut p, lib, ItemKind::Symbol, id).map_err(e)?;
                            }
                        }
                    }
                    let page_id = page_mut(&mut p, &page)?.id.clone();
                    let ids = wc_core::macros::place(&mut p, &page_id, &m, at).map_err(e)?;
                    save(dir, &p)?;
                    println!("{}", ids.join(" "));
                }
            }
        }
        Cmd::Logo(lc) => {
            let mut p = load(dir)?;
            match lc {
                LogoCmd::Show => match &p.meta.logo {
                    Some(l) => println!("{}", l.file),
                    None => println!("no logo"),
                },
                LogoCmd::Set { file } => {
                    let file = wc_core::Logo::copy_into(dir, &file)?;
                    let cover = p.meta.logo.as_ref().and_then(|l| l.cover);
                    p.meta.logo = Some(wc_core::Logo {
                        file: file.clone(),
                        cover,
                    });
                    save(dir, &p)?;
                    println!("{file}");
                }
                LogoCmd::Remove => {
                    if p.meta.logo.take().is_some() {
                        save(dir, &p)?;
                    }
                }
            }
        }
        Cmd::Revision(rc) => {
            let mut p = load(dir)?;
            match rc {
                RevisionCmd::List => {
                    for r in &p.meta.revisions {
                        println!(
                            "{}\t{}\t{}\t{}",
                            escape_field(&r.letter),
                            escape_field(&r.date),
                            escape_field(&r.author),
                            escape_field(&r.text)
                        );
                    }
                    for pg in &p.pages {
                        for (i, c) in pg.clouds.iter().enumerate() {
                            println!(
                                "page {} cloud {i}: revision {} at {:.1},{:.1} to {:.1},{:.1}",
                                escape_field(&pg.id),
                                escape_field(&c.revision),
                                c.min.x,
                                c.min.y,
                                c.max.x,
                                c.max.y
                            );
                        }
                        if let Some(at) = pg.revision_table {
                            println!("page {} table at {:.1},{:.1}", escape_field(&pg.id), at.x, at.y);
                        }
                    }
                }
                RevisionCmd::Add { letter, text, date, by } => {
                    p.add_revision(&letter, &date, &text, &by).map_err(e)?;
                    save(dir, &p)?;
                }
                RevisionCmd::Edit { letter, text, date, by } => {
                    let r = p
                        .revision(&letter)
                        .cloned()
                        .ok_or_else(|| format!("no revision {}", letter.trim()))?;
                    let changed = p
                        .edit_revision(
                            &letter,
                            date.as_deref().unwrap_or(&r.date),
                            text.as_deref().unwrap_or(&r.text),
                            by.as_deref().unwrap_or(&r.author),
                        )
                        .map_err(e)?;
                    if changed {
                        save(dir, &p)?;
                    }
                }
                RevisionCmd::Remove { letter } => {
                    p.remove_revision(&letter).map_err(e)?;
                    save(dir, &p)?;
                }
                RevisionCmd::Cloud {
                    page,
                    x1,
                    y1,
                    x2,
                    y2,
                    rev,
                } => {
                    let i = p
                        .add_cloud(&page, Point::new(x1, y1), Point::new(x2, y2), rev.as_deref())
                        .map_err(e)?;
                    save(dir, &p)?;
                    println!("{i}");
                }
                RevisionCmd::Uncloud { page, index, rev } => {
                    let pg = p.page_mut(&page).ok_or_else(|| format!("no page {page:?}"))?;
                    let before = pg.clouds.len();
                    match (index, rev) {
                        (Some(i), None) if i < before => {
                            pg.clouds.remove(i);
                        }
                        (Some(i), None) => return Err(format!("page {page:?} has no cloud {i}")),
                        (None, Some(r)) => pg.clouds.retain(|c| c.revision != r.trim()),
                        _ => return Err("give a cloud's index or --rev, not both".into()),
                    }
                    if pg.clouds.len() != before {
                        save(dir, &p)?;
                    }
                }
                RevisionCmd::Table { page, x, y, off } => {
                    let at = match (x, y, off) {
                        (Some(x), Some(y), false) => Some(Point::new(x, y)),
                        (None, None, true) => None,
                        _ => return Err("give x and y, or --off".into()),
                    };
                    if p.set_revision_table(&page, at).map_err(e)? {
                        save(dir, &p)?;
                    }
                }
            }
        }
        Cmd::Template(tc) => {
            let mut p = load(dir)?;
            match tc {
                TemplateCmd::Show => {
                    let text = toml::to_string_pretty(&p.meta.template).map_err(|e| e.to_string())?;
                    print!("{text}");
                }
                TemplateCmd::Columns { count } => {
                    let max = wc_core::template::MAX_COLUMNS;
                    if !(1..=max).contains(&count) {
                        return Err(format!("the column count must be 1 to {max}"));
                    }
                    if p.meta.template.columns != count {
                        p.meta.template.columns = count;
                        save(dir, &p)?;
                    }
                }
                TemplateCmd::Reset => {
                    // Saved even when it already is the built-in layout: a file that spells the
                    // built-in layout out loses its `[template]` too.
                    p.meta.template = Default::default();
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
                JumperCmd::Part { strip, terminal, part } => {
                    let strip = DeviceTag::parse(&strip).map_err(e)?;
                    let part = part.map(|p| p.trim().to_owned()).filter(|p| !p.is_empty());
                    // Checked first, so a part is not taken into a project for nothing.
                    if p.meta
                        .clone()
                        .set_jumper_part(&strip, terminal.trim(), part.as_deref())?
                        && let Some(part) = &part
                        && store::read_vendored_part(dir, part).is_err()
                    {
                        let lib = wc_library::Library::open(dir, &p).map_err(e)?;
                        wc_library::take(dir, &mut p, &lib, ItemKind::Part, part).map_err(e)?;
                    }
                    if p.meta.set_jumper_part(&strip, terminal.trim(), part.as_deref())? {
                        save(dir, &p)?;
                    }
                }
                JumperCmd::List => {
                    for j in &p.meta.jumpers {
                        match &j.part {
                            Some(part) => println!("{} {} ({part})", j.strip, j.terminals.join(" ")),
                            None => println!("{} {}", j.strip, j.terminals.join(" ")),
                        }
                    }
                }
            }
        }
        Cmd::Strip(sc) => {
            let mut p = load(dir)?;
            let (symbols, _) = store::vendored_symbols(dir, &p);
            let rail = |p: &Project, strip: &DeviceTag| -> Vec<(String, bool)> {
                wc_core::project::strip_terminals(p, |id| symbols.get(id).map(|s| s.role))
                    .remove(strip)
                    .unwrap_or_default()
            };
            match sc {
                StripCmd::List { strip } => {
                    let only = strip.map(|s| DeviceTag::parse(&s).map_err(e)).transpose()?;
                    let all = wc_core::project::strip_terminals(&p, |id| symbols.get(id).map(|s| s.role));
                    for (strip, list) in all {
                        if only.as_ref().is_some_and(|o| *o != strip) {
                            continue;
                        }
                        let terms: Vec<String> = list
                            .iter()
                            .map(|(t, spare)| {
                                let t = escape_field(t);
                                if *spare { format!("{t} (spare)") } else { t }
                            })
                            .collect();
                        println!("{strip} {}", terms.join(" "));
                    }
                }
                StripCmd::Order { strip, terminals } => {
                    let strip = DeviceTag::parse(&strip).map_err(e)?;
                    if p.meta.set_terminal_order(&strip, &terminals)? {
                        save(dir, &p)?;
                    }
                }
                StripCmd::Move { strip, terminal, to } => {
                    let strip = DeviceTag::parse(&strip).map_err(e)?;
                    if to == 0 {
                        return Err("places on the rail count from 1".into());
                    }
                    let current = rail(&p, &strip);
                    if p.meta.move_terminal(&strip, terminal.trim(), to - 1, &current)? {
                        save(dir, &p)?;
                    }
                }
                StripCmd::Spare { strip, terminal, after } => {
                    let strip = DeviceTag::parse(&strip).map_err(e)?;
                    let current = rail(&p, &strip);
                    p.add_spare(&strip, &terminal, after.as_deref().map(str::trim), &current)?;
                    save(dir, &p)?;
                }
                StripCmd::Unspare { strip, terminal } => {
                    let strip = DeviceTag::parse(&strip).map_err(e)?;
                    let current = rail(&p, &strip);
                    let terminal = terminal.trim();
                    if !p.remove_spare(&strip, terminal, &current) {
                        // Not a spare (any more), but its device entry may be left behind.
                        let no_spare = format!("{strip} has no spare terminal {terminal}");
                        match p.remove_unused_terminal_entry(&strip, terminal, &current) {
                            Ok(true) => println!("{no_spare}; removed its unused device entry"),
                            Ok(false) => return Err(no_spare),
                            Err(why) => return Err(format!("{no_spare}; {why}")),
                        }
                    }
                    save(dir, &p)?;
                }
                StripCmd::Level { strip, terminal, level } => {
                    let strip = DeviceTag::parse(&strip).map_err(e)?;
                    if level == Some(0) {
                        return Err("levels start at 1".into());
                    }
                    let terminal = terminal.trim();
                    if !rail(&p, &strip).iter().any(|(t, _)| t == terminal) {
                        return Err(format!("{strip} has no terminal {terminal}"));
                    }
                    let tag = DeviceTag::parse(&format!("{strip}.{terminal}")).map_err(e)?;
                    let before = p.devices.get(&tag).cloned();
                    let d = p.devices.entry(tag.clone()).or_default();
                    match level {
                        Some(l) => {
                            d.properties.insert(wc_report::LEVEL_PROPERTY.into(), l.to_string());
                        }
                        None => {
                            d.properties.remove(wc_report::LEVEL_PROPERTY);
                        }
                    }
                    if before.is_none() && level.is_none() {
                        p.devices.remove(&tag);
                    }
                    if p.devices.get(&tag) != before.as_ref() {
                        save(dir, &p)?;
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
            lang,
        } => {
            let p = load(dir)?;
            let p = match lang {
                Some(lang) => {
                    let (p, missing) = translated(dir, &p, &lang)?;
                    for m in &missing {
                        eprintln!(
                            "warning: no translation for {:?} ({})",
                            escape_field(&m.text),
                            escape_field(&m.used.join(", "))
                        );
                    }
                    p
                }
                None => p,
            };
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
            warn_report_layouts(&inputs);
            // `all` leaves out the PLC card overview of a project without racks and the network
            // list of one without networks, so it does not gain an empty page; naming
            // `plc-cards` or `networks` prints it anyway.
            let named = |k: wc_report::Kind| reports.iter().any(|r| r == k.name());
            let kinds: Vec<wc_report::Kind> = report_kinds(&reports)?
                .into_iter()
                .filter(|k| match k {
                    wc_report::Kind::PlcCards => named(*k) || !p.meta.racks.is_empty(),
                    wc_report::Kind::Networks => named(*k) || !p.meta.networks.is_empty(),
                    _ => true,
                })
                .collect();
            // The PLC card overview is printed as a table and as a drawn sheet per card.
            let plc_cards = if kinds.contains(&wc_report::Kind::PlcCards) {
                wc_report::plc_card_sheets(&inputs)
            } else {
                Vec::new()
            };
            let tables = kinds.into_iter().map(|k| wc_report::build(k, &inputs)).collect();
            let pictures = wc_pdf::read_pictures(dir, &p);
            let (bytes, report) = wc_pdf::export(
                &p,
                &symbols,
                &wc_pdf::Options {
                    pages,
                    date,
                    wire_labels: true,
                    reports: tables,
                    underlays: pictures.files,
                    unreadable: pictures.unreadable,
                    panels: p.panels.clone(),
                    footprints: wc_pdf::footprints(vendored_parts(dir, &p).values()),
                    plc_cards,
                },
            );
            write_out(&out, bytes).map_err(e)?;
            for w in &report.warnings {
                eprintln!("warning: {}", escape_field(w));
            }
            println!("wrote {} ({} pages)", out.display(), report.pages);
        }
        Cmd::Translations { lang } => {
            let p = load(dir)?;
            let (_, missing) = translated(dir, &p, &lang)?;
            for m in &missing {
                println!("{}\t{}", escape_field(&m.text), escape_field(&m.used.join(", ")));
            }
            if !missing.is_empty() {
                return Err(format!("{} text(s) have no translation into {lang}", missing.len()));
            }
            println!("every text has a translation into {lang}");
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
            // Only SVG shows pictures; a DXF export neither reads nor checks them.
            let (drawings, report) = if ext == "svg" {
                let pictures = wc_pdf::read_pictures(dir, &p);
                let options = wc_pdf::Options {
                    pages,
                    date,
                    underlays: pictures.files,
                    unreadable: pictures.unreadable,
                    ..Default::default()
                };
                wc_pdf::page_drawings(&p, &symbols, &options)
            } else {
                let options = wc_pdf::Options {
                    pages,
                    date,
                    ..Default::default()
                };
                wc_pdf::page_lines(&p, &symbols, &options)
            };
            make_out_dir(&out)?;
            let mut used = std::collections::BTreeSet::new();
            for d in &drawings {
                let mut stem = file_stem(&d.number, &d.id);
                while !used.insert(stem.clone()) {
                    stem.push('_');
                }
                write_out(out.join(format!("{stem}.{ext}")), render(d)).map_err(e)?;
            }
            for w in &report.warnings {
                eprintln!("warning: {}", escape_field(w));
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
                eprintln!("replaced {n} value(s)");
            } else if hits.is_empty() {
                eprintln!("nothing found");
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
            let found = wc_report::check_with(&inputs, &wc_library::library_rules(dir, &p));
            let count = |s: wc_report::Severity| found.iter().filter(|f| f.severity == s).count();
            for f in found.iter().filter(|f| all || f.severity != wc_report::Severity::Note) {
                let page = f
                    .page_id
                    .as_deref()
                    .and_then(|id| p.page(id))
                    .map(|pg| format!("page {}: ", pg.number))
                    .unwrap_or_default();
                println!("{:<7} {page}{}", f.severity.label(), escape_field(&f.message));
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
            warn_report_layouts(&inputs);
            let kinds = report_kinds(std::slice::from_ref(&kind))?;
            if format == "xlsx" {
                let Some(path) = out else {
                    return Err("an Excel workbook is not text: pass --out <file>.xlsx".into());
                };
                let tables: Vec<wc_report::Table> = kinds.iter().map(|k| wc_report::build(*k, &inputs)).collect();
                let refs: Vec<&wc_report::Table> = tables.iter().collect();
                let bytes = wc_report::xlsx::workbook(&refs).map_err(e)?;
                write_out(&path, bytes).map_err(e)?;
                return Ok(());
            }
            let mut text = String::new();
            for k in kinds {
                let t = wc_report::build(k, &inputs);
                text.push_str(&match format.as_str() {
                    "csv" => t.to_csv(),
                    "md" | "markdown" => t.to_markdown(),
                    "text" => plain_table(&t),
                    other => return Err(format!("unknown format {other:?} (text, csv, md, xlsx)")),
                });
                text.push('\n');
            }
            match out {
                Some(path) => write_out(&path, text).map_err(e)?,
                None => print!("{text}"),
            }
        }
        Cmd::Export(ExportCmd::WagoMarking { out, args }) => {
            let done = export_wago_marking(dir, &out, &args)?;
            for w in &done.warnings {
                eprintln!("warning: {}", escape_field(w));
            }
            println!("wrote {} ({} markers)", out.display(), done.markers);
        }
        Cmd::Export(ExportCmd::WagoConfigurator { out }) => {
            let done = export_wago_configurator(dir, &out)?;
            for w in &done.warnings {
                eprintln!("warning: {}", escape_field(w));
            }
            println!("wrote {} ({} rows)", out.display(), done.markers);
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
            write_out(&out, text).map_err(e)?;
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
            write_out(&out, &v.text).map_err(e)?;
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
                    wc_library::State::WrongId => "library file declares another id (not taken)",
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
            let left = wc_library::lock_missing(dir, &mut p, &lib).map_err(e)?;
            save(dir, &p)?;
            for w in lock_warnings(&left) {
                eprintln!("warning: {}", escape_field(&w));
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
                eprintln!("warning: {}", escape_field(w));
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
        ImportCmd::FootprintDxf { file, part, lib } => {
            let text = std::fs::read(&file).map_err(e)?;
            let (footprint, warnings) = wc_import::dxf::footprint(&String::from_utf8_lossy(&text)).map_err(e)?;
            for w in &warnings {
                eprintln!("warning: {}", escape_field(w));
            }
            // A library folder can be given outside a project; without one the project is needed.
            let mut project = load(dir);
            if lib.is_some()
                && dir.join(store::PROJECT_FILE).exists()
                && let Err(err) = &project
            {
                eprintln!(
                    "warning: the project could not be read, so its copy of {part} is not updated: {}",
                    escape_field(err)
                );
            }
            let path = match &lib {
                Some(root) => {
                    store::check_id(&part).map_err(e)?;
                    root.join("parts").join(format!("{part}.json"))
                }
                None => {
                    let p = project.as_ref().map_err(Clone::clone)?;
                    let library = wc_library::Library::open(dir, p).map_err(e)?;
                    library
                        .locate(ItemKind::Part, &part)
                        .ok_or_else(|| {
                            format!("no part {part:?} in the project's libraries; name its folder with --lib")
                        })?
                        .path
                }
            };
            let mut item: wc_core::Part =
                store::read_json(&path).map_err(|err| format!("{}: {err}", path.display()))?;
            if item.id != part {
                return Err(format!("{} holds part {:?}, not {part:?}", path.display(), item.id));
            }
            let (w, h) = (footprint.width, footprint.height);
            let shapes = footprint.shapes.len();
            item.footprint = Some(footprint);
            store::write_if_changed(&path, &store::to_json(&item).map_err(e)?).map_err(e)?;
            println!(
                "wrote the footprint of {part} to {} ({} x {} mm, {shapes} shapes)",
                path.display(),
                wc_core::geom::format_length(w),
                wc_core::geom::format_length(h)
            );
            // The project's pinned copy follows, so its layouts draw the new footprint, but only
            // from the file just written: when another of the project's sources wins for this
            // id, that source's part is not the one that changed.
            if let Ok(p) = &mut project
                && p.lock.parts.contains_key(&part)
            {
                let library = wc_library::Library::open(dir, p).map_err(e)?;
                let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => false,
                };
                let winner = library.locate(ItemKind::Part, &part);
                // A copy changed in the project since it was pinned is not overwritten: those
                // edits must be removed or merged first.
                // A copy that is there but cannot be read is not overwritten either.
                let vendored = store::vendor_path(dir, ItemKind::Part, &part).map_err(e)?;
                let kept: Option<String> = if vendored.exists() {
                    match store::read_vendored_part(dir, &part) {
                        Ok(copy) if p.lock.parts.get(&part).is_some_and(|pin| pin.hash != copy.hash()) => {
                            Some(format!(
                                "it has local changes since it was pinned; remove or merge them, then take {part} again"
                            ))
                        }
                        Ok(_) => None,
                        Err(err) => Some(format!(
                            "it cannot be read ({err}); fix or remove it, then take {part} again"
                        )),
                    }
                } else {
                    None
                };
                // Read first, so a failed refresh puts the copy back as it was: the lock on disk
                // is only saved after a refresh that worked, and must keep matching the copy. A
                // copy whose bytes cannot be read is refused, never replaced or removed.
                let before: Result<Option<Vec<u8>>, String> = match std::fs::read(&vendored) {
                    Ok(bytes) => Ok(Some(bytes)),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(err) => Err(err.to_string()),
                };
                let refused = kept.or_else(|| {
                    before
                        .as_ref()
                        .err()
                        .map(|err| format!("it cannot be read ({err}); fix or remove it, then take {part} again"))
                });
                // Whether `take` ran, so only its own writes are undone.
                let mut tried = false;
                let refreshed = match (&refused, &winner) {
                    (Some(why), _) => Err(why.clone()),
                    (None, Some(loc)) if same(&loc.path, &path) => {
                        tried = true;
                        wc_library::take(dir, p, &library, ItemKind::Part, &part)
                            .map_err(|err| err.to_string())
                            .and_then(|_| {
                                let copy = store::read_vendored_part(dir, &part).map_err(|err| err.to_string())?;
                                if copy.footprint == item.footprint {
                                    Ok(())
                                } else {
                                    Err("the copy taken has another footprint".to_owned())
                                }
                            })
                    }
                    (None, Some(loc)) => Err(format!(
                        "the project takes {part} from source {:?} ({}), not from the file written",
                        loc.source,
                        loc.path.display()
                    )),
                    (None, None) => Err(format!("{part} is in none of the project's library sources")),
                };
                match refreshed {
                    Ok(()) => {
                        save(dir, p)?;
                        println!("updated the project's copy of {part}");
                    }
                    Err(why) => {
                        let restored = match &before {
                            _ if !tried => Ok(()),
                            Ok(Some(bytes)) if std::fs::read(&vendored).ok().as_ref() != Some(bytes) => {
                                std::fs::write(&vendored, bytes).map_err(|err| err.to_string())
                            }
                            Ok(None) if vendored.exists() => {
                                std::fs::remove_file(&vendored).map_err(|err| err.to_string())
                            }
                            _ => Ok(()),
                        };
                        eprintln!(
                            "warning: the project's copy of {part} was not updated: {}",
                            escape_field(&why)
                        );
                        if let Err(err) = restored {
                            return Err(format!(
                                "the project's copy of {part} could not be put back: {err}; run `lib status`"
                            ));
                        }
                    }
                }
            }
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
        ImportCmd::Wago { file } => {
            let done = import_wago(dir, &file)?;
            for w in &done.warnings {
                eprintln!("warning: {}", escape_field(w));
            }
            println!(
                "WAGO configurator: {} device(s) or jumper(s) changed, {} row(s) already up to date, {} not matched",
                done.changed,
                done.unchanged,
                done.unmatched.len()
            );
            for u in &done.unmatched {
                println!("not matched: {u}");
            }
        }
        ImportCmd::PlcCsv { file } => {
            let mut p = load(dir)?;
            let bytes = std::fs::read(&file).map_err(e)?;
            let (rows, warnings) = wc_import::csv::file_rows(&bytes)
                .and_then(|rows| wc_import::csv::plc_addresses_from_rows(&rows))
                .map_err(e)?;
            for w in warnings {
                eprintln!("warning: {}", escape_field(&w));
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
                eprintln!("warning: {}", escape_field(&w));
            }
            println!("imported {n} part(s) into {}", lib.display());
        }
        ImportCmd::PartsEdz { file, lib, symbol_map } => {
            let (n, report, warnings) = import_edz(&file, &lib, symbol_map.as_deref())?;
            for w in warnings {
                eprintln!("warning: {}", escape_field(&w));
            }
            for line in report.lines() {
                println!("{line}");
            }
            println!("imported {n} part(s) into {}", lib.display());
        }
        ImportCmd::PartsCsv { file, lib } => {
            let bytes = std::fs::read(&file).map_err(e)?;
            let (parts, warnings) = wc_import::csv::file_rows(&bytes)
                .and_then(|rows| wc_import::csv::parts_from_rows(&rows))
                .map_err(e)?;
            for p in &parts {
                wc_library::write_part(&lib, p).map_err(e)?;
            }
            for w in warnings {
                eprintln!("warning: {}", escape_field(&w));
            }
            println!("imported {} part(s) into {}", parts.len(), lib.display());
        }
        ImportCmd::Pdf { file, first_page } => {
            let source = file
                .file_name()
                .map(|n| escape_field(&n.to_string_lossy()))
                .unwrap_or_default();
            import_pdf(dir, &file, first_page, &source)?;
        }
        ImportCmd::Eplan { path, first_page } => {
            let mut p = load(dir)?;
            let tables = eplan_tables(&path)?;
            let ep = wc_import::eplan::read(&tables).map_err(e)?;
            let (built, warnings) = wc_import::eplan::build(&ep, first_page);
            for w in ep.warnings.iter().chain(&warnings) {
                eprintln!("warning: {}", escape_field(w));
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
            add_and_save(dir, &mut p, built, &source, Replace::No)?;
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
                (bytes, None, None)
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
                    eprintln!("warning: {}", escape_field(w));
                }
                (got.bytes, got.info, Some(got.folder))
            };
            let (db, info, folder) = db;
            let conn = wc_import::swe::open(&db).map_err(e)?;
            let mut sw = wc_import::swe::read(&conn).map_err(e)?;
            sw.info = info;
            let choice = swe_symbol_choice(dir, &p, symbol_map.as_deref())?;
            for w in &choice.2 {
                eprintln!("warning: {}", escape_field(w));
            }
            // The archive's catalog: devices that name one of its parts are linked to it, the part
            // comes along, and its size is used for layout footprints the source gives no box.
            let catalog_read = !is_db;
            if catalog_read {
                let file = std::io::BufReader::new(std::fs::File::open(&path).map_err(e)?);
                match wc_import::swe::library::catalog_from_archive(file) {
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
            // The symbol and page drawings (DWG): symbols without one stay boxes.
            if let Some(folder) = &folder {
                let file = std::io::BufReader::new(std::fs::File::open(&path).map_err(e)?);
                match wc_import::swe::drawings_from_archive(file, folder) {
                    Ok(drawings) => {
                        for w in &drawings.warnings {
                            eprintln!("warning: drawings: {w}");
                        }
                        if !drawings.skipped.is_empty() {
                            let kinds: Vec<String> = drawings.skipped.iter().map(|(k, n)| format!("{n} {k}")).collect();
                            eprintln!("warning: drawings: not drawn: {}", kinds.join(", "));
                        }
                        sw.drawings = drawings;
                    }
                    Err(x) => eprintln!("warning: symbols are drawn as boxes: {x}"),
                }
            }
            let (built, warnings, report) = wc_import::swe::build_with(&sw, first_page, &choice.0);
            for w in sw.warnings.iter().chain(&warnings) {
                eprintln!("warning: {}", escape_field(w));
            }
            let linked = (catalog_read && !sw.catalog.is_empty())
                .then(|| built.devices.values().filter(|d| d.part.is_some()).count());
            let source = format!(
                "import:{}",
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            let (n_sym, joined) = add_swe_built(dir, &mut p, built, &source, &choice.1)?;
            println!(
                "{:<18} {:>8} {:>9}  why the rest is not imported",
                "", "source", "imported"
            );
            for r in &report {
                println!("{:<18} {:>8} {:>9}  {}", r.what, r.source, r.imported, r.note);
            }
            if joined > 0 {
                println!("{joined} imported jumper group(s) joined a jumper the project already had");
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
                eprintln!("warning: {}", escape_field(&w));
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
                eprintln!("warning: {}", escape_field(w));
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
            add_and_save(dir, &mut p, built, &source, Replace::No)?;
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
                &[
                    Kind::Pdf,
                    Kind::Dxf,
                    Kind::AutomationMl,
                    Kind::KicadSymbols,
                    Kind::Csv,
                    Kind::Spreadsheet,
                ],
            )
            .map_err(e)?;
            if entries.is_empty() {
                return Err(
                    "no importable PDF, DXF, AutomationML, KiCad symbols, CSV lists or Excel lists inside; run `wiringcraft inspect` to see what is there"
                        .into(),
                );
            }
            // Symbol libraries go into a library folder, not the project's pages.
            let (symbols, entries): (Vec<_>, Vec<_>) = entries
                .into_iter()
                .partition(|(_, kind, _)| *kind == Kind::KicadSymbols);
            // Entry names are printed: written so a name cannot send the terminal control
            // sequences. A library's prefix and a page's title are made from the name as it is.
            let entries: Vec<_> = entries
                .into_iter()
                .map(|(raw, kind, data)| ((escape_field(&raw), raw), kind, data))
                .collect();
            // CSV and Excel lists come last, so PLC addresses reach devices that pages in the same
            // archive add.
            let (csvs, entries): (Vec<_>, Vec<_>) = entries
                .into_iter()
                .partition(|(_, kind, _)| matches!(kind, Kind::Csv | Kind::Spreadsheet));
            // Two entries with the same file name would write the same ids over each other.
            let mut prefixes = std::collections::BTreeSet::new();
            for (raw, _, data) in symbols {
                let name = escape_field(&raw);
                println!("{name}:");
                let Some(lib) = &lib else {
                    eprintln!("warning: {name}: KiCad symbols, not imported: pass --lib <folder>");
                    continue;
                };
                let prefix = kicad_prefix(&raw);
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
            let staging = staging_dir("wiringcraft-archive-").map_err(e)?;
            let tmp = staging.path();
            let mut next = first_page;
            for (i, ((name, raw), kind, data)) in entries.into_iter().enumerate() {
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
                        let r = import_pdf(dir, &path, next, &name);
                        next += n.max(1);
                        r
                    }
                    Kind::Dxf => {
                        let mut p = load(dir)?;
                        let id = format!("dxf-{next}");
                        next += 1;
                        // Page files are named by id, and some folders do not tell `DXF-300`
                        // from `dxf-300`.
                        if p.pages.iter().any(|pg| pg.id.eq_ignore_ascii_case(&id)) {
                            // Never added over an earlier page: removing it again on failure
                            // would delete that page.
                            Err(format!(
                                "page {id} exists; remove it or import with --first-page to number the pages from a free number"
                            ))
                        } else {
                            p.pages.push(Page::new(id.clone(), (next - 1).to_string(), raw.clone()));
                            p.sort_pages();
                            // A save that fails part way is cleaned up like a failed import.
                            let r = save(dir, &p).and_then(|()| {
                                import_cmd(
                                    dir,
                                    ImportCmd::PageDxf {
                                        file: path.clone(),
                                        page: id.clone(),
                                        scale: 1.0,
                                    },
                                )
                            });
                            // A drawing that could not be imported leaves no blank page behind.
                            // When that fails too, the entry's own error still counts and the
                            // other entries still come in.
                            if r.is_err() {
                                let remove = || -> Res<()> {
                                    let mut p = load(dir)?;
                                    p.pages.retain(|pg| pg.id != id);
                                    save(dir, &p)
                                };
                                if let Err(why) = remove() {
                                    // The save may fail after the page file is gone already.
                                    let left = load(dir).map_or(true, |p| p.pages.iter().any(|pg| pg.id == id));
                                    if left {
                                        eprintln!("warning: {name}: the empty page {id} could not be removed: {why}");
                                    } else {
                                        eprintln!(
                                            "warning: {name}: the empty page {id} was removed, but the project could not be saved in full: {why}"
                                        );
                                    }
                                }
                            }
                            r
                        }
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
            for (i, ((name, _), _, data)) in csvs.into_iter().enumerate() {
                println!("{name}:");
                // Told apart by header, the same column names `plc-csv` and `parts-csv` read.
                let rows = match wc_import::csv::file_rows(&data) {
                    Ok(rows) => rows,
                    Err(err) => {
                        eprintln!("warning: {name}: {err}");
                        continue;
                    }
                };
                let ext = if wc_import::xlsx::is_xlsx(&data) { "xlsx" } else { "csv" };
                let path = tmp.join(format!("list-{i}.{ext}"));
                let cmd = match wc_import::csv::kind_of_rows(&rows) {
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
            import_page_dwg(dir, &file, page, scale, std::env::var("WIRINGCRAFT_DWG2DXF").ok())?;
        }
        ImportCmd::PageDxf { file, page, scale } => {
            let mut p = load(dir)?;
            let lib = wc_library::Library::open(dir, &p).map_err(e)?;
            let known: std::collections::BTreeSet<String> = lib.index(ItemKind::Symbol).into_keys().collect();
            let text = std::fs::read(&file).map_err(e)?;
            let imported = wc_import::dxf::page(&String::from_utf8_lossy(&text), scale, &known).map_err(e)?;
            for w in &imported.warnings {
                eprintln!("warning: {}", escape_field(w));
            }
            // The page must be there before anything is taken (a retry would skip the copies).
            page_mut(&mut p, &page)?;
            let needed: std::collections::BTreeSet<&str> = imported
                .symbols
                .iter()
                .map(|s| s.symbol.as_str())
                .filter(|id| store::read_vendored_symbol(dir, id).is_err())
                .collect();
            // Check every symbol before taking any, so a refused import leaves no copy behind
            // without its pin (a retry would take that copy for a pinned one).
            for id in &needed {
                let (s, source) = lib.symbol(id).map_err(e)?;
                if s.id != *id {
                    return Err(format!("{source}: the symbol file for {id:?} declares id {:?}", s.id));
                }
            }
            // What is in the vendor folder now, so a write that fails part way leaves no copy
            // without its pin (a retry would skip a readable one).
            let before: Vec<(PathBuf, Option<String>)> = needed
                .iter()
                .map(|id| {
                    let path = store::vendor_path(dir, ItemKind::Symbol, id).map_err(e)?;
                    let was = match std::fs::read_to_string(&path) {
                        Ok(text) => Some(text),
                        Err(x) if x.kind() == std::io::ErrorKind::NotFound => None,
                        Err(x) => return Err(format!("{}: {x}", path.display())),
                    };
                    Ok((path, was))
                })
                .collect::<Res<_>>()?;
            let put_back = |why: String| -> String {
                let failed: Vec<String> = before
                    .iter()
                    .filter_map(|(path, was)| {
                        // Written only when it differs, and whole or not at all.
                        let r = match was {
                            Some(text) => store::write_if_changed(path, text)
                                .map(|_| ())
                                .map_err(|x| x.to_string()),
                            None => std::fs::remove_file(path).or_else(|x| {
                                if x.kind() == std::io::ErrorKind::NotFound {
                                    Ok(())
                                } else {
                                    Err(format!("{}: {x}", path.display()))
                                }
                            }),
                        };
                        r.err()
                    })
                    .collect();
                if failed.is_empty() {
                    why
                } else {
                    format!("{why}; symbol copies could not be put back: {}", failed.join("; "))
                }
            };
            for id in needed {
                wc_library::take(dir, &mut p, &lib, ItemKind::Symbol, id).map_err(|x| put_back(x.to_string()))?;
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

/// Warns about the project's report layouts that name a report or column it does not have,
/// about PLC racks with cards that cannot be laid out or points on no card, and about
/// fieldbus networks with unknown devices, shared addresses or ports used twice.
fn warn_report_layouts(inputs: &wc_report::Inputs<'_>) {
    for w in wc_report::layout_warnings(inputs) {
        // The layout comes from the project file: written so it cannot send control sequences.
        eprintln!("warning: {}", escape_field(&w));
    }
    for w in wc_report::plc_rack_warnings(inputs) {
        // Rack names and tags come from the project too.
        eprintln!("warning: {}", escape_field(&w));
    }
    for w in wc_report::network_warnings(inputs) {
        eprintln!("warning: {}", escape_field(&w));
    }
}

/// `import pdf`: the pages of `file` as trace pages numbered from `first_page`. `source` names
/// the PDF in warnings (the file name, or the entry name inside an archive, where `file` is a
/// staging file), already escaped with [`escape_field`] by the caller.
fn import_pdf(dir: &Path, file: &Path, first_page: usize, source: &str) -> Result<(), String> {
    let mut p = load(dir)?;
    let bytes = std::fs::read(file).map_err(e)?;
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
            // The reason can quote the file: written so it cannot send control sequences.
            let why = escape_field(t.scan_problem.as_deref().unwrap_or("no picture covers the page"));
            eprintln!(
                "warning: {} page {}: no vector content and no usable scan ({why})",
                source,
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
                if let Some(warning) = &t.scan_warning {
                    // The reason can quote the file: written so it cannot send control sequences.
                    eprintln!(
                        "warning: {} page {}: the scanned sheet was read only in part ({})",
                        source,
                        i + 1,
                        escape_field(warning)
                    );
                }
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
        let (size, why) = sheet_for(t.size_mm);
        if let Some(why) = why {
            eprintln!("warning: page {} ({} page {}): {why}", first_page + i, source, i + 1);
        }
        page.size = size;
        println!("page {}: {} shapes", page.number, t.graphics.len());
        page.graphics = t.graphics;
        p.pages.push(page);
    }
    p.sort_pages();
    save(dir, &p)?;
    Ok(())
}

/// The sheet for a traced drawing of `(width, height)` millimetres: the smallest ISO size
/// that holds both sides, upright when the drawing is. A drawing larger than A0 gets a sheet
/// of its own size, each side at least [`PageSize::MIN_MM`]; one beyond the custom range gets
/// A0, with the reason as a warning.
fn sheet_for((w, h): (f64, f64)) -> (PageSize, Option<String>) {
    // PDF sizes are in whole points, so an A3 page reads a fraction of a millimetre off; one
    // point of slack covers that rounding and no more, so no drawing is cut by the sheet.
    const SLACK: f64 = 25.4 / 72.0;
    let portrait = h > w;
    if let Some(named) = PageSize::NAMED
        .into_iter()
        .filter(|s| s.is_portrait() == portrait)
        .find(|s| {
            let (sw, sh) = s.mm();
            w <= sw + SLACK && h <= sh + SLACK
        })
    {
        return (named, None);
    }
    let side = |mm: f64| (mm.ceil() as u32).max(PageSize::MIN_MM);
    match PageSize::custom(side(w), side(h)) {
        Ok(size) => (size, None),
        Err(why) => {
            let a0 = if portrait { PageSize::A0Portrait } else { PageSize::A0 };
            (a0, Some(format!("{why}, so it is drawn on {}", a0.label())))
        }
    }
}

/// What `lib lock` could not pin, one line each.
fn lock_warnings(left: &wc_library::Unpinned) -> Vec<String> {
    let kind = |k: &ItemKind| if *k == ItemKind::Symbol { "symbol" } else { "part" };
    left.missing
        .iter()
        .map(|(k, id)| format!("{} {id} is in no library source", kind(k)))
        .chain(
            left.wrong_id
                .iter()
                .map(|(k, id, why)| format!("{} {id} is not pinned: {why}", kind(k))),
        )
        .collect()
}

/// `import page-dwg`: convert `file` with the converter (`template`, see [`dwg_to_dxf`]) and
/// import the DXF onto `page`. The converter writes into a private folder of its own, removed
/// with everything in it when this ends, also on failure.
fn import_page_dwg(dir: &Path, file: &Path, page: String, scale: f64, template: Option<String>) -> Res<()> {
    let tmp = staging_dir("wiringcraft-dwg-").map_err(e)?;
    let dxf = dwg_to_dxf(file, tmp.path(), template)?;
    import_cmd(dir, ImportCmd::PageDxf { file: dxf, page, scale })
}

/// Convert DWG to a DXF in `tmp` (a private folder, see [`staging_dir`]) with whatever
/// converter is installed: the command `template` (the WIRINGCRAFT_DWG2DXF setting, with `{in}`
/// and `{out}`) when given, otherwise `dwg2dxf` or the ODA File Converter. DWG is a closed
/// format; WiringCraft does not read it itself.
fn dwg_to_dxf(file: &Path, tmp: &Path, template: Option<String>) -> Res<PathBuf> {
    use std::process::Command;
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "drawing".into());
    let out = tmp.join(format!("{stem}.dxf"));
    let input = file.to_string_lossy().into_owned();
    let output = out.to_string_lossy().into_owned();
    let ran = |mut c: Command| c.output().map(|o| o.status.success()).unwrap_or(false);
    if let Some(template) = template {
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
                    write_out(&path, wc_vcs::subway_svg(&rows)).map_err(e)?;
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

/// Stack for the thread the command is parsed and run on. Argument parsing and the command
/// dispatch keep large frames in debug builds, and Windows gives the main thread only 1 MiB,
/// which overflowed there once one more import command was added (#312).
const STACK: usize = 16 * 1024 * 1024;

fn main() -> ExitCode {
    // Parsing runs there too: clap's derived parser builds every subcommand on the stack.
    let worker = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(|| run(Cli::parse()));
    let result = match worker {
        Ok(handle) => handle
            .join()
            .unwrap_or_else(|_| Err("the command stopped unexpectedly".into())),
        Err(err) => Err(format!("could not start the command: {err}")),
    };
    match result {
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
                "unknown report {n:?} (contents, parts, devices, wires, nets, terminals, cables, plc, plc-cards, locations, networks, all)"
            )
        })?;
        out.push(k);
    }
    Ok(out)
}

/// The project's vendored parts by id (parts used by devices, their accessories and jumpers).
/// What [`export_wago_marking`] wrote.
struct Exported {
    markers: usize,
    warnings: Vec<String>,
}

/// Write the WAGO smartScript marker list of the project in `dir` to `out`. Unknown kinds, lines,
/// languages and strips are refused before anything is written. Symbols without a vendored copy
/// are warned about: their terminals are recognised by their tag (`-X1.3`) but their wiring is
/// not known.
fn export_wago_marking(dir: &Path, out: &Path, args: &MarkingArgs) -> Res<Exported> {
    let kinds = wc_report::parse_kinds(&args.kinds)?;
    let lines = wc_report::parse_lines(&args.lines)?;
    let language = wc_report::Language::parse(&args.lang)
        .ok_or_else(|| format!("unknown language {:?} (en, de, nb)", args.lang))?;
    let p = load(dir)?;
    let (symbols, missing) = store::vendored_symbols(dir, &p);
    let mut warnings: Vec<String> = missing
        .iter()
        .map(|m| {
            format!("symbol {m} has no vendored copy (run `wiringcraft lib lock`); its terminals are listed by their tags, without their wiring")
        })
        .collect();
    let parts = vendored_parts(dir, &p);
    let inputs = wc_report::Inputs {
        project: &p,
        symbols: &symbols,
        parts: &parts,
    };
    let strip = args.strip.as_deref().map(str::trim).map(str::to_owned);
    if let Some(s) = &strip {
        let strips = wc_report::terminal_strips(&inputs);
        if !strips.contains(s) {
            return Err(if strips.is_empty() {
                format!("no terminal strip {s:?}: the project has no numbered terminals")
            } else {
                format!("no terminal strip {s:?} (strips: {})", strips.join(", "))
            });
        }
    }
    let options = wc_report::MarkingOptions {
        kinds,
        lines,
        language,
        strip,
        level_separator: args
            .level_separator
            .clone()
            .unwrap_or_else(|| wc_report::LEVEL_SEPARATOR.to_owned()),
    };
    let t = wc_report::marking_list_with(&inputs, &options);
    write_out(out, wc_report::to_smartscript_csv(&t)).map_err(e)?;
    warnings.extend(wc_report::formula_warnings(&t));
    warnings.extend(wc_report::level_warnings(inputs.project));
    Ok(Exported {
        markers: t.rows.len(),
        warnings,
    })
}

/// Write the project's terminal strips in `dir` for the WAGO configurator to `out`. Symbols
/// without a vendored copy are warned about: their terminals are recognised by their tag
/// (`-X1.3`) but their wiring is not known. `markers` in the result counts the rows.
fn export_wago_configurator(dir: &Path, out: &Path) -> Res<Exported> {
    let p = load(dir)?;
    let (symbols, missing) = store::vendored_symbols(dir, &p);
    let mut warnings: Vec<String> = missing
        .iter()
        .map(|m| {
            format!("symbol {m} has no vendored copy (run `wiringcraft lib lock`); its terminals are listed by their tags, without their wiring")
        })
        .collect();
    let mut parts = vendored_parts(dir, &p);
    // Every used part (devices, their accessories and jumpers) that could not be read is
    // written by its id, and the warning says why.
    for id in p.used_parts() {
        if parts.contains_key(&id) {
            continue;
        }
        match store::read_vendored_part(dir, &id) {
            Ok(part) => {
                parts.insert(id, part);
            }
            Err(err) => warnings.push(format!(
                "part {id} could not be read ({err}); its article number is written as the part id"
            )),
        }
    }
    let inputs = wc_report::Inputs {
        project: &p,
        symbols: &symbols,
        parts: &parts,
    };
    let wc_report::Configurator {
        table: t,
        warnings: more,
        ..
    } = wc_report::configurator_list(&inputs);
    warnings.extend(more);
    if t.rows.is_empty() {
        warnings.push("the project has no numbered terminals on its schematic pages".into());
    }
    write_out(out, wc_report::to_smartscript_csv(&t)).map_err(e)?;
    warnings.extend(wc_report::level_warnings(inputs.project));
    Ok(Exported {
        markers: t.rows.len(),
        warnings,
    })
}

/// What [`import_wago`] did.
struct WagoImported {
    changed: usize,
    unchanged: usize,
    unmatched: Vec<String>,
    warnings: Vec<String>,
}

/// Write the articles of a configured WAGO configurator list onto the project in `dir` (see
/// [`wc_import::wago`]). Articles are looked up among the project's own parts first, then its
/// libraries; a library part that is used is taken into the project. A part the project uses
/// without a pin, named by a row or not, is pinned too: the lock is written for that, and a
/// missing copy is written from the library when one has the part; the assignment files are
/// left alone. Nothing else is saved when no row changes.
fn import_wago(dir: &Path, file: &Path) -> Res<WagoImported> {
    let bytes = std::fs::read(file).map_err(|x| format!("{}: {x}", file.display()))?;
    let (rows, mut warnings) = wc_import::wago::read_configured(&bytes).map_err(e)?;
    let mut p = load(dir)?;
    let (symbols, missing) = store::vendored_symbols(dir, &p);
    warnings.extend(missing.iter().map(|m| {
        format!("symbol {m} has no vendored copy (run `wiringcraft lib lock`); its terminals cannot be matched")
    }));
    // The project's own parts, then the libraries'.
    let (candidates, more) = wc_import::wago::Candidates::load(dir, &p);
    warnings.extend(more);
    let plan = {
        let inputs = wc_report::Inputs {
            project: &p,
            symbols: &symbols,
            parts: &candidates.all(),
        };
        wc_import::wago::plan(&inputs, &candidates, &rows)
    };
    let mut unmatched = plan.unmatched.clone();
    let mut taken = plan.clone();
    for id in plan.parts() {
        // Pinned as it is when the project has a copy; else taken from the library it was read
        // from: another library that cannot be opened does not stop it.
        if let Err(why) = candidates.take(dir, &mut p, id) {
            // Its rows are listed, not written with a part the project cannot hold.
            for a in plan.assignments.iter().filter(|a| a.part == id) {
                unmatched.push(format!(
                    "line {}: part {id} cannot be taken into the project: {why}",
                    a.line
                ));
            }
            taken.assignments.retain(|a| a.part != id);
        }
    }
    let changed = wc_import::wago::apply(&mut p, &taken);
    // A part a device or jumper already uses without a pin (left by an import whose lock write
    // failed before the lock went first) is pinned now: its copy as it is, or the library's
    // when the project has none. One no library has and the project has no copy of is left to
    // `lib lock` to report.
    let mut repaired = 0;
    for id in p.used_parts() {
        let known = candidates.project.contains_key(&id)
            || candidates.unreadable.contains_key(&id)
            || candidates.library.contains_key(&id);
        if p.lock.parts.contains_key(&id) || !known {
            continue;
        }
        match candidates.take(dir, &mut p, &id) {
            Ok(()) => repaired += 1,
            Err(why) => warnings.push(format!("part {id} is used without a pin and cannot be pinned: {why}")),
        }
    }
    // The pins first: if a later write fails, a repeat import still finds the part unassigned
    // and saves again, instead of leaving it assigned without a pin. Only pins changed: the
    // other files are left alone.
    if changed > 0 || repaired > 0 {
        store::save_lock(dir, &p).map_err(e)?;
    }
    if changed > 0 {
        save(dir, &p)?;
    }
    Ok(WagoImported {
        changed,
        unchanged: plan.unchanged,
        unmatched,
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
fn import_edz(file: &Path, lib: &Path, symbol_map: Option<&Path>) -> Res<(usize, wc_import::edz::Report, Vec<String>)> {
    // The map is read first, so a bad one stops the import before anything is written.
    let map = match symbol_map {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|x| format!("{}: {x}", path.display()))?;
            Some(wc_import::swe::parse_symbol_map(&text).map_err(|x| format!("{}: {x}", path.display()))?)
        }
        None => None,
    };
    let bytes = std::fs::read(file).map_err(|x| format!("{}: {x}", file.display()))?;
    let wc_import::edz::Edz {
        mut parts,
        symbols,
        mut report,
        mut warnings,
    } = wc_import::edz::read_all(&bytes).map_err(|x| format!("{}: {x}", file.display()))?;
    // The parts whose symbol the map chose.
    let mut mapped: std::collections::BTreeSet<String> = Default::default();
    if let Some(map) = &map {
        let mut probe: Vec<wc_core::Part> = parts
            .iter()
            .map(|p| wc_core::Part {
                symbol: None,
                ..p.clone()
            })
            .collect();
        wc_import::edz::apply_symbol_map(&mut probe, map, &mut report.clone());
        mapped = probe.into_iter().filter(|p| p.symbol.is_some()).map(|p| p.id).collect();
        wc_import::edz::apply_symbol_map(&mut parts, map, &mut report);
    }
    // Symbols first, so no part is written pointing at a symbol the library lacks. A symbol that
    // cannot be written is skipped, and its parts are written without it.
    let mut unwritten: std::collections::BTreeSet<String> = Default::default();
    for s in &symbols {
        if let Err(x) = wc_library::write_symbol(lib, s) {
            warnings.push(format!("symbol {} not written: {x}", s.id));
            unwritten.insert(s.id.clone());
        }
    }
    if !unwritten.is_empty() {
        // A link the symbol map chose is kept while the library still holds a readable symbol of
        // that id; a link to the macro's own symbol, which was not saved, is dropped.
        let readable = |id: &str| {
            std::fs::read_to_string(lib.join("symbols").join(format!("{id}.json")))
                .ok()
                .and_then(|t| serde_json::from_str::<wc_core::Symbol>(&t).ok())
                .is_some()
        };
        let keep = |part: &str, id: &str| !unwritten.contains(id) || (mapped.contains(part) && readable(id));
        for p in &mut parts {
            if p.symbol.as_deref().is_some_and(|s| !keep(&p.id, s)) {
                p.symbol = None;
            }
        }
        for m in &mut report.part_macros {
            if m.symbol.as_deref().is_some_and(|s| !keep(&m.part, s)) {
                m.symbol = None;
            }
        }
    }
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
            add_and_save(dir, &mut p, built, source, Replace::Unchanged)?;
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
        eprintln!("warning: {}", escape_field(&w));
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
    p.used_parts()
        .into_iter()
        .filter_map(|id| store::read_vendored_part(dir, &id).ok().map(|part| (id, part)))
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

/// Adds a SolidWorks Electrical import to `p` and saves it ([`add_and_save`]), keeping the
/// library symbols it used (named in `sources`, new to the project or pinned before) pinned to
/// their libraries. Returns how many symbol shapes the import made itself, and how many imported
/// jumper groups joined the project's own.
fn add_swe_built(
    dir: &Path,
    p: &mut Project,
    built: wc_import::aml::Built,
    source: &str,
    sources: &std::collections::BTreeMap<String, String>,
) -> Res<(usize, usize)> {
    let n_sym = built.symbols.iter().filter(|s| !sources.contains_key(&s.id)).count();
    let from_library: Vec<String> = built
        .symbols
        .iter()
        .filter(|s| sources.contains_key(&s.id))
        .map(|s| s.id.clone())
        .collect();
    // Not replacing: a generated symbol may still be placed on pages the project keeps. Saved
    // like every import: a failed save puts every file back, jumpers and pages together.
    let joined = add_and_save_with(dir, p, built, source, Replace::No, |p| {
        keep_library_sources(p, &from_library, sources);
    })?;
    Ok((n_sym, joined))
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

/// A page's title-block fields, one per line: key, the value the page shows, and whether it is
/// the page's own or the project's.
fn page_fields(p: &Project, page: usize) -> String {
    let Some(pg) = p.pages.get(page) else {
        return String::new();
    };
    let mut keys = pg.title_field_keys();
    for k in p.meta.title_block.keys() {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    let mut out = String::new();
    for k in keys {
        let value = pg.title_field(&p.meta, &k);
        let from = if pg.title_block.get(&k).is_some_and(|v| !v.trim().is_empty()) {
            "page"
        } else if value.is_empty() {
            "-"
        } else {
            "project"
        };
        out.push_str(&format!(
            "{:<16} {:<24} ({from})\n",
            escape_field(&k),
            escape_field(value)
        ));
    }
    out
}

/// A project has one cover page. When the project already has one, imported cover pages are
/// dropped and their title-block values (the first non-blank one of each field) come back, to
/// fill the fields the existing cover leaves empty with [`fill_cover`]; otherwise only the first
/// imported cover is kept. The project itself is not changed, so a refused import leaves it as
/// it was.
fn keep_one_cover(p: &Project, pages: &mut Vec<Page>) -> std::collections::BTreeMap<String, String> {
    let mut kept = p.cover_page().is_some();
    let mut fields = std::collections::BTreeMap::new();
    pages.retain(|pg| {
        if !pg.cover {
            return true;
        }
        if kept {
            for (k, v) in pg.title_block.iter().filter(|(_, v)| !v.trim().is_empty()) {
                fields.entry(k.clone()).or_insert_with(|| v.clone());
            }
            return false;
        }
        kept = true;
        true
    });
    fields
}

/// Fill the project cover's empty title-block fields from `fields` ([`keep_one_cover`]).
fn fill_cover(p: &mut Project, fields: std::collections::BTreeMap<String, String>) {
    let Some(cover) = p.cover_page().and_then(|i| p.pages.get_mut(i)) else {
        return;
    };
    for (k, v) in fields {
        let slot = cover.title_block.entry(k).or_default();
        if slot.trim().is_empty() {
            *slot = v;
        }
    }
}

/// What [`add_built`] wrote: each vendored file with what it held before (none when it had no
/// such file), for [`put_files_back`] when the save after it fails.
type Written = Vec<(PathBuf, Option<Vec<u8>>)>;

/// Whether an import may replace a vendored item the project pins in another version.
#[derive(Clone, Copy, PartialEq)]
enum Replace {
    /// Never: an Eplan, AML or SolidWorks Electrical project import makes its own symbols, and a
    /// page the project kept may still use the old one's pins.
    No,
    /// When the project's copy is still the pinned one (a library import, #258).
    Unchanged,
}

/// [`add_built`], then save: when the save fails, the vendored files and every project file
/// the save may have written go back as they were, so the project on disk is the one before.
fn add_and_save(dir: &Path, p: &mut Project, built: wc_import::aml::Built, source: &str, replace: Replace) -> Res<()> {
    add_and_save_with(dir, p, built, source, replace, |_| {}).map(|_| ())
}

/// [`add_and_save`], with `finish` changing the project after the import and before the save,
/// and giving how many imported jumper groups joined the project's own ([`add_built`]).
fn add_and_save_with(
    dir: &Path,
    p: &mut Project,
    built: wc_import::aml::Built,
    source: &str,
    replace: Replace,
    finish: impl FnOnce(&mut Project),
) -> Res<usize> {
    let pages: Vec<&str> = built.pages.iter().map(|pg| pg.id.as_str()).collect();
    let panels: Vec<&str> = built.panels.iter().map(|n| n.id.as_str()).collect();
    let project_files = snapshot_project(dir, &pages, &panels)?;
    let (written, joined) = add_built(dir, p, built, source, replace)?;
    finish(p);
    if let Err(err) = save_imported(dir, p) {
        let mut failed = put_files_back(&project_files);
        failed.extend(put_files_back(&written));
        if failed.is_empty() {
            return Err(err);
        }
        return Err(format!(
            "{err}; putting back the vendored files failed too: {}",
            failed.join("; ")
        ));
    }
    Ok(joined)
}

/// Every file a save of the project may write or remove, with its bytes before (`None` when it
/// does not exist yet): the project's own files, every file in its page, panel and comment
/// folders, and the files of the pages and panels about to be added. [`put_files_back`] restores
/// them when a save fails half way.
fn snapshot_project(dir: &Path, new_pages: &[&str], new_panels: &[&str]) -> Res<Written> {
    // Every file the save may write or remove, read before anything is written: the project's
    // own files, every file in its page, panel and comment folders, and the files the new pages
    // and panels will get.
    let mut paths: Vec<PathBuf> = [store::PROJECT_FILE, store::DEVICES_FILE, store::LOCK_FILE]
        .iter()
        .map(|name| dir.join(name))
        .collect();
    // Only the names the store reads and writes: another file there is never changed by the save.
    for (folder, suffix) in [
        (store::PAGES_DIR, store::PAGE_SUFFIX),
        (store::PANELS_DIR, store::PANEL_SUFFIX),
        (store::COMMENTS_DIR, store::COMMENTS_SUFFIX),
    ] {
        let folder = dir.join(folder);
        // A folder that cannot be listed would leave files the save may change without a copy.
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => continue,
            Err(x) => return Err(format!("{}: {x}", folder.display())),
        };
        for entry in entries {
            let path = entry.map_err(|x| format!("{}: {x}", folder.display()))?.path();
            let ours = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix));
            if ours && path.is_file() {
                paths.push(path);
            }
        }
    }
    for id in new_pages {
        paths.push(dir.join(store::PAGES_DIR).join(format!("{id}{}", store::PAGE_SUFFIX)));
    }
    for id in new_panels {
        paths.push(dir.join(store::PANELS_DIR).join(format!("{id}{}", store::PANEL_SUFFIX)));
    }
    paths.sort();
    paths.dedup();
    let mut project_files: Written = Vec::new();
    for path in paths {
        let before = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => None,
            Err(x) => return Err(format!("{}: {x}", path.display())),
        };
        project_files.push((path, before));
    }
    Ok(project_files)
}

/// Wire ends in `p` on pins of the symbol `symbol.id` that `symbol` does not have:
/// `page 3 -K1:13` each.
fn wired_pins_lost(p: &Project, symbol: &wc_core::Symbol) -> Vec<String> {
    let mut out = Vec::new();
    for page in &p.pages {
        for w in &page.wires {
            for end in [&w.from, &w.to] {
                let wc_core::Endpoint::Pin { item, pin } = end else {
                    continue;
                };
                let Some(placed) = page.symbol(item) else { continue };
                if placed.symbol == symbol.id && symbol.pin(pin).is_none() {
                    let name = placed.tag.as_ref().map_or_else(|| item.clone(), ToString::to_string);
                    out.push(format!("page {} {name}:{pin}", page.number));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Put each file back as it was ([`put_file_back`]), the last written first. Returns why each
/// one that could not be put back failed.
fn put_files_back(files: &[(PathBuf, Option<Vec<u8>>)]) -> Vec<String> {
    files
        .iter()
        .rev()
        .filter_map(|(path, before)| put_file_back(path, before.as_deref()).err())
        .collect()
}

/// Add an import to `p` (not saved yet), giving the vendored files it wrote and how many
/// imported jumper groups joined a jumper the project already had (directly, or through another
/// imported group that did): the saved project holds those as part of that jumper, not as
/// jumpers of their own.
fn add_built(
    dir: &Path,
    p: &mut Project,
    mut built: wc_import::aml::Built,
    source: &str,
    replace: Replace,
) -> Result<(Written, usize), String> {
    let cover_fields = keep_one_cover(p, &mut built.pages);
    // Refuse before anything is written: a repeated import must not overwrite the symbol files of
    // an earlier one and then fail half way.
    if let Some(page) = built.pages.iter().find(|pg| p.page(&pg.id).is_some()) {
        return Err(format!(
            "page {} exists; remove it or import into a fresh project",
            page.id
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    if let Some(page) = built.pages.iter().find(|pg| !ids.insert(pg.id.as_str())) {
        return Err(format!("the import has page {} twice", page.id));
    }
    let numbers: std::collections::BTreeSet<&str> = p.pages.iter().map(|pg| pg.number.as_str()).collect();
    if let Some(page) = built.pages.iter().find(|pg| numbers.contains(pg.number.as_str())) {
        return Err(format!(
            "page number {} is taken; import with --first-page to number the pages from a free number",
            page.number
        ));
    }
    // A new version of a symbol must keep every pin a wire in the project ends on.
    for symbol in &built.symbols {
        if p.lock.symbols.get(&symbol.id).is_some_and(|e| e.hash != symbol.hash()) {
            let lost = wired_pins_lost(p, symbol);
            if !lost.is_empty() {
                return Err(format!(
                    "symbol {}: wires end on pins the new version no longer has ({}); move those wires first",
                    symbol.id,
                    lost.join(", ")
                ));
            }
        }
    }
    // A new version replaces the project's copy only when both are the same item of their
    // library: ids given by name can move to another block in another catalog (#301).
    for symbol in &built.symbols {
        if replace == Replace::No || !p.lock.symbols.get(&symbol.id).is_some_and(|e| e.hash != symbol.hash()) {
            continue;
        }
        if let Ok(have) = store::read_vendored_symbol(dir, &symbol.id)
            && let Some(why) = wc_core::symbol::replace_refused(&have, symbol)
        {
            return Err(format!("{why}; import into a fresh project"));
        }
    }
    // A library item already in the project may come again unchanged (same content hash), or
    // in a new version when the project's copy is still the one its lock pinned: nobody changed
    // it in the project, so the import replaces it (a block now drawn, a newer catalog). Every
    // file is read first, so a failed write can put them all back as they were.
    let mut files: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::new();
    let items = built
        .symbols
        .iter()
        .map(|s| (ItemKind::Symbol, s.id.as_str(), s.hash()))
        .chain(built.parts.iter().map(|x| (ItemKind::Part, x.id.as_str(), x.hash())));
    for (kind, id, hash) in items {
        let name = match kind {
            ItemKind::Symbol => format!("symbol {id}"),
            ItemKind::Part => format!("part {id}"),
        };
        let (path, before) = vendored_bytes(dir, kind, id)?;
        if let Some(entry) = p.lock.map(kind).get(id) {
            if replace == Replace::No && entry.hash != hash {
                return Err(format!("{name} exists with other content; import into a fresh project"));
            }
            let have = match (&before, kind) {
                (None, _) => None,
                (Some(_), ItemKind::Symbol) => Some(store::read_vendored_symbol(dir, id).map(|x| x.hash())),
                (Some(_), ItemKind::Part) => Some(store::read_vendored_part(dir, id).map(|x| x.hash())),
            };
            match have {
                Some(Ok(h)) if h == entry.hash => {}
                Some(Ok(_)) => {
                    return Err(format!("{name} exists with other content; import into a fresh project"));
                }
                Some(Err(why)) => {
                    return Err(format!(
                        "{name}: its copy {} cannot be read ({why}); repair it and import again",
                        path.display()
                    ));
                }
                None if entry.hash == hash => {}
                None => {
                    return Err(format!(
                        "{name}: the project pins another version and has no copy of it ({}); restore the pinned copy from the project's history, or import the pinned version itself",
                        path.display()
                    ));
                }
            }
        }
        files.push((path, before));
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
    // Jumpers join the project's own: one that shares a terminal with a jumper of the same
    // strip becomes one jumper with it. Checked here, before anything is written.
    let mut jumpers = p.meta.clone();
    for j in &built.jumpers {
        jumpers
            .add_jumper(&j.strip, &j.terminals)
            .map_err(|why| format!("jumper on {}: {why}", j.strip))?;
    }
    // The imported pages can renumber the project's terminals tagged with only their strip, so
    // the project's own jumpers follow first; the imported ones already have the final numbers.
    let before = if !p.meta.follows_terminals() {
        None
    } else {
        // A symbol the project has no readable copy of, which the import brings, changes which
        // placed items are terminals. Whether the jumpers' and rail order's numbers were given
        // with it or without it cannot be told, so a strip it renumbers is not guessed at.
        let (own, _) = store::vendored_symbols(dir, p);
        let mut symbols = own.clone();
        for s in &built.symbols {
            symbols.entry(s.id.clone()).or_insert_with(|| s.clone());
        }
        let with = wc_core::project::terminal_numbers(p, |id| symbols.get(id).map(|s| s.role));
        let without = wc_core::project::terminal_numbers(p, |id| own.get(id).map(|s| s.role));
        let strips: std::collections::BTreeSet<&DeviceTag> = with
            .iter()
            .filter(|(k, v)| without.get(*k) != Some(*v))
            .map(|(_, (strip, _))| strip)
            .chain(
                without
                    .iter()
                    .filter(|(k, v)| with.get(*k) != Some(*v))
                    .map(|(_, (strip, _))| strip),
            )
            .collect();
        if let Some(strip) = strips.into_iter().find(|s| {
            p.meta.jumpers.iter().any(|j| &j.strip == *s) || p.meta.strip_layout(s).is_some_and(|l| !l.is_empty())
        }) {
            let missing: Vec<&str> = built
                .symbols
                .iter()
                .filter(|s| !own.contains_key(&s.id))
                .map(|s| s.id.as_str())
                .filter(|id| p.pages.iter().any(|pg| pg.symbols.iter().any(|ps| ps.symbol == *id)))
                .collect();
            return Err(format!(
                "the jumpers and rail order of {strip} cannot be kept on their terminals: the project has no readable copy of {} that the import brings, so its terminal numbers are unclear; restore the file (for example from the project's history) and import again",
                missing.join(", ")
            ));
        }
        Some(with)
    };
    // A write that fails puts back the files written before it (and removes the ones it made),
    // so each copy still matches its pin. Only files this import changed are put back: one it
    // left as it was keeps whatever it holds now.
    let writes = built
        .symbols
        .iter()
        .map(|s| store::write_vendored_symbol(dir, s))
        .chain(built.parts.iter().map(|x| store::write_vendored_part(dir, x)));
    let mut changed: Written = Vec::new();
    for (file, written) in files.into_iter().zip(writes) {
        match written {
            Ok(true) => changed.push(file),
            Ok(false) => {}
            Err(err) => {
                let failed = put_files_back(&changed);
                if failed.is_empty() {
                    return Err(err.to_string());
                }
                return Err(format!(
                    "{err}; putting back the files written before it failed too: {}",
                    failed.join("; ")
                ));
            }
        }
    }
    fill_cover(p, cover_fields);
    for s in &built.symbols {
        p.lock.symbols.insert(
            s.id.clone(),
            wc_core::LockEntry {
                source: source.to_owned(),
                hash: s.hash(),
            },
        );
    }
    for part in &built.parts {
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
    if let Some(before) = before {
        let (symbols, _) = store::vendored_symbols(dir, p);
        wc_core::project::follow_terminals_in(p, &before, |id| symbols.get(id).map(|s| s.role));
    }
    // Terminals held by the project's own jumpers (with their numbers after following), and by
    // the imported groups that joined one of them.
    let mut joined_terminals: std::collections::BTreeSet<(DeviceTag, String)> = p
        .meta
        .jumpers
        .iter()
        .flat_map(|j| j.terminals.iter().map(|t| (j.strip.clone(), t.clone())))
        .collect();
    // Until nothing more joins, so a group that reaches the project's jumper only through one
    // listed after it counts too.
    let mut joined_groups = vec![false; built.jumpers.len()];
    loop {
        let mut more = false;
        for (j, done) in built.jumpers.iter().zip(joined_groups.iter_mut()) {
            if !*done
                && j.terminals
                    .iter()
                    .any(|t| joined_terminals.contains(&(j.strip.clone(), t.clone())))
            {
                *done = true;
                more = true;
                joined_terminals.extend(j.terminals.iter().map(|t| (j.strip.clone(), t.clone())));
            }
        }
        if !more {
            break;
        }
    }
    let joined = joined_groups.iter().filter(|j| **j).count();
    for j in &built.jumpers {
        p.meta
            .add_jumper(&j.strip, &j.terminals)
            .map_err(|why| format!("jumper on {}: {why}", j.strip))?;
    }
    p.panels.extend(built.panels);
    p.panels.sort_by(|a, b| a.id.cmp(&b.id));
    // Title-block fields fill the ones the project has not set; nothing typed in is overwritten.
    p.meta.fill_title_block(built.title_block);
    Ok((changed, joined))
}

/// The path and bytes of a vendored copy, kept before an import replaces it.
/// The vendored file of an item and its bytes, none when there is no such file.
fn vendored_bytes(dir: &Path, kind: ItemKind, id: &str) -> Result<(PathBuf, Option<Vec<u8>>), String> {
    let path = store::vendor_path(dir, kind, id).map_err(|x| x.to_string())?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok((path, Some(bytes))),
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => Ok((path, None)),
        Err(x) => Err(format!("{}: {x}", path.display())),
    }
}

/// Put `path` back to `before` (remove it when it had none). The bytes go to a file beside it
/// first and then replace it, so a failure never leaves it cut off. Returns why it failed.
fn put_file_back(path: &Path, before: Option<&[u8]>) -> Result<(), String> {
    let fail = |x: std::io::Error| format!("{}: {x}", path.display());
    let Some(bytes) = before else {
        return match std::fs::remove_file(path) {
            Err(x) if x.kind() != std::io::ErrorKind::NotFound => Err(fail(x)),
            _ => Ok(()),
        };
    };
    if std::fs::read(path).is_ok_and(|now| now == bytes) {
        return Ok(());
    }
    // The staging file is made new (never one already there, which could be a link to a file
    // elsewhere), and only that file is removed when something fails.
    let mut made = None;
    for n in 0..100u32 {
        let mut part = path.as_os_str().to_owned();
        part.push(format!(".restore-{}-{n}", std::process::id()));
        let part = PathBuf::from(part);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&part) {
            Ok(file) => {
                made = Some((part, file));
                break;
            }
            Err(x) if x.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(x) => return Err(fail(x)),
        }
    }
    let Some((part, mut file)) = made else {
        return Err(format!("{}: no free name for the staging file", path.display()));
    };
    use std::io::Write as _;
    let done = file.write_all(bytes).and_then(|()| file.sync_all()).and_then(|()| {
        drop(file);
        std::fs::rename(&part, path)
    });
    done.map_err(|x| match std::fs::remove_file(&part) {
        Ok(()) => fail(x),
        Err(y) => format!("{}; the staging file {} is left: {y}", fail(x), part.display()),
    })
}

/// The current time in UTC as `YYYY-MM-DDThh:mm:ssZ`.
fn utc_now() -> String {
    utc_date(now_secs())
}

/// Unix seconds in UTC as `YYYY-MM-DDThh:mm:ssZ`.
fn utc_date(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = wc_core::base::civil_date(days);
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

/// A fresh folder for files taken out of an archive or written by a converter, its name
/// starting with `prefix`: unique, and on Unix readable only by this user (mode 0700, set as it
/// is created), so nobody can plant a file or link there. Removed when the returned value is
/// dropped.
fn staging_dir(prefix: &str) -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
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
        let dir = staging_dir("wiringcraft-archive-").unwrap();
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn a_dwg_is_converted_inside_a_private_folder_that_goes_away() {
        let src = tempfile::tempdir().unwrap();
        let dwg = src.path().join("Motor plan.dwg");
        std::fs::write(&dwg, "0\nEOF\n").unwrap();
        let tmp = staging_dir("wiringcraft-dwg-").unwrap();
        let folder = tmp.path().to_owned();
        // A stand-in converter, as WIRINGCRAFT_DWG2DXF names one; a path with a space stays
        // one argument.
        let out = dwg_to_dxf(&dwg, &folder, Some("cp {in} {out}".into())).unwrap();
        assert_eq!(out, folder.join("Motor plan.dxf"));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "0\nEOF\n");
        let err = dwg_to_dxf(&dwg, &folder, Some("false {in} {out}".into()))
            .err()
            .unwrap_or_default();
        assert!(err.contains("did not produce a DXF"), "{err}");
        drop(tmp);
        assert!(!folder.exists(), "removed with everything in it");
    }

    #[cfg(unix)]
    #[test]
    fn import_page_dwg_leaves_no_folder_behind_when_it_fails() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        store::create(&dir, "Test").unwrap();
        let dwg = tmp.path().join("plan.dwg");
        std::fs::write(
            &dwg,
            "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0\n20\n0\n11\n10\n21\n10\n0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        // A stand-in converter that notes where it was told to write, then copies or fails. The
        // template is split on spaces, so it lives under /tmp, not a TMPDIR that may have them.
        let bin = tempfile::tempdir_in("/tmp").unwrap();
        let record = bin.path().join("out.txt");
        let converter = |name: &str, last: &str| {
            let path = bin.path().join(name);
            std::fs::write(
                &path,
                format!("#!/bin/sh\necho \"$2\" > '{}'\n{last}\n", record.display()),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            format!("{} {{in}} {{out}}", path.display())
        };
        let folder = || {
            PathBuf::from(std::fs::read_to_string(&record).unwrap().trim())
                .parent()
                .unwrap()
                .to_owned()
        };

        // The conversion fails.
        let err = import_page_dwg(&dir, &dwg, "1".into(), 1.0, Some(converter("fail.sh", "exit 1")));
        assert!(err.is_err());
        assert!(folder().starts_with(std::env::temp_dir()), "{:?}", folder());
        assert!(!folder().exists(), "removed after a failed conversion");

        // The conversion works, the DXF import fails (no such page).
        let copy = converter("copy.sh", "cp \"$1\" \"$2\"");
        let err = import_page_dwg(&dir, &dwg, "no-such-page".into(), 1.0, Some(copy.clone()));
        assert!(err.as_ref().is_err_and(|e| e.contains("no-such-page")), "{err:?}");
        assert!(!folder().exists(), "removed after a failed import");

        // And when it works.
        let pages = load(&dir).unwrap().pages;
        import_page_dwg(&dir, &dwg, pages[0].id.clone(), 1.0, Some(copy)).unwrap();
        assert!(!folder().exists(), "removed after the import");
    }

    fn cli(dir: &Path, args: &[&str]) -> Res<()> {
        let mut all = vec!["wiringcraft", "-C", dir.to_str().unwrap()];
        all.extend_from_slice(args);
        run(Cli::try_parse_from(all).map_err(|e| e.to_string())?)
    }

    #[test]
    fn a_maker_dxf_becomes_the_footprint_of_a_library_part_and_sizes_it_on_the_plate() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let lib = tmp.path().join("lib");
        let part = wc_core::Part {
            id: "maker.io-8".into(),
            manufacturer: "Maker".into(),
            order_number: "IO-8".into(),
            ..wc_core::Part::default()
        };
        wc_library::write_part(&lib, &part).unwrap();
        cli(&dir, &["lib", "add", "shop", "folder", lib.to_str().unwrap()]).unwrap();
        cli(&dir, &["device", "--part", "maker.io-8", "--", "+CAB1-A1"]).unwrap();
        cli(&dir, &["lib", "lock"]).unwrap();
        let before = load(&dir).unwrap().lock.parts["maker.io-8"].hash.clone();

        // A 22.5 x 100 mm outline drawn away from the origin.
        let dxf = tmp.path().join("io-8.dxf");
        std::fs::write(
            &dxf,
            "0\nSECTION\n2\nENTITIES\n0\nLWPOLYLINE\n8\n0\n90\n4\n70\n1\n\
             10\n200\n20\n50\n10\n222.5\n20\n50\n10\n222.5\n20\n150\n10\n200\n20\n150\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        let file = dxf.to_str().unwrap();
        assert!(cli(&dir, &["import", "footprint-dxf", file, "--part", "no.such"]).is_err());
        cli(&dir, &["import", "footprint-dxf", file, "--part", "maker.io-8"]).unwrap();
        let in_lib: wc_core::Part = store::read_json(&lib.join("parts").join("maker.io-8.json")).unwrap();
        let Some(f) = in_lib.footprint.as_ref() else {
            unreachable!("stored in the library with the part")
        };
        assert_eq!((f.width, f.height), (22.5, 100.0));
        // The project's pinned copy follows.
        let p = load(&dir).unwrap();
        assert_ne!(p.lock.parts["maker.io-8"].hash, before);
        assert_eq!(store::read_vendored_part(&dir, "maker.io-8").unwrap(), in_lib);

        // The part has no Width/Height: arranging the plate sizes the device from the drawing.
        cli(&dir, &["panel", "new", "main"]).unwrap();
        cli(&dir, &["panel", "arrange", "main"]).unwrap();
        let p = load(&dir).unwrap();
        let sizes: Vec<(f64, f64)> = p.panels[0]
            .items
            .iter()
            .filter_map(|i| match i {
                wc_core::panel::PanelItem::Device { width, height, .. } => Some((*width, *height)),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, [(22.5, 100.0)]);

        // A folder that is not the source the project takes the part from leaves the pinned copy
        // alone (the winning source's part did not change).
        let other_lib = tmp.path().join("lib2");
        wc_library::write_part(&other_lib, &part).unwrap();
        let pinned = load(&dir).unwrap().lock.parts["maker.io-8"].hash.clone();
        let other_arg = other_lib.to_str().unwrap();
        cli(
            &dir,
            &[
                "import",
                "footprint-dxf",
                file,
                "--part",
                "maker.io-8",
                "--lib",
                other_arg,
            ],
        )
        .unwrap();
        assert_eq!(load(&dir).unwrap().lock.parts["maker.io-8"].hash, pinned);

        // A pinned copy changed in the project is not overwritten by a new footprint.
        let mut edited = store::read_vendored_part(&dir, "maker.io-8").unwrap();
        edited.description = "changed in the project".into();
        store::write_vendored_part(&dir, &edited).unwrap();
        cli(&dir, &["import", "footprint-dxf", file, "--part", "maker.io-8"]).unwrap();
        assert_eq!(store::read_vendored_part(&dir, "maker.io-8").unwrap(), edited);
        assert_eq!(load(&dir).unwrap().lock.parts["maker.io-8"].hash, pinned);
        // Nor is a copy that cannot be read.
        let copy_path = store::vendor_path(&dir, ItemKind::Part, "maker.io-8").unwrap();
        std::fs::write(&copy_path, "{ broken").unwrap();
        cli(&dir, &["import", "footprint-dxf", file, "--part", "maker.io-8"]).unwrap();
        assert_eq!(std::fs::read_to_string(&copy_path).unwrap(), "{ broken");
        // Nor one whose bytes cannot be read at all (here a folder in its place).
        std::fs::remove_file(&copy_path).unwrap();
        std::fs::create_dir(&copy_path).unwrap();
        cli(&dir, &["import", "footprint-dxf", file, "--part", "maker.io-8"]).unwrap();
        assert!(copy_path.is_dir(), "left as it was");
        std::fs::remove_dir(&copy_path).unwrap();
        store::write_vendored_part(&dir, &edited).unwrap();
        // An unreadable project does not stop an import into a named library folder.
        let broken = tmp.path().join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(store::PROJECT_FILE), "not = [valid").unwrap();
        let lib_dir = lib.to_str().unwrap();
        cli(
            &broken,
            &[
                "import",
                "footprint-dxf",
                file,
                "--part",
                "maker.io-8",
                "--lib",
                lib_dir,
            ],
        )
        .unwrap();

        // A library folder can be named outside a project; a file holding another part is refused.
        let outside = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        let lib_arg = lib.to_str().unwrap();
        cli(
            &outside,
            &[
                "import",
                "footprint-dxf",
                file,
                "--part",
                "maker.io-8",
                "--lib",
                lib_arg,
            ],
        )
        .unwrap();
        let other = wc_core::Part {
            id: "maker.other".into(),
            ..wc_core::Part::default()
        };
        std::fs::write(
            lib.join("parts").join("maker.wrong.json"),
            store::to_json(&other).unwrap(),
        )
        .unwrap();
        let err = cli(
            &outside,
            &[
                "import",
                "footprint-dxf",
                file,
                "--part",
                "maker.wrong",
                "--lib",
                lib_arg,
            ],
        )
        .unwrap_err();
        assert!(err.contains("holds part"), "{err}");
    }

    #[test]
    fn new_starts_from_a_base_project_with_its_own_name_and_date() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        run(Cli::try_parse_from([
            "wiringcraft",
            "new",
            base.to_str().unwrap(),
            "--no-git",
            "--name",
            "Base",
        ])
        .unwrap())
        .unwrap();
        cli(&base, &["page", "add", "legend", "--number", "2", "--title", "Legend"]).unwrap();
        let mut p = store::load(&base).unwrap();
        p.meta.title_block.insert("customer".into(), "ACME".into());
        p.meta.title_block.insert("project_title".into(), "{name}".into());
        store::save(&base, &p).unwrap();

        let new = tmp.path().join("pump");
        run(Cli::try_parse_from([
            "wiringcraft",
            "new",
            new.to_str().unwrap(),
            "--name",
            "Pump station",
            "--template",
            base.to_str().unwrap(),
        ])
        .unwrap())
        .unwrap();
        let made = store::load(&new).unwrap();
        assert_eq!(made.meta.name, "Pump station");
        assert_eq!(made.meta.title_block["customer"], "ACME");
        assert_eq!(made.meta.title_block["project_title"], "Pump station");
        assert_eq!(made.meta.title_block["date"], wc_core::base::today());
        assert_eq!(made.pages.len(), 2);
        assert!(made.page("legend").is_some());
        assert!(new.join(".git").exists(), "the new project gets its own history");
    }

    #[test]
    fn pages_can_be_upright_large_or_of_their_own_size() {
        let sheet_for_size = |mm| sheet_for(mm).0;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        for (id, size, want) in [
            ("up", "A4-portrait", PageSize::A4Portrait),
            ("big", "A1", PageSize::A1),
            (
                "own",
                "500x700",
                PageSize::Custom {
                    width: 500,
                    height: 700,
                },
            ),
        ] {
            cli(&dir, &["page", "add", id, "--number", id, "--size", size]).unwrap();
            assert_eq!(load(&dir).unwrap().page(id).map(|p| p.size), Some(want), "{size}");
        }
        cli(&dir, &["page", "size", "big", "A0-portrait"]).unwrap();
        assert_eq!(
            load(&dir).unwrap().page("big").map(|p| p.size),
            Some(PageSize::A0Portrait)
        );
        let err = cli(&dir, &["page", "add", "bad", "--number", "bad", "--size", "A5"]).unwrap_err();
        assert!(err.contains("portrait"), "{err}");
        assert_eq!(sheet_for_size((210.0, 297.0)), PageSize::A4Portrait);
        assert_eq!(sheet_for_size((841.0, 594.0)), PageSize::A1);
        assert_eq!(sheet_for_size((420.0, 297.0)), PageSize::A3);
        // Both sides count: a 950 x 600 mm page is wider than A1 though its long side is short
        // of A0's.
        assert_eq!(sheet_for_size((950.0, 600.0)), PageSize::A0);
        // A point-rounded A3 still reads as A3.
        assert_eq!(sheet_for_size((420.3, 297.2)), PageSize::A3);
        // A drawing most of a millimetre wider than A0 is not cut to fit it.
        assert_eq!(
            sheet_for_size((1189.8, 841.0)),
            PageSize::Custom {
                width: 1190,
                height: 841
            }
        );
        assert_eq!(
            sheet_for_size((1300.0, 900.0)),
            PageSize::Custom {
                width: 1300,
                height: 900
            }
        );
        assert_eq!(sheet_for_size((5000.0, 900.0)), PageSize::A0);
        assert!(sheet_for((5000.0, 900.0)).1.is_some_and(|w| w.contains("A0 landscape")));
        // A long, low drawing gets the smallest side a sheet may have.
        assert_eq!(
            sheet_for_size((1300.0, 80.0)),
            PageSize::Custom {
                width: 1300,
                height: 100
            }
        );
    }

    #[test]
    fn a_page_is_added_as_or_made_the_one_cover_page() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(
            &dir,
            &["page", "add", "front", "--number", "0", "--title", "Cover", "--cover"],
        )
        .unwrap();
        cli(&dir, &["page", "add", "info", "--number", "0.1", "--title", "Notes"]).unwrap();
        let covers = |dir: &Path| -> Vec<String> {
            load(dir)
                .unwrap()
                .pages
                .iter()
                .filter(|p| p.cover)
                .map(|p| p.id.clone())
                .collect()
        };
        assert_eq!(covers(&dir), ["front"]);
        // Making another page the cover moves the mark: one cover per project.
        cli(&dir, &["page", "cover", "info"]).unwrap();
        assert_eq!(covers(&dir), ["info"]);
        // Adding a new cover moves it too.
        cli(&dir, &["page", "add", "c2", "--number", "00", "--cover"]).unwrap();
        assert_eq!(covers(&dir), ["c2"]);
        cli(&dir, &["page", "cover", "c2", "--off"]).unwrap();
        assert!(covers(&dir).is_empty());
        assert!(cli(&dir, &["page", "cover", "nope"]).unwrap_err().contains("no page"));
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
    fn lib_lock_warns_about_a_library_file_declaring_another_id() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
        let mut p = load(&dir).unwrap();
        p.meta.libraries.push(LibrarySource {
            name: "starter".into(),
            kind: "folder".into(),
            location: starter.to_string_lossy().into_owned(),
            reference: None,
        });
        let (coil, _) = wc_library::Library::open(&dir, &p).unwrap().symbol("wc.coil").unwrap();
        // `bad.json` declares `other`.
        let lib = tmp.path().join("lib");
        let bad = wc_library::write_symbol(
            &lib,
            &wc_core::Symbol {
                id: "other".into(),
                ..coil
            },
        )
        .unwrap();
        std::fs::rename(&bad, lib.join("symbols").join("bad.json")).unwrap();
        p.meta.libraries = vec![LibrarySource {
            name: "lib".into(),
            kind: "folder".into(),
            location: lib.to_string_lossy().into_owned(),
            reference: None,
        }];
        p.pages[0].symbols.push(wc_core::PlacedSymbol {
            id: "s1".into(),
            symbol: "bad".into(),
            at: wc_core::Point::new(10.0, 10.0),
            rotation: Default::default(),
            mirror: false,
            tag: None,
        });
        let opened = wc_library::Library::open(&dir, &p).unwrap();
        let left = wc_library::lock_missing(&dir, &mut p, &opened).unwrap();
        let warnings = lock_warnings(&left);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("symbol bad is not pinned: "), "{warnings:?}");
        assert!(warnings[0].contains("declares id \"other\""), "{warnings:?}");
        assert!(!warnings[0].contains("no library source"), "{warnings:?}");
        assert_eq!(
            lock_warnings(&wc_library::Unpinned {
                missing: vec![(ItemKind::Part, "gone".into())],
                wrong_id: Vec::new(),
            }),
            ["part gone is in no library source"]
        );
    }

    #[test]
    fn a_dxf_import_refused_for_a_wrong_library_file_takes_nothing_and_a_retry_pins_all() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
        let mut p = load(&dir).unwrap();
        p.meta.libraries.push(LibrarySource {
            name: "starter".into(),
            kind: "folder".into(),
            location: starter.to_string_lossy().into_owned(),
            reference: None,
        });
        let (coil, _) = wc_library::Library::open(&dir, &p).unwrap().symbol("wc.coil").unwrap();
        // A library whose `aa.good` is fine and whose `zz.bad.json` declares another id.
        let lib = tmp.path().join("lib");
        let with_id = |id: &str| wc_core::Symbol {
            id: id.into(),
            ..coil.clone()
        };
        wc_library::write_symbol(&lib, &with_id("aa.good")).unwrap();
        let bad = wc_library::write_symbol(&lib, &with_id("other")).unwrap();
        std::fs::rename(&bad, lib.join("symbols").join("zz.bad.json")).unwrap();
        p.meta.libraries = vec![LibrarySource {
            name: "lib".into(),
            kind: "folder".into(),
            location: lib.to_string_lossy().into_owned(),
            reference: None,
        }];
        save(&dir, &p).unwrap();
        let page = p.pages[0].id.clone();
        let drawing = tmp.path().join("plan.dxf");
        std::fs::write(
            &drawing,
            "0\nSECTION\n2\nENTITIES\n0\nINSERT\n2\nAA.GOOD\n10\n0\n20\n0\n\
             0\nINSERT\n2\nZZ.BAD\n10\n50\n20\n0\n0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        let args = ["import", "page-dxf", drawing.to_str().unwrap(), "--page", page.as_str()];
        let err = cli(&dir, &args).unwrap_err();
        assert!(err.contains("declares id \"other\""), "{err}");
        assert!(
            store::read_vendored_symbol(&dir, "aa.good").is_err(),
            "no copy left behind without its pin"
        );

        // With the library file fixed, a page that is not there takes nothing either.
        wc_library::write_symbol(&lib, &with_id("zz.bad")).unwrap();
        let err = cli(
            &dir,
            &[
                "import",
                "page-dxf",
                drawing.to_str().unwrap(),
                "--page",
                "no-such-page",
            ],
        )
        .unwrap_err();
        assert!(err.contains("no-such-page"), "{err}");
        assert!(store::read_vendored_symbol(&dir, "aa.good").is_err());

        // A copy that cannot be written after `aa.good` was copied takes that copy away again.
        let blocked = store::vendor_path(&dir, ItemKind::Symbol, "zz.bad").unwrap();
        let mut tmp_path = blocked.as_os_str().to_owned();
        tmp_path.push(".tmp");
        let tmp_path = PathBuf::from(tmp_path);
        std::fs::create_dir_all(&tmp_path).unwrap();
        cli(&dir, &args).unwrap_err();
        assert!(
            store::read_vendored_symbol(&dir, "aa.good").is_err(),
            "the copy written first is gone again"
        );
        assert!(!load(&dir).unwrap().lock.symbols.contains_key("aa.good"));
        std::fs::remove_dir(&tmp_path).unwrap();

        // And the retry pins both.
        cli(&dir, &args).unwrap();
        let p = load(&dir).unwrap();
        assert!(p.lock.symbols.contains_key("aa.good"));
        assert!(p.lock.symbols.contains_key("zz.bad"));
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
    fn saving_from_the_command_line_keeps_a_renamed_terminal_in_its_rail_place() {
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
        // No jumper: the rail order alone is followed.
        let x1 = DeviceTag::parse("-X1").unwrap();
        p.meta.set_terminal_order(&x1, &["4".into(), "3".into()]).unwrap();
        save(&dir, &p).unwrap();
        let mut p = load(&dir).unwrap();
        p.pages[0].symbols[0].tag = DeviceTag::parse("-X1.7").ok();
        save(&dir, &p).unwrap();
        let p = load(&dir).unwrap();
        assert_eq!(p.meta.strip_layout(&x1).unwrap().order, ["4", "7"]);
    }

    #[test]
    fn pdf_report_all_prints_the_plc_card_overview_only_with_racks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        // The report title as the PDF writes text: a hex string.
        let title: String = std::iter::once("<".to_string())
            .chain("PLC cards".bytes().map(|b| format!("{b:02X}")))
            .chain(std::iter::once(">".to_string()))
            .collect();
        let has_overview = |report: &str| {
            let out = dir.join("out.pdf");
            cli(&dir, &["pdf", out.to_str().unwrap(), "--report", report]).unwrap();
            String::from_utf8_lossy(&std::fs::read(&out).unwrap()).contains(&title)
        };
        // No racks: `all` leaves the empty overview out, naming it prints it.
        assert!(!has_overview("all"));
        assert!(has_overview("plc-cards"));
        let mut p = load(&dir).unwrap();
        p.meta.racks.push(wc_core::plc_rack::PlcRack {
            name: "-A1".into(),
            cards: vec![wc_core::plc_rack::PlcCard {
                slot: 1,
                channels: 8,
                start: "%IX0.0".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        save(&dir, &p).unwrap();
        assert_eq!(load(&dir).unwrap().meta.racks, p.meta.racks);
        assert!(has_overview("all"));
        // The card is drawn on a sheet of its own too, but only with the PLC card overview.
        let drawn: String = std::iter::once("<".to_string())
            .chain("PLC card slot 1".bytes().map(|b| format!("{b:02X}")))
            .chain(std::iter::once(">".to_string()))
            .collect();
        let pdf = |report: &str| {
            let out = dir.join("out.pdf");
            cli(&dir, &["pdf", out.to_str().unwrap(), "--report", report]).unwrap();
            String::from_utf8_lossy(&std::fs::read(&out).unwrap()).into_owned()
        };
        assert!(pdf("plc-cards").contains(&drawn));
        assert!(!pdf("parts").contains(&drawn));
    }

    #[test]
    fn pdf_report_all_prints_the_network_list_only_with_networks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        // Text as the PDF writes it: a hex string.
        let hex = |t: &str| -> String {
            std::iter::once("<".to_string())
                .chain(t.bytes().map(|b| format!("{b:02X}")))
                .chain(std::iter::once(">".to_string()))
                .collect()
        };
        let pdf = |report: &str| {
            let out = dir.join("out.pdf");
            cli(&dir, &["pdf", out.to_str().unwrap(), "--report", report]).unwrap();
            String::from_utf8_lossy(&std::fs::read(&out).unwrap()).into_owned()
        };
        assert!(!pdf("all").contains(&hex("Networks")));
        assert!(pdf("networks").contains(&hex("Networks")));
        let mut p = load(&dir).unwrap();
        p.devices.insert("-A1".parse().unwrap(), Default::default());
        p.devices.insert("-K10".parse().unwrap(), Default::default());
        p.meta.networks.push(wc_core::network::Network {
            name: "PN1".into(),
            kind: "profinet".into(),
            nodes: [("-A1", "192.168.0.1"), ("-K10", "192.168.0.10")]
                .map(|(device, address)| wc_core::network::Node {
                    device: device.into(),
                    address: address.into(),
                    ..Default::default()
                })
                .into(),
            links: vec![wc_core::network::Link {
                from: "-A1:X1P1".into(),
                to: "-K10:P1".into(),
                cable: "-W20".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        save(&dir, &p).unwrap();
        assert_eq!(load(&dir).unwrap().meta.networks, p.meta.networks);
        let all = pdf("all");
        assert!(all.contains(&hex("Networks")));
        assert!(all.contains(&hex("192.168.0.10")));
        assert!(all.contains(&hex("-A1:X1P1")));
    }

    #[test]
    fn pdf_prints_the_texts_in_the_language_asked_for_and_translations_lists_what_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut p = load(&dir).unwrap();
        p.pages.push(wc_core::Page::new("p9", "9", "Pumps"));
        p.devices.insert(
            "-Q1".parse().unwrap(),
            wc_core::Device {
                description: "Main switch".into(),
                ..Default::default()
            },
        );
        save(&dir, &p).unwrap();
        std::fs::write(
            dir.join(wc_core::translate::TRANSLATIONS_FILE),
            "source = \"en\"\n[[text]]\nen = \"Pumps\"\nnb = \"Pumper\"\n",
        )
        .unwrap();
        // Text as the PDF writes it: a hex string.
        let hex = |t: &str| -> String {
            std::iter::once("<".to_string())
                .chain(t.bytes().map(|b| format!("{b:02X}")))
                .chain(std::iter::once(">".to_string()))
                .collect()
        };
        let pdf = |args: &[&str]| {
            let out = dir.join("out.pdf");
            let mut all = vec!["pdf", out.to_str().unwrap(), "--page", "p9"];
            all.extend_from_slice(args);
            cli(&dir, &all).unwrap();
            String::from_utf8_lossy(&std::fs::read(&out).unwrap()).into_owned()
        };
        let en = pdf(&[]);
        assert!(en.contains(&hex("Pumps")) && !en.contains(&hex("Pumper")));
        let nb = pdf(&["--lang", "nb"]);
        assert!(nb.contains(&hex("Pumper")) && !nb.contains(&hex("Pumps")));
        assert!(pdf(&["--lang", "nb,en"]).contains(&hex("Pumper / Pumps")));
        assert!(cli(&dir, &["pdf", "x.pdf", "--lang", "nb,en,de"]).is_err());

        // The new project's cover page title and the device description have none yet.
        let err = cli(&dir, &["translations", "--lang", "nb"]).unwrap_err();
        assert!(err.contains("2 text(s)"), "{err}");
        assert!(cli(&dir, &["translations", "--lang", "en"]).is_ok());
        std::fs::write(
            dir.join(wc_core::translate::TRANSLATIONS_FILE),
            "[[text]]\nen = \"Pumps\"\nnb = \"Pumper\"\n[[text]]\nen = \"Main switch\"\nnb = \"Hovedbryter\"\n[[text]]\nen = \"Cover\"\nnb = \"Forside\"\n",
        )
        .unwrap();
        assert!(cli(&dir, &["translations", "--lang", "nb"]).is_ok());
    }

    #[test]
    fn the_logo_is_set_and_removed_from_the_command_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 2, image::Rgb([10, 20, 30])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let src = tmp.path().join("Our logo.png");
        std::fs::write(&src, &png).unwrap();
        let text = tmp.path().join("notes.png");
        std::fs::write(&text, b"not a picture").unwrap();
        let pdf_has_picture = |dir: &Path| {
            let out = dir.join("out.pdf");
            cli(dir, &["pdf", out.to_str().unwrap()]).unwrap();
            String::from_utf8_lossy(&std::fs::read(&out).unwrap()).contains("/Subtype /Image")
        };
        assert!(!pdf_has_picture(&dir));

        let err = cli(&dir, &["logo", "set", text.to_str().unwrap()]).unwrap_err();
        assert!(err.contains("PNG or JPEG"), "{err}");
        assert_eq!(load(&dir).unwrap().meta.logo, None);

        cli(&dir, &["logo", "set", src.to_str().unwrap()]).unwrap();
        let logo = load(&dir).unwrap().meta.logo.unwrap();
        assert_eq!(logo.file, "pictures/Our_logo.png");
        assert_eq!(std::fs::read(dir.join(&logo.file)).unwrap(), png);
        assert!(pdf_has_picture(&dir));
        // The copy in the project, set again, is used as it is.
        let inside = dir.join(&logo.file);
        cli(&dir, &["logo", "set", inside.to_str().unwrap()]).unwrap();
        assert_eq!(load(&dir).unwrap().meta.logo.unwrap().file, "pictures/Our_logo.png");
        cli(&dir, &["logo", "show"]).unwrap();

        cli(&dir, &["logo", "remove"]).unwrap();
        assert_eq!(load(&dir).unwrap().meta.logo, None);
        assert!(dir.join("pictures/Our_logo.png").exists(), "the file stays");
        assert!(!pdf_has_picture(&dir));
    }

    #[test]
    fn the_page_template_is_shown_changed_and_reset_from_the_command_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let toml_text = || std::fs::read_to_string(dir.join("project.toml")).unwrap();
        // What `template show` prints is checked by running the program, in tests/template.rs.

        for bad in ["0", "41"] {
            let err = cli(&dir, &["template", "columns", bad]).unwrap_err();
            assert!(err.contains("1 to 40"), "{err}");
        }
        assert!(!toml_text().contains("[template]"));

        cli(&dir, &["template", "columns", "12"]).unwrap();
        assert_eq!(load(&dir).unwrap().meta.template.columns, 12);
        assert!(toml_text().contains("[template]"));

        cli(&dir, &["template", "reset"]).unwrap();
        assert!(load(&dir).unwrap().meta.template.is_default());
        assert!(!toml_text().contains("[template]"), "{}", toml_text());
        // A file that spells the built-in layout out loses it on reset too.
        let spelled = toml::to_string(&std::collections::BTreeMap::from([(
            "template",
            wc_core::template::PageTemplate::default(),
        )]))
        .unwrap();
        std::fs::write(dir.join("project.toml"), format!("{}\n{spelled}", toml_text())).unwrap();
        assert!(toml_text().contains("[template]"));
        assert!(load(&dir).unwrap().meta.template.is_default());
        cli(&dir, &["template", "reset"]).unwrap();
        assert!(!toml_text().contains("[template]"), "{}", toml_text());
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
    fn terminal_strips_are_reordered_given_spares_and_levels_from_the_command_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut p = load(&dir).unwrap();
        for (k, tag) in ["-X1.1", "-X1.2", "-X1.3"].into_iter().enumerate() {
            p.pages[0].symbols.push(PlacedSymbol {
                id: format!("t{k}"),
                symbol: "wc.terminal".into(),
                at: Point::new(10.0 * k as f64, 10.0),
                rotation: Rotation::default(),
                mirror: false,
                tag: DeviceTag::parse(tag).ok(),
            });
        }
        save(&dir, &p).unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let rail = || -> Vec<String> {
            let p = load(&dir).unwrap();
            let (symbols, _) = store::vendored_symbols(&dir, &p);
            wc_core::project::strip_terminals(&p, |id| symbols.get(id).map(|s| s.role))
                .remove(&x1)
                .unwrap_or_default()
                .into_iter()
                .map(|(t, spare)| if spare { format!("{t}*") } else { t })
                .collect()
        };
        assert_eq!(rail(), ["1", "2", "3"]);
        cli(&dir, &["strip", "move", "--", "-X1", "3", "1"]).unwrap();
        assert_eq!(rail(), ["3", "1", "2"]);
        cli(&dir, &["strip", "spare", "--after", "1", "--", "-X1", "S1"]).unwrap();
        cli(&dir, &["strip", "spare", "--", "-X1", "S2"]).unwrap();
        assert_eq!(rail(), ["3", "1", "S1*", "2", "S2*"]);
        assert!(
            cli(&dir, &["strip", "spare", "--", "-X1", "2"]).is_err(),
            "drawn already"
        );
        assert!(cli(&dir, &["strip", "move", "--", "-X1", "3", "0"]).is_err());
        cli(&dir, &["strip", "list"]).unwrap();
        cli(&dir, &["strip", "unspare", "--", "-X1", "S2"]).unwrap();
        assert!(cli(&dir, &["strip", "unspare", "--", "-X1", "S2"]).is_err());
        // A device entry left behind (restored by hand) goes on a second unspare.
        let s2 = DeviceTag::parse("-X1.S2").unwrap();
        let mut p = load(&dir).unwrap();
        p.devices.entry(s2.clone()).or_default().part = Some("wago.2002-1201".into());
        save(&dir, &p).unwrap();
        cli(&dir, &["strip", "unspare", "--", "-X1", "S2"]).unwrap();
        assert!(!load(&dir).unwrap().devices.contains_key(&s2));
        assert!(cli(&dir, &["strip", "unspare", "--", "-X1", "S2"]).is_err());
        cli(&dir, &["strip", "order", "--", "-X1", "2", "S1"]).unwrap();
        assert_eq!(rail(), ["2", "S1*", "1", "3"], "left out ones follow by number");
        // Levels are the terminal device's Level property.
        cli(&dir, &["strip", "level", "--", "-X1", "S1", "2"]).unwrap();
        let level = |t: &str| {
            load(&dir)
                .unwrap()
                .devices
                .get(&DeviceTag::parse(t).unwrap())
                .and_then(|d| d.properties.get(wc_report::LEVEL_PROPERTY).cloned())
        };
        assert_eq!(level("-X1.S1").as_deref(), Some("2"));
        cli(&dir, &["strip", "level", "--", "-X1", "S1"]).unwrap();
        assert_eq!(level("-X1.S1"), None);
        assert!(cli(&dir, &["strip", "level", "--", "-X1", "9", "2"]).is_err());
        assert!(cli(&dir, &["strip", "level", "--", "-X1", "1", "0"]).is_err());
        // Back to the order by number; the spare stays.
        cli(&dir, &["strip", "order", "--", "-X1"]).unwrap();
        assert_eq!(rail(), ["1", "2", "3", "S1*"]);
    }

    #[test]
    fn a_jumper_part_is_set_and_cleared_from_the_command_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut p = load(&dir).unwrap();
        p.meta.libraries.push(LibrarySource {
            name: "starter".into(),
            kind: "folder".into(),
            location: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../library")
                .to_string_lossy()
                .into_owned(),
            reference: None,
        });
        save(&dir, &p).unwrap();
        cli(&dir, &["jumper", "add", "--", "-X1", "3", "4"]).unwrap();
        let err = cli(&dir, &["jumper", "part", "--", "-X1", "9", "example.ex-tb-2-5"]).unwrap_err();
        assert!(err.contains("no jumper of -X1 has terminal 9"), "{err}");
        assert!(
            store::read_vendored_part(&dir, "example.ex-tb-2-5").is_err(),
            "nothing taken"
        );
        assert!(cli(&dir, &["jumper", "part", "--", "-X1", "3", "no.such.part"]).is_err());
        assert_eq!(load(&dir).unwrap().meta.jumpers[0].part, None);

        cli(&dir, &["jumper", "part", "--", "-X1", "4", "example.ex-tb-2-5"]).unwrap();
        let p = load(&dir).unwrap();
        assert_eq!(p.meta.jumpers[0].part.as_deref(), Some("example.ex-tb-2-5"));
        assert!(p.lock.parts.contains_key("example.ex-tb-2-5"), "taken into the project");
        assert!(vendored_parts(&dir, &p).contains_key("example.ex-tb-2-5"));
        cli(&dir, &["jumper", "list"]).unwrap();

        cli(&dir, &["jumper", "part", "--", "-X1", "3"]).unwrap();
        assert_eq!(load(&dir).unwrap().meta.jumpers[0].part, None);
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
    fn a_library_import_keeps_every_wired_pin_of_a_symbol_it_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let coil = |pins: &[&str]| wc_core::Symbol {
            source: String::new(),
            id: "swe.lib.coil".into(),
            name: "Coil".into(),
            category: String::new(),
            letter_code: "K".into(),
            role: Default::default(),
            graphics: vec![],
            pins: pins
                .iter()
                .map(|n| wc_core::Pin {
                    name: (*n).into(),
                    at: Default::default(),
                    dir: Default::default(),
                })
                .collect(),
            tag_at: Default::default(),
            description: String::new(),
        };
        let built = |pins: &[&str]| wc_import::aml::Built {
            symbols: vec![coil(pins)],
            ..Default::default()
        };
        let mut p = load(&dir).unwrap();
        add_and_save(
            &dir,
            &mut p,
            built(&["A1", "A2"]),
            "import:a.tewzip",
            Replace::Unchanged,
        )
        .unwrap();
        // Two coils on the first page, wired A2 to A2.
        let mut p = load(&dir).unwrap();
        let page = &mut p.pages[0];
        for (id, x) in [("k1", 10.0), ("k2", 50.0)] {
            page.symbols.push(wc_core::PlacedSymbol {
                id: id.into(),
                symbol: "swe.lib.coil".into(),
                at: wc_core::geom::Point::new(x, 50.0),
                rotation: Default::default(),
                mirror: false,
                tag: None,
            });
        }
        let end = |item: &str| wc_core::Endpoint::Pin {
            item: item.into(),
            pin: "A2".into(),
        };
        page.wires.push(wc_core::Wire {
            id: "w1".into(),
            from: end("k1"),
            to: end("k2"),
            via: Vec::new(),
            number: None,
            potential: None,
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        });
        let number = page.number.clone();
        save(&dir, &p).unwrap();

        let mut p = load(&dir).unwrap();
        let err = add_and_save(&dir, &mut p, built(&["A1"]), "import:a.tewzip", Replace::Unchanged).unwrap_err();
        assert!(err.contains(&format!("page {number} k1:A2")), "{err}");
        assert_eq!(
            store::read_vendored_symbol(&dir, "swe.lib.coil").unwrap(),
            coil(&["A1", "A2"])
        );
        // A version that keeps the wired pins replaces it.
        let mut p = load(&dir).unwrap();
        add_and_save(
            &dir,
            &mut p,
            built(&["A1", "A2", "X1"]),
            "import:a.tewzip",
            Replace::Unchanged,
        )
        .unwrap();
        assert_eq!(
            store::read_vendored_symbol(&dir, "swe.lib.coil").unwrap(),
            coil(&["A1", "A2", "X1"])
        );
    }

    /// #301: an id given by name can move to another block in a later catalog; the import then
    /// refuses to replace the project's copy with that other block.
    #[test]
    fn an_import_does_not_replace_a_copy_with_another_block_of_the_library() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let coil = |source: &str, name: &str| wc_core::Symbol {
            source: source.into(),
            id: "swe.lib.coil".into(),
            name: name.into(),
            category: String::new(),
            letter_code: "K".into(),
            role: Default::default(),
            graphics: vec![],
            pins: vec![],
            tag_at: Default::default(),
            description: String::new(),
        };
        let built = |s: wc_core::Symbol| wc_import::aml::Built {
            symbols: vec![s],
            ..Default::default()
        };
        let first = coil("solidworks-electrical:block:guid:aaa", "Coil");
        let mut p = load(&dir).unwrap();
        add_built(
            &dir,
            &mut p,
            built(first.clone()),
            "import:a.tewzip",
            Replace::Unchanged,
        )
        .unwrap();
        save(&dir, &p).unwrap();

        // Another block took the id: refused, the copy stays.
        let mut p = load(&dir).unwrap();
        let other = coil("solidworks-electrical:block:guid:000", "Coil (new)");
        let err = add_built(&dir, &mut p, built(other), "import:a.tewzip", Replace::Unchanged).unwrap_err();
        assert!(err.contains("another one of its library"), "{err}");
        // Which block it is cannot be told: refused too.
        let mut p = load(&dir).unwrap();
        let unsure = coil(
            &format!(
                "{}solidworks-electrical:block:name:Coil",
                wc_core::symbol::AMBIGUOUS_SOURCE
            ),
            "Coil 2",
        );
        let err = add_built(&dir, &mut p, built(unsure), "import:a.tewzip", Replace::Unchanged).unwrap_err();
        assert!(err.contains("cannot be told"), "{err}");
        assert_eq!(store::read_vendored_symbol(&dir, "swe.lib.coil").unwrap(), first);

        // The same block in a new version replaces it.
        let mut p = load(&dir).unwrap();
        let newer = coil("solidworks-electrical:block:guid:aaa", "Coil, drawn");
        add_built(
            &dir,
            &mut p,
            built(newer.clone()),
            "import:a.tewzip",
            Replace::Unchanged,
        )
        .unwrap();
        save(&dir, &p).unwrap();
        assert_eq!(store::read_vendored_symbol(&dir, "swe.lib.coil").unwrap(), newer);
    }

    #[test]
    fn an_import_replaces_an_item_the_project_has_not_changed_and_refuses_one_it_has() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let coil = |name: &str| wc_core::Symbol {
            source: String::new(),
            id: "swe.lib.coil".into(),
            name: name.into(),
            category: String::new(),
            letter_code: "K".into(),
            role: Default::default(),
            // A drawn block has its drawing.
            graphics: if name.contains("drawn") {
                vec![wc_core::geom::Shape::Line {
                    a: wc_core::geom::Point::new(0.0, 0.0),
                    b: wc_core::geom::Point::new(10.0, 0.0),
                }]
            } else {
                vec![]
            },
            pins: vec![],
            tag_at: Default::default(),
            description: String::new(),
        };
        let relay = |text: &str| wc_core::Part {
            id: "maker-a.rly-24".into(),
            description: text.into(),
            ..Default::default()
        };
        let built = |name: &str, text: &str| wc_import::aml::Built {
            symbols: vec![coil(name)],
            parts: vec![relay(text)],
            ..Default::default()
        };
        let mut p = load(&dir).unwrap();
        add_built(
            &dir,
            &mut p,
            built("Coil, box", "Relay"),
            "import:a.tewzip",
            Replace::Unchanged,
        )
        .unwrap();
        save(&dir, &p).unwrap();

        // The block is drawn now and the catalog is newer: both replace the untouched copies.
        let mut p = load(&dir).unwrap();
        add_built(
            &dir,
            &mut p,
            built("Coil, drawn", "Relay 24 V"),
            "import:a.tewzip",
            Replace::Unchanged,
        )
        .unwrap();
        save(&dir, &p).unwrap();
        let p = load(&dir).unwrap();
        assert_eq!(
            store::read_vendored_symbol(&dir, "swe.lib.coil").unwrap(),
            coil("Coil, drawn")
        );
        assert_eq!(p.lock.symbols["swe.lib.coil"].hash, coil("Coil, drawn").hash());
        assert_eq!(p.lock.parts["maker-a.rly-24"].hash, relay("Relay 24 V").hash());

        assert!(
            !store::read_vendored_symbol(&dir, "swe.lib.coil")
                .unwrap()
                .graphics
                .is_empty()
        );

        // A write that fails after the coil was replaced puts the coil's copy back as it was,
        // and the saved lock keeps its pins.
        let coil_file = store::vendor_path(&dir, ItemKind::Symbol, "swe.lib.coil").unwrap();
        let coil_bytes = std::fs::read(&coil_file).unwrap();
        let relay_file = store::vendor_path(&dir, ItemKind::Part, "maker-a.rly-24").unwrap();
        let mut blocker = relay_file.as_os_str().to_owned();
        blocker.push(".tmp");
        std::fs::create_dir(&blocker).unwrap();
        let mut p = load(&dir).unwrap();
        let lock = p.lock.clone();
        assert!(
            add_built(
                &dir,
                &mut p,
                built("Coil, drawn again", "Relay 24 V, v2"),
                "import:a.tewzip",
                Replace::Unchanged
            )
            .is_err()
        );
        std::fs::remove_dir(&blocker).unwrap();
        assert_eq!(std::fs::read(&coil_file).unwrap(), coil_bytes, "put back");
        assert_eq!(load(&dir).unwrap().lock, lock);

        // A symbol the import brings new is removed again when a later write fails.
        let lamp = wc_core::Symbol {
            id: "swe.lib.lamp".into(),
            ..coil("Lamp")
        };
        let lamp_file = store::vendor_path(&dir, ItemKind::Symbol, "swe.lib.lamp").unwrap();
        std::fs::create_dir(&blocker).unwrap();
        let mut p = load(&dir).unwrap();
        let fresh = wc_import::aml::Built {
            symbols: vec![lamp],
            parts: vec![relay("Relay 24 V, v2")],
            ..Default::default()
        };
        assert!(add_built(&dir, &mut p, fresh, "import:a.tewzip", Replace::Unchanged).is_err());
        std::fs::remove_dir(&blocker).unwrap();
        assert!(!lamp_file.exists(), "the new copy is removed");
        assert_eq!(load(&dir).unwrap().lock, lock);

        // A save that fails after the files were written puts them back as well.
        let lock_blocker = dir.join(format!("{}.tmp", store::LOCK_FILE));
        std::fs::create_dir(&lock_blocker).unwrap();
        let mut p = load(&dir).unwrap();
        assert!(
            add_and_save(
                &dir,
                &mut p,
                built("Coil, drawn again", "Relay 24 V, v2"),
                "import:a.tewzip",
                Replace::Unchanged
            )
            .is_err()
        );
        std::fs::remove_dir(&lock_blocker).unwrap();
        assert_eq!(
            std::fs::read(&coil_file).unwrap(),
            coil_bytes,
            "put back after the failed save"
        );
        assert_eq!(load(&dir).unwrap().lock, lock);

        // A save that fails at a page, after the lock was written, puts the lock back too, so
        // the same import goes through once the page can be written.
        let page_file = dir
            .join(store::PAGES_DIR)
            .join(format!("swe-page-9{}", store::PAGE_SUFFIX));
        let mut page_blocker = page_file.as_os_str().to_owned();
        page_blocker.push(".tmp");
        std::fs::create_dir_all(&page_blocker).unwrap();
        let with_page = || wc_import::aml::Built {
            pages: vec![Page::new("swe-page-9", "9", "Nine")],
            ..built("Coil, drawn again", "Relay 24 V, v2")
        };
        let mut p = load(&dir).unwrap();
        let lock_bytes = std::fs::read(dir.join(store::LOCK_FILE)).unwrap();
        assert!(add_and_save(&dir, &mut p, with_page(), "import:a.tewzip", Replace::Unchanged).is_err());
        std::fs::remove_dir(&page_blocker).unwrap();
        assert_eq!(
            std::fs::read(dir.join(store::LOCK_FILE)).unwrap(),
            lock_bytes,
            "lock put back"
        );
        assert_eq!(std::fs::read(&coil_file).unwrap(), coil_bytes);
        let mut p = load(&dir).unwrap();
        add_and_save(&dir, &mut p, with_page(), "import:a.tewzip", Replace::Unchanged).unwrap();
        let p = load(&dir).unwrap();
        assert_eq!(p.lock.symbols["swe.lib.coil"].hash, coil("Coil, drawn again").hash());
        // Put the earlier state back for the steps below.
        let mut p = p;
        p.pages.retain(|pg| pg.id != "swe-page-9");
        save(&dir, &p).unwrap();
        std::fs::write(&coil_file, &coil_bytes).unwrap();
        p.lock.symbols.get_mut("swe.lib.coil").unwrap().hash = coil("Coil, drawn").hash();
        p.lock.parts.get_mut("maker-a.rly-24").unwrap().hash = relay("Relay 24 V").hash();
        store::write_vendored_part(&dir, &relay("Relay 24 V")).unwrap();
        save(&dir, &p).unwrap();

        // A file the save may write that cannot be read beforehand stops the import before
        // anything is written: a folder stands where the new page goes.
        let page_dir = dir
            .join(store::PAGES_DIR)
            .join(format!("swe-page-9{}", store::PAGE_SUFFIX));
        let mut p = load(&dir).unwrap();
        std::fs::create_dir_all(&page_dir).unwrap();
        let lock_bytes = std::fs::read(dir.join(store::LOCK_FILE)).unwrap();
        let err = add_and_save(&dir, &mut p, with_page(), "import:a.tewzip", Replace::Unchanged).unwrap_err();
        assert!(err.contains("swe-page-9"), "{err}");
        std::fs::remove_dir(&page_dir).unwrap();
        assert_eq!(std::fs::read(dir.join(store::LOCK_FILE)).unwrap(), lock_bytes);
        assert_eq!(std::fs::read(&coil_file).unwrap(), coil_bytes);

        // A save that changes one comment file and then fails on the next puts the first back.
        let comment = |page: &str, text: &str| wc_core::review::Comment {
            id: format!("c-{page}"),
            page: page.into(),
            at: None,
            author: "Ann".into(),
            time: 0,
            text: text.into(),
            resolved: false,
            replies: Vec::new(),
        };
        let mut p = load(&dir).unwrap();
        let first = p.pages[0].id.clone();
        assert!(first.as_str() < "swe-page-9", "written before the new page's comments");
        p.comments.push(comment(&first, "old"));
        save(&dir, &p).unwrap();
        let first_file = dir
            .join(store::COMMENTS_DIR)
            .join(format!("{first}{}", store::COMMENTS_SUFFIX));
        let first_bytes = std::fs::read(&first_file).unwrap();
        let mut blocker = dir
            .join(store::COMMENTS_DIR)
            .join(format!("swe-page-9{}", store::COMMENTS_SUFFIX))
            .into_os_string();
        blocker.push(".tmp");
        std::fs::create_dir_all(&blocker).unwrap();
        let mut p = load(&dir).unwrap();
        p.comments[0].text = "new".into();
        p.comments.push(comment("swe-page-9", "on the new page"));
        assert!(add_and_save(&dir, &mut p, with_page(), "import:a.tewzip", Replace::Unchanged).is_err());
        std::fs::remove_dir(&blocker).unwrap();
        assert_eq!(
            std::fs::read(&first_file).unwrap(),
            first_bytes,
            "comment file put back"
        );
        assert_eq!(std::fs::read(dir.join(store::LOCK_FILE)).unwrap(), lock_bytes);
        let mut p = load(&dir).unwrap();
        assert!(p.page("swe-page-9").is_none());
        p.comments.clear();
        save(&dir, &p).unwrap();

        // A project import never replaces: its symbols may be placed on pages the project kept.
        let mut p = load(&dir).unwrap();
        let err = add_built(
            &dir,
            &mut p,
            built("Coil, v4", "Relay 24 V"),
            "import:a.tewzip",
            Replace::No,
        )
        .unwrap_err();
        assert!(err.contains("symbol swe.lib.coil exists with other content"), "{err}");
        assert_eq!(std::fs::read(&coil_file).unwrap(), coil_bytes);

        // Two pages with one id in an import are refused before anything is written.
        let twice = wc_import::aml::Built {
            pages: vec![Page::new("swe-page-7", "7", "a"), Page::new("swe-page-7", "8", "b")],
            ..built("Coil, v4", "Relay 24 V, v4")
        };
        let err = add_built(&dir, &mut p, twice, "import:a.tewzip", Replace::Unchanged).unwrap_err();
        assert!(err.contains("page swe-page-7 twice"), "{err}");
        assert_eq!(std::fs::read(&coil_file).unwrap(), coil_bytes);

        // Changed in the project since: still refused, and nothing is written.
        let edited = coil("Coil, edited here");
        store::write_vendored_symbol(&dir, &edited).unwrap();
        let mut p = load(&dir).unwrap();
        let before = p.clone();
        let err = add_built(
            &dir,
            &mut p,
            built("Coil, v3", "Relay 24 V, v3"),
            "import:a.tewzip",
            Replace::Unchanged,
        )
        .unwrap_err();
        assert!(err.contains("symbol swe.lib.coil exists with other content"), "{err}");
        assert_eq!(p, before);
        assert_eq!(store::read_vendored_symbol(&dir, "swe.lib.coil").unwrap(), edited);
        assert_eq!(
            store::read_vendored_part(&dir, "maker-a.rly-24").unwrap(),
            relay("Relay 24 V")
        );
        // Also when the import brings the pinned version itself: the edit is not written over.
        let err = add_built(
            &dir,
            &mut p,
            built("Coil, drawn", "Relay 24 V"),
            "import:a.tewzip",
            Replace::Unchanged,
        )
        .unwrap_err();
        assert!(err.contains("symbol swe.lib.coil exists with other content"), "{err}");
        assert_eq!(store::read_vendored_symbol(&dir, "swe.lib.coil").unwrap(), edited);

        // A copy that cannot be read says why.
        std::fs::write(&coil_file, "{ not json").unwrap();
        let err = add_built(
            &dir,
            &mut p,
            built("Coil, v3", "Relay 24 V, v3"),
            "import:a.tewzip",
            Replace::Unchanged,
        )
        .unwrap_err();
        assert!(err.contains("cannot be read") && err.contains("line 1"), "{err}");
    }

    #[test]
    fn imported_jumpers_join_the_projects_own() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let x1 = wc_core::DeviceTag::parse("-X1").unwrap();
        let mut p = load(&dir).unwrap();
        p.meta.add_jumper(&x1, &["3".to_owned(), "4".to_owned()]).unwrap();
        let jumper = |terminals: &[&str]| wc_core::project::Jumper {
            strip: x1.clone(),
            terminals: terminals.iter().map(|t| (*t).to_owned()).collect(),
            part: None,
        };
        let built = wc_import::aml::Built {
            // 5-6 is listed before the group that links it to the project's jumper.
            jumpers: vec![jumper(&["5", "6"]), jumper(&["4", "5"]), jumper(&["7", "8"])],
            ..Default::default()
        };
        let (_, joined) = add_built(&dir, &mut p, built, "import:a.tewzip", Replace::No).unwrap();
        let mut sets: Vec<Vec<String>> = p.meta.jumpers.iter().map(|j| j.terminals.clone()).collect();
        sets.sort();
        assert_eq!(sets, [vec!["3", "4", "5", "6"], vec!["7", "8"]]);
        // 4-5 joined the project's 3-4, and 5-6 joined it through 4-5; 7-8 is a jumper of its own.
        assert_eq!(joined, 2, "the report tells merged groups apart from new jumpers");
    }

    #[test]
    fn imported_jumpers_keep_their_numbers_when_the_import_renumbers_the_projects_terminals() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let terminal = |id: &str, x: f64, tag: &str| PlacedSymbol {
            id: id.into(),
            symbol: "test.term".into(),
            at: Point::new(x, 10.0),
            rotation: Rotation::default(),
            mirror: false,
            tag: DeviceTag::parse(tag).ok(),
        };
        let mut p = load(&dir).unwrap();
        // A terminal symbol in the project, so terminals tagged with only their strip count.
        let term = wc_core::Symbol {
            source: String::new(),
            id: "test.term".into(),
            name: "Terminal".into(),
            category: String::new(),
            letter_code: "X".into(),
            role: wc_core::symbol::SymbolRole::Terminal,
            graphics: vec![],
            pins: vec![],
            tag_at: Default::default(),
            description: String::new(),
        };
        store::write_vendored_symbol(&dir, &term).unwrap();
        // -X1.1, then two terminals tagged with only the strip: numbered 2 and 3.
        p.pages[0].symbols.extend([
            terminal("a", 10.0, "-X1.1"),
            terminal("b", 20.0, "-X1"),
            terminal("c", 30.0, "-X1"),
        ]);
        let x1 = DeviceTag::parse("-X1").unwrap();
        p.meta.add_jumper(&x1, &["2".into(), "3".into()]).unwrap();
        save(&dir, &p).unwrap();
        // The import brings -X1.2 and -X1.9: the strip-only terminals become 10 and 11.
        let mut page = p.pages[0].clone();
        page.id = "imported".into();
        page.number = "99".into();
        page.cover = false;
        page.overview = false;
        page.symbols = vec![terminal("d", 10.0, "-X1.2"), terminal("e", 20.0, "-X1.9")];
        let built = wc_import::aml::Built {
            pages: vec![page],
            jumpers: vec![wc_core::project::Jumper {
                strip: x1.clone(),
                terminals: vec!["1".into(), "2".into()],
                part: None,
            }],
            ..Default::default()
        };
        let mut p = load(&dir).unwrap();
        add_built(&dir, &mut p, built, "import:a.tewzip", Replace::No).unwrap();
        save_imported(&dir, &p).unwrap();
        let mut sets: Vec<Vec<String>> = load(&dir)
            .unwrap()
            .meta
            .jumpers
            .iter()
            .map(|j| {
                let mut t = j.terminals.clone();
                t.sort();
                t
            })
            .collect();
        sets.sort();
        assert_eq!(sets, [vec!["1", "2"], vec!["10", "11"]]);
    }

    #[test]
    fn an_import_that_brings_a_missing_terminal_symbol_does_not_guess_where_jumpers_go() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let term = |id: &str| wc_core::Symbol {
            source: String::new(),
            id: id.into(),
            name: "Terminal".into(),
            category: String::new(),
            letter_code: "X".into(),
            role: wc_core::symbol::SymbolRole::Terminal,
            graphics: vec![],
            pins: vec![],
            tag_at: Default::default(),
            description: String::new(),
        };
        let placed = |id: &str, symbol: &str, x: f64| PlacedSymbol {
            id: id.into(),
            symbol: symbol.into(),
            at: Point::new(x, 10.0),
            rotation: Rotation::default(),
            mirror: false,
            tag: DeviceTag::parse("-X1").ok(),
        };
        let mut p = load(&dir).unwrap();
        // Three terminals tagged with only the strip; the first one's symbol has no copy in the
        // project, and the import brings it. Jumper 1-2 is b-c if it was numbered without that
        // symbol, a-b if with it.
        store::write_vendored_symbol(&dir, &term("test.term")).unwrap();
        p.pages[0].symbols.extend([
            placed("a", "test.missing", 10.0),
            placed("b", "test.term", 20.0),
            placed("c", "test.term", 30.0),
        ]);
        let x1 = DeviceTag::parse("-X1").unwrap();
        p.meta.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        store::save(&dir, &p).unwrap();
        let mut p = load(&dir).unwrap();
        let built = || wc_import::aml::Built {
            symbols: vec![term("test.missing")],
            ..Default::default()
        };
        let err = add_built(&dir, &mut p, built(), "import:a.tewzip", Replace::No).unwrap_err();
        assert!(err.contains("no readable copy of test.missing"), "{err}");
        assert!(
            store::read_vendored_symbol(&dir, "test.missing").is_err(),
            "nothing written"
        );
        // With the file back the numbering is known, and the import goes ahead.
        store::write_vendored_symbol(&dir, &term("test.missing")).unwrap();
        let mut p = load(&dir).unwrap();
        add_built(&dir, &mut p, built(), "import:a.tewzip", Replace::No).unwrap();
        assert_eq!(p.meta.jumpers[0].terminals, ["1", "2"]);
    }

    #[test]
    fn an_import_that_brings_a_missing_terminal_symbol_does_not_guess_a_rail_order() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let term = |id: &str| wc_core::Symbol {
            source: String::new(),
            id: id.into(),
            name: "Terminal".into(),
            category: String::new(),
            letter_code: "X".into(),
            role: wc_core::symbol::SymbolRole::Terminal,
            graphics: vec![],
            pins: vec![],
            tag_at: Default::default(),
            description: String::new(),
        };
        let placed = |id: &str, symbol: &str, x: f64| PlacedSymbol {
            id: id.into(),
            symbol: symbol.into(),
            at: Point::new(x, 10.0),
            rotation: Rotation::default(),
            mirror: false,
            tag: DeviceTag::parse("-X1").ok(),
        };
        let mut p = load(&dir).unwrap();
        // No jumpers, only a rail order "2, 1": c then b without the missing symbol, b then a
        // with it.
        store::write_vendored_symbol(&dir, &term("test.term")).unwrap();
        p.pages[0].symbols.extend([
            placed("a", "test.missing", 10.0),
            placed("b", "test.term", 20.0),
            placed("c", "test.term", 30.0),
        ]);
        let x1 = DeviceTag::parse("-X1").unwrap();
        p.meta.set_terminal_order(&x1, &["2".into(), "1".into()]).unwrap();
        store::save(&dir, &p).unwrap();
        let mut p = load(&dir).unwrap();
        let built = || wc_import::aml::Built {
            symbols: vec![term("test.missing")],
            ..Default::default()
        };
        let err = add_built(&dir, &mut p, built(), "import:a.tewzip", Replace::No).unwrap_err();
        assert!(err.contains("rail order of -X1"), "{err}");
        assert!(err.contains("no readable copy of test.missing"), "{err}");
        assert!(
            store::read_vendored_symbol(&dir, "test.missing").is_err(),
            "nothing written"
        );
        let order = |p: &Project| p.meta.strip_layout(&x1).map(|l| l.order.clone());
        assert_eq!(order(&load(&dir).unwrap()), Some(vec!["2".into(), "1".into()]));
    }

    #[test]
    fn a_swe_import_whose_save_fails_leaves_jumpers_and_pages_as_they_were() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let term = wc_core::Symbol {
            source: String::new(),
            id: "test.term".into(),
            name: "Terminal".into(),
            category: String::new(),
            letter_code: "X".into(),
            role: wc_core::symbol::SymbolRole::Terminal,
            graphics: vec![],
            pins: vec![],
            tag_at: Default::default(),
            description: String::new(),
        };
        let placed = |id: &str, tag: &str, x: f64| PlacedSymbol {
            id: id.into(),
            symbol: "test.term".into(),
            at: Point::new(x, 10.0),
            rotation: Rotation::default(),
            mirror: false,
            tag: DeviceTag::parse(tag).ok(),
        };
        store::write_vendored_symbol(&dir, &term).unwrap();
        let mut p = load(&dir).unwrap();
        // Two terminals tagged with only the strip (1 and 2), jumpered.
        p.pages[0]
            .symbols
            .extend([placed("b", "-X1", 20.0), placed("c", "-X1", 30.0)]);
        let x1 = DeviceTag::parse("-X1").unwrap();
        p.meta.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        store::save(&dir, &p).unwrap();
        // The import draws terminals 5 and 6 with their own jumper: the project's become 7 and 8.
        let built = || {
            let mut page = Page::new("swe-page-9", "9", "Nine");
            page.symbols
                .extend([placed("t5", "-X1.5", 10.0), placed("t6", "-X1.6", 20.0)]);
            wc_import::aml::Built {
                pages: vec![page],
                jumpers: vec![wc_core::project::Jumper {
                    strip: x1.clone(),
                    terminals: vec!["5".into(), "6".into()],
                    part: None,
                }],
                ..Default::default()
            }
        };
        let page_file = dir
            .join(store::PAGES_DIR)
            .join(format!("swe-page-9{}", store::PAGE_SUFFIX));
        let mut blocker = page_file.as_os_str().to_owned();
        blocker.push(".tmp");
        std::fs::create_dir_all(&blocker).unwrap();
        let project_toml = std::fs::read(dir.join(store::PROJECT_FILE)).unwrap();
        let sources = std::collections::BTreeMap::new();
        let mut p = load(&dir).unwrap();
        assert!(add_swe_built(&dir, &mut p, built(), "import:a.tewzip", &sources).is_err());
        std::fs::remove_dir(&blocker).unwrap();
        assert_eq!(
            std::fs::read(dir.join(store::PROJECT_FILE)).unwrap(),
            project_toml,
            "the jumpers on disk are the ones before the import"
        );
        // The same import again keeps the two jumpers apart.
        let mut p = load(&dir).unwrap();
        add_swe_built(&dir, &mut p, built(), "import:a.tewzip", &sources).unwrap();
        let mut sets: Vec<Vec<String>> = load(&dir)
            .unwrap()
            .meta
            .jumpers
            .iter()
            .map(|j| j.terminals.clone())
            .collect();
        sets.sort();
        assert_eq!(sets, [vec!["5", "6"], vec!["7", "8"]]);
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
        add_built(&dir, &mut p, built, "import:a.tewzip", Replace::Unchanged).unwrap();
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
        let mut cover = Page::new("swe-0001", "1", "Cover");
        cover.cover = true;
        cover.title_block.insert("revision".into(), "C".into());
        let built = wc_import::aml::Built {
            panels: vec![panel("swe-layout-c"), panel("swe-layout-a")],
            parts: vec![part],
            pages: vec![cover],
            ..Default::default()
        };
        let err = add_built(&dir, &mut p, built, "import:b.tewzip", Replace::Unchanged).unwrap_err();
        assert!(err.contains("panel swe-layout-a exists"), "{err}");
        assert_eq!(p, before);
        assert!(
            store::read_vendored_part(&dir, "maker.new-part").is_err(),
            "no part file written"
        );
        assert_eq!(std::fs::read(dir.join("project.toml")).unwrap(), project_file);
    }

    #[test]
    fn a_pages_own_title_fields_are_set_listed_and_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        let d = dir.to_str().unwrap();
        let cli = |args: &[&str]| {
            let mut all = vec!["wiringcraft", "--project", d];
            all.extend(args);
            run(Cli::try_parse_from(all).unwrap())
        };
        run(Cli::try_parse_from(["wiringcraft", "new", d, "--no-git"]).unwrap()).unwrap();
        cli(&["page", "add", "p9", "--number", "9", "--title", "Motors"]).unwrap();
        let mut p = load(&dir).unwrap();
        p.meta.title_block.insert("revision".into(), "A".into());
        save(&dir, &p).unwrap();

        cli(&["page", "fields", "p9", "revision=B", "checked_by=OH"]).unwrap();
        let p = load(&dir).unwrap();
        let i = p.pages.iter().position(|pg| pg.id == "p9").unwrap();
        let listed = page_fields(&p, i);
        assert!(
            listed.contains("revision") && listed.contains("B") && listed.contains("(page)"),
            "{listed}"
        );
        assert!(listed.contains("checked_by") && listed.contains("OH"), "{listed}");

        cli(&["page", "fields", "p9", "revision="]).unwrap();
        let p = load(&dir).unwrap();
        let line = page_fields(&p, i)
            .lines()
            .find(|l| l.starts_with("revision"))
            .unwrap()
            .to_owned();
        assert!(line.contains('A') && line.contains("(project)"), "falls back: {line}");
        assert_eq!(p.pages[i].title_block.get("checked_by").map(String::as_str), Some("OH"));

        // A value that would steer the terminal or start a false row is printed escaped.
        let mut p = load(&dir).unwrap();
        p.pages[i]
            .title_block
            .insert("note\nfake".into(), "A\u{1b}[2JB\nrevision X".into());
        let listed = page_fields(&p, i);
        assert!(!listed.contains('\u{1b}'), "{listed}");
        assert_eq!(
            listed.lines().filter(|l| l.starts_with("revision")).count(),
            1,
            "{listed}"
        );
        assert!(
            listed.contains("note\\nfake") && listed.contains("A\\u{1b}[2JB\\nrevision X"),
            "{listed}"
        );

        assert!(cli(&["page", "fields", "p9", "revision"]).is_err());
        assert!(cli(&["page", "fields", "p9", "bad key=x"]).is_err());
        assert!(cli(&["page", "fields", "nope", "revision=C"]).is_err());
    }

    #[test]
    fn an_imported_cover_leaves_the_project_one_cover_page() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut p = load(&dir).unwrap();
        assert_eq!(
            p.pages.iter().filter(|pg| pg.cover).count(),
            1,
            "a new project has a cover"
        );
        let mut cover = Page::new("swe-0001", "1", "Cover");
        cover.cover = true;
        // A blank value does not hide a later cover's value.
        cover.title_block.insert("revision".into(), " ".into());
        let mut second = Page::new("swe-0002", "2", "Cover 2");
        second.cover = true;
        second.title_block.insert("revision".into(), "C".into());
        let built = wc_import::aml::Built {
            pages: vec![cover, second, Page::new("swe-0003", "3", "Power")],
            ..Default::default()
        };
        // The imported cover is numbered 1 like the project's own: no clash, it is merged.
        add_built(&dir, &mut p, built, "import:a.tewzip", Replace::Unchanged).unwrap();
        let covers: Vec<_> = p.pages.iter().filter(|pg| pg.cover).collect();
        assert_eq!(covers.len(), 1);
        assert_eq!(covers[0].title_block.get("revision").map(String::as_str), Some("C"));
        assert!(p.page("swe-0003").is_some());
        assert!(p.page("swe-0001").is_none() && p.page("swe-0002").is_none());

        // Into a project without a cover only the first imported cover comes.
        let mut p = Project::new("t");
        p.pages.retain(|pg| !pg.cover);
        let mut a = Page::new("swe-0001", "1", "Cover");
        a.cover = true;
        let mut b = Page::new("swe-0002", "2", "Cover 2");
        b.cover = true;
        let mut pages = vec![a, b];
        assert!(keep_one_cover(&p, &mut pages).is_empty());
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].id, "swe-0001");
    }

    #[test]
    fn a_found_value_stays_on_one_line() {
        assert_eq!(escape_field("Motor\nStart\tA\\B\r"), "Motor\\nStart\\tA\\\\B\\r");
        assert_eq!(escape_field("-K1"), "-K1");
        assert_eq!(escape_field("a\u{1b}]52;c;x\u{7}"), "a\\u{1b}]52;c;x\\u{7}");
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
        let write_archive = |app: &rusqlite::Connection| {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
            let opt = zip::write::SimpleFileOptions::default();
            for (name, c) in [
                ("Database/tew_catalog.sqlite", &catalog),
                ("Database/tew_app_data.sqlite", app),
            ] {
                z.start_file(name, opt).unwrap();
                z.write_all(&c.serialize(rusqlite::MAIN_DB).unwrap()).unwrap();
            }
            z.finish().unwrap();
        };
        write_archive(&app);
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

        // A newer archive with another version of the coil replaces the untouched copy.
        app.execute_batch("INSERT INTO tew_blockpt VALUES (1, 'X1', 5, 0);")
            .unwrap();
        write_archive(&app);
        cli(&dir, &["import", "swe-library", file.to_str().unwrap()]).unwrap();
        let coil = store::read_vendored_symbol(&dir, "swe.lib.coil").unwrap();
        assert!(coil.pin("X1").is_some(), "{:?}", coil.pins);
        assert_eq!(load(&dir).unwrap().lock.symbols["swe.lib.coil"].hash, coil.hash());

        std::fs::write(&file, "not an archive").unwrap();
        assert!(cli(&dir, &["import", "swe-library", file.to_str().unwrap()]).is_err());
    }

    #[test]
    fn a_dxf_in_an_archive_that_cannot_be_imported_leaves_no_blank_page() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let pages = load(&dir).unwrap().pages.len();
        use std::io::Write;
        let file = tmp.path().join("drawings.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
            let opt = zip::write::SimpleFileOptions::default();
            z.start_file("broken.dxf", opt).unwrap();
            z.write_all(b"this is not a drawing").unwrap();
            z.finish().unwrap();
        }
        // The drawing itself does not import, so the archive entry reaches that failure; the
        // archive import still succeeds and only warns about it.
        let broken = tmp.path().join("broken.dxf");
        std::fs::write(&broken, b"this is not a drawing").unwrap();
        let mut q = load(&dir).unwrap();
        q.pages.push(Page::new("probe", "999", "Probe"));
        save(&dir, &q).unwrap();
        assert!(
            cli(
                &dir,
                &["import", "page-dxf", broken.to_str().unwrap(), "--page", "probe"]
            )
            .is_err()
        );
        q.pages.retain(|pg| pg.id != "probe");
        save(&dir, &q).unwrap();
        cli(&dir, &["import", "archive", file.to_str().unwrap()]).unwrap();
        let p = load(&dir).unwrap();
        assert_eq!(
            p.pages.len(),
            pages,
            "{:?}",
            p.pages.iter().map(|p| &p.id).collect::<Vec<_>>()
        );
        assert!(p.pages.iter().all(|p| !p.id.starts_with("dxf-")));

        // A page an earlier import left under the id this one would use is never touched.
        let mut p = load(&dir).unwrap();
        let mut earlier = Page::new("dxf-300", "300", "Earlier drawing");
        earlier.title_block.insert("revision".into(), "B".into());
        p.pages.push(earlier.clone());
        save(&dir, &p).unwrap();
        cli(&dir, &["import", "archive", file.to_str().unwrap()]).unwrap();
        let p = load(&dir).unwrap();
        assert_eq!(p.page("dxf-300"), Some(&earlier));
        assert_eq!(p.pages.len(), pages + 1);

        // Nor one whose id differs only in case: its file may be the same file.
        let mut p = load(&dir).unwrap();
        p.pages.retain(|pg| pg.id != "dxf-300");
        // Saved apart: on a folder that ignores case, one save that drops `dxf-300` and adds
        // `DXF-300` would delete the file it just wrote.
        save(&dir, &p).unwrap();
        let mut upper = Page::new("DXF-300", "299", "Earlier drawing");
        upper.title_block.insert("revision".into(), "C".into());
        p.pages.push(upper.clone());
        save(&dir, &p).unwrap();
        cli(&dir, &["import", "archive", file.to_str().unwrap()]).unwrap();
        let mut p = load(&dir).unwrap();
        assert_eq!(p.page("DXF-300"), Some(&upper));
        assert!(p.page("dxf-300").is_none());
        p.pages.retain(|pg| pg.id != "DXF-300");
        save(&dir, &p).unwrap();

        // Every save fails after the page files are written (a panel file that has to be
        // rewritten cannot be): each drawing's page is still removed again, and every entry
        // is tried.
        p.panels
            .push(wc_core::panel::Panel::new("cab", "Cabinet", 600.0, 800.0));
        save(&dir, &p).unwrap();
        let panel = dir.join("panels").join("cab.panel.json");
        let text = std::fs::read_to_string(&panel).unwrap();
        std::fs::write(&panel, format!("{text}\n\n")).unwrap();
        std::fs::create_dir(dir.join("panels").join("cab.panel.json.tmp")).unwrap();
        let two = tmp.path().join("two.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&two).unwrap());
            let opt = zip::write::SimpleFileOptions::default();
            for name in ["a.dxf", "b.dxf"] {
                z.start_file(name, opt).unwrap();
                z.write_all(b"this is not a drawing").unwrap();
            }
            z.finish().unwrap();
        }
        cli(&dir, &["import", "archive", two.to_str().unwrap()]).unwrap();
        let p = load(&dir).unwrap();
        assert!(p.pages.iter().all(|p| !p.id.starts_with("dxf-")));
        let left: Vec<_> = std::fs::read_dir(dir.join("pages"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("dxf-"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
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
    fn excel_workbooks_import_parts_and_reports_save_as_xlsx() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        // A workbook is binary: it needs a file.
        assert!(cli(&dir, &["report", "all", "--format", "xlsx"]).is_err());
        let out = tmp.path().join("reports.xlsx");
        cli(
            &dir,
            &["report", "all", "--format", "xlsx", "--out", out.to_str().unwrap()],
        )
        .unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert!(wc_import::xlsx::is_xlsx(&bytes));
        // One sheet per report, the first one with its header row.
        assert!(!wc_import::xlsx::rows(&bytes).unwrap().is_empty());
        let z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let sheets = z.file_names().filter(|n| n.starts_with("xl/worksheets/")).count();
        assert_eq!(sheets, wc_report::Kind::ALL.len());

        let list = wc_report::Table {
            title: "Parts".into(),
            columns: vec!["Manufacturer".into(), "Order number".into(), "Description".into()],
            rows: vec![vec!["Acme".into(), "R-24".into(), "Relay".into()]],
            ..wc_report::Table::default()
        };
        let xlsx = list.to_xlsx().unwrap();
        let file = tmp.path().join("parts.xlsx");
        std::fs::write(&file, &xlsx).unwrap();
        let lib = tmp.path().join("lib");
        cli(
            &dir,
            &[
                "import",
                "parts-csv",
                file.to_str().unwrap(),
                "--lib",
                lib.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(std::fs::read_dir(lib.join("parts")).unwrap().count(), 1);

        // Inside an archive, an Excel list is told apart by its header like a CSV list.
        use std::io::Write;
        let zipped = tmp.path().join("vendor.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zipped).unwrap());
            z.start_file("lists/parts.xlsx", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(&xlsx).unwrap();
            z.finish().unwrap();
        }
        let lib2 = tmp.path().join("lib2");
        cli(
            &dir,
            &[
                "import",
                "archive",
                zipped.to_str().unwrap(),
                "--lib",
                lib2.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(std::fs::read_dir(lib2.join("parts")).unwrap().count(), 1);
    }

    #[test]
    fn a_black_box_is_drawn_wired_tagged_listed_and_reloaded() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["page", "add", "p1", "--number", "1"]).unwrap();
        let boxed = |args: &[&str]| {
            let mut all = vec!["box", "p1"];
            all.extend_from_slice(args);
            cli(&dir, &all)
        };
        boxed(&[
            "--at",
            "50,50",
            "--size",
            "40,20",
            "--tag",
            "-A1",
            "--pin",
            "1:left:5",
            "--pin",
            "2:right:15",
        ])
        .unwrap();
        boxed(&[
            "--at",
            "150,50",
            "--size",
            "20,20",
            "--tag",
            "-A2",
            "--pin",
            "X1:1:left:15",
        ])
        .unwrap();
        // Refused before anything is written: a point off its edge, a side that is not one.
        assert!(boxed(&["--at", "0,0", "--size", "10,10", "--pin", "1:left:11"]).is_err());
        assert!(boxed(&["--at", "0,0", "--size", "10,10", "--pin", "1:middle:1"]).is_err());
        assert!(boxed(&["--at", "0,0", "--size", "0,10"]).is_err());

        let p = load(&dir).unwrap();
        let page = p.page("p1").unwrap();
        assert_eq!(page.symbols.len(), 2);
        let (a1, a2) = (page.symbols[0].id.clone(), page.symbols[1].id.clone());
        assert_eq!(page.symbols[0].symbol, format!("box.p1.{a1}"));
        assert_eq!(p.lock.symbols[&page.symbols[0].symbol].source, "project");
        let symbol = store::read_vendored_symbol(&dir, &page.symbols[1].symbol).unwrap();
        assert_eq!(symbol.pins[0].name, "X1:1");
        assert_eq!(symbol.pins[0].at, Point::new(0.0, 15.0));

        // A box deleted from the page keeps its symbol file: its id is not taken over.
        let stale = wc_core::black_box("box.p1.s3", "old", 5.0, 5.0, &[]).unwrap();
        store::write_vendored_symbol(&dir, &stale).unwrap();
        boxed(&["--at", "100,100", "--size", "10,10"]).unwrap();
        let p = load(&dir).unwrap();
        let third = &p.page("p1").unwrap().symbols[2];
        assert_eq!((third.id.as_str(), third.symbol.as_str()), ("s4", "box.p1.s4"));
        assert_eq!(store::read_vendored_symbol(&dir, "box.p1.s3").unwrap(), stale);

        cli(&dir, &["wire", "p1", &format!("{a1}:2"), &format!("{a2}:X1:1")]).unwrap();
        let part = wc_core::Part {
            id: "acme.ctrl".into(),
            manufacturer: "Acme".into(),
            order_number: "CTRL-1".into(),
            description: "Controller".into(),
            ..wc_core::Part::default()
        };
        store::write_vendored_part(&dir, &part).unwrap();
        cli(&dir, &["device", "--part", "acme.ctrl", "--", "-A1"]).unwrap();

        let p = load(&dir).unwrap();
        let (symbols, missing) = store::vendored_symbols(&dir, &p);
        assert!(missing.is_empty(), "{missing:?}");
        let parts = vendored_parts(&dir, &p);
        let inputs = wc_report::Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let bom = wc_report::build(wc_report::Kind::Parts, &inputs).to_csv();
        assert!(bom.contains("CTRL-1"), "{bom}");
        let wires = wc_report::build(wc_report::Kind::Wires, &inputs).to_csv();
        assert!(wires.contains("-A1:2") && wires.contains("-A2:X1:1"), "{wires}");
        let devices = wc_report::build(wc_report::Kind::Devices, &inputs).to_csv();
        assert!(devices.contains("-A1") && devices.contains("-A2"), "{devices}");
        // Nothing in the project check about the boxes' symbols.
        let found = wc_report::check(&inputs);
        assert!(
            !found.iter().any(|f| f.message.contains("box.p1")),
            "{:?}",
            found.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_box_whose_save_fails_puts_the_project_back_and_goes_through_after() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["page", "add", "p1", "--number", "1"]).unwrap();
        let args = [
            "box",
            "p1",
            "--at",
            "10,10",
            "--size",
            "20,10",
            "--tag=-A1",
            "--pin",
            "1:left:5",
        ];
        let files = || {
            let mut out: Vec<(PathBuf, Vec<u8>)> = Vec::new();
            for folder in [dir.clone(), dir.join(store::PAGES_DIR)] {
                for entry in std::fs::read_dir(folder).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_file() {
                        out.push((path.clone(), std::fs::read(&path).unwrap()));
                    }
                }
            }
            out.sort();
            out
        };
        let before = files();
        // The page file cannot be written: the lock and devices are written before it.
        let mut blocker = dir
            .join(store::PAGES_DIR)
            .join(format!("p1{}", store::PAGE_SUFFIX))
            .into_os_string();
        blocker.push(".tmp");
        std::fs::create_dir_all(&blocker).unwrap();
        assert!(cli(&dir, &args).is_err());
        std::fs::remove_dir(&blocker).unwrap();
        assert_eq!(files(), before, "project files put back");
        assert!(
            store::read_vendored_symbol(&dir, "box.p1.s1").is_err(),
            "symbol file removed"
        );
        cli(&dir, &args).unwrap();
        let p = load(&dir).unwrap();
        assert_eq!(p.page("p1").unwrap().symbols[0].symbol, "box.p1.s1");
        assert!(p.lock.symbols.contains_key("box.p1.s1"));
        assert!(store::read_vendored_symbol(&dir, "box.p1.s1").is_ok());
    }

    #[test]
    fn a_page_id_too_long_for_a_box_symbol_is_refused_before_anything_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        // `box.` + page + `.s1`: 121 characters fit in 128, 122 do not.
        for (len, fits) in [(121, true), (122, false)] {
            let page = "p".repeat(len);
            cli(&dir, &["page", "add", &page, "--number", &len.to_string()]).unwrap();
            let lock = std::fs::read(dir.join(store::LOCK_FILE)).ok();
            let r = cli(&dir, &["box", &page, "--at", "0,0", "--size", "10,10"]);
            assert_eq!(r.is_ok(), fits, "{len}: {r:?}");
            if !fits {
                assert!(r.unwrap_err().contains("too long"));
                assert_eq!(std::fs::read(dir.join(store::LOCK_FILE)).ok(), lock);
                assert!(load(&dir).unwrap().page(&page).unwrap().symbols.is_empty());
            }
        }
    }

    #[test]
    fn an_edz_symbol_that_cannot_be_written_is_a_warning_and_its_part_is_written_without_it() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("contactor.edz");
        let xml = r#"<partsmanagement><part P_ARTICLE_PARTNR="SIE.3RT2015" P_ARTICLE_MANUFACTURER="SIE" P_ARTICLE_MACRO="SIE\SIE_3RT2015.ema"/></partsmanagement>"#;
        let ema = r#"<EplanPxfRoot><MacroVariant MacroFuncType="1"><O31 A531="0/0" A532="1/1"/></MacroVariant></EplanPxfRoot>"#;
        let mut w = sevenz_rust2::ArchiveWriter::new(std::io::Cursor::new(Vec::new())).unwrap();
        w.push_archive_entry(sevenz_rust2::ArchiveEntry::new_file("parts.xml"), Some(xml.as_bytes()))
            .unwrap();
        w.push_archive_entry(
            sevenz_rust2::ArchiveEntry::new_file("SIE_3RT2015.ema"),
            Some(ema.as_bytes()),
        )
        .unwrap();
        std::fs::write(&file, w.finish().unwrap().into_inner()).unwrap();
        let lib = tmp.path().join("lib");
        // A folder where the symbol's file would go makes writing it fail.
        std::fs::create_dir_all(lib.join("symbols").join("eplan.sie_3rt2015.json")).unwrap();
        let (n, report, warnings) = import_edz(&file, &lib, None).unwrap();
        assert_eq!(n, 1);
        assert!(
            warnings
                .iter()
                .any(|w| w.starts_with("symbol eplan.sie_3rt2015 not written")),
            "{warnings:?}"
        );
        assert_eq!(report.part_macros[0].symbol, None);
        let json = std::fs::read_to_string(lib.join("parts").join("sie.3rt2015.json")).unwrap();
        let p: wc_core::Part = serde_json::from_str(&json).unwrap();
        assert_eq!(p.symbol, None);

        // Once the symbol can be written, the part points at it and it loads back.
        std::fs::remove_dir(lib.join("symbols").join("eplan.sie_3rt2015.json")).unwrap();
        let (_, _, warnings) = import_edz(&file, &lib, None).unwrap();
        assert!(warnings.iter().all(|w| !w.contains("not written")), "{warnings:?}");
        let json = std::fs::read_to_string(lib.join("parts").join("sie.3rt2015.json")).unwrap();
        let p: wc_core::Part = serde_json::from_str(&json).unwrap();
        let id = p.symbol.unwrap();
        let json = std::fs::read_to_string(lib.join("symbols").join(format!("{id}.json"))).unwrap();
        let mut s: wc_core::Symbol = serde_json::from_str(&json).unwrap();
        assert_eq!(s.id, id);

        // The library now holds a different, readable symbol of that id, and the new one cannot
        // replace it (a folder blocks its staging file).
        s.description = "kept from before".into();
        wc_library::write_symbol(&lib, &s).unwrap();
        std::fs::create_dir_all(lib.join("symbols").join(format!("{id}.json.tmp"))).unwrap();
        let part_symbol = |lib: &Path| {
            let json = std::fs::read_to_string(lib.join("parts").join("sie.3rt2015.json")).unwrap();
            serde_json::from_str::<wc_core::Part>(&json).unwrap().symbol
        };
        // The macro's own link is dropped: its symbol was not saved.
        let (_, _, warnings) = import_edz(&file, &lib, None).unwrap();
        assert!(warnings.iter().any(|w| w.contains("not written")), "{warnings:?}");
        assert_eq!(part_symbol(&lib), None);
        // A link the symbol map chose to that readable symbol is kept.
        let map = tmp.path().join("map.txt");
        std::fs::write(&map, format!("SIE_3RT2015 = {id}\n")).unwrap();
        let (_, report, _) = import_edz(&file, &lib, Some(&map)).unwrap();
        assert_eq!(part_symbol(&lib).as_deref(), Some(id.as_str()));
        assert_eq!(report.part_macros[0].symbol.as_deref(), Some(id.as_str()));
    }

    #[test]
    fn edz_parts_land_in_the_library_and_macros_are_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("contactor.edz");
        let xml = r#"<partsmanagement><part P_ARTICLE_PARTNR="SIE.3RT2015" P_ARTICLE_MANUFACTURER="SIE" P_ARTICLE_DESCR1="de_DE@Schütz;en_US@Contactor;" P_ARTICLE_MACRO="SIE\SIE_3RT2015.ema"/></partsmanagement>"#;
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
        let (n, report, warnings) = import_edz(&file, &lib, None).unwrap();
        assert_eq!(n, 1, "{warnings:?}");
        assert_eq!(report.macros, 1);
        let json = std::fs::read_to_string(lib.join("parts").join("sie.3rt2015.json")).unwrap();
        let p: wc_core::Part = serde_json::from_str(&json).unwrap();
        assert_eq!(p.description, "Contactor");
        assert_eq!(p.symbol, None);

        // With a symbol map, the part's macro picks its library symbol.
        let map = tmp.path().join("macros.txt");
        std::fs::write(&map, "SIE_3RT2015 = wc.coil\n").unwrap();
        let (_, report, _) = import_edz(&file, &lib, Some(&map)).unwrap();
        assert_eq!(report.part_macros.len(), 1);
        assert!(report.part_macros[0].in_archive);
        let json = std::fs::read_to_string(lib.join("parts").join("sie.3rt2015.json")).unwrap();
        let p: wc_core::Part = serde_json::from_str(&json).unwrap();
        assert_eq!(p.symbol.as_deref(), Some("wc.coil"));
        // A bad map is refused before anything is written: the library keeps what it had, and a
        // new library is not created.
        std::fs::write(&map, "no equals sign\n").unwrap();
        let part_file = lib.join("parts").join("sie.3rt2015.json");
        let before = std::fs::read(&part_file).unwrap();
        assert!(import_edz(&file, &lib, Some(&map)).unwrap_err().contains("line 1"));
        assert_eq!(std::fs::read(&part_file).unwrap(), before);
        let fresh = tmp.path().join("fresh");
        assert!(import_edz(&file, &fresh, Some(&map)).is_err());
        assert!(!fresh.exists());

        std::fs::write(&file, "not an archive").unwrap();
        assert!(import_edz(&file, &lib, None).is_err());
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
            Cmd::Export(ExportCmd::WagoMarking { ref out, ref args })
                if out == &PathBuf::from("out.csv") && args.kinds == ["terminals", "wires", "cables"]
        ));

        let cli = Cli::try_parse_from(["wiringcraft", "export", "wago-marking", "out.csv"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Export(ExportCmd::WagoMarking { ref args, .. })
                if args.kinds.is_empty() && args.lines.is_empty() && args.lang == "en" && args.strip.is_none()
        ));
        let cli = Cli::try_parse_from([
            "wiringcraft",
            "export",
            "wago-marking",
            "out.csv",
            "--line",
            "marker,description",
            "--lang",
            "de",
            "--strip",
            "-X1",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Export(ExportCmd::WagoMarking { ref args, .. })
                if args.lines == ["marker", "description"] && args.lang == "de" && args.strip.as_deref() == Some("-X1")
        ));
        assert!(Cli::try_parse_from(["wiringcraft", "export", "wago-marking"]).is_err());
    }

    fn kinds(k: &[&str]) -> MarkingArgs {
        MarkingArgs {
            kinds: k.iter().map(|s| (*s).to_owned()).collect(),
            lang: "en".into(),
            ..MarkingArgs::default()
        }
    }

    /// A project in `tmp` with terminals -X1.1 and -X1.2, a folder library holding `parts` and a
    /// second library that cannot be opened, and its configurator list exported: (project, list).
    fn wago_project(tmp: &Path, parts: &[wc_core::Part]) -> (PathBuf, PathBuf) {
        let dir = tmp.join("p");
        let mut p = store::create(&dir, "Test").unwrap();
        // A folder library with the terminal the configurator will choose.
        let lib = tmp.join("lib");
        for part in parts {
            wc_library::write_part(&lib, part).unwrap();
        }
        p.meta.libraries.push(LibrarySource {
            name: "wago".into(),
            kind: "folder".into(),
            location: lib.to_string_lossy().into_owned(),
            reference: None,
        });
        // A second library that cannot be opened does not stop the first one's parts.
        p.meta.libraries.push(LibrarySource {
            name: "broken".into(),
            kind: "nowhere".into(),
            location: String::new(),
            reference: None,
        });
        let term = wc_core::Symbol {
            source: String::new(),
            id: "test.term".into(),
            name: "Terminal".into(),
            category: String::new(),
            letter_code: "X".into(),
            role: wc_core::symbol::SymbolRole::Terminal,
            graphics: vec![],
            pins: ["1", "2"]
                .iter()
                .enumerate()
                .map(|(k, n)| wc_core::symbol::Pin {
                    name: (*n).into(),
                    at: wc_core::geom::Point::new(0.0, 10.0 * k as f64),
                    dir: wc_core::symbol::PinDir::Down,
                })
                .collect(),
            tag_at: Default::default(),
            description: String::new(),
        };
        store::write_vendored_symbol(&dir, &term).unwrap();
        for n in ["1", "2"] {
            p.pages[0].symbols.push(wc_core::PlacedSymbol {
                id: format!("s{n}"),
                symbol: "test.term".into(),
                at: wc_core::geom::Point::new(50.0, 50.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(wc_core::DeviceTag::parse(&format!("-X1.{n}")).unwrap()),
            });
        }
        store::save(&dir, &p).unwrap();
        let out = tmp.join("c.csv");
        export_wago_configurator(&dir, &out).unwrap();
        (dir, out)
    }

    fn part(id: &str, maker: &str, number: &str) -> wc_core::Part {
        wc_core::Part {
            id: id.into(),
            manufacturer: maker.into(),
            order_number: number.into(),
            ..Default::default()
        }
    }

    /// Fill in the articles of terminals -X1.1 and -X1.2 in the list at `out`.
    fn fill_wago_list(out: &Path, first: &str, second: &str) {
        let csv = std::fs::read_to_string(out)
            .unwrap()
            .replace(
                "-X1/T:1;Terminal;-X1;1;1;;;",
                &format!("-X1/T:1;Terminal;-X1;1;1;;{first};"),
            )
            .replace(
                "-X1/T:2;Terminal;-X1;2;2;;;",
                &format!("-X1/T:2;Terminal;-X1;2;2;;{second};"),
            );
        std::fs::write(out, csv).unwrap();
    }

    #[test]
    fn wago_configurator_list_comes_back_onto_the_devices() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, out) = wago_project(tmp.path(), &[part("wago.2002-1201", "WAGO", "2002-1201")]);
        // The configurator fills in the articles: one it knows, one no library has.
        fill_wago_list(&out, "2002-1201", "2002-9999");
        let done = run_import_wago(&dir, &out);
        assert_eq!((done.changed, done.unchanged), (1, 0), "{:?}", done.unmatched);
        assert_eq!(done.unmatched.len(), 1);
        assert!(done.unmatched[0].contains("2002-9999"), "{:?}", done.unmatched);
        let p = load(&dir).unwrap();
        let x11 = wc_core::DeviceTag::parse("-X1.1").unwrap();
        assert_eq!(
            p.devices.get(&x11).and_then(|d| d.part.as_deref()),
            Some("wago.2002-1201")
        );
        assert!(p.lock.parts.contains_key("wago.2002-1201"), "taken into the project");
        assert!(store::read_vendored_part(&dir, "wago.2002-1201").is_ok());
        // Again: nothing to change.
        let again = run_import_wago(&dir, &out);
        assert_eq!((again.changed, again.unchanged), (0, 1));
        assert!(import_wago(&dir, &tmp.path().join("missing.csv")).is_err());
    }

    #[test]
    fn a_wago_import_whose_lock_write_fails_pins_the_part_on_the_next_try() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, out) = wago_project(tmp.path(), &[part("wago.2002-1201", "WAGO", "2002-1201")]);
        fill_wago_list(&out, "2002-1201", "");
        let names = [store::PROJECT_FILE, store::DEVICES_FILE, store::LOCK_FILE];
        let files = |d: &Path| names.map(|f| std::fs::read(d.join(f)).ok());
        let times = |d: &Path| names.map(|f| std::fs::metadata(d.join(f)).and_then(|m| m.modified()).ok());
        let before = files(&dir);
        // The lock's write goes through library.lock.tmp: a folder there makes it fail.
        let blocker = dir.join(format!("{}.tmp", store::LOCK_FILE));
        std::fs::create_dir(&blocker).unwrap();
        let err = import_wago(&dir, &out).err().unwrap_or_default();
        assert!(err.contains("library.lock"), "{err}");
        assert_eq!(files(&dir), before, "the assignment is not written before its pin");
        std::fs::remove_dir(&blocker).unwrap();
        let done = run_import_wago(&dir, &out);
        assert_eq!(done.changed, 1, "{:?}", done.unmatched);
        let p = load(&dir).unwrap();
        let x11 = wc_core::DeviceTag::parse("-X1.1").unwrap();
        assert_eq!(
            p.devices.get(&x11).and_then(|d| d.part.as_deref()),
            Some("wago.2002-1201")
        );
        assert!(p.lock.parts.contains_key("wago.2002-1201"), "pinned");
        // A repeat import of the pinned part writes nothing.
        let saved = times(&dir);
        let again = run_import_wago(&dir, &out);
        assert_eq!((again.changed, again.unchanged), (0, 1));
        assert_eq!(times(&dir), saved, "no file written again");

        // A project left by the old order (assigned, pin missing) is repaired by a re-import,
        // pinning its own copy as it is although the library has another version by now, and
        // writing nothing but the lock.
        let mut p = load(&dir).unwrap();
        p.lock.parts.remove("wago.2002-1201");
        save(&dir, &p).unwrap();
        let mut newer = part("wago.2002-1201", "WAGO", "2002-1201");
        newer.description = "Changed in the library".into();
        wc_library::write_part(&tmp.path().join("lib"), &newer).unwrap();
        let copy_file = store::vendor_path(&dir, ItemKind::Part, "wago.2002-1201").unwrap();
        let copy = std::fs::read(&copy_file).unwrap();
        // A full save would rewrite devices.json written by hand like this, and its write fails.
        let devices_file = dir.join(store::DEVICES_FILE);
        let by_hand = format!("{}\n\n", std::fs::read_to_string(&devices_file).unwrap().trim_end());
        std::fs::write(&devices_file, &by_hand).unwrap();
        let devices_tmp = dir.join(format!("{}.tmp", store::DEVICES_FILE));
        std::fs::create_dir(&devices_tmp).unwrap();
        let devices = std::fs::metadata(&devices_file).unwrap().modified().unwrap();
        let again = run_import_wago(&dir, &out);
        assert_eq!(std::fs::read_to_string(&devices_file).unwrap(), by_hand);
        std::fs::remove_dir(&devices_tmp).unwrap();
        assert_eq!(again.changed, 0, "{:?}", again.unmatched);
        assert_eq!(std::fs::read(&copy_file).unwrap(), copy, "the copy is kept");
        assert_eq!(
            load(&dir)
                .unwrap()
                .lock
                .parts
                .get("wago.2002-1201")
                .map(|e| e.hash.clone()),
            Some(store::read_vendored_part(&dir, "wago.2002-1201").unwrap().hash()),
            "pinned again, to its own copy"
        );
        assert_ne!(
            store::read_vendored_part(&dir, "wago.2002-1201").unwrap().hash(),
            newer.hash()
        );
        assert_eq!(
            std::fs::metadata(dir.join(store::DEVICES_FILE))
                .unwrap()
                .modified()
                .unwrap(),
            devices,
            "only the lock is written"
        );
    }

    #[test]
    fn a_wago_article_several_parts_have_stays_unmatched_after_one_of_them_is_taken() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, out) = wago_project(tmp.path(), &[part("dup.a", "", "DUP-1"), part("dup.b", "", "DUP-1")]);
        fill_wago_list(&out, "dup.a", "DUP-1");
        let first = run_import_wago(&dir, &out);
        assert_eq!(first.changed, 1, "{:?}", first.unmatched);
        assert!(load(&dir).unwrap().lock.parts.contains_key("dup.a"), "taken");
        assert_eq!(first.unmatched.len(), 1);
        assert!(first.unmatched[0].contains("several parts"), "{:?}", first.unmatched);
        // Read again with dup.a in the project: the same answer.
        let again = run_import_wago(&dir, &out);
        assert_eq!((again.changed, again.unchanged), (0, 1), "{:?}", again.unmatched);
        assert_eq!(again.unmatched, first.unmatched);
    }

    #[test]
    fn a_used_part_whose_copy_cannot_be_read_is_not_pinned_or_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, out) = wago_project(tmp.path(), &[part("wago.2002-1201", "WAGO", "2002-1201")]);
        fill_wago_list(&out, "2002-1201", "");
        assert_eq!(run_import_wago(&dir, &out).changed, 1);
        let mut p = load(&dir).unwrap();
        p.lock.parts.remove("wago.2002-1201");
        save(&dir, &p).unwrap();
        let copy_file = store::vendor_path(&dir, ItemKind::Part, "wago.2002-1201").unwrap();
        std::fs::write(&copy_file, "{ not json").unwrap();
        let done = run_import_wago(&dir, &out);
        assert_eq!(
            std::fs::read_to_string(&copy_file).unwrap(),
            "{ not json",
            "the file stays"
        );
        assert!(!load(&dir).unwrap().lock.parts.contains_key("wago.2002-1201"), "no pin");
        assert!(
            done.warnings
                .iter()
                .any(|w| w.contains("wago.2002-1201") && w.contains("cannot be read") && w.contains("line 1")),
            "{:?}",
            done.warnings
        );

        // A link to nothing is there, not absent: the library part does not replace it.
        #[cfg(unix)]
        {
            std::fs::remove_file(&copy_file).unwrap();
            std::os::unix::fs::symlink(tmp.path().join("gone.json"), &copy_file).unwrap();
            let done = run_import_wago(&dir, &out);
            assert!(std::fs::symlink_metadata(&copy_file).unwrap().file_type().is_symlink());
            assert!(!load(&dir).unwrap().lock.parts.contains_key("wago.2002-1201"));
            assert!(
                done.warnings
                    .iter()
                    .any(|w| w.contains("wago.2002-1201") && w.contains("cannot be read")),
                "{:?}",
                done.warnings
            );
        }
    }

    #[test]
    fn a_copy_no_library_has_and_nothing_uses_does_not_hide_a_library_part() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, out) = wago_project(tmp.path(), &[part("current.b", "WAGO", "2002-1201")]);
        store::write_vendored_part(&dir, &part("old.a", "WAGO", "2002-1201")).unwrap();
        fill_wago_list(&out, "2002-1201", "");
        let done = run_import_wago(&dir, &out);
        assert_eq!(done.changed, 1, "{:?}", done.unmatched);
        let p = load(&dir).unwrap();
        let x11 = wc_core::DeviceTag::parse("-X1.1").unwrap();
        assert_eq!(p.devices.get(&x11).and_then(|d| d.part.as_deref()), Some("current.b"));
    }

    fn run_import_wago(dir: &Path, file: &Path) -> WagoImported {
        let cli = Cli::try_parse_from(["wiringcraft", "import", "wago", file.to_str().unwrap()]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Import(ImportCmd::Wago { .. })));
        import_wago(dir, file).unwrap()
    }

    #[test]
    fn wago_configurator_export_writes_one_row_per_terminal_and_jumper() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        let mut p = store::create(&dir, "Test").unwrap();
        let out = tmp.path().join("empty.csv");
        let done = export_wago_configurator(&dir, &out).unwrap();
        assert_eq!(done.markers, 0);
        assert!(
            done.warnings.iter().any(|w| w.contains("no numbered terminals")),
            "{:?}",
            done.warnings
        );

        let x1 = wc_core::DeviceTag::parse("-X1").unwrap();
        let term = wc_core::Symbol {
            source: String::new(),
            id: "test.term".into(),
            name: "Terminal".into(),
            category: String::new(),
            letter_code: "X".into(),
            role: wc_core::symbol::SymbolRole::Terminal,
            graphics: vec![],
            pins: ["1", "2"]
                .iter()
                .enumerate()
                .map(|(k, n)| wc_core::symbol::Pin {
                    name: (*n).into(),
                    at: wc_core::geom::Point::new(0.0, 10.0 * k as f64),
                    dir: wc_core::symbol::PinDir::Down,
                })
                .collect(),
            tag_at: Default::default(),
            description: String::new(),
        };
        store::write_vendored_symbol(&dir, &term).unwrap();
        for (n, id, symbol) in [
            ("1", "s1", "test.term"),
            ("2", "s2", "test.term"),
            ("3", "s3", "no.such.symbol"),
        ] {
            p.pages[0].symbols.push(wc_core::PlacedSymbol {
                id: id.into(),
                symbol: symbol.into(),
                at: wc_core::geom::Point::new(50.0, 50.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(wc_core::DeviceTag::parse(&format!("-X1.{n}")).unwrap()),
            });
        }
        p.meta.add_jumper(&x1, &["1".to_owned(), "2".to_owned()]).unwrap();
        store::save(&dir, &p).unwrap();
        let out = tmp.path().join("c.csv");
        let done = export_wago_configurator(&dir, &out).unwrap();
        // -X1.3 has no vendored symbol: its terminal level makes it a terminal, as in the strip
        // editor and jumpers, so it is on the rail (with a warning that its wiring is not known).
        assert_eq!(done.markers, 4, "{:?}", std::fs::read_to_string(&out));
        assert!(
            std::fs::read_to_string(&out)
                .unwrap()
                .contains("\r\n-X1/T:3;Terminal;-X1;3;3;"),
            "{:?}",
            std::fs::read_to_string(&out)
        );
        let csv = std::fs::read_to_string(&out).unwrap();
        assert!(csv.starts_with("\u{feff}Index;Type;Strip;Position;Terminal;"), "{csv}");
        assert!(csv.contains("\r\n-X1/T:2;Terminal;-X1;2;2;"), "{csv}");
        assert!(csv.contains("\r\n-X1/J1;Jumper;-X1;"), "{csv}");
        assert!(done.warnings.iter().any(|w| w.contains("no.such.symbol")));
        // Its terminal is listed now, so no warning may say it is left out.
        assert!(
            !done.warnings.iter().any(|w| w.contains("not in the list")),
            "{:?}",
            done.warnings
        );

        // A part that is used but cannot be read is named, with the reason.
        p.devices.insert(
            wc_core::DeviceTag::parse("-X1.1").unwrap(),
            wc_core::Device {
                part: Some("broken.part".into()),
                ..Default::default()
            },
        );
        store::save(&dir, &p).unwrap();
        let path = store::vendor_path(&dir, wc_core::lock::ItemKind::Part, "broken.part").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        let done = export_wago_configurator(&dir, &out).unwrap();
        let w = done.warnings.iter().find(|w| w.contains("broken.part"));
        assert!(
            w.is_some_and(|w| w.contains("could not be read (")),
            "{:?}",
            done.warnings
        );

        // The same for a jumper's part.
        p.meta.set_jumper_part(&x1, "1", Some("broken.jumper")).unwrap();
        store::save(&dir, &p).unwrap();
        let path = store::vendor_path(&dir, wc_core::lock::ItemKind::Part, "broken.jumper").unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        let done = export_wago_configurator(&dir, &out).unwrap();
        assert!(
            done.warnings
                .iter()
                .any(|w| w.starts_with("part broken.jumper could not be read (")),
            "{:?}",
            done.warnings
        );

        // An accessory with a readable vendored part gives its article, not the part id.
        let stop = wc_core::Part {
            id: "wago.2002-1291".into(),
            manufacturer: "WAGO".into(),
            order_number: "2002-1291".into(),
            description: "End plate".into(),
            ..Default::default()
        };
        store::write_vendored_part(&dir, &stop).unwrap();
        p.devices.insert(
            x1.clone(),
            wc_core::Device {
                accessories: vec![wc_core::DevicePart {
                    part: stop.id.clone(),
                    qty: 1,
                }],
                ..Default::default()
            },
        );
        store::save(&dir, &p).unwrap();
        export_wago_configurator(&dir, &out).unwrap();
        let csv = std::fs::read_to_string(&out).unwrap();
        assert!(csv.contains(";2002-1291;WAGO;End plate;"), "{csv}");
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
        let err = export_wago_marking(&dir, &out, &kinds(&["rails"])).err().unwrap();
        assert!(err.contains("rails"), "{err}");
        assert!(!out.exists(), "nothing written for an unknown kind");

        let out = tmp.path().join("m.csv");
        let done = export_wago_marking(&dir, &out, &kinds(&["cables", "devices"])).unwrap();
        // -X1.1 has no vendored symbol, but its terminal level makes it a terminal (as in the
        // terminal plan), so it gets a terminal marker and no device marker.
        assert_eq!(done.markers, 1);
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "\u{feff}Type;Group;Marker;Count\r\nCable;;-W1;2\r\n"
        );
        assert!(
            done.warnings.iter().any(|w| w.contains("no.such.symbol")),
            "{:?}",
            done.warnings
        );
        assert!(!done.warnings.iter().any(|w| w.contains("formula")), "tags give none");

        // A wire numbered like a formula is written as it is, with a warning.
        let wire: wc_core::Wire = serde_json::from_str(
            r#"{"id":"w1","from":{"at":{"x":1.0,"y":2.0}},"to":{"at":{"x":9.0,"y":2.0}},"number":"=1"}"#,
        )
        .unwrap();
        p.pages[0].wires.push(wire);
        store::save(&dir, &p).unwrap();
        let done = export_wago_marking(&dir, &out, &kinds(&["wires"])).unwrap();
        assert!(std::fs::read_to_string(&out).unwrap().contains("Wire;;=1;2"));
        let formula: Vec<&String> = done.warnings.iter().filter(|w| w.contains("formula")).collect();
        assert_eq!(formula.len(), 1, "{:?}", done.warnings);
        assert!(formula[0].contains("\"=1\""));
    }

    #[test]
    fn wago_marking_export_takes_lines_language_and_one_strip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        let mut p = store::create(&dir, "Test").unwrap();
        let w1 = wc_core::DeviceTag::parse("-W1").unwrap();
        let dev = p.devices.entry(w1).or_default();
        dev.description = "Motor cable".into();
        dev.properties
            .insert(wc_report::MARKER_PROPERTY.into(), "M1 FEED".into());
        store::save(&dir, &p).unwrap();
        let out = tmp.path().join("m.csv");

        let args = MarkingArgs {
            kinds: vec!["cables".into()],
            lines: vec!["marker".into(), "tag".into(), "description".into()],
            lang: "de".into(),
            strip: None,
            level_separator: None,
        };
        let done = export_wago_marking(&dir, &out, &args).unwrap();
        assert_eq!(done.markers, 1);
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "\u{feff}Typ;Gruppe;Zeile 1;Zeile 2;Zeile 3;Anzahl\r\nKabel;;M1 FEED;-W1;Motor cable;2\r\n"
        );

        // Unknown options and strips are refused before anything is written.
        let bad = tmp.path().join("bad.csv");
        for (args, word) in [
            (
                MarkingArgs {
                    lang: "fr".into(),
                    ..kinds(&[])
                },
                "fr",
            ),
            (
                MarkingArgs {
                    lines: vec!["colour".into()],
                    ..kinds(&[])
                },
                "colour",
            ),
            (
                MarkingArgs {
                    strip: Some("-X9".into()),
                    ..kinds(&[])
                },
                "-X9",
            ),
        ] {
            let err = export_wago_marking(&dir, &bad, &args).err().unwrap();
            assert!(err.contains(word), "{err}");
            assert!(!bad.exists());
        }
    }
}
