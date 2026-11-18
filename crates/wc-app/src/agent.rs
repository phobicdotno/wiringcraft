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
            "Overview of the open project: name, folder, branch, unsaved changes, pages (id, number, title, size, overview, cover, counts), panel layouts, devices count, open comments, the page or panel shown, the selection and whether undo is possible.",
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
            "Change a page's number, title or sheet size, mark it as an overview (a line diagram: its devices do not count as drawings in the project check), make it the project's cover page (it shows the project's name and fields; any other cover page loses the mark), or set the page's own title-block fields (get_page lists them).",
            json!({"page": page_arg(), "number": {"type": "string"}, "title": {"type": "string"}, "size": {"type": "string", "enum": ["A4", "A3", "A2"]}, "overview": {"type": "boolean"}, "cover": {"type": "boolean"}, "title_block": {"type": "object", "additionalProperties": {"type": ["string", "null"]}, "description": "The page's own title-block fields to change, such as revision, date, drawn_by, checked_by. They win over the project's on this page. An empty text or null clears a field so the project's shows again; fields left out are kept."}}),
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
            json!({"page": page_arg(), "content": {"type": "object", "description": "The page: number, title, size, overview (true for a line diagram; leaving it out makes a normal page), cover (true for the cover page; leaving it out keeps the page's current value), title_block (the page's own title-block fields such as revision and date, which win over the project's; leaving it out keeps the page's current ones), symbols, wires, graphics, underlay."}}),
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
            "Rename the project, set title-block fields (customer, project_number, plant, drawn_by, checked_by, revision, date, or any other; empty value removes one) and set or remove the logo.",
            json!({"name": {"type": "string"}, "title_block": {"type": "object", "additionalProperties": {"type": "string"}}, "logo": {"type": "string", "description": "A PNG or JPEG file in the project folder (path relative to it) to make the logo; it is copied into pictures/ when it is elsewhere. An empty text removes the logo (its file stays)."}}),
            &[],
        ),
        tool(
            "get_template",
            "The page template every page is drawn on: columns (reference columns across the frame) and the title block's boxes (label; left and right in mm from the frame side named by left_from/right_from, \"left\" or \"right\"; top and bottom in mm down the title block band, whose height in mm is height; texts with values such as project.name, page.title, page.number, page.sheet or title-block field keys, size, y, align and inset; logo for the logo box). is_default tells whether it is the built-in layout.",
            json!({}),
            &[],
        ),
        tool(
            "set_template",
            "Change the page template as one undo step: set the column count (1 to 40), the title block's height (10 to 60 mm), replace its boxes (in get_template's format), or reset to the built-in layout.",
            json!({
                "columns": {"type": "integer", "minimum": 1, "maximum": 40},
                "height": {"type": "number", "minimum": 10, "maximum": 60, "description": "The title block band's height in mm (24 by default)."},
                "boxes": {"type": "array", "items": {"type": "object"}, "description": "Every box of the title block, as get_template returns them."},
                "reset": {"type": "boolean", "description": "Go back to the built-in layout; columns and boxes are then applied on top of it."}
            }),
            &[],
        ),
        tool(
            "jumpers",
            "Jumpers bridging terminals of one strip: list them (strip, terminals in strip order, part), add (jumper the terminals of a strip together; a jumper that shares a terminal is merged), remove (take the terminals out of their jumpers; one left with fewer than two terminals goes), or part (give the jumper holding a terminal a part id from the project or its libraries; an empty part clears it).",
            json!({
                "action": {"type": "string", "enum": ["list", "add", "remove", "part"]},
                "strip": {"type": "string", "description": "The strip's tag, e.g. -X1."},
                "terminals": {"type": "array", "items": {"type": "string"}, "description": "Terminal numbers on the strip, e.g. [\"1\", \"4\"] (add, remove)."},
                "terminal": {"type": "string", "description": "A terminal of the jumper (part)."},
                "part": {"type": "string", "description": "The part id, e.g. wago.2002-402; empty to clear (part)."}
            }),
            &["action"],
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

/// Jumpers as the assistant sees them: strip, terminals in strip order, and the part if any.
fn jumper_list(jumpers: &[wc_core::project::Jumper]) -> Vec<Value> {
    jumpers
        .iter()
        .map(|j| json!({"strip": j.strip.to_string(), "terminals": j.terminals, "part": j.part}))
        .collect()
}

fn need_s<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    s(v, key).ok_or_else(|| format!("`{key}` is missing."))
}

fn need_num(v: &Value, key: &str) -> Result<f64, String> {
    num(v, key).ok_or_else(|| format!("`{key}` must be a number."))
}

