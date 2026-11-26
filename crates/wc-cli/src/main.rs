//! `wiringcraft`: every project operation as a command, for scripts, agents and the app.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use wc_core::geom::{Point, Rotation};
use wc_core::lock::ItemKind;
use wc_core::project::LibrarySource;
use wc_core::{DeviceTag, Endpoint, Page, PageSize, PlacedSymbol, Project, Wire, store};
use wc_import::run::{
    self, escape_field, kicad_prefix, load, page_mut, put_file_back, put_files_back, read_file, read_kicad, save,
    snapshot_project, vendored_bytes, write_kicad,
};
#[cfg(test)]
use wc_import::run::{
    Replace, add_and_save, add_built, add_swe_built, import_bmecat, import_edz, keep_library_sources, keep_one_cover,
    project_copy, save_imported, sheet_for, swe_symbol_choice,
};
// Only the Unix tests run a converter through a shell command.
#[cfg(all(test, unix))]
use wc_import::run::{dwg_to_dxf, staging_dir};

type Res<T> = Result<T, String>;

/// Run an import with a [`run::Log`] that prints each line as it is added: result lines on
/// standard output, warnings on standard error, so what an archive import has done shows while
/// it goes on to its next entry.
fn logged(f: impl FnOnce(&mut run::Log) -> Res<()>) -> Res<()> {
    logged_to(std::io::stdout(), std::io::stderr(), f)
}

/// [`logged`] onto `out` and `err`. A line that cannot be printed (a closed pipe) does not stop
/// the import or hide its result: the import goes on, and the failure is reported once on the
/// other stream, apart from the import's own result.
fn logged_to<O, E>(out: O, err: E, f: impl FnOnce(&mut run::Log) -> Res<()>) -> Res<()>
where
    O: std::io::Write + Send + 'static,
    E: std::io::Write + Send + 'static,
{
    use std::sync::{Arc, Mutex};
    type Streams<O, E> = (O, E, Option<(bool, std::io::Error)>);
    let streams: Arc<Mutex<Streams<O, E>>> = Arc::new(Mutex::new((out, err, None)));
    let shared = Arc::clone(&streams);
    let mut log = run::Log::echoing(move |m| {
        let Ok(mut s) = shared.lock() else { return };
        let (out, err, failed) = &mut *s;
        let (on_out, r) = match m {
            run::Msg::Out(line) => (true, writeln!(out, "{line}").and_then(|()| out.flush())),
            run::Msg::Err(line) => (false, writeln!(err, "{line}").and_then(|()| err.flush())),
        };
        if let Err(e) = r
            && failed.is_none()
        {
            *failed = Some((on_out, e));
        }
    });
    let r = f(&mut log);
    drop(log);
    if let Ok(mut s) = streams.lock() {
        let (out, err, failed) = &mut *s;
        if let Some((on_out, e)) = failed.take() {
            let (name, other): (&str, &mut dyn std::io::Write) = if on_out {
                ("standard output", err)
            } else {
                ("standard error", out)
            };
            let done = if r.is_ok() {
                "the import was done"
            } else {
                "the import failed"
            };
            // Nothing is left to report this to when the other stream fails as well.
            let _ = writeln!(
                other,
                "warning: the import's report could not be printed on {name}: {e}; {done}"
            );
        }
    }
    r
}

/// `import page-dwg`: [`run::import_page_dwg`], printing what it said.
fn import_page_dwg(dir: &Path, file: &Path, page: String, scale: f64, template: Option<String>) -> Res<()> {
    logged(|log| run::import_page_dwg(dir, file, &page, scale, template, log))
}

// Every `print!` and `println!` below writes through these, not the standard ones: those panic
// when the reader of a pipe stops early (`wiringcraft ... | more`), which on Windows, with no
// SIGPIPE to end the program, reported a crash (#567). These keep the first error, drop later
// output, and let the command run to its end, so its changes are still saved (#550).
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::write_stdout(|out| std::io::Write::write_fmt(out, format_args!($($arg)*)))
    };
}
macro_rules! println {
    () => {
        $crate::write_stdout(|out| std::io::Write::write_all(out, b"\n"))
    };
    ($($arg:tt)*) => {
        $crate::write_stdout(|out| {
            std::io::Write::write_fmt(out, format_args!("{}\n", format_args!($($arg)*)))
        })
    };
}

thread_local! {
    /// The first error writing to stdout on this thread, the command's; output after it is
    /// dropped, and [`with_stdout_error`] reports it once the command has ended.
    static STDOUT_ERROR: std::cell::RefCell<Option<std::io::Error>> =
        const { std::cell::RefCell::new(None) };
}

