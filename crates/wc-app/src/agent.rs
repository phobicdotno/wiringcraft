//! The assistant: a panel where the user talks to Claude, which works in the open project
//! through tools. Every tool runs here on the app's thread, through the same code paths as the
//! toolbar, menus and properties panel, so changes appear on the canvas as they happen and one
//! Undo takes back everything the assistant did in a turn.
//!
//! Coverage: the tools below edit everything the editor edits (symbols, wires, graphics, pages,
//! devices, project properties, comments, macros, panel layouts), run every checker, report,
//! export and version command, `run_action` triggers any toolbar or menu command, and
//! `run_command` runs any `wiringcraft` command-line command (imports, PLC variable lists,
//! STEP export, library sources, merges, ...) on the open project.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use serde_json::{Value, json};
use wc_core::geom::{Point, Rotation, Shape};
use wc_core::project::PageSize;
use wc_core::{DeviceTag, Endpoint, Page};

use crate::actions::Action;
use crate::screenshot::{self, Shot};
use crate::state::{Open, Selection};
use crate::{App, Dialog};
use wc_agent::{Pending, Started};

/// Instructions for the model. Kept fixed so the API can cache it with the tools.
pub const SYSTEM: &str = "You are the assistant built into WiringCraft, an electrical schematic and wiring CAD \
program (pages of schematics with symbols, wires, devices and parts, cabinet panel layouts, reports, PDF export, \
and git-based versions). The user is an electrical designer talking to you while the project is open on their \
screen. You work in the open project through your tools; every change shows on their canvas immediately, and \
one Undo (Ctrl+Z) takes back everything you changed in a turn.

How the drawing works:
- Coordinates are millimetres on the sheet, origin at the top-left corner, x to the right, y DOWN. Sheets are \
landscape: A4 297x210, A3 420x297 (default), A2 594x420. The frame is 10 mm in from the edge and divided into 10 \
reference columns (1-10 along the top) and rows (A-F on A4 and A3, A-H on A2, down both sides); cross-references \
give page, column and row, such as 12/3.C. The title block sits at the bottom. Keep drawings inside roughly x 20..(width-20), y 20..(height-45).
- Put symbols and wire bends on a 5 mm grid (the editor snaps to 2.5 mm). Leave room: about 20-30 mm between \
neighbouring symbols, current paths drawn as vertical columns from a top supply rail down to a bottom rail.
- A placed symbol has a page-unique id (s1, s2, ...) and pins with names (13, 14, A1, A2, L1 ...). Wires go from \
pin to pin (`{\"item\": \"s3\", \"pin\": \"13\"}`) or to a free point (`{\"x\": 50, \"y\": 40}`), with optional \
bend points in `via`. Always look up pin names and positions (find_symbols, get_page) before wiring; never guess.
- Devices are identified by tags like =PUMP+CAB1-K1 (=function, +location, -product with letter code: K relay or \
contactor, Q breaker/contactor, F fuse, S switch, H lamp, M motor, X terminal, W cable, A assembly/PLC). Several \
symbols with the same tag are one device (a coil and its contacts). A new symbol gets the next free tag \
automatically; set a tag yourself to make it part of an existing device.
- Wires with the same `potential` name are connected across pages. Interruption-point symbols continue a wire on \
another page.

How to work:
- Start by calling get_state to see the project and current page, unless you already know them. Use get_page \
to see what is drawn, find_symbols / find_parts to find library items.
- Make the change with the specific tools. Prefer them over edit_page_json, which is for bulk changes the others \
cannot do. run_command runs the wiringcraft command line for everything else (imports, PLC variable lists, panel \
STEP export, library sources, branches and merges); run it with [\"--help\"] or [\"<command>\", \"--help\"] to \
see the options.
- After drawing, check your work: get_page to confirm positions, run_check for open pins, shorts and missing coils, \
and fix what you broke.
- Do not save, commit, delete pages, or overwrite files unless the user asked for it or it is plainly part of \
what they asked. Never invent part numbers; use find_parts.
- Work only through the WiringCraft tools: do not use shell, file or web tools.
- Requests make WiringCraft better. When the user reports an error, asks for something WiringCraft cannot do yet, \
or misses something (a symbol, part, report, setting), and when you hit a tool error that looks like a bug or a \
capability that is not there, put it on the todo list with the todo tool: search first so you do not file a \
duplicate, then file it with a title, what and done-when. Show the user the text and ask before filing with \
confirmed: true; without it the form opens in their browser for them to submit. Keep customer names, project \
contents and paths out of it. Then carry on with what you can do now.
- Answer briefly in plain language: what you did, and anything the user should look at. The user cannot see your \
tool calls in detail.";

/// The assistant panel's state.
pub struct Assistant {
    pub session: wc_agent::Session,
    pub input: String,
    pub show_settings: bool,
    /// The project folder and undo mark when the running turn started.
    pub turn: Option<(PathBuf, usize)>,
    specs: Vec<Value>,
    /// Claude login: WiringCraft's tools as an MCP server, started with the first message.
    pub bridge: Option<wc_agent::bridge::Bridge>,
    /// Claude login: the running Claude Code session.
    pub acp: Option<wc_agent::acp::Acp>,
    /// Claude login: the conversation as shown.
    pub login_log: Vec<wc_agent::Entry>,
    /// Screenshots waiting to go with the next message, with their thumbnails once shown.
    pub waiting: Vec<(Shot, Option<egui::TextureHandle>)>,
    /// The last message's words and screenshots, offered again if it could not be sent.
    pub sent_text: String,
    pub sent: Vec<Shot>,
    /// The latest screenshots of this conversation, for a request the assistant files.
    pub shared: Vec<Shot>,
    pub clipboard: screenshot::Clipboard,
    /// The project folder and project when a `run_command` started on its worker thread, so
    /// its end can tell the command's changes from the user's edits made meanwhile.
    command_base: Option<(PathBuf, wc_core::Project)>,
}

impl Default for Assistant {
    fn default() -> Self {
        Self::with_transport(Arc::new(wc_agent::Http::default()))
    }
}

impl Assistant {
    pub fn with_transport(t: Arc<dyn wc_agent::Transport>) -> Self {
        Self {
            session: wc_agent::Session::new(t),
            input: String::new(),
            show_settings: false,
            turn: None,
            specs: specs(),
            bridge: None,
            acp: None,
            login_log: Vec::new(),
            waiting: Vec::new(),
            sent_text: String::new(),
            sent: Vec::new(),
            shared: Vec::new(),
            clipboard: screenshot::Clipboard::default(),
            command_base: None,
        }
    }

    pub fn specs(&self) -> &[Value] {
        &self.specs
    }

    /// The last message was not answered: put its words (unless something new was typed) and
    /// its screenshots back in the box. Screenshots added since stay, after the given-back ones,
    /// as far as there is room; returns how many of those did not fit. (Drops are refused while
    /// a message is on its way, so normally nothing was added.)
    pub fn give_back(&mut self, text: Option<String>) -> usize {
        let text = text.unwrap_or_else(|| std::mem::take(&mut self.sent_text));
        if self.input.trim().is_empty() {
            self.input = text;
        }
        let newer = std::mem::take(&mut self.waiting);
        self.waiting = std::mem::take(&mut self.sent).into_iter().map(|s| (s, None)).collect();
        let room = screenshot::MAX_WAITING.saturating_sub(self.waiting.len());
        let left_out = newer.len().saturating_sub(room);
        self.waiting.extend(newer.into_iter().take(room));
        left_out
    }

    /// Working on a message, with either backend.
    pub fn busy(&self) -> bool {
        self.session.busy()
            || self.acp.as_ref().is_some_and(wc_agent::acp::Acp::busy)
            || self.bridge.as_ref().is_some_and(wc_agent::bridge::Bridge::working)
    }

    /// A `run_command` is running on its worker thread. The window does not save meanwhile,
    /// so whatever changes on disk is the command's.
    pub fn command_running(&self) -> bool {
        self.command_base.is_some()
    }
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "input_schema": {
            "type": "object",
            "properties": properties,
            "required": required,
        }
    })
}

/// The `page` argument most tools take.
fn page_arg() -> Value {
    json!({"type": "string", "description": "Page number as shown (e.g. \"3\") or page id (e.g. \"p0003\"). Default: the page shown."})
}

fn end_arg(what: &str) -> Value {
    json!({
        "description": format!("{what}: a pin {{\"item\": \"s1\", \"pin\": \"13\"}}, or a free point {{\"x\": 50, \"y\": 40}} in mm."),
        "type": "object",
        "properties": {
            "item": {"type": "string"}, "pin": {"type": "string"},
            "x": {"type": "number"}, "y": {"type": "number"}
        }
    })
}

fn ids_arg() -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": "Item ids on the page: symbols (s1), wires (w2) and free drawings (g0, g1 by index as get_page lists them)."})
}