/// `terminals` as texts: an entry that is not one is a mistake, so the whole request is refused
/// rather than run on the rest.
fn terminal_names(v: &Value) -> Result<Vec<String>, String> {
    let Some(list) = v.get("terminals") else {
        return Ok(Vec::new());
    };
    let list = list
        .as_array()
        .ok_or("`terminals` must be a list of terminal numbers.")?;
    list.iter()
        .map(|t| {
            t.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("`terminals` holds {t}, which is not a terminal number in quotes."))
        })
        .collect()
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
            "overview": p.overview, "cover": p.cover,
            "title_block": p.title_block,
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
        Ok(self.project.rename_device(from, to))
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
            "comments" | "macros" | "panels" | "library" | "versions" | "jumpers" => {
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
                            "tag": t.to_string(), "description": d.description, "part": d.part, "accessories": d.accessories,
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
                if let Some(why) = open.held_reason(to) {
                    return Err(why);
                }
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
                i = open.set_page_props(i, &number, &title, size.unwrap_or(cur.size), cur.overview, None)?;
                let p = open.project.pages.get(i).cloned().ok_or("The page was not added.")?;
                self.go_to(i);
                Ok(format!("Added page {} ({}), id {}.", p.number, p.title, p.id))
            }
            "update_page" => {
                let page = self.page_of(v, "page")?;
                let size = page_size(v)?;
                let open = need_open(self.open.as_mut())?;
                // Someone else in a shared session has the page open: it is theirs to change.
                if let Some(why) = open.held_reason(page) {
                    return Err(why);
                }
                let cur = open.project.pages.get(page).cloned().ok_or("No such page.")?;
                let fields = match v.get("title_block") {
                    None | Some(Value::Null) => None,
                    Some(Value::Object(map)) => Some(
                        map.iter()
                            .map(|(k, val)| match val {
                                Value::String(t) => Ok((k.clone(), t.clone())),
                                Value::Null => Ok((k.clone(), String::new())),
                                _ => Err(format!("title_block.{k} must be a text, or empty or null to clear it")),
                            })
                            .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?,
                    ),
                    Some(_) => return Err("title_block must be an object of field names and texts.".into()),
                };
                let i = open.set_page_props(
                    page,
                    s(v, "number").unwrap_or(&cur.number),
                    s(v, "title").unwrap_or(&cur.title),
                    size.unwrap_or(cur.size),
                    v.get("overview").and_then(Value::as_bool).unwrap_or(cur.overview),
                    fields.as_ref(),
                )?;
                if let Some(cover) = v.get("cover").and_then(Value::as_bool) {
                    open.set_page_cover(i, cover)?;
                }
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
                let other = if up { page.checked_sub(1) } else { Some(page + 1) };
                if let Some(why) = open
                    .held_reason(page)
                    .or_else(|| other.and_then(|o| open.held_reason(o)))
                {
                    return Err(why);
                }
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
                // A page content without `cover` keeps the page's own value.
                let was_cover = open.project.pages.get(page).is_some_and(|p| p.cover);
                if content.get("cover").is_none_or(Value::is_null) {
                    content["cover"] = json!(was_cover);
                }
                // Nor does one without `title_block` drop the page's own fields (its revision,
                // date, ...): they are kept.
                if content.get("title_block").is_none_or(Value::is_null) {
                    let own = open
                        .project
                        .pages
                        .get(page)
                        .map(|p| p.title_block.clone())
                        .unwrap_or_default();
                    content["title_block"] = json!(own);
                }
                let new: Page = serde_json::from_value(content).map_err(|e| format!("Not a page: {e}"))?;
                open.ensure_page_symbols(&new)?;
                open.checkpoint();
                for s in &new.symbols {
                    if let Some(t) = &s.tag {
                        open.project.devices.entry(t.clone()).or_default();
                    }
                }
                let cover = new.cover;
                if let Some(p) = open.project.pages.get_mut(page) {
                    *p = new;
                }
                // One cover page per project: a new cover takes the mark from any other page.
                if cover {
                    open.project.set_cover(page, true);
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
                // The logo first: a file that cannot be used changes nothing.
                let logo = match s(v, "logo").map(str::trim) {
                    None => None,
                    Some("") => Some(None),
                    Some(path) => {
                        if !wc_core::project::relative_file_is_safe(path) {
                            return Err(format!(
                                "{path}: give a file in the project folder, as a path relative to it"
                            ));
                        }
                        let src = open.dir.join(path);
                        let real = src.canonicalize().map_err(|e| format!("{path}: {e}"))?;
                        let dir = open.dir.canonicalize().map_err(|e| e.to_string())?;
                        if !real.starts_with(&dir) {
                            return Err(format!("{path} leads out of the project folder"));
                        }
                        let bytes = std::fs::read(&real).map_err(|e| format!("{path}: {e}"))?;
                        if wc_core::Logo::picture_kind(&bytes).is_none() {
                            return Err(format!("{path}: the logo must be a PNG or JPEG picture"));
                        }
                        Some(Some(src))
                    }
                };
                // The only way the properties can be refused; checked before the logo is copied,
                // and the logo copied before the properties change, so a failure changes nothing.
                if name.trim().is_empty() {
                    return Err("The project name cannot be empty.".into());
                }
                let said = match logo {
                    Some(Some(src)) => {
                        let file = open.set_logo_from(&src)?;
                        format!("Project properties set; the logo is {file}.")
                    }
                    Some(None) => {
                        open.remove_logo();
                        "Project properties set; the logo is removed.".to_owned()
                    }
                    None => "Project properties set.".to_owned(),
                };
                let fields: Vec<(String, String)> = fields.into_iter().collect();
                open.set_project_props(&name, &fields, None)?;
                Ok(said)
            }
            "get_template" => {
                let open = need_open(self.open.as_mut())?;
                let t = &open.project.meta.template;
                Ok(pretty(&json!({
                    "columns": t.columns,
                    "height": t.height,
                    "boxes": t.boxes,
                    "is_default": t.is_default(),
                })))
            }
            "set_template" => self.tool_set_template(v),
            "comments" => self.tool_comments(v),
            "jumpers" => self.tool_jumpers(v),
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
                    let done = open.update_library(&lib, only)?;
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
                let redo = v.get("redo").and_then(Value::as_bool).unwrap_or(false);
                // In a session the host takes back only this person's own change; swapping the
                // whole project here would take back other people's work too.
                if self.share.active() {
                    // Changes made earlier in this answer go into the session's history first.
                    self.sync_session_now(false);
                    let can = if redo {
                        self.share.can_redo()
                    } else {
                        self.share.can_undo()
                    };
                    // The host's list is complete here. A guest's may not yet hold the change
                    // just sent: the host, which gets the undo after that change, decides, and
                    // says so in the session's notes when there is nothing to take back.
                    if !can && self.share.is_host() {
                        return Ok("Nothing to undo or redo.".into());
                    }
                    if redo { self.share.redo() } else { self.share.undo() }?;
                    self.ed.selection.clear();
                    return Ok(if can {
                        "Sent to the session; it shows with the next sync."
                    } else {
                        "Sent to the session. Your latest change may not be in the session's list yet; if there is nothing of yours to take back, the session says so with the next sync."
                    }
                    .into());
                }
                let open = need_open(self.open.as_mut())?;
                let done = if redo { open.redo() } else { open.undo() };
                if let Some(e) = open.take_undo_error() {
                    return Err(format!("Not done: {e}"));
                }
                self.ed.selection.clear();
                if let Some(w) = open.take_step_warning() {
                    return Ok(format!("Done, but {w}."));
                }
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
                    "overview": p.overview, "cover": p.cover,
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
            "logo": open.project.meta.logo.as_ref().map(|l| l.file.clone()),
            "libraries": open.project.meta.libraries.iter().map(|l| format!("{} ({}: {})", l.name, l.kind, l.location)).collect::<Vec<_>>(),
            "pages": pages,
            "panels": panels,
            "devices": open.project.devices.len(),
            "jumpers": jumper_list(&open.project.meta.jumpers),
            "open_comments": open.project.comments.iter().filter(|c| !c.resolved).count(),
            "shown": shown,
            "selection": self.ed.selection.iter().map(selection_id).collect::<Vec<_>>(),
            // In a session, Undo takes back only this person's changes made in it.
            "can_undo": if self.share.active() { self.share.can_undo() } else { open.can_undo() },
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

    fn tool_set_template(&mut self, v: &Value) -> Result<String, String> {
        let open = need_open(self.open.as_mut())?;
        let mut t = if v.get("reset").and_then(Value::as_bool) == Some(true) {
            wc_core::template::PageTemplate::default()
        } else {
            open.project.meta.template.clone()
        };
        if let Some(c) = v.get("columns").filter(|c| !c.is_null()) {
            let max = wc_core::template::MAX_COLUMNS;
            t.columns = c
                .as_u64()
                .and_then(|c| usize::try_from(c).ok())
                .filter(|c| (1..=max).contains(c))
                .ok_or_else(|| format!("columns must be a whole number from 1 to {max}"))?;
        }
        if let Some(h) = v.get("height").filter(|h| !h.is_null()) {
            t.height = h.as_f64().ok_or("height must be a number of millimetres")?;
        }
        if let Some(b) = v.get("boxes").filter(|b| !b.is_null()) {
            t.boxes = serde_json::from_value(b.clone()).map_err(|e| format!("boxes: {e}"))?;
        }
        if let Some(why) = t.problem() {
            return Err(format!("The template was not changed: {why}."));
        }
        if t == open.project.meta.template {
            return Ok("The template is already like that.".into());
        }
        open.checkpoint();
        open.project.meta.template = t;
        Ok("Template set.".into())
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

    fn tool_jumpers(&mut self, v: &Value) -> Result<String, String> {
        let action = need_s(v, "action")?;
        let open = need_open(self.open.as_mut())?;
        if action == "list" {
            return Ok(pretty(&json!(jumper_list(&open.project.meta.jumpers))));
        }
        let strip = tag(need_s(v, "strip")?)?;
        match action {
            "add" => {
                let terminals = terminal_names(v)?;
                if !open.jumper_offer(&strip, &terminals)? {
                    return Ok(format!(
                        "Terminals {} of {strip} are jumpered together already.",
                        terminals.join(", ")
                    ));
                }
                open.add_jumper(&strip, &terminals)?;
                Ok(format!("Jumpered {strip} {}.", terminals.join(", ")))
            }
            "remove" => {
                let terminals = terminal_names(v)?;
                if terminals.is_empty() {
                    return Err("`terminals` is missing.".into());
                }
                // A terminal is one tag level, as `add` checks: refuse the whole request
                // before anything changes.
                for t in &terminals {
                    let mut one = strip.clone();
                    one.product.push(t.trim().to_owned());
                    if DeviceTag::parse(&one.to_string()).ok().as_ref() != Some(&one) {
                        return Err(format!(
                            "{t:?} cannot be a terminal of {strip}: a terminal is one tag level."
                        ));
                    }
                }
                if !open.remove_from_jumpers(&strip, &terminals) {
                    return Err(format!("No jumper of {strip} has terminal {}.", terminals.join(", ")));
                }
                Ok(format!("Took {strip} {} out of their jumpers.", terminals.join(", ")))
            }
            "part" => {
                let terminal = need_s(v, "terminal")?.trim();
                // Only a text, empty to clear it: a missing or other value is a mistake, not a
                // request to clear the part.
                if !v.get("part").is_some_and(Value::is_string) {
                    return Err("`part` must be a part id, or an empty text to clear it.".into());
                }
                let part = s(v, "part").map(str::trim).filter(|p| !p.is_empty());
                open.set_jumper_part(&strip, terminal, part)?;
                Ok(match part {
                    Some(p) => format!("The jumper with {strip} {terminal} has part {p}."),
                    None => format!("The jumper with {strip} {terminal} has no part now."),
                })
            }
            other => Err(format!("Unknown jumpers action {other}.")),
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
        // Pending window changes follow their terminals under the roles they were made with.
        open.follow_terminals();
        // The versions the command replaced are kept, so its undo step can put them back.
        open.archive_symbols();
        open.branch = fresh.branch;
        let by_command = fresh.project != base;
        let by_window = open.project != base;
        let mut note = None;
        if by_command && by_window {
            // The window's project, and so its symbol lock, is kept: its symbol files go back
            // over the ones the command wrote, so the lock and the files still match.
            // A symbol the pages use counts too: a project from before the lock can use one
            // without a lock entry, and it is loaded from its file all the same.
            let used = open.project.used_symbols();
            let mut unrestored = Vec::new();
            for (id, sym) in open.symbols.clone() {
                if fresh.symbols.get(&id) != Some(&sym)
                    && (open.project.lock.symbols.contains_key(&id) || used.contains(&id))
                    && let Err(e) = open.write_symbol_file(&sym)
                {
                    // Saving writes it again, and does not go ahead until it can.
                    open.mark_symbol_file_stale(&id);
                    unrestored.push(format!("{id} ({e})"));
                }
            }
            note = Some(if unrestored.is_empty() {
                "The project was edited in the window while the command ran, so the command's changes were \
                 not loaded: the window's edits are kept, and saving writes them over the command's result."
                    .to_owned()
            } else {
                format!(
                    "The project was edited in the window while the command ran, so the command's changes were \
                     not loaded: the window's edits are kept, and saving writes them over the command's result. \
                     The command's version of these symbols is still in the project folder, and saving \
                     writes the window's version over it first: {}.",
                    unrestored.join(", ")
                )
            });
        } else if by_command {
            // Symbols the command added or changed come in; symbols the window vendored before
            // it ran stay.
            open.symbols.extend(fresh.symbols);
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
                self.ed.message = n.clone();
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
            symbol_editor: None,
            find: None,
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
    fn the_assistant_reads_and_changes_the_page_template_as_one_undo_step() {
        let (_tmp, mut a) = app(vec![]);
        let read =
            |a: &mut App| -> Value { serde_json::from_str(&call(a, "get_template", json!({})).unwrap()).unwrap() };
        let t = read(&mut a);
        assert_eq!(t["is_default"], json!(true));
        assert_eq!(t["columns"], json!(10));
        let boxes = t["boxes"].clone();
        assert!(boxes.as_array().is_some_and(|b| !b.is_empty()));

        // Bad input changes nothing.
        for bad in [
            json!({"columns": 0}),
            json!({"columns": 41}),
            json!({"columns": "many"}),
            json!({"boxes": [{"left": 0, "right": 10, "top": 5, "bottom": 2}]}),
            json!({"boxes": "none"}),
        ] {
            assert!(call(&mut a, "set_template", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(read(&mut a)["is_default"], json!(true));

        // Columns and boxes together: one undo step.
        let one = json!([{"label": "Order", "left": 0, "right": 60, "top": 0, "bottom": 24,
            "text": [{"values": ["project.name"], "size": 3.5, "y": 10}]}]);
        call(&mut a, "set_template", json!({"columns": 12, "boxes": one})).unwrap();
        let t = read(&mut a);
        assert_eq!(t["columns"], json!(12));
        assert_eq!(t["boxes"].as_array().map(Vec::len), Some(1));
        assert_eq!(t["boxes"][0]["label"], json!("Order"));
        assert!(a.open.as_mut().unwrap().undo());
        assert_eq!(read(&mut a)["is_default"], json!(true));
        assert!(a.open.as_mut().unwrap().redo());
        assert_eq!(read(&mut a)["columns"], json!(12));

        // The title block's height, checked like the rest.
        assert!(call(&mut a, "set_template", json!({"height": 70})).is_err());
        call(&mut a, "set_template", json!({"height": 36})).unwrap();
        assert_eq!(read(&mut a)["height"], json!(36.0));
        call(&mut a, "set_template", json!({"reset": true})).unwrap();
        let t = read(&mut a);
        assert_eq!(t["is_default"], json!(true));
        assert_eq!(t["boxes"], boxes);
    }

    #[test]
    fn the_assistant_lists_adds_removes_and_gives_parts_to_jumpers() {
        let (_tmp, mut a) = app(vec![]);
        let list = |a: &mut App| -> Value {
            serde_json::from_str(&call(a, "jumpers", json!({"action": "list"})).unwrap()).unwrap()
        };
        let undo_mark = |a: &App| a.open.as_ref().unwrap().undo_mark();
        assert_eq!(list(&mut a), json!([]));

        // Refusals change nothing and take no undo step.
        let before = undo_mark(&a);
        for bad in [
            json!({"action": "add", "strip": "-X1", "terminals": ["1"]}),
            json!({"action": "add", "strip": "-X1", "terminals": ["1", "2.1"]}),
            json!({"action": "add", "terminals": ["1", "2"]}),
            json!({"action": "remove", "strip": "-X1", "terminals": ["1"]}),
            json!({"action": "part", "strip": "-X1", "terminal": "1", "part": "example.ex-tb-2-5"}),
            json!({"action": "explode", "strip": "-X1"}),
        ] {
            assert!(call(&mut a, "jumpers", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(list(&mut a), json!([]));
        assert_eq!(undo_mark(&a), before);

        // Add: one undo step, and the state shows it.
        call(
            &mut a,
            "jumpers",
            json!({"action": "add", "strip": "-X1", "terminals": ["1", "2", "4"]}),
        )
        .unwrap();
        assert_eq!(undo_mark(&a), before + 1);
        let state: Value = serde_json::from_str(&call(&mut a, "get_state", json!({})).unwrap()).unwrap();
        assert_eq!(
            state["jumpers"],
            json!([{"strip": "-X1", "terminals": ["1", "2", "4"], "part": null}])
        );
        let again = call(
            &mut a,
            "jumpers",
            json!({"action": "add", "strip": "-X1", "terminals": ["2", "4"]}),
        )
        .unwrap();
        assert!(again.contains("already"), "{again}");
        assert_eq!(undo_mark(&a), before + 1);

        // Part: an unknown part is refused; a library part is taken into the project.
        let unknown = json!({"action": "part", "strip": "-X1", "terminal": "2", "part": "no.such"});
        assert!(call(&mut a, "jumpers", unknown).is_err());
        assert_eq!(undo_mark(&a), before + 1);
        call(
            &mut a,
            "jumpers",
            json!({"action": "part", "strip": "-X1", "terminal": "2", "part": "example.ex-tb-2-5"}),
        )
        .unwrap();
        assert_eq!(undo_mark(&a), before + 2);
        assert_eq!(list(&mut a)[0]["part"], json!("example.ex-tb-2-5"));
        assert!(a.open.as_ref().unwrap().parts.contains_key("example.ex-tb-2-5"));

        // Remove two terminals at once: one undo step, and the jumper (one left) goes.
        call(
            &mut a,
            "jumpers",
            json!({"action": "remove", "strip": "-X1", "terminals": ["1", "4"]}),
        )
        .unwrap();
        assert_eq!(undo_mark(&a), before + 3);
        assert_eq!(list(&mut a), json!([]));
        assert!(a.open.as_mut().unwrap().undo());
        let with_part = json!([{"strip": "-X1", "terminals": ["1", "2", "4"], "part": "example.ex-tb-2-5"}]);
        assert_eq!(list(&mut a), with_part);

        // A two-level or non-text terminal, or a part that is missing or not a text, is refused whole.
        let mark = undo_mark(&a);
        for bad in [
            json!({"action": "remove", "strip": "-X1", "terminals": ["1", "2.1"]}),
            json!({"action": "remove", "strip": "-X1", "terminals": ["1", 42]}),
            json!({"action": "add", "strip": "-X1", "terminals": ["4", "5", 6]}),
            json!({"action": "remove", "strip": "-X1", "terminals": "1"}),
            json!({"action": "part", "strip": "-X1", "terminal": "4", "part": 42}),
            json!({"action": "part", "strip": "-X1", "terminal": "4", "part": null}),
            json!({"action": "part", "strip": "-X1", "terminal": "4"}),
        ] {
            assert!(call(&mut a, "jumpers", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(list(&mut a), with_part);
        assert_eq!(undo_mark(&a), mark);

        // Clearing the part keeps the jumper.
        call(
            &mut a,
            "jumpers",
            json!({"action": "part", "strip": "-X1", "terminal": "4", "part": ""}),
        )
        .unwrap();
        assert_eq!(
            list(&mut a),
            json!([{"strip": "-X1", "terminals": ["1", "2", "4"], "part": null}])
        );
    }

    #[test]
    fn the_assistant_reads_sets_and_removes_the_logo() {
        let (_tmp, mut a) = app(vec![]);
        let dir = a.open.as_ref().unwrap().dir.clone();
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/brand.png"), &png).unwrap();
        std::fs::write(dir.join("docs/readme.png"), b"text").unwrap();
        let logo_of = |a: &mut App| {
            let state: Value = serde_json::from_str(&call(a, "get_state", json!({})).unwrap()).unwrap();
            state["logo"].clone()
        };
        assert_eq!(logo_of(&mut a), Value::Null);

        // Files outside the project, missing ones and other kinds are refused, changing nothing.
        for bad in ["../brand.png", "/etc/passwd", "docs/none.png", "docs/readme.png"] {
            assert!(
                call(&mut a, "set_project", json!({"logo": bad, "name": "Renamed"})).is_err(),
                "{bad}"
            );
        }
        assert_ne!(a.open.as_ref().unwrap().project.meta.name, "Renamed");
        assert_eq!(logo_of(&mut a), Value::Null);

        let r = call(
            &mut a,
            "set_project",
            json!({"logo": "docs/brand.png", "title_block": {"customer": "ACME"}}),
        )
        .unwrap();
        assert!(r.contains("pictures/brand.png"), "{r}");
        assert_eq!(logo_of(&mut a), json!("pictures/brand.png"));
        assert_eq!(std::fs::read(dir.join("pictures/brand.png")).unwrap(), png);
        assert_eq!(a.open.as_ref().unwrap().project.meta.title_block["customer"], "ACME");

        call(&mut a, "set_project", json!({"logo": ""})).unwrap();
        assert_eq!(logo_of(&mut a), Value::Null);
        assert!(dir.join("pictures/brand.png").exists(), "the file stays");

        // A copy that fails (pictures/ is a file) leaves the name, fields and undo as they were.
        std::fs::remove_dir_all(dir.join("pictures")).unwrap();
        std::fs::write(dir.join("pictures"), b"in the way").unwrap();
        let before = a.open.as_ref().unwrap().project.meta.clone();
        let could_undo = a.open.as_ref().unwrap().can_undo();
        assert!(
            call(
                &mut a,
                "set_project",
                json!({"logo": "docs/brand.png", "name": "Renamed", "title_block": {"customer": "Other"}})
            )
            .is_err()
        );
        assert_eq!(a.open.as_ref().unwrap().project.meta, before);
        assert_eq!(a.open.as_ref().unwrap().can_undo(), could_undo);
        assert!(call(&mut a, "set_project", json!({"name": "  "})).is_err());
    }

    #[test]
    fn the_symbol_editor_says_why_a_symbol_or_the_library_could_not_be_opened() {
        let (tmp, mut a) = app(vec![]);
        let lib = tmp.path().join("mine");
        std::fs::create_dir_all(lib.join("symbols")).unwrap();
        std::fs::write(lib.join("symbols/bad.json"), "{ not json").unwrap();
        let id = call(
            &mut a,
            "place_symbol",
            json!({"symbol": "wc.coil", "x": 50, "y": 50, "page": "2"}),
        )
        .unwrap();
        let open = a.open.as_mut().unwrap();
        open.project.meta.libraries.push(wc_core::project::LibrarySource {
            name: "mine".into(),
            kind: "folder".into(),
            location: lib.display().to_string(),
            reference: None,
        });
        let page = open.project.pages.iter().position(|p| p.number == "2").unwrap();
        let placed = open.project.pages[page]
            .symbols
            .iter_mut()
            .find(|p| id.contains(&p.id))
            .unwrap();
        placed.symbol = "bad".into();
        let item = placed.id.clone();
        a.ed.page = page;
        a.ed.selection = vec![Selection::Symbol(item)];
        a.run(Action::EditSymbol);
        let msg = &a.symbol_editor.as_ref().unwrap().message;
        assert!(
            msg.starts_with("Symbol bad could not be opened from the library: "),
            "{msg}"
        );
        let open = a.open.as_ref().unwrap();
        let read_error = wc_library::Library::open(&open.dir, &open.project)
            .unwrap()
            .symbol("bad")
            .unwrap_err()
            .to_string();
        assert!(!read_error.is_empty());
        assert!(msg.ends_with(&read_error), "the read error is kept: {msg}");

        a.symbol_editor = None;
        a.open.as_mut().unwrap().project.meta.libraries[0].kind = "bogus".into();
        a.run(Action::NewSymbol);
        let msg = &a.symbol_editor.as_ref().unwrap().message;
        assert!(msg.starts_with("The project's library could not be opened: "), "{msg}");
        assert!(msg.contains("bogus"), "{msg}");
    }

    #[test]
    fn editing_actions_are_off_and_do_nothing_on_a_page_someone_else_holds() {
        use crate::chrome::Status;
        let (_tmp, mut a) = app(vec![]);
        {
            let o = a.open.as_mut().unwrap();
            o.project.pages[0].graphics.push(wc_core::Shape::Line {
                a: wc_core::geom::Point::new(50.0, 50.0),
                b: wc_core::geom::Point::new(80.0, 50.0),
            });
        }
        a.ed.selection = vec![crate::state::Selection::Graphic(0)];
        a.clipboard = a.open.as_ref().unwrap().copy(0, &a.ed.selection);
        let status = |a: &App, act: Action| {
            crate::StatusView {
                open: a.open.as_ref(),
                ed: &a.ed,
                prefs: &a.prefs,
                clipboard: &a.clipboard,
                sharing: true,
                share_undo: false,
                share_redo: false,
            }
            .enabled(act)
        };
        let editing = [
            Action::Delete,
            Action::Cut,
            Action::Paste,
            Action::PlaceSymbol,
            Action::DrawWire,
            Action::TextTool,
            Action::LineTool,
            Action::RectTool,
            Action::PageProps,
        ];
        for act in editing {
            assert!(status(&a, act), "{act:?} works on your own page");
        }

        let page_id = a.open.as_ref().unwrap().project.pages[0].id.clone();
        a.ed.held_by.insert(page_id, "Ola".into());
        for act in editing {
            assert!(!status(&a, act), "{act:?} is greyed out on a held page");
        }
        for act in [
            Action::Select,
            Action::ZoomIn,
            Action::ZoomFit,
            Action::Copy,
            Action::CommentTool,
        ] {
            assert!(status(&a, act), "{act:?} still works");
        }

        // Pressed anyway (a shortcut): nothing changes, and the status line says why.
        let before = a.open.as_ref().unwrap().project.clone();
        for act in editing {
            a.run(act);
        }
        assert_eq!(a.open.as_ref().unwrap().project, before, "no project change");
        assert_eq!(a.ed.tool, crate::state::Tool::Select, "no drawing tool was picked");
        assert!(a.ed.message.contains("Ola"), "{}", a.ed.message);
        assert!(a.dialog.is_none(), "no page properties dialog");

        // Moving the held page up or down is refused too: both page numbers stay.
        a.ed.page = 0;
        let numbers = |a: &App| -> Vec<String> {
            a.open
                .as_ref()
                .unwrap()
                .project
                .pages
                .iter()
                .map(|p| p.number.clone())
                .collect()
        };
        let before_numbers = numbers(&a);
        assert!(!status(&a, Action::PageDown));
        a.run(Action::PageDown);
        assert_eq!(numbers(&a), before_numbers);

        // With a panel shown instead of the held page, the page's own actions stay refused,
        // while the panel's are not affected.
        a.ed.panel = Some(0);
        let schematic = a.open.as_ref().unwrap().project.pages[0].clone();
        for act in [
            Action::PageProps,
            Action::DeletePage,
            Action::PageDown,
            // These work on the schematic page under the panel.
            Action::Paste,
            Action::Cut,
            Action::Rotate,
            Action::Mirror,
        ] {
            assert!(!status(&a, act), "{act:?} is still about the held page");
            a.run(act);
        }
        assert!(a.dialog.is_none());
        assert_eq!(numbers(&a), before_numbers);
        assert_eq!(
            a.open.as_ref().unwrap().project.pages[0],
            schematic,
            "the held page is as it was"
        );

        // A page properties dialog opened before the page became held cannot change it.
        let id = schematic.id.clone();
        let o = a.open.as_mut().unwrap();
        let refused = crate::confirm_page_props(
            o,
            &a.ed,
            &id,
            "99",
            "Renamed",
            schematic.size,
            false,
            &Default::default(),
        );
        assert!(refused.is_err_and(|e| e.contains("Ola")));
        assert_eq!(o.project.pages[0], schematic);
        assert!(status(&a, Action::ArrangePanel), "the panel's own actions work");
        a.ed.panel = None;

        // On another page everything works as before.
        a.ed.page = 1;
        assert!(status(&a, Action::DrawWire));

        // Held too, the second page cannot move up past the first: both numbers stay.
        let second = a.open.as_ref().unwrap().project.pages[1].id.clone();
        a.ed.held_by.insert(second.clone(), "Ola".into());
        assert!(!status(&a, Action::PageUp));
        a.run(Action::PageUp);
        assert_eq!(numbers(&a), before_numbers);

        // Only the neighbour held: the first page cannot move down past it either, as that
        // would renumber the held page.
        a.ed.held_by.clear();
        a.ed.held_by.insert(second, "Ola".into());
        a.open.as_mut().unwrap().held_by_others = a.ed.held_by.clone();
        a.ed.page = 0;
        assert!(!status(&a, Action::PageDown));
        a.run(Action::PageDown);
        assert_eq!(numbers(&a), before_numbers);
        assert!(a.ed.message.contains("Ola"), "{}", a.ed.message);
        assert_eq!(
            a.open.as_mut().unwrap().move_page(0, false),
            None,
            "refused below the menu too"
        );
        assert_eq!(numbers(&a), before_numbers);
        // The assistant's tools say who holds the page instead of claiming the page is at the end
        // or that nothing was pasted.
        let first = before_numbers[0].clone();
        let moved = call(&mut a, "move_page", json!({"page": first, "direction": "down"}));
        assert!(moved.is_err_and(|e| e.contains("Ola")));
        let copied = call(
            &mut a,
            "copy_items",
            json!({"page": first, "to_page": before_numbers[1], "ids": []}),
        );
        assert!(copied.is_err_and(|e| e.contains("Ola")));
        assert_eq!(numbers(&a), before_numbers);
    }

    #[test]
    fn editing_keys_go_to_the_symbol_editor_while_it_is_open() {
        let (_tmp, mut a) = app(vec![]);
        let keys = vec![Action::Undo, Action::Delete, Action::Redo, Action::ZoomIn];
        assert_eq!(
            a.route_to_symbol_editor(keys.clone()),
            keys,
            "closed: the project gets them"
        );

        let mut ed = crate::symbol_editor::SymbolEditor::new(Vec::new());
        ed.set_tool(crate::symbol_editor::Tool::Line);
        ed.click(wc_core::geom::Point::new(0.0, 0.0));
        ed.click(wc_core::geom::Point::new(10.0, 0.0));
        ed.set_tool(crate::symbol_editor::Tool::Select);
        ed.click(wc_core::geom::Point::new(5.0, 0.0));
        assert!(ed.selected.is_some());
        a.symbol_editor = Some(ed);
        let before = a.open.as_ref().unwrap().project.clone();
        assert_eq!(
            a.route_to_symbol_editor(vec![Action::Delete, Action::ZoomIn]),
            vec![Action::ZoomIn]
        );
        assert!(
            a.symbol_editor.as_ref().unwrap().symbol.graphics.is_empty(),
            "the editor deleted"
        );
        assert_eq!(
            a.route_to_symbol_editor(vec![Action::Undo, Action::Copy]),
            Vec::<Action>::new()
        );
        assert_eq!(
            a.symbol_editor.as_ref().unwrap().symbol.graphics.len(),
            1,
            "the editor undid"
        );
        assert_eq!(a.route_to_symbol_editor(vec![Action::Redo]), Vec::<Action>::new());
        assert!(
            a.symbol_editor.as_ref().unwrap().symbol.graphics.is_empty(),
            "the editor redid the delete"
        );
        assert_eq!(a.open.as_ref().unwrap().project, before, "the project was not touched");
    }

    #[test]
    fn the_symbol_editor_closes_when_another_project_is_opened() {
        let (_tmp, mut a) = app(vec![]);
        a.run(Action::NewSymbol);
        assert!(a.symbol_editor.is_some());
        let (_other_tmp, other) = crate::state::tests::project();
        let other_before = other.project.clone();
        a.open = Some(other);
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| a.symbol_editor_window(ctx));
        assert!(a.symbol_editor.is_none(), "it belonged to the first project");
        assert_eq!(a.open.as_ref().unwrap().project, other_before);
    }

    #[test]
    fn find_and_replace_renames_a_device_everywhere_as_one_undo_step() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        let first = open.place(0, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let tag = open.project.pages[0].symbol(&first).unwrap().tag.clone().unwrap();
        let copy = open.place(1, "wc.coil", Point::new(40.0, 20.0)).unwrap();
        open.project.pages[1]
            .symbols
            .iter_mut()
            .find(|s| s.id == copy)
            .unwrap()
            .tag = Some(tag.clone());
        let before = open.project.clone();
        a.run(Action::Find);
        let find = a.find.as_mut().unwrap();
        find.query = tag.to_string();
        find.with = "-K77".into();
        find.match_case = true;
        find.refresh(&a.open.as_ref().unwrap().project);
        assert_eq!(find.hits.len(), 2, "{:?}", find.hits);
        // Only the hit on the second page is ticked: the device is still renamed on both.
        find.chosen = vec![false, true];
        a.replace_found();
        assert!(
            a.find.as_ref().unwrap().message.starts_with("Replaced"),
            "{}",
            a.find.as_ref().unwrap().message
        );
        let open = a.open.as_mut().unwrap();
        let new = wc_core::DeviceTag::parse("-K77").unwrap();
        assert_eq!(open.project.pages[0].symbol(&first).unwrap().tag.as_ref(), Some(&new));
        assert_eq!(open.project.pages[1].symbol(&copy).unwrap().tag.as_ref(), Some(&new));
        assert!(open.undo());
        assert_eq!(open.project, before, "one undo takes the whole replace back");
    }

    #[test]
    fn find_shows_a_hit_on_its_page() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        let id = open.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let page = open.project.pages[1].id.clone();
        a.ed.page = 0;
        let start = wc_core::Endpoint::Point {
            at: Point::new(0.0, 0.0),
        };
        a.ed.wire = Some((start.clone(), Vec::new()));
        a.show_found(&page, Some(Selection::Symbol(id.clone())));
        assert_eq!(a.ed.page, 1);
        assert_eq!(a.ed.selection, vec![Selection::Symbol(id.clone())]);
        assert!(a.ed.wire.is_none(), "a wire begun on another page is dropped");
        // On the same page a wire being drawn is kept.
        a.ed.wire = Some((start, Vec::new()));
        a.show_found(&page, Some(Selection::Symbol(id)));
        assert!(a.ed.wire.is_some());
    }

    #[test]
    fn undo_puts_back_a_symbol_a_command_replaced() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        open.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        open.save().unwrap();
        let old = open.symbols["wc.coil"].clone();
        let old_lock = open.project.lock.symbols["wc.coil"].clone();
        a.assistant.command_base = Some((open.dir.clone(), open.project.clone()));
        // What a command does on disk: a new version of the symbol, and the lock naming it.
        let mut new = old.clone();
        new.name = "Coil, from the command".into();
        wc_core::store::write_vendored_symbol(&open.dir, &new).unwrap();
        let mut on_disk = open.project.clone();
        on_disk.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        wc_core::store::save(&open.dir, &on_disk).unwrap();
        a.command_finish(Ok("Done.".into())).unwrap();
        let open = a.open.as_mut().unwrap();
        assert_eq!(open.symbols["wc.coil"], new);
        assert!(open.undo());
        assert_eq!(open.project.lock.symbols["wc.coil"], old_lock);
        assert_eq!(
            open.symbols["wc.coil"], old,
            "the version from before the command is back"
        );
        assert_eq!(wc_core::store::read_vendored_symbol(&open.dir, "wc.coil").unwrap(), old);
        assert!(open.redo());
        assert_eq!(open.project.lock.symbols["wc.coil"].hash, new.hash());
        assert_eq!(open.symbols["wc.coil"], new);
        assert_eq!(wc_core::store::read_vendored_symbol(&open.dir, "wc.coil").unwrap(), new);
    }

    #[test]
    fn a_command_symbol_update_is_not_loaded_when_the_window_changed_the_project_too() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        open.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        open.save().unwrap();
        let old = open.symbols["wc.coil"].clone();
        let old_lock = open.project.lock.symbols["wc.coil"].clone();
        a.assistant.command_base = Some((open.dir.clone(), open.project.clone()));
        // The command writes a new version of the symbol and the lock naming it.
        let mut new = old.clone();
        new.name = "Coil, from the command".into();
        wc_core::store::write_vendored_symbol(&open.dir, &new).unwrap();
        let mut on_disk = open.project.clone();
        on_disk.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        wc_core::store::save(&open.dir, &on_disk).unwrap();
        // Meanwhile the window changes a page.
        open.project.pages[0].title = "Edited in the window".into();
        assert!(a.command_finish(Ok("Done.".into())).is_err());
        let open = a.open.as_mut().unwrap();
        assert_eq!(open.project.lock.symbols["wc.coil"], old_lock);
        assert_eq!(open.symbols["wc.coil"], old, "the window's definition stays");
        assert_eq!(
            wc_core::store::read_vendored_symbol(&open.dir, "wc.coil").unwrap(),
            old,
            "the file matches the window's lock again"
        );
        open.save().unwrap();
        let saved = Open::load(&open.dir).unwrap();
        assert_eq!(saved.project.lock.symbols["wc.coil"].hash, old.hash());
        assert_eq!(saved.symbols["wc.coil"], old);
    }

    #[test]
    fn a_used_symbol_without_a_lock_entry_is_put_back_when_the_window_changed_the_project_too() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        open.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        // A project from before the lock: the coil is used and loaded, but not pinned.
        open.project.lock.symbols.remove("wc.coil");
        open.save().unwrap();
        let old = open.symbols["wc.coil"].clone();
        a.assistant.command_base = Some((open.dir.clone(), open.project.clone()));
        let mut new = old.clone();
        new.name = "Coil, from the command".into();
        wc_core::store::write_vendored_symbol(&open.dir, &new).unwrap();
        open.project.pages[0].title = "Edited in the window".into();
        let on_disk = {
            let mut p = a.assistant.command_base.as_ref().unwrap().1.clone();
            p.pages[0].title = "Changed by the command".into();
            p
        };
        wc_core::store::save(&a.open.as_ref().unwrap().dir, &on_disk).unwrap();
        assert!(a.command_finish(Ok("Done.".into())).is_err());
        let open = a.open.as_mut().unwrap();
        assert_eq!(open.symbols["wc.coil"], old);
        assert_eq!(wc_core::store::read_vendored_symbol(&open.dir, "wc.coil").unwrap(), old);
        open.save().unwrap();
        assert_eq!(Open::load(&open.dir).unwrap().symbols["wc.coil"], old);
    }

    #[test]
    fn a_symbol_file_that_could_not_be_put_back_is_written_before_the_next_save() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        open.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        open.save().unwrap();
        let old = open.symbols["wc.coil"].clone();
        a.assistant.command_base = Some((open.dir.clone(), open.project.clone()));
        let mut new = old.clone();
        new.name = "Coil, from the command".into();
        wc_core::store::write_vendored_symbol(&open.dir, &new).unwrap();
        let mut on_disk = open.project.clone();
        on_disk.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        wc_core::store::save(&open.dir, &on_disk).unwrap();
        open.project.pages[0].title = "Edited in the window".into();
        // Putting the window's version back fails, and so does the first save's retry.
        open.fail_symbol_writes = [true, true].into();
        let Err(answer) = a.command_finish(Ok("Done.".into())) else {
            unreachable!("both sides changed the project")
        };
        assert!(answer.contains("wc.coil"), "{answer}");
        let open = a.open.as_mut().unwrap();
        assert_eq!(wc_core::store::read_vendored_symbol(&open.dir, "wc.coil").unwrap(), new);
        assert!(open.save().is_err(), "a save waits until the file is right again");
        assert_eq!(
            Open::load(&open.dir).unwrap().project.lock.symbols["wc.coil"].hash,
            new.hash(),
            "nothing was saved"
        );
        open.save().unwrap();
        let saved = Open::load(&open.dir).unwrap();
        assert_eq!(saved.project.lock.symbols["wc.coil"].hash, old.hash());
        assert_eq!(saved.symbols["wc.coil"], old);
        assert_eq!(saved.project.pages[0].title, "Edited in the window");
    }

    #[test]
    fn a_symbol_waiting_to_be_put_back_survives_taking_another_symbol() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        open.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        open.save().unwrap();
        let old = open.symbols["wc.coil"].clone();
        a.assistant.command_base = Some((open.dir.clone(), open.project.clone()));
        let mut new = old.clone();
        new.name = "Coil, from the command".into();
        wc_core::store::write_vendored_symbol(&open.dir, &new).unwrap();
        let mut on_disk = open.project.clone();
        on_disk.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        wc_core::store::save(&open.dir, &on_disk).unwrap();
        open.project.pages[0].title = "Edited in the window".into();
        open.fail_symbol_writes = [true].into();
        assert!(a.command_finish(Ok("Done.".into())).is_err());
        let open = a.open.as_mut().unwrap();
        // Another symbol is taken from the library, which reloads the vendored symbols.
        let lib = wc_library::Library::open(&open.dir, &open.project).unwrap();
        let other = lib
            .index(wc_core::lock::ItemKind::Symbol)
            .into_keys()
            .find(|id| id != "wc.coil")
            .unwrap();
        open.take_saved_symbol(&lib, &other).unwrap();
        assert_eq!(open.symbols["wc.coil"], old, "the window's version stays loaded");
        open.save().unwrap();
        let saved = Open::load(&open.dir).unwrap();
        assert_eq!(saved.project.lock.symbols["wc.coil"].hash, old.hash());
        assert_eq!(saved.symbols["wc.coil"], old);
    }

    /// A second library folder after the starter one, holding its own `wc.coil`.
    fn later_library_with_a_coil(a: &mut App, root: &std::path::Path) -> crate::symbol_editor::Saved {
        let open = a.open.as_mut().unwrap();
        let lib = wc_library::Library::open(&open.dir, &open.project).unwrap();
        let (mut symbol, _) = lib.symbol("wc.coil").unwrap();
        symbol.name = "Coil, mine".into();
        let folder = root.join("mine");
        std::fs::create_dir_all(folder.join("symbols")).unwrap();
        let path = folder.join("symbols/wc.coil.json");
        std::fs::write(&path, serde_json::to_string(&symbol).unwrap()).unwrap();
        open.project.meta.libraries.push(wc_core::project::LibrarySource {
            name: "mine".into(),
            kind: "folder".into(),
            location: folder.display().to_string(),
            reference: None,
        });
        crate::symbol_editor::Saved {
            id: "wc.coil".into(),
            path,
            source: "mine".into(),
            symbol,
        }
    }

    #[test]
    fn a_symbol_saved_behind_an_earlier_library_says_so_whether_or_not_it_is_used() {
        // Not used yet: the place list would give the starter library's coil.
        let root = tempfile::tempdir().unwrap();
        let (_tmp, mut a) = app(vec![]);
        let saved = later_library_with_a_coil(&mut a, root.path());
        a.symbol_saved(saved);
        assert!(a.ed.message.contains("comes first"), "{}", a.ed.message);
        assert!(
            a.ed.message.contains("placing it takes the one from"),
            "{}",
            a.ed.message
        );
        // Used: the project keeps its copy.
        let (_tmp2, mut a) = app(vec![]);
        a.open
            .as_mut()
            .unwrap()
            .place(1, "wc.coil", Point::new(20.0, 20.0))
            .unwrap();
        let before = a.open.as_ref().unwrap().symbols["wc.coil"].clone();
        let saved = later_library_with_a_coil(&mut a, root.path());
        a.symbol_saved(saved);
        assert!(a.ed.message.contains("comes first"), "{}", a.ed.message);
        assert!(a.ed.message.contains("keeps the copy it has"), "{}", a.ed.message);
        assert_eq!(a.open.as_ref().unwrap().symbols["wc.coil"], before);
        // Used, with a copy that matches neither library (an older version): the message
        // still names the project's own copy, not the first library's.
        let (_tmp3, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        open.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        open.symbols.get_mut("wc.coil").unwrap().name = "Coil, older".into();
        let before = open.symbols["wc.coil"].clone();
        let saved = later_library_with_a_coil(&mut a, root.path());
        a.symbol_saved(saved);
        assert!(a.ed.message.contains("keeps the copy it has"), "{}", a.ed.message);
        assert!(!a.ed.message.contains("takes the one from"), "{}", a.ed.message);
        assert_eq!(a.open.as_ref().unwrap().symbols["wc.coil"], before);
    }

    #[test]
    fn a_selection_follows_its_graphic_through_a_session_sync_and_goes_with_its_page() {
        let line = |x: f64| Shape::Line {
            a: Point::new(x, 0.0),
            b: Point::new(x, 5.0),
        };
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        // A third page, so the page under a panel has one after it.
        open.duplicate_page(0).unwrap();
        assert_eq!(open.project.pages.len(), 3);
        open.project.pages[1].graphics = vec![line(1.0), line(2.0)];
        a.share.start_host(open, "Hanna", "hanna@example.com").unwrap();
        a.ed.page = 1;
        // Delete the first line; the second is then graphic 0 and selected.
        let open = a.open.as_mut().unwrap();
        open.checkpoint();
        open.project.pages[1].graphics.remove(0);
        a.sync_session_now(false);
        a.ed.selection = vec![Selection::Graphic(0)];
        // The session's undo puts the first line back in front: the selection follows its line,
        // also with a panel open over the page while a page before it goes.
        a.share.undo().unwrap();
        a.sync_session_now(false);
        assert_eq!(
            a.open.as_ref().unwrap().project.pages[1].graphics,
            vec![line(1.0), line(2.0)]
        );
        assert_eq!(a.ed.selection, vec![Selection::Graphic(1)]);
        // With a panel open over the page, the session takes back a page added before it: the
        // page under the panel is found again at its new place, and so is the selection.
        let open = a.open.as_mut().unwrap();
        let id = open.project.pages[1].id.clone();
        open.checkpoint();
        let mut extra = open.project.pages[0].clone();
        (extra.id, extra.number) = ("p0099".into(), "0".into());
        open.project.pages.insert(0, extra);
        a.sync_session_now(false);
        let pages = &a.open.as_ref().unwrap().project.pages;
        a.ed.page = pages.iter().position(|p| p.id == id).unwrap();
        assert_eq!((a.ed.page, pages[0].id.as_str()), (2, "p0099"));
        a.ed.panel = Some(0);
        a.share.undo().unwrap();
        a.sync_session_now(false);
        assert_eq!(a.open.as_ref().unwrap().project.pages.len(), 3);
        assert_eq!(a.open.as_ref().unwrap().project.pages[a.ed.page].id, id);
        assert_eq!(a.ed.selection, vec![Selection::Graphic(1)]);
        a.ed.panel = None;
        a.ed.page = 0;

        // A page added and shown, something selected on it, and the session takes the page
        // back: nothing stays selected on the page shown instead, though it has a graphic 0 too.
        let open = a.open.as_mut().unwrap();
        open.checkpoint();
        for p in &mut open.project.pages {
            p.graphics.push(line(5.0));
        }
        a.sync_session_now(false);
        let open = a.open.as_mut().unwrap();
        let i = open.duplicate_page(0).unwrap();
        a.sync_session_now(false);
        a.ed.page = i;
        a.ed.selection = vec![Selection::Graphic(0)];
        a.share.undo().unwrap();
        a.sync_session_now(false);
        let pages = &a.open.as_ref().unwrap().project.pages;
        assert_eq!(pages.len(), i);
        assert!(!pages[a.ed.page].graphics.is_empty(), "graphic 0 is there to select");
        assert!(a.ed.selection.is_empty(), "{:?}", a.ed.selection);
    }

    #[test]
    fn undo_in_a_shared_session_goes_through_the_host() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        a.share.start_host(open, "Hanna", "hanna@example.com").unwrap();
        let tag = wc_core::DeviceTag::parse("-K7").unwrap();
        let open = a.open.as_mut().unwrap();
        open.checkpoint();
        open.project.devices.insert(tag.clone(), wc_core::Device::default());
        a.share.frame(a.open.as_mut(), None, false);
        assert!(a.share.can_undo());
        let state: Value = serde_json::from_str(&call(&mut a, "get_state", json!({})).unwrap()).unwrap();
        assert_eq!(state["can_undo"], json!(true), "get_state asks the session");
        a.run(Action::Undo);
        a.share.frame(a.open.as_mut(), None, false);
        assert!(!a.open.as_ref().unwrap().project.devices.contains_key(&tag));
        assert!(a.share.can_redo());
        a.run(Action::Redo);
        a.share.frame(a.open.as_mut(), None, false);
        assert!(a.open.as_ref().unwrap().project.devices.contains_key(&tag));
        // The assistant's undo tool goes the same way.
        assert_eq!(
            call(&mut a, "undo", json!({})),
            Ok("Sent to the session; it shows with the next sync.".into())
        );
        a.share.frame(a.open.as_mut(), None, false);
        assert!(!a.open.as_ref().unwrap().project.devices.contains_key(&tag));
        call(&mut a, "undo", json!({"redo": true})).unwrap();
        a.share.frame(a.open.as_mut(), None, false);
        assert!(a.open.as_ref().unwrap().project.devices.contains_key(&tag));
        // A change the assistant made earlier in the same answer (no frame in between) is the
        // one its undo takes back.
        let k8 = wc_core::DeviceTag::parse("-K8").unwrap();
        let open = a.open.as_mut().unwrap();
        open.checkpoint();
        open.project.devices.insert(k8.clone(), wc_core::Device::default());
        call(&mut a, "undo", json!({})).unwrap();
        a.share.frame(a.open.as_mut(), None, false);
        let devices = &a.open.as_ref().unwrap().project.devices;
        assert!(!devices.contains_key(&k8) && devices.contains_key(&tag));
        // The host's refusal comes back to the caller.
        call(&mut a, "undo", json!({"redo": true})).unwrap();
        a.share.frame(a.open.as_mut(), None, false);
        assert!(a.share.redo().is_err_and(|e| e.starts_with("Nothing was taken back")));
        a.share.stop();
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
    fn page_content_without_title_block_keeps_the_pages_own_fields() {
        let (_tmp, mut a) = app(vec![]);
        if let Some(p) = a.open.as_mut().unwrap().project.page_mut("p0002") {
            p.title_block.insert("revision".into(), "B".into());
        }
        let mut content: Value =
            serde_json::from_str(&call(&mut a, "get_page", json!({"page": "p0002"})).unwrap()).unwrap();
        assert_eq!(content["title_block"]["revision"], json!("B"), "get_page lists them");
        content.as_object_mut().unwrap().remove("title_block");
        content["title"] = json!("Changed");
        call(
            &mut a,
            "edit_page_json",
            json!({"page": "p0002", "content": content.clone()}),
        )
        .unwrap();
        assert_eq!(page2(&a).title, "Changed");
        assert_eq!(page2(&a).title_block["revision"], "B", "kept");
        // Given, they are used: an empty map clears them.
        content["title_block"] = json!({});
        call(&mut a, "edit_page_json", json!({"page": "p0002", "content": content})).unwrap();
        assert!(page2(&a).title_block.is_empty());
    }

    #[test]
    fn the_assistant_sets_and_clears_a_pages_own_title_fields() {
        let (_tmp, mut a) = app(vec![]);
        call(
            &mut a,
            "update_page",
            json!({"page": "p0002", "title_block": {"revision": "C", "checked_by": "OH"}}),
        )
        .unwrap();
        assert_eq!(page2(&a).title_block["revision"], "C");
        let content: Value =
            serde_json::from_str(&call(&mut a, "get_page", json!({"page": "p0002"})).unwrap()).unwrap();
        assert_eq!(content["title_block"]["checked_by"], json!("OH"), "get_page lists them");
        // Null or an empty text clears a field; one left out is kept.
        call(
            &mut a,
            "update_page",
            json!({"page": "p0002", "title_block": {"revision": null}}),
        )
        .unwrap();
        assert_eq!(page2(&a).title_block.get("revision"), None);
        assert_eq!(page2(&a).title_block["checked_by"], "OH");
        let refused = call(
            &mut a,
            "update_page",
            json!({"page": "p0002", "title_block": {"date": 5}}),
        );
        assert!(refused.is_err_and(|e| e.contains("title_block.date")));
        assert_eq!(
            page2(&a).title_block["checked_by"],
            "OH",
            "a refused change changes nothing"
        );

        // In a shared session, a page someone else has open is refused, fields and all.
        a.open
            .as_mut()
            .unwrap()
            .held_by_others
            .insert("p0002".into(), "Ola".into());
        let before = page2(&a).clone();
        let refused = call(
            &mut a,
            "update_page",
            json!({"page": "p0002", "title": "Mine", "title_block": {"revision": "D"}}),
        );
        assert!(refused.is_err_and(|e| e.contains("Ola")));
        assert_eq!(page2(&a), &before);
    }

    #[test]
    fn the_assistant_reads_and_sets_the_cover_page_and_page_content_keeps_it() {
        let (_tmp, mut a) = app(vec![]);
        let covers = |a: &App| -> Vec<String> {
            a.open
                .as_ref()
                .unwrap()
                .project
                .pages
                .iter()
                .filter(|p| p.cover)
                .map(|p| p.id.clone())
                .collect()
        };
        call(&mut a, "update_page", json!({"page": "p0002", "cover": true})).unwrap();
        assert_eq!(covers(&a), ["p0002"]);
        let state: Value = serde_json::from_str(&call(&mut a, "get_state", json!({})).unwrap()).unwrap();
        let listed = state["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "p0002")
            .unwrap();
        assert_eq!(listed["cover"], json!(true));
        // Page content without `cover` keeps the page the cover page.
        let mut content: Value =
            serde_json::from_str(&call(&mut a, "get_page", json!({"page": "p0002"})).unwrap()).unwrap();
        assert_eq!(content["cover"], json!(true));
        content.as_object_mut().unwrap().remove("cover");
        content["title"] = json!("Front");
        call(&mut a, "edit_page_json", json!({"page": "p0002", "content": content})).unwrap();
        assert_eq!(covers(&a), ["p0002"]);
        assert_eq!(page2(&a).title, "Front");
        // A new cover takes the mark from the old one, as one undo step.
        let out = call(&mut a, "add_page", json!({"title": "Second"})).unwrap();
        let new_id = out.rsplit("id ").next().unwrap().trim_end_matches('.').to_owned();
        call(&mut a, "update_page", json!({"page": new_id, "cover": true})).unwrap();
        assert_eq!(covers(&a), std::slice::from_ref(&new_id));
        a.open.as_mut().unwrap().undo();
        assert_eq!(covers(&a), ["p0002"]);
        // Page content with `cover: true` on a normal page takes the mark from the cover page,
        // and one undo gives it back.
        let mut content: Value =
            serde_json::from_str(&call(&mut a, "get_page", json!({"page": new_id})).unwrap()).unwrap();
        assert_eq!(content["cover"], json!(false));
        content["cover"] = json!(true);
        call(&mut a, "edit_page_json", json!({"page": new_id, "content": content})).unwrap();
        assert_eq!(covers(&a), std::slice::from_ref(&new_id));
        a.open.as_mut().unwrap().undo();
        assert_eq!(covers(&a), ["p0002"]);
        // `cover: false` makes it a normal page.
        call(&mut a, "update_page", json!({"page": "p0002", "cover": false})).unwrap();
        assert!(covers(&a).is_empty());
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
    fn undo_from_the_toolbar_first_sends_what_was_not_synced() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        a.share.start_host(open, "Hanna", "hanna@example.com").unwrap();
        let k7 = wc_core::DeviceTag::parse("-K7").unwrap();
        let k8 = wc_core::DeviceTag::parse("-K8").unwrap();
        let open = a.open.as_mut().unwrap();
        open.project.devices.insert(k7.clone(), wc_core::Device::default());
        a.share.frame(a.open.as_mut(), None, false);
        // -K8 is made with no frame before Undo: Undo takes back -K8, not -K7.
        let open = a.open.as_mut().unwrap();
        open.project.devices.insert(k8.clone(), wc_core::Device::default());
        a.run(Action::Undo);
        a.share.frame(a.open.as_mut(), None, false);
        let devices = &a.open.as_ref().unwrap().project.devices;
        assert!(!devices.contains_key(&k8) && devices.contains_key(&k7));
        a.share.stop();
    }

    #[test]
    fn undo_in_a_session_waits_while_an_assistant_command_owns_the_files() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        a.share.start_host(open, "Hanna", "hanna@example.com").unwrap();
        let k7 = wc_core::DeviceTag::parse("-K7").unwrap();
        let open = a.open.as_mut().unwrap();
        open.project.devices.insert(k7.clone(), wc_core::Device::default());
        a.share.frame(a.open.as_mut(), None, false);
        let open = a.open.as_ref().unwrap();
        a.assistant.command_base = Some((open.dir.clone(), open.project.clone()));
        let on_disk = std::fs::read_dir(&open.dir).unwrap().count();
        let dir = open.dir.clone();
        for action in [Action::Undo, Action::Redo] {
            a.run(action);
            assert!(a.ed.message.contains("wait"), "{}", a.ed.message);
        }
        assert!(!a.share.take_notes().iter().any(|n| n.contains("Undo")));
        assert!(
            a.open.as_ref().unwrap().project.devices.contains_key(&k7),
            "nothing taken back"
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), on_disk);
        a.assistant.command_base = None;
        a.run(Action::Undo);
        a.share.frame(a.open.as_mut(), None, false);
        assert!(
            !a.open.as_ref().unwrap().project.devices.contains_key(&k7),
            "taken back once free"
        );
        a.share.stop();
    }

    #[test]
    fn a_page_edit_made_right_after_an_undo_does_not_bring_back_what_it_took_back() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        a.share.start_host(open, "Hanna", "hanna@example.com").unwrap();
        let shown = Some("p0002".to_owned());
        a.share.frame(a.open.as_mut(), shown.clone(), false);
        let original = page2(&a).title.clone();
        a.open.as_mut().unwrap().project.page_mut("p0002").unwrap().title = "First".into();
        a.share.frame(a.open.as_mut(), shown.clone(), false);
        assert!(a.share.undo().is_ok());
        // Edited before the frame that brings the undo's result: built on the old page.
        a.open.as_mut().unwrap().project.page_mut("p0002").unwrap().title = "First, then more".into();
        a.share.frame(a.open.as_mut(), shown.clone(), false);
        assert_eq!(page2(&a).title, original, "the undo is kept");
        a.share.frame(a.open.as_mut(), shown, false);
        assert_eq!(page2(&a).title, original, "and not sent over afterwards");
        assert!(
            a.share.take_notes().iter().any(|n| n.contains("was not kept")),
            "the dropped change is reported"
        );
        a.share.stop();
    }

    #[test]
    fn undo_in_a_shared_session_does_not_take_back_work_from_before_it() {
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
        let state: Value = serde_json::from_str(&call(&mut a, "get_state", json!({})).unwrap()).unwrap();
        assert_eq!(
            state["can_undo"],
            json!(false),
            "work from before the session is not undone"
        );
        for redo in [false, true] {
            assert_eq!(
                call(&mut a, "undo", json!({"redo": redo})),
                Ok("Nothing to undo or redo.".into())
            );
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