/// Write to stdout with `write`, unless an earlier write failed.
fn write_stdout(write: impl FnOnce(&mut std::io::StdoutLock<'static>) -> std::io::Result<()>) {
    if STDOUT_ERROR.with_borrow(Option::is_some) {
        return;
    }
    if let Err(err) = write(&mut std::io::stdout().lock()) {
        STDOUT_ERROR.set(Some(err));
    }
}

/// The command's `result`, with what went wrong writing its output: nothing when the reader of
/// a pipe went away early (`| more`, `| head -1`), as other command-line tools do.
fn with_stdout_error(result: Res<()>) -> Res<()> {
    let failed = STDOUT_ERROR.take();
    result?;
    match failed {
        Some(err) if err.kind() != std::io::ErrorKind::BrokenPipe => Err(format!("could not write the output: {err}")),
        _ => Ok(()),
    }
}

#[derive(Parser)]
#[command(name = "wiringcraft", version, about = "Electrical schematic and wiring design")]
struct Cli {
    /// Project folder. After a subcommand that takes a device tag, write it as `-C <dir>` or
    /// `--project <dir>`: an attached `-C<dir>` there is read as a tag, since tags such as
    /// `-C1` start with a dash too.
    #[arg(short = 'C', long = "project", global = true, default_value = ".")]
    project: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new project folder (also a git repository).
    New {
        /// The folder to create the project in; it must not hold a project already.
        dir: PathBuf,
        /// The project's name (default: the folder's name).
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
        /// The page's id.
        page: String,
        /// The library symbol's id, e.g. `iec.contactor.coil`.
        symbol: String,
        /// Position in mm, `x,y`.
        #[arg(long, value_parser = parse_point)]
        at: Point,
        /// Device tag, e.g. `-K1` (taken as a value, though it starts with `-`).
        #[arg(long, allow_hyphen_values = true)]
        tag: Option<String>,
        /// 0, 90, 180 or 270.
        #[arg(long, default_value_t = 0)]
        rotate: i64,
        /// Mirror the symbol left to right (before turning it).
        #[arg(long)]
        mirror: bool,
    },
    /// Draw a black box (device box) for a device without a symbol: a rectangle with its
    /// top-left corner at `--at`, and connection points on its edges that wires attach to like
    /// symbol pins. It is stored as a symbol of the project (`box.<page>.<id>`), so it carries a
    /// device tag and part and shows in the reports and checks like any symbol. Prints its id.
    Box {
        /// The page's id.
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
    /// Change a black box drawn with `box`: its size and its connection points. Sizes, sides
    /// and offsets are in the box's own frame, and the corner that stays put is its origin (its
    /// own top-left; the page top-left only when the box is not turned or mirrored). `--pin` given replaces every point (an old point not named again is
    /// taken off, and a wire on it keeps a free end where it was); left out, the points stay,
    /// pulled onto the edges of the new size. `--rename OLD=NEW` keeps a point under a new name
    /// and its wires follow. Every box placing the same symbol changes. Prints what to check.
    BoxEdit {
        /// The page's id.
        page: String,
        /// The box's item id, e.g. `s3`.
        id: String,
        /// New width and height in mm, `w,h`.
        #[arg(long, value_parser = parse_point)]
        size: Option<Point>,
        /// A connection point, `name:side:offset` as for `box`. Repeat for each point.
        #[arg(long = "pin")]
        pins: Vec<String>,
        /// Take every connection point off (wires on them keep a free end).
        #[arg(long, conflicts_with = "pins")]
        clear_pins: bool,
        /// Rename a point, `OLD=NEW`. Repeat for each.
        #[arg(long = "rename")]
        renames: Vec<String>,
        /// Rename a point given as two values, `OLD NEW`: for a name holding `=`. Repeat for each.
        #[arg(long = "rename-pair", num_args = 2, value_names = ["OLD", "NEW"])]
        rename_pairs: Vec<String>,
    },
    /// Number the wires by rule. Every wire of one connection gets the same number.
    /// Rules: `sequential` (1, 2, 3 in drawing order), `page-column` (`3.4`: page and column
    /// where the connection starts) or `potential` (the potential's name, else sequential).
    NumberWires {
        /// `sequential`, `page-column` or `potential`.
        #[arg(long, default_value = "page-column")]
        rule: String,
        /// Renumber connections that already have a number too.
        #[arg(long)]
        overwrite: bool,
    },
    /// Draw a wire. Ends are `item:pin` (e.g. `s1:13`) or a point `x,y`.
    Wire {
        /// The page's id.
        page: String,
        /// Start: `item:pin` (e.g. `s1:13`) or a point `x,y` in mm.
        from: String,
        /// End: `item:pin` or a point `x,y` in mm.
        to: String,
        /// Bend points, `x,y` each.
        #[arg(long = "via", value_parser = parse_point)]
        via: Vec<Point>,
        /// Wire number, e.g. `101`.
        #[arg(long)]
        number: Option<String>,
        /// Potential name, e.g. `L1` or `24V`.
        #[arg(long)]
        potential: Option<String>,
        /// The cable this wire is a core of, e.g. `=PUMP-W1` or `-W1` (taken as a value, though
        /// it starts with `-`).
        #[arg(long, allow_hyphen_values = true)]
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
    /// Delete symbols and wires from a page by their ids (as `place`, `box` and `wire` print
    /// them). Wires that end on a pin of a deleted symbol go too, as in the app; devices keep
    /// their data. An id that is not on the page is an error and nothing changes. Prints each
    /// deleted id.
    Delete {
        /// The page's id.
        page: String,
        /// The ids of the symbols and wires to delete, e.g. `s1 w3`.
        #[arg(required = true)]
        ids: Vec<String>,
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
    /// PLC racks and the I/O cards in their slots (`[[rack]]` in project.toml), laid out by the
    /// PLC card overview (`pdf --report plc-cards`). After a change the overview's rack
    /// warnings are printed.
    #[command(subcommand)]
    Rack(RackCmd),
    /// The project logo, shown on the cover page and in every title block.
    #[command(subcommand)]
    Logo(LogoCmd),
    /// The check rules `check` runs: the project's own `[[rule]]` tables in `project.toml`,
    /// and the house rules in each library source's `rules.toml`, which the project can turn
    /// off one by one (`rules_off`).
    #[command(subcommand)]
    Rules(RulesCmd),
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
    /// The layouts of the reports (`[[report]]` in project.toml): which columns show in what
    /// order under which headings, sorting, grouping and the title-block values above the table.
    #[command(subcommand)]
    ReportLayout(ReportLayoutCmd),
    /// Set a device's part and description, or with neither option print them.
    Device {
        /// Device tag, e.g. `-K1` (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        tag: String,
        /// The part's library id; it is taken from the libraries when the project lacks it.
        #[arg(long)]
        part: Option<String>,
        /// What the device does, e.g. `Pump motor`.
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
        /// The PDF file to write.
        out: PathBuf,
        /// Page ids to export (default: all).
        #[arg(long = "page")]
        pages: Vec<String>,
        /// Date for the title block where the page and project set none; free text, printed as
        /// given, e.g. `2026-11-26`.
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
    /// translation into a language (exits with an error when any is missing, so it can guard a
    /// release), or set or remove one text's translation.
    Translations {
        /// The language, such as `nb`.
        #[arg(long, global = true)]
        lang: Option<String>,
        #[command(subcommand)]
        edit: Option<TranslationsCmd>,
    },
    /// Export pages as drawings for other tools: one SVG or DXF file per page in a folder,
    /// drawn like the PDF (frame, title block, symbols, wires, cross-references), and one per
    /// panel layout (plate, rails, ducts, devices).
    Drawing {
        /// Output folder; files are named `<page number>.<format>`, and `panel-<name>.<format>`
        /// for panel layouts.
        out: PathBuf,
        /// `svg` or `dxf`.
        #[arg(long, default_value = "svg")]
        format: String,
        /// Page ids to export (default: all).
        #[arg(long = "page")]
        pages: Vec<String>,
        /// Date for the title block where the page and project set none; free text, printed as
        /// given, e.g. `2026-11-26`.
        #[arg(long, default_value = "")]
        date: String,
    },
    /// Print a report (contents, parts, devices, wires, nets, terminals, cables, plc, plc-cards, locations, networks) as a table, CSV or Markdown,
    /// or save it as an Excel workbook (`--format xlsx --out <file>`, one sheet per report).
    Report {
        /// contents, parts, devices, wires, nets, terminals, cables, plc, plc-cards, locations,
        /// networks, or `all` (every report; one sheet each with `--format xlsx`).
        kind: String,
        /// `text`, `csv`, `md` or `xlsx`.
        #[arg(long, default_value = "text")]
        format: String,
        /// Write to a file instead of standard output. With `all` and `--format csv` this is
        /// a folder, which gets one `<report>.csv` per report.
        #[arg(long)]
        out: Option<PathBuf>,
        /// CSV field separator: `,` (the default) or `;`. With `;` a file written with `--out`
        /// also starts with a UTF-8 byte order mark, so Excel in a comma-decimal locale opens
        /// it in columns with letters like æ ø å intact.
        #[arg(long, default_value = ",")]
        sep: char,
    },
    /// List the potentials (connected pins) across all pages.
    Nets,
    /// Export devices, their part data and connection points, and the connections between them
    /// as AutomationML (CAEX 3.0), for tools that take engineering data rather than drawings.
    Aml {
        /// The AutomationML (`.aml`) file to write.
        out: PathBuf,
    },
    /// Export data for other tools: marker lists for printing marking strips.
    #[command(subcommand)]
    Export(ExportCmd),
    /// Write the PLC inputs and outputs as an IEC 61131-3 global variable list (structured
    /// text) for TwinCAT, CODESYS and similar tools: one variable per point, named after its
    /// device tag and located at its address (`%I*`/`%Q*` when it has none).
    PlcVars {
        /// The structured text file to write, e.g. `GVL_IO.st`.
        out: PathBuf,
    },
    /// Check the project for mistakes: revision clouds whose revision the project does not
    /// list, revision letters the PDF cannot print, symbols missing from the project's library
    /// copy and wires on pins that do not exist, devices drawn twice, more contacts than a
    /// device's parts have, contacts without a coil, interruption points without a partner,
    /// open pins, wires ending in the air, wires with both ends on one pin, jumpers naming a
    /// terminal drawn nowhere, one cable core on two connections, shorted potentials, a wire
    /// number used on more than one connection or one connection carrying several wire
    /// numbers, panel layouts with overlapping devices, devices sticking out of the plate or no
    /// longer in the project, parts missing from the project's library copy, devices without a
    /// part, then the house rules in each library source's `rules.toml`, less the ones
    /// `rules_off` turns off, and last the project's own
    /// `[[rule]]` tables in `project.toml` (a cable on every motor, a tag pattern, at most N
    /// wires on a terminal).
    /// Exits with an error when an error is found, so it can guard a commit or a CI job.
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
        /// The text to find; upper and lower case are alike unless --match-case. A device tag
        /// such as `-K1` is taken as the text, though it starts with `-`.
        #[arg(allow_hyphen_values = true)]
        query: String,
        /// Replace the found text with this, in every hit (taken as a value, though it starts
        /// with `-`, e.g. `-K2`).
        #[arg(long, allow_hyphen_values = true)]
        replace: Option<String>,
        /// Upper and lower case must match.
        #[arg(long)]
        match_case: bool,
    },
    /// Show what is inside a project archive from another tool (.tewzip, .zw1, .elk, .zip).
    Inspect {
        /// The archive file.
        file: PathBuf,
    },
    /// Version control: commit, branches, merge, history.
    #[command(subcommand)]
    Vcs(VcsCmd),
    /// Review comments pinned on pages: add, reply, resolve, list.
    #[command(subcommand)]
    Comment(CommentCmd),
}

/// `translations set` and `translations remove`: one text of `translations.toml`.
#[derive(Subcommand)]
enum TranslationsCmd {
    /// Set the translation of a project text (as drawn, in the source language) into `--lang`.
    Set {
        /// The text as drawn, such as a page title or device description.
        text: String,
        /// The text in `--lang`.
        translation: String,
    },
    /// Remove the translation of a project text into `--lang`.
    Remove {
        /// The text as drawn.
        text: String,
    },
}

#[derive(Subcommand)]
enum ExportCmd {
    /// Terminal, wire, device and cable markers for WAGO smartScript: a semicolon-separated
    /// CSV (UTF-8) with one row per marker (type, group, marker text, copies), in natural
    /// tag order. In smartScript, import it as Excel/CSV data and map the Marker column (or
    /// the Line columns, one per text line of the marker).
    WagoMarking {
        /// The CSV file to write.
        out: PathBuf,
        #[command(flatten)]
        args: MarkingArgs,
    },
    /// Terminal strips for the WAGO configurator (smartDESIGNER): a semicolon-separated CSV
    /// (UTF-8) with, for every strip in natural tag order, its plates and DIN rails, its
    /// terminals in strip order (part, level, wire cross-section and colour on each side), its
    /// jumpers and its accessories. Each row has a stable Index for importing the list back.
    WagoConfigurator {
        /// The CSV file to write.
        out: PathBuf,
    },
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
        /// The page's id.
        page: String,
        /// The comment.
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
        /// The comment's id, as `comment list` shows it.
        id: String,
        /// The answer.
        text: String,
        /// Who is writing (default: WIRINGCRAFT_USER, then git's user.name, then the login).
        #[arg(long)]
        author: Option<String>,
    },
    /// Mark a comment resolved.
    Resolve {
        /// The comment's id, as `comment list` shows it.
        id: String,
    },
    /// Open a resolved comment again.
    Reopen {
        /// The comment's id, as `comment list` shows it.
        id: String,
    },
    /// Delete a comment and its replies.
    Remove {
        /// The comment's id, as `comment list` shows it.
        id: String,
    },
    /// List the comments, of one page or all; open ones only unless --all.
    List {
        /// Only this page's comments (page id).
        page: Option<String>,
        /// Resolved comments too.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum PanelCmd {
    /// A new mounting plate with ducts around the edge and a DIN rail in every 125 mm row.
    New {
        /// The new layout's id, e.g. `plate1`.
        id: String,
        /// The layout's name, shown on its page.
        #[arg(long, default_value = "")]
        name: String,
        /// Plate width in mm.
        #[arg(long, default_value_t = 600.0)]
        width: f64,
        /// Plate height in mm.
        #[arg(long, default_value_t = 800.0)]
        height: f64,
    },
    /// Change a layout's name or plate size. Rails, ducts and devices stay where they are; a
    /// size that would leave any of them sticking out of the plate is refused.
    Edit {
        /// The layout's id.
        id: String,
        /// The new name, shown on its page; `""` uses the id.
        #[arg(long)]
        name: Option<String>,
        /// New plate width in mm.
        #[arg(long)]
        width: Option<f64>,
        /// New plate height in mm.
        #[arg(long)]
        height: Option<f64>,
    },
    /// Put devices that have a part and are not on the plate yet onto its rails, sized by the
    /// part's `Width` and `Height` properties.
    Arrange {
        /// The layout's id.
        id: String,
        /// Only devices in this location, e.g. `+CAB1`.
        #[arg(long)]
        location: Option<String>,
    },
    /// Write a layout as a 3D STEP model (plate, rails, ducts and devices as solids, depths
    /// from the part's `Depth` property) for SolidCraft or any other CAD program.
    Step {
        /// The layout's id.
        id: String,
        /// The STEP file to write.
        out: PathBuf,
    },
    /// Put one device on a plate, or move or turn it when it is there already, or take it off
    /// with `--remove`. Its size comes from its part as `arrange` sizes it.
    Device {
        /// The layout's id.
        id: String,
        /// Device tag, e.g. `-K1` (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        tag: String,
        /// Top-left corner of the device's (turned) outline in mm, `x,y`. Needed to put a device
        /// on the plate; a device already there keeps its place when left out.
        #[arg(long, value_parser = parse_point, conflicts_with = "remove")]
        at: Option<Point>,
        /// Turn clockwise: 0, 90, 180 or 270. A device already there keeps its turn when left
        /// out.
        #[arg(long, conflicts_with = "remove")]
        rotate: Option<i64>,
        /// Take the device off the plate.
        #[arg(long)]
        remove: bool,
    },
    /// Add a 35 mm DIN rail to a plate, or take one off with `--remove N` (rails numbered as
    /// `panel show` lists them).
    Rail {
        /// The layout's id.
        id: String,
        /// Left end of the rail on its centre line in mm, `x,y`.
        #[arg(long, value_parser = parse_point, required_unless_present = "remove", conflicts_with = "remove")]
        at: Option<Point>,
        /// Length in mm, to the right of `--at`.
        #[arg(long, required_unless_present = "remove", conflicts_with = "remove")]
        length: Option<f64>,
        /// Take off the rail with this number.
        #[arg(long, value_name = "N")]
        remove: Option<usize>,
    },
    /// Add a wire duct to a plate, or take one off with `--remove N` (ducts numbered as
    /// `panel show` lists them).
    Duct {
        /// The layout's id.
        id: String,
        /// Top-left corner of the duct in mm, `x,y`.
        #[arg(long, value_parser = parse_point, required_unless_present = "remove", conflicts_with = "remove")]
        at: Option<Point>,
        /// Length in mm: to the right, or down with `--vertical`.
        #[arg(long, required_unless_present = "remove", conflicts_with = "remove")]
        length: Option<f64>,
        /// Width in mm, across the length.
        #[arg(long, default_value_t = wc_core::panel::DUCT, conflicts_with = "remove")]
        width: f64,
        /// Run the duct down the plate instead of across.
        #[arg(long, conflicts_with = "remove")]
        vertical: bool,
        /// Take off the duct with this number.
        #[arg(long, value_name = "N")]
        remove: Option<usize>,
    },
    /// Print a layout's plate, its numbered rails and ducts, and each device on it with its
    /// position, size and turn.
    Show {
        /// The layout's id.
        id: String,
    },
    /// List the layouts and what is on them.
    List,
    /// Delete a layout.
    Remove {
        /// The layout's id.
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
    Remove {
        /// The location, with or without its `+`.
        path: String,
    },
}

#[derive(Subcommand)]
enum MacroCmd {
    /// List the macros, project first, with their value sets.
    List,
    /// Place a macro on a page with its top-left corner at `--at`. Its symbols get the next
    /// free tags and its devices the macro's data, placeholders filled from `--set`. Prints
    /// the new symbol and wire ids.
    Place {
        /// The page's id.
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
    /// Show a macro: its description, placeholders (name, default, description), value sets,
    /// the placeholders its texts use, and a problem `edit` would refuse to save.
    Show {
        /// The macro's name or file name.
        name: String,
    },
    /// Edit a project macro's description, placeholders and value sets (a library's macros
    /// are not edited here). The changes are made in the order of the options below, and the
    /// macro is written only when its placeholders and value sets then check out.
    Edit {
        /// The macro's name or file name.
        name: String,
        /// The macro's new description.
        #[arg(long)]
        description: Option<String>,
        /// Rename a placeholder, `old=new`: in its declaration, its values in the value sets
        /// and its uses in the macro's texts. Repeatable.
        #[arg(long, value_name = "OLD=NEW")]
        rename: Vec<String>,
        /// Declare a placeholder, or change one's default and description, as
        /// `name[:default[:description]]`; a part left out stays as it was. Repeatable.
        #[arg(long, value_name = "NAME[:DEFAULT[:DESCRIPTION]]")]
        placeholder: Vec<String>,
        /// Remove a placeholder and its values in the value sets. Its texts must no longer use
        /// it. Repeatable.
        #[arg(long, value_name = "NAME")]
        remove: Vec<String>,
        /// Give a value set a placeholder's value, `set:name=value`, adding the set if it is
        /// not there; an empty value is stored empty, not the default. Repeatable.
        #[arg(long, value_name = "SET:NAME=VALUE")]
        set: Vec<String>,
        /// Remove a value set. Repeatable.
        #[arg(long, value_name = "SET")]
        remove_set: Vec<String>,
    },
}

#[derive(Subcommand)]
enum LogoCmd {
    /// Show the logo's file, if the project has one.
    Show,
    /// Make a PNG or JPEG picture the logo: it is copied into the project's `pictures` folder
    /// (a picture already there is used as it is). Its place on the cover page is kept.
    Set {
        /// The PNG or JPEG file.
        file: PathBuf,
    },
    /// Take the logo off the cover page and the title blocks; its file stays in the project.
    Remove,
}

#[derive(Subcommand)]
enum RulesCmd {
    /// List the project's own rules (by number), then each library source's rules
    /// (`source:number`, on or off), one per line: the rule, whether it runs, and its kind and
    /// values. Entries of `rules_off` that name no rule are listed last.
    List {
        /// Print everything as JSON instead: `rules`, `library` and `rules_off`.
        #[arg(long)]
        json: bool,
    },
    /// Replace the project's own rules with the ones in a file, in order: `[[rule]]` tables
    /// (TOML, as in `project.toml`), or with a `.json` name a list of rules (or `{"rules":
    /// [...]}`). `-` reads TOML from stdin. A rule has `kind` (cable, tag, max-wires) and
    /// optionally `devices`, `role`, `pattern`, `max`, `severity`, `message` and `name`; a rule
    /// the check could not run is refused and nothing is changed. An empty file removes them
    /// all.
    Set {
        /// The rules file (`.toml` or `.json`), or `-` for TOML from stdin.
        file: PathBuf,
    },
    /// Turn a library rule off for this project, by `source:number` or `source:name`.
    Off {
        /// The library rule, `source:number` or `source:name`.
        rule: String,
    },
    /// Turn a library rule back on, by `source:number` or `source:name`: every `rules_off`
    /// entry that turns it off is removed.
    On {
        /// The library rule, `source:number` or `source:name`.
        rule: String,
    },
}

#[derive(Subcommand)]
enum RevisionCmd {
    /// List the revisions, oldest first, and the clouds on each page.
    List,
    /// Add a revision after the others.
    Add {
        /// The revision's letter or name, e.g. `A`.
        letter: String,
        /// What changed.
        #[arg(long, default_value = "")]
        text: String,
        /// Date of the revision, shown in `revision list` and the revision table; free text,
        /// printed as given, e.g. `2026-11-26`.
        #[arg(long, default_value = "")]
        date: String,
        /// Who made it.
        #[arg(long, default_value = "")]
        by: String,
    },
    /// Change a revision's description, date and author.
    Edit {
        /// The revision's letter, e.g. `A`.
        letter: String,
        /// New description of what changed.
        #[arg(long)]
        text: Option<String>,
        /// New date; free text, printed as given.
        #[arg(long)]
        date: Option<String>,
        /// New author.
        #[arg(long)]
        by: Option<String>,
    },
    /// Remove a revision; refused while a cloud carries it.
    Remove {
        /// The revision's letter, e.g. `A`.
        letter: String,
    },
    /// Draw a cloud on a page around the rectangle from (x1, y1) to (x2, y2), in mm.
    Cloud {
        /// The page's id.
        page: String,
        /// One corner's x in mm.
        x1: f64,
        /// One corner's y in mm.
        y1: f64,
        /// The opposite corner's x in mm.
        x2: f64,
        /// The opposite corner's y in mm.
        y2: f64,
        /// The revision in its tag; the latest when left out.
        #[arg(long)]
        rev: Option<String>,
    },
    /// Remove a page's clouds: the one at an index `revision list` shows, or with `--rev` every
    /// cloud of that revision.
    Uncloud {
        /// The page's id.
        page: String,
        /// The cloud's index on the page, as `revision list` shows it.
        index: Option<usize>,
        /// Remove every cloud of this revision instead.
        #[arg(long)]
        rev: Option<String>,
    },
    /// Show the revision table on a page with its top-left corner at (x, y) in mm, or take it
    /// off with `--off`.
    Table {
        /// The page's id.
        page: String,
        /// Left edge of the table in mm.
        x: Option<f64>,
        /// Top edge of the table in mm.
        y: Option<f64>,
        /// Take the table off the page.
        #[arg(long)]
        off: bool,
    },
}

#[derive(Subcommand)]
enum TemplateCmd {
    /// Print the template as TOML (the built-in one when the project has none).
    Show,
    /// Set the number of reference columns across the frame (1 to 40).
    Columns {
        /// How many columns, 1 to 40.
        count: usize,
    },
    /// Go back to the built-in layout (removes `[template]` from project.toml).
    Reset,
}

#[derive(Subcommand)]
enum ReportLayoutCmd {
    /// Print the layouts as JSON: for each report its kind, its layout (null when it has the
    /// built-in one), its built-in columns and the warnings about its layout.
    Show {
        /// One report (parts, wires, ...); every report when left out.
        kind: Option<String>,
    },
    /// Replace one report's layout with the JSON object in `file` (`-` for standard input),
    /// with the keys of a `[[report]]` table: title, column (a list of {from, heading}), sort,
    /// group, header. It is kept as the report layout dialog keeps it.
    Set {
        /// The report (parts, wires, ...).
        kind: String,
        /// The layout as JSON, or `-` to read it from standard input.
        file: PathBuf,
    },
    /// Go back to the report's built-in layout (removes its `[[report]]` table).
    Reset {
        /// The report (parts, wires, ...).
        kind: String,
    },
}

#[derive(Subcommand)]
enum JumperCmd {
    /// Jumper terminals of a strip, e.g. `jumper add -X1 3 4 5`. A jumper that shares a
    /// terminal with them becomes one with them.
    Add {
        /// The strip, e.g. `-X1` (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        strip: String,
        /// Terminal numbers, at least two.
        terminals: Vec<String>,
    },
    /// Take terminals out of their jumper, e.g. `jumper remove -X1 4`.
    Remove {
        /// The strip, e.g. `-X1` (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        strip: String,
        /// Terminal numbers.
        terminals: Vec<String>,
    },
    /// Give the jumper holding a terminal a part (article), e.g.
    /// `jumper part -X1 3 wago.2002-402`; leave the part out to clear it. A part the project
    /// does not have yet is taken from its libraries.
    Part {
        /// The strip, e.g. `-X1` (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        strip: String,
        /// A terminal the jumper holds.
        terminal: String,
        /// The part's library id; left out clears it.
        part: Option<String>,
    },
    /// List the project's jumpers.
    List,
}

#[derive(Subcommand)]
enum RackCmd {
    /// List the racks with their cards by slot: tag, type, channels, start address and step.
    List,
    /// Add a rack, e.g. `rack add -A1 --description "Main PLC"`.
    Add {
        /// The rack's name, usually the PLC's device tag, e.g. `-A1` (taken as a value, though
        /// it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        name: String,
        /// What the rack is, e.g. `Main PLC`.
        #[arg(long)]
        description: Option<String>,
    },
    /// Remove a rack and its cards, e.g. `rack remove -A1`.
    Remove {
        /// The rack's name (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
    /// Rename a rack or change its description, e.g. `rack set -A1 --name -A2`.
    Set {
        /// The rack's name (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        name: String,
        /// Its new name (taken as a value, though it starts with `-`).
        #[arg(long = "name", allow_hyphen_values = true)]
        new_name: Option<String>,
        /// Its new description; empty clears it.
        #[arg(long)]
        description: Option<String>,
    },
    /// The cards in a rack's slots.
    #[command(subcommand)]
    Card(RackCardCmd),
}

#[derive(Subcommand)]
enum RackCardCmd {
    /// Add a card to a free slot, e.g.
    /// `rack card add -A1 1 --type "DI 16x24 V DC" --channels 16 --start %IX0.0`.
    Add {
        /// The rack's name (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        rack: String,
        /// The card's slot, 0 to 999.
        slot: u32,
        /// The card's device tag, e.g. `-A1.DI1` (taken as a value, though it starts with `-`).
        #[arg(long, allow_hyphen_values = true)]
        tag: Option<String>,
        /// What the card is, e.g. `DI 16x24 V DC` or an order number.
        #[arg(long = "type")]
        kind: Option<String>,
        /// How many channels it has, at most 256.
        #[arg(long, default_value_t = 16)]
        channels: u32,
        /// The address of channel 0, e.g. `%IX0.0` or `%IW64`.
        #[arg(long)]
        start: Option<String>,
        /// Bytes from one channel to the next for byte, word and double word cards, 1 to 64
        /// (default: the address's size).
        #[arg(long)]
        step: Option<u32>,
    },
    /// Change a card, e.g. `rack card set -A1 1 --start %IX4.0`; options left out are kept.
    Set {
        /// The rack's name (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        rack: String,
        /// The card's slot.
        slot: u32,
        /// Move the card to this slot, 0 to 999.
        #[arg(long = "slot")]
        new_slot: Option<u32>,
        /// The card's device tag; empty clears it (taken as a value, though it starts with `-`).
        #[arg(long, allow_hyphen_values = true)]
        tag: Option<String>,
        /// What the card is; empty clears it.
        #[arg(long = "type")]
        kind: Option<String>,
        /// How many channels it has, at most 256.
        #[arg(long)]
        channels: Option<u32>,
        /// The address of channel 0.
        #[arg(long)]
        start: Option<String>,
        /// Bytes from one channel to the next, 1 to 64.
        #[arg(long, conflicts_with = "no_step")]
        step: Option<u32>,
        /// Go back to the address's own size as the step.
        #[arg(long)]
        no_step: bool,
    },
    /// Take a card out of its slot, e.g. `rack card remove -A1 1`.
    Remove {
        /// The rack's name (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        rack: String,
        /// The card's slot.
        slot: u32,
    },
}

#[derive(Subcommand)]
enum StripCmd {
    /// List terminal strips with their terminals in rail order; spares are marked `(spare)`.
    /// Give a strip to list only that one, e.g. `strip list -X1`.
    List {
        /// Only this strip, e.g. `-X1` (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        strip: Option<String>,
    },
    /// Put a strip's terminals in this rail order, e.g. `strip order -X1 3 1 2`. Terminals
    /// left out follow at the end; no terminals goes back to the order by number.
    Order {
        /// The strip, e.g. `-X1` (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        strip: String,
        /// Terminal numbers in their new rail order.
        terminals: Vec<String>,
    },
    /// Move a terminal to a place on the rail (1 is first), e.g. `strip move -X1 3 1`.
    Move {
        /// The strip, e.g. `-X1` (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        strip: String,
        /// The terminal to move.
        terminal: String,
        /// Its new place on the rail, 1 being first.
        to: usize,
    },
    /// Add a spare terminal (on the rail, not drawn), e.g. `strip spare -X1 S1 --after 2`;
    /// without `--after` it goes at the end.
    Spare {
        /// The strip, e.g. `-X1` (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        strip: String,
        /// The spare terminal's number, e.g. `S1`.
        terminal: String,
        /// The terminal it goes right after.
        #[arg(long)]
        after: Option<String>,
    },
    /// Take a spare terminal off the strip (and out of its jumper), e.g.
    /// `strip unspare -X1 S1`.
    Unspare {
        /// The strip, e.g. `-X1` (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        strip: String,
        /// The spare terminal's number.
        terminal: String,
    },
    /// Set a terminal's level on a multi-level terminal (1 is the bottom), e.g.
    /// `strip level -X1 4 2`; leave the level out to clear the terminal's own `Level`
    /// property. Levels are inferred in number order, not rail order: a terminal whose next
    /// number up has level 2 or more is still level 1.
    Level {
        /// The strip, e.g. `-X1` (taken as a value, though it starts with `-`).
        #[arg(allow_hyphen_values = true)]
        strip: String,
        /// The terminal's number.
        terminal: String,
        /// Its level, 1 being the bottom; left out clears it.
        level: Option<u32>,
    },
}

#[derive(Subcommand)]
enum PageCmd {
    /// Add a page, placed by its `--number` among the existing pages.
    Add {
        /// The new page's id, e.g. `p1`; other commands name the page by it.
        id: String,
        /// The page number printed in the title block, e.g. `1` or `=A1/3`.
        #[arg(long)]
        number: String,
        /// The page's title, shown in the title block.
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
    /// List the pages: id, number, title, and how many symbols and wires each has.
    List,
    /// Delete a page and everything drawn on it.
    Remove {
        /// The page's id.
        id: String,
    },
    /// Mark a page as an overview (line diagram), or with `--off` as a normal page again.
    Overview {
        /// The page's id.
        id: String,
        /// Make it a normal page again.
        #[arg(long)]
        off: bool,
    },
    /// Change a page's number or title. A number another page already uses is refused; the
    /// pages are sorted by number again afterwards.
    Edit {
        /// The page's id.
        id: String,
        /// The new page number, e.g. `3` or `3.1`.
        #[arg(long)]
        number: Option<String>,
        /// The new page title.
        #[arg(long)]
        title: Option<String>,
    },
    /// Change a page's sheet size (as `page add --size` takes it). What is drawn stays where it
    /// is; a smaller sheet can leave some of it outside the frame.
    Size {
        /// The page's id.
        id: String,
        /// The new size, e.g. `A3`, `A4-portrait` or `500x700`.
        size: String,
    },
    /// Make a page the project's cover page (any other page loses the mark), or with `--off` a
    /// normal page again.
    Cover {
        /// The page's id.
        id: String,
        /// Make it a normal page again.
        #[arg(long)]
        off: bool,
    },
    /// Show a page's title-block fields, or set the page's own with `key=value` (revision,
    /// date, drawn_by, checked_by, ...). The page's own values win over the project's on that
    /// page; `key=` clears one, so the project's shows again.
    Fields {
        /// The page's id.
        id: String,
        /// `key=value` pairs to set; none lists the fields.
        set: Vec<String>,
    },
}

#[derive(Subcommand)]
enum LibCmd {
    /// Add a source. Earlier sources win over later ones.
    Add {
        /// The source's name, used in item listings.
        name: String,
        /// `folder` or `git`.
        kind: String,
        /// The folder's path, or the git repository's URL.
        location: String,
        /// For git: the branch, tag or commit to use (default: the default branch).
        #[arg(long)]
        reference: Option<String>,
    },
    /// Clone or refresh git sources.
    Sync,
    /// Compare the project's pinned items with the library.
    Status,
    /// Take newer library versions (all changed items, or the ids given).
    Update {
        /// Symbol or part ids to update (default: all changed items).
        ids: Vec<String>,
    },
    /// Pin every used item that is not pinned yet, and write a pinned item's vendored copy again
    /// when it is missing or unreadable.
    Lock,
    /// Find symbols and parts in the library sources by id, name, category, letter code or
    /// description.
    Search {
        /// The words to look for: an item matches when it contains every word, in any order;
        /// upper and lower case are alike.
        query: String,
    },
    /// Offer the project's copy of an item back to a source for review.
    Propose {
        /// The library source's name.
        source: String,
        /// The symbol's or part's id.
        id: String,
        /// The id is a part, not a symbol.
        #[arg(long)]
        part: bool,
    },
}

#[derive(Subcommand)]
enum ImportCmd {
    /// A symbol from a DXF drawing. Pins: POINT entities, or entities on a layer named PIN.
    SymbolDxf {
        /// The DXF file.
        file: PathBuf,
        /// The new symbol's id, e.g. `my.relay.coil`.
        #[arg(long)]
        id: String,
        /// The symbol's name, shown in the library.
        #[arg(long)]
        name: String,
        /// The library folder to write into.
        #[arg(long)]
        lib: PathBuf,
        /// Library category to file it under, e.g. `relays`.
        #[arg(long, default_value = "")]
        category: String,
        /// Letter code for new devices, e.g. `K`.
        #[arg(long, default_value = "")]
        letter_code: String,
    },
    /// A maker's outline drawing (DXF, the front view, in mm at 1:1) as a part's footprint on
    /// cabinet layouts: stored in the library with the part, and drawn on layout pages instead
    /// of a box. A part without `Width` and `Height` takes its size from the drawing. When the
    /// project uses the part, its pinned copy is updated too, unless that copy has local changes
    /// or the project takes the part from another source (a warning says so). The project's own
    /// pinned copy cannot be named with --lib.
    FootprintDxf {
        /// The DXF file.
        file: PathBuf,
        /// The part's library id.
        #[arg(long)]
        part: String,
        /// The library folder that holds the part (default: the project's library source that
        /// has it).
        #[arg(long)]
        lib: Option<PathBuf>,
    },
    /// A maker's 3D model (STEP) of a part: its bounding box gives the part's size for cabinet
    /// layouts and their 3D export. The plate size comes from the part's `Width` and `Height`
    /// first, then its footprint drawing, and only then the model; the depth from `Depth`, then
    /// the model. The model's x axis runs across the plate, y up it and z out of it. Stored in
    /// the library with the part. When the project uses the part, its pinned copy is updated
    /// too, unless that copy has local changes or the project takes the part from another
    /// source (a warning says so). The project's own pinned copy cannot be named with --lib.
    ModelStep {
        /// The STEP file.
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
        /// The SVG file.
        file: PathBuf,
        /// The new symbol's id, e.g. `my.relay.coil`.
        #[arg(long)]
        id: String,
        /// The symbol's name, shown in the library.
        #[arg(long)]
        name: String,
        /// The library folder to write into.
        #[arg(long)]
        lib: PathBuf,
        /// Library category to file it under, e.g. `relays`.
        #[arg(long, default_value = "")]
        category: String,
        /// Letter code for new devices, e.g. `K`.
        #[arg(long, default_value = "")]
        letter_code: String,
    },
    /// PLC addresses from a CSV list or an Excel workbook (device and address columns,
    /// description optional), as a PLC tool or spreadsheet writes it. `report plc --format csv`
    /// (or `xlsx`) writes one that reads back.
    PlcCsv {
        /// The CSV or `.xlsx` file.
        file: PathBuf,
    },
    /// The WAGO configurator list back: a list written by `export wago-configurator`, with the
    /// articles and accessories chosen in the configurator. Each row is matched by its Index to
    /// the terminal or strip device it stands for, and its article to a part of the project or
    /// its libraries (by order number, or part id); matched articles become the devices' parts
    /// and accessories. Rows that cannot be matched are listed with the reason.
    Wago {
        /// The CSV file from the configurator.
        file: PathBuf,
    },
    /// Every symbol of a KiCad symbol library (`.kicad_sym`, KiCad 6 or later). Ids are
    /// `<prefix>.<symbol name>`; the first pin becomes the origin.
    SymbolKicad {
        /// The `.kicad_sym` file.
        file: PathBuf,
        /// The library folder to write into.
        #[arg(long)]
        lib: PathBuf,
        /// Id prefix (default: `kicad.<file name>`).
        #[arg(long)]
        prefix: Option<String>,
        /// Multiply coordinates, e.g. 2 to put KiCad's 2.54 mm pin grid near a 5 mm grid.
        #[arg(long, default_value_t = 1.0)]
        scale: f64,
        /// Library category to file the symbols under.
        #[arg(long, default_value = "")]
        category: String,
    },
    /// Parts from a BMEcat catalogue (XML, version 1.2 or 2005): order number, maker,
    /// description, the maker's part number, GTIN, and ECLASS class and features as properties.
    PartsBmecat {
        /// The BMEcat XML file.
        file: PathBuf,
        /// The library folder to write into.
        #[arg(long)]
        lib: PathBuf,
    },
    /// Parts from an EDZ file (EPLAN Data Exchange archive, 7-Zip or zip): part number, maker,
    /// description and the EPLAN part properties. Each macro's multi-line representation,
    /// variant A, becomes a library symbol (`eplan.<maker>.<macro name>`) with its connection points as
    /// pins, and each part gets the symbol made from the macro it names; what the macros leave
    /// out is reported. Pictures are counted.
    PartsEdz {
        /// The `.edz` file.
        file: PathBuf,
        /// The library folder to write into.
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
        /// The `.elmt` files.
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// The library folder to write into.
        #[arg(long)]
        lib: PathBuf,
        /// Id prefix.
        #[arg(long, default_value = "qet")]
        prefix: String,
        /// Millimetres per QElectroTech pixel; 0.5 puts its 10 px grid on a 5 mm grid.
        #[arg(long, default_value_t = wc_import::qet::DEFAULT_SCALE)]
        scale: f64,
        /// Library category to file the symbols under.
        #[arg(long, default_value = "")]
        category: String,
        /// Letter code for new devices, e.g. `K`.
        #[arg(long, default_value = "")]
        letter_code: String,
    },
    /// Parts from a CSV or semicolon-separated export, or straight from an Excel workbook
    /// (`.xlsx`, its first visible sheet), with the same column names either way.
    PartsCsv {
        /// The CSV or `.xlsx` file.
        file: PathBuf,
        /// The library folder to write into.
        #[arg(long)]
        lib: PathBuf,
    },
    /// The open-format parts of another tool's project archive (PDF pages as trace pages,
    /// DXF drawings as pages, AutomationML devices and connections, PLC address lists onto the
    /// project's devices, KiCad symbol libraries and CSV parts lists into `--lib`).
    Archive {
        /// The archive file.
        file: PathBuf,
        /// Number of the first new page; the others follow it.
        #[arg(long, default_value_t = 300)]
        first_page: usize,
        /// Library folder for the symbols of `.kicad_sym` entries and the parts of CSV parts
        /// lists; without it they are listed, not imported.
        #[arg(long)]
        lib: Option<PathBuf>,
    },
    /// A DWG drawing onto a page, read by WiringCraft's own DWG reader (AutoCAD 2004 to 2018).
    /// A file it refuses is converted to DXF by an installed converter (ODA File Converter,
    /// `dwg2dxf`, or the command in WIRINGCRAFT_DWG2DXF with {in} and {out}).
    PageDwg {
        /// The DWG file.
        file: PathBuf,
        /// The id of the page to draw onto; it must exist.
        #[arg(long)]
        page: String,
        /// Multiply the drawing's coordinates by this to get mm.
        #[arg(long, default_value = "1.0")]
        scale: f64,
    },
    /// PDF pages as trace graphics on new pages (vector content; scanned pages have none).
    Pdf {
        /// The PDF file.
        file: PathBuf,
        /// Number of the first new page; the others follow it.
        #[arg(long, default_value_t = 200)]
        first_page: usize,
    },
    /// An Eplan Electric P8 project (.elk, its .edb folder, or a zip of the folder): its pages as
    /// drawn, with devices at their original positions and wires traced, plus devices, parts and
    /// connections not on a drawn page as overview pages.
    Eplan {
        /// The `.elk` file, its `.edb` folder, or a zip of the folder.
        path: PathBuf,
        /// Number of the first new page; the others follow it.
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
        /// The `.tewzip` archive or `tew_project_data.sqlite` file.
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
        /// The `.tewzip` archive.
        path: PathBuf,
        /// A library folder to write into instead of the project.
        #[arg(long)]
        lib: Option<PathBuf>,
    },
    /// AutomationML (CAEX) devices, properties and connections, laid out as overview pages.
    Aml {
        /// The AutomationML (`.aml`) file.
        file: PathBuf,
        /// Number of the first new page; the others follow it.
        #[arg(long, default_value_t = 100)]
        first_page: usize,
    },
    /// A DXF drawing onto a page as graphics; blocks named like library symbols become symbols.
    PageDxf {
        /// The DXF file.
        file: PathBuf,
        /// The id of the page to draw onto; it must exist.
        #[arg(long)]
        page: String,
        /// Multiply the drawing's coordinates by this to get mm.
        #[arg(long, default_value = "1.0")]
        scale: f64,
    },
}

#[derive(Subcommand)]
enum VcsCmd {
    /// Make the project folder a git repository.
    Init,
    /// Record every change in the project as a new version. Prints its short id.
    Commit {
        /// What changed and why.
        #[arg(short, long)]
        message: String,
    },
    /// List the branches (the current one marked `*`), or create one.
    Branch {
        /// The new branch's name; left out lists the branches.
        name: Option<String>,
    },
    /// Switch the project to another branch.
    Switch {
        /// The branch's name.
        name: String,
    },
    /// Merge another branch into the current one; conflicting files are listed.
    Merge {
        /// The branch to merge.
        name: String,
    },
    /// History as a subway map (terminal), or as SVG with --svg.
    Log {
        /// Write the map as SVG to this file.
        #[arg(long)]
        svg: Option<PathBuf>,
        /// At most this many versions, newest first.
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },
    /// What changed, in schematic terms. Defaults to uncommitted changes.
    Changes {
        /// The version to compare from: a commit, branch or tag.
        #[arg(default_value = "HEAD")]
        from: String,
        /// The version to compare to (default: the uncommitted files).
        to: Option<String>,
    },
    /// One page's history: who changed it, when, and what changed, newest first.
    PageLog {
        /// The page's id or number.
        page: String,
        /// At most this many versions.
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

/// `delete`: take the symbols and wires named by `ids` off `page`, and the wires ending on a
/// deleted symbol's pin, as the app's Delete does. An id not on the page is an error and nothing
/// changes. Returns the deleted ids: symbols first, then wires, each in page order.
fn delete_items(page: &mut Page, ids: &[String]) -> Res<Vec<String>> {
    let missing: Vec<String> = ids
        .iter()
        .filter(|id| page.symbol(id).is_none() && !page.wires.iter().any(|w| &w.id == *id))
        .map(|id| format!("{:?}", id))
        .collect();
    if !missing.is_empty() {
        return Err(format!("no symbol or wire {} on page {}", missing.join(", "), page.id));
    }
    let symbols: Vec<&str> = ids
        .iter()
        .map(String::as_str)
        .filter(|id| page.symbol(id).is_some())
        .collect();
    let wires: Vec<&str> = ids.iter().map(String::as_str).collect();
    Ok(page.remove_symbols_and_wires(&symbols, &wires))
}

/// A wire end from `item:pin` or `x,y`. A pin must be one the placed symbol has (its
/// definition read from the project's vendored symbols in `dir`).
fn parse_end(dir: &Path, page: &Page, s: &str) -> Res<Endpoint> {
    if let Ok(p) = parse_point(s) {
        return Ok(Endpoint::Point { at: p });
    }
    let (item, pin) = s
        .split_once(':')
        .ok_or_else(|| format!("{s:?}: expected item:pin or x,y"))?;
    let Some(placed) = page.symbol(item) else {
        return Err(format!("no symbol {item:?} on page {}", page.id));
    };
    let name = || {
        placed
            .tag
            .as_ref()
            .map_or_else(|| item.to_owned(), |t| format!("{item} ({t})"))
    };
    let symbol = match store::read_vendored_symbol(dir, &placed.symbol) {
        Ok(symbol) => Some(symbol),
        // No copy yet is allowed, as elsewhere (`lib lock` fills it in): the pin goes unchecked.
        Err(wc_core::Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(why) => {
            return Err(format!(
                "symbol {} on page {}: its definition {} could not be read ({})",
                name(),
                page.id,
                placed.symbol,
                escape_field(&why.to_string())
            ));
        }
    };
    if let Some(symbol) = symbol
        && symbol.pin(pin).is_none()
    {
        let name = name();
        let pins: Vec<String> = symbol.pins.iter().map(|p| escape_field(&p.name)).collect();
        return Err(format!(
            "symbol {name} on page {} has no pin {pin:?}; its pins are {}",
            page.id,
            if pins.is_empty() {
                "none".to_owned()
            } else {
                pins.join(", ")
            }
        ));
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

/// Refuse `--page` ids the project does not have, naming the ids it does, so a typo fails
/// instead of writing an empty export (#551).
fn check_pages(p: &Project, pages: &[String]) -> Res<()> {
    match pages.iter().find(|id| p.page(id).is_none()) {
        None => Ok(()),
        Some(id) => {
            let ids: Vec<String> = p.pages.iter().map(|pg| escape_field(&pg.id)).collect();
            Err(format!("no page {id:?} (pages: {})", ids.join(", ")))
        }
    }
}

/// Write an output file the user named. Under [`output_guard`] the file is written so that it
/// cannot land on a project file, a link swapped in after the app's own look included (#458).
/// A failure names the file: the guarded write does that itself, a plain one is given it here.
fn write_out(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    let path = path.as_ref();
    match output_guard() {
        Some(g) => wc_core::guard::write_output(Some(g), path, bytes),
        None => std::fs::write(path, bytes).map_err(|err| {
            std::io::Error::new(
                err.kind(),
                format!("cannot write {}: {err}", escape_field(&path.display().to_string())),
            )
        }),
    }
}

/// The text of the input file `path`; a failure names the file.
fn read_text(path: impl AsRef<Path>) -> Res<String> {
    let path = path.as_ref();
    std::fs::read_to_string(path)
        .map_err(|err| format!("cannot read {}: {err}", escape_field(&path.display().to_string())))
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

/// What a command says for a strip tag that no terminal on any page carries. Symbols without a
/// readable vendored copy have no role, so a terminal drawn with one may not count: they are
/// named, as a possible reason.
fn no_strip(strip: &DeviceTag, unreadable: &[String]) -> String {
    if unreadable.is_empty() {
        return format!("no terminal strip {strip}");
    }
    format!(
        "no terminal strip {strip} (these symbols have no readable vendored copy, and a terminal drawn with one may not count: {})",
        escape_field(&unreadable.join(", "))
    )
}

/// Says which pages stopped being the cover page when another one took the mark.
fn cover_moved(ids: Vec<String>) {
    for id in ids {
        eprintln!("page {id:?} is no longer the cover page");
    }
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

/// One check rule on a line: its kind, then each value it sets as `key=value` (keys it does
/// not know too, so a misspelt one shows).
fn rule_line(r: &wc_core::check_rule::CheckRule) -> String {
    let mut line = if r.kind.trim().is_empty() {
        "(no kind)".to_owned()
    } else {
        r.kind.clone()
    };
    let max = r.max.map(|m| m.to_string());
    let values = [
        ("name", r.name.as_ref()),
        ("devices", r.devices.as_ref()),
        ("role", r.role.as_ref()),
        ("pattern", r.pattern.as_ref()),
        ("max", max.as_ref()),
        ("severity", r.severity.as_ref()),
        ("message", r.message.as_ref()),
    ];
    for (key, v) in values {
        if let Some(v) = v {
            line.push_str(&format!(" {key}={v}"));
        }
    }
    for (key, v) in &r.unknown {
        line.push_str(&format!(" {key}={v}"));
    }
    escape_field(&line)
}

/// The `rules list` output: the project's rules, each library source's rules with whether the
/// project turns them off, and `rules_off` entries that name no rule; or all of it as JSON.
fn rules_list(p: &Project, library: &[wc_core::check_rule::RuleSet], json: bool) -> Res<String> {
    use wc_core::check_rule::turned_off;
    let off = &p.meta.rules_off;
    // An entry turns something off when it names a rule of a source that can be read.
    let names_a_rule = |o: &String| {
        library.iter().any(|set| {
            set.rules.as_ref().is_ok_and(|rules| {
                rules
                    .iter()
                    .enumerate()
                    .any(|(k, r)| turned_off(std::slice::from_ref(o), &set.source, k + 1, r))
            })
        })
    };
    if json {
        let all = wc_core::check_rule::rules_listing(&p.meta, library);
        return serde_json::to_string_pretty(&all).map(|t| t + "\n").map_err(e);
    }
    let mut out = String::new();
    for (k, r) in p.meta.rules.iter().enumerate() {
        out.push_str(&format!("{}\ton\t{}\n", k + 1, rule_line(r)));
    }
    for set in library {
        match &set.rules {
            Ok(rules) => {
                for (k, r) in rules.iter().enumerate() {
                    let state = if turned_off(off, &set.source, k + 1, r) {
                        "off"
                    } else {
                        "on"
                    };
                    out.push_str(&format!(
                        "{}:{}\t{state}\t{}\n",
                        escape_field(&set.source),
                        k + 1,
                        rule_line(r)
                    ));
                }
            }
            Err(why) => out.push_str(&format!(
                "{}\terror\t{}\n",
                escape_field(&set.source),
                escape_field(&format!("{}: {why}", wc_core::check_rule::LIBRARY_RULES_FILE))
            )),
        }
    }
    for o in off.iter().filter(|o| !names_a_rule(o)) {
        out.push_str(&format!("{}\toff\tnames no library rule\n", escape_field(o)));
    }
    Ok(out)
}

impl Cmd {
    /// Every path this command reads as an input file, as named on the command line (so `-`
    /// for stdin, and a folder where an importer takes one). Output paths and library folders
    /// are not inputs. [`spool_inputs`] goes through them before the project lock is taken.
    fn inputs_mut(&mut self) -> Vec<&mut PathBuf> {
        match self {
            Cmd::Inspect { file }
            | Cmd::Logo(LogoCmd::Set { file })
            | Cmd::Rules(RulesCmd::Set { file })
            | Cmd::ReportLayout(ReportLayoutCmd::Set { file, .. }) => vec![file],
            // Every import is listed, so a new one has to say what it reads.
            Cmd::Import(import) => match import {
                ImportCmd::SymbolDxf { file, .. }
                | ImportCmd::FootprintDxf { file, .. }
                | ImportCmd::ModelStep { file, .. }
                | ImportCmd::SymbolSvg { file, .. }
                | ImportCmd::PlcCsv { file }
                | ImportCmd::Wago { file }
                | ImportCmd::SymbolKicad { file, .. }
                | ImportCmd::PartsBmecat { file, .. }
                | ImportCmd::PartsCsv { file, .. }
                | ImportCmd::Archive { file, .. }
                | ImportCmd::PageDwg { file, .. }
                | ImportCmd::Pdf { file, .. }
                | ImportCmd::Aml { file, .. }
                | ImportCmd::PageDxf { file, .. } => vec![file],
                ImportCmd::Eplan { path, .. } | ImportCmd::SweLibrary { path, .. } => vec![path],
                ImportCmd::PartsEdz { file, symbol_map, .. } => std::iter::once(file).chain(symbol_map).collect(),
                ImportCmd::Swe { path, symbol_map, .. } => std::iter::once(path).chain(symbol_map).collect(),
                ImportCmd::SymbolQet { files, .. } => files.iter_mut().collect(),
            },
            _ => Vec::new(),
        }
    }
}

/// The most [`spool_inputs`] copies of one stream that no importer limit applies to: the
/// operand is refused when it has more. A stream without an end (`/dev/zero`, a FIFO whose
/// writer never stops) would otherwise hold the command up until the temporary folder is
/// full. Above every limit an importer has for what it unpacks, so an archive it would read
/// from a file is read from a stream too. Regular files are not copied and have no such limit.
const MAX_SPOOLED_BYTES: u64 = 4 << 30;

/// How much of one stream [`spool_inputs_within`] copies at most.
#[derive(Clone, Copy)]
struct SpoolLimits {
    /// A SolidWorks Electrical database given to `import swe` as it is, not in an archive.
    database: u64,
    /// Any other stream, but an `import symbol-qet` element, which has the importer's limit.
    stream: u64,
}

/// [`spool_inputs_within`] the limits the commands have: every copy has one.
fn spool_inputs(cmd: &mut Cmd) -> Res<Option<tempfile::TempDir>> {
    let limits = SpoolLimits {
        database: wc_import::swe::MAX_DATABASE_BYTES,
        stream: MAX_SPOOLED_BYTES,
    };
    spool_inputs_within(cmd, limits)
}

/// Copy every input of `cmd` that is not a regular file (a FIFO, a character device) into a
/// private temporary folder, under the name it has, and point the command at the copy (no
/// more of it than the importer reads). Every copy is bounded: an `import symbol-qet` element
/// is cut one byte over the importer's limit, which then refuses that element as too large; a
/// database for `import swe` (a stream that starts with the SQLite header) larger than
/// `limits.database`, and any other stream larger than `limits.stream`, is refused here, by
/// the name the operand has. An input named again, under the same or another name,
/// is read once: each operand gets the same bytes in a folder of its own, under its own name,
/// as an importer may take an id from the name. Called
/// before the project lock is taken: a FIFO that another command on the same project fills
/// would otherwise never be read, as its writer waits for the lock this command holds while it
/// waits for the writer (#658). Regular files, folders, `-` and paths that do not exist are
/// left as they are, for the command to read (or report) itself, and so is a stream that cannot
/// be opened, and an element stream `import symbol-qet` refuses unread for its id, which an
/// earlier element has. On Unix the `.edb` folder an
/// Eplan `.elk` stream names stays beside its copy, as a link to the one beside the operand.
/// The copies are removed when the returned folder is dropped; `None` when nothing was copied.
fn spool_inputs_within(cmd: &mut Cmd, limits: SpoolLimits) -> Res<Option<tempfile::TempDir>> {
    use std::io::{Read, Write};
    // A database is told from an archive by its first bytes, as the importer does.
    const SQLITE: &[u8] = b"SQLite format 3\0";
    #[cfg(unix)]
    let eplan = matches!(cmd, Cmd::Import(ImportCmd::Eplan { .. }));
    // The first input of `import swe` is a database or an archive; its symbol map is neither.
    let swe = matches!(cmd, Cmd::Import(ImportCmd::Swe { .. }));
    // An importer that reads no more than so much of its input gets no more than that: a
    // stream without an end (`/dev/zero`) would otherwise be copied until the disk is full.
    // One byte over the limit is kept, which is how the importer sees the input is too large.
    let element = matches!(cmd, Cmd::Import(ImportCmd::SymbolQet { .. })).then_some(wc_import::qet::MAX_LEN as u64);
    let qet = match cmd {
        Cmd::Import(ImportCmd::SymbolQet { prefix, scale, .. }) => Some((prefix.clone(), *scale)),
        _ => None,
    };
    // The elements before this one, as `import symbol-qet` will read them, each with its id.
    let mut earlier: Vec<(String, PathBuf)> = Vec::new();
    let mut spool = None;
    // A stream can be read once: an input named again gets the copy made the first time.
    let mut copied: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (n, path) in cmd.inputs_mut().into_iter().enumerate() {
        let id = qet.as_ref().map(|(prefix, _)| qet_id(path, prefix));
        let stream = path.as_os_str() != "-" && std::fs::metadata(&*path).is_ok_and(|m| !m.is_file() && !m.is_dir());
        if !stream {
            earlier.extend(id.map(|id| (id, path.clone())));
            continue;
        }
        // An element whose id an earlier one has is refused without being opened: it is not
        // opened here either, where a FIFO without a writer would hold the command up. The
        // earlier one has the id when it can be read as an element, as the import decides.
        if let (Some(id), Some((_, scale))) = (&id, &qet)
            && earlier
                .iter()
                .any(|(other, file)| other == id && qet_element(file, id, *scale).is_ok())
        {
            continue;
        }
        let same = std::fs::canonicalize(&*path).unwrap_or_else(|_| path.clone());
        let first = copied
            .iter()
            .find(|(from, _)| *from == same)
            .map(|(_, copy)| copy.clone());
        // A stream that cannot be opened is left for the command to report, as a path that
        // does not exist is: nothing of it was read.
        let from = match &first {
            Some(_) => None,
            None => match std::fs::File::open(&*path) {
                Ok(from) => Some(from),
                Err(_) => {
                    earlier.extend(id.map(|id| (id, path.clone())));
                    continue;
                }
            },
        };
        let fail = |x: std::io::Error| format!("{}: {x}", escape_field(&path.display().to_string()));
        let folder = match &spool {
            Some(folder) => folder,
            None => spool.insert(run::staging_dir("wc-input-").map_err(|x| format!("temporary folder: {x}"))?),
        };
        // One folder per input: two inputs may share a file name.
        let own = folder.path().join(n.to_string());
        std::fs::create_dir(&own).map_err(|x| format!("{}: {x}", own.display()))?;
        let copy = own.join(path.file_name().unwrap_or(std::ffi::OsStr::new("input")));
        let at_copy = |x: std::io::Error| format!("{}: {x}", copy.display());
        if let Some(first) = &first {
            // The bytes read the first time, under the name this operand has.
            std::fs::hard_link(first, &copy)
                .or_else(|_| std::fs::copy(first, &copy).map(|_| ()))
                .map_err(at_copy)?;
        } else if let Some(from) = from {
            let mut to = std::fs::File::create(&copy).map_err(at_copy)?;
            let mut head = Vec::new();
            if swe && n == 0 {
                (&from).take(SQLITE.len() as u64).read_to_end(&mut head).map_err(fail)?;
                to.write_all(&head).map_err(at_copy)?;
            }
            // The most this copy may hold, and whether one byte more is refused here (with
            // the importer's own words for a database) or left for the importer to refuse.
            let (most, refused) = match element {
                Some(most) => (most, None),
                None if head == SQLITE => (limits.database, Some("")),
                None => (limits.stream, Some("the stream ")),
            };
            let rest = most.saturating_add(1).saturating_sub(head.len() as u64);
            let len = std::io::copy(&mut (&from).take(rest), &mut to).map_err(fail)? + head.len() as u64;
            if let (true, Some(what)) = (len > most, refused) {
                return Err(format!(
                    "{}: {what}is larger than {} MB",
                    escape_field(&path.display().to_string()),
                    most >> 20
                ));
            }
            copied.push((same, copy.clone()));
        }
        #[cfg(unix)]
        if eplan {
            link_elk_folder(path, &copy)?;
        }
        earlier.extend(id.map(|id| (id, copy.clone())));
        *path = copy;
    }
    Ok(spool)
}

/// Beside `copy`, the copy of the Eplan `.elk` stream `operand`, a link to the `.edb` folder
/// the file names, which the importer looks for beside the file it is given: the folder is
/// beside the operand, not beside its copy. Nothing when the copy is not an `.elk` file or
/// names no folder that is there, which the importer then reports itself.
#[cfg(unix)]
fn link_elk_folder(operand: &Path, copy: &Path) -> Res<()> {
    use std::io::Read;
    // The folder name is the first line; a zip of the folder is not an `.elk` file.
    let mut head = Vec::new();
    std::fs::File::open(copy)
        .and_then(|f| f.take(1 << 16).read_to_end(&mut head))
        .map_err(|x| format!("{}: {x}", copy.display()))?;
    if head.starts_with(b"PK") {
        return Ok(());
    }
    let Some(name) = wc_import::eplan::elk_target(&head) else {
        return Ok(());
    };
    let folder = operand.with_file_name(&name);
    let link = copy.with_file_name(&name);
    if name == "." || name == ".." || link == copy || !folder.is_dir() {
        return Ok(());
    }
    // An absolute target: a relative one would be looked up from the link's folder.
    let folder = std::fs::canonicalize(&folder).map_err(|x| format!("{}: {x}", folder.display()))?;
    std::os::unix::fs::symlink(&folder, &link).map_err(|x| format!("{}: {x}", link.display()))
}

/// The rules in `file` for `rules set`, in order, each as the check runs it: `[[rule]]`
/// tables, or a JSON list (or `{"rules": [...]}`) for a `.json` name; `-` is TOML on stdin.
/// `stdin` is what was read from standard input for `-`, when it already was.
/// The text of an input file named on the command line, `-` meaning stdin.
fn read_input(file: &Path) -> Res<String> {
    if file.as_os_str() == "-" {
        std::io::read_to_string(std::io::stdin()).map_err(|err| format!("stdin: {err}"))
    } else {
        std::fs::read_to_string(file).map_err(|err| format!("{}: {err}", file.display()))
    }
}

/// The rules in `file`, from `input` (its text, read already) when given.
fn rules_from_file(file: &Path, input: Option<String>) -> Res<Vec<wc_core::check_rule::CheckRule>> {
    use wc_core::check_rule::{CheckRule, check_rule, parse_rules};
    let text = match input {
        Some(text) => text,
        None => read_input(file)?,
    };
    let json = file.extension().is_some_and(|x| x.eq_ignore_ascii_case("json"));
    let rules: Vec<CheckRule> = if json {
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|err| format!("{}: {err}", file.display()))?;
        let list = match value {
            serde_json::Value::Array(l) => l,
            serde_json::Value::Object(mut o) if o.len() == 1 && o.get("rules").is_some_and(|r| r.is_array()) => {
                match o.remove("rules") {
                    Some(serde_json::Value::Array(l)) => l,
                    _ => Vec::new(),
                }
            }
            _ => {
                return Err(format!(
                    "{}: not a list of rules or {{\"rules\": [...]}}",
                    file.display()
                ));
            }
        };
        list.into_iter()
            .enumerate()
            .map(|(k, r)| serde_json::from_value(r).map_err(|err| format!("Rule {}: {err}.", k + 1)))
            .collect::<Res<_>>()?
    } else {
        // Only `[[rule]]` tables: a misspelt `[[rules]]` would otherwise read as no rules and
        // remove the project's.
        let table: toml::Table = text
            .parse()
            .map_err(|err: toml::de::Error| format!("{}: {}", file.display(), err.message()))?;
        if let Some(key) = table.keys().find(|k| *k != "rule") {
            return Err(format!(
                "{}: {key:?} is not a rule table: write [[rule]] tables",
                file.display()
            ));
        }
        parse_rules(&text).map_err(|why| format!("{}: {why}", file.display()))?
    };
    rules.iter().enumerate().map(|(k, r)| check_rule(r, k + 1)).collect()
}

/// `rules off` / `rules on`: turn the library rule `which` off or back on in `rules_off`.
fn turn_library_rule(
    dir: &Path,
    p: &mut Project,
    library: &[wc_core::check_rule::RuleSet],
    which: &str,
    off: bool,
) -> Res<()> {
    let state = if off { "off" } else { "on" };
    match wc_core::check_rule::turn_rule(&p.meta.rules_off, library, which, off)? {
        None => println!("{} is {state} already", which.trim()),
        Some(now) => {
            p.meta.rules_off = now;
            save(dir, p)?;
            println!("{} is {state}", which.trim());
        }
    }
    Ok(())
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

/// A macro `macro` commands can name: from the project's own `macros` folder or a library
/// source's. Whether it is the project's comes from where it was found, not from the source's
/// name, which a library source may share.
struct FoundMacro {
    source: String,
    id: String,
    path: PathBuf,
    project: bool,
}

/// The macro named `name` among `found`: a file id match first, then one by its name, both
/// without regard to case. With no match, files that could not be read are named with why.
fn find_macro<'a>(found: &'a [FoundMacro], name: &str) -> Res<&'a FoundMacro> {
    let file = name.trim();
    let file = file.strip_suffix(".json").unwrap_or(file);
    if let Some(f) = found.iter().find(|f| f.id.eq_ignore_ascii_case(file)) {
        return Ok(f);
    }
    let mut unread = Vec::new();
    for f in found {
        match wc_core::macros::read_macro(&f.path) {
            Ok(m) if m.name.eq_ignore_ascii_case(name.trim()) => return Ok(f),
            Ok(_) => {}
            // Escaped: a folder name or a read error could hold control characters.
            Err(err) => unread.push(format!(
                "{}: {}",
                escape_field(&f.path.display().to_string()),
                escape_field(&err.to_string())
            )),
        }
    }
    if unread.is_empty() {
        Err(format!("no macro {name:?}"))
    } else {
        Err(format!(
            "no macro {name:?}; these macro files could not be read: {}",
            unread.join("; ")
        ))
    }
}

/// What `macro show` prints for `m` from `source`. Each value from the file is escaped with
/// [`escape_field`], so it stays on its line and cannot send the terminal control sequences.
fn macro_text(m: &wc_core::macros::Macro, source: &str) -> String {
    use std::fmt::Write as _;
    let x = |v: &str| escape_field(v);
    let mut out = format!("{} ({})\n", x(&m.name), x(source));
    if !m.description.is_empty() {
        let _ = writeln!(out, "description: {}", x(&m.description));
    }
    let _ = writeln!(
        out,
        "placeholders:{}",
        if m.placeholders.is_empty() { " none" } else { "" }
    );
    for p in &m.placeholders {
        let default = if p.default.is_empty() {
            String::new()
        } else {
            format!(" = {}", x(&p.default))
        };
        let description = if p.description.is_empty() {
            String::new()
        } else {
            format!("  ({})", x(&p.description))
        };
        let _ = writeln!(out, "  {}{default}{description}", x(&p.name));
    }
    let _ = writeln!(out, "value sets:{}", if m.value_sets.is_empty() { " none" } else { "" });
    for v in &m.value_sets {
        let values: Vec<String> = v.values.iter().map(|(k, val)| format!("{}={}", x(k), x(val))).collect();
        let description = if v.description.is_empty() {
            String::new()
        } else {
            format!("  ({})", x(&v.description))
        };
        let _ = writeln!(out, "  {}: {}{description}", x(&v.name), values.join(", "));
    }
    let used = m.used_placeholders();
    let _ = writeln!(
        out,
        "used in texts: {}",
        if used.is_empty() {
            "none".to_owned()
        } else {
            used.iter().map(|u| x(u)).collect::<Vec<_>>().join(", ")
        }
    );
    if let Err(err) = m.check() {
        let _ = writeln!(out, "problem: {}", x(&err.to_string()));
    }
    out
}

/// Apply `macro edit`'s options to `m`, in the order its help gives. The caller checks and
/// writes the result.
fn edit_macro(
    m: &mut wc_core::macros::Macro,
    description: Option<String>,
    rename: &[String],
    placeholder: &[String],
    remove: &[String],
    set: &[String],
    remove_set: &[String],
) -> Res<()> {
    use wc_core::macros::{Placeholder, ValueSet, key};
    if let Some(d) = description {
        m.description = d.trim().to_owned();
    }
    for r in rename {
        let (old, new) = r
            .split_once('=')
            .ok_or_else(|| format!("--rename {r:?}: write it as old=new"))?;
        m.rename_declared(old, new).map_err(e)?;
    }
    for p in placeholder {
        let mut parts = p.splitn(3, ':');
        let name = parts.next().unwrap_or_default().trim();
        if name.is_empty() {
            return Err(format!("--placeholder {p:?}: give a name"));
        }
        let (default, description) = (parts.next(), parts.next());
        let i = match m.placeholders.iter().position(|q| key(&q.name) == key(name)) {
            Some(i) => i,
            None => {
                m.placeholders.push(Placeholder {
                    name: name.to_owned(),
                    ..Default::default()
                });
                m.placeholders.len() - 1
            }
        };
        if let Some(q) = m.placeholders.get_mut(i) {
            if let Some(d) = default {
                q.default = d.to_owned();
            }
            if let Some(d) = description {
                q.description = d.trim().to_owned();
            }
        }
    }
    for name in remove {
        let before = m.placeholders.len();
        m.placeholders.retain(|q| key(&q.name) != key(name));
        if m.placeholders.len() == before {
            return Err(format!("--remove {name:?}: no such placeholder"));
        }
        for v in &mut m.value_sets {
            v.values.retain(|k, _| key(k) != key(name));
        }
    }
    for s in set {
        let parsed = s
            .split_once(':')
            .and_then(|(set, value)| value.split_once('=').map(|(k, v)| (set.trim(), k.trim(), v)));
        let Some((set_name, name, value)) = parsed.filter(|(set, k, _)| !set.is_empty() && !k.is_empty()) else {
            return Err(format!("--set {s:?}: write it as set:name=value"));
        };
        let i = match m.value_sets.iter().position(|v| key(&v.name) == key(set_name)) {
            Some(i) => i,
            None => {
                m.value_sets.push(ValueSet {
                    name: set_name.to_owned(),
                    ..Default::default()
                });
                m.value_sets.len() - 1
            }
        };
        if let Some(v) = m.value_sets.get_mut(i) {
            // The value replaces one given under the name in another case.
            v.values.retain(|k, _| key(k) != key(name));
            v.values.insert(name.to_owned(), value.to_owned());
        }
    }
    for name in remove_set {
        let before = m.value_sets.len();
        m.value_sets.retain(|v| key(&v.name) != key(name));
        if m.value_sets.len() == before {
            return Err(format!("--remove-set {name:?}: no such value set"));
        }
    }
    Ok(())
}

fn run(mut cli: Cli) -> Res<()> {
    // One command at a time on a project: a second run waits, so it reads what the first one
    // wrote instead of overwriting it with an edit planned from the old files (#610).
    // An input file (or stdin) is read before the lock is taken: fed by another command on the
    // same project, through a pipe or a named FIFO, waiting for the lock before reading would
    // hold both up for good. Any command's input that is such a stream is copied to a
    // temporary file first (#658); the commands below also read theirs here when it is a
    // regular file.
    let _spooled = spool_inputs(&mut cli.cmd)?;
    let dir = cli.project.as_path();
    let mut input_text = match &cli.cmd {
        Cmd::Rules(RulesCmd::Set { file }) | Cmd::ReportLayout(ReportLayoutCmd::Set { file, .. }) => {
            Some(read_input(file)?)
        }
        _ => None,
    };
    let mut import_bytes = match &cli.cmd {
        Cmd::Import(ImportCmd::Wago { file }) => {
            Some(std::fs::read(file).map_err(|x| format!("{}: {x}", file.display()))?)
        }
        Cmd::Import(ImportCmd::PlcCsv { file }) => Some(run::read_file(file)?),
        _ => None,
    };
    let _lock = match cli.cmd {
        Cmd::New { .. } => None,
        _ => project_lock(dir),
    };
    // Under the project lock, so a run still writing is never undone. `new` makes a project
    // elsewhere and `inspect` reads only an archive: the folder `-C` names is not their project.
    if !matches!(cli.cmd, Cmd::New { .. } | Cmd::Inspect { .. })
        && let Some(note) = undo_stopped_box_edit(dir)?
    {
        // A warning that cannot be shown does not stop the command.
        let _ = std::io::Write::write_all(&mut std::io::stderr(), format!("warning: {note}\n").as_bytes());
    }
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
                    // A template a box-edit run stopped half way in, local or cloned, is refused
                    // by `create_from` (#653).
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
                    check_plate_size(width, height)?;
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
                PanelCmd::Edit {
                    id,
                    name,
                    width,
                    height,
                } => {
                    if name.is_none() && width.is_none() && height.is_none() {
                        return Err("nothing to change: give --name, --width or --height".into());
                    }
                    let Some(panel) = p.panels.iter_mut().find(|x| x.id == id) else {
                        return Err(format!("no panel {id:?}"));
                    };
                    let (w, h) = (width.unwrap_or(panel.width), height.unwrap_or(panel.height));
                    // A name-only edit leaves the plate alone, so it still works when something
                    // already sticks out past an edge (as `panel arrange` can leave it).
                    if width.is_some() || height.is_some() {
                        check_plate_size(w, h)?;
                        panel.resize(w, h)?;
                    }
                    if let Some(name) = name {
                        panel.name = if name.is_empty() { id.clone() } else { name };
                    }
                    println!(
                        "panel {id}: {} ({} x {} mm)",
                        escape_field(&panel.name),
                        panel.width,
                        panel.height
                    );
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
                    write_out(&out, &step.text).map_err(e)?;
                    println!(
                        "wrote {} ({} solids)",
                        out.display(),
                        step.text.matches("MANIFOLD_SOLID_BREP(").count()
                    );
                    if !step.default_depth.is_empty() {
                        let line = format!(
                            "warning: no Depth on the part, drawn {} mm deep: {}\n",
                            wc_report::step::DEFAULT_DEPTH,
                            step.default_depth.join(", ")
                        );
                        // A warning that cannot be shown does not stop the export: it goes to
                        // standard output instead, with the reason, as `logged_to` does.
                        if let Err(why) = std::io::Write::write_all(&mut std::io::stderr(), line.as_bytes()) {
                            // Nothing is left to report this to when standard output fails as well.
                            let note = format!("warning: could not print on standard error: {why}\n{line}");
                            let _ = std::io::Write::write_all(&mut std::io::stdout(), note.as_bytes());
                        }
                    }
                    return Ok(());
                }
                PanelCmd::Device {
                    id,
                    tag,
                    at,
                    rotate,
                    remove,
                } => {
                    let tag = DeviceTag::parse(&tag).map_err(e)?;
                    let known = p.devices.contains_key(&tag)
                        || p.pages
                            .iter()
                            .any(|pg| pg.symbols.iter().any(|s| s.tag.as_ref() == Some(&tag)));
                    let size = match device_part_size(dir, &p, &tag) {
                        Ok(size) => size,
                        Err(warning) => {
                            eprintln!("warning: {warning}");
                            None
                        }
                    };
                    let Some(panel) = p.panels.iter_mut().find(|x| x.id == id) else {
                        return Err(format!("no panel {id:?}"));
                    };
                    if remove {
                        if !panel.remove_device(&tag) {
                            return Err(format!("{tag} is not on panel {id:?}"));
                        }
                        println!("removed {tag} from {id}");
                    } else {
                        let here = match panel.device(&tag) {
                            Some(wc_core::panel::PanelItem::Device {
                                at,
                                width,
                                height,
                                rotation,
                                ..
                            }) => Some((*at, (*width, *height), *rotation)),
                            _ => None,
                        };
                        if here.is_none() && !known {
                            return Err(format!("no device {tag} in the project"));
                        }
                        let Some(at) = at.or(here.map(|h| h.0)) else {
                            return Err(format!("{tag} is not on panel {id:?}: give --at x,y"));
                        };
                        let rotation = match rotate {
                            Some(d) => Rotation::from_degrees(d).ok_or("rotation must be 0, 90, 180 or 270")?,
                            None => here.map(|h| h.2).unwrap_or_default(),
                        };
                        let estimated = size.is_none() && here.is_none();
                        let size = size.or(here.map(|h| h.1)).unwrap_or(wc_core::panel::DEFAULT_SIZE);
                        panel.place_device(tag.clone(), at, size, rotation)?;
                        println!(
                            "{tag} on {id} at {},{} ({} x {} mm, turned {})",
                            at.x,
                            at.y,
                            size.0,
                            size.1,
                            rotation.degrees()
                        );
                        if estimated {
                            eprintln!("warning: no Width/Height on the part, drawn 18 x 90 mm: {tag}");
                        }
                    }
                }
                PanelCmd::Rail { id, at, length, remove } => {
                    let Some(panel) = p.panels.iter_mut().find(|x| x.id == id) else {
                        return Err(format!("no panel {id:?}"));
                    };
                    if let Some(n) = remove {
                        remove_rail_or_duct(panel, true, n)?;
                        println!("removed rail {n} from {id}");
                    } else {
                        let (Some(at), Some(length)) = (at, length) else {
                            return Err("give --at x,y and --length".into());
                        };
                        let rail = panel_rail(panel, at, length)?;
                        panel.items.push(rail);
                        println!("rail on {id} at {},{}, {length} mm long", at.x, at.y);
                    }
                }
                PanelCmd::Duct {
                    id,
                    at,
                    length,
                    width,
                    vertical,
                    remove,
                } => {
                    let Some(panel) = p.panels.iter_mut().find(|x| x.id == id) else {
                        return Err(format!("no panel {id:?}"));
                    };
                    if let Some(n) = remove {
                        remove_rail_or_duct(panel, false, n)?;
                        println!("removed duct {n} from {id}");
                    } else {
                        let (Some(at), Some(length)) = (at, length) else {
                            return Err("give --at x,y and --length".into());
                        };
                        let (w, h) = if vertical { (width, length) } else { (length, width) };
                        let duct = panel_duct(panel, at, w, h)?;
                        panel.items.push(duct);
                        println!("duct on {id} at {},{}, {w} x {h} mm", at.x, at.y);
                    }
                }
                PanelCmd::Show { id } => {
                    let Some(panel) = p.panels.iter().find(|x| x.id == id) else {
                        return Err(format!("no panel {id:?}"));
                    };
                    print!("{}", panel_show(panel));
                    return Ok(());
                }
                PanelCmd::List => {
                    for x in &p.panels {
                        println!(
                            "{:<10} {} ({} x {} mm, {} device(s))",
                            x.id,
                            escape_field(&x.name),
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
                    if !p.pages.iter().any(|pg| pg.id == id) {
                        return Err(format!("no page {id:?}"));
                    }
                    // Same rule as the app: a project keeps at least one page.
                    if p.pages.len() <= 1 {
                        return Err("A project needs at least one page.".into());
                    }
                    p.pages.retain(|pg| pg.id != id);
                }
                PageCmd::Overview { id, off } => {
                    let page = p
                        .pages
                        .iter_mut()
                        .find(|pg| pg.id == id)
                        .ok_or_else(|| format!("no page {id:?}"))?;
                    page.overview = !off;
                }
                PageCmd::Edit { id, number, title } => {
                    if number.is_none() && title.is_none() {
                        return Err("give --number, --title or both".into());
                    }
                    let i = p
                        .pages
                        .iter()
                        .position(|pg| pg.id == id)
                        .ok_or_else(|| format!("no page {id:?}"))?;
                    let number = number.map(|n| n.trim().to_owned());
                    if let Some(n) = &number {
                        if n.is_empty() {
                            return Err("the page number cannot be empty".into());
                        }
                        if let Some(other) = p.pages.iter().find(|pg| pg.number == *n && pg.id != id) {
                            return Err(format!("page {:?} already has number {n:?}", other.id));
                        }
                    }
                    if let Some(page) = p.pages.get_mut(i) {
                        if let Some(n) = number {
                            page.number = n;
                        }
                        if let Some(t) = title {
                            page.title = t.trim().to_owned();
                        }
                    }
                    p.sort_pages();
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
            // Check the inputs before taking the symbol, so a bad page, tag or turn leaves no copy
            // behind.
            page_mut(&mut p, &page)?;
            let tag = tag.map(|t| DeviceTag::parse(&t)).transpose().map_err(e)?;
            let rotation = Rotation::from_degrees(rotate).ok_or("rotation must be 0, 90, 180 or 270")?;
            if store::read_vendored_symbol(dir, &symbol).is_err() {
                let lib = wc_library::Library::open(dir, &p).map_err(e)?;
                wc_library::take(dir, &mut p, &lib, ItemKind::Symbol, &symbol).map_err(e)?;
            }
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
        Cmd::BoxEdit {
            page,
            id,
            size,
            pins,
            clear_pins,
            renames,
            rename_pairs,
        } => {
            let mut p = load(dir)?;
            let pins = pins.iter().map(|s| parse_box_pin(s)).collect::<Res<Vec<_>>>()?;
            let mut renamed = std::collections::BTreeMap::new();
            let mut pairs = Vec::new();
            for r in &renames {
                pairs.push(
                    r.split_once('=')
                        .ok_or_else(|| format!("{r:?}: expected OLD=NEW, e.g. OUT=Q"))?,
                );
            }
            pairs.extend(
                rename_pairs
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|[a, b]| (a.as_str(), b.as_str())),
            );
            for (from, to) in pairs {
                if renamed.insert(from.trim().to_owned(), to.trim().to_owned()).is_some() {
                    return Err(format!(
                        "connection point {} is renamed twice",
                        escape_field(from.trim())
                    ));
                }
            }
            let pg = p.page(&page).ok_or_else(|| format!("no page {page:?}"))?;
            let symbol_id = pg
                .symbol(&id)
                .ok_or_else(|| format!("no symbol {id:?} on page {page}"))?
                .symbol
                .clone();
            let old = store::read_vendored_symbol(dir, &symbol_id).map_err(e)?;
            let Some((w, h, old_pins)) = wc_core::black_box_layout(&old) else {
                return Err(format!("{id} is not a black box"));
            };
            match p.lock.symbols.get(&symbol_id) {
                None => {
                    return Err(format!(
                        "the project does not pin symbol {symbol_id}; put its entry in {} back from the project's history (e.g. `git checkout`), or run `wiringcraft lib lock` when a configured library has the symbol",
                        store::LOCK_FILE
                    ));
                }
                Some(l) if l.hash != old.hash() => {
                    return Err(format!(
                        "the file of symbol {symbol_id} is not the version the project pins (changed outside wiringcraft?); put the pinned version back from the project's history (e.g. `git checkout`) before editing the box"
                    ));
                }
                Some(_) => {}
            }
            if let Some(from) = renamed.keys().find(|f| old.pin(f).is_none()) {
                return Err(format!("the box has no connection point {from} to rename"));
            }
            // Two points renamed to one name would join their wires into one connection.
            let mut targets = std::collections::BTreeSet::new();
            if let Some(to) = renamed.values().find(|to| !targets.insert(to.as_str())) {
                return Err(format!("two connection points are renamed to {to}"));
            }
            let (width, height) = size.map_or((w, h), |s| (s.x, s.y));
            // As `wc_core::black_box` asks, checked before the points are fitted to the size.
            if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
                return Err(format!(
                    "a black box needs a width and height above zero, not {width} by {height}"
                ));
            }
            let pins = if !pins.is_empty() || clear_pins {
                pins
            } else {
                // The points stay (renamed as asked), each kept on its edge of the new size.
                wc_core::box_edit::kept_pins(&old_pins, &renamed, width, height)
            };
            // Every rename lands on a point of the new list, a name kept as it is (`A=A`) too: else
            // the point would be taken off and its wires cut loose.
            if let Some(to) = renamed
                .values()
                .find(|to| !pins.iter().any(|b| b.name.trim() == to.as_str()))
            {
                return Err(format!(
                    "the box has no connection point {} to rename a point to",
                    escape_field(to)
                ));
            }
            let page_index = p
                .pages
                .iter()
                .position(|pg| pg.id == page)
                .ok_or_else(|| format!("no page {page:?}"))?;
            let removed = std::collections::BTreeSet::new();
            let edit = wc_core::box_edit::BoxEdit {
                page: page_index,
                item: &id,
                size: (width, height),
                pins: &pins,
                renamed: &renamed,
                removed: &removed,
                moved: Point::new(0.0, 0.0),
            };
            // The reason may name points from the project file: escaped for the terminal.
            let plan = wc_core::box_edit::plan(&p, &old, &edit).map_err(|err| escape_field(&err.to_string()))?;
            let Some(plan) = plan else {
                // Nothing changes: the files stay as they are.
                return Ok(());
            };
            plan.apply_ends(&mut p);
            let wc_core::box_edit::BoxEditPlan {
                symbol: new, warnings, ..
            } = plan;
            p.lock.symbols.insert(
                symbol_id.clone(),
                wc_core::LockEntry {
                    source: "project".into(),
                    hash: new.hash(),
                },
            );
            // As for `box`: a save that fails half way puts back the project files and the
            // box's symbol file.
            let before = snapshot_project(dir, &[], &[])?;
            let (symbol_path, symbol_before) = vendored_bytes(dir, ItemKind::Symbol, &symbol_id)?;
            // A link would be replaced by a plain file, and a failed save could not put it back.
            if is_link(&symbol_path)? {
                return Err(format!(
                    "the file of symbol {symbol_id} is a link; box-edit does not replace it (copy its target over it first)"
                ));
            }
            // The same for the project's own files (pages, panels, lock, ...): both the save and
            // the rollback would put a plain file where the link was (#614), so refuse before any
            // write.
            let mut linked: Vec<String> = Vec::new();
            for (path, _) in &before {
                if is_link(path)? {
                    linked.push(escape_field(&path.display().to_string()));
                }
            }
            if !linked.is_empty() {
                return Err(format!(
                    "{} a link; box-edit does not replace project files that are links (copy each target over its link first)",
                    if linked.len() == 1 {
                        format!("{} is", linked[0])
                    } else {
                        format!("{} are", linked.join(", "))
                    }
                ));
            }
            // A run that stops between these writes leaves the journal: the next run puts the
            // files back from it (#653).
            let mut journaled = before.clone();
            journaled.push((symbol_path.clone(), symbol_before.clone()));
            let journal = Journal::begin(dir, &journaled)?;
            let written = store::write_vendored_symbol(dir, &new).map_err(e).and_then(|_| {
                // Tests stop the run here, as a kill between the writes would.
                #[cfg(test)]
                if tests::STOP_BOX_EDIT_AFTER_SYMBOL.get() {
                    return Err(STOPPED.to_owned());
                }
                save(dir, &p)
            });
            #[cfg(test)]
            if written.as_ref().is_err_and(|err| err == STOPPED) {
                // The journal stays behind, no longer locked, as after a kill.
                drop(journal);
                return written;
            }
            if let Err(err) = written {
                let mut failed = put_files_back(&before);
                failed.extend(put_file_back(&symbol_path, symbol_before.as_deref()).err());
                // The files put back are on disk before the journal goes, or it stays.
                if failed.is_empty() {
                    sync_folders(&journaled).map_err(|x| format!("{err}; the project files are put back, but {x}"))?;
                    journal.finish(dir).map_err(|x| format!("{err}; {x}"))?;
                    return Err(err);
                }
                // The journal stays, so the next run tries again.
                return Err(format!(
                    "{err}; putting back the project files failed too: {}",
                    failed.join("; ")
                ));
            }
            journal.finish(dir).map_err(|x| format!("the edit is saved, but {x}"))?;
            for w in warnings {
                eprintln!("warning: {}", escape_field(&w));
            }
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
            let from = parse_end(dir, pg, &from)?;
            let to = parse_end(dir, pg, &to)?;
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
        Cmd::Delete { page, ids } => {
            let mut p = load(dir)?;
            let pg = page_mut(&mut p, &page)?;
            let deleted = delete_items(pg, &ids)?;
            save(dir, &p)?;
            for id in deleted {
                println!("{}", escape_field(&id));
            }
        }
        Cmd::Location(lc) => {
            let mut p = load(dir)?;
            match lc {
                LocationCmd::List => {
                    write_stdout(|out| write_locations(out, &p.meta.locations));
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
            let mut found: Vec<FoundMacro> = wc_core::macros::list_macros(dir)
                .into_iter()
                .map(|(id, path)| FoundMacro {
                    source: "project".to_owned(),
                    id,
                    path,
                    project: true,
                })
                .collect();
            // A library that cannot be opened is reported; the project's macros stay usable.
            match wc_library::Library::open(dir, &p) {
                Ok(lib) => found.extend(lib.macros().into_iter().map(|(source, id, path)| FoundMacro {
                    source,
                    id,
                    path,
                    project: false,
                })),
                Err(err) => eprintln!("warning: the library's macros are not listed: {err}"),
            }
            match mc {
                MacroCmd::List => {
                    for FoundMacro { source, id, path, .. } in &found {
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
                MacroCmd::Show { name } => {
                    let f = find_macro(&found, &name)?;
                    let m = wc_core::macros::read_macro(&f.path).map_err(e)?;
                    print!("{}", macro_text(&m, &f.source));
                }
                MacroCmd::Edit {
                    name,
                    description,
                    rename,
                    placeholder,
                    remove,
                    set,
                    remove_set,
                } => {
                    let f = find_macro(&found, &name)?;
                    if !f.project {
                        return Err(format!(
                            "macro {name:?} belongs to the {} library; only the project's own macros are edited",
                            f.source
                        ));
                    }
                    let mut m = wc_core::macros::read_macro(&f.path).map_err(e)?;
                    edit_macro(&mut m, description, &rename, &placeholder, &remove, &set, &remove_set)?;
                    wc_core::macros::write_macro_file(&f.path, &m).map_err(e)?;
                    print!("{}", macro_text(&m, &f.source));
                }
                MacroCmd::Place {
                    page,
                    name,
                    at,
                    set,
                    values,
                } => {
                    let path = find_macro(&found, &name)?.path.clone();
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
        Cmd::Rules(rc) => {
            let mut p = load(dir)?;
            let library = wc_library::library_rules(dir, &p);
            match rc {
                RulesCmd::List { json } => {
                    let text = rules_list(&p, &library, json)?;
                    print!("{text}");
                }
                RulesCmd::Set { file } => {
                    let rules = rules_from_file(&file, input_text.take())?;
                    let n = rules.len();
                    if rules != p.meta.rules {
                        p.meta.rules = rules;
                        save(dir, &p)?;
                    }
                    println!("{n} check rules");
                }
                RulesCmd::Off { rule } => turn_library_rule(dir, &mut p, &library, &rule, true)?,
                RulesCmd::On { rule } => turn_library_rule(dir, &mut p, &library, &rule, false)?,
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
        Cmd::ReportLayout(rc) => report_layout(dir, rc, input_text.take())?,
        Cmd::Jumper(jc) => {
            let mut p = load(dir)?;
            match jc {
                JumperCmd::Add { strip, terminals } => {
                    let strip = DeviceTag::parse(&strip).map_err(e)?;
                    let (symbols, unreadable) = store::vendored_symbols(dir, &p);
                    let strips = wc_core::project::strip_terminals(&p, |id| symbols.get(id).map(|s| s.role));
                    let Some(rail) = strips.get(&strip) else {
                        return Err(no_strip(&strip, &unreadable));
                    };
                    // Every terminal must be on the strip, drawn or spare, as `strip move` checks.
                    for t in terminals.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
                        if !rail.iter().any(|(n, _)| n == t) {
                            return Err(format!("{strip} has no terminal {}", escape_field(t)));
                        }
                    }
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
        Cmd::Rack(rc) => {
            let mut p = load(dir)?;
            if let RackCmd::List = rc {
                print!("{}", rack_list(&p.meta.racks));
                return Ok(());
            }
            if edit_racks(&mut p.meta.racks, rc)? {
                save(dir, &p)?;
            }
            let (symbols, _) = store::vendored_symbols(dir, &p);
            let parts = vendored_parts_warning_unreadable(dir, &p);
            let inputs = wc_report::Inputs {
                project: &p,
                symbols: &symbols,
                parts: &parts,
            };
            for w in wc_report::plc_rack_warnings(&inputs) {
                // Rack names and tags come from the command line or the project file.
                eprintln!("warning: {}", escape_field(&w));
            }
        }
        Cmd::Strip(sc) => {
            let mut p = load(dir)?;
            let (symbols, unreadable) = store::vendored_symbols(dir, &p);
            let rail = |p: &Project, strip: &DeviceTag| -> Vec<(String, bool)> {
                wc_core::project::strip_terminals(p, |id| symbols.get(id).map(|s| s.role))
                    .remove(strip)
                    .unwrap_or_default()
            };
            match sc {
                StripCmd::List { strip } => {
                    let only = strip.map(|s| DeviceTag::parse(&s).map_err(e)).transpose()?;
                    let all = wc_core::project::strip_terminals(&p, |id| symbols.get(id).map(|s| s.role));
                    if let Some(only) = &only
                        && !all.contains_key(only)
                    {
                        return Err(no_strip(only, &unreadable));
                    }
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
            if part.is_none() && description.is_none() {
                let info = device_info(&p, &t)?;
                print!("{info}");
                return Ok(());
            }
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
        Cmd::Import(ic) => import_cmd(dir, ic, import_bytes.take())?,
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
            check_pages(&p, &pages)?;
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
            warn_levels(&p, &kinds);
            // The network list is followed by a topology page per network.
            let networks = if kinds.contains(&wc_report::Kind::Networks) {
                p.meta.networks.clone()
            } else {
                Vec::new()
            };
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
                    networks,
                    footprints: wc_pdf::footprints(vendored_parts(dir, &p).values()),
                    plc_cards,
                },
            );
            if report.pages == 0 {
                return Err(wc_pdf::NOTHING_TO_EXPORT.into());
            }
            write_out(&out, bytes).map_err(e)?;
            for w in &report.warnings {
                eprintln!("warning: {}", escape_field(w));
            }
            println!("wrote {} ({} pages)", out.display(), report.pages);
        }
        Cmd::Translations { lang, edit } => {
            let lang = lang.ok_or("--lang is needed, such as --lang nb")?;
            // Only a project folder has a dictionary: a folder without one is refused before
            // translations.toml is made or changed there.
            let p = load(dir)?;
            if let Some(edit) = edit {
                let (text, translation) = match &edit {
                    TranslationsCmd::Set { text, translation } => {
                        if translation.trim().is_empty() {
                            return Err("the translation is empty: use `translations remove` to take one away".into());
                        }
                        (text, translation.as_str())
                    }
                    TranslationsCmd::Remove { text } => (text, ""),
                };
                let changed = wc_core::translate::set_translation(dir, text, &lang, translation).map_err(e)?;
                let file = wc_core::translate::TRANSLATIONS_FILE;
                println!(
                    "{}",
                    match (&edit, changed) {
                        (TranslationsCmd::Set { .. }, true) => format!("set the translation in {file}"),
                        (TranslationsCmd::Remove { .. }, true) => format!("removed the translation from {file}"),
                        _ => format!("{file} unchanged"),
                    }
                );
                return Ok(());
            }
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
            check_pages(&p, &pages)?;
            let (symbols, missing) = store::vendored_symbols(dir, &p);
            for m in missing {
                eprintln!("warning: symbol {m} has no vendored copy (run `wiringcraft lib lock`)");
            }
            // Panel layouts follow the pages, one file each, as their PDF sheets.
            let options = wc_pdf::Options {
                pages,
                date,
                panels: p.panels.clone(),
                // A part that cannot be read is named in a warning; the others still export.
                footprints: if p.panels.is_empty() {
                    Default::default()
                } else {
                    wc_pdf::footprints(vendored_parts_warning_unreadable(dir, &p).values())
                },
                ..Default::default()
            };
            // Only SVG shows pictures; a DXF export neither reads nor checks them.
            let (drawings, report) = if ext == "svg" {
                let pictures = wc_pdf::read_pictures(dir, &p);
                let options = wc_pdf::Options {
                    underlays: pictures.files,
                    unreadable: pictures.unreadable,
                    ..options
                };
                wc_pdf::page_drawings(&p, &symbols, &options)
            } else {
                wc_pdf::page_lines(&p, &symbols, &options)
            };
            make_out_dir(&out)?;
            let mut used = std::collections::BTreeSet::new();
            for d in &drawings {
                // A panel is named after itself: `panel-Cabinet_1`.
                let mut stem = if d.panel {
                    wc_pdf::panel_file_stem(d)
                } else {
                    wc_pdf::page_file_stem(d)
                };
                // Compared without case: `A1` and `a1` are one file name on Windows and macOS.
                while !used.insert(stem.to_lowercase()) {
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
            let (parts, unreadable) = vendored_parts_and_unreadable(dir, &p);
            let inputs = wc_report::Inputs {
                project: &p,
                symbols: &symbols,
                parts: &parts,
            };
            let found = wc_report::check_with_unreadable(&inputs, &wc_library::library_rules(dir, &p), &unreadable);
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
        Cmd::Report { kind, format, out, sep } => {
            if !matches!(sep, ',' | ';') {
                return Err(format!("unknown separator {sep:?} (, or ;)"));
            }
            if sep != ',' && format != "csv" {
                return Err("--sep is for --format csv".into());
            }
            // Excel reads a `;` file as the comma-decimal locales write it, and needs the byte
            // order mark to take it as UTF-8; standard output stays without one.
            let csv_file = |t: &wc_report::Table| {
                let bom = if sep == ';' { "\u{feff}" } else { "" };
                format!("{bom}{}", t.to_csv_with(sep))
            };
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
            warn_levels(&p, &kinds);
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
            // Several tables in one CSV file is not a CSV any reader takes: `--out` names a
            // folder then, with one `<report>.csv` in it per report, as `drawing` does per page.
            if format == "csv"
                && kinds.len() > 1
                && let Some(folder) = &out
            {
                make_out_dir(folder)?;
                for k in &kinds {
                    let t = wc_report::build(*k, &inputs);
                    write_out(folder.join(format!("{}.csv", k.name())), csv_file(&t)).map_err(e)?;
                }
                println!("wrote {} csv file(s) to {}", kinds.len(), folder.display());
                return Ok(());
            }
            let mut text = String::new();
            if format == "csv" && sep == ';' && out.is_some() {
                text.push('\u{feff}');
            }
            for k in kinds {
                let t = wc_report::build(k, &inputs);
                text.push_str(&match format.as_str() {
                    "csv" => t.to_csv_with(sep),
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
            let file_name = out
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (text, counts) = wc_report::automationml(&inputs, &utc_now(), &file_name);
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
            let bytes = read_file(&file)?;
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
            for (k, id) in &left.restored {
                let kind = if *k == ItemKind::Symbol { "symbol" } else { "part" };
                println!("restored the vendored copy of {kind} {}", escape_field(id));
            }
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

/// `input`: the file of `import wago` or `import plc-csv`, read already (before the project
/// lock).
fn import_cmd(dir: &Path, ic: ImportCmd, input: Option<Vec<u8>>) -> Res<()> {
    match ic {
        ImportCmd::SymbolDxf {
            file,
            id,
            name,
            lib,
            category,
            letter_code,
        } => {
            let text = read_file(&file)?;
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
            let text = read_file(&file)?;
            let (footprint, warnings) = wc_import::dxf::footprint(&String::from_utf8_lossy(&text)).map_err(e)?;
            for w in &warnings {
                eprintln!("warning: {}", escape_field(w));
            }
            update_library_part(
                dir,
                &part,
                lib.as_deref(),
                "footprint",
                |item| {
                    let summary = format!(
                        "{} x {} mm, {} shapes",
                        wc_core::geom::format_length(footprint.width),
                        wc_core::geom::format_length(footprint.height),
                        footprint.shapes.len()
                    );
                    item.footprint = Some(footprint);
                    summary
                },
                |a, b| a.footprint == b.footprint,
            )?;
        }
        ImportCmd::ModelStep { file, part, lib } => {
            let text = read_file(&file)?;
            let (size, warnings) = wc_import::step::model_size(&String::from_utf8_lossy(&text)).map_err(e)?;
            for w in &warnings {
                eprintln!("warning: {}", escape_field(w));
            }
            update_library_part(
                dir,
                &part,
                lib.as_deref(),
                "3D model size",
                |item| {
                    item.model = Some(size);
                    format!(
                        "{} x {} x {} mm",
                        wc_core::geom::format_length(size.width),
                        wc_core::geom::format_length(size.height),
                        wc_core::geom::format_length(size.depth)
                    )
                },
                |a, b| a.model == b.model,
            )?;
        }
        ImportCmd::SymbolSvg {
            file,
            id,
            name,
            lib,
            category,
            letter_code,
        } => {
            let text = read_text(&file)?;
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
            let text = read_text(&file)?;
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
            let done = match input {
                Some(bytes) => import_wago_bytes(dir, &bytes)?,
                None => import_wago(dir, &file)?,
            };
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
            logged(|log| match &input {
                Some(bytes) => run::plc_csv_bytes(dir, bytes, log),
                None => run::plc_csv(dir, &file, log),
            })?;
        }
        ImportCmd::PartsBmecat { file, lib } => {
            logged(|log| run::parts_bmecat(&file, &lib, log))?;
        }
        ImportCmd::PartsEdz { file, lib, symbol_map } => {
            logged(|log| run::parts_edz(&file, &lib, symbol_map.as_deref(), log))?;
        }
        ImportCmd::PartsCsv { file, lib } => {
            logged(|log| run::parts_csv(&file, &lib, log))?;
        }
        ImportCmd::Pdf { file, first_page } => {
            logged(|log| run::pdf(dir, &file, first_page, log))?;
        }
        ImportCmd::Eplan { path, first_page } => {
            logged(|log| run::eplan(dir, &path, first_page, log))?;
        }
        ImportCmd::Swe {
            path,
            pick,
            first_page,
            symbol_map,
        } => {
            logged(|log| run::swe(dir, &path, pick, first_page, symbol_map.as_deref(), log))?;
        }
        ImportCmd::SweLibrary { path, lib } => {
            logged(|log| run::swe_library(dir, &path, lib.as_deref(), log))?;
        }
        ImportCmd::Aml { file, first_page } => {
            logged(|log| run::aml(dir, &file, first_page, log))?;
        }
        ImportCmd::Archive { file, first_page, lib } => {
            logged(|log| run::archive(dir, &file, first_page, lib.as_deref(), log))?;
        }
        ImportCmd::PageDwg { file, page, scale } => {
            import_page_dwg(dir, &file, page, scale, std::env::var("WIRINGCRAFT_DWG2DXF").ok())?;
        }
        ImportCmd::PageDxf { file, page, scale } => {
            logged(|log| run::page_dxf(dir, &file, &page, scale, log))?;
        }
    }
    Ok(())
}

/// Change part `part` in its library file (in `lib`, else the project's library source that has
/// it) with `set`, which returns a summary of what it wrote, and bring the project's pinned
/// copy along when the project uses the part. `what` names the change in messages ("footprint");
/// `has_change` tells whether a copy taken has the change of the part written.
fn update_library_part(
    dir: &Path,
    part: &str,
    lib: Option<&Path>,
    what: &str,
    set: impl FnOnce(&mut wc_core::Part) -> String,
    has_change: impl Fn(&wc_core::Part, &wc_core::Part) -> bool,
) -> Res<()> {
    let part = part.to_owned();
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
                .ok_or_else(|| format!("no part {part:?} in the project's libraries; name its folder with --lib"))?
                .path
        }
    };
    // The project's own pinned copy is not a library: changing it there would leave its pin
    // naming other bytes.
    let vendored = store::vendor_path(dir, ItemKind::Part, &part).map_err(e)?;
    if let (Ok(a), Ok(b)) = (path.canonicalize(), vendored.canonicalize())
        && a == b
    {
        return Err(format!(
            "{} is the project's pinned copy of {part}, not a library; name the library folder the part comes from",
            path.display()
        ));
    }
    let mut item: wc_core::Part = store::read_json(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    if item.id != part {
        return Err(format!("{} holds part {:?}, not {part:?}", path.display(), item.id));
    }
    let summary = set(&mut item);
    store::write_if_changed(&path, &store::to_json(&item).map_err(e)?).map_err(e)?;
    println!("wrote the {what} of {part} to {} ({summary})", path.display());
    // The project's pinned copy follows, so its layouts draw the new footprint, but only
    // from the file just written: when another of the project's sources wins for this
    // id, that source's part is not the one that changed.
    if let Ok(p) = &mut project
        && p.lock.parts.contains_key(&part)
        // A pin the project keeps for a part it no longer uses stays as it is.
        && p.used_parts().contains(&part)
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
                Ok(copy) if p.lock.parts.get(&part).is_some_and(|pin| pin.hash != copy.hash()) => Some(format!(
                    "it has local changes since it was pinned; remove or merge them, then take {part} again"
                )),
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
        // A link that points nowhere is there, not missing, and is refused like the rest.
        let before: Result<Option<Vec<u8>>, String> = match std::fs::read(&vendored) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && std::fs::symlink_metadata(&vendored).is_err() => {
                Ok(None)
            }
            Err(err) => Err(err.to_string()),
        };
        // The lock too, before `take`: if saving the new pin fails, it goes back to these
        // bytes. A lock that exists but cannot be read is never removed or replaced.
        let lock_path = dir.join(store::LOCK_FILE);
        let lock_before: Result<Option<Vec<u8>>, String> = match std::fs::read(&lock_path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.to_string()),
        };
        let refused = kept
            .or_else(|| {
                before
                    .as_ref()
                    .err()
                    .map(|err| format!("it cannot be read ({err}); fix or remove it, then take {part} again"))
            })
            .or_else(|| {
                lock_before.as_ref().err().map(|err| {
                    format!(
                        "{} cannot be read ({err}); fix it, then take {part} again",
                        store::LOCK_FILE
                    )
                })
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
                        if has_change(&copy, &item) {
                            Ok(())
                        } else {
                            Err(format!("the copy taken has another {what}"))
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
                // The new pin must reach the disk with the copy, or both go back: `save`
                // can fail after it wrote the lock.
                // Only the lock changed with the copy: unrelated project files cannot block it.
                if let Err(err) = store::save_lock(dir, p).map_err(e) {
                    // Both are tried, and every failure is named.
                    let copy_back = match &before {
                        Ok(bytes) => put_file_back(&vendored, bytes.as_deref()),
                        Err(_) => Ok(()),
                    };
                    let lock_back = match &lock_before {
                        Ok(bytes) => put_file_back(&lock_path, bytes.as_deref()),
                        Err(_) => Ok(()),
                    };
                    let back = match (copy_back, lock_back) {
                        (Ok(()), Ok(())) => Ok(()),
                        (Err(a), Ok(())) => Err(a),
                        (Ok(()), Err(b)) => Err(b),
                        (Err(a), Err(b)) => Err(format!("{a}; {b}")),
                    };
                    return Err(match back {
                        Ok(()) => format!("the project's copy of {part} was not updated: {err}"),
                        Err(why) => format!(
                            "the project's copy of {part} was not updated ({err}) and could not be put back: {why}; run `lib status`"
                        ),
                    });
                }
                println!("updated the project's copy of {part}");
            }
            Err(why) => {
                let restored = match &before {
                    _ if !tried => Ok(()),
                    Ok(bytes) => put_file_back(&vendored, bytes.as_deref()),
                    Err(_) => Ok(()),
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
    Ok(())
}

/// `rack list`: each rack on a line of its own, its cards under it by slot.
fn rack_list(racks: &[wc_core::plc_rack::PlcRack]) -> String {
    let mut out = String::new();
    for rack in racks {
        let mut line = escape_field(&rack.name);
        if !rack.description.is_empty() {
            line += &format!(" ({})", escape_field(&rack.description));
        }
        out += &line;
        out.push('\n');
        let mut cards: Vec<_> = rack.cards.iter().collect();
        cards.sort_by_key(|c| c.slot);
        for c in cards {
            let mut line = format!("  slot {}:", c.slot);
            if !c.tag.is_empty() {
                line += &format!(" {}", escape_field(&c.tag));
            }
            if !c.kind.is_empty() {
                line += &format!(" \"{}\"", escape_field(&c.kind));
            }
            line += &format!(" {} channels", c.channels);
            if !c.start.is_empty() {
                line += &format!(" from {}", escape_field(&c.start));
            }
            if let Some(step) = c.step {
                line += &format!(" step {step}");
            }
            out += &line;
            out.push('\n');
        }
    }
    out
}

/// `rack add`, `remove`, `set` and `rack card ...` on `racks`, refused with the PLC racks
/// form's range check ([`wc_core::plc_rack::problem`]); true when they changed.
fn edit_racks(racks: &mut Vec<wc_core::plc_rack::PlcRack>, cmd: RackCmd) -> Res<bool> {
    use wc_core::plc_rack::{PlcCard, PlcRack};
    let find = |racks: &[PlcRack], name: &str| -> Res<usize> {
        let name = name.trim();
        racks
            .iter()
            .position(|r| r.name.trim() == name)
            .ok_or_else(|| format!("there is no rack {}", escape_field(name)))
    };
    let taken = |racks: &[PlcRack], name: &str| -> Res<()> {
        if racks.iter().any(|r| r.name.trim() == name) {
            return Err(format!("there already is a rack {}", escape_field(name)));
        }
        Ok(())
    };
    let card_at = |rack: &PlcRack, slot: u32| -> Res<usize> {
        rack.cards
            .iter()
            .position(|c| c.slot == slot)
            .ok_or_else(|| format!("rack {} has no card in slot {slot}", escape_field(&rack.name)))
    };
    let slot_free = |rack: &PlcRack, slot: u32| -> Res<()> {
        if rack.cards.iter().any(|c| c.slot == slot) {
            return Err(format!(
                "rack {} already has a card in slot {slot}",
                escape_field(&rack.name)
            ));
        }
        Ok(())
    };
    let mut new = racks.clone();
    match cmd {
        RackCmd::List => {}
        RackCmd::Add { name, description } => {
            let name = name.trim();
            if name.is_empty() {
                return Err("a rack needs a name".into());
            }
            taken(&new, name)?;
            new.push(PlcRack {
                name: name.to_owned(),
                description: description.unwrap_or_default().trim().to_owned(),
                ..PlcRack::default()
            });
        }
        RackCmd::Remove { name } => {
            let i = find(&new, &name)?;
            new.remove(i);
        }
        RackCmd::Set {
            name,
            new_name,
            description,
        } => {
            let i = find(&new, &name)?;
            if let Some(to) = new_name.as_deref().map(str::trim) {
                if to.is_empty() {
                    return Err("a rack needs a name".into());
                }
                if new.get(i).is_some_and(|r| r.name.trim() != to) {
                    taken(&new, to)?;
                }
            }
            let Some(r) = new.get_mut(i) else {
                return Err(format!("there is no rack {}", escape_field(&name)));
            };
            if let Some(to) = new_name.as_deref().map(str::trim) {
                r.name = to.to_owned();
            }
            if let Some(d) = description {
                r.description = d.trim().to_owned();
            }
        }
        RackCmd::Card(RackCardCmd::Add {
            rack,
            slot,
            tag,
            kind,
            channels,
            start,
            step,
        }) => {
            let i = find(&new, &rack)?;
            let Some(r) = new.get_mut(i) else {
                return Err(format!("there is no rack {}", escape_field(&rack)));
            };
            slot_free(r, slot)?;
            r.cards.push(PlcCard {
                slot,
                tag: tag.unwrap_or_default().trim().to_owned(),
                kind: kind.unwrap_or_default().trim().to_owned(),
                channels,
                start: start.unwrap_or_default().trim().to_owned(),
                step,
                ..PlcCard::default()
            });
        }
        RackCmd::Card(RackCardCmd::Set {
            rack,
            slot,
            new_slot,
            tag,
            kind,
            channels,
            start,
            step,
            no_step,
        }) => {
            let i = find(&new, &rack)?;
            let Some(r) = new.get_mut(i) else {
                return Err(format!("there is no rack {}", escape_field(&rack)));
            };
            let c = card_at(r, slot)?;
            let to = new_slot.filter(|&to| to != slot);
            if let Some(to) = to {
                slot_free(r, to)?;
            }
            let Some(card) = r.cards.get_mut(c) else {
                return Err(format!("rack {} has no card in slot {slot}", escape_field(&r.name)));
            };
            if let Some(to) = to {
                card.slot = to;
            }
            if let Some(t) = tag {
                card.tag = t.trim().to_owned();
            }
            if let Some(k) = kind {
                card.kind = k.trim().to_owned();
            }
            if let Some(n) = channels {
                card.channels = n;
            }
            if let Some(a) = start {
                card.start = a.trim().to_owned();
            }
            if step.is_some() {
                card.step = step;
            }
            if no_step {
                card.step = None;
            }
        }
        RackCmd::Card(RackCardCmd::Remove { rack, slot }) => {
            let i = find(&new, &rack)?;
            let Some(r) = new.get_mut(i) else {
                return Err(format!("there is no rack {}", escape_field(&rack)));
            };
            let c = card_at(r, slot)?;
            r.cards.remove(c);
        }
    }
    if let Some(why) = wc_core::plc_rack::problem(&new) {
        return Err(format!("the racks were not changed: {}", escape_field(&why)));
    }
    if new == *racks {
        return Ok(false);
    }
    *racks = new;
    Ok(true)
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

/// Warns about each terminal `Level` value that is not a level, when `kinds` has the terminal
/// plan: such a terminal shows no level there.
fn warn_levels(project: &wc_core::Project, kinds: &[wc_report::Kind]) {
    if !kinds.contains(&wc_report::Kind::Terminals) {
        return;
    }
    use std::io::Write as _;
    let mut err = std::io::stderr().lock();
    for w in wc_report::level_warnings(project) {
        // Tags and values come from the project file. A warning that cannot be written (stderr
        // closed or full) is dropped: it must not stop the export.
        let _ = writeln!(err, "warning: {}", escape_field(&w));
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
        .chain(left.unrestorable.iter().map(|(k, id, why)| {
            format!(
                "{} {id} has no readable vendored copy and was not restored: {why}",
                kind(k)
            )
        }))
        .collect()
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

/// What a box-edit run a test stops between its writes returns.
#[cfg(test)]
const STOPPED: &str = "stopped between the writes";

/// The file `box-edit` keeps in the project folder while it writes: the bytes every file it may
/// change had before (#653). It is removed once the last file is written; one left behind means
/// a run stopped half way, and the next run puts the files back from it
/// ([`undo_stopped_box_edit`]).
use store::BOX_EDIT_JOURNAL;
const JOURNAL_HEADER: &[u8] = b"wiringcraft box-edit journal 2\n";

/// A journal written and held by this run. The file stays locked while the run writes, so
/// another run never takes it for one a stopped run left behind.
struct Journal {
    path: PathBuf,
    file: std::fs::File,
}

impl Journal {
    /// Write the journal of `files` (each a project file box-edit may change, with its bytes
    /// before, `None` when it does not exist yet), durably, before the first file is changed.
    fn begin(dir: &Path, files: &[(PathBuf, Option<Vec<u8>>)]) -> Res<Self> {
        use std::io::Write as _;
        let path = dir.join(BOX_EDIT_JOURNAL);
        let shown = |p: &Path| escape_field(&p.display().to_string());
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(format!(
                "{} is there: another box-edit run on this project has not finished; run again when it has",
                shown(&path)
            ));
        }
        let mut bytes = JOURNAL_HEADER.to_vec();
        for (file, before) in files {
            let rel = journal_name(dir, file)
                .ok_or_else(|| format!("{} is not a file box-edit keeps a copy of", shown(file)))?;
            journal_target_ok(dir, &rel)?;
            match before {
                Some(b) => bytes.extend_from_slice(format!("{} {}\n", rel.len(), b.len()).as_bytes()),
                None => bytes.extend_from_slice(format!("{} -\n", rel.len()).as_bytes()),
            }
            bytes.extend_from_slice(rel.as_bytes());
            bytes.extend_from_slice(before.as_deref().unwrap_or_default());
        }
        // The end, with the count: a journal cut short anywhere is refused, never half applied.
        bytes.extend_from_slice(format!("end {}\n", files.len()).as_bytes());
        // Written beside it under a name of its own, locked, then moved into place: the journal
        // is either whole or not there.
        let mut part = path.as_os_str().to_owned();
        // Ending in `.tmp`, so a project's git ignore rule keeps one a stop left behind out.
        part.push(format!(".{}.tmp", std::process::id()));
        let part = PathBuf::from(part);
        let fail = |x: std::io::Error| format!("{}: {x}", shown(&part));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // It holds the bytes of the project's files: only their owner reads it.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&part).map_err(fail)?;
        let written = file
            .try_lock()
            .map_err(|x| fail(std::io::Error::other(x.to_string())))
            .and_then(|()| file.write_all(&bytes).map_err(fail))
            .and_then(|()| file.sync_all().map_err(fail))
            .and_then(|()| std::fs::rename(&part, &path).map_err(fail));
        if let Err(err) = written {
            drop(file);
            return Err(match std::fs::remove_file(&part) {
                Err(x) if x.kind() != std::io::ErrorKind::NotFound => {
                    format!("{err}; removing {} failed too: {x}", shown(&part))
                }
                _ => err,
            });
        }
        // Without its folder entry on disk a power loss could lose the journal after the first
        // write: stop before writing anything.
        if let Err(x) = store::sync_dir(dir) {
            let removed = std::fs::remove_file(&path);
            drop(file);
            return Err(match removed {
                Ok(()) => format!("{}: {x}; nothing was changed", shown(dir)),
                Err(r) => format!(
                    "{}: {x}; nothing was changed, but removing {} failed too ({r}), so it may still be there: remove it before the next run, or that run puts back the files it holds (as they are now)",
                    shown(dir),
                    shown(&path)
                ),
            });
        }
        Ok(Self { path, file })
    }

    /// Every file is written (or put back): the journal goes, removed while still locked.
    fn finish(self, dir: &Path) -> Res<()> {
        let shown = escape_field(&self.path.display().to_string());
        let removed = match std::fs::remove_file(&self.path) {
            Err(x) => Err(format!(
                "removing {shown} failed: {x}; remove that file, or the next run puts the project back as it was before the edit"
            )),
            Ok(()) => store::sync_dir(dir).map_err(|x| {
                format!(
                    "{shown} is removed, but making that last on disk failed ({x}); after a power loss the next run may put the project back as it was before the edit"
                )
            }),
        };
        drop(self.file);
        removed
    }
}

/// The name a journal keeps for `file`: its path inside `dir` with `/` between the parts.
/// The black box symbol files the save may remove come with the vendored symbol folder
/// resolved, so they are named by that folder as the project sees it.
fn journal_name(dir: &Path, file: &Path) -> Option<String> {
    let rel = match file.strip_prefix(dir) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => {
            let symbols = dir.join(store::VENDOR_DIR).join(ItemKind::Symbol.dir());
            let resolved = symbols.canonicalize().ok()?;
            if file.parent()? != resolved {
                return None;
            }
            Path::new(store::VENDOR_DIR)
                .join(ItemKind::Symbol.dir())
                .join(file.file_name()?)
        }
    };
    let parts = rel
        .components()
        .map(|c| match c {
            std::path::Component::Normal(n) => n.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    journal_name_allowed(&parts).then(|| parts.join("/"))
}

/// Whether a journal may name the file with these path parts: only the files box-edit may
/// change (the project's own files, its pages, panels and comments, and vendored symbols).
fn journal_name_allowed(parts: &[&str]) -> bool {
    match parts {
        [name] => [store::PROJECT_FILE, store::DEVICES_FILE, store::LOCK_FILE].contains(name),
        [folder, name] => [
            (store::PAGES_DIR, store::PAGE_SUFFIX),
            (store::PANELS_DIR, store::PANEL_SUFFIX),
            (store::COMMENTS_DIR, store::COMMENTS_SUFFIX),
        ]
        .iter()
        // As the store and the snapshot take them: any name ending in the suffix, the suffix alone
        // too (`comments/.comments.json`).
        .any(|(f, suffix)| folder == f && name.ends_with(suffix)),
        [vendor, kind, name] => {
            *vendor == store::VENDOR_DIR && *kind == ItemKind::Symbol.dir() && name.ends_with(".json")
        }
        _ => false,
    }
}

/// A folder on the way to a journaled file that is a link could lead out of the project, and
/// a write or removal there would land elsewhere: refused.
fn journal_target_ok(dir: &Path, rel: &str) -> Res<()> {
    let mut at = dir.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        at.push(part);
        if is_link(&at)? {
            return Err(format!(
                "{} is a link; box-edit does not write through a linked folder",
                escape_field(&at.display().to_string())
            ));
        }
    }
    Ok(())
}

/// The files and their bytes before, as [`Journal::begin`] wrote them; `None` when the bytes
/// are not a whole journal or name a file box-edit does not change.
fn parse_journal(dir: &Path, bytes: &[u8]) -> Option<run::Written> {
    let mut rest = bytes.strip_prefix(JOURNAL_HEADER)?;
    let mut files = Vec::new();
    loop {
        let end = rest.iter().position(|&b| b == b'\n')?;
        let line = std::str::from_utf8(&rest[..end]).ok()?;
        rest = &rest[end + 1..];
        let (first, len) = line.split_once(' ')?;
        if first == "end" {
            // The count the run wrote, and nothing after it.
            return (len.parse::<usize>().ok()? == files.len() && rest.is_empty()).then_some(files);
        }
        let rel_len: usize = first.parse().ok()?;
        let len: Option<usize> = if len == "-" { None } else { Some(len.parse().ok()?) };
        let rel = std::str::from_utf8(rest.get(..rel_len)?).ok()?;
        rest = &rest[rel_len..];
        let parts: Vec<&str> = rel.split('/').collect();
        if !journal_name_allowed(&parts) || parts.iter().any(|p| matches!(*p, "" | "." | "..")) {
            return None;
        }
        let before = match len {
            Some(n) => {
                let b = rest.get(..n)?.to_vec();
                rest = &rest[n..];
                Some(b)
            }
            None => None,
        };
        files.push((parts.iter().fold(dir.to_path_buf(), |p, part| p.join(part)), before));
    }
}

/// When a `box-edit` run stopped before it had written every file (killed, power lost), put
/// the files it may have changed back as they were before it, so the symbol, `library.lock`
/// and pages agree again (#653). A journal another run still holds is left alone. Says what
/// it did, `None` when there was nothing to do.
fn undo_stopped_box_edit(dir: &Path) -> Res<Option<String>> {
    let path = dir.join(BOX_EDIT_JOURNAL);
    let shown = escape_field(&path.display().to_string());
    let not_a_file = || format!("{shown} is not a plain file; remove it");
    // Only a plain file: a link could lead anywhere, and reading a FIFO would never end.
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_file() => {}
        Ok(_) => return Err(not_a_file()),
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(x) => return Err(format!("{shown}: {x}")),
    }
    let mut file = None;
    let mut denied = None;
    for _ in 0..10 {
        match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => {
                file = Some(f);
                break;
            }
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            // Windows: the run holding it may be removing it; it is gone a moment later.
            Err(x) if x.kind() == std::io::ErrorKind::PermissionDenied && cfg!(windows) => {
                denied = Some(x);
                std::thread::sleep(std::time::Duration::from_millis(50));
                if std::fs::symlink_metadata(&path).is_err_and(|m| m.kind() == std::io::ErrorKind::NotFound) {
                    return Ok(None);
                }
            }
            Err(x) => return Err(format!("{shown}: {x}")),
        }
    }
    let Some(mut file) = file else {
        return Err(format!(
            "{shown}: {}",
            denied.map_or_else(|| "access denied".to_owned(), |x| x.to_string())
        ));
    };
    if !file.metadata().map_err(|x| format!("{shown}: {x}"))?.is_file() {
        return Err(not_a_file());
    }
    // Tests swap the journal here, as another run finishing and starting could.
    #[cfg(test)]
    if let Some(between) = tests::BETWEEN_JOURNAL_OPEN_AND_LOCK.take() {
        between();
    }
    match file.try_lock() {
        Ok(()) => {}
        // A run is writing now: its journal is not one left behind.
        Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
        Err(std::fs::TryLockError::Error(x)) => return Err(format!("{shown}: {x}")),
    }
    // The run that held it may have finished and removed it just before the lock was taken,
    // and another run may have put a journal of its own there since: only the file locked
    // here is one left behind.
    match std::fs::symlink_metadata(&path) {
        Ok(now) => {
            if !same_file(&now, &file).map_err(|x| format!("{shown}: {x}"))? {
                return Ok(None);
            }
        }
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(x) => return Err(format!("{shown}: {x}")),
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).map_err(|x| format!("{shown}: {x}"))?;
    let unreadable = || {
        format!(
            "{shown} is left from a box-edit run that stopped, but it cannot be read; put the project files back from the project's history (e.g. `git checkout`) and remove it"
        )
    };
    let files = parse_journal(dir, &bytes).ok_or_else(unreadable)?;
    let mut names = Vec::with_capacity(files.len());
    for (target, _) in &files {
        let name = journal_name(dir, target).ok_or_else(unreadable)?;
        journal_target_ok(dir, &name)?;
        names.push(name);
        // Each file is put back over a plain file or where none is: a link would be replaced by
        // a plain file, and opening a FIFO to compare its bytes could wait for ever.
        let target_shown = || escape_field(&target.display().to_string());
        match std::fs::symlink_metadata(target) {
            Ok(m) if m.file_type().is_file() => {}
            Ok(_) => {
                return Err(format!(
                    "{} is not a plain file, so the box-edit run that stopped half way cannot be undone; put it back from the project's history (e.g. `git checkout`) and run again",
                    target_shown()
                ));
            }
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => {}
            Err(x) => return Err(format!("{}: {x}", target_shown())),
        }
    }
    // Tests swap a folder for a link here, as someone could after the check above.
    #[cfg(test)]
    if let Some(between) = tests::BETWEEN_JOURNAL_CHECK_AND_REPLAY.take() {
        between();
    }
    let failed = put_journal_back(dir, &names, &files);
    if !failed.is_empty() {
        return Err(format!(
            "a box-edit run stopped half way, and putting back the files it changed failed: {}",
            failed.join("; ")
        ));
    }
    // The files put back are on disk before the journal goes, or it stays for the next run.
    sync_folders(&files).map_err(|x| format!("a box-edit run stopped half way; its files are put back, but {x}"))?;
    std::fs::remove_file(&path).map_err(|x| format!("{shown}: {x}"))?;
    store::sync_dir(dir).map_err(|x| format!("{}: {x}", escape_field(&dir.display().to_string())))?;
    drop(file);
    Ok(Some(
        "a box-edit run stopped before it had written every file; the project files are put back as they were before it".into(),
    ))
}

/// Put each journaled file back as it was, the last written first; `names` are their names in
/// the project ([`journal_name`]). Returns why each one that could not be put back failed.
///
/// On Unix each folder on the way is opened relative to the one before it, never following a
/// link, and the file is written, renamed and removed relative to its folder held open: a folder
/// swapped for a link after [`journal_target_ok`] checked it is refused, not written through
/// (#666). Elsewhere the files are put back by path, after that check alone.
fn put_journal_back(dir: &Path, names: &[String], files: &run::Written) -> Vec<String> {
    #[cfg(unix)]
    {
        names
            .iter()
            .zip(files)
            .rev()
            .filter_map(|(name, (_, before))| put_back_at(dir, name, before.as_deref()).err())
            .collect()
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, names);
        put_files_back(files)
    }
}

/// [`put_file_back`] for the file `rel` names in the project `dir`, each folder on the way
/// opened without following a link and the file reached only through its folder held open.
#[cfg(unix)]
fn put_back_at(dir: &Path, rel: &str, before: Option<&[u8]>) -> Result<(), String> {
    use rustix::fs::{AtFlags, Mode, OFlags};
    use rustix::io::Errno;
    use std::io::{Read as _, Write as _};
    let shown = |rel: &str| escape_field(&dir.join(rel).display().to_string());
    let fail = |x: std::io::Error| format!("{}: {x}", shown(rel));
    let (folders, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    let folder_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    // The project folder itself may be reached through a link: the one the run was given.
    let mut at = rustix::fs::open(dir, folder_flags, Mode::empty())
        .map_err(|x| format!("{}: {}", shown(""), std::io::Error::from(x)))?;
    let mut walked = String::new();
    for part in folders.split('/').filter(|p| !p.is_empty()) {
        if !walked.is_empty() {
            walked.push('/');
        }
        walked.push_str(part);
        at = match rustix::fs::openat(&at, part, folder_flags | OFlags::NOFOLLOW, Mode::empty()) {
            Ok(fd) => fd,
            // A link (or no longer a folder): what it leads to is not written.
            Err(Errno::LOOP | Errno::NOTDIR) => {
                return Err(format!(
                    "{} is a link; box-edit does not write through a linked folder",
                    shown(&walked)
                ));
            }
            // No folder, so no file there to remove.
            Err(Errno::NOENT) if before.is_none() => return Ok(()),
            Err(x) => return Err(fail(x.into())),
        };
    }
    let Some(bytes) = before else {
        // A link there is removed itself, never what it leads to.
        return match rustix::fs::unlinkat(&at, name, AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => Ok(()),
            Err(x) => Err(fail(x.into())),
        };
    };
    // A plain file there keeps its permissions, and is left as it is when it holds the bytes
    // already; anything else is replaced, never opened to read (a FIFO would never end).
    let now = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let keep = match rustix::fs::openat(&at, name, now, Mode::empty()) {
        Ok(fd) => {
            let mut file = std::fs::File::from(fd);
            let m = file.metadata().map_err(fail)?;
            if m.is_file() {
                let mut held = Vec::new();
                if file.read_to_end(&mut held).is_ok() && held == bytes {
                    return Ok(());
                }
            }
            m.is_file().then(|| m.permissions())
        }
        Err(Errno::NOENT | Errno::LOOP) => None,
        // A file this user may write but not read (mode 0200) is still put back, keeping its
        // permissions, as `put_file_back` does.
        Err(Errno::ACCESS) => {
            let m = rustix::fs::statat(&at, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|x| fail(x.into()))?;
            (rustix::fs::FileType::from_raw_mode(m.st_mode) == rustix::fs::FileType::RegularFile).then(|| {
                use std::os::unix::fs::PermissionsExt as _;
                // `st_mode` is a u16 on some systems (macOS).
                #[allow(clippy::useless_conversion)]
                let mode = u32::from(m.st_mode) & 0o7777;
                std::fs::Permissions::from_mode(mode)
            })
        }
        Err(x) => return Err(fail(x.into())),
    };
    // The staging file is made new beside it, readable by its owner only before any bytes go in.
    let mut made = None;
    for n in 0..100u32 {
        let part = format!("{name}.restore-{}-{n}", std::process::id());
        let new = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        match rustix::fs::openat(&at, part.as_str(), new, Mode::RUSR | Mode::WUSR) {
            Ok(fd) => {
                made = Some((part, std::fs::File::from(fd)));
                break;
            }
            Err(Errno::EXIST) => continue,
            Err(x) => return Err(fail(x.into())),
        }
    }
    let Some((part, mut file)) = made else {
        return Err(format!("{}: no free name for the staging file", shown(rel)));
    };
    let done = file
        .write_all(bytes)
        .and_then(|()| keep.map_or(Ok(()), |perm| file.set_permissions(perm)))
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            rustix::fs::renameat(&at, part.as_str(), &at, name).map_err(Into::into)
        });
    done.map_err(|x| match rustix::fs::unlinkat(&at, part.as_str(), AtFlags::empty()) {
        Ok(()) => fail(x),
        Err(y) => format!(
            "{}; the staging file {} is left: {}",
            fail(x),
            shown(&if folders.is_empty() {
                part
            } else {
                format!("{folders}/{part}")
            }),
            std::io::Error::from(y)
        ),
    })
}

/// Make the files put back last on disk: sync the folder of each (Unix). Says which folder
/// failed.
fn sync_folders(files: &[(PathBuf, Option<Vec<u8>>)]) -> Res<()> {
    let mut folders: Vec<&Path> = files.iter().filter_map(|(f, _)| f.parent()).collect();
    folders.sort();
    folders.dedup();
    for folder in folders {
        match store::sync_dir(folder) {
            Err(x) if x.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("{}: {x}", escape_field(&folder.display().to_string())));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether the entry `at` names is the file `file` has open. Only Unix tells it (device and
/// inode); elsewhere the entry being there has to do.
fn same_file(at: &std::fs::Metadata, file: &std::fs::File) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let m = file.metadata()?;
        Ok(m.dev() == at.dev() && m.ino() == at.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = (at, file);
        Ok(true)
    }
}

/// Whether `path` is a symbolic link. A missing file is not one; any other error reading its
/// metadata is returned with the path, so a check that cannot tell refuses instead of guessing.
fn is_link(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => Ok(m.file_type().is_symlink()),
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(x) => Err(format!("{}: {x}", escape_field(&path.display().to_string()))),
    }
}

fn main() -> ExitCode {
    // Parsing runs there too: clap's derived parser builds every subcommand on the stack.
    let worker = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(|| with_stdout_error(run(Cli::parse())));
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

/// The lock that keeps two wiringcraft commands from working on the project in `dir` at once,
/// held until the returned file is dropped (or the process ends, even by a crash). A second
/// command waits for the first, saying so. The lock file is [`project_lock_path`], inside the
/// project, so every command on the project finds the same one whatever its environment.
/// `None`, and the command runs unlocked, when `dir` is not a project; when a git command that
/// a wiringcraft command runs on this project started this one (a hook), as that command holds
/// the lock and waits for this one; or, with a warning, when the lock cannot be taken (a
/// read-only folder): the lock guards against overlapping runs, it is never a reason to refuse
/// one.
fn project_lock(dir: &Path) -> Option<std::fs::File> {
    if !dir.join(store::PROJECT_FILE).is_file() {
        return None;
    }
    if let Some(parent) = std::env::var_os(wc_vcs::PARENT_PROJECT_ENV)
        && same_folder(Path::new(&parent), dir)
    {
        return None;
    }
    let path = project_lock_path(dir);
    let unlocked = |why: String| {
        eprintln!(
            "warning: running without the project lock ({}: {}); another wiringcraft command on this project at the same time could undo this one's changes",
            escape_field(&path.display().to_string()),
            escape_field(&why)
        );
    };
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(file) => file,
        Err(err) => {
            unlocked(err.to_string());
            return None;
        }
    };
    let file = match file.try_lock() {
        Ok(()) => file,
        Err(std::fs::TryLockError::WouldBlock) => {
            eprintln!("waiting for another wiringcraft command on this project to finish");
            match file.lock() {
                Ok(()) => file,
                Err(err) => {
                    unlocked(err.to_string());
                    return None;
                }
            }
        }
        Err(std::fs::TryLockError::Error(err)) => {
            unlocked(err.to_string());
            return None;
        }
    };
    keep_lock_out_of_git(dir);
    Some(file)
}

/// Whether `a` and `b` are the same folder (by their canonical paths).
fn same_folder(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Where [`project_lock`] keeps the lock of the project in `dir`: `.wiringcraft.lock` in the
/// project folder, always there so a command started before `vcs init` and one started after
/// it wait on the same file. A new project's `.gitignore` leaves it out.
fn project_lock_path(dir: &Path) -> PathBuf {
    dir.join(wc_vcs::LOCK_FILE)
}

/// Leave the lock file out of the project's git history in a repository whose `.gitignore`
/// predates it (or a linked worktree of one): listed in the repository's local exclude list.
/// Best effort, with a warning: a project whose exclude list cannot be written only shows the
/// file as new.
fn keep_lock_out_of_git(dir: &Path) {
    if !dir.join(".git").exists() {
        return;
    }
    if let Err(err) = wc_vcs::Git::new(dir).exclude_locally(wc_vcs::LOCK_FILE) {
        eprintln!(
            "warning: could not leave {} out of git: {}",
            wc_vcs::LOCK_FILE,
            escape_field(&err.to_string())
        );
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

/// `wiringcraft report-layout`: print, replace or reset report layouts, keeping them as the
/// report layout dialog and the assistant's `report_layouts` tool do. `stdin` is what was read
/// from standard input for `set <kind> -`, when it already was.
fn report_layout(dir: &Path, cmd: ReportLayoutCmd, input: Option<String>) -> Res<()> {
    use wc_core::report_layout::ReportLayout;
    let one = |name: &str| -> Res<wc_report::Kind> {
        match report_kinds(&[name.to_owned()])?.as_slice() {
            [k] => Ok(*k),
            _ => Err(format!("name one report, not {name:?}")),
        }
    };
    let mut p = load(dir)?;
    let (symbols, _) = store::vendored_symbols(dir, &p);
    let parts = vendored_parts(dir, &p);
    let builtin = |p: &Project, kind| {
        let inputs = wc_report::Inputs {
            project: p,
            symbols: &symbols,
            parts: &parts,
        };
        wc_report::build_builtin(kind, &inputs).columns
    };
    let warnings = |p: &Project, about: &str| -> Vec<String> {
        let inputs = wc_report::Inputs {
            project: p,
            symbols: &symbols,
            parts: &parts,
        };
        let about = format!("report layout for \"{about}\":");
        wc_report::layout_warnings(&inputs)
            .into_iter()
            .filter(|w| w.starts_with(&about))
            .collect()
    };
    let (kind, layout) = match cmd {
        ReportLayoutCmd::Show { kind } => {
            let kinds = match kind {
                Some(k) => vec![one(&k)?],
                None => wc_report::Kind::ALL.to_vec(),
            };
            let mut out: Vec<serde_json::Value> = kinds
                .iter()
                .map(|k| {
                    serde_json::json!({
                        "kind": k.name(),
                        "layout": wc_report::layout_for(&p, *k),
                        "builtin_columns": builtin(&p, *k),
                        "warnings": warnings(&p, k.name()),
                    })
                })
                .collect();
            // Every report: a stored layout for a report that does not exist is listed too, so
            // it and its warnings are not hidden.
            if kinds.len() > 1 {
                for l in &p.meta.reports {
                    if wc_report::Kind::parse(&l.kind).is_none() {
                        out.push(serde_json::json!({
                            "kind": l.kind,
                            "layout": l,
                            "builtin_columns": null,
                            "warnings": warnings(&p, &l.kind),
                        }));
                    }
                }
            }
            let text = serde_json::to_string_pretty(&out).map_err(e)?;
            println!("{text}");
            return Ok(());
        }
        ReportLayoutCmd::Set { kind, file } => {
            let kind = one(&kind)?;
            let text = match input {
                Some(text) => text,
                None => read_input(&file)?,
            };
            let name = if file.as_os_str() == "-" {
                "stdin".to_owned()
            } else {
                file.display().to_string()
            };
            let value: serde_json::Value = serde_json::from_str(&text).map_err(|err| format!("{name}: {err}"))?;
            let layout = ReportLayout::from_json(kind.name(), value)?;
            (kind, layout.kept(&builtin(&p, kind))?)
        }
        ReportLayoutCmd::Reset { kind } => (one(&kind)?, None),
    };
    let mut reports = p.meta.reports.clone();
    match &layout {
        Some(l) => match reports.iter_mut().find(|r| r.kind == kind.name()) {
            Some(slot) => *slot = l.clone(),
            None => reports.push(l.clone()),
        },
        None => reports.retain(|r| r.kind != kind.name()),
    }
    if reports != p.meta.reports {
        p.meta.reports = reports;
        save(dir, &p)?;
    }
    match layout {
        Some(_) => println!("the {} report's layout is set", kind.name()),
        None => println!("the {} report has its built-in layout", kind.name()),
    }
    for w in warnings(&p, kind.name()) {
        eprintln!("warning: {}", escape_field(&w));
    }
    Ok(())
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
    import_wago_bytes(dir, &bytes)
}

/// [`import_wago`] from the file's bytes, read already (before the project lock).
fn import_wago_bytes(dir: &Path, bytes: &[u8]) -> Res<WagoImported> {
    let (rows, mut warnings) = wc_import::wago::read_configured(bytes).map_err(e)?;
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

/// Import every symbol (one per unit) of a KiCad library text into `lib`, printing each.
fn import_kicad(text: &str, lib: &Path, prefix: &str, scale: f64, category: &str) -> Res<usize> {
    let mut n = 0;
    logged(|log| {
        n = write_kicad(read_kicad(text, prefix, scale, log)?, lib, category, log)?;
        Ok(())
    })?;
    Ok(n)
}

/// The symbol id `import symbol-qet` gives the element in `file`: the prefix and the file stem.
fn qet_id(file: &Path, prefix: &str) -> String {
    let stem = wc_import::id_segment(
        &file
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    format!("{}.{stem}", prefix.trim_end_matches('.'))
}

/// The element in `file` as the symbol `id`, with warnings.
fn qet_element(file: &Path, id: &str, scale: f64) -> Res<(wc_import::qet::Imported, Vec<String>)> {
    use std::io::Read;
    std::fs::File::open(file)
        .and_then(|f| {
            // Read at most one byte past the limit, so a huge file is refused cheaply.
            let mut text = String::new();
            f.take(wc_import::qet::MAX_LEN as u64 + 1).read_to_string(&mut text)?;
            Ok(text)
        })
        .map_err(e)
        .and_then(|text| wc_import::qet::element(&text, id, scale).map_err(e))
}

fn import_qet(
    files: &[PathBuf],
    lib: &Path,
    prefix: &str,
    scale: f64,
    category: &str,
    letter_code: &str,
) -> Res<(usize, usize)> {
    let (mut written, mut failed) = (0, 0);
    // Two files whose names give the same id would overwrite each other: the later one fails.
    let mut ids: std::collections::BTreeMap<String, PathBuf> = Default::default();
    // The first operand with each id, whether it was imported or not.
    let mut named: std::collections::BTreeMap<String, PathBuf> = Default::default();
    for file in files {
        let id = qet_id(file, prefix);
        // A stream left as it is (not a copy) whose id an earlier operand has is one
        // `spool_inputs` left unread for that id. It is refused for it here too, without being
        // opened, whatever the earlier operand reads as now: a FIFO without a writer would
        // otherwise hold the command, and the project lock, up (#700).
        let unread = std::fs::metadata(file).is_ok_and(|m| !m.is_file() && !m.is_dir());
        let read = match ids.get(&id).or(named.get(&id).filter(|_| unread)) {
            Some(first) => Err(format!("gives the same id {id} as {}, skipped", first.display())),
            None => qet_element(file, &id, scale),
        };
        named.entry(id.clone()).or_insert_with(|| file.clone());
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

/// Refuses a plate size outside the 200 to 5000 mm range `panel new` and `panel edit` accept.
fn check_plate_size(width: f64, height: f64) -> Result<(), String> {
    if width.is_finite()
        && height.is_finite()
        && (200.0..=5000.0).contains(&width)
        && (200.0..=5000.0).contains(&height)
    {
        Ok(())
    } else {
        Err("plate width and height must be 200 to 5000 mm".into())
    }
}

fn vendored_parts(dir: &Path, p: &wc_core::Project) -> std::collections::BTreeMap<String, wc_core::Part> {
    p.used_parts()
        .into_iter()
        .filter_map(|id| store::read_vendored_part(dir, &id).ok().map(|part| (id, part)))
        .collect()
}

/// [`vendored_parts`], and the used parts whose copy is there but cannot be read, by id with the
/// reason: the project check reports those as unreadable, not as missing.
fn vendored_parts_and_unreadable(
    dir: &Path,
    p: &wc_core::Project,
) -> (
    std::collections::BTreeMap<String, wc_core::Part>,
    std::collections::BTreeMap<String, String>,
) {
    let mut parts = std::collections::BTreeMap::new();
    let mut unreadable = std::collections::BTreeMap::new();
    for id in p.used_parts() {
        match store::read_vendored_part(dir, &id) {
            Ok(part) => {
                parts.insert(id, part);
            }
            Err(err) => {
                let there = store::vendor_path(dir, ItemKind::Part, &id).is_ok_and(|f| f.exists());
                if there {
                    unreadable.insert(id, err.to_string());
                }
            }
        }
    }
    (parts, unreadable)
}

/// [`vendored_parts`], with a warning naming each used part that cannot be read and why, so
/// the warnings worked out from the parts do not quietly leave it out.
fn vendored_parts_warning_unreadable(
    dir: &Path,
    p: &wc_core::Project,
) -> std::collections::BTreeMap<String, wc_core::Part> {
    let mut parts = std::collections::BTreeMap::new();
    for id in p.used_parts() {
        match store::read_vendored_part(dir, &id) {
            Ok(part) => {
                parts.insert(id, part);
            }
            // The part id and the reason come from the project: escaped, so they cannot steer
            // the terminal.
            Err(err) => eprintln!("warning: {}", escape_field(&format!("cannot read part {id}: {err}"))),
        }
    }
    parts
}

/// The upright size of the device `tag`'s part, if it has a part with a size. A part that
/// cannot be read gives an error naming it and the reason, so a damaged or missing copy is not
/// mistaken for a part without a size.
fn device_part_size(dir: &Path, p: &wc_core::Project, tag: &DeviceTag) -> Result<Option<(f64, f64)>, String> {
    let Some(part) = p.devices.get(tag).and_then(|d| d.part.as_ref()) else {
        return Ok(None);
    };
    match store::read_vendored_part(dir, part) {
        Ok(read) => Ok(wc_core::panel::part_size(&read)),
        // The part id and the reason come from the project file: escaped, so they cannot steer
        // the terminal.
        Err(err) => Err(escape_field(&format!("cannot read part {part} of {tag}: {err}"))),
    }
}

/// Height of a DIN rail on the plate, centred on its line.
const RAIL_HEIGHT: f64 = 35.0;

/// A rail for `panel rail`, or an error when its length is not positive or the rail is not
/// wholly on the plate.
fn panel_rail(panel: &wc_core::panel::Panel, at: Point, length: f64) -> Result<wc_core::panel::PanelItem, String> {
    if !(length.is_finite() && length > 0.0) {
        return Err(format!("rail length must be more than 0 mm, got {length}"));
    }
    let half = RAIL_HEIGHT / 2.0;
    check_on_plate(panel, "rail", Point::new(at.x, at.y - half), length, RAIL_HEIGHT)?;
    Ok(wc_core::panel::PanelItem::Rail { at, length })
}

/// A `width` x `height` duct with its top-left corner at `at` for `panel duct`, or an error
/// when a size is not positive or the duct is not wholly on the plate.
fn panel_duct(
    panel: &wc_core::panel::Panel,
    at: Point,
    width: f64,
    height: f64,
) -> Result<wc_core::panel::PanelItem, String> {
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err(format!(
            "duct length and width must be more than 0 mm, got {width} x {height}"
        ));
    }
    check_on_plate(panel, "duct", at, width, height)?;
    Ok(wc_core::panel::PanelItem::Duct {
        min: at,
        max: Point::new(at.x + width, at.y + height),
    })
}

/// An error naming `what` when a `w` x `h` box with its top-left corner at `at` is not on the
/// plate (see [`wc_core::panel::Panel::outline_fits`]).
fn check_on_plate(panel: &wc_core::panel::Panel, what: &str, at: Point, w: f64, h: f64) -> Result<(), String> {
    if !(at.x.is_finite() && at.y.is_finite()) {
        return Err(format!("{what}: position must be finite"));
    }
    if !panel.outline_fits(at, w, h) {
        return Err(format!(
            "{what}: a {w} x {h} mm outline at {},{} is not on the {} x {} mm plate",
            at.x, at.y, panel.width, panel.height
        ));
    }
    Ok(())
}

/// Take rail (or duct) number `n`, counted from 1 as `panel show` lists them, off the plate.
fn remove_rail_or_duct(panel: &mut wc_core::panel::Panel, rail: bool, n: usize) -> Result<(), String> {
    let what = if rail { "rail" } else { "duct" };
    let index = panel
        .items
        .iter()
        .enumerate()
        .filter(|(_, i)| match i {
            wc_core::panel::PanelItem::Rail { .. } => rail,
            wc_core::panel::PanelItem::Duct { .. } => !rail,
            wc_core::panel::PanelItem::Device { .. } => false,
        })
        .nth(n.wrapping_sub(1))
        .map(|(i, _)| i);
    let Some(index) = index else {
        return Err(format!(
            "no {what} {n} on panel {:?}: `panel show` numbers them",
            panel.id
        ));
    };
    panel.items.remove(index);
    Ok(())
}

/// `panel show`: the plate, its numbered rails and ducts, then each device with its place,
/// size and turn, one per line.
fn panel_show(panel: &wc_core::panel::Panel) -> String {
    let mut out = format!(
        "{} {} ({} x {} mm)\n",
        panel.id,
        escape_field(&panel.name),
        panel.width,
        panel.height
    );
    let (mut rails, mut ducts) = (0, 0);
    for item in &panel.items {
        match item {
            wc_core::panel::PanelItem::Rail { at, length } => {
                rails += 1;
                let name = format!("rail {rails}");
                out.push_str(&format!("{name:<12} at {},{}  {length} mm long\n", at.x, at.y));
            }
            wc_core::panel::PanelItem::Duct { min, max } => {
                ducts += 1;
                let name = format!("duct {ducts}");
                out.push_str(&format!(
                    "{name:<12} at {},{}  {} x {} mm\n",
                    min.x,
                    min.y,
                    max.x - min.x,
                    max.y - min.y
                ));
            }
            wc_core::panel::PanelItem::Device { .. } => {}
        }
    }
    for item in &panel.items {
        if let wc_core::panel::PanelItem::Device {
            tag,
            at,
            width,
            height,
            rotation,
        } = item
        {
            out.push_str(&format!(
                "{:<12} at {},{}  {} x {} mm  turned {}\n",
                escape_field(&tag.to_string()),
                at.x,
                at.y,
                width,
                height,
                rotation.degrees()
            ));
        }
    }
    out
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

/// The part and description of device `tag`, one per line (`-` when unset), as
/// `wiringcraft device <tag>` prints them. An error when no device has that tag, in the devices
/// table or on a page.
fn device_info(p: &Project, tag: &DeviceTag) -> Res<String> {
    let drawn = || {
        p.pages
            .iter()
            .any(|pg| pg.symbols.iter().any(|s| s.tag.as_ref() == Some(tag)))
    };
    let (part, description) = match p.devices.get(tag) {
        Some(d) => (d.part.as_deref().unwrap_or(""), d.description.as_str()),
        None if drawn() => ("", ""),
        None => return Err(format!("no device {tag}")),
    };
    let show = |v: &str| if v.is_empty() { "-".to_owned() } else { escape_field(v) };
    Ok(format!(
        "{:<12} {}\n{:<12} {}\n",
        "part",
        show(part),
        "description",
        show(description)
    ))
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    thread_local! {
        /// Set by a test to stop `box-edit` after its symbol write, as a kill would.
        pub(super) static STOP_BOX_EDIT_AFTER_SYMBOL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        /// Run by recovery between opening a journal and locking it.
        pub(super) static BETWEEN_JOURNAL_OPEN_AND_LOCK: std::cell::Cell<Option<Box<dyn FnOnce()>>> = const { std::cell::Cell::new(None) };
        /// Run by recovery between checking the files a journal names and putting them back.
        pub(super) static BETWEEN_JOURNAL_CHECK_AND_REPLAY: std::cell::Cell<Option<Box<dyn FnOnce()>>> = const { std::cell::Cell::new(None) };
    }

    /// A stdout whose reader has gone away, as Windows reports it for `| more` that stopped.
    struct ClosedPipe;

    impl std::io::Write for ClosedPipe {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }

    /// Run `f` on its own thread, as `main` runs a command, with its stdout errors.
    fn run_like_main(f: impl FnOnce() -> Res<()> + Send + 'static) -> Res<()> {
        std::thread::spawn(move || with_stdout_error(f())).join().unwrap()
    }

    #[test]
    fn a_closed_stdout_lets_the_command_finish_quietly() {
        let result = run_like_main(|| {
            let locations = [wc_core::project::Location {
                path: "A1".into(),
                description: String::new(),
            }];
            write_stdout(|_| write_locations(&mut ClosedPipe, &locations));
            // Later output is dropped, not written to the real stdout or failed on.
            println!("after the reader went away");
            Ok(())
        });
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn the_command_error_wins_over_a_closed_stdout() {
        let result = run_like_main(|| {
            write_stdout(|_| Err(std::io::ErrorKind::BrokenPipe.into()));
            Err("no such page".into())
        });
        assert_eq!(result, Err("no such page".into()));
    }

    #[test]
    fn other_stdout_errors_are_reported() {
        let result = run_like_main(|| {
            write_stdout(|_| Err(std::io::Error::other("disk full")));
            write_stdout(|_| Err(std::io::Error::other("a second error")));
            Ok(())
        });
        assert_eq!(result, Err("could not write the output: disk full".into()));
    }

    /// Walks the whole command tree and lists every subcommand and argument
    /// (positional or option) whose `--help` line would be blank.
    fn commands_and_args_without_help(cmd: &clap::Command, path: &str, out: &mut Vec<String>) {
        for arg in cmd.get_arguments() {
            let id = arg.get_id().as_str();
            if id == "help" || id == "version" {
                continue;
            }
            if arg.get_help().is_none() && arg.get_long_help().is_none() {
                out.push(format!("{path} <{id}>"));
            }
        }
        for sub in cmd.get_subcommands() {
            let sub_path = format!("{path} {}", sub.get_name());
            if sub.get_about().is_none() && sub.get_long_about().is_none() {
                out.push(sub_path.clone());
            }
            commands_and_args_without_help(sub, &sub_path, out);
        }
    }

    #[test]
    fn every_subcommand_and_argument_has_help_text() {
        use clap::CommandFactory;
        let mut missing = Vec::new();
        commands_and_args_without_help(&Cli::command(), "wiringcraft", &mut missing);
        assert!(missing.is_empty(), "no help text for: {missing:#?}");
    }

    /// A stream the test reads while the import is still running.
    #[derive(Clone, Default)]
    struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Shared {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Shared {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    /// A stream whose reader went away (`wiringcraft import ... | head -0`).
    struct Closed;

    impl std::io::Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn an_import_line_is_printed_before_the_import_goes_on_to_the_next_entry() {
        let (out, err) = (Shared::default(), Shared::default());
        let r = logged_to(out.clone(), err.clone(), |log| {
            log.out("archive: a.dxf".into());
            log.err("warning: b".into());
            // The next entry is still being read here, and the first is already shown.
            assert_eq!(out.text(), "archive: a.dxf\n");
            assert_eq!(err.text(), "warning: b\n");
            log.out("archive: c.pdf".into());
            Ok(())
        });
        assert_eq!(r, Ok(()));
        assert_eq!(out.text(), "archive: a.dxf\narchive: c.pdf\n");
    }

    #[test]
    fn a_report_that_cannot_be_printed_does_not_hide_a_finished_import() {
        let t = tempfile::tempdir().unwrap();
        let saved = t.path().join("page.toml");
        let err = Shared::default();
        let r = logged_to(Closed, err.clone(), |log| {
            log.out("archive: a.dxf".into());
            // The import goes on after the line could not be printed, and saves.
            std::fs::write(&saved, "page").map_err(|e| e.to_string())?;
            log.out("imported page 300".into());
            log.err("warning: kept".into());
            Ok(())
        });
        assert_eq!(r, Ok(()));
        assert!(saved.is_file());
        let shown = err.text();
        assert!(shown.starts_with("warning: kept\n"), "{shown}");
        assert!(
            shown.contains("could not be printed on standard output") && shown.contains("the import was done"),
            "{shown}"
        );
        assert_eq!(shown.matches("could not be printed").count(), 1, "{shown}");
    }

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
        let (out, by) = dwg_to_dxf(&dwg, &folder, Some("cp {in} {out}".into())).unwrap();
        assert_eq!(out, folder.join("Motor plan.dxf"));
        assert_eq!(by, "the WIRINGCRAFT_DWG2DXF command");
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

        // The converter makes a DXF that cannot be read: both reasons are given.
        let pages = load(&dir).unwrap().pages;
        let junk = converter("junk.sh", "echo junk > \"$2\"");
        let err = import_page_dwg(&dir, &dwg, pages[0].id.clone(), 1.0, Some(junk)).unwrap_err();
        assert!(
            err.contains("own DWG reader refused") && err.contains("could not be read either"),
            "{err}"
        );

        // And when it works.
        import_page_dwg(&dir, &dwg, pages[0].id.clone(), 1.0, Some(copy)).unwrap();
        assert!(!folder().exists(), "removed after the import");
    }

    fn cli(dir: &Path, args: &[&str]) -> Res<()> {
        let mut all = vec!["wiringcraft", "-C", dir.to_str().unwrap()];
        all.extend_from_slice(args);
        run(Cli::try_parse_from(all).map_err(|e| e.to_string())?)
    }

    #[test]
    fn panel_rail_and_duct_add_list_and_remove() {
        use wc_core::panel::PanelItem;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        store::create(&dir, "Test").unwrap();
        cli(&dir, &["panel", "new", "main"]).unwrap();
        let items = || load(&dir).unwrap().panels[0].items.clone();
        let count = |rail: bool| {
            items()
                .iter()
                .filter(|i| matches!(i, PanelItem::Rail { .. }) == rail && !matches!(i, PanelItem::Device { .. }))
                .count()
        };
        let (rails, ducts) = (count(true), count(false));

        // Added at the end, and listed with the next number.
        cli(&dir, &["panel", "rail", "main", "--at", "50,700", "--length", "300"]).unwrap();
        assert_eq!(
            items().last(),
            Some(&PanelItem::Rail {
                at: Point::new(50.0, 700.0),
                length: 300.0
            })
        );
        cli(
            &dir,
            &[
                "panel",
                "duct",
                "main",
                "--at",
                "300,100",
                "--length",
                "500",
                "--vertical",
            ],
        )
        .unwrap();
        assert_eq!(
            items().last(),
            Some(&PanelItem::Duct {
                min: Point::new(300.0, 100.0),
                max: Point::new(340.0, 600.0)
            })
        );
        cli(
            &dir,
            &[
                "panel", "duct", "main", "--at", "0,760", "--length", "600", "--width", "40",
            ],
        )
        .unwrap();
        assert_eq!(count(true), rails + 1);
        assert_eq!(count(false), ducts + 2);
        let shown = panel_show(&load(&dir).unwrap().panels[0]);
        let rail_line = format!("{:<12} at 50,700  300 mm long", format!("rail {}", rails + 1));
        assert!(shown.contains(&rail_line), "{shown}");
        let duct_line = format!("{:<12} at 300,100  40 x 500 mm", format!("duct {}", ducts + 1));
        assert!(shown.contains(&duct_line), "{shown}");

        // Bad sizes, places off the plate and missing values are refused; nothing changes.
        let before = items();
        for args in [
            &["panel", "rail", "main", "--at", "50,700", "--length", "0"][..],
            &["panel", "rail", "main", "--at", "50,700", "--length", "NaN"],
            &["panel", "rail", "main", "--at", "400,700", "--length", "300"],
            &["panel", "rail", "main", "--at", "50,790", "--length", "100"],
            &["panel", "rail", "main", "--at", "50,700"],
            &["panel", "rail", "nope", "--at", "50,700", "--length", "100"],
            &[
                "panel", "duct", "main", "--at", "0,0", "--length", "100", "--width", "-5",
            ],
            &["panel", "duct", "main", "--at", "-1,0", "--length", "100"],
            &[
                "panel",
                "duct",
                "main",
                "--at",
                "0,500",
                "--length",
                "400",
                "--vertical",
            ],
            &[
                "panel", "duct", "main", "--at", "0,0", "--length", "100", "--remove", "1",
            ],
            &["panel", "rail", "main", "--remove", "0"],
            &["panel", "rail", "main", "--remove", "99"],
            &["panel", "duct", "main", "--remove", "99"],
        ] {
            assert!(cli(&dir, args).is_err(), "{args:?}");
        }
        assert_eq!(items(), before);

        // Removed by the number `panel show` gives; the others keep their order.
        let n = (rails + 1).to_string();
        cli(&dir, &["panel", "rail", "main", "--remove", &n]).unwrap();
        assert_eq!(count(true), rails);
        let n = (ducts + 1).to_string();
        cli(&dir, &["panel", "duct", "main", "--remove", &n]).unwrap();
        assert_eq!(count(false), ducts + 1);
        assert_eq!(
            items().last(),
            Some(&PanelItem::Duct {
                min: Point::new(0.0, 760.0),
                max: Point::new(600.0, 800.0)
            })
        );
        cli(&dir, &["panel", "rail", "main", "--remove", "1"]).unwrap();
        assert_eq!(count(true), rails - 1);
    }

    #[test]
    fn a_command_waits_while_another_holds_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        // Not a project yet: nothing to lock.
        assert!(project_lock(&dir).is_none());
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let held = project_lock(&dir).unwrap();
        assert_eq!(project_lock_path(&dir), dir.join(".wiringcraft.lock"));
        // In a repository, git is told to leave it out, once.
        let other = tmp.path().join("other");
        store::create(&other, "Other").unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&other)
            .status()
            .unwrap();
        assert!(init.success());
        drop(project_lock(&other).unwrap());
        drop(project_lock(&other).unwrap());
        let exclude = std::fs::read_to_string(other.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches("/.wiringcraft.lock").count(), 1);
        assert!(exclude.ends_with("/.wiringcraft.lock\n"));
        // Another project is not held up.
        cli(&other, &["template", "columns", "14"]).unwrap();
        let toml = dir.join(store::PROJECT_FILE);
        let before = std::fs::read_to_string(&toml).unwrap();
        let second = {
            let dir = dir.clone();
            std::thread::spawn(move || cli(&dir, &["template", "columns", "14"]))
        };
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert!(!second.is_finished(), "the second command waits");
        assert_eq!(std::fs::read_to_string(&toml).unwrap(), before);
        drop(held);
        second.join().unwrap().unwrap();
        assert_ne!(std::fs::read_to_string(&toml).unwrap(), before);
    }

    #[test]
    fn commands_on_either_side_of_vcs_init_wait_on_one_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let spawn = |args: &'static [&'static str]| {
            let dir = dir.clone();
            std::thread::spawn(move || cli(&dir, args))
        };
        let pause = || std::thread::sleep(std::time::Duration::from_millis(400));
        // Taken before the project is a repository, as `vcs init` takes it.
        let held = project_lock(&dir).unwrap();
        let before = project_lock_path(&dir);
        // One command starts waiting before .git exists ...
        let early = spawn(&["page", "add", "p2", "--number", "2"]);
        pause();
        wc_vcs::Git::new(&dir).init().unwrap();
        assert!(dir.join(".git").exists());
        assert_eq!(project_lock_path(&dir), before, "the same lock file once .git exists");
        // ... and another after: both wait for the lock taken before init.
        let late = spawn(&["page", "add", "p3", "--number", "3"]);
        pause();
        assert!(!early.is_finished(), "the earlier command waits");
        assert!(!late.is_finished(), "the later command waits");
        assert!(load(&dir).unwrap().page("p2").is_none());
        assert!(load(&dir).unwrap().page("p3").is_none());
        drop(held);
        early.join().unwrap().unwrap();
        late.join().unwrap().unwrap();
        // One after the other: neither overwrote the other's page.
        let p = load(&dir).unwrap();
        assert!(p.page("p2").is_some() && p.page("p3").is_some());
    }

    #[test]
    fn delete_takes_symbols_and_wires_off_a_page_with_the_wires_on_them() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["page", "add", "p1", "--number", "1"]).unwrap();
        for (at, tag) in [("40,40", "-A1"), ("100,40", "-A2")] {
            let args = ["box", "p1", "--at", at, "--size", "20,20", "--tag", tag];
            cli(&dir, &[&args[..], &["--pin", "1:top:10"]].concat()).unwrap();
        }
        // w1 runs between the boxes, w2 from s2 into the air, w3 from point to point.
        cli(&dir, &["wire", "p1", "s1:1", "s2:1"]).unwrap();
        cli(&dir, &["wire", "p1", "s2:1", "150,20"]).unwrap();
        cli(&dir, &["wire", "p1", "10,10", "20,10"]).unwrap();
        let ids = |dir: &Path| {
            let p = load(dir).unwrap();
            let pg = p.page("p1").unwrap();
            let mut ids: Vec<String> = pg.symbols.iter().map(|s| s.id.clone()).collect();
            ids.extend(pg.wires.iter().map(|w| w.id.clone()));
            ids
        };

        // An id that is not there changes nothing, though the others are.
        let err = cli(&dir, &["delete", "p1", "w3", "nope", "s9"]).unwrap_err();
        assert_eq!(err, "no symbol or wire \"nope\", \"s9\" on page p1");
        assert_eq!(ids(&dir), ["s1", "s2", "w1", "w2", "w3"]);
        assert!(
            cli(&dir, &["delete", "p9", "w3"])
                .unwrap_err()
                .contains("no page \"p9\"")
        );

        // A wire alone.
        let mut p = load(&dir).unwrap();
        let deleted = delete_items(p.page_mut("p1").unwrap(), &["w3".into()]).unwrap();
        assert_eq!(deleted, ["w3"]);
        cli(&dir, &["delete", "p1", "w3"]).unwrap();
        assert_eq!(ids(&dir), ["s1", "s2", "w1", "w2"]);

        // A symbol takes the wires on its pins along; the other symbol and its device stay.
        let mut p = load(&dir).unwrap();
        let deleted = delete_items(p.page_mut("p1").unwrap(), &["s2".into()]).unwrap();
        assert_eq!(deleted, ["s2", "w1", "w2"]);
        cli(&dir, &["delete", "p1", "s2"]).unwrap();
        assert_eq!(ids(&dir), ["s1"]);
        let p = load(&dir).unwrap();
        assert!(
            p.devices.contains_key(&DeviceTag::parse("-A2").unwrap()),
            "the device keeps its data"
        );
    }

    #[test]
    fn a_wire_to_a_pin_the_symbol_does_not_have_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["page", "add", "p1", "--number", "1"]).unwrap();
        let args = ["box", "p1", "--at", "50,50", "--size", "20,20", "--tag", "-F1"];
        cli(
            &dir,
            &[&args[..], &["--pin", "1:top:10", "--pin", "2:bottom:10"]].concat(),
        )
        .unwrap();
        let s1 = load(&dir).unwrap().page("p1").unwrap().symbols[0].id.clone();

        let err = cli(&dir, &["wire", "p1", &format!("{s1}:99"), &format!("{s1}:1")]).unwrap_err();
        assert_eq!(
            err,
            format!("symbol {s1} (-F1) on page p1 has no pin \"99\"; its pins are 1, 2")
        );
        assert!(
            load(&dir).unwrap().page("p1").unwrap().wires.is_empty(),
            "nothing saved"
        );

        cli(&dir, &["wire", "p1", &format!("{s1}:2"), &format!("{s1}:1")]).unwrap();
        assert_eq!(load(&dir).unwrap().page("p1").unwrap().wires.len(), 1);

        // The listed pin names are escaped, so a control character cannot reach the terminal.
        let symbol_id = load(&dir).unwrap().page("p1").unwrap().symbols[0].symbol.clone();
        let vendored = store::vendor_path(&dir, ItemKind::Symbol, &symbol_id).unwrap();
        let mut symbol = store::read_vendored_symbol(&dir, &symbol_id).unwrap();
        symbol.pins[1].name = "x\u{1b}y".into();
        store::write_vendored_symbol(&dir, &symbol).unwrap();
        let err = cli(&dir, &["wire", "p1", &format!("{s1}:99"), &format!("{s1}:1")]).unwrap_err();
        assert!(
            err.ends_with("its pins are 1, x\\u{1b}y") && !err.contains('\u{1b}'),
            "{err}"
        );

        // A definition that cannot be read is an error naming the placed symbol, not a pass;
        // point ends do not read it.
        std::fs::write(&vendored, "not json").unwrap();
        let err = cli(&dir, &["wire", "p1", &format!("{s1}:1"), "0,0"]).unwrap_err();
        assert!(
            err.starts_with(&format!(
                "symbol {s1} (-F1) on page p1: its definition {symbol_id} could not be read"
            )),
            "{err}"
        );
        cli(&dir, &["wire", "p1", "0,0", "10,0"]).unwrap();
        assert_eq!(load(&dir).unwrap().page("p1").unwrap().wires.len(), 2);

        // A symbol with no copy yet (as before `lib lock`) leaves the pin unchecked.
        std::fs::remove_file(&vendored).unwrap();
        cli(&dir, &["wire", "p1", &format!("{s1}:1"), "0,0"]).unwrap();
        assert_eq!(load(&dir).unwrap().page("p1").unwrap().wires.len(), 3);
    }

    #[test]
    fn device_with_only_a_tag_shows_it_and_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let mut p = load(&dir).unwrap();
        let k1 = DeviceTag::parse("-K1").unwrap();
        p.devices.insert(
            k1.clone(),
            wc_core::Device {
                part: Some("maker.contactor".into()),
                description: "Pump motor".into(),
                ..Default::default()
            },
        );
        // -K2 is drawn on a page but not in the devices table.
        p.pages[0].symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: "wc.coil".into(),
            at: Point::new(10.0, 10.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(DeviceTag::parse("-K2").unwrap()),
        });
        save(&dir, &p).unwrap();
        let files = |d: &Path| {
            let mut v: Vec<_> = std::fs::read_dir(d)
                .unwrap()
                .map(|f| f.unwrap().path())
                // The lock file appears with the first command run; it is not project data.
                .filter(|f| !f.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
                .map(|f| {
                    let body = if f.is_file() {
                        std::fs::read(&f).unwrap()
                    } else {
                        Vec::new()
                    };
                    (f, body)
                })
                .collect();
            v.sort();
            v
        };
        let before = files(&dir);

        let p = load(&dir).unwrap();
        assert_eq!(
            device_info(&p, &k1).unwrap(),
            "part         maker.contactor\ndescription  Pump motor\n"
        );
        let k2 = DeviceTag::parse("-K2").unwrap();
        assert_eq!(device_info(&p, &k2).unwrap(), "part         -\ndescription  -\n");
        cli(&dir, &["device", "-K1"]).unwrap();
        cli(&dir, &["device", "-K2"]).unwrap();
        let err = cli(&dir, &["device", "-K9"]).unwrap_err();
        assert_eq!(err, "no device -K9");
        assert_eq!(files(&dir), before, "showing a device saves nothing");
        let p = load(&dir).unwrap();
        assert!(!p.devices.contains_key(&DeviceTag::parse("-K9").unwrap()));
        assert!(!p.devices.contains_key(&k2));

        // An option still edits, and creates a device that was not there.
        cli(&dir, &["device", "-K9", "--description", "Spare"]).unwrap();
        let p = load(&dir).unwrap();
        assert_eq!(p.devices[&DeviceTag::parse("-K9").unwrap()].description, "Spare");
    }

    #[test]
    fn a_panel_is_renamed_and_resized_keeping_its_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["device", "-K1", "--description", "Pump"]).unwrap();
        cli(&dir, &["panel", "new", "main"]).unwrap();
        cli(&dir, &["panel", "device", "main", "-K1", "--at", "100,600"]).unwrap();
        let before = load(&dir).unwrap().panels[0].clone();

        cli(
            &dir,
            &["panel", "edit", "main", "--name", "Main plate", "--height", "1000"],
        )
        .unwrap();
        let after = load(&dir).unwrap().panels[0].clone();
        assert_eq!(after.name, "Main plate");
        assert_eq!((after.width, after.height), (600.0, 1000.0));
        assert_eq!(after.items, before.items, "rails, ducts and devices are not rebuilt");

        // Too small for the rails, or for the device at y 600..690; out of range; nothing to do;
        // no such panel: refused, nothing changes.
        let err = cli(&dir, &["panel", "edit", "main", "--height", "650"]).unwrap_err();
        assert!(err.contains("sticking out") && err.contains("-K1"), "{err}");
        assert!(cli(&dir, &["panel", "edit", "main", "--width", "300"]).is_err());
        assert!(cli(&dir, &["panel", "edit", "main", "--width", "100"]).is_err());
        assert!(cli(&dir, &["panel", "edit", "main", "--width", "NaN"]).is_err());
        assert!(cli(&dir, &["panel", "edit", "main"]).is_err());
        assert!(cli(&dir, &["panel", "edit", "nope", "--name", "x"]).is_err());
        assert_eq!(load(&dir).unwrap().panels[0], after);

        // An empty name falls back to the id, as `panel new` does.
        cli(&dir, &["panel", "edit", "main", "--name", ""]).unwrap();
        assert_eq!(load(&dir).unwrap().panels[0].name, "main");

        // A device taller than the plate (as `panel arrange` can leave it) does not block a
        // name-only edit, which keeps the plate size; resizing is still refused.
        let mut p = load(&dir).unwrap();
        for item in &mut p.panels[0].items {
            if let wc_core::panel::PanelItem::Device { at, height, .. } = item {
                *at = Point::new(100.0, 0.0);
                *height = 1200.0;
            }
        }
        store::save(&dir, &p).unwrap();
        cli(&dir, &["panel", "edit", "main", "--name", "Tall"]).unwrap();
        let after = load(&dir).unwrap().panels[0].clone();
        assert_eq!(after.name, "Tall");
        assert_eq!((after.width, after.height), (600.0, 1000.0));
        assert!(cli(&dir, &["panel", "edit", "main", "--name", "x", "--width", "700"]).is_err());
        assert_eq!(load(&dir).unwrap().panels[0].name, "Tall");
    }

    #[test]
    fn a_device_is_placed_turned_moved_and_taken_off_a_plate() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let lib = tmp.path().join("lib");
        let mut part = wc_core::Part {
            id: "maker.contactor".into(),
            ..wc_core::Part::default()
        };
        part.properties.insert("Width".into(), "45".into());
        part.properties.insert("Height".into(), "85".into());
        wc_library::write_part(&lib, &part).unwrap();
        cli(&dir, &["lib", "add", "shop", "folder", lib.to_str().unwrap()]).unwrap();
        cli(&dir, &["device", "--part", "maker.contactor", "--", "+CAB1-K1"]).unwrap();
        cli(&dir, &["lib", "lock"]).unwrap();
        cli(&dir, &["panel", "new", "main"]).unwrap();

        let k1 = DeviceTag::parse("+CAB1-K1").unwrap();
        let on_plate = || -> Option<(Point, f64, f64, Rotation)> {
            let p = load(&dir).unwrap();
            match p.panels[0].device(&k1) {
                Some(wc_core::panel::PanelItem::Device {
                    at,
                    width,
                    height,
                    rotation,
                    ..
                }) => Some((*at, *width, *height, *rotation)),
                _ => None,
            }
        };

        // Not on the plate yet, so a place is needed; an unknown device is refused.
        assert!(cli(&dir, &["panel", "device", "main", "+CAB1-K1"]).is_err());
        assert!(cli(&dir, &["panel", "device", "main", "-K9", "--at", "100,100"]).is_err());
        cli(&dir, &["panel", "device", "main", "+CAB1-K1", "--at", "100,200"]).unwrap();
        assert_eq!(on_plate(), Some((Point::new(100.0, 200.0), 45.0, 85.0, Rotation::R0)));

        // Turned in place, then moved keeping the turn.
        cli(&dir, &["panel", "device", "main", "+CAB1-K1", "--rotate", "90"]).unwrap();
        assert_eq!(on_plate(), Some((Point::new(100.0, 200.0), 45.0, 85.0, Rotation::R90)));
        cli(&dir, &["panel", "device", "main", "+CAB1-K1", "--at", "300,50"]).unwrap();
        assert_eq!(on_plate(), Some((Point::new(300.0, 50.0), 45.0, 85.0, Rotation::R90)));

        // Off the plate's edge (turned it is 85 mm wide on a 600 mm plate), or a bad turn:
        // refused, nothing changes.
        assert!(cli(&dir, &["panel", "device", "main", "+CAB1-K1", "--at", "520,0"]).is_err());
        assert!(cli(&dir, &["panel", "device", "main", "+CAB1-K1", "--rotate", "45"]).is_err());
        assert_eq!(on_plate(), Some((Point::new(300.0, 50.0), 45.0, 85.0, Rotation::R90)));
        cli(&dir, &["panel", "show", "main"]).unwrap();
        let mut p = load(&dir).unwrap();
        p.panels[0].name = "Main\u{1b}]52;c;x\u{7}".into();
        let shown = panel_show(&p.panels[0]);
        assert!(shown.starts_with("main Main"), "{shown}");
        assert!(shown.contains("(600 x 800 mm)"), "{shown}");
        assert!(!shown.contains('\u{1b}'), "control characters are escaped: {shown}");
        assert!(
            shown.contains("+CAB1-K1     at 300,50  45 x 85 mm  turned 90"),
            "{shown}"
        );

        cli(&dir, &["panel", "device", "main", "+CAB1-K1", "--remove"]).unwrap();
        assert_eq!(on_plate(), None);
        assert!(cli(&dir, &["panel", "device", "main", "+CAB1-K1", "--remove"]).is_err());
        assert!(
            cli(
                &dir,
                &["panel", "device", "main", "+CAB1-K1", "--remove", "--at", "1,1"]
            )
            .is_err()
        );

        // A damaged pinned copy of the part: the reason is named, and the device is still
        // placed, with the default size.
        let copy = store::vendor_path(&dir, ItemKind::Part, "maker.contactor").unwrap();
        std::fs::write(&copy, "{ not json").unwrap();
        let p = load(&dir).unwrap();
        let prefix = "cannot read part maker.contactor of +CAB1-K1: ";
        let err = device_part_size(&dir, &p, &k1).unwrap_err();
        assert!(err.starts_with(prefix) && err.len() > prefix.len(), "{err}");
        cli(&dir, &["panel", "device", "main", "+CAB1-K1", "--at", "100,200"]).unwrap();
        assert_eq!(on_plate(), Some((Point::new(100.0, 200.0), 18.0, 90.0, Rotation::R0)));

        // A part id with control characters, as a hand-edited project may hold, is escaped.
        let mut p = load(&dir).unwrap();
        if let Some(d) = p.devices.get_mut(&k1) {
            d.part = Some("bad\u{1b}]52;c;x\u{7}".into());
        }
        let err = device_part_size(&dir, &p, &k1).unwrap_err();
        assert!(!err.contains('\u{1b}') && err.contains("\\u{1b}"), "{err}");
    }

    #[test]
    fn a_maker_step_model_sizes_a_library_part_on_the_plate_and_in_depth() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let lib = tmp.path().join("lib");
        let part = wc_core::Part {
            id: "maker.relay".into(),
            ..wc_core::Part::default()
        };
        wc_library::write_part(&lib, &part).unwrap();
        cli(&dir, &["lib", "add", "shop", "folder", lib.to_str().unwrap()]).unwrap();
        cli(&dir, &["device", "--part", "maker.relay", "--", "+CAB1-K1"]).unwrap();
        cli(&dir, &["lib", "lock"]).unwrap();

        // A 22.5 x 90 x 70 mm box in metres, as the maker's model.
        let mut data = String::new();
        let mut id = 1;
        for x in [0.0, 0.0225] {
            for y in [0.0, 0.09] {
                for z in [0.0, 0.07] {
                    data.push_str(&format!(
                        "#{id}=CARTESIAN_POINT('',({x:?},{y:?},{z:?}));#{}=VERTEX_POINT('',#{id});\n",
                        id + 1
                    ));
                    id += 2;
                }
            }
        }
        let stp = tmp.path().join("relay.stp");
        std::fs::write(
            &stp,
            format!(
                "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#900=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT($,.METRE.));\n{data}ENDSEC;\nEND-ISO-10303-21;\n"
            ),
        )
        .unwrap();
        let file = stp.to_str().unwrap();
        assert!(cli(&dir, &["import", "model-step", file, "--part", "no.such"]).is_err());
        cli(&dir, &["import", "model-step", file, "--part", "maker.relay"]).unwrap();
        let in_lib: wc_core::Part = store::read_json(&lib.join("parts").join("maker.relay.json")).unwrap();
        let Some(m) = in_lib.model else {
            unreachable!("stored in the library with the part")
        };
        assert!((m.width - 22.5).abs() < 1e-9 && (m.height - 90.0).abs() < 1e-9 && (m.depth - 70.0).abs() < 1e-9);
        // The project's pinned copy follows.
        let copy = store::read_vendored_part(&dir, "maker.relay").unwrap();
        assert_eq!(copy, in_lib);
        assert!((wc_core::panel::part_depth(&copy).unwrap() - 70.0).abs() < 1e-9);

        // The part has no Width/Height: arranging the plate sizes the device from the model.
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
        assert_eq!(sizes.len(), 1);
        assert!(
            (sizes[0].0 - 22.5).abs() < 1e-9 && (sizes[0].1 - 90.0).abs() < 1e-9,
            "{sizes:?}"
        );

        // The project's own pinned copy named as the library: refused before anything is written.
        let vendor = dir.join(store::VENDOR_DIR);
        let copy_before = std::fs::read(store::vendor_path(&dir, ItemKind::Part, "maker.relay").unwrap()).unwrap();
        let err = cli(
            &dir,
            &[
                "import",
                "model-step",
                file,
                "--part",
                "maker.relay",
                "--lib",
                vendor.to_str().unwrap(),
            ],
        )
        .unwrap_err();
        assert!(err.contains("pinned copy"), "{err}");
        assert_eq!(
            std::fs::read(store::vendor_path(&dir, ItemKind::Part, "maker.relay").unwrap()).unwrap(),
            copy_before
        );

        // A part the project pins but no longer uses: the library changes, the pinned copy and
        // its pin stay as they are.
        let spare = wc_core::Part {
            id: "maker.spare".into(),
            ..wc_core::Part::default()
        };
        wc_library::write_part(&lib, &spare).unwrap();
        cli(&dir, &["device", "--part", "maker.spare", "--", "+CAB1-K2"]).unwrap();
        cli(&dir, &["lib", "lock"]).unwrap();
        cli(&dir, &["device", "--part", "maker.relay", "--", "+CAB1-K2"]).unwrap();
        let spare_pin = load(&dir).unwrap().lock.parts["maker.spare"].hash.clone();
        let spare_copy = std::fs::read(store::vendor_path(&dir, ItemKind::Part, "maker.spare").unwrap()).unwrap();
        cli(&dir, &["import", "model-step", file, "--part", "maker.spare"]).unwrap();
        let spare_lib: wc_core::Part = store::read_json(&lib.join("parts").join("maker.spare.json")).unwrap();
        assert!(spare_lib.model.is_some(), "the library part changes");
        assert_eq!(load(&dir).unwrap().lock.parts["maker.spare"].hash, spare_pin);
        assert_eq!(
            std::fs::read(store::vendor_path(&dir, ItemKind::Part, "maker.spare").unwrap()).unwrap(),
            spare_copy
        );

        // Not a STEP file: refused, the part is left as it was.
        let bad = tmp.path().join("bad.stp");
        std::fs::write(&bad, "solid nothing").unwrap();
        assert!(
            cli(
                &dir,
                &["import", "model-step", bad.to_str().unwrap(), "--part", "maker.relay"]
            )
            .is_err()
        );
        let still: wc_core::Part = store::read_json(&lib.join("parts").join("maker.relay.json")).unwrap();
        assert_eq!(still, in_lib);
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

        // When the new pin cannot be saved, the copy goes back as it was, so copy and pin agree.
        let wider = tmp.path().join("io-8-wide.dxf");
        std::fs::write(
            &wider,
            "0\nSECTION\n2\nENTITIES\n0\nLWPOLYLINE\n8\n0\n90\n4\n70\n1\n\
             10\n0\n20\n0\n10\n45\n20\n0\n10\n45\n20\n100\n10\n0\n20\n100\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        let copy_before = std::fs::read(store::vendor_path(&dir, ItemKind::Part, "maker.io-8").unwrap()).unwrap();
        let blocker = dir.join(format!("{}.tmp", store::LOCK_FILE));
        std::fs::create_dir(&blocker).unwrap();
        let wide = wider.to_str().unwrap();
        assert!(cli(&dir, &["import", "footprint-dxf", wide, "--part", "maker.io-8"]).is_err());
        std::fs::remove_dir(&blocker).unwrap();
        assert_eq!(
            std::fs::read(store::vendor_path(&dir, ItemKind::Part, "maker.io-8").unwrap()).unwrap(),
            copy_before
        );
        assert_eq!(load(&dir).unwrap().lock.parts["maker.io-8"].hash, pinned);
        // Only the lock is saved with the copy: a panel file that cannot be written does not
        // block the refresh.
        let panel_file = dir.join("panels").join("main.panel.json");
        let panel_text = std::fs::read_to_string(&panel_file).unwrap();
        std::fs::write(&panel_file, format!("{panel_text}\n\n")).unwrap();
        let panel_tmp = dir.join("panels").join("main.panel.json.tmp");
        std::fs::create_dir(&panel_tmp).unwrap();
        let lock_bytes = std::fs::read(dir.join(store::LOCK_FILE)).unwrap();
        cli(&dir, &["import", "footprint-dxf", wide, "--part", "maker.io-8"]).unwrap();
        std::fs::remove_dir(&panel_tmp).unwrap();
        assert_ne!(load(&dir).unwrap().lock.parts["maker.io-8"].hash, pinned);
        // Back to the earlier footprint for what follows.
        let copy_file = store::vendor_path(&dir, ItemKind::Part, "maker.io-8").unwrap();
        let lock_file = dir.join(store::LOCK_FILE);
        std::fs::write(&copy_file, &copy_before).unwrap();
        std::fs::write(&lock_file, &lock_bytes).unwrap();
        // When the copy cannot be put back after the lock failed (every staging name is
        // taken), the error names it.
        let staged = |f: &Path| -> Vec<PathBuf> {
            (0..100)
                .map(|n| {
                    let mut name = f.as_os_str().to_owned();
                    name.push(format!(".restore-{}-{n}", std::process::id()));
                    PathBuf::from(name)
                })
                .collect()
        };
        let taken: Vec<PathBuf> = staged(&copy_file).into_iter().chain(staged(&lock_file)).collect();
        for t in &taken {
            std::fs::write(t, "").unwrap();
        }
        std::fs::create_dir(&blocker).unwrap();
        let err = cli(&dir, &["import", "footprint-dxf", wide, "--part", "maker.io-8"]).unwrap_err();
        std::fs::remove_dir(&blocker).unwrap();
        for t in &taken {
            std::fs::remove_file(t).unwrap();
        }
        assert!(err.contains("could not be put back"), "{err}");
        assert!(err.contains(&copy_file.display().to_string()), "{err}");
        assert!(err.contains(&lock_file.display().to_string()), "{err}");
        std::fs::write(&copy_file, &copy_before).unwrap();
        std::fs::write(&lock_file, &lock_bytes).unwrap();
        std::fs::write(&panel_file, &panel_text).unwrap();
        // And when `take` itself fails (its staged write is blocked): the import still
        // succeeds with a warning, and the copy and its pin are as they were.
        let copy_file = store::vendor_path(&dir, ItemKind::Part, "maker.io-8").unwrap();
        let copy_tmp = copy_file.with_file_name(format!(
            "{}.tmp",
            copy_file.file_name().and_then(|n| n.to_str()).unwrap_or_default()
        ));
        std::fs::create_dir(&copy_tmp).unwrap();
        cli(&dir, &["import", "footprint-dxf", wide, "--part", "maker.io-8"]).unwrap();
        std::fs::remove_dir(&copy_tmp).unwrap();
        assert_eq!(std::fs::read(&copy_file).unwrap(), copy_before);
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
        // Nor a link that points nowhere: it stays a link, and the pin is kept.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.path().join("gone.json"), &copy_path).unwrap();
            cli(&dir, &["import", "footprint-dxf", file, "--part", "maker.io-8"]).unwrap();
            assert!(std::fs::symlink_metadata(&copy_path).unwrap().file_type().is_symlink());
            assert_eq!(load(&dir).unwrap().lock.parts["maker.io-8"].hash, pinned);
            std::fs::remove_file(&copy_path).unwrap();
        }
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
    fn a_page_is_renumbered_and_retitled() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["page", "add", "a", "--number", "2", "--title", "Power"]).unwrap();
        cli(&dir, &["page", "add", "b", "--number", "3", "--title", "Control"]).unwrap();
        let pages = |dir: &Path| -> Vec<(String, String, String)> {
            load(dir)
                .unwrap()
                .pages
                .iter()
                .map(|p| (p.id.clone(), p.number.clone(), p.title.clone()))
                .collect()
        };
        cli(&dir, &["page", "edit", "b", "--number", " 1.5 "]).unwrap();
        cli(&dir, &["page", "edit", "a", "--title", "Main power"]).unwrap();
        let got = pages(&dir);
        let ids: Vec<&str> = got.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(ids, ["p0001", "b", "a"], "sorted by the new number");
        assert_eq!(got[1], ("b".into(), "1.5".into(), "Control".into()));
        assert_eq!(got[2], ("a".into(), "2".into(), "Main power".into()));
        // Refused, and nothing changes.
        let before = pages(&dir);
        let err = cli(&dir, &["page", "edit", "a", "--number", "1"]).unwrap_err();
        assert!(err.contains("already has number"), "{err}");
        assert!(
            cli(&dir, &["page", "edit", "a", "--number", "  "])
                .unwrap_err()
                .contains("empty")
        );
        assert!(
            cli(&dir, &["page", "edit", "nope", "--title", "X"])
                .unwrap_err()
                .contains("no page")
        );
        assert!(cli(&dir, &["page", "edit", "a"]).is_err());
        assert_eq!(pages(&dir), before);
        // Its own number again is fine.
        cli(&dir, &["page", "edit", "a", "--number", "2", "--title", "Power"]).unwrap();
        assert_eq!(pages(&dir)[2], ("a".into(), "2".into(), "Power".into()));
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
    fn page_remove_keeps_the_last_page_of_a_project() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let ids = |dir: &Path| -> Vec<String> { load(dir).unwrap().pages.iter().map(|p| p.id.clone()).collect() };
        let only = ids(&dir);
        assert_eq!(only.len(), 1);
        let err = cli(&dir, &["page", "remove", &only[0]]).unwrap_err();
        assert!(err.contains("at least one page"), "{err}");
        assert_eq!(ids(&dir), only);
        // With two pages, removing one works.
        cli(&dir, &["page", "add", "p0002", "--number", "2"]).unwrap();
        cli(&dir, &["page", "remove", &only[0]]).unwrap();
        assert_eq!(ids(&dir), ["p0002"]);
        assert!(cli(&dir, &["page", "remove", "nope"]).unwrap_err().contains("no page"));
    }

    #[test]
    fn pdf_and_drawing_refuse_an_unknown_page_and_write_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let first = load(&dir).unwrap().pages[0].id.clone();
        let pdf = tmp.path().join("out.pdf");
        let err = cli(&dir, &["pdf", pdf.to_str().unwrap(), "--page", "nope"]).unwrap_err();
        assert!(err.contains("no page \"nope\"") && err.contains(&first), "{err}");
        assert!(!pdf.exists());
        let drawings = tmp.path().join("dr");
        let err = cli(
            &dir,
            &[
                "drawing",
                drawings.to_str().unwrap(),
                "--page",
                &first,
                "--page",
                "nope",
            ],
        )
        .unwrap_err();
        assert!(err.contains("no page \"nope\""), "{err}");
        assert!(!drawings.exists());
        // A page that exists still exports.
        cli(&dir, &["pdf", pdf.to_str().unwrap(), "--page", &first]).unwrap();
        assert!(pdf.exists());
        // The listed ids are escaped, so a control character in one cannot steer the terminal.
        let mut p = load(&dir).unwrap();
        p.pages[0].id = "a\u{1b}[2Jb".into();
        let err = check_pages(&p, &["nope".into()]).unwrap_err();
        assert!(err.contains("a\\u{1b}[2Jb") && !err.contains('\u{1b}'), "{err}");
    }

    #[test]
    fn drawing_writes_a_file_per_panel_layout_next_to_the_pages() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["panel", "new", "plate1", "--name", "Main cabinet/A"]).unwrap();
        let number = load(&dir).unwrap().pages[0].number.clone();
        for ext in ["dxf", "svg"] {
            let out = tmp.path().join(ext);
            cli(&dir, &["drawing", out.to_str().unwrap(), "--format", ext]).unwrap();
            let mut names: Vec<String> = std::fs::read_dir(&out)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            let page = format!("{number}.{ext}");
            let mut want = [format!("panel-Main_cabinet_A.{ext}"), page];
            want.sort();
            assert_eq!(names, want, "{ext}");
        }
        let dxf = std::fs::read_to_string(tmp.path().join("dxf").join("panel-Main_cabinet_A.dxf")).unwrap();
        assert!(dxf.contains("POLYLINE") && dxf.contains("Main cabinet/A"), "{dxf}");
        // Names that differ only in case get files of their own, also on Windows and macOS.
        cli(&dir, &["panel", "new", "plate2", "--name", "Main"]).unwrap();
        cli(&dir, &["panel", "new", "plate3", "--name", "main"]).unwrap();
        let out = tmp.path().join("cased");
        cli(&dir, &["drawing", out.to_str().unwrap(), "--format", "dxf"]).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_lowercase())
            .collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 4, "{names:?}");
        assert!(names.contains(&"panel-main.dxf".into()) && names.contains(&"panel-main_.dxf".into()));
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
    fn find_takes_a_tag_and_a_replacement_that_start_with_a_dash() {
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
        let parsed = Cli::try_parse_from(["wiringcraft", "find", "-K1", "--replace", "-K2", "--match-case"]).unwrap();
        let Cmd::Find {
            query,
            replace,
            match_case,
        } = parsed.cmd
        else {
            unreachable!()
        };
        assert_eq!(
            (query.as_str(), replace.as_deref(), match_case),
            ("-K1", Some("-K2"), true)
        );
        cli(&dir, &["find", "-K1"]).unwrap();
        cli(&dir, &["find", "--replace", "-K2", "-K1"]).unwrap();
        let p = load(&dir).unwrap();
        let k2 = wc_core::DeviceTag::parse("-K2").unwrap();
        assert_eq!(p.pages[0].symbols[0].tag.as_ref(), Some(&k2));
        assert!(p.devices.contains_key(&k2));
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
                ..Default::default()
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

        // A lock that cannot be written after both copies were taken puts them back, and the
        // page is not saved.
        let page_bytes =
            std::fs::read(dir.join(store::PAGES_DIR).join(format!("{page}{}", store::PAGE_SUFFIX))).unwrap();
        let lock_blocker = dir.join(format!("{}.tmp", store::LOCK_FILE));
        std::fs::create_dir(&lock_blocker).unwrap();
        cli(&dir, &args).unwrap_err();
        std::fs::remove_dir(&lock_blocker).unwrap();
        assert!(store::read_vendored_symbol(&dir, "aa.good").is_err(), "copy put back");
        assert!(store::read_vendored_symbol(&dir, "zz.bad").is_err(), "copy put back");
        let p = load(&dir).unwrap();
        assert!(!p.lock.symbols.contains_key("aa.good"));
        assert!(p.page(&page).is_some_and(|pg| pg.symbols.is_empty()), "page not saved");
        assert_eq!(
            std::fs::read(dir.join(store::PAGES_DIR).join(format!("{page}{}", store::PAGE_SUFFIX))).unwrap(),
            page_bytes
        );

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
        // The network list is followed by a topology page per network; another report has none.
        let topology = format!("/Title {}", hex("N1 Network PN1"));
        assert!(all.contains(&topology));
        assert!(pdf("networks").contains(&topology));
        assert!(!pdf("parts").contains(&topology));
        // The list comes first: its bookmark is before the topology page's.
        let titles = |text: &str| -> Vec<String> {
            text.split("/Title <")
                .skip(1)
                .filter_map(|s| s.split_once('>'))
                .map(|(h, _)| {
                    (0..h.len() / 2)
                        .filter_map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok())
                        .map(char::from)
                        .collect()
                })
                .collect()
        };
        let networks = titles(&pdf("networks"));
        let at = |t: &str| networks.iter().position(|x| x.ends_with(t));
        let (Some(list), Some(page)) = (at(" Networks"), at("N1 Network PN1")) else {
            unreachable!("both bookmarks are there: {networks:?}");
        };
        assert!(list < page, "{networks:?}");
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

        // One text set and removed from the command line.
        cli(
            &dir,
            &["translations", "set", "Main switch", "Hovedbryter 1", "--lang", "nb"],
        )
        .unwrap();
        cli(&dir, &["translations", "--lang", "de", "set", "Pumps", "Pumpen"]).unwrap();
        let dict = wc_core::translate::Dictionary::load(&dir).unwrap();
        assert_eq!(dict.lookup("Main switch", "nb").as_deref(), Some("Hovedbryter 1"));
        assert_eq!(dict.lookup("Pumps", "de").as_deref(), Some("Pumpen"));
        cli(&dir, &["translations", "remove", "Cover", "--lang", "nb"]).unwrap();
        let err = cli(&dir, &["translations", "--lang", "nb"]).unwrap_err();
        assert!(err.contains("1 text(s)"), "{err}");
        assert!(
            cli(&dir, &["translations", "set", "Pumps", "x"]).is_err(),
            "no language"
        );
        assert!(cli(&dir, &["translations", "set", "Pumps", "x", "--lang", "en"]).is_err());
        assert!(cli(&dir, &["translations", "set", "Pumps", " ", "--lang", "nb"]).is_err());

        // A folder that is not a project is refused, and its translations.toml left alone.
        let other = tmp.path().join("not-a-project");
        std::fs::create_dir(&other).unwrap();
        let file = other.join(wc_core::translate::TRANSLATIONS_FILE);
        let text = "source = \"en\"\n";
        std::fs::write(&file, text).unwrap();
        assert!(cli(&other, &["translations", "set", "Pumps", "Pumper", "--lang", "nb"]).is_err());
        assert!(cli(&other, &["translations", "remove", "Pumps", "--lang", "nb"]).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
        let empty = tmp.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(cli(&empty, &["translations", "set", "Pumps", "Pumper", "--lang", "nb"]).is_err());
        assert!(!empty.join(wc_core::translate::TRANSLATIONS_FILE).exists());
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

    /// Draws terminals `-X1.1` to `-X1.5` on the first page, so the strip `-X1` exists.
    fn draw_x1_terminals(dir: &Path) {
        let mut p = load(dir).unwrap();
        for k in 1..=5 {
            p.pages[0].symbols.push(PlacedSymbol {
                id: format!("x1t{k}"),
                symbol: "wc.terminal".into(),
                at: Point::new(10.0 * k as f64, 10.0),
                rotation: Rotation::default(),
                mirror: false,
                tag: DeviceTag::parse(&format!("-X1.{k}")).ok(),
            });
        }
        save(dir, &p).unwrap();
    }

    #[test]
    fn jumpers_are_added_listed_and_removed_from_the_command_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let err = cli(&dir, &["jumper", "add", "--", "-X1", "3", "4"]).unwrap_err();
        assert_eq!(err, "no terminal strip -X1");
        assert!(load(&dir).unwrap().meta.jumpers.is_empty());
        draw_x1_terminals(&dir);
        let err = cli(&dir, &["jumper", "add", "--", "-X1", "3", "99"]).unwrap_err();
        assert_eq!(err, "-X1 has no terminal 99");
        assert!(load(&dir).unwrap().meta.jumpers.is_empty(), "nothing saved");
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
    fn rack_commands_add_list_edit_and_remove_racks_and_cards() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let racks = || load(&dir).unwrap().meta.racks;
        cli(&dir, &["rack", "add", "-A1", "--description", "Main PLC"]).unwrap();
        let err = cli(&dir, &["rack", "add", "-A1"]).unwrap_err();
        assert!(err.contains("there already is a rack -A1"), "{err}");
        let add = ["rack", "card", "add", "-A1"];
        cli(
            &dir,
            &[
                &add[..],
                &["2", "--type", "AI 4x 4-20 mA", "--channels", "4", "--start", "%IW64"],
            ]
            .concat(),
        )
        .unwrap();
        cli(
            &dir,
            &[&add[..], &["1", "--tag", "-A1.DI1", "--start", "%IX0.0"]].concat(),
        )
        .unwrap();
        let err = cli(&dir, &[&add[..], &["1"]].concat()).unwrap_err();
        assert!(err.contains("already has a card in slot 1"), "{err}");
        assert_eq!(
            rack_list(&racks()),
            "-A1 (Main PLC)\n  slot 1: -A1.DI1 16 channels from %IX0.0\n  \
             slot 2: \"AI 4x 4-20 mA\" 4 channels from %IW64\n"
        );

        // The form's range checks: nothing is saved past them.
        for bad in [
            &["1000"][..],
            &["3", "--channels", "257"],
            &["3", "--step", "0"],
            &["3", "--step", "65"],
        ] {
            let err = cli(&dir, &[&add[..], bad].concat()).unwrap_err();
            assert!(err.contains("the racks were not changed"), "{bad:?}: {err}");
        }
        assert_eq!(racks()[0].cards.len(), 2);

        let set = ["rack", "card", "set", "-A1"];
        cli(
            &dir,
            &[&set[..], &["2", "--slot", "3", "--step", "1", "--tag", "-A1.AI1"]].concat(),
        )
        .unwrap();
        let card = racks()[0].cards.iter().find(|c| c.slot == 3).cloned().unwrap();
        assert_eq!((card.step, card.tag.as_str(), card.channels), (Some(1), "-A1.AI1", 4));
        cli(&dir, &[&set[..], &["3", "--no-step"]].concat()).unwrap();
        assert_eq!(racks()[0].cards.iter().find(|c| c.slot == 3).unwrap().step, None);
        let err = cli(&dir, &[&set[..], &["3", "--slot", "1"]].concat()).unwrap_err();
        assert!(err.contains("already has a card in slot 1"), "{err}");
        assert!(cli(&dir, &[&set[..], &["9", "--channels", "8"]].concat()).is_err());

        cli(&dir, &["rack", "set", "-A1", "--name", "-A2", "--description", ""]).unwrap();
        assert_eq!((racks()[0].name.as_str(), racks()[0].description.as_str()), ("-A2", ""));
        cli(&dir, &["rack", "card", "remove", "-A2", "1"]).unwrap();
        assert_eq!(racks()[0].cards.len(), 1);
        let err = cli(&dir, &["rack", "remove", "-A1"]).unwrap_err();
        assert!(err.contains("there is no rack -A1"), "{err}");
        cli(&dir, &["rack", "remove", "-A2"]).unwrap();
        assert!(racks().is_empty());
        cli(&dir, &["rack", "list"]).unwrap();
    }

    #[test]
    fn the_project_option_is_kept_apart_from_dash_tags() {
        let parsed = |args: &[&str]| -> (PathBuf, Option<String>) {
            let Ok(c) = Cli::try_parse_from(args) else {
                unreachable!("{args:?}")
            };
            let Cmd::Strip(StripCmd::List { strip }) = c.cmd else {
                unreachable!()
            };
            (c.project, strip)
        };
        let other = || PathBuf::from("other");
        let x1 = Some("-X1".to_string());
        // Separated, long or before the subcommand: the project, the tag stays a tag.
        assert_eq!(
            parsed(&["wiringcraft", "strip", "list", "-C", "other", "-X1"]),
            (other(), x1.clone())
        );
        assert_eq!(
            parsed(&["wiringcraft", "strip", "list", "-X1", "-C", "other"]),
            (other(), x1.clone())
        );
        assert_eq!(
            parsed(&["wiringcraft", "strip", "list", "--project", "other", "-X1"]),
            (other(), x1.clone())
        );
        assert_eq!(
            parsed(&["wiringcraft", "strip", "list", "--project=other", "-X1"]),
            (other(), x1.clone())
        );
        assert_eq!(
            parsed(&["wiringcraft", "-Cother", "strip", "list", "-X1"]),
            (other(), x1.clone())
        );
        assert_eq!(parsed(&["wiringcraft", "-Cother", "strip", "list"]), (other(), None));
        // Attached after the subcommand it is a tag: `-C1` is a capacitor strip as much as a path.
        assert_eq!(
            parsed(&["wiringcraft", "strip", "list", "-C1"]),
            (PathBuf::from("."), Some("-C1".into()))
        );
        let Ok(c) = Cli::try_parse_from(["wiringcraft", "device", "-C", "other", "-C1"]) else {
            unreachable!()
        };
        let Cmd::Device { tag, .. } = c.cmd else { unreachable!() };
        assert_eq!((c.project, tag.as_str()), (other(), "-C1"));
    }

    #[test]
    fn cable_device_and_strip_tags_may_start_with_a_dash_without_a_double_dash() {
        let Ok(tmp) = tempfile::tempdir() else { unreachable!() };
        let dir = tmp.path().join("p");
        let Some(path) = dir.to_str() else { unreachable!() };
        let Ok(cmd) = Cli::try_parse_from(["wiringcraft", "new", path, "--no-git"]) else {
            unreachable!()
        };
        assert!(run(cmd).is_ok());
        assert!(cli(&dir, &["page", "add", "p1", "--number", "1"]).is_ok());
        let tag = |t: &str| DeviceTag::parse(t).ok();

        // A cable tag right after `--cable`, before or after other options.
        let wired = cli(&dir, &["wire", "p1", "1,2", "30,2", "--cable", "-W2", "--core", "1"]);
        assert!(wired.is_ok(), "{wired:?}");
        let wired = cli(&dir, &["wire", "p1", "1,5", "30,5", "--core", "2", "--cable", "-W2"]);
        assert!(wired.is_ok(), "{wired:?}");
        let Ok(p) = load(&dir) else { unreachable!() };
        let Some(pg) = p.pages.iter().find(|pg| pg.id == "p1") else {
            unreachable!()
        };
        let cores: Vec<_> = pg
            .wires
            .iter()
            .filter_map(|w| w.cable.as_ref())
            .map(|c| (Some(c.cable.clone()), c.core.clone()))
            .collect();
        assert_eq!(cores, [(tag("-W2"), "1".to_string()), (tag("-W2"), "2".to_string())]);

        // A device tag as typed, with its options on either side; the options stay options.
        assert!(cli(&dir, &["device", "-K1", "--description", "one"]).is_ok());
        assert!(cli(&dir, &["device", "--description", "two", "-K2"]).is_ok());
        assert!(cli(&dir, &["device", "--description", "three"]).is_err(), "no tag");
        let Ok(p) = load(&dir) else { unreachable!() };
        let described = |t: &str| tag(t).and_then(|t| p.devices.get(&t)).map(|d| d.description.clone());
        assert_eq!(described("-K1").as_deref(), Some("one"));
        assert_eq!(described("-K2").as_deref(), Some("two"));

        // A strip tag in the strip and jumper commands; `--after` stays an option.
        assert!(cli(&dir, &["strip", "spare", "-X1", "S1"]).is_ok());
        assert!(cli(&dir, &["strip", "spare", "-X1", "S3", "--after", "S1"]).is_ok());
        assert!(cli(&dir, &["strip", "spare", "--after", "S1", "-X1", "S2"]).is_ok());
        assert!(cli(&dir, &["strip", "move", "-X1", "S3", "1"]).is_ok());
        assert!(cli(&dir, &["strip", "list", "-X1"]).is_ok());
        let Ok(p) = load(&dir) else { unreachable!() };
        let (symbols, _) = store::vendored_symbols(&dir, &p);
        let strips = wc_core::project::strip_terminals(&p, |id| symbols.get(id).map(|s| s.role));
        let Some(x1) = tag("-X1") else { unreachable!() };
        let rail: Vec<_> = strips.get(&x1).into_iter().flatten().map(|(t, _)| t.as_str()).collect();
        assert_eq!(rail, ["S3", "S1", "S2"]);
        let rail = || -> Vec<String> {
            let Ok(p) = load(&dir) else { unreachable!() };
            let (symbols, _) = store::vendored_symbols(&dir, &p);
            let mut strips = wc_core::project::strip_terminals(&p, |id| symbols.get(id).map(|s| s.role));
            strips
                .remove(&x1)
                .unwrap_or_default()
                .into_iter()
                .map(|(t, _)| t)
                .collect()
        };
        assert!(cli(&dir, &["strip", "order", "-X1", "S2", "S1", "S3"]).is_ok());
        assert_eq!(rail(), ["S2", "S1", "S3"]);
        assert!(cli(&dir, &["strip", "unspare", "-X1", "S3"]).is_ok());
        assert_eq!(rail(), ["S2", "S1"]);
        assert!(cli(&dir, &["strip", "level", "-X1", "S1", "2"]).is_ok());
        let level = || {
            let Ok(p) = load(&dir) else { unreachable!() };
            tag("-X1.S1")
                .and_then(|t| p.devices.get(&t))
                .and_then(|d| d.properties.get(wc_report::LEVEL_PROPERTY).cloned())
        };
        assert_eq!(level().as_deref(), Some("2"));
        assert!(cli(&dir, &["strip", "level", "-X1", "S1"]).is_ok());
        assert_eq!(level(), None);

        // Jumpers: a jumper left with one terminal is dropped, so this one keeps two. `jumper add`
        // takes only terminals the strip has (#572), so S4 is added as a spare first.
        assert!(cli(&dir, &["strip", "spare", "-X1", "S4"]).is_ok());
        assert!(cli(&dir, &["jumper", "add", "-X1", "S1", "S2", "S4"]).is_ok());
        assert!(cli(&dir, &["jumper", "remove", "-X1", "S2"]).is_ok());
        let Ok(p) = load(&dir) else { unreachable!() };
        let Some(j) = p.meta.jumpers.first() else {
            unreachable!()
        };
        let kept = ["S1", "S4"].map(String::from);
        assert_eq!((&j.strip, j.terminals.as_slice()), (&x1, kept.as_slice()));
        let Ok(mut p) = load(&dir) else { unreachable!() };
        p.meta.libraries.push(LibrarySource {
            name: "starter".into(),
            kind: "folder".into(),
            location: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../library")
                .to_string_lossy()
                .into_owned(),
            reference: None,
        });
        assert!(save(&dir, &p).is_ok());
        let parted = cli(&dir, &["jumper", "part", "-X1", "S4", "example.ex-tb-2-5"]);
        assert!(parted.is_ok(), "{parted:?}");
        let Ok(p) = load(&dir) else { unreachable!() };
        let part = p.meta.jumpers.first().and_then(|j| j.part.as_deref());
        assert_eq!(part, Some("example.ex-tb-2-5"));
        assert!(cli(&dir, &["jumper", "part", "-X1", "S1"]).is_ok());
        let Ok(p) = load(&dir) else { unreachable!() };
        assert_eq!(p.meta.jumpers.first().and_then(|j| j.part.clone()), None);
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
        cli(&dir, &["strip", "list", "--", "-X1"]).unwrap();
        let err = cli(&dir, &["strip", "list", "--", "-X9"]).unwrap_err();
        assert!(err.starts_with("no terminal strip -X9"), "{err}");
        // A terminal whose symbol cannot be read does not count; the error says why.
        let mut p = load(&dir).unwrap();
        p.pages[0].symbols.push(PlacedSymbol {
            id: "t9".into(),
            symbol: "no.such.terminal".into(),
            at: Point::new(90.0, 10.0),
            rotation: Rotation::default(),
            mirror: false,
            // Tagged with its strip only: without the symbol's role it is not known as a terminal.
            tag: DeviceTag::parse("-X9").ok(),
        });
        save(&dir, &p).unwrap();
        let err = cli(&dir, &["strip", "list", "--", "-X9"]).unwrap_err();
        assert!(
            err.contains("no readable vendored copy") && err.contains("no.such.terminal"),
            "{err}"
        );
        let err = cli(&dir, &["jumper", "add", "--", "-X9", "1", "2"]).unwrap_err();
        assert!(err.contains("no.such.terminal"), "{err}");
        let mut p = load(&dir).unwrap();
        p.pages[0].symbols.retain(|s| s.id != "t9");
        save(&dir, &p).unwrap();
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
        draw_x1_terminals(&dir);
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
    fn all_reports_as_csv_go_to_one_file_each() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let out = tmp.path().join("reports");
        cli(
            &dir,
            &["report", "all", "--format", "csv", "--out", out.to_str().unwrap()],
        )
        .unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|f| f.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut want: Vec<String> = wc_report::Kind::ALL
            .iter()
            .map(|k| format!("{}.csv", k.name()))
            .collect();
        want.sort();
        assert_eq!(names, want);
        // Each file is that one report's table alone, the same as asking for it by name.
        let one = tmp.path().join("wires.csv");
        cli(
            &dir,
            &["report", "wires", "--format", "csv", "--out", one.to_str().unwrap()],
        )
        .unwrap();
        let wires = std::fs::read_to_string(out.join("wires.csv")).unwrap();
        assert_eq!(format!("{wires}\n"), std::fs::read_to_string(&one).unwrap());
    }

    #[test]
    fn report_csv_for_excel_is_semicolon_separated_with_a_bom() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        // One report to a file: `;` between fields and a byte order mark first.
        let one = tmp.path().join("wires.csv");
        cli(
            &dir,
            &[
                "report",
                "wires",
                "--format",
                "csv",
                "--sep",
                ";",
                "--out",
                one.to_str().unwrap(),
            ],
        )
        .unwrap();
        let semi = std::fs::read_to_string(&one).unwrap();
        let plain = tmp.path().join("plain.csv");
        cli(
            &dir,
            &["report", "wires", "--format", "csv", "--out", plain.to_str().unwrap()],
        )
        .unwrap();
        let plain = std::fs::read_to_string(&plain).unwrap();
        // The default stays comma-separated without a byte order mark (a new project's wires
        // report is its header row alone, with no comma inside a field).
        assert!(!plain.starts_with('\u{feff}'), "{plain}");
        let first = plain.lines().next().unwrap();
        assert!(first.contains(','), "{first}");
        assert_eq!(semi, format!("\u{feff}{}", plain.replace(',', ";")));
        // `report all` writes every file of the folder the same way.
        let out = tmp.path().join("reports");
        cli(
            &dir,
            &[
                "report",
                "all",
                "--format",
                "csv",
                "--sep",
                ";",
                "--out",
                out.to_str().unwrap(),
            ],
        )
        .unwrap();
        for k in wc_report::Kind::ALL {
            let csv = std::fs::read_to_string(out.join(format!("{}.csv", k.name()))).unwrap();
            assert!(csv.starts_with('\u{feff}'), "{}: {csv}", k.name());
        }
        let wires = std::fs::read_to_string(out.join("wires.csv")).unwrap();
        assert_eq!(format!("{wires}\n"), semi);
        // Only `,` and `;`, and only for CSV.
        assert!(cli(&dir, &["report", "wires", "--format", "csv", "--sep", "|"]).is_err());
        assert!(cli(&dir, &["report", "wires", "--format", "md", "--sep", ";"]).is_err());
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
    fn a_black_box_is_resized_and_its_points_edited() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["page", "add", "p1", "--number", "1"]).unwrap();
        let pins = ["--pin", "IN:left:5", "--pin", "OUT:right:5", "--pin", "PE:bottom:18"];
        let mut args = vec!["box", "p1", "--at", "40,40", "--size", "20,10"];
        args.extend_from_slice(&pins);
        cli(&dir, &args).unwrap();
        for pin in ["IN", "OUT", "PE"] {
            cli(&dir, &["wire", "p1", &format!("s1:{pin}"), "0,0"]).unwrap();
        }
        let layout = || {
            let p = load(&dir).unwrap();
            let sym = store::read_vendored_symbol(&dir, "box.p1.s1").unwrap();
            assert_eq!(
                p.lock.symbols["box.p1.s1"].hash,
                sym.hash(),
                "the lock pins the new version"
            );
            wc_core::black_box_layout(&sym).unwrap()
        };

        // Narrower: the points stay on their edges, PE pulled in to the new width.
        cli(&dir, &["box-edit", "p1", "s1", "--size", "10,10"]).unwrap();
        let (w, h, now) = layout();
        assert_eq!((w, h), (10.0, 10.0));
        assert_eq!(now.iter().find(|b| b.name == "PE").unwrap().offset, 10.0);

        // A new list: OUT renamed to Q (its wire follows), PE taken off (its wire keeps a free
        // end where the point was), A added.
        cli(
            &dir,
            &[
                "box-edit",
                "p1",
                "s1",
                "--size",
                "10,20",
                "--rename",
                "OUT=Q",
                "--pin",
                "IN:left:5",
                "--pin",
                "Q:right:5",
                "--pin",
                "A:top:2",
            ],
        )
        .unwrap();
        let names: Vec<String> = layout().2.into_iter().map(|b| b.name).collect();
        assert_eq!(names, ["IN", "Q", "A"]);
        let p = load(&dir).unwrap();
        let ends: Vec<Endpoint> = p.page("p1").unwrap().wires.iter().map(|w| w.from.clone()).collect();
        let pin = |n: &str| Endpoint::Pin {
            item: "s1".into(),
            pin: n.into(),
        };
        assert_eq!(
            ends,
            [
                pin("IN"),
                pin("Q"),
                Endpoint::Point {
                    at: Point::new(50.0, 50.0)
                }
            ]
        );

        // Refused, and nothing changes: a rename of a point the box does not have, a point off
        // its edge, a box without area and an item that is not a box.
        let before = layout();
        assert!(cli(&dir, &["box-edit", "p1", "s1", "--rename", "X=Y"]).is_err());
        assert!(cli(&dir, &["box-edit", "p1", "s1", "--pin", "IN:top:99"]).is_err());
        assert!(cli(&dir, &["box-edit", "p1", "s1", "--size", "0,5"]).is_err());
        assert!(cli(&dir, &["box-edit", "p1", "s1", "--size=-1,10"]).is_err());
        assert!(cli(&dir, &["box-edit", "p1", "s1", "--size=10,-1"]).is_err());
        assert!(cli(&dir, &["box-edit", "p1", "s1", "--rename", "IN=Z", "--rename", "Q=Z"]).is_err());
        assert!(cli(&dir, &["box-edit", "p1", "w1", "--size", "5,5"]).is_err());
        assert_eq!(layout(), before);
        assert_eq!(load(&dir).unwrap().page("p1").unwrap().wires.len(), 3);
        // A point kept under its own name must be in the new list too: else its wire would be
        // cut loose. Refused, and not a byte of the project changes.
        cli(&dir, &["wire", "p1", "s1:A", "0,0"]).unwrap();
        let files = snapshot_project(&dir, &[], &[]).unwrap();
        let args = [
            "box-edit",
            "p1",
            "s1",
            "--rename",
            "A=A",
            "--pin",
            "IN:left:5",
            "--pin",
            "Q:right:5",
        ];
        assert!(cli(&dir, &args).is_err());
        for (path, was) in &files {
            assert_eq!(std::fs::read(path).ok(), *was, "{} unchanged", path.display());
        }
        assert_eq!(layout(), before);
        // A swap of two names is fine: each wire follows its point.
        cli(&dir, &["box-edit", "p1", "s1", "--rename", "IN=Q", "--rename", "Q=IN"]).unwrap();
        let names: Vec<String> = layout().2.into_iter().map(|b| b.name).collect();
        assert_eq!(names, ["Q", "IN", "A"]);
        let p = load(&dir).unwrap();
        let ends: Vec<Endpoint> = p.page("p1").unwrap().wires.iter().map(|w| w.from.clone()).collect();
        assert_eq!(ends[..2], [pin("Q"), pin("IN")]);

        // An edit that changes nothing writes nothing: a page file laid out by hand keeps its bytes.
        let page_file = dir.join(store::PAGES_DIR).join(format!("p1{}", store::PAGE_SUFFIX));
        let by_hand = format!("{}\n\n", std::fs::read_to_string(&page_file).unwrap());
        std::fs::write(&page_file, &by_hand).unwrap();
        let files = snapshot_project(&dir, &[], &[]).unwrap();
        cli(&dir, &["box-edit", "p1", "s1"]).unwrap();
        cli(&dir, &["box-edit", "p1", "s1", "--rename", "A=A"]).unwrap();
        for (path, was) in &files {
            assert_eq!(std::fs::read(path).ok(), *was, "{} unchanged", path.display());
        }
        assert_eq!(std::fs::read_to_string(&page_file).unwrap(), by_hand);

        // One point renamed twice is refused; `--clear-pins` cannot go with `--pin`.
        assert!(cli(&dir, &["box-edit", "p1", "s1", "--rename", "Q=Z", "--rename", "Q=Y"]).is_err());
        assert!(cli(&dir, &["box-edit", "p1", "s1", "--clear-pins", "--pin", "Q:left:5"]).is_err());
        assert_eq!(layout().2.len(), 3);
        // Every point taken off: the wires keep free ends where the points were.
        cli(&dir, &["box-edit", "p1", "s1", "--clear-pins"]).unwrap();
        assert!(layout().2.is_empty());
        let p = load(&dir).unwrap();
        let ends: Vec<Endpoint> = p.page("p1").unwrap().wires.iter().map(|w| w.from.clone()).collect();
        let at = |x, y| Endpoint::Point { at: Point::new(x, y) };
        assert_eq!(ends[..2], [at(40.0, 45.0), at(50.0, 45.0)]);
    }

    #[test]
    fn place_takes_a_tag_that_starts_with_a_dash_as_box_does() {
        let Ok(tmp) = tempfile::tempdir() else { unreachable!() };
        let dir = tmp.path().join("proj");
        let Some(path) = dir.to_str() else { unreachable!() };
        let Ok(cmd) = Cli::try_parse_from(["wiringcraft", "new", path, "--no-git"]) else {
            unreachable!()
        };
        assert!(run(cmd).is_ok());
        let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
        let Some(starter) = starter.to_str() else {
            unreachable!()
        };
        assert!(cli(&dir, &["lib", "add", "starter", "folder", starter]).is_ok());
        assert!(cli(&dir, &["page", "add", "p1", "--number", "1"]).is_ok());

        let placed = cli(&dir, &["place", "p1", "wc.coil", "--at", "50,50", "--tag", "-K1"]);
        assert!(placed.is_ok(), "{placed:?}");
        let Ok(p) = load(&dir) else { unreachable!() };
        let Some(pg) = p.pages.iter().find(|pg| pg.id == "p1") else {
            unreachable!()
        };
        let Some(s) = pg.symbols.first() else { unreachable!() };
        assert_eq!(s.tag.as_ref().map(ToString::to_string).as_deref(), Some("-K1"));
    }

    #[test]
    fn place_checks_the_page_turn_and_tag_before_taking_the_symbol() {
        let Ok(tmp) = tempfile::tempdir() else { unreachable!() };
        let dir = tmp.path().join("proj");
        let Some(path) = dir.to_str() else { unreachable!() };
        let Ok(cmd) = Cli::try_parse_from(["wiringcraft", "new", path, "--no-git"]) else {
            unreachable!()
        };
        assert!(run(cmd).is_ok());
        let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
        let Some(starter) = starter.to_str() else {
            unreachable!()
        };
        assert!(cli(&dir, &["lib", "add", "starter", "folder", starter]).is_ok());
        assert!(cli(&dir, &["page", "add", "p1", "--number", "1"]).is_ok());

        let missing_page = ["place", "nope", "wc.coil", "--at", "50,50"];
        let bad_turn = ["place", "p1", "wc.coil", "--at", "50,50", "--rotate", "45"];
        let bad_tag = ["place", "p1", "wc.coil", "--at", "50,50", "--tag", "-K1."];
        for bad in [&missing_page[..], &bad_turn[..], &bad_tag[..]] {
            let args = bad.to_vec();
            assert!(cli(&dir, &args).is_err(), "{bad:?}");
            assert!(
                store::read_vendored_symbol(&dir, "wc.coil").is_err(),
                "no copy left by {bad:?}"
            );
            let Ok(p) = load(&dir) else { unreachable!() };
            assert!(!p.lock.symbols.contains_key("wc.coil"), "no lock entry from {bad:?}");
        }
    }

    #[test]
    fn box_edit_renames_a_name_holding_an_equals_sign_and_refuses_links_and_other_symbols() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
        cli(&dir, &["lib", "add", "starter", "folder", starter.to_str().unwrap()]).unwrap();
        cli(&dir, &["page", "add", "p1", "--number", "1"]).unwrap();
        cli(
            &dir,
            &["box", "p1", "--at", "40,40", "--size", "20,10", "--pin", "X=1:left:5"],
        )
        .unwrap();
        cli(&dir, &["wire", "p1", "s1:X=1", "0,0"]).unwrap();

        // `--rename X=1=Q` splits at the first `=`; `--rename-pair` takes the names apart.
        assert!(cli(&dir, &["box-edit", "p1", "s1", "--rename", "X=1=Q"]).is_err());
        cli(&dir, &["box-edit", "p1", "s1", "--rename-pair", "X=1", "Q"]).unwrap();
        let p = load(&dir).unwrap();
        let wire = &p.page("p1").unwrap().wires[0];
        assert_eq!(
            wire.from,
            Endpoint::Pin {
                item: "s1".into(),
                pin: "Q".into()
            }
        );

        // A placed symbol that is not a box is refused as one.
        cli(&dir, &["place", "p1", "wc.coil", "--at", "100,100"]).unwrap();
        let coil = load(&dir).unwrap().page("p1").unwrap().symbols[1].id.clone();
        let err = cli(&dir, &["box-edit", "p1", &coil, "--size", "5,5"]).unwrap_err();
        assert!(err.contains("is not a black box"), "{err}");

        // A symbol file that is a link is not replaced: the link and the project stay.
        #[cfg(unix)]
        {
            let path = store::vendor_path(&dir, ItemKind::Symbol, "box.p1.s1").unwrap();
            let target = tmp.path().join("elsewhere.toml");
            std::fs::rename(&path, &target).unwrap();
            std::os::unix::fs::symlink(&target, &path).unwrap();
            let before = std::fs::read(&target).unwrap();
            let err = cli(&dir, &["box-edit", "p1", "s1", "--size", "30,10"]).unwrap_err();
            assert!(err.contains("link"), "{err}");
            assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read_link(&path).unwrap(), target);
            assert_eq!(std::fs::read(&target).unwrap(), before);
        }
    }

    /// A missing file is not a link; a path whose metadata cannot be read for another reason is
    /// an error naming it, not "not a link".
    #[cfg(unix)]
    #[test]
    fn is_link_tells_missing_from_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("plain.json");
        std::fs::write(&file, "{}").unwrap();
        let link = tmp.path().join("link.json");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert_eq!(is_link(&file), Ok(false));
        assert_eq!(is_link(&link), Ok(true));
        assert_eq!(is_link(&tmp.path().join("missing.json")), Ok(false));
        // A plain file used as a folder: not a "not found" error.
        let err = is_link(&file.join("inside.json")).unwrap_err();
        assert!(err.contains("inside.json"), "{err}");
    }

    /// A page file that is a link is refused before anything is written, so neither the save
    /// nor a rollback replaces the link with a plain file (#614).
    #[cfg(unix)]
    #[test]
    fn box_edit_refuses_a_linked_page_file_before_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["page", "add", "p1", "--number", "1"]).unwrap();
        cli(
            &dir,
            &["box", "p1", "--at", "40,40", "--size", "20,10", "--pin", "1:left:5"],
        )
        .unwrap();
        let page = dir.join(store::PAGES_DIR).join(format!("p1{}", store::PAGE_SUFFIX));
        let target = tmp.path().join("p1-elsewhere.json");
        std::fs::rename(&page, &target).unwrap();
        std::os::unix::fs::symlink(&target, &page).unwrap();
        let page_before = std::fs::read(&target).unwrap();
        let symbol_before = store::read_vendored_symbol(&dir, "box.p1.s1").unwrap();
        let lock_before = std::fs::read(dir.join(store::LOCK_FILE)).unwrap();

        let err = cli(&dir, &["box-edit", "p1", "s1", "--size", "30,10"]).unwrap_err();
        assert!(err.contains("link"), "{err}");
        assert!(err.contains(&format!("p1{}", store::PAGE_SUFFIX)), "{err}");
        assert!(std::fs::symlink_metadata(&page).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&page).unwrap(), target);
        assert_eq!(std::fs::read(&target).unwrap(), page_before);
        assert_eq!(store::read_vendored_symbol(&dir, "box.p1.s1").unwrap(), symbol_before);
        assert_eq!(std::fs::read(dir.join(store::LOCK_FILE)).unwrap(), lock_before);

        // With the link replaced by its target's bytes the same edit goes through.
        std::fs::remove_file(&page).unwrap();
        std::fs::copy(&target, &page).unwrap();
        cli(&dir, &["box-edit", "p1", "s1", "--size", "30,10"]).unwrap();
        assert_ne!(store::read_vendored_symbol(&dir, "box.p1.s1").unwrap(), symbol_before);
    }

    /// A box-edit project: a box `s1` on page p1 with point 1 wired. Returns the project folder,
    /// the box's symbol file, and the files a box-edit may change with their bytes now.
    fn box_edit_project(tmp: &Path) -> (PathBuf, PathBuf, run::Written) {
        let dir = tmp.join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["page", "add", "p1", "--number", "1"]).unwrap();
        cli(
            &dir,
            &["box", "p1", "--at", "40,40", "--size", "20,10", "--pin", "1:left:5"],
        )
        .unwrap();
        cli(&dir, &["wire", "p1", "s1:1", "0,0"]).unwrap();
        let (symbol, symbol_bytes) = vendored_bytes(&dir, ItemKind::Symbol, "box.p1.s1").unwrap();
        let mut files = snapshot_project(&dir, &[], &[]).unwrap();
        files.push((symbol.clone(), symbol_bytes));
        (dir, symbol, files)
    }

    fn bytes_now(files: &run::Written) -> run::Written {
        files
            .iter()
            .map(|(path, _)| (path.clone(), std::fs::read(path).ok()))
            .collect()
    }

    /// A box-edit run that stops between its writes (here: the symbol and `library.lock` are
    /// new, the page is not) leaves its journal, and the next run puts every file back as it
    /// was before, so symbol, lock and page agree again (#653).
    #[test]
    fn box_edit_stopped_between_writes_is_put_back_by_the_next_run() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, symbol_path, before) = box_edit_project(tmp.path());

        // A page file the run adds is in its journal as not there before.
        let extra = dir.join(store::PAGES_DIR).join(format!("zz{}", store::PAGE_SUFFIX));
        let mut journaled = before.clone();
        journaled.push((extra.clone(), None));
        let journal = Journal::begin(&dir, &journaled).unwrap();
        let mut sym = store::read_vendored_symbol(&dir, "box.p1.s1").unwrap();
        sym.pins.clear();
        store::write_vendored_symbol(&dir, &sym).unwrap();
        let mut p = load(&dir).unwrap();
        p.lock.symbols.get_mut("box.p1.s1").unwrap().hash = sym.hash();
        store::save_lock(&dir, &p).unwrap();
        std::fs::write(&extra, b"{}").unwrap();
        // The run stops here: the journal stays, no longer locked.
        drop(journal);
        assert_ne!(bytes_now(&before), before);

        // Any next run puts the files back first.
        cli(&dir, &["location", "list"]).unwrap();
        assert_eq!(bytes_now(&before), before);
        assert!(!extra.exists());
        assert!(!dir.join(BOX_EDIT_JOURNAL).exists());
        assert!(std::fs::read(&symbol_path).is_ok());

        // And the edit then goes through, leaving no journal.
        cli(&dir, &["box-edit", "p1", "s1", "--size", "30,10"]).unwrap();
        assert!(!dir.join(BOX_EDIT_JOURNAL).exists());
        assert_ne!(
            std::fs::read(&symbol_path).unwrap(),
            before.last().unwrap().1.clone().unwrap()
        );
    }

    /// The journal of a run still writing is locked: another run leaves it and the files alone,
    /// and a second box-edit refuses instead of writing over it.
    #[test]
    fn a_journal_another_run_holds_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _, before) = box_edit_project(tmp.path());
        let journal = Journal::begin(&dir, &before).unwrap();
        // What the other run has written so far stays as it is.
        let page = dir.join(store::PAGES_DIR).join(format!("p1{}", store::PAGE_SUFFIX));
        let page_before = std::fs::read(&page).unwrap();
        std::fs::write(&page, b"half written by the other run").unwrap();
        assert_eq!(undo_stopped_box_edit(&dir), Ok(None));
        assert_eq!(std::fs::read(&page).unwrap(), b"half written by the other run");
        std::fs::write(&page, &page_before).unwrap();

        let err = cli(&dir, &["box-edit", "p1", "s1", "--size", "30,10"]).unwrap_err();
        assert!(err.contains("has not finished"), "{err}");
        assert_eq!(bytes_now(&before), before);

        journal.finish(&dir).unwrap();
        assert!(!dir.join(BOX_EDIT_JOURNAL).exists());
        assert_eq!(undo_stopped_box_edit(&dir), Ok(None));
    }

    /// A journal left behind that is replaced, between recovery opening it and locking it, by
    /// the journal of a run now writing: the old one, locked but no longer at the path, is not
    /// replayed, and the new one is left alone.
    #[cfg(unix)]
    #[test]
    fn a_journal_swapped_between_open_and_lock_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _, before) = box_edit_project(tmp.path());
        let lock = dir.join(store::LOCK_FILE);
        // The stopped run's journal would put `library.lock` back to other bytes.
        let mut stale = before.clone();
        for (path, bytes) in &mut stale {
            if *path == lock {
                *bytes = Some(b"stale".to_vec());
            }
        }
        drop(Journal::begin(&dir, &stale).unwrap());
        let live: std::rc::Rc<std::cell::RefCell<Option<Journal>>> = std::rc::Rc::default();
        let (into, journal_dir, now) = (live.clone(), dir.clone(), before.clone());
        BETWEEN_JOURNAL_OPEN_AND_LOCK.set(Some(Box::new(move || {
            std::fs::remove_file(journal_dir.join(BOX_EDIT_JOURNAL)).unwrap();
            *into.borrow_mut() = Some(Journal::begin(&journal_dir, &now).unwrap());
        })));

        assert_eq!(undo_stopped_box_edit(&dir), Ok(None));
        assert_eq!(bytes_now(&before), before);
        assert!(dir.join(BOX_EDIT_JOURNAL).exists());
        live.take().unwrap().finish(&dir).unwrap();
        assert!(!dir.join(BOX_EDIT_JOURNAL).exists());
    }

    /// A journal that is not whole, or names a file box-edit does not change, is refused, and
    /// no file is changed from it.
    #[test]
    fn a_broken_journal_is_refused_and_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _, before) = box_edit_project(tmp.path());
        let journal = dir.join(BOX_EDIT_JOURNAL);
        let with = |records: &[u8]| {
            let mut b = JOURNAL_HEADER.to_vec();
            b.extend_from_slice(records);
            b
        };
        // A whole record, its lengths counted from the name and bytes, and the end line.
        let one = |name: &str, tail: &[u8]| {
            let mut b = JOURNAL_HEADER.to_vec();
            b.extend_from_slice(format!("{} -\n{name}", name.len()).as_bytes());
            b.extend_from_slice(tail);
            b
        };
        let bad = [
            b"not a journal".to_vec(),
            // Cut inside a record, and cut at a record's end (no end line).
            with(b"12 5\nlibrary.lock"),
            one("library.lock", b""),
            // A count that does not match, and bytes after the end.
            one("library.lock", b"end 2\n"),
            one("library.lock", b"end 1\nmore"),
            // Outside the project, or a file box-edit does not change.
            one("../x.json", b"end 1\n"),
            one("/x.json", b"end 1\n"),
            one("notes.txt", b"end 1\n"),
            one("pages/../x.page.json", b"end 1\n"),
            one("pages/./x.page.json", b"end 1\n"),
        ];
        // The same record with a name box-edit changes is whole: only the name is refused.
        assert!(parse_journal(&dir, &one("pages/x.page.json", b"end 1\n")).is_some());
        for bad in bad {
            std::fs::write(&journal, &bad).unwrap();
            let err = cli(&dir, &["location", "list"]).unwrap_err();
            assert!(err.contains("cannot be read"), "{err}");
            assert_eq!(bytes_now(&before), before);
            assert_eq!(std::fs::read(&journal).unwrap(), bad);
        }
        assert!(!tmp.path().join("x.json").exists());

        // Each file and its bytes come back as written.
        let good = with(b"12 2\nlibrary.lockhi18 -\npages/p2.page.jsonend 2\n");
        assert_eq!(
            parse_journal(&dir, &good),
            Some(vec![
                (dir.join(store::LOCK_FILE), Some(b"hi".to_vec())),
                (dir.join("pages").join("p2.page.json"), None)
            ])
        );
    }

    /// A box-edit run stopped right after its symbol write, in the command's own save path,
    /// leaves a journal holding the symbol and the project files; the next run puts the symbol
    /// back so it matches `library.lock` and the pages again. A `new` run elsewhere leaves the
    /// journal alone.
    #[test]
    fn box_edit_killed_after_its_symbol_write_is_put_back_by_the_next_run() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, symbol_path, before) = box_edit_project(tmp.path());
        STOP_BOX_EDIT_AFTER_SYMBOL.set(true);
        let stopped = cli(&dir, &["box-edit", "p1", "s1", "--size", "30,10", "--clear-pins"]);
        STOP_BOX_EDIT_AFTER_SYMBOL.set(false);
        assert_eq!(stopped, Err(STOPPED.to_owned()));
        // Permissions given to the half-edited symbol file are the ones it keeps when put back.
        #[cfg(unix)]
        let symbol_mode = {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&symbol_path, std::fs::Permissions::from_mode(0o640)).unwrap();
            0o640
        };
        // The journal holds the project files and the symbol, each with its bytes before.
        let left = std::fs::read(dir.join(BOX_EDIT_JOURNAL)).unwrap();
        // Compared by each file's name in the project: the black box symbol files the save may
        // remove come with their folder resolved (on Windows a `\\?\` path, long names).
        let named = |files: &run::Written| {
            files
                .iter()
                .map(|(path, bytes)| (journal_name(&dir, path), bytes.clone()))
                .collect::<Vec<_>>()
        };
        let Some(left) = parse_journal(&dir, &left) else {
            unreachable!("the journal left behind reads")
        };
        assert_eq!(named(&left), named(&before));
        assert!(named(&left).iter().all(|(name, _)| name.is_some()));
        assert_ne!(std::fs::read(&symbol_path).ok(), before.last().unwrap().1);

        let other = tmp.path().join("other");
        cli(&dir, &["new", other.to_str().unwrap(), "--no-git"]).unwrap();
        assert!(dir.join(BOX_EDIT_JOURNAL).exists());

        cli(&dir, &["location", "list"]).unwrap();
        assert!(!dir.join(BOX_EDIT_JOURNAL).exists());
        assert_eq!(bytes_now(&before), before);
        // The symbol file put back keeps the permissions it had.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&symbol_path), symbol_mode, "{:o}", mode(&symbol_path));
        }
        cli(&dir, &["box-edit", "p1", "s1", "--size", "30,10"]).unwrap();
        assert!(!dir.join(BOX_EDIT_JOURNAL).exists());
    }

    /// On Unix a backslash is part of a file name: `pages/a\b.page.json` is journaled and put
    /// back as that file, and `pages/a/b.page.json` is left as it is.
    #[cfg(unix)]
    #[test]
    fn a_backslash_in_a_journaled_name_stays_part_of_the_name() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _, _) = box_edit_project(tmp.path());
        let pages = dir.join(store::PAGES_DIR);
        let slashed = pages.join("a\\b.page.json");
        let nested = pages.join("a").join("b.page.json");
        std::fs::create_dir(pages.join("a")).unwrap();
        std::fs::write(&slashed, b"old").unwrap();
        std::fs::write(&nested, b"nested").unwrap();
        let journal = Journal::begin(&dir, &[(slashed.clone(), Some(b"old".to_vec()))]).unwrap();
        std::fs::write(&slashed, b"new").unwrap();
        drop(journal);

        assert!(undo_stopped_box_edit(&dir).unwrap().is_some());
        assert_eq!(std::fs::read(&slashed).unwrap(), b"old");
        assert_eq!(std::fs::read(&nested).unwrap(), b"nested");
    }

    /// A comment file named by the suffix alone (an empty list) is one the store reads and a
    /// save removes: box-edit journals it, and a stopped run's journal puts it back.
    #[test]
    fn a_comment_file_named_by_the_suffix_alone_is_journaled_and_put_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _, _) = box_edit_project(tmp.path());
        let comments = dir.join(store::COMMENTS_DIR);
        std::fs::create_dir_all(&comments).unwrap();
        let empty = comments.join(store::COMMENTS_SUFFIX);
        std::fs::write(&empty, b"[]").unwrap();
        let mut before = snapshot_project(&dir, &[], &[]).unwrap();
        assert!(before.iter().any(|(path, _)| *path == empty));
        let (symbol, symbol_bytes) = vendored_bytes(&dir, ItemKind::Symbol, "box.p1.s1").unwrap();
        before.push((symbol, symbol_bytes));

        STOP_BOX_EDIT_AFTER_SYMBOL.set(true);
        let stopped = cli(&dir, &["box-edit", "p1", "s1", "--size", "30,10"]);
        STOP_BOX_EDIT_AFTER_SYMBOL.set(false);
        assert_eq!(stopped, Err(STOPPED.to_owned()));
        std::fs::remove_file(&empty).unwrap();
        cli(&dir, &["location", "list"]).unwrap();
        assert_eq!(bytes_now(&before), before);
        assert_eq!(std::fs::read(&empty).unwrap(), b"[]");
        // Made again where none was: its staging file was readable by its owner only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&empty).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }

        cli(&dir, &["box-edit", "p1", "s1", "--size", "30,10"]).unwrap();
        assert!(!dir.join(BOX_EDIT_JOURNAL).exists());
    }

    /// A command that does not use the `-C` project (`inspect`) leaves a journal there alone,
    /// and `new --template` refuses a template a box-edit run stopped half way in.
    #[test]
    fn commands_that_do_not_use_the_project_leave_its_journal_alone() {
        use std::io::Write as _;
        let tmp = tempfile::tempdir().unwrap();
        let (dir, symbol_path, before) = box_edit_project(tmp.path());
        STOP_BOX_EDIT_AFTER_SYMBOL.set(true);
        let stopped = cli(&dir, &["box-edit", "p1", "s1", "--size", "30,10", "--clear-pins"]);
        STOP_BOX_EDIT_AFTER_SYMBOL.set(false);
        assert_eq!(stopped, Err(STOPPED.to_owned()));
        let journal = std::fs::read(dir.join(BOX_EDIT_JOURNAL)).unwrap();
        let now = bytes_now(&before);

        let archive = tmp.path().join("a.zip");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        z.start_file("readme.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(b"hi").unwrap();
        z.finish().unwrap();
        cli(&dir, &["inspect", archive.to_str().unwrap()]).unwrap();
        assert_eq!(std::fs::read(dir.join(BOX_EDIT_JOURNAL)).unwrap(), journal);
        assert_eq!(bytes_now(&before), now);

        let made = tmp.path().join("made");
        let err = run(Cli::try_parse_from([
            "wiringcraft",
            "new",
            made.to_str().unwrap(),
            "--no-git",
            "--template",
            dir.to_str().unwrap(),
        ])
        .unwrap())
        .unwrap_err();
        assert!(err.contains("stopped half way"), "{err}");
        assert!(!made.exists());
        assert_eq!(bytes_now(&before), now);

        // Once the template is put back it serves again.
        cli(&dir, &["check"]).ok();
        assert!(!dir.join(BOX_EDIT_JOURNAL).exists());
        assert_eq!(bytes_now(&before), before);
        assert!(symbol_path.exists());
        run(Cli::try_parse_from([
            "wiringcraft",
            "new",
            made.to_str().unwrap(),
            "--no-git",
            "--template",
            dir.to_str().unwrap(),
        ])
        .unwrap())
        .unwrap();
    }

    /// The project reached through a linked folder: the box symbol files the save may remove
    /// come resolved, and are journaled by their name in the project.
    #[cfg(unix)]
    #[test]
    fn box_edit_through_a_linked_project_folder_journals_its_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, symbol_path, _) = box_edit_project(tmp.path());
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let symbol_before = std::fs::read(&symbol_path).unwrap();
        cli(&link, &["box-edit", "p1", "s1", "--size", "30,10"]).unwrap();
        assert_ne!(std::fs::read(&symbol_path).unwrap(), symbol_before);
        assert!(!dir.join(BOX_EDIT_JOURNAL).exists());
    }

    /// A journal that is not a plain file (a FIFO, whose reading would never end), or whose
    /// records lead through a linked folder, is refused and nothing is written through it.
    #[cfg(unix)]
    #[test]
    fn a_journal_that_is_no_plain_file_or_leads_through_a_link_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _, before) = box_edit_project(tmp.path());
        let journal = dir.join(BOX_EDIT_JOURNAL);
        let made = std::process::Command::new("mkfifo").arg(&journal).status().unwrap();
        assert!(made.success());
        let err = cli(&dir, &["location", "list"]).unwrap_err();
        assert!(err.contains("not a plain file"), "{err}");
        std::fs::remove_file(&journal).unwrap();

        // `comments` is a link to a folder outside the project.
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join(store::COMMENTS_DIR)).unwrap();
        let mut bytes = JOURNAL_HEADER.to_vec();
        bytes.extend_from_slice(b"25 2\ncomments/p1.comments.jsonhiend 1\n");
        std::fs::write(&journal, &bytes).unwrap();
        let err = cli(&dir, &["location", "list"]).unwrap_err();
        assert!(err.contains("linked folder"), "{err}");
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        assert_eq!(bytes_now(&before), before);
        assert!(journal.exists());
        std::fs::remove_file(dir.join(store::COMMENTS_DIR)).unwrap();

        // A file to put back that is now a link or a FIFO: refused before anything is put back.
        let lock = dir.join(store::LOCK_FILE);
        let page = dir.join(store::PAGES_DIR).join(format!("p1{}", store::PAGE_SUFFIX));
        let journal_of = |name: &str| {
            // The unsafe file first: put back last of all, so a replay begun before every
            // target is checked would already have changed `library.lock`.
            let mut b = JOURNAL_HEADER.to_vec();
            b.extend_from_slice(format!("{} 2\n{name}hi", name.len()).as_bytes());
            b.extend_from_slice(b"12 2\nlibrary.lockhiend 2\n");
            b
        };
        let page_name = format!("{}/p1{}", store::PAGES_DIR, store::PAGE_SUFFIX);
        let page_bytes = std::fs::read(&page).unwrap();
        let target = tmp.path().join("page-elsewhere.json");
        std::fs::write(&target, &page_bytes).unwrap();
        std::fs::remove_file(&page).unwrap();
        std::os::unix::fs::symlink(&target, &page).unwrap();
        std::fs::write(&journal, journal_of(&page_name)).unwrap();
        let err = cli(&dir, &["location", "list"]).unwrap_err();
        assert!(err.contains("not a plain file"), "{err}");
        assert!(std::fs::symlink_metadata(&page).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&target).unwrap(), page_bytes);
        assert_ne!(std::fs::read(&lock).unwrap(), b"hi");

        std::fs::remove_file(&page).unwrap();
        let made = std::process::Command::new("mkfifo").arg(&page).status().unwrap();
        assert!(made.success());
        let err = cli(&dir, &["location", "list"]).unwrap_err();
        assert!(err.contains("not a plain file"), "{err}");
        assert_ne!(std::fs::read(&lock).unwrap(), b"hi");
        assert!(journal.exists());
    }

    /// Folders swapped for links after recovery checked them, before it puts the files back
    /// (#666): nothing is written or removed through them, and the journal stays.
    #[cfg(unix)]
    #[test]
    fn a_folder_swapped_for_a_link_after_the_check_is_not_written_through() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _, _) = box_edit_project(tmp.path());
        let comments = dir.join(store::COMMENTS_DIR);
        let vendor = dir.join(store::VENDOR_DIR);
        std::fs::create_dir_all(&comments).unwrap();
        let kind = ItemKind::Symbol.dir();
        // Outside the project: a comments file to be removed, and a folder for symbols.
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(outside.join(kind)).unwrap();
        std::fs::write(outside.join("a.comments.json"), b"outside").unwrap();
        let records = [
            (format!("{}/a.comments.json", store::COMMENTS_DIR), None),
            (format!("{}/b.comments.json", store::COMMENTS_DIR), Some(&b"hi"[..])),
            (format!("{}/{kind}/x.json", store::VENDOR_DIR), Some(&b"hi"[..])),
        ];
        let mut bytes = JOURNAL_HEADER.to_vec();
        for (name, before) in &records {
            let len = before.map_or_else(|| "-".to_owned(), |b| b.len().to_string());
            bytes.extend_from_slice(format!("{} {len}\n{name}", name.len()).as_bytes());
            bytes.extend_from_slice(before.unwrap_or_default());
        }
        bytes.extend_from_slice(format!("end {}\n", records.len()).as_bytes());
        let journal = dir.join(BOX_EDIT_JOURNAL);
        std::fs::write(&journal, &bytes).unwrap();

        let (swap_comments, swap_vendor, to) = (comments.clone(), vendor.clone(), outside.clone());
        BETWEEN_JOURNAL_CHECK_AND_REPLAY.set(Some(Box::new(move || {
            for folder in [&swap_comments, &swap_vendor] {
                let mut away = folder.as_os_str().to_owned();
                away.push("-real");
                std::fs::rename(folder, away).unwrap();
                std::os::unix::fs::symlink(&to, folder).unwrap();
            }
        })));
        let Err(err) = undo_stopped_box_edit(&dir) else {
            unreachable!("a replay through a linked folder is refused")
        };
        assert!(err.contains("linked folder"), "{err}");
        let mut left: Vec<_> = std::fs::read_dir(&outside)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["a.comments.json", kind]);
        assert_eq!(std::fs::read(outside.join("a.comments.json")).unwrap(), b"outside");
        assert!(std::fs::read_dir(outside.join(kind)).unwrap().next().is_none());
        assert!(journal.exists());
    }

    /// A journaled file its owner may write but not read (mode 0200) is put back all the same,
    /// keeping its mode. Only a user other than root is refused the read: run as one, e.g.
    /// `setpriv --reuid=65534 --regid=65534 --clear-groups`, to take that path.
    #[cfg(unix)]
    #[test]
    fn a_write_only_file_is_put_back_keeping_its_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let (dir, _, _) = box_edit_project(tmp.path());
        let comments = dir.join(store::COMMENTS_DIR);
        std::fs::create_dir_all(&comments).unwrap();
        let target = comments.join("w.comments.json");
        std::fs::write(&target, b"half written").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o200)).unwrap();
        let name = format!("{}/w.comments.json", store::COMMENTS_DIR);
        let mut bytes = JOURNAL_HEADER.to_vec();
        bytes.extend_from_slice(format!("{} 2\n{name}hiend 1\n", name.len()).as_bytes());
        std::fs::write(dir.join(BOX_EDIT_JOURNAL), &bytes).unwrap();

        assert!(undo_stopped_box_edit(&dir).unwrap().is_some());
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o200);
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"hi");
        assert!(!dir.join(BOX_EDIT_JOURNAL).exists());
    }

    #[test]
    fn box_edit_refuses_to_join_a_removed_points_wire_to_a_free_wire_end_there() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        run(Cli::try_parse_from(["wiringcraft", "new", dir.to_str().unwrap(), "--no-git"]).unwrap()).unwrap();
        cli(&dir, &["page", "add", "p1", "--number", "1"]).unwrap();
        cli(
            &dir,
            &["box", "p1", "--at", "40,40", "--size", "20,10", "--pin", "IN:left:5"],
        )
        .unwrap();
        cli(&dir, &["wire", "p1", "s1:IN", "0,0"]).unwrap();
        let p = load(&dir).unwrap();
        let pg = p.page("p1").unwrap();
        let sym = store::read_vendored_symbol(&dir, &pg.symbols[0].symbol).unwrap();
        let at = wc_pdf::pin_position(&pg.symbols[0], &sym, "IN").unwrap();
        // Another wire ends freely where IN is, not joined to it.
        cli(&dir, &["wire", "p1", &format!("{},{}", at.x, at.y), "0,90"]).unwrap();
        let before = load(&dir).unwrap();
        let err = cli(&dir, &["box-edit", "p1", "s1", "--clear-pins"]).unwrap_err();
        assert!(err.contains("would be joined"), "{err}");
        assert_eq!(load(&dir).unwrap(), before, "nothing changes");
        assert_eq!(store::read_vendored_symbol(&dir, &sym.id).unwrap(), sym);
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

        // A symbol file left by a box that was never saved: its id is not taken over, and the
        // save removes it, as no page places it and the lock does not pin it (#490). Other
        // symbols nothing uses stay.
        let stale = wc_core::black_box("box.p1.s3", "old", 5.0, 5.0, &[]).unwrap();
        store::write_vendored_symbol(&dir, &stale).unwrap();
        let other = wc_core::black_box("maker.thing", "other", 5.0, 5.0, &[]).unwrap();
        store::write_vendored_symbol(&dir, &other).unwrap();
        boxed(&["--at", "100,100", "--size", "10,10"]).unwrap();
        let p = load(&dir).unwrap();
        let third = &p.page("p1").unwrap().symbols[2];
        assert_eq!((third.id.as_str(), third.symbol.as_str()), ("s4", "box.p1.s4"));
        assert!(
            store::read_vendored_symbol(&dir, "box.p1.s3").is_err(),
            "left-over removed"
        );
        assert!(store::read_vendored_symbol(&dir, "box.p1.s4").is_ok());
        assert_eq!(store::read_vendored_symbol(&dir, "maker.thing").unwrap(), other);

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
        std::fs::create_dir_all(lib.join("symbols").join("eplan.sie.sie_3rt2015.json")).unwrap();
        let (n, report, warnings) = import_edz(&file, &lib, None).unwrap();
        assert_eq!(n, 1);
        assert!(
            warnings
                .iter()
                .any(|w| w.starts_with("symbol eplan.sie.sie_3rt2015 not written")),
            "{warnings:?}"
        );
        assert_eq!(report.part_macros[0].symbol, None);
        let json = std::fs::read_to_string(lib.join("parts").join("sie.3rt2015.json")).unwrap();
        let p: wc_core::Part = serde_json::from_str(&json).unwrap();
        assert_eq!(p.symbol, None);

        // Once the symbol can be written, the part points at it and it loads back.
        std::fs::remove_dir(lib.join("symbols").join("eplan.sie.sie_3rt2015.json")).unwrap();
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