fn point_list() -> Value {
    json!({"type": "array", "items": {"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}}, "required": ["x", "y"]}})
}

/// Every tool the assistant has.
pub fn specs() -> Vec<Value> {
    let actions: Vec<String> = Action::ALL.iter().map(|a| a.name()).collect();
    vec![
        tool(
            "get_state",
            "Overview of the open project: name, folder, branch, unsaved changes, pages (id, number, title, size, counts), panel layouts, devices count, open comments, the page or panel shown, the selection and whether undo is possible.",
            json!({}),
            &[],
        ),
        tool(
            "get_page",
            "Everything on one page: placed symbols with tag, rotation and the absolute position of every pin; wires with their end points, bends, number, potential and cable; free drawings (g0, g1, ...); the underlay; review comments.",
            json!({"page": page_arg()}),
            &[],
        ),
        tool(
            "find_symbols",
            "Search the project's symbols and its library sources by id, name, category or description. Returns id, name, category, letter code, role and pins (name, position relative to the symbol origin, direction).",
            json!({"query": {"type": "string", "description": "Words to look for; empty lists all."}, "limit": {"type": "integer"}}),
            &[],
        ),
        tool(
            "find_parts",
            "Search the parts (manufacturer, order number, description) in the project and its library sources. Returns part ids to use with set_device.",
            json!({"query": {"type": "string"}, "limit": {"type": "integer"}}),
            &[],
        ),
        tool(
            "get_devices",
            "List devices: tag, description, part, properties, and where each is drawn (page number and symbol id).",
            json!({"query": {"type": "string", "description": "Only tags or descriptions containing this."}}),
            &[],
        ),
        tool(
            "place_symbol",
            "Place a library symbol on a page. The symbol is taken into the project from the library when needed. Without `tag` the device gets the next free tag for its letter code. Returns the new id, tag and pin positions.",
            json!({
                "symbol": {"type": "string", "description": "Symbol id from find_symbols, e.g. wc.contact.no"},
                "x": {"type": "number"}, "y": {"type": "number"},
                "page": page_arg(),
                "rotation": {"type": "integer", "enum": [0, 90, 180, 270]},
                "mirror": {"type": "boolean"},
                "tag": {"type": "string", "description": "Device tag, e.g. =PUMP+CAB1-K1, to join an existing device or set your own."},
                "description": {"type": "string", "description": "Device description."}
            }),
            &["symbol", "x", "y"],
        ),
        tool(
            "add_wire",
            "Draw a wire between two pins or points on one page.",
            json!({
                "from": end_arg("Start"), "to": end_arg("End"),
                "via": point_list(),
                "page": page_arg(),
                "number": {"type": "string"},
                "potential": {"type": "string", "description": "Signal name, e.g. 24V or L1; same name = connected across pages."}
            }),
            &["from", "to"],
        ),
        tool(
            "update_symbol",
            "Change a placed symbol: move it, rotate (absolute 0/90/180/270), mirror, change its tag (empty string removes it) or swap the symbol drawing.",
            json!({
                "id": {"type": "string"}, "page": page_arg(),
                "x": {"type": "number"}, "y": {"type": "number"},
                "rotation": {"type": "integer", "enum": [0, 90, 180, 270]},
                "mirror": {"type": "boolean"},
                "tag": {"type": "string"},
                "symbol": {"type": "string"}
            }),
            &["id"],
        ),
        tool(
            "update_wire",
            "Change a wire: its ends, bends, number, potential, the cable it is a core of, or its cross-section, colour and type. Empty strings clear number, potential, cable, section, colour or type.",
            json!({
                "id": {"type": "string"}, "page": page_arg(),
                "from": end_arg("New start"), "to": end_arg("New end"), "via": point_list(),
                "number": {"type": "string"}, "potential": {"type": "string"},
                "cable": {"type": "string", "description": "Cable device tag, e.g. -W1"},
                "core": {"type": "string", "description": "Core name in the cable, e.g. 1 or BN"},
                "section": {"type": "string", "description": "Cross-section with unit, e.g. 1.5 mm² or 18 AWG"},
                "colour": {"type": "string", "description": "Colour code, e.g. BU or GNYE"},
                "type": {"type": "string", "description": "Wire type, e.g. H07V-K"}
            }),
            &["id"],
        ),
        tool(
            "update_graphic",
            "Replace a free drawing (line, polyline, rect, circle, arc, text) by its id from get_page (g0, g1 ...).",
            json!({"id": {"type": "string"}, "shape": shape_arg(), "page": page_arg()}),
            &["id", "shape"],
        ),
        tool(
            "add_graphic",
            "Add a free drawing to a page: a line, polyline, rectangle, circle, arc or text (labels, notes, frames, boxes around a group).",
            json!({"shape": shape_arg(), "page": page_arg()}),
            &["shape"],
        ),
        tool(
            "delete_items",
            "Delete symbols, wires and free drawings. Wires on a deleted symbol go too.",
            json!({"ids": ids_arg(), "page": page_arg()}),
            &["ids"],
        ),
        tool(
            "move_items",
            "Move items by an offset in mm. Wire ends on moved symbols follow.",
            json!({"ids": ids_arg(), "dx": {"type": "number"}, "dy": {"type": "number"}, "page": page_arg()}),
            &["ids", "dx", "dy"],
        ),
        tool(
            "rotate_items",
            "Rotate symbols 90 degrees each (repeat for more).",
            json!({"ids": ids_arg(), "page": page_arg()}),
            &["ids"],
        ),
        tool(
            "mirror_items",
            "Mirror symbols left to right.",
            json!({"ids": ids_arg(), "page": page_arg()}),
            &["ids"],
        ),
        tool(
            "copy_items",
            "Copy items and paste them, on the same or another page. The group's top-left corner lands on x, y (default: 10 mm right and down of the original). Copied devices get the next free tags. Returns the new ids.",
            json!({"ids": ids_arg(), "page": page_arg(), "to_page": page_arg(), "x": {"type": "number"}, "y": {"type": "number"}}),
            &["ids"],
        ),
        tool(
            "select_items",
            "Show the user something: go to its page and select the items on the canvas.",
            json!({"ids": ids_arg(), "page": page_arg()}),
            &["ids"],
        ),
        tool(
            "add_page",
            "Add a schematic page and show it. Returns its id and number.",
            json!({"number": {"type": "string"}, "title": {"type": "string"}, "size": {"type": "string", "enum": ["A4", "A3", "A2"]}}),
            &[],
        ),
        tool(
            "update_page",
            "Change a page's number, title or sheet size, or mark it as an overview (a line diagram: its devices do not count as drawings in the project check).",
            json!({"page": page_arg(), "number": {"type": "string"}, "title": {"type": "string"}, "size": {"type": "string", "enum": ["A4", "A3", "A2"]}, "overview": {"type": "boolean"}}),
            &[],
        ),
        tool(
            "delete_page",
            "Delete a page (the last page of a project cannot be deleted).",
            json!({"page": page_arg()}),
            &["page"],
        ),
        tool(
            "duplicate_page",
            "Copy a page as a new page; its devices get the next free tags.",
            json!({"page": page_arg()}),
            &[],
        ),
        tool(
            "move_page",
            "Move a page one place up or down in the page order.",
            json!({"page": page_arg(), "direction": {"type": "string", "enum": ["up", "down"]}}),
            &["direction"],
        ),
        tool(
            "show",
            "Show a page or a panel layout on the user's screen.",
            json!({"page": page_arg(), "panel": {"type": "string", "description": "Panel layout id or name."}}),
            &[],
        ),
        tool(
            "edit_page_json",
            "Replace a whole page with new content in the page file format (as get_page returns it, without the extra pin and point fields). For bulk edits the other tools cannot do. The page id stays.",
            json!({"page": page_arg(), "content": {"type": "object", "description": "The page: number, title, size, overview (true for a line diagram; leaving it out makes a normal page), symbols, wires, graphics, underlay."}}),
            &["content"],
        ),
        tool(
            "set_device",
            "Set a device's description, part (from find_parts; empty string removes it) and properties (empty value removes one), or rename its tag everywhere (symbols, cables, panel layouts).",
            json!({
                "tag": {"type": "string"},
                "description": {"type": "string"},
                "part": {"type": "string"},
                "properties": {"type": "object", "additionalProperties": {"type": "string"}},
                "rename_to": {"type": "string"}
            }),
            &["tag"],
        ),
        tool(
            "set_project",
            "Rename the project and set title-block fields (customer, project_number, plant, drawn_by, checked_by, revision, date, or any other; empty value removes one).",
            json!({"name": {"type": "string"}, "title_block": {"type": "object", "additionalProperties": {"type": "string"}}}),
            &[],
        ),
        tool(
            "comments",
            "Review comments: list (open ones, or all), add (pinned at x, y on a page, or about the whole page), reply, resolve, reopen or delete.",
            json!({
                "action": {"type": "string", "enum": ["list", "add", "reply", "resolve", "reopen", "delete"]},
                "id": {"type": "string"}, "text": {"type": "string"}, "page": page_arg(),
                "x": {"type": "number"}, "y": {"type": "number"},
                "all": {"type": "boolean"}
            }),
            &["action"],
        ),
        tool(
            "macros",
            "Macros (saved groups of symbols, wires and drawings): list them, insert one on a page (its top-left corner at x, y), or save items from a page as a new project macro.",
            json!({
                "action": {"type": "string", "enum": ["list", "insert", "save"]},
                "name": {"type": "string"}, "description": {"type": "string"},
                "ids": ids_arg(), "page": page_arg(),
                "x": {"type": "number"}, "y": {"type": "number"}
            }),
            &["action"],
        ),
        tool(
            "number_wires",
            "Give every connection without a number one. Existing numbers are kept.",
            json!({"rule": {"type": "string", "enum": ["page_column", "sequential", "potential"], "description": "page_column (3.4: page and column, default), sequential (1, 2, 3) or potential (the potential's name)."}}),
            &[],
        ),
        tool(
            "run_check",
            "Check the project: devices drawn twice, contacts without a coil, shorted potentials, open pins, wires ending in the air, devices without a part.",
            json!({}),
            &[],
        ),
        tool(
            "get_report",
            "A report as a Markdown table.",
            json!({"kind": {"type": "string", "enum": ["contents", "parts", "devices", "wires", "nets", "terminals", "cables", "plc", "locations"]}}),
            &["kind"],
        ),
        tool(
            "panels",
            "Cabinet panel layouts: get (items as JSON), new (mounting plate with ducts and DIN rails), arrange (put devices with parts onto the rails, optionally one location like +CAB1), delete, or replace (set the layout's items, name and size from JSON).",
            json!({
                "action": {"type": "string", "enum": ["get", "new", "arrange", "delete", "replace"]},
                "panel": {"type": "string", "description": "Layout id or name (default: the one shown)."},
                "name": {"type": "string"}, "width": {"type": "number"}, "height": {"type": "number"},
                "location": {"type": "string"},
                "content": {"type": "object"}
            }),
            &["action"],
        ),
        tool(
            "library",
            "The project's library: status (pinned items compared with the library), or update (take newer library versions, all or the ids given).",
            json!({"action": {"type": "string", "enum": ["status", "update"]}, "ids": {"type": "array", "items": {"type": "string"}}}),
            &["action"],
        ),
        tool(
            "export",
            "Export into the project's export folder: pdf (all pages and reports), drawings (SVG and DXF per page) or aml (AutomationML).",
            json!({"format": {"type": "string", "enum": ["pdf", "drawings", "aml"]}}),
            &["format"],
        ),
        tool(
            "versions",
            "Saving and versions: save (write changed pages to disk), commit (save and record a version with a message), branch (start a branch and switch to it), history (the version graph), page_history (who changed a page and what).",
            json!({
                "action": {"type": "string", "enum": ["save", "commit", "branch", "history", "page_history"]},
                "message": {"type": "string"}, "name": {"type": "string"}, "page": page_arg()
            }),
            &["action"],
        ),
        tool(
            "undo",
            "Undo or redo the last change.",
            json!({"redo": {"type": "boolean"}}),
            &[],
        ),
        tool(
            "open_project",
            "Open a project folder, or create a new one (a git repository with the starter library).",
            json!({"path": {"type": "string"}, "create": {"type": "boolean"}, "name": {"type": "string"}}),
            &["path"],
        ),
        tool(
            "run_action",
            "Run any toolbar or menu command by name, exactly as if the user clicked it. Commands that open a dialog show it to the user. Prefer the specific tools above; use this for view commands (ZoomIn, ZoomOut, ZoomFit, ToggleGrid, ToggleTheme, ToggleChrome) and anything they do not cover.",
            json!({"action": {"type": "string", "enum": actions}}),
            &["action"],
        ),
        tool(
            "run_command",
            "Run a wiringcraft command-line command on the open project (unsaved changes are saved first, and the project is reloaded after). Covers everything: page, place, wire, device, number-wires, library (add, sync, status, update, lock, search, propose), import (DXF/SVG/KiCad/QElectroTech symbols, parts CSV and BMEcat, PLC address CSV, archives, DWG, PDF, Eplan and SolidWorks Electrical projects), pdf, drawing, report, nets, aml, plc-vars, check, inspect, vcs (commit, branch, switch, merge, history, page-log), comment, panel (new, arrange, step, list, remove). Use [\"--help\"] to list commands.",
            json!({"args": {"type": "array", "items": {"type": "string"}, "description": "Arguments, e.g. [\"plc-vars\", \"export/io.st\"]. The project folder is passed for you."}}),
            &["args"],
        ),
        tool(
            "todo",
            "WiringCraft's todo list (GitHub issues labelled todo, built in order by the project's agents and shipped in later versions). action \"search\": look for an existing todo before filing (needs the GitHub command line). action \"file\": put a problem, a requested function or a missing item on the list. Without confirmed, or without the GitHub command line, it opens the filled-in form in the user's browser for them to read and submit; with confirmed: true and the GitHub command line signed in, it files the issue directly and returns its link. The text is public: describe the problem in general terms, never customer names, project contents or file paths.",
            json!({
                "action": {"type": "string", "enum": ["search", "file"]},
                "query": {"type": "string", "description": "search: words to look for."},
                "kind": {"type": "string", "enum": ["problem", "function", "missing"], "description": "file: problem (error, crash, wrong result), function (something WiringCraft should do), missing (a symbol, part, report, setting or step that is not there)."},
                "title": {"type": "string", "description": "file: one line that makes sense on its own, e.g. \"PDF export: wire numbers overlap at 90 degree bends\"."},
                "what": {"type": "string", "description": "file: what happened or what is wanted, and why it matters."},
                "detail": {"type": "string", "description": "file: for a problem, how to make it happen again; for a function, an example of using it."},
                "done": {"type": "string", "description": "file: how anyone can check it is finished."},
                "confirmed": {"type": "boolean", "description": "file: true only when the user read this text and agreed to file it."}
            }),
            &["action"],
        ),
    ]
}

fn shape_arg() -> Value {
    json!({
        "type": "object",
        "description": "{\"kind\": \"line\", \"a\": {x,y}, \"b\": {x,y}} | {\"kind\": \"polyline\", \"points\": [...], \"closed\": bool} | {\"kind\": \"rect\", \"min\": {x,y}, \"max\": {x,y}} | {\"kind\": \"circle\", \"center\": {x,y}, \"radius\": r, \"filled\": bool} | {\"kind\": \"arc\", \"center\": {x,y}, \"radius\": r, \"start\": deg, \"end\": deg} | {\"kind\": \"text\", \"at\": {x,y}, \"text\": \"...\", \"height\": 2.5, \"align\": \"left|center|right\", \"angle\": deg}",
        "properties": {"kind": {"type": "string", "enum": ["line", "polyline", "rect", "circle", "arc", "text"]}},
        "required": ["kind"]
    })
}

// ---- argument helpers ----

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn need_s<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    s(v, key).ok_or_else(|| format!("`{key}` is missing."))
}

fn need_num(v: &Value, key: &str) -> Result<f64, String> {
    num(v, key).ok_or_else(|| format!("`{key}` must be a number."))
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

fn rotation(v: &Value) -> Result<Option<Rotation>, String> {
    match v.get("rotation").and_then(Value::as_i64) {
        None => Ok(None),
        Some(0) => Ok(Some(Rotation::R0)),
        Some(90) => Ok(Some(Rotation::R90)),
        Some(180) => Ok(Some(Rotation::R180)),
        Some(270) => Ok(Some(Rotation::R270)),
        Some(r) => Err(format!("rotation {r} is not 0, 90, 180 or 270")),
    }
}

fn page_size(v: &Value) -> Result<Option<PageSize>, String> {
    match s(v, "size") {
        None => Ok(None),
        Some(t) => match t.to_ascii_uppercase().as_str() {
            "A4" => Ok(Some(PageSize::A4)),
            "A3" => Ok(Some(PageSize::A3)),
            "A2" => Ok(Some(PageSize::A2)),
            _ => Err(format!("size {t} is not A4, A3 or A2")),
        },
    }
}

fn points(v: &Value, key: &str) -> Result<Option<Vec<Point>>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(p) => serde_json::from_value(p.clone())
            .map(Some)
            .map_err(|e| format!("`{key}` must be a list of {{x, y}} points: {e}")),
    }
}

fn tag(t: &str) -> Result<DeviceTag, String> {
    DeviceTag::parse(t.trim()).map_err(|e| format!("{t:?} is not a device tag: {e}"))
}

fn pretty(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

fn clip_text(t: String, max: usize) -> String {
    if t.chars().count() <= max {
        return t;
    }
    let mut out: String = t.chars().take(max).collect();
    out.push_str("\n… (cut off)");
    out
}

fn selection_id(sel: &Selection) -> String {
    match sel {
        Selection::Symbol(id) | Selection::Wire(id) => id.clone(),
        Selection::Graphic(i) => format!("g{i}"),
    }
}

/// The open project, or the error the model sees when there is none.
fn need_open(open: Option<&mut Open>) -> Result<&mut Open, String> {
    open.ok_or_else(|| "No project is open. Use open_project first.".to_owned())
}

impl Open {
    /// A page by its number or id.
    fn find_page(&self, key: &str) -> Option<usize> {
        let key = key.trim();
        self.project
            .pages
            .iter()
            .position(|p| p.number == key)
            .or_else(|| self.project.pages.iter().position(|p| p.id == key))
    }

    /// Item ids on a page as selections; unknown ids are an error.
    fn selections(&self, page: usize, ids: &[String]) -> Result<Vec<Selection>, String> {
        let p = self.project.pages.get(page).ok_or("No such page.")?;
        if ids.is_empty() {
            return Err("`ids` is empty.".into());
        }
        ids.iter()
            .map(|id| {
                if p.symbols.iter().any(|s| &s.id == id) {
                    Ok(Selection::Symbol(id.clone()))
                } else if p.wires.iter().any(|w| &w.id == id) {
                    Ok(Selection::Wire(id.clone()))
                } else if let Some(i) = id.strip_prefix('g').and_then(|n| n.parse::<usize>().ok())
                    && i < p.graphics.len()
                {
                    Ok(Selection::Graphic(i))
                } else {
                    Err(format!("No item {id:?} on page {}.", p.number))
                }
            })
            .collect()
    }

    /// A wire end from the model's input, checked against the page.
    fn endpoint(&self, page: usize, v: Option<&Value>) -> Result<Endpoint, String> {
        let v = v.ok_or("A wire end is missing.")?;
        let pg = self.project.pages.get(page).ok_or("No such page.")?;
        if let Some(text) = v.as_str() {
            let (item, pin) = text
                .split_once(':')
                .ok_or_else(|| format!("{text:?}: give an end as {{item, pin}} or {{x, y}}"))?;
            return self.endpoint(page, Some(&json!({"item": item, "pin": pin})));
        }
        if let (Some(item), Some(pin)) = (s(v, "item"), v.get("pin")) {
            let pin = match pin {
                Value::String(p) => p.clone(),
                other => other.to_string(),
            };
            let placed = pg
                .symbol(item)
                .ok_or_else(|| format!("No symbol {item:?} on page {}.", pg.number))?;
            if self.pin_pos(pg, item, &pin).is_none() {
                let names: Vec<String> = self
                    .symbols
                    .get(&placed.symbol)
                    .map(|s| s.pins.iter().map(|p| p.name.clone()).collect())
                    .unwrap_or_default();
                return Err(format!("{item} has no pin {pin:?}; its pins are {}.", names.join(", ")));
            }
            return Ok(Endpoint::Pin {
                item: item.to_owned(),
                pin,
            });
        }
        match (num(v, "x"), num(v, "y")) {
            (Some(x), Some(y)) => Ok(Endpoint::Point { at: Point::new(x, y) }),
            _ => Err("A wire end needs {item, pin} or {x, y}.".into()),
        }
    }

    /// One page as the model sees it.
    fn page_json(&self, page: usize) -> Value {
        let Some(p) = self.project.pages.get(page) else {
            return Value::Null;
        };
        let symbols: Vec<Value> = p
            .symbols
            .iter()
            .map(|placed| {
                let mut v = serde_json::to_value(placed).unwrap_or_default();
                let sym = self.symbols.get(&placed.symbol);
                v["name"] = json!(sym.map(|s| s.name.clone()).unwrap_or_default());
                v["pins"] = json!(
                    sym.map(|s| {
                        s.pins
                            .iter()
                            .filter_map(|pin| {
                                self.pin_pos(p, &placed.id, &pin.name)
                                    .map(|at| json!({"name": pin.name, "x": at.x, "y": at.y}))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
                );
                v
            })
            .collect();
        let wires: Vec<Value> = p
            .wires
            .iter()
            .map(|w| {
                let mut v = serde_json::to_value(w).unwrap_or_default();
                v["from_at"] = json!(self.end_pos(p, &w.from));
                v["to_at"] = json!(self.end_pos(p, &w.to));
                v
            })
            .collect();
        let graphics: Vec<Value> = p
            .graphics
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let mut v = serde_json::to_value(g).unwrap_or_default();
                v["id"] = json!(format!("g{i}"));
                v
            })
            .collect();
        let comments: Vec<Value> = self
            .project
            .page_comments(&p.id)
            .map(|c| serde_json::to_value(c).unwrap_or_default())
            .collect();
        let (w, h) = p.size.mm();
        json!({
            "id": p.id, "number": p.number, "title": p.title, "size": p.size,
            "size_mm": [w, h],
            "symbols": symbols, "wires": wires, "graphics": graphics,
            "underlay": p.underlay, "comments": comments,
        })
    }

    /// Symbol drawing by id, from the project or the library.
    fn symbol_any(&self, id: &str) -> Option<wc_core::Symbol> {
        self.symbols.get(id).cloned().or_else(|| {
            wc_library::Library::open(&self.dir, &self.project)
                .ok()
                .and_then(|lib| lib.symbol(id).ok().map(|(s, _)| s))
        })
    }

    /// Rename a device everywhere it appears.
    fn rename_device(&mut self, from: &DeviceTag, to: &DeviceTag) -> Result<usize, String> {
        if from == to {
            return Ok(0);
        }
        let used = self.project.devices.contains_key(to) || self.project.tag_index().contains_key(to);
        if used {
            return Err(format!("{to} is already used."));
        }
        self.checkpoint();
        let mut n = 0;
        if let Some(d) = self.project.devices.remove(from) {
            self.project.devices.insert(to.clone(), d);
        }
        for p in &mut self.project.pages {
            for s in &mut p.symbols {
                if s.tag.as_ref() == Some(from) {
                    s.tag = Some(to.clone());
                    n += 1;
                }
            }
            for w in &mut p.wires {
                if let Some(c) = &mut w.cable
                    && &c.cable == from
                {
                    c.cable = to.clone();
                }
            }
        }
        for panel in &mut self.project.panels {
            for item in &mut panel.items {
                if let wc_core::panel::PanelItem::Device { tag, .. } = item
                    && tag == from
                {
                    *tag = to.clone();
                }
            }
        }
        Ok(n)
    }

    /// Make every symbol a page uses available in the project.
    fn ensure_page_symbols(&mut self, page: &Page) -> Result<(), String> {
        let ids: Vec<String> = page.symbols.iter().map(|s| s.symbol.clone()).collect();
        for id in ids {
            self.ensure_symbol(&id).map_err(|e| format!("symbol {id}: {e}"))?;
        }
        Ok(())
    }
}

/// The app as the assistant's toolbox.
pub struct AppTools<'a>(pub &'a mut App);

impl wc_agent::Tools for AppTools<'_> {
    fn specs(&self) -> Vec<Value> {
        self.0.assistant.specs().to_vec()
    }

    fn run(&mut self, name: &str, input: &Value) -> Result<String, String> {
        // The previous tool's change is followed first, so this one sees the jumpers right.
        if let Some(o) = self.0.open.as_mut() {
            o.follow_terminals();
        }
        let r = self.0.agent_tool(name, input);
        // Items the assistant removed must not stay selected.
        self.0.prune_selection();
        r
    }

    fn start(&mut self, name: &str, input: &Value) -> Started {
        // As in `run`: the previous tool's change is followed before this one starts.
        if let Some(o) = self.0.open.as_mut() {
            o.follow_terminals();
        }
        let r = self.0.agent_start(name, input);
        if matches!(r, Started::Done(_)) {
            self.0.prune_selection();
        }
        r
    }

    fn finish(&mut self, name: &str, input: &Value, result: Result<String, String>) -> Result<String, String> {
        let r = self.0.agent_finish(name, input, result);
        self.0.prune_selection();
        r
    }

    fn describe(&self, name: &str, input: &Value) -> String {
        let page = s(input, "page").map(|p| format!(" on page {p}")).unwrap_or_default();
        match name {
            "place_symbol" => format!(
                "Placed {}{}{page}",
                s(input, "symbol").unwrap_or("a symbol"),
                s(input, "tag").map(|t| format!(" as {t}")).unwrap_or_default()
            ),
            "add_wire" => format!("Drew a wire{page}"),
            "update_symbol" | "update_wire" | "update_graphic" => {
                format!("Changed {}{page}", s(input, "id").unwrap_or("an item"))
            }
            "add_graphic" => format!("Drew a {}{page}", input["shape"]["kind"].as_str().unwrap_or("drawing")),
            "delete_items" => format!("Deleted {}{page}", strings(input, "ids").join(", ")),
            "move_items" => format!("Moved {}{page}", strings(input, "ids").join(", ")),
            "rotate_items" => format!("Rotated {}{page}", strings(input, "ids").join(", ")),
            "mirror_items" => format!("Mirrored {}{page}", strings(input, "ids").join(", ")),
            "copy_items" => format!("Copied {}{page}", strings(input, "ids").join(", ")),
            "select_items" => format!("Selected {}{page}", strings(input, "ids").join(", ")),
            "set_device" => format!("Set device {}", s(input, "tag").unwrap_or_default()),
            "run_action" => format!("Ran {}", s(input, "action").unwrap_or_default()),
            "run_command" => format!("wiringcraft {}", strings(input, "args").join(" ")),
            "comments" | "macros" | "panels" | "library" | "versions" => {
                format!("{name}: {}", s(input, "action").unwrap_or_default())
            }
            "export" => format!("Exported {}", s(input, "format").unwrap_or_default()),
            "todo" => match s(input, "action") {
                Some("file") => format!("Put on the todo list: {}", s(input, "title").unwrap_or_default()),
                _ => format!("Searched the todo list for {:?}", s(input, "query").unwrap_or_default()),
            },
            "get_report" => format!("Read the {} report", s(input, "kind").unwrap_or_default()),
            "get_page" => format!("Looked at page {}", s(input, "page").unwrap_or("shown")),
            "find_symbols" | "find_parts" => format!("Searched {:?}", s(input, "query").unwrap_or_default()),
            _ => name.replace('_', " "),
        }
    }
}

impl App {
    /// What the assistant should know about the screen right now, sent with each message.
    pub(crate) fn agent_context(&self) -> String {
        let Some(open) = &self.open else {
            return "No project is open.".into();
        };
        let mut out = format!("Project {:?} in {}", open.project.meta.name, open.dir.display());
        if let Some(pi) = self.ed.panel
            && let Some(p) = open.project.panels.get(pi)
        {
            out.push_str(&format!(". Showing panel layout {} ({})", p.id, p.name));
        } else if let Some(p) = open.project.pages.get(self.ed.page) {
            out.push_str(&format!(". Showing page {} ({}, id {})", p.number, p.title, p.id));
        }
        if !self.ed.selection.is_empty() {
            let ids: Vec<String> = self.ed.selection.iter().map(selection_id).collect();
            out.push_str(&format!(". Selected: {}", ids.join(", ")));
        }
        out.push('.');
        out
    }

    /// Drop selected items that no longer exist.
    pub(crate) fn prune_selection(&mut self) {
        let Some(open) = &self.open else {
            self.ed.selection.clear();
            return;
        };
        if self.ed.page >= open.project.pages.len() {
            self.ed.page = open.project.pages.len().saturating_sub(1);
            self.ed.selection.clear();
        }
        if let Some(pi) = self.ed.panel
            && pi >= open.project.panels.len()
        {
            self.ed.panel = None;
            self.ed.panel_pick = None;
        }
        let Some(p) = open.project.pages.get(self.ed.page) else {
            return;
        };
        self.ed.selection.retain(|s| match s {
            Selection::Symbol(id) => p.symbols.iter().any(|x| &x.id == id),
            Selection::Wire(id) => p.wires.iter().any(|x| &x.id == id),
            Selection::Graphic(i) => *i < p.graphics.len(),
        });
    }

    fn page_of(&self, v: &Value, key: &str) -> Result<usize, String> {
        let open = self
            .open
            .as_ref()
            .ok_or("No project is open. Use open_project first.")?;
        match v.get(key) {
            None | Some(Value::Null) => {
                if open.project.pages.is_empty() {
                    Err("The project has no pages.".into())
                } else {
                    Ok(self.ed.page.min(open.project.pages.len() - 1))
                }
            }
            Some(Value::Number(n)) => open.find_page(&n.to_string()).ok_or_else(|| format!("No page {n}.")),
            Some(Value::String(t)) => open.find_page(t).ok_or_else(|| format!("No page {t:?}.")),
            Some(other) => Err(format!("`{key}` must be a page number, not {other}.")),
        }
    }

    fn panel_of(&self, v: &Value) -> Result<usize, String> {
        let open = self.open.as_ref().ok_or("No project is open.")?;
        match s(v, "panel") {
            Some(key) => open
                .project
                .panels
                .iter()
                .position(|p| p.id == key || p.name == key)
                .ok_or_else(|| format!("No panel layout {key:?}.")),
            None => self
                .ed
                .panel
                .ok_or_else(|| "Say which panel layout (`panel`).".to_owned()),
        }
    }

    /// Show a page on screen.
    fn go_to(&mut self, page: usize) {
        if self.ed.page != page || self.ed.panel.is_some() {
            self.ed.page = page;
            self.ed.panel = None;
            self.ed.panel_pick = None;
            self.ed.selection.clear();
            self.ed.view.fit = true;
        }
    }

    /// Take the message a command left in a dialog, so the model gets it as a result.
    fn take_message(&mut self) -> Option<String> {
        // Log it first: once taken, the frame's own logging never sees it.
        self.log_messages();
        match self.dialog.take() {
            Some(Dialog::Message(m)) => {
                // Closed now, so the same message is logged again the next time it shows.
                self.logged.1.clear();
                Some(m)
            }
            other => {
                self.dialog = other;
                None
            }
        }
    }

    /// Run one assistant tool.
    pub(crate) fn agent_tool(&mut self, name: &str, v: &Value) -> Result<String, String> {
        match name {
            "get_state" => self.tool_state(),
            "get_page" => {
                let page = self.page_of(v, "page")?;
                let open = need_open(self.open.as_mut())?;
                Ok(clip_text(pretty(&open.page_json(page)), 60_000))
            }
            "find_symbols" => self.tool_find_symbols(v),
            "find_parts" => {
                let open = need_open(self.open.as_mut())?;
                let q = s(v, "query").unwrap_or_default().to_lowercase();
                let limit = v.get("limit").and_then(Value::as_u64).unwrap_or(40) as usize;
                let hits: Vec<Value> = open
                    .part_library
                    .iter()
                    .filter(|(id, label)| q.is_empty() || format!("{id} {label}").to_lowercase().contains(&q))
                    .take(limit)
                    .map(|(id, label)| json!({"id": id, "label": label}))
                    .collect();
                Ok(pretty(&json!(hits)))
            }
            "get_devices" => {
                let open = need_open(self.open.as_mut())?;
                let q = s(v, "query").unwrap_or_default().to_lowercase();
                let index = open.project.tag_index();
                let mut tags: std::collections::BTreeSet<DeviceTag> = index.keys().cloned().collect();
                tags.extend(open.project.devices.keys().cloned());
                let list: Vec<Value> = tags
                    .into_iter()
                    .filter_map(|t| {
                        let d = open.project.devices.get(&t).cloned().unwrap_or_default();
                        let hay = format!("{t} {}", d.description).to_lowercase();
                        if !q.is_empty() && !hay.contains(&q) {
                            return None;
                        }
                        let drawn: Vec<Value> = index
                            .get(&t)
                            .into_iter()
                            .flatten()
                            .map(|(pid, sid)| {
                                let number = open.project.page(pid).map(|p| p.number.clone()).unwrap_or_default();
                                json!({"page": number, "symbol": sid})
                            })
                            .collect();
                        Some(json!({
                            "tag": t.to_string(), "description": d.description, "part": d.part,
                            "properties": d.properties, "drawn": drawn
                        }))
                    })
                    .collect();
                Ok(clip_text(pretty(&json!(list)), 60_000))
            }
            "place_symbol" => self.tool_place(v),
            "add_wire" => self.tool_wire(v),
            "update_symbol" => self.tool_update_symbol(v),
            "update_wire" => self.tool_update_wire(v),
            "add_graphic" | "update_graphic" => {
                let page = self.page_of(v, "page")?;
                let shape: Shape = serde_json::from_value(v.get("shape").cloned().unwrap_or_default())
                    .map_err(|e| format!("`shape` is not a drawing: {e}"))?;
                let open = need_open(self.open.as_mut())?;
                if name == "add_graphic" {
                    let sel = open.add_graphic(page, shape).ok_or("No such page.")?;
                    Ok(format!("Added {}.", selection_id(&sel)))
                } else {
                    let id = need_s(v, "id")?;
                    let sel = open.selections(page, &[id.to_owned()])?;
                    let Some(Selection::Graphic(i)) = sel.first() else {
                        return Err(format!("{id} is not a free drawing."));
                    };
                    let i = *i;
                    open.checkpoint();
                    if let Some(g) = open.project.pages.get_mut(page).and_then(|p| p.graphics.get_mut(i)) {
                        *g = shape;
                    }
                    Ok(format!("Changed g{i}."))
                }
            }
            "delete_items" | "move_items" | "rotate_items" | "mirror_items" | "select_items" => {
                let page = self.page_of(v, "page")?;
                let ids = strings(v, "ids");
                let open = need_open(self.open.as_mut())?;
                let sel = open.selections(page, &ids)?;
                match name {
                    "delete_items" => open.delete_many(page, &sel),
                    "move_items" => {
                        let d = Point::new(need_num(v, "dx")?, need_num(v, "dy")?);
                        open.checkpoint();
                        open.move_by(page, &sel, d);
                    }
                    "rotate_items" => open.rotate_many(page, &sel),
                    "mirror_items" => open.mirror_many(page, &sel),
                    _ => {
                        self.go_to(page);
                        self.ed.selection = sel;
                        return Ok(format!("Selected {} on the user's screen.", ids.join(", ")));
                    }
                }
                Ok(format!("Done: {}.", ids.join(", ")))
            }
            "copy_items" => {
                let page = self.page_of(v, "page")?;
                let to = if v.get("to_page").is_some() {
                    self.page_of(v, "to_page")?
                } else {
                    page
                };
                let open = need_open(self.open.as_mut())?;
                let sel = open.selections(page, &strings(v, "ids"))?;
                let clip = open.copy(page, &sel);
                let at = match (num(v, "x"), num(v, "y")) {
                    (Some(x), Some(y)) => Point::new(x, y),
                    _ => clip.anchor + Point::new(10.0, 10.0),
                };
                let new = open.paste(to, &clip, at);
                let ids: Vec<String> = new.iter().map(selection_id).collect();
                Ok(format!("Pasted as {}.", ids.join(", ")))
            }
            "add_page" => {
                let size = page_size(v)?;
                let open = need_open(self.open.as_mut())?;
                let mut i = open.add_page();
                let cur = open.project.pages.get(i).cloned().ok_or("The page was not added.")?;
                let number = s(v, "number").unwrap_or(&cur.number).to_owned();
                let title = s(v, "title").unwrap_or(&cur.title).to_owned();
                i = open.set_page_props(i, &number, &title, size.unwrap_or(cur.size), cur.overview)?;
                let p = open.project.pages.get(i).cloned().ok_or("The page was not added.")?;
                self.go_to(i);
                Ok(format!("Added page {} ({}), id {}.", p.number, p.title, p.id))
            }
            "update_page" => {
                let page = self.page_of(v, "page")?;
                let size = page_size(v)?;
                let open = need_open(self.open.as_mut())?;
                let cur = open.project.pages.get(page).cloned().ok_or("No such page.")?;
                let i = open.set_page_props(
                    page,
                    s(v, "number").unwrap_or(&cur.number),
                    s(v, "title").unwrap_or(&cur.title),
                    size.unwrap_or(cur.size),
                    v.get("overview").and_then(Value::as_bool).unwrap_or(cur.overview),
                )?;
                if self.ed.page == page {
                    self.ed.page = i;
                }
                Ok("Page changed.".into())
            }
            "delete_page" => {
                let page = self.page_of(v, "page")?;
                let open = need_open(self.open.as_mut())?;
                let next = open.delete_page(page)?;
                self.ed.page = next;
                self.ed.selection.clear();
                Ok("Page deleted.".into())
            }
            "duplicate_page" => {
                let page = self.page_of(v, "page")?;
                let open = need_open(self.open.as_mut())?;
                let i = open.duplicate_page(page).ok_or("No such page.")?;
                let n = open.project.pages.get(i).map(|p| p.number.clone()).unwrap_or_default();
                self.go_to(i);
                Ok(format!("Copied as page {n}."))
            }
            "move_page" => {
                let page = self.page_of(v, "page")?;
                let up = need_s(v, "direction")? == "up";
                let open = need_open(self.open.as_mut())?;
                let i = open.move_page(page, up).ok_or("The page is already at that end.")?;
                self.ed.page = i;
                Ok("Page moved.".into())
            }
            "show" => {
                if v.get("panel").is_some() {
                    let pi = self.panel_of(v)?;
                    self.ed.panel = Some(pi);
                    self.ed.panel_pick = None;
                    self.ed.view.fit = true;
                    return Ok("Showing the panel layout.".into());
                }
                let page = self.page_of(v, "page")?;
                self.go_to(page);
                Ok("Showing the page.".into())
            }
            "edit_page_json" => {
                let page = self.page_of(v, "page")?;
                let open = need_open(self.open.as_mut())?;
                let id = open
                    .project
                    .pages
                    .get(page)
                    .map(|p| p.id.clone())
                    .ok_or("No such page.")?;
                let mut content = v.get("content").cloned().ok_or("`content` is missing.")?;
                content["id"] = json!(id);
                let new: Page = serde_json::from_value(content).map_err(|e| format!("Not a page: {e}"))?;
                open.ensure_page_symbols(&new)?;
                open.checkpoint();
                for s in &new.symbols {
                    if let Some(t) = &s.tag {
                        open.project.devices.entry(t.clone()).or_default();
                    }
                }
                if let Some(p) = open.project.pages.get_mut(page) {
                    *p = new;
                }
                open.project.sort_pages();
                let i = open.find_page(&id).unwrap_or(0);
                self.ed.page = i;
                Ok("Page replaced.".into())
            }
            "set_device" => self.tool_set_device(v),
            "set_project" => {
                let open = need_open(self.open.as_mut())?;
                let name = s(v, "name").unwrap_or(&open.project.meta.name).to_owned();
                let mut fields = open.project.meta.title_block.clone();
                if let Some(map) = v.get("title_block").and_then(Value::as_object) {
                    for (k, val) in map {
                        let val = val.as_str().map(str::to_owned).unwrap_or_else(|| val.to_string());
                        fields.insert(k.clone(), val);
                    }
                }
                let fields: Vec<(String, String)> = fields.into_iter().collect();
                open.set_project_props(&name, &fields, None)?;
                Ok("Project properties set.".into())
            }
            "comments" => self.tool_comments(v),
            "macros" => self.tool_macros(v),
            "number_wires" => {
                use wc_core::numbering::Rule;
                let rule = match s(v, "rule").unwrap_or("page_column") {
                    "sequential" => Rule::Sequential,
                    "potential" => Rule::Potential,
                    _ => Rule::PageColumn,
                };
                let open = need_open(self.open.as_mut())?;
                Ok(open.number_wires(rule).report())
            }
            "run_check" => {
                let open = need_open(self.open.as_mut())?;
                let found = crate::run_check(open);
                if found.is_empty() {
                    return Ok("No findings: the project checks clean.".into());
                }
                let lines: Vec<String> = found
                    .iter()
                    .map(|f| {
                        let page = f
                            .page_id
                            .as_deref()
                            .and_then(|id| open.project.page(id))
                            .map(|p| {
                                format!(
                                    " [page {}{}]",
                                    p.number,
                                    f.item.as_deref().map(|i| format!(", {i}")).unwrap_or_default()
                                )
                            })
                            .unwrap_or_default();
                        format!("{}: {}{page}", f.severity.label(), f.message)
                    })
                    .collect();
                Ok(clip_text(lines.join("\n"), 40_000))
            }
            "get_report" => {
                let kind = wc_report::Kind::parse(need_s(v, "kind")?).ok_or("Unknown report.")?;
                let open = need_open(self.open.as_mut())?;
                let inputs = wc_report::Inputs {
                    project: &open.project,
                    symbols: &open.symbols,
                    parts: &open.parts,
                };
                Ok(clip_text(wc_report::build(kind, &inputs).to_markdown(), 60_000))
            }
            "panels" => self.tool_panels(v),
            "library" => {
                let open = need_open(self.open.as_mut())?;
                let lib = wc_library::Library::open(&open.dir, &open.project).map_err(|e| e.to_string())?;
                if need_s(v, "action")? == "update" {
                    let ids = strings(v, "ids");
                    let only = (!ids.is_empty()).then_some(ids.as_slice());
                    open.checkpoint();
                    let done =
                        wc_library::update(&open.dir, &mut open.project, &lib, only).map_err(|e| e.to_string())?;
                    let (syms, _) = wc_core::store::vendored_symbols(&open.dir, &open.project);
                    open.symbols = syms;
                    open.refresh_library();
                    let ids: Vec<String> = done.iter().map(|(_, id)| id.clone()).collect();
                    return Ok(format!("Updated {} item(s): {}", ids.len(), ids.join(", ")));
                }
                let lines: Vec<String> = wc_library::status(&open.dir, &open.project, &lib)
                    .into_iter()
                    .map(|st| {
                        format!(
                            "{:?} {} {:?} used by {}{}",
                            st.kind,
                            st.id,
                            st.state,
                            st.used_by.join(", "),
                            if st.changes.is_empty() {
                                String::new()
                            } else {
                                format!(" — {}", st.changes.join("; "))
                            }
                        )
                    })
                    .collect();
                Ok(clip_text(lines.join("\n"), 40_000))
            }
            "export" => {
                let open = need_open(self.open.as_mut())?;
                Ok(match need_s(v, "format")? {
                    "pdf" => crate::export_pdf(open),
                    "drawings" => crate::export_drawings(open),
                    "aml" => crate::export_aml(open),
                    f => return Err(format!("Unknown format {f}.")),
                })
            }
            "versions" => self.tool_versions(v),
            "undo" => {
                // Undo swaps the whole project, which would take back other people's work too.
                if self.share.active() {
                    return Err("Undo and redo are off in a shared session; use the page history.".into());
                }
                let open = need_open(self.open.as_mut())?;
                let redo = v.get("redo").and_then(Value::as_bool).unwrap_or(false);
                let done = if redo { open.redo() } else { open.undo() };
                self.ed.selection.clear();
                Ok(if done { "Done." } else { "Nothing to undo or redo." }.into())
            }
            "open_project" => {
                let path = PathBuf::from(need_s(v, "path")?);
                if v.get("create").and_then(Value::as_bool).unwrap_or(false) {
                    self.new_project(&path, s(v, "name").unwrap_or_default());
                } else {
                    let mut prefs = std::mem::take(&mut self.prefs);
                    self.open_dir(&path, &mut prefs);
                    self.prefs = prefs;
                }
                if let Some(m) = self.take_message() {
                    return Err(m);
                }
                match &self.open {
                    Some(o) if o.dir == path => Ok(format!("Opened {}.", o.project.meta.name)),
                    _ => Err("The project was not opened.".into()),
                }
            }
            "run_action" => {
                let want = need_s(v, "action")?;
                let a = Action::ALL
                    .iter()
                    .copied()
                    .find(|a| a.name().eq_ignore_ascii_case(want))
                    .ok_or_else(|| format!("No command {want:?}."))?;
                if a == Action::Assistant {
                    return Err("That would hide this panel.".into());
                }
                let before = self.dialog.is_some();
                self.run(a);
                if let Some(m) = self.take_message() {
                    return Ok(m);
                }
                if !before && self.dialog.is_some() {
                    return Ok(format!(
                        "{} opened a dialog on the user's screen for them to fill in.",
                        a.info().label
                    ));
                }
                let msg = std::mem::take(&mut self.ed.message);
                self.ed.message = msg.clone();
                Ok(if msg.is_empty() {
                    format!("Ran {}.", a.info().label)
                } else {
                    msg
                })
            }
            "run_command" | "todo" => match self.agent_start(name, v) {
                Started::Done(r) => r,
                Started::Running(p) => {
                    let r = p.wait();
                    self.agent_finish(name, v, r)
                }
            },
            other => Err(format!("Unknown tool {other}.")),
        }
    }

    fn tool_state(&self) -> Result<String, String> {
        let Some(open) = &self.open else {
            return Ok(pretty(&json!({
                "open": false,
                "recent_projects": self.prefs.recent,
            })));
        };
        let pages: Vec<Value> = open
            .project
            .pages
            .iter()
            .map(|p| {
                json!({
                    "id": p.id, "number": p.number, "title": p.title, "size": p.size,
                    "symbols": p.symbols.len(), "wires": p.wires.len(), "graphics": p.graphics.len()
                })
            })
            .collect();
        let panels: Vec<Value> = open
            .project
            .panels
            .iter()
            .map(|p| json!({"id": p.id, "name": p.name, "width": p.width, "height": p.height, "items": p.items.len()}))
            .collect();
        let shown = match self.ed.panel {
            Some(pi) => json!({"panel": open.project.panels.get(pi).map(|p| p.id.clone())}),
            None => json!({"page": open.project.pages.get(self.ed.page).map(|p| p.number.clone())}),
        };
        Ok(pretty(&json!({
            "open": true,
            "name": open.project.meta.name,
            "folder": open.dir,
            "branch": open.branch,
            "unsaved_changes": open.dirty,
            "title_block": open.project.meta.title_block,
            "libraries": open.project.meta.libraries.iter().map(|l| format!("{} ({}: {})", l.name, l.kind, l.location)).collect::<Vec<_>>(),
            "pages": pages,
            "panels": panels,
            "devices": open.project.devices.len(),
            "open_comments": open.project.comments.iter().filter(|c| !c.resolved).count(),
            "shown": shown,
            "selection": self.ed.selection.iter().map(selection_id).collect::<Vec<_>>(),
            "can_undo": open.can_undo(),
        })))
    }

    fn tool_find_symbols(&mut self, v: &Value) -> Result<String, String> {
        let open = need_open(self.open.as_mut())?;
        let q = s(v, "query").unwrap_or_default().to_lowercase();
        let limit = v.get("limit").and_then(Value::as_u64).unwrap_or(30) as usize;
        let ids: Vec<String> = open.library.iter().map(|(id, _)| id.clone()).collect();
        let lib = wc_library::Library::open(&open.dir, &open.project).ok();
        let mut out = Vec::new();
        for id in ids {
            let sym = open
                .symbols
                .get(&id)
                .cloned()
                .or_else(|| lib.as_ref().and_then(|l| l.symbol(&id).ok().map(|(s, _)| s)));
            let Some(sym) = sym else { continue };
            let hay = format!("{} {} {} {}", sym.id, sym.name, sym.category, sym.description).to_lowercase();
            if !q.is_empty() && !q.split_whitespace().all(|w| hay.contains(w)) {
                continue;
            }
            out.push(json!({
                "id": sym.id, "name": sym.name, "category": sym.category,
                "letter_code": sym.letter_code, "role": sym.role,
                "description": sym.description,
                "pins": sym.pins.iter().map(|p| json!({"name": p.name, "x": p.at.x, "y": p.at.y, "dir": p.dir})).collect::<Vec<_>>(),
            }));
            if out.len() >= limit {
                break;
            }
        }
        if out.is_empty() {
            return Ok("No symbols match. Try fewer or other words, or an empty query to list all.".into());
        }
        Ok(clip_text(pretty(&json!(out)), 60_000))
    }

    fn tool_place(&mut self, v: &Value) -> Result<String, String> {
        let page = self.page_of(v, "page")?;
        let symbol = need_s(v, "symbol")?.to_owned();
        let at = Point::new(need_num(v, "x")?, need_num(v, "y")?);
        let rot = rotation(v)?;
        let wanted = s(v, "tag").filter(|t| !t.trim().is_empty()).map(tag).transpose()?;
        let open = need_open(self.open.as_mut())?;
        if open.symbol_any(&symbol).is_none() {
            return Err(format!(
                "No symbol {symbol:?} in the project or its library. Use find_symbols."
            ));
        }
        let id = open.place(page, &symbol, at)?;
        let auto = open
            .project
            .pages
            .get(page)
            .and_then(|p| p.symbol(&id))
            .and_then(|s| s.tag.clone());
        if let Some(t) = &wanted
            && Some(t) != auto.as_ref()
        {
            // The automatic tag was not used after all.
            if let Some(a) = &auto
                && open.project.tag_index().get(a).is_none_or(|v| v.len() <= 1)
                && open
                    .project
                    .devices
                    .get(a)
                    .is_some_and(|d| *d == wc_core::Device::default())
            {
                open.project.devices.remove(a);
            }
            open.project.devices.entry(t.clone()).or_default();
        }
        if let Some(s) = open
            .project
            .pages
            .get_mut(page)
            .and_then(|p| p.symbols.iter_mut().find(|s| s.id == id))
        {
            if let Some(r) = rot {
                s.rotation = r;
            }
            if let Some(m) = v.get("mirror").and_then(Value::as_bool) {
                s.mirror = m;
            }
            if let Some(t) = &wanted {
                s.tag = Some(t.clone());
            }
        }
        let final_tag = open
            .project
            .pages
            .get(page)
            .and_then(|p| p.symbol(&id))
            .and_then(|s| s.tag.clone());
        if let (Some(t), Some(d)) = (&final_tag, s(v, "description")) {
            open.project.devices.entry(t.clone()).or_default().description = d.trim().to_owned();
        }
        let pg = open.project.pages.get(page).ok_or("No such page.")?;
        let pins: Vec<Value> = open
            .symbols
            .get(&symbol)
            .map(|sym| {
                sym.pins
                    .iter()
                    .filter_map(|p| {
                        open.pin_pos(pg, &id, &p.name)
                            .map(|at| json!({"name": p.name, "x": at.x, "y": at.y}))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(pretty(&json!({
            "id": id,
            "tag": final_tag.map(|t| t.to_string()),
            "page": pg.number,
            "pins": pins,
        })))
    }

    fn tool_wire(&mut self, v: &Value) -> Result<String, String> {
        let page = self.page_of(v, "page")?;
        let via = points(v, "via")?.unwrap_or_default();
        let open = need_open(self.open.as_mut())?;
        let from = open.endpoint(page, v.get("from"))?;
        let to = open.endpoint(page, v.get("to"))?;
        if from == to {
            return Err("Both ends are the same.".into());
        }
        open.add_wire(page, from, to, via);
        let p = open.project.pages.get_mut(page).ok_or("No such page.")?;
        let w = p.wires.last_mut().ok_or("The wire was not added.")?;
        if let Some(n) = s(v, "number").filter(|n| !n.trim().is_empty()) {
            w.number = Some(n.trim().to_owned());
        }
        if let Some(n) = s(v, "potential").filter(|n| !n.trim().is_empty()) {
            w.potential = Some(n.trim().to_owned());
        }
        Ok(format!("Added wire {}.", w.id))
    }

    fn tool_update_symbol(&mut self, v: &Value) -> Result<String, String> {
        let page = self.page_of(v, "page")?;
        let id = need_s(v, "id")?.to_owned();
        let rot = rotation(v)?;
        let new_tag = match s(v, "tag") {
            None => None,
            Some(t) if t.trim().is_empty() => Some(None),
            Some(t) => Some(Some(tag(t)?)),
        };
        let open = need_open(self.open.as_mut())?;
        let cur = open
            .project
            .pages
            .get(page)
            .and_then(|p| p.symbol(&id))
            .cloned()
            .ok_or_else(|| format!("No symbol {id:?} on that page."))?;
        if let Some(sym) = s(v, "symbol") {
            open.ensure_symbol(sym)?;
        }
        open.checkpoint();
        if let Some(Some(t)) = &new_tag {
            open.project.devices.entry(t.clone()).or_default();
        }
        let p = open.project.pages.get_mut(page).ok_or("No such page.")?;
        let placed = p.symbols.iter_mut().find(|x| x.id == id).ok_or("No such symbol.")?;
        placed.at = Point::new(num(v, "x").unwrap_or(cur.at.x), num(v, "y").unwrap_or(cur.at.y));
        if let Some(r) = rot {
            placed.rotation = r;
        }
        if let Some(m) = v.get("mirror").and_then(Value::as_bool) {
            placed.mirror = m;
        }
        if let Some(t) = new_tag {
            placed.tag = t;
        }
        if let Some(sym) = s(v, "symbol") {
            placed.symbol = sym.to_owned();
        }
        Ok(format!("Changed {id}."))
    }

    fn tool_update_wire(&mut self, v: &Value) -> Result<String, String> {
        let page = self.page_of(v, "page")?;
        let id = need_s(v, "id")?.to_owned();
        let via = points(v, "via")?;
        let open = need_open(self.open.as_mut())?;
        let from = v.get("from").map(|e| open.endpoint(page, Some(e))).transpose()?;
        let to = v.get("to").map(|e| open.endpoint(page, Some(e))).transpose()?;
        if !open
            .project
            .pages
            .get(page)
            .is_some_and(|p| p.wires.iter().any(|w| w.id == id))
        {
            return Err(format!("No wire {id:?} on that page."));
        }
        // Check the cable tag before changing anything, so a bad one leaves the wire as it was.
        let cable_tag = match s(v, "cable") {
            Some(c) if !c.trim().is_empty() => Some(tag(c)?),
            _ => None,
        };
        open.checkpoint();
        if let Some(w) = open
            .project
            .pages
            .get_mut(page)
            .and_then(|p| p.wires.iter_mut().find(|w| w.id == id))
        {
            if let Some(e) = from {
                w.from = e;
            }
            if let Some(e) = to {
                w.to = e;
            }
            if let Some(via) = via {
                w.via = via;
            }
            if let Some(n) = s(v, "number") {
                w.number = Some(n.trim().to_owned()).filter(|n| !n.is_empty());
            }
            if let Some(n) = s(v, "potential") {
                w.potential = Some(n.trim().to_owned()).filter(|n| !n.is_empty());
            }
            if let Some(x) = s(v, "section") {
                w.spec.section = wc_core::WireSpec::value(x);
            }
            if let Some(x) = s(v, "colour") {
                w.spec.colour = wc_core::WireSpec::value(x);
            }
            if let Some(x) = s(v, "type") {
                w.spec.kind = wc_core::WireSpec::value(x);
            }
        }
        if s(v, "cable").is_some() {
            let cable = cable_tag.map(|cable| wc_core::CableCore {
                cable,
                core: s(v, "core").unwrap_or_default().trim().to_owned(),
            });
            open.set_wire_cable(page, &id, cable);
        } else if let Some(core) = s(v, "core")
            && let Some(mut cc) = open
                .project
                .pages
                .get(page)
                .and_then(|p| p.wires.iter().find(|w| w.id == id))
                .and_then(|w| w.cable.clone())
        {
            cc.core = core.trim().to_owned();
            open.set_wire_cable(page, &id, Some(cc));
        }
        Ok(format!("Changed {id}."))
    }

    fn tool_set_device(&mut self, v: &Value) -> Result<String, String> {
        let mut t = tag(need_s(v, "tag")?)?;
        let open = need_open(self.open.as_mut())?;
        let mut done = Vec::new();
        if let Some(to) = s(v, "rename_to") {
            let to = tag(to)?;
            let n = open.rename_device(&t, &to)?;
            done.push(format!("renamed to {to} ({n} symbol(s))"));
            t = to;
        }
        if let Some(d) = s(v, "description") {
            let cur = open
                .project
                .devices
                .get(&t)
                .map(|d| d.description.clone())
                .unwrap_or_default();
            if cur != d.trim() {
                open.checkpoint();
                open.project.devices.entry(t.clone()).or_default().description = d.trim().to_owned();
                done.push("description set".into());
            }
        }
        if let Some(p) = s(v, "part") {
            let p = p.trim();
            open.set_device_part(&t, (!p.is_empty()).then_some(p))?;
            done.push(if p.is_empty() {
                "part removed".into()
            } else {
                format!("part {p}")
            });
        }
        if let Some(map) = v.get("properties").and_then(Value::as_object) {
            for (k, val) in map {
                let val = val.as_str().map(str::to_owned).unwrap_or_else(|| val.to_string());
                open.set_device_property(&t, k, &val);
            }
            done.push(format!("{} propert(ies)", map.len()));
        }
        if done.is_empty() {
            open.checkpoint();
            open.project.devices.entry(t.clone()).or_default();
            done.push("created".into());
        }
        Ok(format!("{t}: {}.", done.join(", ")))
    }

    fn tool_comments(&mut self, v: &Value) -> Result<String, String> {
        let action = need_s(v, "action")?;
        let page = if v.get("page").is_some() || action == "add" {
            Some(self.page_of(v, "page")?)
        } else {
            None
        };
        let open = need_open(self.open.as_mut())?;
        let who = format!("{} (assistant)", crate::state::author_name());
        let now = crate::state::now_secs();
        let err = |e: wc_core::Error| e.to_string();
        match action {
            "list" => {
                let all = v.get("all").and_then(Value::as_bool).unwrap_or(false);
                let page_id = page.and_then(|i| open.project.pages.get(i)).map(|p| p.id.clone());
                let list: Vec<Value> = open
                    .project
                    .comments
                    .iter()
                    .filter(|c| all || !c.resolved)
                    .filter(|c| page_id.as_ref().is_none_or(|p| &c.page == p))
                    .map(|c| serde_json::to_value(c).unwrap_or_default())
                    .collect();
                Ok(clip_text(pretty(&json!(list)), 40_000))
            }
            "add" => {
                let page_id = page
                    .and_then(|i| open.project.pages.get(i))
                    .map(|p| p.id.clone())
                    .ok_or("No such page.")?;
                let at = match (num(v, "x"), num(v, "y")) {
                    (Some(x), Some(y)) => Some(Point::new(x, y)),
                    _ => None,
                };
                let mut project = open.project.clone();
                let id = project
                    .add_comment(&page_id, at, &who, now, need_s(v, "text")?)
                    .map_err(err)?;
                open.checkpoint();
                open.project = project;
                Ok(format!("Added comment {id}."))
            }
            "reply" | "resolve" | "reopen" | "delete" => {
                let id = need_s(v, "id")?;
                let mut project = open.project.clone();
                match action {
                    "reply" => project
                        .reply_to_comment(id, &who, now, need_s(v, "text")?)
                        .map_err(err)?,
                    "resolve" => project.resolve_comment(id, true).map_err(err)?,
                    "reopen" => project.resolve_comment(id, false).map_err(err)?,
                    _ => project.remove_comment(id).map(|_| ()).map_err(err)?,
                }
                open.checkpoint();
                open.project = project;
                Ok(format!("Comment {id}: {action} done."))
            }
            other => Err(format!("Unknown comments action {other}.")),
        }
    }

    fn tool_macros(&mut self, v: &Value) -> Result<String, String> {
        let action = need_s(v, "action")?;
        let page = self.page_of(v, "page")?;
        let open = need_open(self.open.as_mut())?;
        match action {
            "list" => {
                let list: Vec<Value> = open
                    .macros()
                    .into_iter()
                    .map(
                        |m| json!({"name": m.name, "description": m.description, "source": m.source, "error": m.error}),
                    )
                    .collect();
                Ok(pretty(&json!(list)))
            }
            "insert" => {
                let name = need_s(v, "name")?;
                let entry = open
                    .macros()
                    .into_iter()
                    .find(|m| m.name.eq_ignore_ascii_case(name) || stem(&m.path).eq_ignore_ascii_case(name))
                    .ok_or_else(|| format!("No macro {name:?}."))?;
                let anchor = wc_core::macros::read_macro(&entry.path)
                    .map_err(|e| e.to_string())?
                    .anchor;
                let items = open.insert_macro(page, &entry.path)?;
                if let (Some(x), Some(y)) = (num(v, "x"), num(v, "y")) {
                    open.move_by(page, &items, Point::new(x - anchor.x, y - anchor.y));
                }
                let ids: Vec<String> = items.iter().map(selection_id).collect();
                Ok(format!("Inserted {} as {}.", entry.name, ids.join(", ")))
            }
            "save" => {
                let sel = open.selections(page, &strings(v, "ids"))?;
                let clip = open.copy(page, &sel);
                let path = open.save_macro(&clip, need_s(v, "name")?, s(v, "description").unwrap_or_default())?;
                Ok(format!("Saved the macro to {}.", path.display()))
            }
            other => Err(format!("Unknown macros action {other}.")),
        }
    }

    fn tool_panels(&mut self, v: &Value) -> Result<String, String> {
        let action = need_s(v, "action")?;
        if action == "new" {
            let open = need_open(self.open.as_mut())?;
            let n = (1..)
                .find(|n| !open.project.panels.iter().any(|p| p.id == format!("panel{n}")))
                .unwrap_or(1);
            let id = format!("panel{n}");
            let name = s(v, "name").map(str::to_owned).unwrap_or_else(|| format!("Panel {n}"));
            let panel = wc_core::panel::Panel::with_rails(
                id.clone(),
                name,
                num(v, "width").unwrap_or(600.0),
                num(v, "height").unwrap_or(800.0),
            );
            open.checkpoint();
            open.project.panels.push(panel);
            open.project.panels.sort_by(|a, b| a.id.cmp(&b.id));
            self.ed.panel = open.project.panels.iter().position(|p| p.id == id);
            self.ed.panel_pick = None;
            self.ed.view.fit = true;
            return Ok(format!("Added panel layout {id} and showed it."));
        }
        let pi = self.panel_of(v)?;
        let open = need_open(self.open.as_mut())?;
        match action {
            "get" => Ok(clip_text(
                pretty(&serde_json::to_value(&open.project.panels[pi]).unwrap_or_default()),
                60_000,
            )),
            "arrange" => {
                open.checkpoint();
                let mut panels = std::mem::take(&mut open.project.panels);
                let done = panels
                    .get_mut(pi)
                    .map(|p| wc_core::panel::arrange(p, &open.project, &open.parts, s(v, "location")))
                    .unwrap_or_default();
                open.project.panels = panels;
                Ok(format!(
                    "Placed {} device(s). Sizes estimated for: {}. Did not fit: {}.",
                    done.placed.len(),
                    if done.estimated.is_empty() {
                        "none".into()
                    } else {
                        done.estimated.join(", ")
                    },
                    if done.did_not_fit.is_empty() {
                        "none".into()
                    } else {
                        done.did_not_fit.join(", ")
                    }
                ))
            }
            "delete" => {
                open.checkpoint();
                open.project.panels.remove(pi);
                self.ed.panel = None;
                self.ed.panel_pick = None;
                Ok("Panel layout deleted.".into())
            }
            "replace" => {
                let id = open.project.panels[pi].id.clone();
                let mut content = v.get("content").cloned().ok_or("`content` is missing.")?;
                content["id"] = json!(id);
                let new: wc_core::panel::Panel =
                    serde_json::from_value(content).map_err(|e| format!("Not a panel layout: {e}"))?;
                open.checkpoint();
                open.project.panels[pi] = new;
                self.ed.panel_pick = None;
                Ok("Panel layout replaced.".into())
            }
            other => Err(format!("Unknown panels action {other}.")),
        }
    }

    fn tool_versions(&mut self, v: &Value) -> Result<String, String> {
        let action = need_s(v, "action")?;
        let page = if action == "page_history" {
            Some(self.page_of(v, "page")?)
        } else {
            None
        };
        let open = need_open(self.open.as_mut())?;
        let git = wc_vcs::Git::new(&open.dir);
        match action {
            "save" => {
                let n = open.save()?;
                Ok(format!("Saved ({n} files changed)."))
            }
            "commit" => {
                let msg = need_s(v, "message")?;
                open.save()?;
                let res = if git.is_repo() { Ok(()) } else { git.init() }.and_then(|()| git.commit(msg));
                open.branch = git.current_branch().unwrap_or_default();
                match res.map_err(|e| e.to_string())? {
                    Some(id) => Ok(format!("Recorded version {}.", id.get(..8).unwrap_or(&id))),
                    None => Ok("Nothing changed since the last version.".into()),
                }
            }
            "branch" => {
                let name = need_s(v, "name")?;
                git.create_branch(name)
                    .and_then(|()| git.switch(name))
                    .map_err(|e| e.to_string())?;
                open.branch = name.to_owned();
                Ok(format!("Now on branch {name}."))
            }
            "history" => {
                let rows = wc_vcs::history(&git, 60).map_err(|e| e.to_string())?;
                Ok(clip_text(wc_vcs::subway_text(&rows), 30_000))
            }
            "page_history" => {
                let pg = page.and_then(|i| open.project.pages.get(i)).ok_or("No such page.")?;
                let versions = wc_vcs::page_history(&git, &pg.id, 30).map_err(|e| e.to_string())?;
                let lines: Vec<String> = versions
                    .iter()
                    .map(|pv| {
                        let changes: Vec<String> = pv.changes.iter().map(ToString::to_string).collect();
                        format!(
                            "{} {} {}: {}\n  {}",
                            pv.commit.get(..8).unwrap_or(&pv.commit),
                            wc_core::review::utc_date(pv.time),
                            pv.author,
                            pv.subject,
                            changes.join("\n  ")
                        )
                    })
                    .collect();
                Ok(clip_text(lines.join("\n"), 30_000))
            }
            other => Err(format!("Unknown versions action {other}.")),
        }
    }

    /// Start a tool. Tools that wait on another program (`run_command`, and the `todo` tool's
    /// `gh` calls) run on a worker thread, so the window keeps drawing; [`App::agent_finish`]
    /// applies their result on the window thread. Every other tool runs here and now.
    pub(crate) fn agent_start(&mut self, name: &str, v: &Value) -> Started {
        match name {
            "run_command" => match self.command_start(v) {
                Ok(p) => Started::Running(p),
                Err(e) => Started::Done(Err(e)),
            },
            "todo" => self.todo_start(v),
            _ => Started::Done(self.agent_tool(name, v)),
        }
    }

    /// Complete a tool that [`App::agent_start`] sent to a worker thread.
    pub(crate) fn agent_finish(&mut self, name: &str, v: &Value, r: Result<String, String>) -> Result<String, String> {
        match name {
            "run_command" => self.command_finish(r),
            "todo" => self.todo_finish(v, r),
            _ => r,
        }
    }

    /// Search the todo list, or put a request on it. The `gh` calls run on a worker thread.
    fn todo_start(&mut self, v: &Value) -> Started {
        use crate::feedback;
        let action = match need_s(v, "action") {
            Ok(a) => a,
            Err(e) => return Started::Done(Err(e)),
        };
        match action {
            "search" => match need_s(v, "query") {
                Ok(q) => {
                    let q = q.to_owned();
                    Started::Running(Pending::spawn(move || feedback::search(&q)))
                }
                Err(e) => Started::Done(Err(e)),
            },
            "file" => {
                let (kind, r, confirmed) = match todo_request(v) {
                    Ok(x) => x,
                    Err(e) => return Started::Done(Err(e)),
                };
                // gh cannot attach pictures, so with a screenshot the form opens and the latest
                // screenshot goes on the clipboard, ready to paste into it.
                if let Some(shot) = self.assistant.shared.last().cloned() {
                    self.open_url.push(feedback::form_url(kind, &r));
                    let pasted = self.assistant.clipboard.copy(&shot);
                    self.ed.message = match &pasted {
                        Ok(()) => {
                            "Opened the request on GitHub; press Ctrl+V in its Screenshots box to add the screenshot."
                                .into()
                        }
                        Err(_) => "Opened the request on GitHub; drag the screenshot into its Screenshots box.".into(),
                    };
                    return Started::Done(Ok(match pasted {
                        Ok(()) => "Opened the filled-in form in the user's browser and put the latest screenshot on their clipboard. Tell them to click the form's Screenshots box, press Ctrl+V (Cmd+V on a Mac), then Submit.".into(),
                        Err(e) => format!("Opened the filled-in form in the user's browser. The screenshot could not go on the clipboard ({e}); tell them to drag their screenshot file into the form's Screenshots box, then Submit."),
                    }));
                }
                if confirmed {
                    return Started::Running(Pending::spawn(move || {
                        if feedback::gh_ready() {
                            feedback::file(kind, &r)
                        } else {
                            Ok(GH_NOT_READY.to_owned())
                        }
                    }));
                }
                Started::Done(self.todo_open_form(kind, &r))
            }
            other => Started::Done(Err(format!("Unknown action {other:?} (search, file)."))),
        }
    }

    /// The `todo` tool's `gh` call is done: note a filed request, or open the form when `gh`
    /// could not be used.
    fn todo_finish(&mut self, v: &Value, r: Result<String, String>) -> Result<String, String> {
        if s(v, "action") != Some("file") {
            return r;
        }
        let url = r?;
        if url == GH_NOT_READY {
            let (kind, req, _) = todo_request(v)?;
            return self.todo_open_form(kind, &req);
        }
        self.ed.message = format!("Filed {url}");
        Ok(format!("Filed on the todo list: {url}"))
    }

    fn todo_open_form(&mut self, kind: crate::feedback::Kind, r: &crate::feedback::Request) -> Result<String, String> {
        self.open_url.push(crate::feedback::form_url(kind, r));
        self.ed.message = "Opened a filled-in request on GitHub in your browser.".into();
        Ok("Opened the filled-in form in the user's browser; it is on the todo list once they press Submit.".into())
    }

    /// Save the project, then start the `wiringcraft` command on a worker thread.
    fn command_start(&mut self, v: &Value) -> Result<Pending, String> {
        let args = strings(v, "args");
        if args.is_empty() {
            return Err("`args` is empty.".into());
        }
        let bin = cli_path();
        let mut cmd = std::process::Command::new(&bin);
        self.assistant.command_base = None;
        if let Some(open) = self.open.as_mut() {
            if open.dirty {
                open.save()
                    .map_err(|e| format!("Saving before the command failed: {e}"))?;
            }
            cmd.arg("-C").arg(&open.dir);
            self.assistant.command_base = Some((open.dir.clone(), open.project.clone()));
        }
        cmd.args(&args);
        Ok(Pending::spawn(move || {
            let out = cmd
                .output()
                .map_err(|e| format!("Could not run {}: {e}", bin.display()))?;
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.trim().is_empty() {
                text.push_str(&format!("\n[stderr]\n{err}"));
            }
            let text = clip_text(text, 30_000);
            if out.status.success() {
                Ok(if text.trim().is_empty() { "Done.".into() } else { text })
            } else {
                Err(format!("The command failed ({}):\n{text}", out.status))
            }
        }))
    }

    /// The command is done: read what it changed. The window does not save while a command
    /// runs, so a change on disk is the command's. Edits the user made in the window meanwhile
    /// always win: if both changed the project, the command's changes are not loaded and the
    /// answer says so, so the assistant can ask before running it again.
    fn command_finish(&mut self, r: Result<String, String>) -> Result<String, String> {
        let base = self.assistant.command_base.take();
        let Some(open) = self.open.as_mut() else {
            return r;
        };
        let Some((dir, base)) = base else {
            return r;
        };
        if open.dir != dir {
            // Another project was opened meanwhile; the command worked on the old one.
            return r;
        }
        let fresh = Open::load(&open.dir)?;
        // Symbols the command added or changed come in; symbols the window vendored meanwhile
        // (not saved yet, so not on disk) stay.
        // Pending window changes follow their terminals under the roles they were made with.
        open.follow_terminals();
        open.symbols.extend(fresh.symbols);
        open.branch = fresh.branch;
        let by_command = fresh.project != base;
        let by_window = open.project != base;
        let mut note = None;
        if by_command && by_window {
            note = Some(
                "The project was edited in the window while the command ran, so the command's changes were \
                 not loaded: the window's edits are kept, and saving writes them over the command's result.",
            );
        } else if by_command {
            // The roles may have changed with the command: nothing is left to follow for the
            // undo snapshot, which keeps the drawing and its jumpers as they were.
            open.forget_terminal_change();
            open.checkpoint();
            open.project = fresh.project;
            open.forget_terminal_change();
            open.dirty = false;
        }
        open.refresh_library();
        self.prune_selection();
        match note {
            Some(n) => {
                self.ed.message = n.into();
                Err(match r {
                    Ok(t) | Err(t) => format!("{t}\n\n{n} Ask the user before running it again."),
                })
            }
            None => r,
        }
    }
}

/// What the worker answers when `gh` cannot be used, so the form opens instead.
const GH_NOT_READY: &str = "\u{0}gh not ready";

/// The `todo` tool's request to file: its kind, text, and whether the user confirmed it.
fn todo_request(v: &Value) -> Result<(crate::feedback::Kind, crate::feedback::Request, bool), String> {
    use crate::feedback::{Kind, Request};
    let kind = Kind::parse(need_s(v, "kind")?).ok_or("`kind` is problem, function or missing.")?;
    let r = Request {
        title: need_s(v, "title")?.trim().to_owned(),
        what: need_s(v, "what")?.trim().to_owned(),
        detail: s(v, "detail").unwrap_or_default().trim().to_owned(),
        done: s(v, "done").unwrap_or_default().trim().to_owned(),
    };
    if r.title.is_empty() || r.what.is_empty() {
        return Err("`title` and `what` must not be empty.".into());
    }
    Ok((kind, r, v["confirmed"].as_bool().unwrap_or(false)))
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().trim_end_matches(".macro").to_owned())
        .unwrap_or_default()
}

/// The `wiringcraft` command-line program: `WIRINGCRAFT_CLI`, then next to this program, then
/// on the PATH.
fn cli_path() -> PathBuf {
    if let Some(p) = std::env::var_os("WIRINGCRAFT_CLI") {
        return PathBuf::from(p);
    }
    let name = format!("wiringcraft{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(&name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Plays back model answers in order.
    struct Script(Mutex<Vec<Value>>);

    impl wc_agent::Transport for Script {
        fn send(&self, _key: &str, _body: &Value) -> Result<Value, String> {
            let mut v = self.0.lock().map_err(|e| e.to_string())?;
            if v.is_empty() {
                return Err("script ran out".into());
            }
            Ok(v.remove(0))
        }
    }

    fn app(answers: Vec<Value>) -> (tempfile::TempDir, App) {
        let (tmp, open) = crate::state::tests::project();
        let mut prefs = crate::prefs::Prefs::default();
        prefs.assistant.api_key = "test-key".into();
        let app = App {
            prefs,
            open: Some(open),
            ed: crate::canvas::Editor::default(),
            chrome: Default::default(),
            panels: Default::default(),
            dialog: None,
            clipboard: Default::default(),
            copied_text: None,
            applied_theme: crate::theme::Mode::Light,
            assistant: Assistant::with_transport(Arc::new(Script(Mutex::new(answers)))),
            open_url: Vec::new(),
            logged: Default::default(),
            share: Default::default(),
        };
        (tmp, app)
    }

    fn call(app: &mut App, name: &str, input: Value) -> Result<String, String> {
        wc_agent::Tools::run(&mut AppTools(app), name, &input)
    }

    fn page2(app: &App) -> &Page {
        app.open.as_ref().unwrap().project.page("p0002").unwrap()
    }

    #[test]
    fn a_message_box_is_logged_again_after_it_closed() {
        let (_tmp, mut a) = app(vec![]);
        let fail = || Some(Dialog::Message("Saving failed:\ndisk full".into()));
        let boxes = |a: &mut App| {
            a.new_messages()
                .into_iter()
                .filter(|(l, _)| *l == crate::applog::Level::Warn)
                .count()
        };
        a.dialog = fail();
        assert_eq!(boxes(&mut a), 1);
        assert_eq!(boxes(&mut a), 0, "once while it is open");
        a.dialog = None;
        assert_eq!(boxes(&mut a), 0);
        // A save that worked, then the same failure again.
        a.ed.message = "Saved".into();
        a.new_messages();
        a.dialog = fail();
        assert_eq!(boxes(&mut a), 1, "the second failure is logged too");
    }

    #[test]
    fn the_report_gets_the_log_only_when_chosen() {
        let (request, copy) = crate::report_request("line one\nline two\n", false);
        assert!(request.detail.is_empty() && copy.is_none());
        let (request, copy) = crate::report_request("", true);
        assert!(request.detail.is_empty() && copy.is_none(), "no empty log");
        let (request, copy) = crate::report_request("line one\nline two\n", true);
        assert!(request.detail.contains("line two"));
        assert_eq!(copy.as_deref(), Some("line one\nline two\n"));
    }

    #[test]
    fn a_taken_message_was_logged_first() {
        let (_tmp, mut a) = app(vec![]);
        crate::applog::take_captured();
        a.dialog = Some(Dialog::Message("Export failed".into()));
        // `take_message` logs before taking, and forgets the text once taken: the cache that
        // stops a message from being logged twice must not hold it, or the same message taken
        // again right after (no frame in between) would not be logged.
        assert_eq!(a.take_message().as_deref(), Some("Export failed"));
        assert!(a.logged.1.is_empty(), "the taken message no longer blocks the next one");
        a.dialog = Some(Dialog::Message("Export failed".into()));
        assert_eq!(a.take_message().as_deref(), Some("Export failed"));
        assert!(a.logged.1.is_empty());
        let warned = (crate::applog::Level::Warn, "Export failed".to_owned());
        assert_eq!(
            crate::applog::take_captured(),
            [warned.clone(), warned],
            "both reached the log"
        );
        // And the frame's own logging sees a third one as new.
        a.dialog = Some(Dialog::Message("Export failed".into()));
        assert_eq!(a.new_messages().len(), 1);
    }

    #[test]
    fn every_tool_is_handled_and_named_once() {
        let (_tmp, mut a) = app(vec![]);
        let mut names = std::collections::BTreeSet::new();
        for spec in specs() {
            let name = spec["name"].as_str().unwrap().to_owned();
            assert!(names.insert(name.clone()), "{name} twice");
            assert!(spec["input_schema"]["type"] == "object");
            if name == "run_command" || name == "open_project" {
                continue;
            }
            if let Err(e) = a.agent_tool(&name, &json!({})) {
                assert!(!e.starts_with("Unknown tool"), "{name}: {e}");
            }
        }
    }

    #[test]
    fn a_bad_cable_tag_leaves_the_wire_unchanged() {
        let (_tmp, mut a) = app(vec![]);
        let coil: Value = serde_json::from_str(
            &call(
                &mut a,
                "place_symbol",
                json!({"symbol": "wc.coil", "x": 60, "y": 120, "page": "2", "tag": "-K1"}),
            )
            .unwrap(),
        )
        .unwrap();
        let c = coil["id"].as_str().unwrap().to_owned();
        call(
            &mut a,
            "add_wire",
            json!({"page": "2", "from": {"item": c, "pin": "A1"}, "to": {"item": c, "pin": "A2"}}),
        )
        .unwrap();
        let id = page2(&a).wires[0].id.clone();
        let bad = call(
            &mut a,
            "update_wire",
            json!({"page": "2", "id": id, "section": "1.5 mm²", "cable": "?"}),
        );
        assert!(bad.is_err());
        assert_eq!(page2(&a).wires[0].spec.section, None, "nothing changed");
        call(
            &mut a,
            "update_wire",
            json!({"page": "2", "id": id, "section": "1.5 mm²"}),
        )
        .unwrap();
        assert_eq!(page2(&a).wires[0].spec.section.as_deref(), Some("1.5 mm²"));
    }

    #[test]
    fn draws_a_coil_and_its_contact_and_wires_them() {
        let (_tmp, mut a) = app(vec![]);
        let found = call(&mut a, "find_symbols", json!({"query": "coil"})).unwrap();
        assert!(found.contains("wc.coil"));
        let coil: Value = serde_json::from_str(
            &call(&mut a, "place_symbol", json!({"symbol": "wc.coil", "x": 60, "y": 120, "page": "2", "tag": "=PUMP-K1", "description": "Pump relay"})).unwrap(),
        )
        .unwrap();
        assert_eq!(coil["tag"], "=PUMP-K1");
        let contact: Value = serde_json::from_str(
            &call(
                &mut a,
                "place_symbol",
                json!({"symbol": "wc.contact.no", "x": 120, "y": 120, "page": "2", "tag": "=PUMP-K1"}),
            )
            .unwrap(),
        )
        .unwrap();
        let (c, k) = (coil["id"].as_str().unwrap(), contact["id"].as_str().unwrap());
        // A wrong pin name is refused with the right ones.
        let bad = call(
            &mut a,
            "add_wire",
            json!({"page": "2", "from": {"item": c, "pin": "13"}, "to": {"item": k, "pin": "13"}}),
        );
        assert!(bad.unwrap_err().contains("A1, A2"));
        call(&mut a, "add_wire", json!({"page": "2", "from": {"item": c, "pin": "A1"}, "to": {"item": k, "pin": "13"}, "via": [{"x": 60, "y": 100}, {"x": 120, "y": 100}], "potential": "24V"})).unwrap();
        let p = page2(&a);
        assert_eq!(p.symbols.len(), 2);
        assert_eq!(p.wires.len(), 1);
        assert_eq!(p.wires[0].potential.as_deref(), Some("24V"));
        let open = a.open.as_ref().unwrap();
        let t = DeviceTag::parse("=PUMP-K1").unwrap();
        assert_eq!(open.project.devices[&t].description, "Pump relay");
        // No stray automatic device was left behind.
        assert!(!open.project.devices.keys().any(|k| k.to_string() == "-K1"));

        // The page view gives absolute pin positions.
        let seen: Value = serde_json::from_str(&call(&mut a, "get_page", json!({"page": "p0002"})).unwrap()).unwrap();
        assert_eq!(seen["symbols"][0]["pins"].as_array().unwrap().len(), 2);

        // Rename the device everywhere.
        call(
            &mut a,
            "set_device",
            json!({"tag": "=PUMP-K1", "rename_to": "=PUMP-K7"}),
        )
        .unwrap();
        assert!(
            page2(&a)
                .symbols
                .iter()
                .all(|s| s.tag.as_ref().unwrap().to_string() == "=PUMP-K7")
        );
    }

    #[test]
    fn edits_pages_comments_graphics_and_runs_commands() {
        let (_tmp, mut a) = app(vec![]);
        let out = call(&mut a, "add_page", json!({"title": "Motors", "size": "A2"})).unwrap();
        assert!(out.contains("Motors"));
        let open = a.open.as_ref().unwrap();
        let p = open.project.pages.iter().find(|p| p.title == "Motors").unwrap();
        assert_eq!(p.size, PageSize::A2);
        assert_eq!(a.ed.page, open.find_page(&p.number).unwrap());

        call(
            &mut a,
            "add_graphic",
            json!({"page": "2", "shape": {"kind": "text", "at": {"x": 30, "y": 30}, "text": "Hello"}}),
        )
        .unwrap();
        call(&mut a, "update_graphic", json!({"page": "2", "id": "g0", "shape": {"kind": "rect", "min": {"x": 0, "y": 0}, "max": {"x": 5, "y": 5}}})).unwrap();
        assert!(matches!(page2(&a).graphics[0], Shape::Rect { .. }));

        let c = call(
            &mut a,
            "comments",
            json!({"action": "add", "page": "2", "text": "Check this", "x": 10, "y": 10}),
        )
        .unwrap();
        assert!(c.contains("c1"));
        call(&mut a, "comments", json!({"action": "resolve", "id": "c1"})).unwrap();
        assert!(a.open.as_ref().unwrap().project.comments[0].resolved);

        let r = call(&mut a, "run_action", json!({"action": "ZoomFit"})).unwrap();
        assert!(r.starts_with("Ran"));
        assert!(call(&mut a, "run_action", json!({"action": "Nope"})).is_err());

        call(&mut a, "set_project", json!({"title_block": {"customer": "ACME"}})).unwrap();
        assert_eq!(a.open.as_ref().unwrap().project.meta.title_block["customer"], "ACME");

        assert!(
            call(&mut a, "get_report", json!({"kind": "devices"}))
                .unwrap()
                .contains('|')
        );
        call(&mut a, "run_check", json!({})).unwrap();
    }

    #[test]
    fn edit_page_json_replaces_the_page_and_keeps_its_id() {
        let (_tmp, mut a) = app(vec![]);
        let content = json!({
            "number": "2", "title": "Rewritten",
            "symbols": [{"id": "s1", "symbol": "wc.lamp", "at": {"x": 50, "y": 50}, "tag": "-H1"}],
            "wires": [{"id": "w1", "from": {"item": "s1", "pin": "X1"}, "to": {"at": {"x": 50, "y": 20}}}]
        });
        call(&mut a, "edit_page_json", json!({"page": "2", "content": content})).unwrap();
        let p = page2(&a);
        assert_eq!((p.title.as_str(), p.symbols.len(), p.wires.len()), ("Rewritten", 1, 1));
        assert!(a.open.as_ref().unwrap().symbols.contains_key("wc.lamp"));
    }

    #[test]
    fn undo_is_refused_in_a_shared_session() {
        let (_tmp, mut a) = app(vec![]);
        call(
            &mut a,
            "place_symbol",
            json!({"symbol": "wc.coil", "x": 50, "y": 50, "page": "2"}),
        )
        .unwrap();
        let mut open = a.open.take().unwrap();
        a.share.start_host(&mut open, "Hanna", "hanna@example.com").unwrap();
        a.open = Some(open);
        let before = a.open.as_ref().unwrap().project.clone();
        for redo in [false, true] {
            assert!(call(&mut a, "undo", json!({"redo": redo})).is_err());
            assert_eq!(a.open.as_ref().unwrap().project, before, "nothing was undone");
        }
        a.share.stop();
        assert!(call(&mut a, "undo", json!({})).is_ok());
    }

    #[test]
    fn a_whole_turn_is_one_undo_step() {
        let answers = vec![
            json!({
                "model": "claude-opus-5-5", "stop_reason": "tool_use",
                "content": [
                    {"type": "tool_use", "id": "t1", "name": "place_symbol", "input": {"symbol": "wc.coil", "x": 50, "y": 50, "page": "2"}},
                    {"type": "tool_use", "id": "t2", "name": "place_symbol", "input": {"symbol": "wc.lamp", "x": 90, "y": 50, "page": "2"}}
                ]
            }),
            json!({
                "model": "claude-opus-5-5", "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "Placed a coil and a lamp."}]
            }),
        ];
        let (_tmp, mut a) = app(answers);
        let ctx = eframe::egui::Context::default();
        a.prefs.assistant.backend = wc_agent::Backend::ApiKey;
        a.assistant.input = "Put a coil and a lamp on page 2".into();
        a.assistant_send(&ctx);
        for _ in 0..500 {
            a.assistant_poll(&ctx);
            if !a.assistant.session.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!a.assistant.session.busy());
        assert_eq!(page2(&a).symbols.len(), 2);
        assert!(matches!(
            a.assistant.session.log.last(),
            Some(wc_agent::Entry::Assistant(_))
        ));
        let open = a.open.as_mut().unwrap();
        assert!(open.undo());
        assert!(
            open.project.page("p0002").unwrap().symbols.is_empty(),
            "one undo takes back the whole turn"
        );
        assert!(!open.can_undo());
    }

    /// Runs when `WIRINGCRAFT_CLI` points at a built `wiringcraft` program.
    #[test]
    fn todo_requests_open_the_filled_in_form_without_the_go_ahead() {
        let (_tmp, mut a) = app(vec![]);
        let out = call(
            &mut a,
            "todo",
            json!({"action": "file", "kind": "missing", "title": "4-pole breaker symbol", "what": "No 4-pole breaker in the library"}),
        )
        .unwrap();
        assert!(out.contains("browser"), "{out}");
        // A second request in the same reply opens its own form too.
        call(
            &mut a,
            "todo",
            json!({"action": "file", "kind": "function", "title": "Cable plan per cabinet", "what": "Group the cable plan by cabinet"}),
        )
        .unwrap();
        assert_eq!(a.open_url.len(), 2);
        assert!(
            a.open_url[0].contains("template=missing.yml&title=4-pole%20breaker%20symbol"),
            "{:?}",
            a.open_url
        );
        assert!(a.open_url[1].contains("template=feature.yml"), "{:?}", a.open_url);
        assert!(
            call(
                &mut a,
                "todo",
                json!({"action": "file", "kind": "nope", "title": "x", "what": "y"})
            )
            .is_err()
        );
        assert!(
            call(
                &mut a,
                "todo",
                json!({"action": "file", "kind": "problem", "title": " ", "what": "y"})
            )
            .is_err()
        );
        assert!(call(&mut a, "todo", json!({"action": "other"})).is_err());
        // With a screenshot in the conversation the form opens even with the go-ahead, since
        // pictures can only be added there.
        a.assistant
            .shared
            .push(Shot::from_rgba(1, 1, vec![0, 0, 0, 255]).unwrap());
        let out = call(
            &mut a,
            "todo",
            json!({"action": "file", "kind": "problem", "title": "Wrong arrow", "what": "See screenshot", "confirmed": true}),
        )
        .unwrap();
        assert!(out.contains("Screenshots box"), "{out}");
        assert_eq!(a.open_url.len(), 3);
    }

    #[test]
    fn an_unanswered_message_gets_its_screenshots_back() {
        let shot = |w| Shot::from_rgba(w, 1, vec![0; w * 4]).unwrap();
        let mut s = Assistant::with_transport(Arc::new(Script(Mutex::new(Vec::new()))));
        s.sent_text = "look at this".into();
        s.sent = vec![shot(1), shot(2)];
        // A picture dropped while the message was on its way stays, after the given-back ones.
        s.waiting = vec![(shot(3), None)];
        assert_eq!(s.give_back(None), 0);
        let widths: Vec<u32> = s.waiting.iter().map(|(x, _)| x.image.width()).collect();
        assert_eq!(widths, vec![1, 2, 3]);
        assert_eq!(s.input, "look at this");
        assert!(s.sent.is_empty());
        // Words typed since are kept; the given-back words come from the API.
        s.input = "new words".into();
        s.give_back(Some("old".into()));
        assert_eq!(s.input, "new words");
        assert_eq!(s.waiting.len(), 3);
        // Full: every given-back screenshot stays, newer ones that do not fit are counted.
        s.sent = (1..=5).map(shot).collect();
        s.waiting = vec![(shot(9), None)];
        assert_eq!(s.give_back(None), 1);
        let widths: Vec<u32> = s.waiting.iter().map(|(x, _)| x.image.width()).collect();
        assert_eq!(widths, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn run_command_saves_runs_and_reloads() {
        if std::env::var_os("WIRINGCRAFT_CLI").is_none() {
            return;
        }
        let (_tmp, mut a) = app(vec![]);
        call(
            &mut a,
            "place_symbol",
            json!({"symbol": "wc.coil", "x": 50, "y": 50, "page": "2"}),
        )
        .unwrap();
        let out = call(
            &mut a,
            "run_command",
            json!({"args": ["place", "p0002", "wc.lamp", "--at", "100,50"]}),
        );
        assert!(out.is_ok(), "{out:?}");
        assert_eq!(page2(&a).symbols.len(), 2, "the command's change is loaded back");
        let help = call(&mut a, "run_command", json!({"args": ["--help"]})).unwrap();
        assert!(help.contains("plc-vars"));
    }

    /// Start `run_command` (it fails fast when the command-line program is not built, which is
    /// fine here), let `meanwhile` act as the window and the command, then finish it.
    fn command_around(a: &mut App, meanwhile: impl FnOnce(&mut App)) -> Result<String, String> {
        let args = json!({"args": ["--help"]});
        let started = a.agent_start("run_command", &args);
        assert!(
            matches!(started, Started::Running(_)),
            "run_command should run on a worker thread"
        );
        let Started::Running(p) = started else {
            return Err("not started".into());
        };
        meanwhile(a);
        let r = p.wait();
        a.agent_finish("run_command", &args, r)
    }

    fn customer(a: &App) -> Option<String> {
        a.open.as_ref()?.project.meta.title_block.get("customer").cloned()
    }

    /// The command writes a customer into the project on disk, as `wiringcraft` would.
    fn command_writes_customer(a: &App, name: &str) {
        let mut disk = Open::load(&a.open.as_ref().unwrap().dir).unwrap();
        disk.project.meta.title_block.insert("customer".into(), name.into());
        disk.dirty = true;
        disk.save().unwrap();
    }

    #[test]
    fn a_command_does_not_hold_up_the_window() {
        let (_tmp, mut a) = app(vec![]);
        let started = std::time::Instant::now();
        let r = command_around(&mut a, |a| {
            // The window thread is free while the command runs: it can still edit.
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            call(a, "set_project", json!({"title_block": {"customer": "Window"}})).unwrap();
        });
        // The command left the project alone, so the edit made meanwhile stays.
        assert_eq!(customer(&a).as_deref(), Some("Window"), "{r:?}");
    }

    #[test]
    fn a_command_s_changes_are_read_back() {
        let (_tmp, mut a) = app(vec![]);
        let r = command_around(&mut a, |a| command_writes_customer(a, "Disk"));
        assert_eq!(customer(&a).as_deref(), Some("Disk"));
        assert!(!format!("{r:?}").contains("not loaded"));
    }

    #[test]
    fn when_both_changed_the_project_the_window_s_edits_win() {
        let (_tmp, mut a) = app(vec![]);
        let r = command_around(&mut a, |a| {
            call(a, "set_project", json!({"title_block": {"customer": "Window"}})).unwrap();
            command_writes_customer(a, "Disk");
        });
        assert_eq!(customer(&a).as_deref(), Some("Window"));
        assert!(
            a.open.as_ref().unwrap().dirty,
            "the window's edits are still to be saved"
        );
        let e = r.unwrap_err();
        assert!(e.contains("not loaded"), "{e}");
    }

    #[test]
    fn the_window_does_not_save_while_a_command_runs() {
        let (_tmp, mut a) = app(vec![]);
        let r = command_around(&mut a, |a| {
            assert!(a.assistant.command_running());
            call(a, "set_project", json!({"title_block": {"customer": "First"}})).unwrap();
            let _ = call(a, "run_action", json!({"action": "Save"}));
            let dir = a.open.as_ref().unwrap().dir.clone();
            let disk = Open::load(&dir).unwrap();
            assert_eq!(disk.project.meta.title_block.get("customer"), None, "nothing saved");
            call(a, "set_project", json!({"title_block": {"customer": "Second"}})).unwrap();
        });
        // A read-only command: the latest window edit stays, with no conflict reported.
        assert_eq!(customer(&a).as_deref(), Some("Second"));
        assert!(!format!("{r:?}").contains("not loaded"), "{r:?}");
        assert!(!a.assistant.command_running());
    }

    #[test]
    fn todo_searches_run_on_a_worker_thread() {
        let (_tmp, mut a) = app(vec![]);
        let v = json!({"action": "search", "query": "relay"});
        assert!(matches!(a.agent_start("todo", &v), Started::Running(_)));
        // Bad input is answered straight away.
        assert!(matches!(
            a.agent_start("todo", &json!({"action": "file"})),
            Started::Done(Err(_))
        ));
    }
}