#[cfg(test)]
mod box_cleanup_rollback_tests {
    use super::*;

    /// A save that removes some unused black box symbol files and then fails: the rollback puts
    /// the removed ones back (#606), and leaves links and other symbols alone.
    #[test]
    fn a_failed_save_puts_back_box_symbol_files_its_cleanup_removed() {
        let Ok(tmp) = tempfile::tempdir() else {
            unreachable!("no temporary folder")
        };
        let dir = tmp.path();
        let p = Project::new("Cleanup");
        if let Err(err) = store::save(dir, &p) {
            unreachable!("{err}");
        }
        let folder = dir.join("vendor").join("symbols");
        let Ok(()) = std::fs::create_dir_all(&folder) else {
            unreachable!("no symbol folder")
        };
        for (name, body) in [
            ("box.p1.s1.json", "one"),
            ("box.p1.s2.json", "two"),
            ("relay.json", "relay"),
        ] {
            let Ok(()) = std::fs::write(folder.join(name), body) else {
                unreachable!("cannot write {name}")
            };
        }
        #[cfg(unix)]
        let Ok(()) = std::os::unix::fs::symlink("relay.json", folder.join("box.p1.s3.json")) else {
            unreachable!("cannot make a link")
        };
        let Ok(before) = snapshot_project(dir, &[], &[]) else {
            unreachable!("no snapshot")
        };
        let saved: Vec<&Path> = before.iter().map(|(path, _)| path.as_path()).collect();
        let named = |name: &str| saved.iter().any(|p| p.file_name().is_some_and(|n| n == name));
        assert!(named("box.p1.s1.json") && named("box.p1.s2.json"));
        assert!(!named("relay.json") && !named("box.p1.s3.json"), "{saved:?}");
        // The cleanup removed one file before it failed on the next.
        let Ok(()) = std::fs::remove_file(folder.join("box.p1.s1.json")) else {
            unreachable!("cannot remove")
        };
        assert_eq!(put_files_back(&before), Vec::<String>::new());
        assert_eq!(
            std::fs::read_to_string(folder.join("box.p1.s1.json")).ok().as_deref(),
            Some("one")
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("box.p1.s2.json")).ok().as_deref(),
            Some("two")
        );
        #[cfg(unix)]
        assert!(std::fs::symlink_metadata(folder.join("box.p1.s3.json")).is_ok_and(|m| m.file_type().is_symlink()));
    }
}

/// Inputs are found for every command that names one, and only streams are copied (#658).
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod input_tests {
    use super::*;
    use clap::CommandFactory;

    include!("input_commands.rs");

    /// Path arguments that are not input files: where output goes, library and project folders.
    const NOT_INPUTS: &[&str] = &["project", "dir", "out", "lib", "svg"];

    /// The commands under `command` that take a path argument which is not in [`NOT_INPUTS`],
    /// each as the words that name it.
    fn commands_with_a_path(command: &clap::Command, words: &mut Vec<String>, found: &mut Vec<Vec<String>>) {
        let path = clap::builder::ValueParser::path_buf().type_id();
        let takes_one = command
            .get_arguments()
            .any(|a| a.get_value_parser().type_id() == path && !NOT_INPUTS.contains(&a.get_id().as_str()));
        if takes_one && !words.is_empty() {
            found.push(words.clone());
        }
        for sub in command.get_subcommands() {
            words.push(sub.get_name().to_owned());
            commands_with_a_path(sub, words, found);
            words.pop();
        }
    }

    #[test]
    fn every_command_with_an_input_file_lists_it() {
        for args in INPUT_COMMANDS {
            let mut cli = Cli::try_parse_from(["wiringcraft"].iter().chain(*args)).unwrap();
            let got: Vec<PathBuf> = cli.cmd.inputs_mut().into_iter().map(|p| p.clone()).collect();
            let want: Vec<PathBuf> = args
                .iter()
                .filter(|a| ["FIFO", "FILE"].contains(a))
                .map(PathBuf::from)
                .collect();
            assert_eq!(got, want, "{args:?}");
        }
        // A library folder is not an input, also where the command can do without one.
        for args in [
            &["import", "archive", "in", "--lib", "lib"][..],
            &["import", "swe-library", "in", "--lib", "lib"],
        ] {
            let mut cli = Cli::try_parse_from(["wiringcraft"].iter().chain(args)).unwrap();
            let got: Vec<PathBuf> = cli.cmd.inputs_mut().into_iter().map(|p| p.clone()).collect();
            assert_eq!(got, [PathBuf::from("in")], "{args:?}");
        }
        // A command that takes a path which is not an output or a folder is in the list above,
        // so a new command with an input file fails here until it is covered.
        let mut found = Vec::new();
        commands_with_a_path(&Cli::command(), &mut Vec::new(), &mut found);
        assert!(found.len() >= 23, "{found:?}");
        for words in found {
            assert!(
                INPUT_COMMANDS
                    .iter()
                    .any(|args| args.iter().zip(&words).filter(|(a, w)| a == w).count() == words.len()),
                "`{}` takes a path that Cmd::inputs_mut and this test do not know",
                words.join(" ")
            );
        }
    }

    #[test]
    fn a_command_without_an_input_file_has_none() {
        for args in [
            &["check"][..],
            &["pdf", "out.pdf"],
            &["export", "wago-marking", "out.csv"],
        ] {
            let mut cli = Cli::try_parse_from(["wiringcraft"].iter().chain(args)).unwrap();
            assert!(cli.cmd.inputs_mut().is_empty(), "{args:?}");
        }
    }

    #[test]
    fn regular_files_folders_and_stdin_are_not_copied() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.elmt");
        std::fs::write(&file, "<definition/>").unwrap();
        let folder = tmp.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let missing = tmp.path().join("missing.elmt");
        let lib = tmp.path().join("lib");
        let mut cli = Cli::try_parse_from([
            "wiringcraft".as_ref(),
            "import".as_ref(),
            "symbol-qet".as_ref(),
            file.as_os_str(),
            folder.as_os_str(),
            missing.as_os_str(),
            "--lib".as_ref(),
            lib.as_os_str(),
        ])
        .unwrap();
        assert!(spool_inputs(&mut cli.cmd).unwrap().is_none());
        let after: Vec<PathBuf> = cli.cmd.inputs_mut().into_iter().map(|p| p.clone()).collect();
        assert_eq!(after, [file, folder, missing]);

        let mut cli = Cli::try_parse_from(["wiringcraft", "rules", "set", "-"]).unwrap();
        assert!(spool_inputs(&mut cli.cmd).unwrap().is_none());
        assert!(matches!(cli.cmd, Cmd::Rules(RulesCmd::Set { ref file }) if file.as_os_str() == "-"));
    }

    /// [`spool_inputs`] on another thread, given twenty seconds: a copy that waits for a
    /// writer that never comes, or that has no end, fails the test instead of holding it up.
    #[cfg(unix)]
    fn spooled(cli: Cli) -> (Vec<PathBuf>, Option<tempfile::TempDir>) {
        let limits = SpoolLimits {
            database: wc_import::swe::MAX_DATABASE_BYTES,
            stream: MAX_SPOOLED_BYTES,
        };
        let (inputs, spool) = spooled_within(cli, limits);
        (inputs, spool.unwrap())
    }

    /// [`spooled`] with `limits` in place of the ones the commands have, and what it said.
    #[cfg(unix)]
    fn spooled_within(mut cli: Cli, limits: SpoolLimits) -> (Vec<PathBuf>, Res<Option<tempfile::TempDir>>) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let spool = spool_inputs_within(&mut cli.cmd, limits);
            let inputs: Vec<PathBuf> = cli.cmd.inputs_mut().into_iter().map(|p| p.clone()).collect();
            let _ = tx.send((inputs, spool));
        });
        rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap()
    }

    /// Small limits for a test: a database of 64 KiB, any other stream of 1 MiB.
    #[cfg(unix)]
    const SMALL: SpoolLimits = SpoolLimits {
        database: 1 << 16,
        stream: 1 << 20,
    };

    /// One writer on another thread that opens the FIFO at `fifo` once and writes `head`, then
    /// zeros until the reader is gone or `most` bytes are written. The bytes it wrote.
    #[cfg(unix)]
    fn endless_writer(
        fifo: &Path,
        head: &'static [u8],
        most: u64,
    ) -> std::thread::JoinHandle<(u64, std::io::Result<()>)> {
        use std::io::Write;
        let fifo = fifo.to_path_buf();
        std::thread::spawn(move || {
            let mut to = match std::fs::OpenOptions::new().write(true).open(&fifo) {
                Ok(to) => to,
                Err(x) => return (0, Err(x)),
            };
            let mut sent = 0;
            let mut block: &[u8] = head;
            while sent < most {
                if let Err(x) = to.write_all(block) {
                    return (sent, Err(x));
                }
                sent += block.len() as u64;
                block = &[0; 4096];
            }
            (sent, Ok(()))
        })
    }

    /// A database for `import swe` on a FIFO whose writer does not stop: the copy ends one
    /// byte over the database limit, well under the limit of other streams, and the command
    /// is refused with the importer's words for a database that is too large.
    #[cfg(unix)]
    #[test]
    fn an_endless_swe_database_stream_is_refused_as_too_large() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("plant.db");
        mkfifo(&fifo);
        let writer = endless_writer(&fifo, b"SQLite format 3\0", 8 << 20);
        let cli = Cli::try_parse_from([
            "wiringcraft".as_ref(),
            "import".as_ref(),
            "swe".as_ref(),
            fifo.as_os_str(),
        ])
        .unwrap();
        let (_, spool) = spooled_within(cli, SMALL);
        let why = spool.map(|_| ()).unwrap_err();
        assert_eq!(why, format!("{}: is larger than 0 MB", fifo.display()));
        // The reader left: the writer was stopped, long before the limit of other streams.
        let (sent, end) = writer.join().unwrap();
        assert_eq!(end.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
        assert!(sent < SMALL.stream, "{sent}");
    }

    /// The database limit is for a database only: an archive for `import swe` and its symbol
    /// map, both larger than that on FIFOs that end, are copied whole.
    #[cfg(unix)]
    #[test]
    fn a_finite_swe_archive_stream_and_its_symbol_map_are_copied_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let (archive, map) = (tmp.path().join("plant.tewzip"), tmp.path().join("map.toml"));
        mkfifo(&archive);
        mkfifo(&map);
        let len = SMALL.database as usize + 4096;
        let mut zip = b"PK\x03\x04".to_vec();
        zip.resize(len, 0);
        // The header on the symbol map: only the first input is a database.
        let mut text = b"SQLite format 3\0".to_vec();
        text.resize(len, b'#');
        let writers = [writer(&archive, zip.clone()), writer(&map, text.clone())];
        let cli = Cli::try_parse_from([
            "wiringcraft".as_ref(),
            "import".as_ref(),
            "swe".as_ref(),
            archive.as_os_str(),
            "--symbol-map".as_ref(),
            map.as_os_str(),
        ])
        .unwrap();
        let (inputs, spool) = spooled_within(cli, SMALL);
        for writer in writers {
            writer.join().unwrap().unwrap();
        }
        let spool = spool.unwrap().unwrap();
        assert_eq!(inputs.len(), 2);
        assert!(inputs.iter().all(|i| i.starts_with(spool.path())), "{inputs:?}");
        assert!(std::fs::read(&inputs[0]).unwrap() == zip);
        assert!(std::fs::read(&inputs[1]).unwrap() == text);
    }

    /// A stream no importer limit applies to is refused when it has more than the limit for
    /// every stream, by the name the operand has; a regular file larger than that is not
    /// copied and not refused.
    #[cfg(unix)]
    #[test]
    fn an_endless_stream_without_an_importer_limit_is_refused_by_its_name() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from(["wiringcraft", "import", "pdf", "/dev/zero"]).unwrap();
        let (_, spool) = spooled_within(cli, SMALL);
        assert_eq!(
            spool.map(|_| ()).unwrap_err(),
            "/dev/zero: the stream is larger than 1 MB"
        );
        // A database header does not make another command's input a database.
        let fifo = tmp.path().join("plan.pdf");
        mkfifo(&fifo);
        let writer = endless_writer(&fifo, b"SQLite format 3\0", 8 << 20);
        let cli = Cli::try_parse_from([
            "wiringcraft".as_ref(),
            "import".as_ref(),
            "pdf".as_ref(),
            fifo.as_os_str(),
        ])
        .unwrap();
        let (_, spool) = spooled_within(cli, SMALL);
        let why = spool.map(|_| ()).unwrap_err();
        assert_eq!(why, format!("{}: the stream is larger than 1 MB", fifo.display()));
        let (sent, end) = writer.join().unwrap();
        assert_eq!(end.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
        assert!(sent > SMALL.stream, "{sent}");

        let file = tmp.path().join("large.pdf");
        std::fs::write(&file, vec![0; SMALL.stream as usize + 4096]).unwrap();
        let cli = Cli::try_parse_from([
            "wiringcraft".as_ref(),
            "import".as_ref(),
            "pdf".as_ref(),
            file.as_os_str(),
        ])
        .unwrap();
        let (inputs, spool) = spooled_within(cli, SMALL);
        assert!(spool.unwrap().is_none());
        assert_eq!(inputs, [file]);
    }

    /// A FIFO made with `mkfifo` at `path`.
    #[cfg(unix)]
    fn mkfifo(path: &Path) {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }

    /// One writer on another thread that opens the FIFO at `fifo` once and writes `bytes`.
    #[cfg(unix)]
    fn writer(fifo: &Path, bytes: Vec<u8>) -> std::thread::JoinHandle<std::io::Result<()>> {
        use std::io::Write;
        let fifo = fifo.to_path_buf();
        std::thread::spawn(move || std::fs::OpenOptions::new().write(true).open(&fifo)?.write_all(&bytes))
    }

    /// Two differently named operands that are one FIFO: it is read once, each operand keeps
    /// its own name, so the importer gives each its own id.
    #[cfg(unix)]
    #[test]
    fn two_names_of_one_fifo_keep_their_names() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("pipe");
        mkfifo(&fifo);
        let (first, second) = (tmp.path().join("first.elmt"), tmp.path().join("second.elmt"));
        std::os::unix::fs::symlink(&fifo, &first).unwrap();
        std::os::unix::fs::symlink(&fifo, &second).unwrap();
        let element =
            "<definition type=\"element\"><description><terminal x=\"0\" y=\"0\"/></description></definition>";
        let writer = writer(&fifo, element.as_bytes().to_vec());
        let lib = tmp.path().join("lib");
        let cli = Cli::try_parse_from([
            "wiringcraft".as_ref(),
            "import".as_ref(),
            "symbol-qet".as_ref(),
            first.as_os_str(),
            second.as_os_str(),
            "--lib".as_ref(),
            lib.as_os_str(),
        ])
        .unwrap();
        let (inputs, spool) = spooled(cli);
        writer.join().unwrap().unwrap();
        let spool = spool.unwrap();
        assert_eq!(inputs.len(), 2);
        for (input, operand) in inputs.iter().zip([&first, &second]) {
            assert!(input.starts_with(spool.path()), "{inputs:?}");
            assert_eq!(input.file_name(), operand.file_name());
            assert_eq!(std::fs::read_to_string(input).unwrap(), element);
        }
        let (written, failed) = import_qet(&inputs, &lib, "qet.", 0.5, "", "").unwrap();
        assert_eq!((written, failed), (2, 0));
        assert!(lib.join("symbols").join("qet.first.json").exists());
        assert!(lib.join("symbols").join("qet.second.json").exists());
    }

    /// An Eplan `.elk` FIFO whose `.edb` folder is beside it: the importer, given the copy,
    /// still finds the folder and reads the project's page from it.
    #[cfg(unix)]
    #[test]
    fn the_folder_of_an_elk_fifo_is_found_from_its_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let edb = tmp.path().join("plant.edb");
        std::fs::create_dir(&edb).unwrap();
        // One page record as the importer reads it: padding, the record start, its own
        // reference (class 4, a page, id 10), the page name and its properties block.
        let mut page = vec![0u8; 24];
        page.extend([0x0a, 0, 0, 0, 0x38, 0, 0xff, 0xff]);
        page.extend(60001u16.to_le_bytes());
        page.extend(4u16.to_le_bytes());
        page.extend(10u32.to_le_bytes());
        page.extend(61101u16.to_le_bytes());
        page.extend(b"1\0");
        page.extend(60011u16.to_le_bytes());
        page.extend(11011u16.to_le_bytes());
        page.extend(0u16.to_le_bytes());
        page.extend(b"en_US@MAIN CIRCUIT;\0");
        page.extend([0xff, 0xff]);
        std::fs::write(edb.join("page.eod"), &page).unwrap();
        let fifo = tmp.path().join("plant.elk");
        mkfifo(&fifo);
        let writer = writer(&fifo, b"plant.edb\r\n12.0\r\n".to_vec());
        let cli = Cli::try_parse_from([
            "wiringcraft".as_ref(),
            "import".as_ref(),
            "eplan".as_ref(),
            fifo.as_os_str(),
        ])
        .unwrap();
        let (inputs, spool) = spooled(cli);
        writer.join().unwrap().unwrap();
        let spool = spool.unwrap();
        assert!(inputs[0].starts_with(spool.path()), "{inputs:?}");
        let (tables, warnings) = wc_import::run::eplan_tables(&inputs[0]).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(tables.get("page.eod"), Some(&page));
        let project = wc_import::eplan::read(&tables).unwrap();
        let pages: Vec<(&str, &str)> = project
            .pages
            .iter()
            .map(|p| (p.name.as_str(), p.description.as_str()))
            .collect();
        assert_eq!(pages, [("1", "MAIN CIRCUIT")]);
        // The original folder is linked, not copied or moved: it is still where it was.
        drop(spool);
        assert!(edb.join("page.eod").exists());
    }

    // Used by the stream tests, which run on Unix only.
    #[cfg(unix)]
    const ELEMENT: &str =
        "<definition type=\"element\"><description><terminal x=\"0\" y=\"0\"/></description></definition>";

    /// A FIFO whose id an earlier element has is not opened: the import refuses it for its
    /// id, so a FIFO without a writer does not hold the command up. When the earlier file is
    /// no element, the FIFO is read and imported.
    #[cfg(unix)]
    #[test]
    fn a_fifo_with_the_id_of_an_earlier_element_is_not_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let (file, fifo) = (a.join("lamp.elmt"), b.join("lamp.elmt"));
        mkfifo(&fifo);
        let parse = |lib: &Path| {
            Cli::try_parse_from([
                "wiringcraft".as_ref(),
                "import".as_ref(),
                "symbol-qet".as_ref(),
                file.as_os_str(),
                fifo.as_os_str(),
                "--lib".as_ref(),
                lib.as_os_str(),
            ])
            .unwrap()
        };
        // No writer: a copy of the FIFO would never end.
        std::fs::write(&file, ELEMENT).unwrap();
        let lib = tmp.path().join("lib");
        let (inputs, spool) = spooled(parse(&lib));
        assert!(spool.is_none());
        assert_eq!(inputs, [file.clone(), fifo.clone()]);
        let (written, failed) = import_qet(&inputs, &lib, "qet.", 0.5, "", "").unwrap();
        assert_eq!((written, failed), (1, 1));

        std::fs::write(&file, "no element").unwrap();
        let lib = tmp.path().join("lib2");
        let writer = writer(&fifo, ELEMENT.as_bytes().to_vec());
        let (inputs, spool) = spooled(parse(&lib));
        writer.join().unwrap().unwrap();
        let spool = spool.unwrap();
        assert!(inputs[1].starts_with(spool.path()), "{inputs:?}");
        let (written, failed) = import_qet(&inputs, &lib, "qet.", 0.5, "", "").unwrap();
        assert_eq!((written, failed), (1, 1));
        assert!(lib.join("symbols").join("qet.lamp.json").exists());
    }

    /// The earlier element is changed between `spool_inputs` and the import so that it no
    /// longer reads as one: the FIFO `spool_inputs` left unread for its id is still refused
    /// without being opened, and the command ends (#700).
    #[cfg(unix)]
    #[test]
    fn a_fifo_left_unread_stays_unread_when_the_earlier_element_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let (file, fifo) = (a.join("lamp.elmt"), b.join("lamp.elmt"));
        mkfifo(&fifo);
        std::fs::write(&file, ELEMENT).unwrap();
        let lib = tmp.path().join("lib");
        let cli = Cli::try_parse_from([
            "wiringcraft".as_ref(),
            "import".as_ref(),
            "symbol-qet".as_ref(),
            file.as_os_str(),
            fifo.as_os_str(),
            "--lib".as_ref(),
            lib.as_os_str(),
        ])
        .unwrap();
        let (inputs, spool) = spooled(cli);
        assert!(spool.is_none());
        assert_eq!(inputs, [file.clone(), fifo.clone()]);
        // Between preparation and import, the earlier file stops being an element.
        std::fs::write(&file, "no element").unwrap();
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(import_qet(&inputs, &lib, "qet.", 0.5, "", ""));
        });
        // The FIFO has no writer: opening it would never return.
        let imported = result.recv_timeout(std::time::Duration::from_secs(20)).ok();
        assert_eq!(
            imported.map(|r| r.unwrap()),
            Some((0, 2)),
            "the import opened the FIFO left unread"
        );
    }

    /// A stream that cannot be opened (a socket) is left to the import, which counts it as
    /// failed and imports the elements before and after it.
    #[cfg(unix)]
    #[test]
    fn a_stream_that_cannot_be_opened_fails_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (before, socket, after) = (
            tmp.path().join("before.elmt"),
            tmp.path().join("socket.elmt"),
            tmp.path().join("after.elmt"),
        );
        std::fs::write(&before, ELEMENT).unwrap();
        std::fs::write(&after, ELEMENT).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let lib = tmp.path().join("lib");
        let cli = Cli::try_parse_from([
            "wiringcraft".as_ref(),
            "import".as_ref(),
            "symbol-qet".as_ref(),
            before.as_os_str(),
            socket.as_os_str(),
            after.as_os_str(),
            "--lib".as_ref(),
            lib.as_os_str(),
        ])
        .unwrap();
        let (inputs, spool) = spooled(cli);
        assert!(spool.is_none());
        assert_eq!(inputs, [before, socket, after]);
        let (written, failed) = import_qet(&inputs, &lib, "qet.", 0.5, "", "").unwrap();
        assert_eq!((written, failed), (2, 1));
        assert!(lib.join("symbols").join("qet.before.json").exists());
        assert!(lib.join("symbols").join("qet.after.json").exists());
    }

    /// A FIFO named twice is read once: its one writer writes one element, the first operand
    /// gets the copy, and the second is left unread, as the importer refuses it for its id as
    /// it does for a file.
    #[cfg(unix)]
    #[test]
    fn a_fifo_named_twice_is_copied_once() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("lamp.elmt");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let element =
            "<definition type=\"element\"><description><terminal x=\"0\" y=\"0\"/></description></definition>";
        let writer = {
            let fifo = fifo.clone();
            std::thread::spawn(move || {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&fifo)?
                    .write_all(element.as_bytes())
            })
        };
        let lib = tmp.path().join("lib");
        let cli = Cli::try_parse_from([
            "wiringcraft".as_ref(),
            "import".as_ref(),
            "symbol-qet".as_ref(),
            fifo.as_os_str(),
            fifo.as_os_str(),
            "--lib".as_ref(),
            lib.as_os_str(),
        ])
        .unwrap();
        let (inputs, spool) = spooled(cli);
        writer.join().unwrap().unwrap();
        let spool = spool.unwrap();
        assert_eq!(inputs.len(), 2);
        assert!(inputs[0].starts_with(spool.path()), "{inputs:?}");
        assert_eq!(inputs[0].file_name(), fifo.file_name());
        assert_eq!(std::fs::read_to_string(&inputs[0]).unwrap(), element);
        assert_eq!(inputs[1], fifo);
        let (written, failed) = import_qet(&inputs, &lib, "qet.", 0.5, "", "").unwrap();
        assert_eq!((written, failed), (1, 1));
        assert!(lib.join("symbols").join("qet.lamp.json").exists());
    }

    /// An element stream without an end is copied up to one byte over what the importer
    /// reads, and refused by it as too large: that element fails, the command does not.
    #[cfg(unix)]
    #[test]
    fn an_endless_element_stream_is_copied_up_to_the_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        let cli = Cli::try_parse_from([
            "wiringcraft".as_ref(),
            "import".as_ref(),
            "symbol-qet".as_ref(),
            "/dev/zero".as_ref(),
            "--lib".as_ref(),
            lib.as_os_str(),
        ])
        .unwrap();
        let (inputs, spool) = spooled(cli);
        assert!(spool.is_some());
        let len = std::fs::metadata(&inputs[0]).unwrap().len();
        assert_eq!(len, wc_import::qet::MAX_LEN as u64 + 1);
        let (written, failed) = import_qet(&inputs, &lib, "qet.", 0.5, "", "").unwrap();
        assert_eq!((written, failed), (0, 1));
    }
}
