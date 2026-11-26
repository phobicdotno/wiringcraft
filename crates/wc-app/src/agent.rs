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

use std::collections::{BTreeMap, HashMap};
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
A4 to A0, landscape (A4 297x210, A3 420x297 is the default, A2 594x420) or portrait (A4 portrait 210x297), or a \
custom size; get_page gives the page's size_mm, which is the sheet to place things on. The frame is 10 mm in from the edge and divided into 10 \
reference columns (1-10 along the top) and rows (A-F up to 297 mm high, more on taller sheets, down both sides); cross-references \
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
    /// The free drawings of each page as the assistant last read them (project folder and page
    /// id). Drawings are addressed by position (`g0`, `g1` ...), so a write by position is
    /// refused once someone else changed the page's drawings (a session sync, the user).
    /// Shared, so the copy a tool call keeps to put back costs nothing.
    graphics_read: HashMap<(PathBuf, String), Arc<[Shape]>>,
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
            graphics_read: HashMap::new(),
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
    let reports: Vec<&str> = wc_report::Kind::ALL.iter().map(|k| k.name()).collect();
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
            "Replace a free drawing (line, polyline, rect, circle, arc, text, dimension) by its id from get_page (g0, g1 ...). Ids are positions: a write is refused when the page's drawings changed since get_page, so read it again then.",
            json!({"id": {"type": "string"}, "shape": shape_arg(), "page": page_arg()}),
            &["id", "shape"],
        ),
        tool(
            "add_graphic",
            "Add a free drawing to a page: a line, polyline (closed or filled), rectangle, circle, arc, text (labels, notes, frames, boxes around a group) or dimension (a measured distance in mm with arrows and extension lines).",
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
            json!({"number": {"type": "string"}, "title": {"type": "string"}, "size": {"type": "string", "description": "A4, A3, A2, A1 or A0 (landscape), the same with -portrait (A4-portrait), or WIDTHxHEIGHT in millimetres (500x700)"}}),
            &[],
        ),
        tool(
            "update_page",
            "Change a page's number, title or sheet size, mark it as an overview (a line diagram: its devices do not count as drawings in the project check), make it the project's cover page (it shows the project's name and fields; any other cover page loses the mark), or set the page's own title-block fields (get_page lists them).",
            json!({"page": page_arg(), "number": {"type": "string"}, "title": {"type": "string"}, "size": {"type": "string", "description": "A4, A3, A2, A1 or A0 (landscape), the same with -portrait (A4-portrait), or WIDTHxHEIGHT in millimetres (500x700)"}, "overview": {"type": "boolean"}, "cover": {"type": "boolean"}, "title_block": {"type": "object", "additionalProperties": {"type": ["string", "null"]}, "description": "The page's own title-block fields to change, such as revision, date, drawn_by, checked_by. They win over the project's on this page. An empty text or null clears a field so the project's shows again; fields left out are kept."}}),
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
            "plc_racks",
            "PLC racks and the I/O cards in their slots, as the PLC racks form edits them (kept in project.toml, laid out on the PLC cards report). get returns them as JSON: a list of racks, each with name, description and card (its cards: slot, tag, type, channels, start address such as %IX0.0 or %IW64, and step when the channels are numbered with a step of their own). replace sets the whole list (in get's format; an empty list removes every rack) as one undo step, and answers with the PLC cards report's warnings about the result.",
            json!({
                "action": {"type": "string", "enum": ["get", "replace"]},
                "racks": {"type": "array", "items": {"type": "object"}, "description": "replace: every rack, as get returns them."}
            }),
            &["action"],
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
            "strips",
            "Terminal strips as the terminal strip editor arranges them: list them (each strip's terminals in rail order, with spares and levels marked), move a terminal to another place on the rail (1 is first), add a spare terminal (after a terminal, or at the end), remove a spare, set a terminal's level (1 is the bottom level of a multi-level terminal; null clears it), or reset a strip to the order by number. Each change is one undo step.",
            json!({
                "action": {"type": "string", "enum": ["list", "move", "spare", "unspare", "level", "reset"]},
                "strip": {"type": "string", "description": "The strip's tag, e.g. -X1 (optional for list)."},
                "terminal": {"type": "string", "description": "A terminal number on the strip, e.g. 3 or S1 (move, spare, unspare, level)."},
                "to": {"type": "integer", "minimum": 1, "description": "The place on the rail to move the terminal to, 1 is first (move)."},
                "after": {"type": "string", "description": "The terminal the new spare goes right after; at the end when left out (spare)."},
                "level": {"type": ["integer", "null"], "minimum": 1, "maximum": u32::MAX, "description": "The terminal's level, or null to clear it (level)."}
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
            "Macros (saved groups of symbols, wires and drawings): list them, insert one on a page (its top-left corner at x, y), or save items from a page as a new project macro. A macro with value sets (variants, such as sizes of a motor starter) needs value_set when inserted; list shows them.",
            json!({
                "action": {"type": "string", "enum": ["list", "insert", "save"]},
                "name": {"type": "string"}, "description": {"type": "string"},
                "value_set": {"type": "string", "description": "insert: which of the macro's value sets fills its placeholders."},
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
            "Check the project: devices drawn twice, contacts without a coil, shorted potentials, open pins, wires ending in the air, devices without a part, then the check rules of the project's library sources (less the ones the project turns off) and the project's own check rules (see the rules tool).",
            json!({}),
            &[],
        ),
        tool(
            "rules",
            "The project's check rules, which run_check runs: list (the project's own rules, each library source's rules numbered from 1 and whether the project turns it off, and rules_off), set (replace the project's own rules with `rules`, a list in order; send the whole list), off or on (turn a library rule off or back on by `rule`, written source:number or source:name). A rule has kind (cable: the devices have a cable on a pin; tag: their tags match `pattern`, where * is any run, ? one character, # a number; max-wires: at most `max` wire ends on one pin, terminals unless devices or role says otherwise), and optionally devices (tag pattern such as -M*), role (main, auxiliary, standalone, terminal, interruption), severity (error, warning, note; default warning), message ({tag} stands for the tag) and name.",
            json!({
                "action": {"type": "string", "enum": ["list", "set", "off", "on"]},
                "rules": {"type": "array", "items": {"type": "object", "properties": {
                    "kind": {"type": "string", "enum": wc_core::check_rule::KINDS},
                    "devices": {"type": "string"}, "role": {"type": "string"}, "pattern": {"type": "string"},
                    "max": {"type": "integer"}, "severity": {"type": "string"}, "message": {"type": "string"},
                    "name": {"type": "string"}
                }, "required": ["kind"]}},
                "rule": {"type": "string", "description": "off/on: the library rule, as source:number or source:name."}
            }),
            &["action"],
        ),
        tool(
            "get_report",
            "A report as a Markdown table.",
            json!({"kind": {"type": "string", "enum": reports}}),
            &["kind"],
        ),
        tool(
            "report_layouts",
            "Report layouts (as the report layout dialog edits them): get (each report's layout as JSON, null when built in, with its built-in columns and what in the layout names nothing; one report with kind), set (replace one report's layout) or reset (back to the built-in layout). A layout is {title, column: [{from, heading}], sort, group, header}: column lists the shown columns in order by their built-in heading (empty keeps them all), sort names columns with - in front for descending, group a column to group rows by, header title-block keys printed above the table (project.name, revision ...). Each change is one undo step.",
            json!({
                "action": {"type": "string", "enum": ["get", "set", "reset"]},
                "kind": {"type": "string", "enum": reports, "description": "The report (optional for get)."},
                "layout": {"type": "object", "description": "set: the report's whole layout, in get's format."}
            }),
            &["action"],
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
            "Export into the project's export folder: pdf (all pages and reports), drawings (SVG and DXF per page), aml (AutomationML), wago-marking (WAGO smartScript marker list CSV) or wago-configurator (terminal strips as a WAGO configurator article list CSV).",
            json!({
                "format": {"type": "string", "enum": ["pdf", "drawings", "aml", "wago-marking", "wago-configurator"]},
                "lang": {"type": "string", "description": "pdf: print the texts in this language from translations.toml, e.g. \"nb\", or two as \"nb,en\" (default: as drawn). wago-marking: language of the header row and type column: en (default), de or nb."},
                "kinds": {"type": "array", "items": {"type": "string", "enum": ["terminals", "wires", "devices", "cables"]}, "description": "wago-marking: only these marker kinds (default: all)."},
                "lines": {"type": "array", "items": {"type": "string", "enum": ["marker", "tag", "description", "location"]}, "description": "wago-marking: the text lines of each marker, one column each (default: marker)."},
                "strip": {"type": "string", "description": "wago-marking: only this terminal strip's terminals, e.g. \"-X1\"."}
            }),
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
            "Open a project folder, or create a new one (a git repository with the starter library, or a copy of a base project when base is given).",
            json!({"path": {"type": "string"}, "create": {"type": "boolean"}, "name": {"type": "string"}, "base": {"type": "string", "description": "create: a base project folder or git URL to start from (its pages, settings, title block and libraries)."}}),
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
            "Run a wiringcraft command-line command on the open project (unsaved changes are saved first, and the project is reloaded after). Covers everything. \
             Project and pages: new (a project folder, --template), page (add, list, remove, overview, size, cover, fields), template (show, columns, reset: frame columns and title block), logo (show, set, remove), location (list, set, remove). \
             Drawing: place, box (a black box with connection points, --pin name:side:offset), wire, device (part, description), number-wires, macro (list, place), find (text in pages, --replace, --match-case). \
             Terminals: strip (list, order, move, spare, unspare, level), jumper (add, remove, part, list). \
             Revisions: revision (list, add, edit, remove, cloud, uncloud, table). \
             Panels: panel (new, arrange, step, device, show, list, remove). \
             Libraries: lib (add, sync, status, update, lock, search, propose). \
             Import: import (symbol-dxf, symbol-svg, symbol-kicad, symbol-qet, footprint-dxf, model-step, parts-csv, parts-bmecat, parts-edz, plc-csv, wago, archive, page-dxf, page-dwg, pdf, eplan, swe, swe-library, aml). \
             Output: pdf (--page, --report, --lang for a translated print), drawing (SVG/DXF per page), report, nets, aml, plc-vars, export (wago-marking, wago-configurator), translations (texts missing a translation, --lang). \
             Checks and history: check, inspect (another tool's archive), vcs (init, commit, branch, switch, merge, log, changes, page-log), comment (add, reply, resolve, reopen, remove, list). \
             Use [\"--help\"] or [\"<command>\", \"--help\"] for the arguments.",
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
        "description": "{\"kind\": \"line\", \"a\": {x,y}, \"b\": {x,y}} | {\"kind\": \"polyline\", \"points\": [...], \"closed\": bool, \"filled\": bool (solid, implies closed)} | {\"kind\": \"rect\", \"min\": {x,y}, \"max\": {x,y}} | {\"kind\": \"circle\", \"center\": {x,y}, \"radius\": r, \"filled\": bool} | {\"kind\": \"arc\", \"center\": {x,y}, \"radius\": r, \"start\": deg, \"end\": deg} | {\"kind\": \"text\", \"at\": {x,y}, \"text\": \"...\", \"height\": 2.5, \"align\": \"left|center|right\", \"angle\": deg} | {\"kind\": \"dimension\", \"a\": {x,y}, \"b\": {x,y}, \"offset\": mm (dimension line to the left of a to b, negative: right), \"height\": 2.5}",
        "properties": {"kind": {"type": "string", "enum": ["line", "polyline", "rect", "circle", "arc", "text", "dimension"]}},
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

/// Terminal strips as the assistant sees them: each strip (only `only`, when given) with its
/// terminals in rail order, a spare marked, and the level the terminal plan shows if any.
fn strip_list(open: &Open, only: Option<&DeviceTag>) -> Vec<Value> {
    open.strip_terminals()
        .into_iter()
        .filter(|(strip, _)| only.is_none_or(|o| o == strip))
        .map(|(strip, rail)| {
            let levels = crate::strip_editor::rail_levels(open, &strip, &rail);
            let terminals: Vec<Value> = rail
                .iter()
                .map(|(n, spare)| json!({"terminal": n, "spare": spare, "level": levels.get(n)}))
                .collect();
            json!({"strip": strip.to_string(), "terminals": terminals})
        })
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

/// A list of strings under `key`: left out (or null) is empty, anything else but strings is an
/// error that names the value.
fn string_list(v: &Value, key: &str) -> Result<Vec<String>, String> {
    let Some(items) = v.get(key).filter(|x| !x.is_null()) else {
        return Ok(Vec::new());
    };
    items
        .as_array()
        .ok_or_else(|| format!("`{key}` must be a list of strings, not {items}."))?
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("`{key}` holds {x}, which is not a string in quotes."))
        })
        .collect()
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
        Some(t) => PageSize::parse(t).map(Some),
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

/// The id the assistant is given for a revision cloud: what it is (its revision and corners),
/// not where it is in the page's list, so an id read before someone else removed a cloud
/// cannot name another one.
fn cloud_id(c: &wc_core::revision::RevisionCloud) -> String {
    format!(
        "cloud {} at {},{} to {},{}",
        c.revision.trim(),
        c.min.x,
        c.min.y,
        c.max.x,
        c.max.y
    )
}

/// [`selection_id`] for an item on `page`, a revision cloud by [`cloud_id`].
fn item_id(page: Option<&Page>, sel: &Selection) -> String {
    match (sel, page) {
        (Selection::Cloud(i), Some(p)) => p.clouds.get(*i).map(cloud_id).unwrap_or_else(|| selection_id(sel)),
        _ => selection_id(sel),
    }
}

fn selection_id(sel: &Selection) -> String {
    match sel {
        Selection::Symbol(id) | Selection::Wire(id) => id.clone(),
        Selection::Graphic(i) => format!("g{i}"),
        Selection::Cloud(i) => wc_report::cloud_item(*i),
        Selection::RevisionTable => "revision table".to_owned(),
    }
}

/// The open project, or the error the model sees when there is none.
fn need_open(open: Option<&mut Open>) -> Result<&mut Open, String> {
    open.ok_or_else(|| "No project is open. Use open_project first.".to_owned())
}

/// An error naming the first key of a report layout (or of one of its column entries) that
/// [`wc_core::report_layout::ReportLayout`] does not have, since serde would drop it silently
/// (`columns` for `column` would then keep the built-in columns). The known keys are the ones
/// the layout itself serializes, so they follow its fields.
fn report_layout_unknown_key(layout: &serde_json::Map<String, Value>) -> Result<(), String> {
    use wc_core::report_layout::{ReportColumn, ReportLayout};
    let keys = |v: Value| -> Vec<String> { v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default() };
    let column = ReportColumn {
        from: "x".into(),
        heading: Some("x".into()),
    };
    let known = keys(json!(ReportLayout {
        kind: "x".into(),
        title: Some("x".into()),
        columns: vec![column.clone()],
        sort: vec!["x".into()],
        group: Some("x".into()),
        header: vec!["x".into()],
    }));
    let known_column = keys(json!(column));
    let unknown = |fields: &serde_json::Map<String, Value>, known: &[String], what: &str| match fields
        .keys()
        .find(|k| !known.contains(k))
    {
        Some(k) => Err(format!(
            "Unknown {what} key {k:?}; known keys are {}.",
            known.join(", ")
        )),
        None => Ok(()),
    };
    unknown(layout, &known, "layout")?;
    if let Some(Value::Array(columns)) = layout.get("column") {
        for c in columns.iter().filter_map(Value::as_object) {
            unknown(c, &known_column, "column")?;
        }
    }
    Ok(())
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

    /// Item ids on a page as selections; unknown ids are an error. A revision cloud is named by
    /// what it is ([`cloud_id`]); two clouds alike cannot be told apart, so neither is taken.
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
                } else if id.trim() == selection_id(&Selection::RevisionTable) && p.revision_table.is_some() {
                    Ok(Selection::RevisionTable)
                } else if id.trim().starts_with("cloud ")
                    && let matching = p
                        .clouds
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| cloud_id(c) == id.trim())
                        .map(|(i, _)| i)
                        .collect::<Vec<_>>()
                    && let Some(&i) = matching.first()
                {
                    if matching.len() == 1 {
                        Ok(Selection::Cloud(i))
                    } else {
                        Err(format!(
                            "{id:?}: {} clouds are alike, so which one is not known.",
                            matching.len()
                        ))
                    }
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
            "clouds": p.clouds, "revision_table": p.revision_table,
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
        self.note_device_rename(from, to);
        Ok(self.project.rename_device(from, to))
    }

    /// Make every symbol a page uses available in the project, or none of them.
    fn ensure_page_symbols(&mut self, page: &Page) -> Result<(), String> {
        self.ensure_symbols(page.symbols.iter().map(|s| s.symbol.as_str()))
    }
}

/// The app as the assistant's toolbox.
pub struct AppTools<'a>(pub &'a mut App);

/// Whether tool `name` with input `v` changes the page its `page` field names (the shown page
/// without one). Tools that only read, select, show or comment do not.
fn edits_page(name: &str, v: &Value) -> bool {
    match name {
        "place_symbol" | "add_wire" | "update_symbol" | "update_wire" | "add_graphic" | "update_graphic"
        | "delete_items" | "move_items" | "rotate_items" | "mirror_items" | "delete_page" | "edit_page_json" => true,
        "macros" => s(v, "action") == Some("insert"),
        _ => false,
    }
}

/// Whether a tool names a free drawing by its position (`g0`, `g1` ...) on a page.
fn names_graphics(name: &str, v: &Value) -> bool {
    // The same rule as `Open::selections` reads them by.
    let positional = |id: &str| id.strip_prefix('g').and_then(|n| n.parse::<usize>().ok()).is_some();
    match name {
        "update_graphic" => true,
        "delete_items" | "move_items" | "rotate_items" | "mirror_items" | "copy_items" | "select_items" => {
            strings(v, "ids").iter().any(|id| positional(id))
        }
        "macros" if s(v, "action") == Some("save") => strings(v, "ids").iter().any(|id| positional(id)),
        _ => false,
    }
}

/// What [`AppTools::held_check`] checks a tool's change against, and what it puts back.
struct HeldGuard {
    dir: PathBuf,
    held: Vec<Page>,
    mark: crate::state::Mark,
    page: usize,
    selection: Vec<Selection>,
    panel: Option<usize>,
    /// Whether a cover or template form was open before: one the change opened goes with it.
    had_form: bool,
}

impl AppTools<'_> {
    /// In a shared session, what a tool's change is checked against: the pages someone else
    /// holds, and the state to go back to. `None` when nobody holds a page; for undo and redo
    /// (the tool, or the action), which the session's host carries out and checks itself; and
    /// for `open_project`, which replaces the project instead of changing it.
    /// In a guest's window, the open project's folder and project, to tell after a tool
    /// whether it changed the project (a tool can take an undo step and change nothing).
    fn guest_snapshot(&self) -> Option<(PathBuf, wc_core::Project)> {
        if !self.0.share.active() || self.0.share.is_host() {
            return None;
        }
        self.0.open.as_ref().map(|o| (o.dir.clone(), o.project.clone()))
    }

    /// A guest's change is only kept once the host accepts it: the host refuses it when
    /// someone else has its page open by then, which the guest may not know of yet. A tool
    /// that changed the project (since the [`AppTools::guest_snapshot`] `before`) says so in
    /// its answer.
    fn pending_note(
        &self,
        before: Option<(PathBuf, wc_core::Project)>,
        r: Result<String, String>,
    ) -> Result<String, String> {
        let changed = before.is_some_and(|(dir, project)| {
            self.0
                .open
                .as_ref()
                .is_some_and(|o| o.dir == dir && o.project != project)
        });
        if !changed {
            return r;
        }
        // Also on an error that left a change behind (a command that failed partway).
        let note = |t: String| {
            format!(
                "{t}\n\nPending: in this shared session the change is kept only once the host accepts it. \
                 If someone else opened the page meanwhile, the host refuses it and it is taken back here; \
                 read the page again before building on it."
            )
        };
        r.map(note).map_err(note)
    }

    fn held_guard(&mut self, name: &str, input: &Value) -> Option<HeldGuard> {
        let undoes = match name {
            "undo" | "open_project" => true,
            "run_action" => s(input, "action").is_some_and(|a| {
                [Action::Undo, Action::Redo]
                    .iter()
                    .any(|u| u.name().eq_ignore_ascii_case(a))
            }),
            _ => false,
        };
        let (page, selection, panel) = (self.0.ed.page, self.0.ed.selection.clone(), self.0.ed.panel);
        let had_form = matches!(self.0.dialog, Some(Dialog::Cover { .. } | Dialog::Template { .. }));
        let open = self.0.open.as_mut()?;
        let held = open.held_pages();
        (!held.is_empty() && !undoes).then(|| HeldGuard {
            dir: open.dir.clone(),
            held,
            mark: open.mark(),
            page,
            selection,
            panel,
            had_form,
        })
    }

    /// A page someone else holds is read-only here, whatever the tool did: a change that
    /// reached one (a rename that also renames symbols there, a cover flag moved off it) is
    /// taken back as a whole, with the page shown and the selection, and refused with who
    /// holds the page, as the host would refuse it. The tool's own error, if any, is kept.
    fn held_check(&mut self, guard: Option<HeldGuard>, r: Result<String, String>) -> Result<String, String> {
        let Some(g) = guard else {
            return r;
        };
        let Some(open) = self.0.open.as_mut() else {
            return r;
        };
        // Another project opened in between is not this project's change.
        if open.dir != g.dir {
            return r;
        }
        let Some(why) = open.held_change(&g.held) else {
            open.keep(g.mark);
            return r;
        };
        let files = open.back_to(g.mark).err();
        (self.0.ed.page, self.0.ed.selection, self.0.ed.panel) = (g.page, g.selection, g.panel);
        // A form the refused change opened (the cover page made from a held page) edits what
        // was taken back: it closes, and the panel shown before stays shown.
        if !g.had_form && matches!(self.0.dialog, Some(Dialog::Cover { .. } | Dialog::Template { .. })) {
            self.0.dialog = None;
        }
        let mut msg = match r {
            Ok(_) => format!("Not done: {why}"),
            Err(e) => format!("Not done: {why} The tool also said: {e}"),
        };
        if let Some(f) = files {
            msg.push(' ');
            msg.push_str(&f);
        }
        Err(msg)
    }
}

impl wc_agent::Tools for AppTools<'_> {
    fn specs(&self) -> Vec<Value> {
        self.0.assistant.specs().to_vec()
    }

    fn run(&mut self, name: &str, input: &Value) -> Result<String, String> {
        // The previous tool's change is followed first, so this one sees the jumpers right.
        if let Some(o) = self.0.open.as_mut() {
            o.follow_terminals();
        }
        let mark = self.guest_snapshot();
        let guard = self.held_guard(name, input);
        let read = self.0.assistant.graphics_read.clone();
        let r = self.0.agent_tool(name, input);
        let r = self.held_check(guard, r);
        // A change taken back is not what the assistant knows of the drawings either.
        if r.is_err() {
            self.0.assistant.graphics_read = read;
        }
        // Items the assistant removed must not stay selected.
        self.0.prune_selection();
        self.pending_note(mark, r)
    }

    fn start(&mut self, name: &str, input: &Value) -> Started {
        // As in `run`: the previous tool's change is followed before this one starts.
        if let Some(o) = self.0.open.as_mut() {
            o.follow_terminals();
        }
        let mark = self.guest_snapshot();
        let guard = self.held_guard(name, input);
        let read = self.0.assistant.graphics_read.clone();
        match self.0.agent_start(name, input) {
            Started::Done(r) => {
                let r = self.held_check(guard, r);
                if r.is_err() {
                    self.0.assistant.graphics_read = read;
                }
                self.0.prune_selection();
                Started::Done(self.pending_note(mark, r))
            }
            running => {
                // A tool that runs on in the background is not checked here (its result is
                // applied later); the undo history set aside for the check goes back now.
                if let (Some(g), Some(open)) = (guard, self.0.open.as_mut())
                    && open.dir == g.dir
                {
                    open.keep(g.mark);
                }
                running
            }
        }
    }

    fn finish(&mut self, name: &str, input: &Value, result: Result<String, String>) -> Result<String, String> {
        // As in `run`: the window's pending change is followed first, so the jumpers it moves
        // are not taken for the finish's own change.
        if let Some(o) = self.0.open.as_mut() {
            o.follow_terminals();
        }
        // Only what the finish itself changes: what the tool did when it started was told then.
        let mark = self.guest_snapshot();
        let r = self.0.agent_finish(name, input, result);
        self.0.prune_selection();
        self.pending_note(mark, r)
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
            "comments" | "macros" | "panels" | "library" | "versions" | "jumpers" | "strips" | "report_layouts" => {
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
            "plc_racks" => format!("PLC racks: {}", s(input, "action").unwrap_or_default()),
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
            let page = if self.ed.panel.is_none() {
                open.project.pages.get(self.ed.page)
            } else {
                None
            };
            let ids: Vec<String> = self.ed.selection.iter().map(|s| item_id(page, s)).collect();
            out.push_str(&format!(". Selected: {}", ids.join(", ")));
        }
        out.push('.');
        out
    }

    /// [`App::agent_context`] for a message on its way: a selected free drawing is named by
    /// position there, so the drawings of the page shown count as read from then on, unless
    /// they changed since the assistant last read them.
    pub(crate) fn agent_context_to_send(&mut self) -> String {
        let context = self.agent_context();
        if self.ed.panel.is_none()
            && self.ed.selection.iter().any(|s| matches!(s, Selection::Graphic(_)))
            && let Some(key) = self.graphics_key(&Value::Null, "page")
            && self.graphics_as_read(&key)
            && let Some(now) = self.graphics_now(&key).map(Arc::<[Shape]>::from)
        {
            self.assistant.graphics_read.insert(key, now);
        }
        context
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
            Selection::Cloud(i) => *i < p.clouds.len(),
            Selection::RevisionTable => p.revision_table.is_some(),
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

    /// The project folder and page id a tool's page argument `arg` names, as the key of
    /// [`Assistant::graphics_read`].
    fn graphics_key(&self, v: &Value, arg: &str) -> Option<(PathBuf, String)> {
        let page = self.page_of(v, arg).ok()?;
        let open = self.open.as_ref()?;
        Some((open.dir.clone(), open.project.pages.get(page)?.id.clone()))
    }

    /// The drawings now on the page a [`Assistant::graphics_read`] key names, if it is there.
    fn graphics_now(&self, key: &(PathBuf, String)) -> Option<&[Shape]> {
        let open = self.open.as_ref().filter(|o| o.dir == key.0)?;
        Some(&open.project.pages.iter().find(|p| p.id == key.1)?.graphics)
    }

    /// Whether the drawings of a page are as the assistant last read them (true when it has not
    /// read them).
    fn graphics_as_read(&self, key: &(PathBuf, String)) -> bool {
        self.assistant
            .graphics_read
            .get(key)
            .is_none_or(|read| self.graphics_now(key).is_some_and(|now| now == &read[..]))
    }

    /// Run one assistant tool. A free drawing named by position is refused when the page's
    /// drawings changed since the assistant read them, so a write never lands on another one.
    pub(crate) fn agent_tool(&mut self, name: &str, v: &Value) -> Result<String, String> {
        // The pages the tool works on, found before it runs (it may show or remove a page):
        // its page (for a toolbar action always the page shown, whatever else it is given),
        // and the page a copy goes to.
        let mut keys = vec![if name == "run_action" {
            self.graphics_key(&Value::Null, "page")
        } else {
            self.graphics_key(v, "page")
        }];
        if name == "copy_items" && v.get("to_page").is_some() {
            keys.push(self.graphics_key(v, "to_page"));
        }
        let keys: Vec<((PathBuf, String), bool)> = keys
            .into_iter()
            .flatten()
            .map(|k| {
                let as_read = self.graphics_as_read(&k);
                (k, as_read)
            })
            .collect();
        if names_graphics(name, v)
            && let Some((key, false)) = keys.first()
        {
            return Err(format!(
                "The free drawings on page {} changed since you read it (someone else edited it): \
                 call get_page again and use the drawing ids it gives.",
                key.1
            ));
        }
        let out = self.agent_tool_run(name, v)?;
        // What the assistant read, or changed itself on drawings it had read, is what it knows.
        // A toolbar action changes the page shown (undo and redo can bring back drawings in an
        // order the assistant never read, so they do not count).
        let toolbar = name == "run_action"
            && !s(v, "action").is_some_and(|a| {
                [Action::Undo, Action::Redo]
                    .iter()
                    .any(|u| u.name().eq_ignore_ascii_case(a))
            });
        let edits = edits_page(name, v) || name == "copy_items" || toolbar;
        for (key, as_read) in keys {
            // The id `add_graphic` answers with counts as read too.
            let known = name == "add_graphic" || self.assistant.graphics_read.contains_key(&key);
            if name == "get_page" || (edits && as_read && known) {
                match self.graphics_now(&key).map(Arc::<[Shape]>::from) {
                    Some(now) => self.assistant.graphics_read.insert(key, now),
                    None => self.assistant.graphics_read.remove(&key),
                };
            }
        }
        Ok(out)
    }

    fn agent_tool_run(&mut self, name: &str, v: &Value) -> Result<String, String> {
        // In a shared session a page someone else has open is read-only here: an editing tool
        // aimed at it is refused before it runs (an unknown page gets the tool's own error).
        if edits_page(name, v)
            && let (Ok(page), Some(open)) = (self.page_of(v, "page"), self.open.as_ref())
            && let Some(why) = open.held_reason(page)
        {
            return Err(why);
        }
        // Cover Page marks an unmarked first page as the cover: refused before it runs when
        // someone else holds that page, so the form open now (and its Cancel) stays as it is.
        if name == "run_action"
            && s(v, "action").is_some_and(|a| Action::CoverPage.name().eq_ignore_ascii_case(a))
            && let Some(open) = self.open.as_ref()
            && open.cover_marks_first()
            && let Some(why) = open.held_reason(0)
        {
            return Err(why);
        }
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
                    "delete_items" => {
                        let before = open.project.pages.get(page).map(|p| p.clouds.clone());
                        open.delete_many(page, &sel);
                        // The clouds after one deleted move up: what the user had selected
                        // follows its cloud, and a deleted one is let go.
                        if page == self.ed.page
                            && let (Some(old), Some(p)) = (before, open.project.pages.get(page))
                        {
                            crate::edit::remap_clouds(&mut self.ed.selection, &old, &p.clouds);
                        }
                    }
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
                let new = open.paste(to, &clip, at)?;
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
                // Nor does one without `clouds` or `revision_table` remove the page's revision
                // marks. Given, they are used: `[]` clears the clouds and `null` the table.
                if content.get("clouds").is_none() {
                    let own = open
                        .project
                        .pages
                        .get(page)
                        .map(|p| p.clouds.clone())
                        .unwrap_or_default();
                    content["clouds"] = json!(own);
                }
                if content.get("revision_table").is_none() {
                    let own = open.project.pages.get(page).and_then(|p| p.revision_table);
                    content["revision_table"] = json!(own);
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
                let shown = page == self.ed.page;
                if let Some(p) = open.project.pages.get_mut(page) {
                    let old = std::mem::replace(p, new);
                    // On the shown page, what the user had selected follows its graphic or
                    // cloud, and one no longer there is let go. Another page is shown after
                    // this one: its selection does not carry over.
                    if shown {
                        crate::edit::remap_graphics(&mut self.ed.selection, &old.graphics, &p.graphics);
                        crate::edit::remap_clouds(&mut self.ed.selection, &old.clouds, &p.clouds);
                    } else {
                        self.ed.selection.clear();
                    }
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
                open.set_project_props(&name, &fields, None, None)?;
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
            "plc_racks" => self.tool_plc_racks(v),
            "comments" => self.tool_comments(v),
            "jumpers" => self.tool_jumpers(v),
            "strips" => self.tool_strips(v),
            "macros" => self.tool_macros(v),
            "rules" => self.tool_rules(v),
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
            "report_layouts" => self.tool_report_layouts(v),
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
                    "pdf" => match s(v, "lang").map(str::trim).filter(|l| !l.is_empty()) {
                        None => crate::export_pdf(open),
                        Some(l) => crate::export_pdf_in(
                            open,
                            &wc_core::translate::parse_languages(l).ok_or_else(|| {
                                format!("Bad language {l:?}: use a code such as nb, or two as nb,en.")
                            })?,
                        ),
                    },
                    "drawings" => crate::export_drawings(open),
                    "aml" => crate::export_aml(open),
                    "wago-marking" => {
                        let lang = s(v, "lang").unwrap_or("en");
                        let options = wc_report::MarkingOptions {
                            kinds: wc_report::parse_kinds(&string_list(v, "kinds")?)?,
                            lines: wc_report::parse_lines(&string_list(v, "lines")?)?,
                            language: wc_report::Language::parse(lang)
                                .ok_or_else(|| format!("Unknown language {lang:?} (en, de, nb)."))?,
                            strip: s(v, "strip")
                                .map(str::trim)
                                .filter(|x| !x.is_empty())
                                .map(str::to_owned),
                            ..Default::default()
                        };
                        crate::export_wago_marking(open, &options)
                    }
                    "wago-configurator" => crate::export_wago_configurator(open),
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
                    self.new_project(&path, s(v, "name").unwrap_or_default(), s(v, "base"));
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
                // The toolbar's own hold check (which page an action edits, with or without a
                // panel shown): its refusal is an error here, as for the editing tools.
                if let Some(name) = self
                    .open
                    .as_ref()
                    .and_then(|o| crate::held_for(a, &self.ed, &o.project))
                {
                    return Err(format!(
                        "{name} has this page open; it is read-only here until they leave it."
                    ));
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
            "strips": strip_list(open, None),
            "open_comments": open.project.comments.iter().filter(|c| !c.resolved).count(),
            "shown": shown,
            "selection": self
                .ed
                .selection
                .iter()
                .map(|s| item_id(self.ed.panel.is_none().then(|| open.project.pages.get(self.ed.page)).flatten(), s))
                .collect::<Vec<_>>(),
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

    /// The PLC racks form for the assistant: the same range checks, and the same warnings
    /// the form shows under it.
    fn tool_plc_racks(&mut self, v: &Value) -> Result<String, String> {
        let open = need_open(self.open.as_mut())?;
        match need_s(v, "action")? {
            "get" => serde_json::to_string_pretty(&open.project.meta.racks).map_err(|e| e.to_string()),
            "replace" => {
                let racks: Vec<wc_core::plc_rack::PlcRack> =
                    serde_json::from_value(v.get("racks").cloned().ok_or("replace needs racks")?)
                        .map_err(|e| format!("racks: {e}"))?;
                if let Some(why) = crate::racks_form::problem(&racks) {
                    return Err(format!("The racks were not changed: {why}."));
                }
                if racks == open.project.meta.racks {
                    return Ok("The racks are already like that.".into());
                }
                open.checkpoint();
                open.project.meta.racks = racks;
                let warnings = crate::rack_warnings(open);
                Ok(if warnings.is_empty() {
                    "Racks set.".into()
                } else {
                    format!("Racks set. Warnings:\n{}", warnings.join("\n"))
                })
            }
            other => Err(format!("Unknown action {other:?}: use get or replace.")),
        }
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

    fn tool_strips(&mut self, v: &Value) -> Result<String, String> {
        let action = need_s(v, "action")?;
        let open = need_open(self.open.as_mut())?;
        if action == "list" {
            let only = s(v, "strip").map(tag).transpose()?;
            return Ok(pretty(&json!(strip_list(open, only.as_ref()))));
        }
        if !["move", "spare", "unspare", "level", "reset"].contains(&action) {
            return Err(format!("Unknown strips action {action}."));
        }
        let strip = tag(need_s(v, "strip")?)?;
        if action == "reset" {
            open.reset_terminal_order(&strip);
            return Ok(format!("{strip} is in the order by terminal number now."));
        }
        let terminal = need_s(v, "terminal")?.trim().to_owned();
        let rail = open.strip_terminals().remove(&strip).unwrap_or_default();
        let on_rail = |t: &str| rail.iter().find(|(n, _)| n == t).map(|(_, spare)| *spare);
        match action {
            "move" => {
                let to = v
                    .get("to")
                    .and_then(Value::as_u64)
                    .filter(|t| *t >= 1)
                    .ok_or("`to` must be a place on the rail, 1 or more.")?;
                let to = usize::try_from(to - 1).map_err(|e| e.to_string())?;
                if to >= rail.len() {
                    return Err(format!("{strip} has {} places on the rail.", rail.len()));
                }
                open.move_terminal(&strip, &terminal, to)?;
                Ok(format!("Moved {strip} {terminal} to place {} on the rail.", to + 1))
            }
            "spare" => {
                let after = s(v, "after").map(str::trim);
                if let Some(a) = after
                    && on_rail(a).is_none()
                {
                    return Err(format!("{strip} has no terminal {a}."));
                }
                open.add_spare(&strip, &terminal, after)?;
                Ok(match after {
                    Some(a) => format!("Added spare {strip} {terminal} after {a}."),
                    None => format!("Added spare {strip} {terminal} at the end."),
                })
            }
            "unspare" => {
                if on_rail(&terminal) != Some(true) {
                    return Err(format!("{strip} has no spare terminal {terminal}."));
                }
                open.remove_spare(&strip, &terminal);
                Ok(format!("Took spare {strip} {terminal} off the rail."))
            }
            "level" => {
                // A number, or null to clear it: a missing or other value is a mistake, not a
                // request to clear the level.
                let level = match v.get("level") {
                    Some(Value::Null) => None,
                    Some(l) => Some(
                        // The terminal plan's range: a level it cannot read would be stored
                        // and then shown as none.
                        wc_report::parse_level(
                            &l.as_u64()
                                .ok_or("`level` must be a whole number, or null to clear it.")?
                                .to_string(),
                        )
                        .map_err(|e| format!("`level` cannot be {l}: {e}."))?,
                    ),
                    None => return Err("`level` is missing: give 1 or more, or null to clear it.".into()),
                };
                if on_rail(&terminal).is_none() {
                    return Err(format!("{strip} has no terminal {terminal}."));
                }
                let text = level.map(|l| l.to_string()).unwrap_or_default();
                crate::strip_editor::set_level(open, &strip, &terminal, &text)?;
                Ok(match level {
                    Some(l) => format!("{strip} {terminal} is on level {l}."),
                    None => format!("{strip} {terminal} has no level of its own now."),
                })
            }
            other => Err(format!("Unknown strips action {other}.")),
        }
    }

    /// The `rules` tool: the project's check rules and the library rules it turns off, kept
    /// as the check rules dialog keeps them (its checks, one undo step).
    fn tool_rules(&mut self, v: &Value) -> Result<String, String> {
        use wc_core::check_rule::{CheckRule, turned_off};
        let action = need_s(v, "action")?;
        let open = need_open(self.open.as_mut())?;
        let library = wc_library::library_rules(&open.dir, &open.project);
        let off = &open.project.meta.rules_off;
        match action {
            "list" => {
                let sources: Vec<Value> = library
                    .iter()
                    .map(|set| match &set.rules {
                        Ok(rules) => {
                            let rules: Vec<Value> = rules
                                .iter()
                                .enumerate()
                                .map(|(k, r)| {
                                    json!({"rule": format!("{}:{}", set.source, k + 1), "off": turned_off(off, &set.source, k + 1, r), "is": r})
                                })
                                .collect();
                            json!({"source": set.source, "rules": rules})
                        }
                        Err(e) => json!({"source": set.source, "error": e}),
                    })
                    .collect();
                Ok(pretty(
                    &json!({"rules": open.project.meta.rules, "library": sources, "rules_off": off}),
                ))
            }
            "set" => {
                let list = v
                    .get("rules")
                    .and_then(Value::as_array)
                    .ok_or("`rules` must be a list of rules (an empty list removes them all).")?;
                let mut rules = Vec::with_capacity(list.len());
                for (k, r) in list.iter().enumerate() {
                    let rule: CheckRule =
                        serde_json::from_value(r.clone()).map_err(|e| format!("Rule {}: {e}.", k + 1))?;
                    if !rule.unknown.is_empty() {
                        let keys: Vec<&str> = rule.unknown.keys().map(String::as_str).collect();
                        return Err(format!(
                            "Rule {}: no such key {} (a rule has kind, devices, role, pattern, max, severity, message and name).",
                            k + 1,
                            keys.join(", ")
                        ));
                    }
                    // The dialog's checks: the check would report such a rule instead of running it.
                    rules.push(crate::rules_form::RuleRow::of(&rule).rule(k + 1)?);
                }
                let n = rules.len();
                open.set_check_rules(rules);
                Ok(format!("The project has {n} check rule(s) now; run_check runs them."))
            }
            "off" | "on" => {
                let which = need_s(v, "rule")?.trim();
                // The one rule `which` names, as `turned_off` reads it: by number, or by name.
                // A selector that could stand for two rules (a rule named "2" and rule 2,
                // or two rules of one name) is refused rather than guessed.
                let mut found = None;
                if let Some((src, sel)) = which.split_once(':')
                    && let Some(set) = library.iter().find(|set| set.source == src.trim())
                {
                    let rules = match &set.rules {
                        Ok(rules) => rules,
                        // `on` can still drop the entry as written, below.
                        Err(e) if action == "off" => {
                            return Err(format!(
                                "The check rules of library {} cannot be read, so {which:?} cannot be turned off: {e}",
                                set.source
                            ));
                        }
                        Err(_) => &Vec::new(),
                    };
                    let one = [which.to_owned()];
                    let hits: Vec<(usize, &CheckRule)> = rules
                        .iter()
                        .enumerate()
                        .map(|(k, r)| (k + 1, r))
                        .filter(|&(n, r)| turned_off(&one, &set.source, n, r))
                        .collect();
                    if hits.len() > 1 {
                        let list: Vec<String> = hits.iter().map(|(n, _)| format!("{}:{n}", set.source)).collect();
                        return Err(format!(
                            "{which:?} could be rule {} of library {}: {:?} is a number and a rule's name too, or two rules share it. An entry in rules_off turns off all of them; name the rule another way (list shows each rule's number and name).",
                            list.join(" or "),
                            set.source,
                            sel.trim()
                        ));
                    }
                    found = hits.first().map(|&(n, r)| (set.source.clone(), n, r.clone()));
                }
                let mut now = off.clone();
                match (action, found) {
                    ("off", None) => {
                        return Err(format!(
                            "No library check rule {which:?}: write source:number or source:name (list shows them)."
                        ));
                    }
                    ("off", Some((src, n, r))) => {
                        if turned_off(&now, &src, n, &r) {
                            return Ok(format!("{which} is off already."));
                        }
                        now.push(which.to_owned());
                    }
                    (_, Some((src, n, r))) => {
                        // Every entry that turns it off, by number or by name.
                        now.retain(|o| !turned_off(std::slice::from_ref(o), &src, n, &r));
                    }
                    // An entry naming no rule (a source since removed) can still be dropped.
                    (_, None) => now.retain(|o| o.trim() != which),
                }
                if now == *off {
                    return Ok(format!("{which} is on already."));
                }
                open.set_rules_off(now);
                Ok(format!("{which} is {action} now; run_check shows the effect."))
            }
            other => Err(format!("Unknown action {other:?}: use list, set, off or on.")),
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
                        |m| json!({"name": m.name, "description": m.description, "value_sets": m.value_sets, "source": m.source, "error": m.error}),
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
                let items = open.insert_macro(page, &entry.path, s(v, "value_set"))?;
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

    /// The `report_layouts` tool: read, replace or reset a report's layout through the report
    /// layout dialog's form, so a layout is checked and kept as OK in the dialog keeps it.
    fn tool_report_layouts(&mut self, v: &Value) -> Result<String, String> {
        use crate::report_layout_form::Form;
        let action = need_s(v, "action")?;
        let kind = s(v, "kind")
            .map(|k| wc_report::Kind::parse(k.trim()).ok_or_else(|| format!("Unknown report {k:?}.")))
            .transpose()?;
        let open = need_open(self.open.as_mut())?;
        let builtin = |open: &crate::state::Open, kind| {
            let inputs = wc_report::Inputs {
                project: &open.project,
                symbols: &open.symbols,
                parts: &open.parts,
            };
            wc_report::build_builtin(kind, &inputs).columns
        };
        // What wc_report warns about the project's report layouts (as `wc_report` and
        // `wiringcraft report` name it), and which of those are about `kind`.
        let all_warnings = |open: &crate::state::Open| -> Vec<String> {
            let inputs = wc_report::Inputs {
                project: &open.project,
                symbols: &open.symbols,
                parts: &open.parts,
            };
            wc_report::layout_warnings(&inputs)
        };
        let warnings_for = |all: &[String], kind: wc_report::Kind| -> Vec<String> {
            let about = format!("report layout for \"{}\":", kind.name());
            all.iter().filter(|w| w.starts_with(&about)).cloned().collect()
        };
        match action {
            "get" => {
                let kinds = kind.map_or_else(|| wc_report::Kind::ALL.to_vec(), |k| vec![k]);
                let all = all_warnings(open);
                let out: Vec<Value> = kinds
                    .into_iter()
                    .map(|k| {
                        json!({
                            "kind": k.name(),
                            "layout": wc_report::layout_for(&open.project, k),
                            "builtin_columns": builtin(open, k),
                            "warnings": warnings_for(&all, k),
                        })
                    })
                    .collect();
                Ok(pretty(&json!(out)))
            }
            "set" | "reset" => {
                let kind = kind.ok_or("Give the report's kind.")?;
                let columns = builtin(open, kind);
                let form = if action == "reset" {
                    Form::new(kind, columns, wc_report::layout_for(&open.project, kind)).cleared()
                } else {
                    let mut given = v.get("layout").cloned().ok_or("Give the layout.")?;
                    let Some(fields) = given.as_object_mut() else {
                        return Err("The layout must be an object.".into());
                    };
                    report_layout_unknown_key(fields)?;
                    fields.insert("kind".into(), json!(kind.name()));
                    let layout: wc_core::report_layout::ReportLayout =
                        serde_json::from_value(given).map_err(|e| format!("Bad layout: {e}"))?;
                    // Read into the dialog's form as the dialog would show it, so it is kept
                    // as OK keeps it (a layout that shows the built-in one keeps none).
                    Form::new(kind, columns, Some(&layout))
                };
                let layout = form.kept()?;
                let built_in = layout.is_none();
                open.set_report_layout(kind, layout);
                let mut said = if built_in {
                    format!("The {} report has its built-in layout.", kind.name())
                } else {
                    format!("The {} report's layout is set.", kind.name())
                };
                let warned = warnings_for(&all_warnings(open), kind);
                if !warned.is_empty() {
                    said.push_str(&format!(" Warnings:\n{}", warned.join("\n")));
                }
                Ok(said)
            }
            a => Err(format!("Unknown report_layouts action {a}.")),
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
            // In a shared session a page someone else has open is read-only here, and the
            // command line writes the project folder directly: only commands that read it run.
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            if let Some(why) = writes_while_held(&args, &open.dir, &open.held_by_others, &cwd) {
                return Err(why);
            }
            // The look above cannot see a link swapped in after it: the command line opens
            // each output file so that it cannot land on a project file either (#458).
            if !open.held_by_others.is_empty() {
                cmd.env(wc_core::guard::GUARD_ENV, &open.dir);
            } else {
                cmd.env_remove(wc_core::guard::GUARD_ENV);
            }
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

/// Why the `wiringcraft` command `args` does not run on the project folder `dir` while others
/// hold pages (`held`, page id to name): it may change the project, or it names a path into the
/// project folder (an export written over a page). `None` when it runs: nobody holds a page, or
/// it only reads the project.
///
/// `cwd` is the folder the command runs in, which relative paths are taken from.
fn writes_while_held(args: &[String], dir: &Path, held: &BTreeMap<String, String>, cwd: &Path) -> Option<String> {
    if held.is_empty() {
        return None;
    }
    let mut who: Vec<&str> = held.values().map(String::as_str).collect();
    who.sort_unstable();
    who.dedup();
    let who = who.join(", ");
    let arg = |i: usize| args.get(i).map(String::as_str);
    let has = |flag: &str| args.iter().any(|a| a == flag || a.starts_with(&format!("{flag}=")));
    // Commands that only read the project; their output goes to the screen or to a file.
    let reads = match (arg(0), arg(1)) {
        (Some("--help" | "-h" | "help" | "--version" | "nets" | "check" | "report" | "pdf" | "drawing" | "aml"), _)
        | (Some("export" | "plc-vars" | "inspect"), _) => true,
        (Some("find"), _) => !has("--replace"),
        (Some("vcs"), Some("log" | "changes" | "page-log")) => true,
        (
            Some(
                "page" | "panel" | "location" | "jumper" | "strip" | "macro" | "revision" | "template" | "lib"
                | "comment",
            ),
            Some("list" | "show"),
        ) => true,
        _ => false,
    };
    if !reads || has("-C") || has("--project") {
        return Some(format!(
            "`wiringcraft {}` is not run while {who} has a page open in the shared session: the command line \
             writes the project folder directly, also pages that are read-only here. Make the change with the \
             drawing tools, or run it once they have left the page.",
            args.join(" ")
        ));
    }
    let project = Guarded::new(dir);
    // Run from inside the project folder, even a bare file name could land on a held page: only
    // a command with nothing after its own words (`check`, `page list`) runs there.
    let words = match arg(0) {
        Some("vcs") => 2,
        _ if matches!(arg(1), Some("list" | "show")) => 2,
        _ => 1,
    };
    let more = args.iter().skip(words).any(|a| !a.starts_with('-'));
    // When that cannot be told, the command is not run from there either.
    if more && project.holds(cwd).unwrap_or(true) {
        return Some(format!(
            "`wiringcraft {}` is not run while {who} has a page open in the shared session: this program runs \
             from inside the project folder, so an output file could land on a page. Start it from another folder.",
            args.join(" ")
        ));
    }
    for a in args {
        let why = match project.reached_by(cwd, split_flag(a).1) {
            Ok(false) => continue,
            Ok(true) => "is in the project folder or links to a file in it".to_owned(),
            Err(e) => format!("could not be looked through ({e})"),
        };
        return Some(format!(
            "`{a}` {why}, and {who} has a page open in the shared session: write the output to a folder of \
             its own outside the project folder."
        ));
    }
    None
}

/// `--name=value` split after its `=`; anything else is all value.
fn split_flag(a: &str) -> (&str, &str) {
    match a.find('=') {
        Some(i) if a.starts_with("--") => a.split_at(i + 1),
        _ => ("", a),
    }
}

/// The project's files ([`wc_core::guard::ProjectFiles`]), to tell whether a command's output
/// could land on one.
struct Guarded(wc_core::guard::ProjectFiles);

impl std::ops::Deref for Guarded {
    type Target = wc_core::guard::ProjectFiles;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Guarded {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// How many files in an output folder are looked at; a bigger folder is not used while a
/// page is held.
const OUTPUT_FOLDER_FILES: usize = 10_000;

impl Guarded {
    fn new(project: &Path) -> Self {
        Self(wc_core::guard::ProjectFiles::new(project))
    }

    /// Whether writing to `value` could land on a project file: it is a path into the project
    /// or a link to a file in it, or it is a folder with such a link in it (an export writes
    /// its own file names there). `value` counts as a path when it is absolute, clearly a
    /// path (it has a separator or starts with a dot), or names something in `cwd`. A folder
    /// that cannot be listed is an error: what is in it is not known.
    fn reached_by(&self, cwd: &Path, value: &str) -> std::io::Result<bool> {
        let p = Path::new(value);
        let path = if p.is_absolute() {
            p.to_path_buf()
        } else if value.contains('/')
            || value.contains('\\')
            || value.starts_with('.')
            || (!value.is_empty() && std::fs::symlink_metadata(cwd.join(p)).is_ok())
        {
            cwd.join(p)
        } else {
            return Ok(false);
        };
        if self.holds(&path)? {
            return Ok(true);
        }
        let mut todo = vec![wc_core::guard::real(&path)];
        let mut seen = 0;
        while let Some(d) = todo.pop() {
            if !d.is_dir() {
                continue;
            }
            let entries = match std::fs::read_dir(&d) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            for e in entries {
                let e = e?;
                seen += 1;
                if seen > OUTPUT_FOLDER_FILES || self.holds(&e.path())? {
                    return Ok(true);
                }
                if e.file_type()?.is_dir() {
                    todo.push(e.path());
                }
            }
        }
        Ok(false)
    }
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
            strip_editor: None,
            show_layers: false,
            live: Default::default(),
            fetch: None,
        };
        (tmp, app)
    }

    /// A `file://` URL for a local folder, also with a Windows drive letter.
    fn file_url(p: &std::path::Path) -> String {
        let path = p.to_string_lossy().replace('\\', "/");
        if path.starts_with('/') {
            format!("file://{path}")
        } else {
            format!("file:///{path}")
        }
    }

    /// Frames until the base project fetch is over (at most a few seconds).
    fn until_fetched(a: &mut App) {
        let ctx = egui::Context::default();
        for _ in 0..500 {
            a.poll_fetch(&ctx);
            if a.fetch.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(a.fetch.is_none(), "the fetch did not end");
    }

    #[test]
    fn a_base_project_from_a_repository_is_fetched_off_the_window_thread() {
        let (tmp, mut a) = app(vec![]);
        let base = tmp.path().join("base");
        wc_core::store::create(&base, "ACME base").unwrap();
        wc_vcs::Git::new(&base).init().unwrap();
        let dir = tmp.path().join("pump");
        a.start_new_project(&dir, "Pump station", Some(&file_url(&base)));
        // The window goes on while it runs: a note with Cancel, and the old project still open.
        assert!(matches!(a.dialog, Some(Dialog::Fetching(_))));
        assert!(a.fetch.is_some());
        until_fetched(&mut a);
        assert!(a.dialog.is_none(), "{:?}", a.dialog.as_ref().map(|_| ()));
        let open = a.open.as_ref().unwrap();
        assert_eq!(open.dir, dir);
        assert_eq!(open.project.meta.name, "Pump station");
        assert!(dir.join(".git").exists(), "recorded as a first version");
        // A repository that cannot be fetched says why, and makes nothing.
        let missing = tmp.path().join("missing");
        a.start_new_project(&missing, "X", Some(&file_url(&tmp.path().join("nowhere"))));
        until_fetched(&mut a);
        assert!(matches!(&a.dialog, Some(Dialog::Message(m)) if m.contains("Could not create")));
        assert!(!missing.exists());
    }

    #[test]
    fn a_cancelled_fetch_opens_nothing() {
        let (tmp, mut a) = app(vec![]);
        let before = a.open.as_ref().unwrap().dir.clone();
        let (tx, done) = std::sync::mpsc::channel();
        a.fetch = Some(crate::BaseFetch {
            dir: tmp.path().join("new"),
            source: "file:///slow".into(),
            was_open: Some(before.clone()),
            made: false,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            done,
        });
        a.dialog = Some(Dialog::Fetching("file:///slow".into()));
        // Cancel sets the flag the fetch stops on.
        a.cancel_fetch();
        assert!(
            a.fetch
                .as_ref()
                .unwrap()
                .cancel
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        // Still running: nothing changes.
        until_fetched_once(&mut a);
        assert!(a.fetch.is_some());
        tx.send(Err(None)).unwrap();
        until_fetched(&mut a);
        assert!(a.dialog.is_none());
        assert!(a.ed.message.contains("not made"), "{}", a.ed.message);
        assert_eq!(a.open.as_ref().unwrap().dir, before);
    }

    #[test]
    fn a_second_fetch_keeps_the_first_ones_cancel() {
        let (tmp, mut a) = app(vec![]);
        let (_tx, done) = std::sync::mpsc::channel();
        a.fetch = Some(crate::BaseFetch {
            dir: tmp.path().join("new"),
            source: "file:///slow".into(),
            was_open: None,
            made: false,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            done,
        });
        a.start_new_project(&tmp.path().join("other"), "X", Some("file:///other"));
        assert!(matches!(&a.dialog, Some(Dialog::Fetching(s)) if s == "file:///slow"));
        assert!(a.ed.message.contains("already"), "{}", a.ed.message);
    }

    #[test]
    fn a_fetched_project_keeps_the_unsaved_work_of_the_open_one() {
        let (tmp, mut a) = app(vec![]);
        let base = tmp.path().join("base");
        wc_core::store::create(&base, "ACME base").unwrap();
        wc_vcs::Git::new(&base).init().unwrap();
        let old = a.open.as_ref().unwrap().dir.clone();
        let o = a.open.as_mut().unwrap();
        o.project.meta.name = "Unsaved name".into();
        o.dirty = true;
        let dir = tmp.path().join("pump");
        a.start_new_project(&dir, "Pump station", Some(&file_url(&base)));
        until_fetched(&mut a);
        assert_eq!(a.open.as_ref().unwrap().dir, dir);
        assert_eq!(wc_core::store::load(&old).unwrap().meta.name, "Unsaved name");
    }

    #[test]
    fn a_fetched_project_waits_for_a_running_command_and_an_open_dialog() {
        let (tmp, mut a) = app(vec![]);
        let before = a.open.as_ref().unwrap().dir.clone();
        // The fetch is in and the project made.
        let dir = tmp.path().join("pump");
        wc_core::store::create(&dir, "Pump station").unwrap();
        let (tx, done) = std::sync::mpsc::channel();
        tx.send(Ok(())).unwrap();
        a.fetch = Some(crate::BaseFetch {
            dir: dir.clone(),
            source: "file:///base".into(),
            was_open: Some(before.clone()),
            made: false,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            done,
        });
        // A command works on the open project's files while the window edits it too: neither
        // is saved over nor replaced until the command is done and reconciled.
        let open = a.open.as_mut().unwrap();
        a.assistant.command_base = Some((open.dir.clone(), open.project.clone()));
        let mut by_command = open.project.clone();
        by_command.meta.name = "Named by the command".into();
        wc_core::store::save(&before, &by_command).unwrap();
        open.project.meta.name = "Named in the window".into();
        open.dirty = true;
        until_fetched_once(&mut a);
        assert!(a.fetch.is_some());
        assert_eq!(a.open.as_ref().unwrap().dir, before);
        assert_eq!(
            wc_core::store::load(&before).unwrap().meta.name,
            "Named by the command",
            "not saved over the command"
        );
        let _ = a.command_finish(Ok("done".into()));
        assert!(!a.assistant.command_running());
        // Reconciled: the window's edit is kept, still unsaved.
        assert_eq!(a.open.as_ref().unwrap().project.meta.name, "Named in the window");
        assert!(a.open.as_ref().unwrap().dirty);
        // A dialog open on the project: the same, until it is closed.
        a.dialog = Some(Dialog::Message("about the open project".into()));
        until_fetched_once(&mut a);
        assert!(a.fetch.is_some());
        assert_eq!(a.open.as_ref().unwrap().dir, before);
        a.dialog = None;
        until_fetched(&mut a);
        assert_eq!(a.open.as_ref().unwrap().dir, dir);
        // The handoff saved the reconciled project: the window's edit is on disk.
        assert_eq!(wc_core::store::load(&before).unwrap().meta.name, "Named in the window");
    }

    #[test]
    fn a_new_project_that_cannot_be_recorded_or_opened_says_both() {
        let (tmp, mut a) = app(vec![]);
        // Not a project, and a `.git` git cannot use.
        let dir = tmp.path().join("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".git"), "not a repository").unwrap();
        let opening = crate::state::Open::load(&dir).err().map(|e| e.to_string());
        a.finish_new_project(&dir, false);
        let Some(Dialog::Message(m)) = &a.dialog else {
            unreachable!("no message")
        };
        assert!(m.contains("Could not open"), "{m}");
        let opening = opening.unwrap_or_default();
        assert!(
            !opening.is_empty() && m.contains(&opening),
            "the reason it did not open, {opening:?}: {m}"
        );
        assert!(m.contains("could not be recorded"), "{m}");
        assert!(m.contains("invalid gitfile format"), "git's reason: {m}");
    }

    #[test]
    fn a_new_project_that_cannot_be_recorded_while_the_open_one_cannot_be_saved_says_both() {
        let (tmp, mut a) = app(vec![]);
        let before = a.open.as_ref().unwrap().dir.clone();
        // The open project has unsaved work and its folder is gone: a file stands in its place.
        a.open.as_mut().unwrap().dirty = true;
        std::fs::remove_dir_all(&before).unwrap();
        std::fs::write(&before, "not a folder").unwrap();
        let dir = tmp.path().join("broken");
        wc_core::store::create(&dir, "New").unwrap();
        std::fs::write(dir.join(".git"), "not a repository").unwrap();
        a.finish_new_project(&dir, false);
        let Some(Dialog::Message(m)) = &a.dialog else {
            unreachable!("no message")
        };
        assert!(m.contains("could not be saved first"), "{m}");
        assert!(m.contains("i/o error"), "the save's reason: {m}");
        assert!(m.contains("could not be recorded either"), "{m}");
        assert!(m.contains("invalid gitfile format"), "git's reason: {m}");
        assert_eq!(a.open.as_ref().unwrap().dir, before, "the open project stays open");
    }

    #[test]
    fn a_fetched_project_waits_for_an_assistant_turn_and_the_symbol_editor() {
        let answers = vec![json!({
            "model": "claude-opus-5-5", "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "Done."}]
        })];
        let (tmp, mut a) = app(answers);
        let before = a.open.as_ref().unwrap().dir.clone();
        let dir = tmp.path().join("pump");
        wc_core::store::create(&dir, "Pump station").unwrap();
        let (tx, done) = std::sync::mpsc::channel();
        tx.send(Ok(())).unwrap();
        a.fetch = Some(crate::BaseFetch {
            dir: dir.clone(),
            source: "file:///base".into(),
            was_open: Some(before.clone()),
            made: false,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            done,
        });
        // A turn asked about the open project: its answer is for that project.
        let ctx = eframe::egui::Context::default();
        a.prefs.assistant.backend = wc_agent::Backend::ApiKey;
        a.assistant.input = "What is on page 2?".into();
        a.assistant_send(&ctx);
        assert!(a.assistant.busy());
        until_fetched_once(&mut a);
        assert!(a.fetch.is_some());
        for _ in 0..500 {
            a.assistant_poll(&ctx);
            if !a.assistant.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!a.assistant.busy());
        // A symbol draft open: it is not dropped under the user.
        a.symbol_editor = Some(crate::symbol_editor::SymbolEditor::new(Vec::new()));
        until_fetched_once(&mut a);
        assert!(a.fetch.is_some());
        assert!(a.symbol_editor.is_some());
        assert_eq!(a.open.as_ref().unwrap().dir, before);
        a.symbol_editor = None;
        until_fetched(&mut a);
        assert_eq!(a.open.as_ref().unwrap().dir, dir);
    }

    #[test]
    fn a_fetch_outcome_is_taken_in_while_the_symbol_editor_is_open() {
        let (tmp, mut a) = app(vec![]);
        let before = a.open.as_ref().unwrap().dir.clone();
        let fetch = |a: &mut App, outcome: Result<(), Option<String>>| {
            let (tx, done) = std::sync::mpsc::channel();
            tx.send(outcome).unwrap();
            a.fetch = Some(crate::BaseFetch {
                dir: tmp.path().join("pump"),
                source: "file:///base".into(),
                was_open: Some(before.clone()),
                made: false,
                cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                done,
            });
            a.dialog = Some(Dialog::Fetching("file:///base".into()));
        };
        a.symbol_editor = Some(crate::symbol_editor::SymbolEditor::new(Vec::new()));
        // Cancelled and failed fetches are said at once, and a new one can start.
        fetch(&mut a, Err(None));
        until_fetched_once(&mut a);
        assert!(a.fetch.is_none());
        assert!(a.ed.message.contains("not made"), "{}", a.ed.message);
        fetch(&mut a, Err(Some("no such server".into())));
        until_fetched_once(&mut a);
        assert!(a.fetch.is_none());
        assert!(matches!(&a.dialog, Some(Dialog::Message(m)) if m.contains("no such server")));
        // A made project waits for the editor, without a Cancel that would do nothing.
        wc_core::store::create(&tmp.path().join("pump"), "Pump station").unwrap();
        fetch(&mut a, Ok(()));
        until_fetched_once(&mut a);
        assert!(a.fetch.as_ref().unwrap().made);
        assert!(a.dialog.is_none(), "the Fetching note with Cancel is gone");
        a.cancel_fetch();
        assert!(
            !a.fetch
                .as_ref()
                .unwrap()
                .cancel
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        assert!(a.ed.message.contains("made already"), "{}", a.ed.message);
        a.symbol_editor = None;
        until_fetched(&mut a);
        assert_eq!(a.open.as_ref().unwrap().dir, tmp.path().join("pump"));
    }

    #[test]
    fn a_project_opened_while_fetching_stays_open() {
        let (tmp, mut a) = app(vec![]);
        let base = tmp.path().join("base");
        wc_core::store::create(&base, "ACME base").unwrap();
        wc_vcs::Git::new(&base).init().unwrap();
        let dir = tmp.path().join("pump");
        a.start_new_project(&dir, "Pump station", Some(&file_url(&base)));
        // Another project is opened before the fetch is in.
        a.fetch.as_mut().unwrap().was_open = Some(tmp.path().join("before"));
        until_fetched(&mut a);
        assert_ne!(a.open.as_ref().map(|o| o.dir.clone()), Some(dir.clone()));
        assert!(matches!(&a.dialog, Some(Dialog::Message(m)) if m.contains("ready")));
        assert!(dir.join(".git").exists(), "made and recorded all the same");
    }

    #[test]
    fn an_assistant_edit_is_checked_while_drawing() {
        let (_tmp, mut a) = app(vec![]);
        let o = a.open.as_mut().unwrap();
        let mut ids = Vec::new();
        for x in [20.0, 60.0] {
            let id = o.place(1, "wc.coil", Point::new(x, 20.0)).unwrap();
            let s = o.project.pages[1].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(wc_core::DeviceTag::parse("-K1").unwrap());
            ids.push(id);
        }
        o.touch();
        let ctx = egui::Context::default();
        let drawn_twice = |a: &App| a.ed.live.iter().filter(|f| f.message.contains("-K1 is drawn")).count();
        // Well within the sweep: only the edit's own revision can start the check.
        let until = |a: &mut App, want: usize| {
            for _ in 0..100 {
                a.live_check(&ctx);
                if drawn_twice(a) == want && !a.live.busy() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        until(&mut a, 2);
        assert_eq!(drawn_twice(&a), 2, "{:?}", a.ed.live);
        // The assistant renames the second coil: no input in any frame.
        call(
            &mut a,
            "update_symbol",
            json!({"page": "p0002", "id": ids[1], "tag": "-K2"}),
        )
        .unwrap();
        until(&mut a, 0);
        assert_eq!(drawn_twice(&a), 0, "{:?}", a.ed.live);
    }

    fn until_fetched_once(a: &mut App) {
        a.poll_fetch(&egui::Context::default());
    }

    #[test]
    fn checks_while_drawing_are_shown_again_after_the_editor_is_replaced() {
        let (_tmp, mut a) = app(vec![]);
        let o = a.open.as_mut().unwrap();
        for x in [20.0, 60.0] {
            let id = o.place(1, "wc.coil", Point::new(x, 20.0)).unwrap();
            let s = o.project.pages[1].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(wc_core::DeviceTag::parse("-K1").unwrap());
        }
        let ctx = egui::Context::default();
        let drawn_twice = |a: &App| a.ed.live.iter().filter(|f| f.message.contains("-K1 is drawn")).count();
        for _ in 0..200 {
            a.live_check(&ctx);
            if drawn_twice(&a) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(drawn_twice(&a), 2, "{:?}", a.ed.live);
        // Opening the same unchanged project replaces the editor; the checker keeps its
        // findings for the same inputs, and they show again at once.
        a.ed = crate::canvas::Editor::default();
        a.live_check(&ctx);
        assert_eq!(drawn_twice(&a), 2, "{:?}", a.ed.live);
        // Turned off while markers are shown: they go.
        a.prefs.live_check = false;
        a.live_check(&ctx);
        assert!(a.ed.live.is_empty(), "{:?}", a.ed.live);
        a.prefs.live_check = true;
        // On again: the checker checks project A afresh and shows both findings, so the
        // switch below starts from a checker that holds A's findings.
        for _ in 0..200 {
            a.live_check(&ctx);
            if drawn_twice(&a) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(drawn_twice(&a), 2, "{:?}", a.ed.live);
        // Another project is opened: the first one's findings are not shown on it.
        let (_other, b) = crate::state::tests::project();
        a.open = Some(b);
        a.ed = crate::canvas::Editor::default();
        a.live_check(&ctx);
        assert!(a.ed.live.is_empty(), "{:?}", a.ed.live);
        assert_eq!(
            a.live.status(&a.ed.live),
            "Check: checking",
            "no counts of the first project"
        );
    }

    #[test]
    fn enter_ends_a_polyline_only_without_a_dialog_or_a_focused_field() {
        let (_tmp, mut a) = app(vec![]);
        a.ed.tool = crate::state::Tool::Polyline;
        let ctx = egui::Context::default();
        let enter = || egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        // A frame (focusing a text field first when asked), then one with Enter pressed.
        let press = |a: &mut App, field: bool| -> bool {
            a.ed.finish_shape = false;
            let id = egui::Id::new("field");
            for events in [vec![], vec![enter()]] {
                let enter_frame = !events.is_empty();
                let raw = egui::RawInput {
                    events,
                    ..Default::default()
                };
                let _ = ctx.run(raw, |ctx| {
                    if field {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let mut s = String::new();
                            ui.add(egui::TextEdit::singleline(&mut s).id(id));
                        });
                        ctx.memory_mut(|m| m.request_focus(id));
                    }
                    if enter_frame {
                        let _ = a.shortcuts(ctx);
                    }
                });
            }
            a.ed.finish_shape
        };
        assert!(press(&mut a, false), "Enter ends the polyline");
        a.dialog = Some(crate::Dialog::Message("Busy".into()));
        assert!(!press(&mut a, false), "not under a dialog");
        a.dialog = None;
        assert!(!press(&mut a, true), "not while a text field has the keys");
        a.ed.tool = crate::state::Tool::Line;
        assert!(!press(&mut a, false), "only with the polyline tool");
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
    fn the_assistant_reads_and_replaces_the_plc_racks_as_one_undo_step() {
        let (_tmp, mut a) = app(vec![]);
        let read = |a: &mut App| -> Value {
            serde_json::from_str(&call(a, "plc_racks", json!({"action": "get"})).unwrap()).unwrap()
        };
        assert_eq!(read(&mut a), json!([]));
        let mark = a.open.as_ref().unwrap().undo_mark();

        // Numbers the form would not take, and racks that are not racks, change nothing.
        for bad in [
            json!({"action": "replace"}),
            json!({"action": "replace", "racks": "none"}),
            json!({"action": "replace", "racks": [{"name": "-A1", "card": [{"slot": 1000, "start": "%IX0.0"}]}]}),
            json!({"action": "replace", "racks": [{"name": "-A1", "card": [{"slot": 1, "channels": 257}]}]}),
            json!({"action": "replace", "racks": [{"name": "-A1", "card": [{"slot": 1, "step": 0}]}]}),
            json!({"action": "rename"}),
        ] {
            assert!(call(&mut a, "plc_racks", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(read(&mut a), json!([]));
        assert_eq!(a.open.as_ref().unwrap().undo_mark(), mark);

        // A rack with two cards, one without its start address: set, with the form's warning.
        let racks = json!([{"name": "-A1", "description": "Main PLC", "card": [
            {"slot": 1, "tag": "-A1.DI1", "type": "DI 16x24 V DC", "channels": 16, "start": "%IX0.0"},
            {"slot": 2, "type": "AI 4x 4-20 mA", "channels": 4, "start": ""}
        ]}]);
        let said = call(&mut a, "plc_racks", json!({"action": "replace", "racks": racks})).unwrap();
        assert!(said.contains("Warnings"), "{said}");
        let got = read(&mut a);
        assert_eq!(got[0]["name"], json!("-A1"));
        assert_eq!(got[0]["card"][0]["type"], json!("DI 16x24 V DC"));
        assert_eq!(got[0]["card"].as_array().map(Vec::len), Some(2));
        assert_eq!(a.open.as_ref().unwrap().undo_mark(), mark + 1);
        // The same racks again is no new step.
        call(&mut a, "plc_racks", json!({"action": "replace", "racks": got.clone()})).unwrap();
        assert_eq!(a.open.as_ref().unwrap().undo_mark(), mark + 1);

        assert!(a.open.as_mut().unwrap().undo());
        assert_eq!(read(&mut a), json!([]));
        assert!(a.open.as_mut().unwrap().redo());
        assert_eq!(read(&mut a), got);

        // An empty list removes every rack.
        call(&mut a, "plc_racks", json!({"action": "replace", "racks": []})).unwrap();
        assert_eq!(read(&mut a), json!([]));
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
    fn the_assistant_lists_moves_spares_and_levels_terminal_strips() {
        let (_tmp, mut a) = app(vec![]);
        {
            let o = a.open.as_mut().unwrap();
            for (k, t) in ["-X1.1", "-X1.2", "-X1.3"].into_iter().enumerate() {
                let id = o
                    .place(
                        1,
                        "wc.terminal",
                        wc_core::geom::Point::new(20.0 + 20.0 * k as f64, 20.0),
                    )
                    .unwrap();
                let s = o.project.pages[1].symbols.iter_mut().find(|s| s.id == id).unwrap();
                s.tag = Some(DeviceTag::parse(t).unwrap());
            }
        }
        // The rail as "number" with "*" for a spare and "@level" for a level.
        let rail = |a: &mut App| -> Vec<String> {
            let v: Value =
                serde_json::from_str(&call(a, "strips", json!({"action": "list", "strip": "-X1"})).unwrap()).unwrap();
            assert_eq!(v.as_array().map(Vec::len), Some(1), "{v}");
            v[0]["terminals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    let mut out = t["terminal"].as_str().unwrap().to_owned();
                    if t["spare"] == json!(true) {
                        out.push('*');
                    }
                    if let Some(l) = t["level"].as_u64() {
                        out.push_str(&format!("@{l}"));
                    }
                    out
                })
                .collect()
        };
        let undo_mark = |a: &App| a.open.as_ref().unwrap().undo_mark();
        assert_eq!(rail(&mut a), ["1", "2", "3"]);
        let before = undo_mark(&a);

        // Refusals change nothing and take no undo step.
        for bad in [
            json!({"action": "move", "strip": "-X1", "terminal": "3", "to": 0}),
            json!({"action": "move", "strip": "-X1", "terminal": "3", "to": 4}),
            json!({"action": "move", "strip": "-X1", "terminal": "9", "to": 1}),
            json!({"action": "move", "strip": "-X1", "terminal": "3"}),
            json!({"action": "spare", "strip": "-X1", "terminal": "2"}),
            json!({"action": "spare", "strip": "-X1", "terminal": "S1", "after": "9"}),
            json!({"action": "unspare", "strip": "-X1", "terminal": "2"}),
            json!({"action": "level", "strip": "-X1", "terminal": "2", "level": 0}),
            json!({"action": "level", "strip": "-X1", "terminal": "2", "level": 4_294_967_296_u64}),
            json!({"action": "level", "strip": "-X1", "terminal": "2", "level": 1.5}),
            json!({"action": "level", "strip": "-X1", "terminal": "2", "level": "2"}),
            json!({"action": "level", "strip": "-X1", "terminal": "2"}),
            json!({"action": "level", "strip": "-X1", "terminal": "9", "level": 2}),
            json!({"action": "move", "terminal": "3", "to": 1}),
            json!({"action": "explode", "strip": "-X1"}),
        ] {
            assert!(call(&mut a, "strips", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(rail(&mut a), ["1", "2", "3"]);
        assert_eq!(undo_mark(&a), before);
        let zero = json!({"action": "level", "strip": "-X1", "terminal": "2", "level": 0});
        let why = call(&mut a, "strips", zero).unwrap_err();
        assert!(why.contains("levels start at 1"), "{why}");

        // Move: one undo step, and the state shows the rail.
        call(
            &mut a,
            "strips",
            json!({"action": "move", "strip": "-X1", "terminal": "3", "to": 1}),
        )
        .unwrap();
        assert_eq!(undo_mark(&a), before + 1);
        assert_eq!(rail(&mut a), ["3", "1", "2"]);
        let state: Value = serde_json::from_str(&call(&mut a, "get_state", json!({})).unwrap()).unwrap();
        assert_eq!(state["strips"][0]["strip"], json!("-X1"));
        assert_eq!(state["strips"][0]["terminals"][0]["terminal"], json!("3"));

        // A spare after a terminal, and one at the end.
        call(
            &mut a,
            "strips",
            json!({"action": "spare", "strip": "-X1", "terminal": "S1", "after": "3"}),
        )
        .unwrap();
        call(
            &mut a,
            "strips",
            json!({"action": "spare", "strip": "-X1", "terminal": "S2"}),
        )
        .unwrap();
        assert_eq!(undo_mark(&a), before + 3);
        assert_eq!(rail(&mut a), ["3", "S1*", "1", "2", "S2*"]);

        // A level, shown on the terminal; the one above is taken as its upper level.
        call(
            &mut a,
            "strips",
            json!({"action": "level", "strip": "-X1", "terminal": "2", "level": 2}),
        )
        .unwrap();
        assert_eq!(undo_mark(&a), before + 4);
        assert_eq!(rail(&mut a), ["3", "S1*", "1@1", "2@2", "S2*"]);
        call(
            &mut a,
            "strips",
            json!({"action": "level", "strip": "-X1", "terminal": "2", "level": null}),
        )
        .unwrap();
        assert_eq!(undo_mark(&a), before + 5);
        assert_eq!(rail(&mut a), ["3", "S1*", "1", "2", "S2*"]);

        // Unspare, then reset: each one undo step.
        call(
            &mut a,
            "strips",
            json!({"action": "unspare", "strip": "-X1", "terminal": "S1"}),
        )
        .unwrap();
        assert_eq!(undo_mark(&a), before + 6);
        assert_eq!(rail(&mut a), ["3", "1", "2", "S2*"]);
        call(&mut a, "strips", json!({"action": "reset", "strip": "-X1"})).unwrap();
        assert_eq!(undo_mark(&a), before + 7);
        assert_eq!(rail(&mut a), ["1", "2", "3", "S2*"]);
        assert!(a.open.as_mut().unwrap().undo());
        assert_eq!(rail(&mut a), ["3", "1", "2", "S2*"]);
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
    fn the_black_box_tool_puts_a_tagged_box_with_its_points_on_the_page() {
        use crate::black_box_tool::BoxDraft;
        use wc_core::BoxSide;
        use wc_core::geom::Point;
        let (_tmp, mut a) = app(vec![]);
        a.ed.page = 1;
        a.run(Action::BlackBoxTool);
        assert_eq!(a.ed.tool, crate::state::Tool::BlackBox);
        assert!(a.add_black_box("-A7").is_err(), "nothing drawn yet");

        let page_id = a.open.as_ref().unwrap().project.pages[1].id.clone();
        let mut d = BoxDraft::new(&page_id, Point::new(30.0, 30.0));
        d.to = Some(Point::new(50.0, 40.0));
        d.add_pin(BoxSide::Left, 5.0, "1").unwrap();
        d.add_pin(BoxSide::Right, 5.0, "2").unwrap();
        a.ed.black_box = Some(d);
        let err = a.add_black_box("not a tag").unwrap_err();
        assert!(!err.is_empty());
        assert!(a.ed.black_box.is_some(), "a refused tag keeps the box being drawn");

        a.add_black_box("-A7").unwrap();
        assert!(a.ed.black_box.is_none());
        let o = a.open.as_ref().unwrap();
        let [crate::state::Selection::Symbol(id)] = a.ed.selection.as_slice() else {
            unreachable!("the new box is selected: {:?}", a.ed.selection)
        };
        let placed = o.project.pages[1].symbol(id).unwrap();
        assert_eq!(placed.tag.as_ref().map(|t| t.to_string()).as_deref(), Some("-A7"));
        let pins: Vec<&str> = o.symbols[&placed.symbol].pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(pins, ["1", "2"]);

        // Undo takes the whole box away in one step.
        a.run(Action::Undo);
        let o = a.open.as_ref().unwrap();
        assert!(o.project.pages[1].symbols.is_empty());

        // The view moved to another page while the tag dialog was open: the box goes on the
        // page it was drawn on, and that page is shown with the box selected.
        let mut d = BoxDraft::new(&page_id, Point::new(30.0, 30.0));
        d.to = Some(Point::new(50.0, 40.0));
        a.ed.black_box = Some(d);
        a.ed.page = 0;
        a.add_black_box("").unwrap();
        assert_eq!(a.ed.page, 1);
        let o = a.open.as_ref().unwrap();
        let [crate::state::Selection::Symbol(id)] = a.ed.selection.as_slice() else {
            unreachable!("the new box is selected: {:?}", a.ed.selection)
        };
        assert!(o.project.pages[1].symbol(id).is_some());
        assert!(o.project.pages[0].symbols.is_empty());
    }

    #[test]
    fn escape_after_leaving_the_black_box_tool_cancels_the_new_tool_s_drawing() {
        use wc_core::geom::Point;
        let (_tmp, mut a) = app(vec![]);
        let ctx = egui::Context::default();
        let escape = |a: &mut App| {
            let raw = egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                let _ = a.shortcuts(ctx);
            });
        };
        let page_id = a.open.as_ref().unwrap().project.pages[0].id.clone();
        a.run(Action::BlackBoxTool);
        a.ed.black_box = Some(crate::black_box_tool::BoxDraft::new(&page_id, Point::new(10.0, 10.0)));
        escape(&mut a);
        assert!(a.ed.black_box.is_none(), "Esc drops the box being drawn");
        assert_eq!(a.ed.tool, crate::state::Tool::BlackBox, "and keeps the tool");

        // Started a box, switched to Line and clicked its first point: Esc cancels the line.
        a.ed.black_box = Some(crate::black_box_tool::BoxDraft::new(&page_id, Point::new(10.0, 10.0)));
        a.run(Action::LineTool);
        a.ed.shape_points = vec![Point::new(20.0, 20.0)];
        escape(&mut a);
        assert!(a.ed.shape_points.is_empty(), "the line's first point is dropped");
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
                show_layers: false,
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
            Action::CircleTool,
            Action::ArcTool,
            Action::PolylineTool,
            Action::DimensionTool,
            Action::BlackBoxTool,
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
    fn find_says_why_a_hit_on_a_hidden_layer_is_not_selected() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        let id = open.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let page = open.project.pages[1].id.clone();
        open.project.pages[1].graphics.push(wc_core::Shape::Text {
            at: Point::new(60.0, 60.0),
            text: "Pump 7 supply".into(),
            height: 3.5,
            align: Default::default(),
            angle: 0.0,
        });
        // The hit comes from a real search for the free text, as the Find window makes it.
        let mut find = crate::find_window::FindState::default();
        find.query = "Pump 7".into();
        find.refresh(&open.project);
        let hit = Selection::Graphic(0);
        assert_eq!(
            find.hits.first().and_then(crate::find_window::go_to),
            Some(crate::find_window::FindOutcome::Go {
                page: page.clone(),
                select: Some(hit.clone()),
            }),
            "{:?}",
            find.hits
        );
        a.find = Some(find);
        a.prefs.layers.set_shown(crate::layers::Layer::Drawings, false);
        a.show_found(&page, Some(hit));
        assert!(a.ed.selection.is_empty());
        assert!(
            a.ed.message.contains("hidden layer Drawings and notes"),
            "{}",
            a.ed.message
        );
        assert_eq!(a.find.as_ref().unwrap().message, a.ed.message);
        // The next hit, which can be selected, clears the note in both places.
        a.show_found(&page, Some(Selection::Symbol(id.clone())));
        assert_eq!(a.ed.selection, vec![Selection::Symbol(id)]);
        assert!(a.ed.message.is_empty(), "{}", a.ed.message);
        assert!(a.find.as_ref().unwrap().message.is_empty());
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
    fn add_graphic_draws_a_dimension_and_a_filled_polyline() {
        let (_tmp, mut a) = app(vec![]);
        let spec = specs().into_iter().find(|t| t["name"] == "add_graphic").unwrap();
        let kinds = &spec["input_schema"]["properties"]["shape"]["properties"]["kind"]["enum"];
        assert!(kinds.as_array().unwrap().iter().any(|k| k == "dimension"));
        let dimension = json!({"kind": "dimension", "a": {"x": 10, "y": 20}, "b": {"x": 50, "y": 20}, "offset": 8});
        call(&mut a, "add_graphic", json!({"page": "p0002", "shape": dimension})).unwrap();
        let arrow = json!({"kind": "polyline", "points": [{"x": 0, "y": 0}, {"x": 4, "y": 1}, {"x": 0, "y": 2}], "filled": true});
        call(&mut a, "add_graphic", json!({"page": "p0002", "shape": arrow})).unwrap();
        let graphics = &a.open.as_ref().unwrap().project.pages[1].graphics;
        assert_eq!(
            graphics[graphics.len() - 2],
            Shape::Dimension {
                a: Point::new(10.0, 20.0),
                b: Point::new(50.0, 20.0),
                offset: 8.0,
                height: 2.5,
            }
        );
        assert!(matches!(
            graphics[graphics.len() - 1],
            Shape::Polyline { filled: true, .. }
        ));
    }

    /// `run_command`'s description names every top-level command of the command line, so the
    /// assistant knows each one exists (the names come from `enum Cmd` in wc-cli's main.rs).
    #[test]
    fn run_command_names_every_command_line_command() {
        // A Windows checkout may have CRLF line endings; normalise so the delimiters match.
        let cli = include_str!("../../wc-cli/src/main.rs").replace("\r\n", "\n");
        let body = cli.split("\nenum Cmd {\n").nth(1).unwrap();
        let body = &body[..body.find("\n}\n").unwrap()];
        let commands: Vec<String> = body
            .lines()
            .filter_map(|l| l.strip_prefix("    "))
            .filter(|l| l.starts_with(|c: char| c.is_ascii_uppercase()))
            .map(|l| {
                let name = l.split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap();
                let mut kebab = String::new();
                for (i, c) in name.chars().enumerate() {
                    if c.is_ascii_uppercase() && i > 0 {
                        kebab.push('-');
                    }
                    kebab.push(c.to_ascii_lowercase());
                }
                kebab
            })
            .collect();
        assert!(commands.len() >= 30, "only found {commands:?}");
        let spec = specs().into_iter().find(|t| t["name"] == "run_command").unwrap();
        let text = spec["description"].as_str().unwrap();
        let named = |w: &str| {
            text.match_indices(w).any(|(i, _)| {
                let before = text[..i].chars().next_back();
                let after = text[i + w.len()..].chars().next();
                let edge = |c: Option<char>| c.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '-');
                edge(before) && edge(after)
            })
        };
        let missing: Vec<&String> = commands.iter().filter(|c| !named(c)).collect();
        assert!(missing.is_empty(), "run_command does not name {missing:?}");
    }

    #[test]
    fn a_graphic_named_by_position_is_refused_after_a_sync_reorders_the_page() {
        let line = |x: f64| Shape::Line {
            a: Point::new(x, 0.0),
            b: Point::new(x, 5.0),
        };
        let rect = Shape::Rect {
            min: Point::new(0.0, 0.0),
            max: Point::new(5.0, 5.0),
        };
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        open.project.pages[1].graphics = vec![line(1.0), line(2.0)];
        a.share.start_host(open, "Hanna", "hanna@example.com").unwrap();
        let graphics = |a: &App| a.open.as_ref().unwrap().project.pages[1].graphics.clone();
        // The assistant reads the page, and its own edits keep what it read up to date: after
        // adding a line, that line is g2.
        call(&mut a, "get_page", json!({"page": "p0002"})).unwrap();
        call(&mut a, "add_graphic", json!({"page": "p0002", "shape": line(3.0)})).unwrap();
        call(
            &mut a,
            "move_items",
            json!({"page": "p0002", "ids": ["g2"], "dx": 1, "dy": 0}),
        )
        .unwrap();
        a.sync_session_now(false);
        assert_eq!(graphics(&a), vec![line(1.0), line(2.0), line(4.0)]);
        // The first line goes and is synced; the assistant reads the page: the second is g0.
        let open = a.open.as_mut().unwrap();
        open.checkpoint();
        open.project.pages[1].graphics.remove(0);
        a.sync_session_now(false);
        call(&mut a, "get_page", json!({"page": "p0002"})).unwrap();
        // The session's undo puts the first line back in front: g0 now names another line, so
        // every write by position is refused.
        a.share.undo().unwrap();
        a.sync_session_now(false);
        assert_eq!(graphics(&a), vec![line(1.0), line(2.0), line(4.0)]);
        let shape = serde_json::to_value(&rect).unwrap();
        for (tool, input) in [
            ("update_graphic", json!({"page": "p0002", "id": "g0", "shape": shape})),
            ("delete_items", json!({"page": "p0002", "ids": ["g0"]})),
            ("move_items", json!({"page": "p0002", "ids": ["g0"], "dx": 1, "dy": 0})),
            ("copy_items", json!({"page": "p0002", "ids": ["g0"], "dx": 1, "dy": 0})),
        ] {
            let err = call(&mut a, tool, input).unwrap_err();
            assert!(err.contains("changed since you read it"), "{tool}: {err}");
        }
        assert_eq!(graphics(&a), vec![line(1.0), line(2.0), line(4.0)], "nothing written");
        // Read again, the same position is the line now there.
        call(&mut a, "get_page", json!({"page": "p0002"})).unwrap();
        call(
            &mut a,
            "update_graphic",
            json!({"page": "p0002", "id": "g0", "shape": shape}),
        )
        .unwrap();
        assert_eq!(graphics(&a), vec![rect, line(2.0), line(4.0)]);
    }

    /// A guest window over the session of `crate::share::tests::session`, showing a panel
    /// over page 3: it holds no page, so a change of page 3 waits for the host.
    fn guest_window(tmp: &std::path::Path) -> (tempfile::TempDir, App, crate::share::Share, Open) {
        let (mut h, _log, mut host_open, mut g, mut guest_open) = crate::share::tests::session(tmp).unwrap();
        crate::share::tests::frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
        let (dir, mut a) = app(vec![]);
        a.open = Some(guest_open);
        a.share = g;
        a.ed.page = 2;
        a.ed.panel = Some(0);
        a.sync_session_now(false);
        (dir, a, h, host_open)
    }

    /// The guest pastes a line on page 3 and selects it.
    fn paste_line(a: &mut App) -> Shape {
        let line = Shape::Line {
            a: Point::new(7.0, 0.0),
            b: Point::new(7.0, 5.0),
        };
        let page = a.open.as_mut().unwrap().project.page_mut("p0003").unwrap();
        page.graphics.push(line.clone());
        a.ed.selection = vec![Selection::Graphic(page.graphics.len() - 1)];
        line
    }

    fn graphics3(a: &App) -> Vec<Shape> {
        a.open.as_ref().unwrap().project.page("p0003").unwrap().graphics.clone()
    }

    #[test]
    fn a_guests_pasted_drawing_is_selected_again_once_the_host_accepts_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (_dir, mut a, mut h, mut host_open) = guest_window(tmp.path());
        let line = paste_line(&mut a);
        // Sent: until the host answers, the page shows the session's copy without it.
        a.sync_session_now(false);
        a.sync_session_now(false);
        assert!(graphics3(&a).is_empty());
        assert!(a.ed.selection.is_empty(), "no stale index stays selected");
        h.frame(Some(&mut host_open), Some("p0002".into()), false);
        a.sync_session_now(false);
        assert_eq!(graphics3(&a), vec![line]);
        assert_eq!(a.ed.selection, vec![Selection::Graphic(0)]);
        assert!(a.ed.waiting_graphics.is_none());
    }

    #[test]
    fn a_selection_made_while_a_paste_waits_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let (_dir, mut a, mut h, mut host_open) = guest_window(tmp.path());
        paste_line(&mut a);
        a.sync_session_now(false);
        a.ed.selection = vec![Selection::Symbol("other".into())];
        h.frame(Some(&mut host_open), Some("p0002".into()), false);
        a.sync_session_now(false);
        assert_eq!(graphics3(&a).len(), 1);
        assert_eq!(a.ed.selection, vec![Selection::Symbol("other".into())]);
    }

    #[test]
    fn a_later_paste_ends_the_wait_for_an_earlier_one() {
        let tmp = tempfile::tempdir().unwrap();
        let (_dir, mut a, mut h, mut host_open) = guest_window(tmp.path());
        let first = paste_line(&mut a);
        a.sync_session_now(false);
        // A second paste, selected on its own, also waits for the host.
        let page = a.open.as_mut().unwrap().project.page_mut("p0003").unwrap();
        page.graphics.push(Shape::Line {
            a: Point::new(8.0, 0.0),
            b: Point::new(8.0, 5.0),
        });
        a.ed.selection = vec![Selection::Graphic(page.graphics.len() - 1)];
        a.sync_session_now(false);
        for _ in 0..3 {
            h.frame(Some(&mut host_open), Some("p0002".into()), false);
            a.sync_session_now(false);
        }
        // The first paste is back, but the user had moved on from it: it is not selected.
        assert!(graphics3(&a).contains(&first));
        let at = graphics3(&a).iter().position(|g| *g == first).unwrap();
        assert!(
            !a.ed.selection.contains(&Selection::Graphic(at)),
            "{:?}",
            a.ed.selection
        );
    }

    #[test]
    fn a_selected_drawing_that_moves_does_not_end_the_wait() {
        let tmp = tempfile::tempdir().unwrap();
        let (_dir, mut a, mut h, mut host_open) = guest_window(tmp.path());
        let line = |x: f64| Shape::Line {
            a: Point::new(x, 0.0),
            b: Point::new(x, 5.0),
        };
        // A drawing already on the page, then a paste; both are selected.
        let page = host_open.project.page_mut("p0003").unwrap();
        page.graphics.push(line(1.0));
        h.frame(Some(&mut host_open), Some("p0002".into()), false);
        a.sync_session_now(false);
        assert_eq!(graphics3(&a), vec![line(1.0)]);
        let pasted = paste_line(&mut a);
        a.ed.selection = vec![Selection::Graphic(0), Selection::Graphic(1)];
        a.sync_session_now(false);
        assert_eq!(a.ed.selection, vec![Selection::Graphic(0)]);
        // The host takes the paste and puts another drawing in front before the guest looks.
        h.frame(Some(&mut host_open), Some("p0002".into()), false);
        let page = host_open.project.page_mut("p0003").unwrap();
        page.graphics.insert(0, line(2.0));
        h.frame(Some(&mut host_open), Some("p0002".into()), false);
        a.sync_session_now(false);
        let now = graphics3(&a);
        let at = |g: &Shape| Selection::Graphic(now.iter().position(|x| x == g).unwrap());
        let mut sel = a.ed.selection.clone();
        sel.sort_by_key(|s| format!("{s:?}"));
        let mut want = vec![at(&line(1.0)), at(&pasted)];
        want.sort_by_key(|s| format!("{s:?}"));
        assert_eq!(sel, want, "{now:?}");
    }

    #[test]
    fn an_equal_drawing_the_host_adds_is_not_taken_for_a_refused_paste() {
        let tmp = tempfile::tempdir().unwrap();
        let (_dir, mut a, mut h, mut host_open) = guest_window(tmp.path());
        h.frame(Some(&mut host_open), Some("p0003".into()), false);
        let line = paste_line(&mut a);
        a.sync_session_now(false);
        assert!(a.ed.waiting_graphics.is_some(), "the paste waits for the host");
        // The host refuses it and draws the same line itself; the guest sees both at once.
        h.frame(Some(&mut host_open), Some("p0003".into()), false);
        host_open.project.page_mut("p0003").unwrap().graphics.push(line.clone());
        h.frame(Some(&mut host_open), Some("p0003".into()), false);
        a.sync_session_now(false);
        assert_eq!(graphics3(&a), vec![line]);
        assert!(a.ed.selection.is_empty(), "the host's line is not the guest's paste");
        assert!(a.ed.waiting_graphics.is_none());
    }

    #[test]
    fn an_equal_drawing_that_comes_with_a_late_refusal_is_not_taken_for_the_paste() {
        let tmp = tempfile::tempdir().unwrap();
        let (_dir, mut a, mut h, mut host_open) = guest_window(tmp.path());
        h.frame(Some(&mut host_open), Some("p0003".into()), false);
        let line = paste_line(&mut a);
        a.sync_session_now(false);
        assert!(a.ed.waiting_graphics.is_some(), "the paste waits for the host");
        // The host refuses the paste and draws the same line itself; the refusal reaches the
        // guest only after its frame looked at the picks (in the frame's late pump).
        h.frame(Some(&mut host_open), Some("p0003".into()), false);
        host_open.project.page_mut("p0003").unwrap().graphics.push(line.clone());
        h.frame(Some(&mut host_open), Some("p0003".into()), false);
        a.share.refusals_come_late = true;
        a.sync_session_now(false);
        assert_eq!(graphics3(&a), vec![line]);
        assert!(a.ed.selection.is_empty(), "the host's line is not the guest's paste");
        assert!(a.ed.waiting_graphics.is_none());
    }

    #[test]
    fn a_guests_paste_the_host_refuses_leaves_nothing_selected() {
        let tmp = tempfile::tempdir().unwrap();
        let (_dir, mut a, mut h, mut host_open) = guest_window(tmp.path());
        // The host opens page 3 before the guest's paste reaches it.
        h.frame(Some(&mut host_open), Some("p0003".into()), false);
        paste_line(&mut a);
        a.sync_session_now(false);
        assert!(a.ed.waiting_graphics.is_some(), "the paste waits for the host");
        for _ in 0..3 {
            h.frame(Some(&mut host_open), Some("p0003".into()), false);
            a.sync_session_now(false);
        }
        assert!(graphics3(&a).is_empty());
        assert!(a.ed.selection.is_empty());
        assert!(a.ed.waiting_graphics.is_none(), "nothing waits once the host refused");
    }

    #[test]
    fn a_guests_tool_edit_says_it_waits_for_the_host_and_is_taken_back_when_refused() {
        let line = Shape::Line {
            a: Point::new(1.0, 0.0),
            b: Point::new(1.0, 5.0),
        };
        let tmp = tempfile::tempdir().unwrap();
        let (mut h, _log, mut host_open, mut g, mut guest_open) = crate::share::tests::session(tmp.path()).unwrap();
        crate::share::tests::frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0001"));
        let (_tmp, mut a) = app(vec![]);
        a.open = Some(guest_open);
        a.share = g;
        a.ed.page = 0;
        a.sync_session_now(false);
        // Reading changes nothing, so nothing waits on the host.
        let out = call(&mut a, "get_page", json!({"page": "p0003"})).unwrap();
        assert!(!out.contains("Pending"), "{out}");
        // The host opens page 3; before that reaches the guest, its assistant draws there.
        h.frame(Some(&mut host_open), Some("p0003".into()), false);
        let out = call(&mut a, "add_graphic", json!({"page": "p0003", "shape": line})).unwrap();
        assert!(out.contains("Pending") && out.contains("host accepts"), "{out}");
        for _ in 0..4 {
            a.sync_session_now(false);
            h.frame(Some(&mut host_open), Some("p0003".into()), false);
        }
        let graphics = |o: &Open| o.project.page("p0003").unwrap().graphics.clone();
        assert!(graphics(&host_open).is_empty(), "the host refused the edit");
        assert!(
            graphics(a.open.as_ref().unwrap()).is_empty(),
            "and it is taken back here"
        );
        // Refused outright when the hold is known.
        let err = call(&mut a, "add_graphic", json!({"page": "p0003", "shape": line})).unwrap_err();
        assert!(err.contains("Hanna"), "{err}");
    }

    /// A guest's window in a session with the host showing page 2 and the guest page 1.
    fn guest_app(tmp: &std::path::Path) -> (tempfile::TempDir, App) {
        let (mut h, _log, mut host_open, mut g, mut guest_open) = crate::share::tests::session(tmp).unwrap();
        crate::share::tests::frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0001"));
        let (dir, mut a) = app(vec![]);
        a.open = Some(guest_open);
        a.share = g;
        a.ed.page = 0;
        a.sync_session_now(false);
        (dir, a)
    }

    #[test]
    fn a_guests_tool_call_that_changes_nothing_is_not_pending() {
        let tmp = tempfile::tempdir().unwrap();
        let (_dir, mut a) = guest_app(tmp.path());
        let out = call(
            &mut a,
            "place_symbol",
            json!({"symbol": "wc.coil", "x": 50, "y": 50, "page": "3"}),
        )
        .unwrap();
        assert!(out.contains("Pending"), "{out}");
        let id = a.open.as_ref().unwrap().project.page("p0003").unwrap().symbols[0]
            .id
            .clone();
        // An update that names only the symbol takes an undo step but changes nothing.
        let out = call(&mut a, "update_symbol", json!({"page": "3", "id": id})).unwrap();
        assert!(!out.contains("Pending"), "{out}");
    }

    #[test]
    fn a_guests_command_that_changes_the_project_is_pending_when_it_ends() {
        let tmp = tempfile::tempdir().unwrap();
        let (_dir, mut a) = guest_app(tmp.path());
        // As the assistant runs it: started, then finished through the tools.
        let through_tools = |a: &mut App, meanwhile: &dyn Fn(&mut App)| {
            let args = json!({"args": ["--help"]});
            let Started::Running(p) = wc_agent::Tools::start(&mut AppTools(a), "run_command", &args) else {
                return "not started".to_owned();
            };
            meanwhile(a);
            let r = p.wait();
            wc_agent::Tools::finish(&mut AppTools(a), "run_command", &args, r).unwrap_or_else(|e| e)
        };
        let r = through_tools(&mut a, &|a| command_writes_customer(a, "Disk"));
        assert_eq!(customer(&a).as_deref(), Some("Disk"));
        assert!(r.contains("Pending"), "{r}");
        // A command that leaves the project alone is not.
        let r = through_tools(&mut a, &|_| {});
        assert!(!r.contains("Pending"), "{r}");
        // Nor is one during which the user moved a jumpered terminal in the window: its jumper
        // follows when the command ends, but that is the window's change, not the command's.
        let o = a.open.as_mut().unwrap();
        let page = o.project.pages.iter().position(|p| p.id == "p0003").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut ids = Vec::new();
        for at in [10.0, 20.0, 30.0] {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            o.project.pages[page]
                .symbols
                .iter_mut()
                .find(|s| s.id == id)
                .unwrap()
                .tag = Some(x1.clone());
            ids.push(id);
        }
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        let moved = ids[0].clone();
        let r = through_tools(&mut a, &|a| {
            let o = a.open.as_mut().unwrap();
            o.checkpoint();
            o.project.pages[page]
                .symbols
                .iter_mut()
                .find(|s| s.id == moved)
                .unwrap()
                .at = Point::new(40.0, 20.0);
        });
        assert_eq!(
            a.open.as_ref().unwrap().project.meta.jumpers[0].terminals,
            ["1", "3"],
            "the jumper followed"
        );
        assert!(!r.contains("Pending"), "{r}");
    }

    #[test]
    fn a_hosts_tool_edit_is_not_pending() {
        let tmp = tempfile::tempdir().unwrap();
        let (h, _log, host_open, ..) = crate::share::tests::session(tmp.path()).unwrap();
        let (_tmp, mut a) = app(vec![]);
        a.open = Some(host_open);
        a.share = h;
        let out = call(&mut a, "add_page", json!({"title": "Pumps"})).unwrap();
        assert!(!out.contains("Pending"), "{out}");
    }

    #[test]
    fn revision_clouds_and_the_table_are_named_cut_and_followed_safely() {
        use crate::state::RevisionRow;
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        open.project.add_revision("A", "2026-11-19", "First issue", "").unwrap();
        let p = |x, y| Point::new(x, y);
        open.add_cloud(1, p(20.0, 20.0), p(60.0, 40.0)).unwrap();
        open.add_cloud(1, p(100.0, 20.0), p(140.0, 40.0)).unwrap();
        open.place_revision_table(1, p(200.0, 150.0)).unwrap();
        let line = Shape::Line {
            a: p(0.0, 0.0),
            b: p(5.0, 0.0),
        };
        open.project.pages[1].graphics = vec![line.clone()];
        a.ed.page = 1;

        // The ids the state gives are taken back.
        let page = a.open.as_ref().unwrap().project.pages[1].number.clone();
        call(
            &mut a,
            "move_items",
            json!({"page": page, "ids": ["revision table"], "dx": 10, "dy": 0}),
        )
        .unwrap();
        assert_eq!(
            a.open.as_ref().unwrap().project.pages[1].revision_table,
            Some(p(210.0, 150.0))
        );
        // A cloud is named by what it is, as the state shows it when selected.
        a.ed.selection = vec![Selection::Cloud(1)];
        let second = cloud_id(&a.open.as_ref().unwrap().project.pages[1].clouds[1]);
        assert_eq!(second, "cloud A at 100,20 to 140,40");
        assert!(a.agent_context().contains(&second), "{}", a.agent_context());
        // get_state gives the same id.
        let state: Value = serde_json::from_str(&call(&mut a, "get_state", json!({})).unwrap()).unwrap();
        assert_eq!(state["selection"], json!([second]));
        call(
            &mut a,
            "move_items",
            json!({"page": page, "ids": [second], "dx": 1, "dy": 0}),
        )
        .unwrap();
        assert_eq!(a.open.as_ref().unwrap().project.pages[1].clouds[1].min, p(101.0, 20.0));
        // An id read before the cloud changed names nothing now, rather than another cloud.
        let err = call(
            &mut a,
            "move_items",
            json!({"page": page, "ids": [second], "dx": 1, "dy": 0}),
        )
        .unwrap_err();
        assert!(err.contains("No item"), "{err}");

        // Cut of a drawing with a cloud and the table: the drawing goes to the clipboard, the
        // cloud and the table (which Paste cannot bring back) stay, selected.
        a.ed.selection = vec![Selection::Graphic(0), Selection::Cloud(0), Selection::RevisionTable];
        a.run(Action::Cut);
        let pg = &a.open.as_ref().unwrap().project.pages[1];
        assert!(pg.graphics.is_empty());
        assert_eq!((pg.clouds.len(), pg.revision_table.is_some()), (2, true));
        assert_eq!(a.ed.selection, [Selection::Cloud(0), Selection::RevisionTable]);
        // Paste brings the drawing back; the cloud and the table were never gone.
        a.run(Action::Paste);
        let pg = &a.open.as_ref().unwrap().project.pages[1];
        assert_eq!(pg.graphics.len(), 1, "the drawing is pasted back");
        assert_eq!((pg.clouds.len(), pg.revision_table.is_some()), (2, true));

        // The assistant deletes the first cloud while it is selected on screen: the other, now
        // in the first place, does not become selected.
        let page = a.open.as_ref().unwrap().project.pages[1].number.clone();
        let second = a.open.as_ref().unwrap().project.pages[1].clouds[1].clone();
        a.ed.selection = vec![Selection::Cloud(0)];
        let pg = &a.open.as_ref().unwrap().project.pages[1];
        let first = cloud_id(&pg.clouds[0]);
        call(&mut a, "delete_items", json!({"page": page, "ids": [first]})).unwrap();
        assert!(
            a.ed.selection.is_empty(),
            "the other cloud is not selected: {:?}",
            a.ed.selection
        );
        // The deleted cloud's id, used again, does not name the cloud now in its place.
        let err = call(&mut a, "delete_items", json!({"page": page, "ids": [first]})).unwrap_err();
        assert!(err.contains("No item"), "{err}");
        a.ed.selection.clear();
        assert_eq!(
            a.open.as_ref().unwrap().project.pages[1].clouds,
            std::slice::from_ref(&second)
        );
        a.open.as_mut().unwrap().undo();

        // A selected cloud follows its cloud when one before it goes.
        let old = a.open.as_ref().unwrap().project.pages[1].clouds.clone();
        let new = vec![old[1].clone()];
        let mut sel = vec![Selection::Cloud(1), Selection::Cloud(0), Selection::RevisionTable];
        crate::edit::remap_clouds(&mut sel, &old, &new);
        assert_eq!(sel, [Selection::Cloud(0), Selection::RevisionTable]);

        // Revision rows edited after someone else changed the revisions are not applied.
        let open = a.open.as_mut().unwrap();
        let loaded = RevisionRow::of(&open.project.meta.revisions);
        let mut draft = loaded.clone();
        draft[0].text = "First issue, corrected".into();
        assert_eq!(open.revision_draft_problem(&loaded, &draft), None);
        open.project
            .add_revision("B", "2026-11-20", "Added elsewhere", "")
            .unwrap();
        let why = open.revision_draft_problem(&loaded, &draft).unwrap();
        assert!(why.contains("changed by someone else"), "{why}");
        assert_eq!(
            open.revision_draft_problem(&loaded, &loaded),
            None,
            "rows left alone are not applied"
        );
    }

    #[test]
    fn a_selected_cloud_follows_its_cloud_when_the_session_puts_one_back_before_it() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        open.project.add_revision("A", "2026-11-19", "First issue", "").unwrap();
        for x in [20.0, 100.0, 180.0] {
            open.add_cloud(1, Point::new(x, 20.0), Point::new(x + 40.0, 40.0))
                .unwrap();
        }
        let clouds = open.project.pages[1].clouds.clone();
        a.share.start_host(open, "Hanna", "hanna@example.com").unwrap();
        a.ed.page = 1;
        // Take the first cloud off; the second is then cloud 0, and selected.
        let open = a.open.as_mut().unwrap();
        open.checkpoint();
        open.project.pages[1].clouds.remove(0);
        a.sync_session_now(false);
        a.ed.selection = vec![Selection::Cloud(0)];
        // The session's undo puts the first cloud back in front: the selection follows the
        // second cloud rather than staying on the place it had.
        a.share.undo().unwrap();
        a.sync_session_now(false);
        assert_eq!(a.open.as_ref().unwrap().project.pages[1].clouds, clouds);
        assert_eq!(a.ed.selection, vec![Selection::Cloud(1)]);
    }

    #[test]
    fn the_pdf_export_draws_a_topology_page_per_network() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        open.project.meta.networks.push(wc_core::network::Network {
            name: "PN1".into(),
            links: vec![wc_core::network::Link {
                from: "-A1:X1".into(),
                to: "-K10:P1".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let m = crate::export_pdf(a.open.as_ref().unwrap());
        assert!(m.starts_with("Exported"), "{m}");
        let dir = a.open.as_ref().unwrap().dir.join("export");
        let pdf = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "pdf"))
            .unwrap();
        let text = String::from_utf8_lossy(&std::fs::read(pdf).unwrap()).into_owned();
        let title: String = "N1 Network PN1".bytes().map(|b| format!("{b:02X}")).collect();
        assert!(text.contains(&format!("/Title <{title}>")));
    }

    #[test]
    fn the_assistant_exports_the_wago_lists_and_a_pdf_in_another_language() {
        let (_tmp, mut a) = app(vec![]);
        let dir = a.open.as_ref().unwrap().dir.join("export");
        let m = call(&mut a, "export", json!({"format": "wago-configurator"})).unwrap();
        assert!(m.starts_with("Nothing exported"), "{m}");
        let open = a.open.as_mut().unwrap();
        for (x, tag) in [(20.0, "-X1.2"), (40.0, "-X1.1"), (60.0, "-X2.1")] {
            let id = open.place(1, "wc.terminal", Point::new(x, 20.0)).unwrap();
            let s = open.project.pages[1].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(wc_core::DeviceTag::parse(tag).unwrap());
        }
        let m = call(&mut a, "export", json!({"format": "wago-configurator"})).unwrap();
        assert!(m.contains("Exported 2 terminal strips (3 rows)"), "{m}");
        assert!(dir.join("wago-configurator.csv").exists());

        let m = call(
            &mut a,
            "export",
            json!({"format": "wago-marking", "strip": "-X1", "lang": "de", "kinds": ["terminals"], "lines": ["marker", "tag"]}),
        )
        .unwrap();
        assert!(m.starts_with("Exported 2 markers"), "{m}");
        let csv = std::fs::read_to_string(dir.join("wago-marking--X1.csv")).unwrap();
        assert!(csv.starts_with("\u{feff}Typ;Gruppe;Zeile 1;Zeile 2;"), "{csv}");
        let m = call(&mut a, "export", json!({"format": "wago-marking"})).unwrap();
        assert!(m.starts_with("Exported "), "{m}");
        assert!(dir.join("wago-marking.csv").exists());
        assert!(call(&mut a, "export", json!({"format": "wago-marking", "lang": "fr"})).is_err());
        assert!(call(&mut a, "export", json!({"format": "wago-marking", "kinds": ["lamps"]})).is_err());
        // A member that is not a string, or a value that is not a list, is refused by name.
        let err = call(
            &mut a,
            "export",
            json!({"format": "wago-marking", "kinds": ["terminals", 42]}),
        )
        .unwrap_err();
        assert!(err.contains("`kinds` holds 42"), "{err}");
        let err = call(
            &mut a,
            "export",
            json!({"format": "wago-marking", "lines": ["marker", null]}),
        )
        .unwrap_err();
        assert!(err.contains("`lines` holds null"), "{err}");
        let err = call(
            &mut a,
            "export",
            json!({"format": "wago-marking", "kinds": "terminals"}),
        )
        .unwrap_err();
        assert!(err.contains("`kinds` must be a list"), "{err}");
        let err = call(&mut a, "export", json!({"format": "wago-marking", "lines": {"a": 1}})).unwrap_err();
        assert!(err.contains("`lines` must be a list"), "{err}");

        let project_dir = a.open.as_ref().unwrap().dir.clone();
        a.open.as_mut().unwrap().project.pages[1].title = "Pumps".into();
        std::fs::write(
            project_dir.join(wc_core::translate::TRANSLATIONS_FILE),
            "source = \"en\"\n\n[[text]]\nen = \"Pumps\"\nnb = \"Pumper\"\n",
        )
        .unwrap();
        assert!(call(&mut a, "export", json!({"format": "pdf", "lang": "nb,en,de"})).is_err());
        let m = call(&mut a, "export", json!({"format": "pdf", "lang": "nb"})).unwrap();
        assert!(m.starts_with("Exported"), "{m}");
        let pdf = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "pdf"))
            .unwrap();
        let text = String::from_utf8_lossy(&std::fs::read(pdf).unwrap()).into_owned();
        let pumper: String = "Pumper".bytes().map(|b| format!("{b:02X}")).collect();
        assert!(
            text.contains(&format!("<{pumper}>")),
            "the page title is printed in Norwegian"
        );
    }

    #[test]
    fn a_macros_placeholders_and_value_sets_are_edited_in_the_dialog_and_each_variant_inserted() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        let id = open.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let mut clip = open.copy(1, &[Selection::Symbol(id)]);
        clip.graphics.push(wc_core::Shape::Text {
            at: Point::new(20.0, 40.0),
            text: "{{kW}} kW".into(),
            height: 2.5,
            align: Default::default(),
            angle: 0.0,
        });
        // Saved with an undeclared placeholder, it cannot be inserted yet.
        let path = open.save_macro(&clip, "Starter", "").unwrap();
        assert!(open.insert_macro(1, &path, None).is_err());

        a.run(Action::InsertMacro);
        let Some(Dialog::Macros(list)) = &a.dialog else {
            unreachable!("the macros are listed");
        };
        assert!(list[0].error.is_some(), "the undeclared placeholder is shown");
        a.edit_macro(&path);
        let Some(Dialog::MacroEdit { mut form, .. }) = a.dialog.take() else {
            unreachable!("the placeholders dialog opens");
        };
        form.add_placeholder();
        form.placeholders[0].name = "kW".into();
        form.placeholders[0].description = "Motor power".into();
        // A second placeholder of the same name is refused.
        form.add_placeholder();
        form.placeholders[1].name = "KW".into();
        let err = form.save().unwrap_err();
        assert!(err.contains("declared twice"), "{err}");
        form.remove_placeholder(1);
        form.add_set();
        form.sets[0].name = "Small".into();
        form.sets[0].values[0] = "0.75".into();
        form.add_set();
        form.sets[1].name = "Large".into();
        // A value for a placeholder without a name is refused.
        form.add_placeholder();
        form.sets[1].values[1] = "x".into();
        let err = form.save().unwrap_err();
        assert!(err.contains("without a name"), "{err}");
        form.remove_placeholder(1);
        form.sets[1].values[0] = "3".into();
        form.save().unwrap();

        // Renamed in the dialog, the placeholder is renamed in the macro's text.
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        assert_eq!(form.sets.len(), 2);
        assert_eq!(form.sets[1].values, ["3"]);
        form.placeholders[0].name = "power".into();
        form.save().unwrap();
        let m = wc_core::macros::read_macro(&path).unwrap();
        assert_eq!(m.used_placeholders(), ["power"]);
        assert_eq!(m.value_set("small").unwrap().values["power"], "0.75");

        let open = a.open.as_mut().unwrap();
        let entry = open.macros().into_iter().find(|e| e.path == path).unwrap();
        assert_eq!(
            (entry.error, entry.value_sets),
            (None, vec!["Small".to_owned(), "Large".to_owned()])
        );
        for (set, text) in [("Small", "0.75 kW"), ("Large", "3 kW")] {
            open.insert_macro(1, &path, Some(set)).unwrap();
            assert!(
                open.project.pages[1]
                    .graphics
                    .iter()
                    .any(|g| matches!(g, wc_core::Shape::Text { text: t, .. } if t == text)),
                "{set} is inserted with {text}"
            );
        }
    }

    #[test]
    fn the_placeholders_dialog_keeps_what_it_does_not_edit_and_renames_together() {
        use wc_core::macros::{Macro, Placeholder, ValueSet};
        let (tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        let text = |t: &str| wc_core::Shape::Text {
            at: Point::new(0.0, 0.0),
            text: t.into(),
            height: 2.5,
            align: Default::default(),
            angle: 0.0,
        };
        let declare = |name: &str, default: &str| Placeholder {
            name: name.into(),
            description: String::new(),
            default: default.into(),
        };
        let m = Macro {
            name: "Swap".into(),
            graphics: vec![text("{{a}}"), text("{{b}}"), text("{{Ø}}")],
            placeholders: vec![declare("a", "1"), declare("b", "2"), declare("Ø", "fallback")],
            value_sets: vec![ValueSet {
                name: "One".into(),
                description: String::new(),
                // Stored under a name that differs only in case (not ASCII), and empty.
                values: [("ø", ""), ("a", "x")]
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
            }],
            ..Default::default()
        };
        let path = wc_core::macros::write_macro(&open.dir, "swap", &m).unwrap();

        // Opened and saved untouched, the empty value stays a value, not the default.
        let form = crate::macro_form::Form::open(&path).unwrap();
        assert_eq!(form.sets[0].values, ["x", "", ""]);
        form.save().unwrap();
        let saved = wc_core::macros::read_macro(&path).unwrap();
        assert_eq!(
            saved.value_set("one").unwrap().values.get("Ø").map(String::as_str),
            Some("")
        );
        let placed = saved.apply(Some("One"), &Default::default()).unwrap();
        assert!(
            placed
                .graphics
                .iter()
                .all(|g| !matches!(g, wc_core::Shape::Text { text, .. } if text == "fallback"))
        );
        // Typed in and cleared in the dialog, a cell uses the default again.
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        let ctx = egui::Context::default();
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let cell = crate::macro_form::cell_id(0, 2);
        for events in [
            vec![],
            vec![egui::Event::Text("7".into())],
            vec![key(egui::Key::Backspace)],
            vec![],
        ] {
            let raw = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| crate::macro_form::show(ui, &mut form));
                ctx.memory_mut(|m| m.request_focus(cell));
            });
        }
        assert_eq!(form.sets[0].values[2], "");
        assert!(
            !form.sets[0].stored_empty[2],
            "edited, it is no longer the stored empty value"
        );
        form.save().unwrap();
        let saved = wc_core::macros::read_macro(&path).unwrap();
        assert_eq!(saved.value_set("one").unwrap().values.get("Ø"), None);

        // Two placeholders swapped: each use takes the other's name, once.
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.placeholders[0].name = "b".into();
        form.placeholders[1].name = "a".into();
        form.save().unwrap();
        let saved = wc_core::macros::read_macro(&path).unwrap();
        assert_eq!(saved.used_placeholders(), ["b", "a", "Ø"]);
        assert_eq!(
            saved.value_set("one").unwrap().values.get("b").map(String::as_str),
            Some("x")
        );

        // A value for a name no placeholder declares is reported, not dropped.
        let mut m = saved.clone();
        m.value_sets[0].values.insert("voltage".into(), "400".into());
        wc_core::macros::write_macro(&open.dir, "swap", &m).unwrap();
        let form = crate::macro_form::Form::open(&path).unwrap();
        let err = form.save().unwrap_err();
        assert!(err.contains("voltage"), "the undeclared value is reported: {err}");
        assert_eq!(
            wc_core::macros::read_macro(&path).unwrap(),
            m,
            "and the file is left as it was"
        );

        // A file changed outside the dialog after it opened is not written over.
        wc_core::macros::write_macro(&open.dir, "swap", &saved).unwrap();
        let form = crate::macro_form::Form::open(&path).unwrap();
        let mut changed = saved.clone();
        changed.description = "Edited elsewhere".into();
        wc_core::macros::write_macro(&open.dir, "swap", &changed).unwrap();
        let err = form.save().unwrap_err();
        assert!(err.contains("changed outside"), "{err}");
        assert_eq!(wc_core::macros::read_macro(&path).unwrap(), changed);
        // A file that cannot be read again says why, not that it changed.
        let form = crate::macro_form::Form::open(&path).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        let err = form.save().unwrap_err();
        assert!(err.contains("could not be read again"), "{err}");
        wc_core::macros::write_macro(&open.dir, "swap", &changed).unwrap();

        // A library source named "project" is still a library: its macros are not the project's.
        let lib = tmp.path().join("lib");
        wc_core::macros::write_macro(&lib, "lamp", &saved).unwrap();
        open.project.meta.libraries.push(wc_core::project::LibrarySource {
            name: "project".into(),
            kind: "folder".into(),
            location: lib.display().to_string(),
            reference: None,
        });
        let list = open.macros();
        let of = |p: &std::path::Path| {
            list.iter()
                .find(|e| e.path.starts_with(p))
                .map(|e| (e.source.as_str(), e.project))
        };
        assert_eq!(of(&path), Some(("project", true)));
        assert_eq!(of(&lib), Some(("project", false)));
    }

    #[test]
    fn translations_edited_in_the_dialog_are_saved_and_the_pdf_is_exported_in_the_language_chosen() {
        use wc_core::translate::{Dictionary, TRANSLATIONS_FILE};
        let hex = |s: &str| {
            let mut out = String::from("<");
            for b in s.bytes() {
                out.push_str(&format!("{b:02X}"));
            }
            out + ">"
        };
        let (_tmp, mut a) = app(vec![]);
        let dir = a.open.as_ref().unwrap().dir.clone();
        a.open.as_mut().unwrap().project.pages[1].title = "Pumps".into();
        a.open.as_mut().unwrap().project.pages[1]
            .graphics
            .push(wc_core::Shape::Text {
                at: Point::new(20.0, 20.0),
                text: "Spare".into(),
                height: 2.5,
                align: Default::default(),
                angle: 0.0,
            });
        // Without a dictionary the PDF is exported at once, as drawn.
        a.run(Action::ExportPdf);
        assert!(matches!(&a.dialog, Some(Dialog::Message(m)) if m.starts_with("Exported")));
        std::fs::write(
            dir.join(TRANSLATIONS_FILE),
            "source = \"en\"\n\n[[text]]\nen = \"Pumps\"\nnb = \"Pumper\"\n",
        )
        .unwrap();

        a.dialog = None;
        a.run(Action::Translations);
        let Some(Dialog::Translations(mut form)) = a.dialog.take() else {
            unreachable!("the translations dialog opens");
        };
        assert_eq!(form.langs, ["nb"]);
        let row = |f: &crate::translations_form::Form, t: &str| f.rows.iter().position(|r| r.text == t).unwrap();
        assert_eq!(form.rows[row(&form, "Pumps")].cells, ["Pumper"]);
        assert_eq!(form.missing(0), form.rows.len() - 1, "every text but Pumps is missing");
        let spare = row(&form, "Spare");
        form.rows[spare].cells[0] = "Reserve".into();
        form.new_lang = "DE".into();
        form.add_language().unwrap();
        assert!(form.add_language().is_err(), "an empty code is refused");
        form.new_lang = "nb".into();
        assert!(form.add_language().is_err(), "a language already shown is refused");
        form.rows[spare].cells[1] = "Ersatz".into();
        // Edited in the file meanwhile: kept, as the dialog did not change it.
        std::fs::write(
            dir.join(TRANSLATIONS_FILE),
            "source = \"en\"\n\n[[text]]\nen = \"Pumps\"\nnb = \"Pumpene\"\n",
        )
        .unwrap();
        assert_eq!(form.save().unwrap(), "Saved 2 translations to translations.toml.");
        let d = Dictionary::load(&dir).unwrap();
        assert_eq!(d.lookup("Spare", "nb").as_deref(), Some("Reserve"));
        assert_eq!(d.lookup("Spare", "de").as_deref(), Some("Ersatz"));
        assert_eq!(d.lookup("Pumps", "nb").as_deref(), Some("Pumpene"));

        // With translations, the export asks for the language.
        a.run(Action::ExportPdf);
        let Some(Dialog::PdfLanguage { langs, .. }) = &a.dialog else {
            unreachable!("the language is asked for");
        };
        assert_eq!(langs, &["en", "nb", "de"]);
        let open = a.open.as_ref().unwrap();
        let m = crate::export_pdf_in(open, &["nb".into(), "en".into()]);
        assert!(m.starts_with("Exported"), "{m}");
        assert!(m.contains("no translation for"), "missing texts are warned about: {m}");
        assert!(!m.contains("\"Spare\""), "{m}");
        // The message is logged, so it names where the text is used but not the text itself.
        let dict = Dictionary::load(&dir).unwrap();
        let (_, missing) = wc_core::translate::translate(&open.project, &dict, &["nb".into(), "en".into()]);
        assert!(!missing.is_empty());
        for t in &missing {
            assert!(
                !m.contains(&format!("{:?}", t.text)),
                "drawing text stays out of the logged message: {m}"
            );
        }
        let pdf_path = std::fs::read_dir(dir.join("export"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "pdf"))
            .unwrap();
        let pdf = String::from_utf8_lossy(&std::fs::read(pdf_path).unwrap()).into_owned();
        assert!(pdf.contains(&hex("Reserve / Spare")), "the texts are in both languages");
        assert!(pdf.contains(&hex("Pumpene / Pumps")));
    }

    #[test]
    fn the_rules_tool_refuses_a_selector_that_names_two_library_rules() {
        let (tmp, mut a) = app(vec![]);
        let odd = tmp.path().join("odd");
        std::fs::create_dir_all(&odd).unwrap();
        let rule = |name: &str| format!("[[rule]]\nname = \"{name}\"\nkind = \"cable\"\n\n");
        let text = ["2", "tags", "dup", "dup"].map(rule).concat();
        std::fs::write(odd.join("rules.toml"), text).unwrap();
        a.open.as_mut().unwrap().project.meta.libraries = vec![wc_core::project::LibrarySource {
            name: "odd".into(),
            kind: "folder".into(),
            location: odd.to_string_lossy().into(),
            reference: None,
        }];
        let off = |a: &App| a.open.as_ref().unwrap().project.meta.rules_off.clone();
        let is_off = |a: &mut App| -> Vec<bool> {
            let list: Value = serde_json::from_str(&call(a, "rules", json!({"action": "list"})).unwrap()).unwrap();
            let rules = list["library"][0]["rules"].as_array().unwrap().clone();
            rules.iter().map(|r| r["off"] == true).collect()
        };
        let rules = |a: &mut App, action: &str, rule: &str| call(a, "rules", json!({"action": action, "rule": rule}));

        // "2" is rule 2 by number and rule 1 by name: refused, nothing changes.
        rules(&mut a, "off", "odd:tags").unwrap();
        for action in ["on", "off"] {
            let r = rules(&mut a, action, "odd:2");
            assert!(r.as_ref().is_err_and(|e| e.contains("odd:1 or odd:2")), "{r:?}");
        }
        assert_eq!(off(&a), vec!["odd:tags".to_owned()]);
        rules(&mut a, "on", "odd:tags").unwrap();
        assert_eq!(is_off(&mut a), [false; 4], "list agrees with the tool");

        // Two rules named dup: refused by name, each kept apart by number.
        assert!(rules(&mut a, "off", "odd:dup").is_err());
        rules(&mut a, "off", "odd:3").unwrap();
        rules(&mut a, "off", "odd:4").unwrap();
        rules(&mut a, "on", "odd:3").unwrap();
        assert_eq!(is_off(&mut a), [false, false, false, true]);
        assert!(rules(&mut a, "on", "odd:dup").is_err());

        // An entry turning off two rules goes when either is turned on, so list shows both on.
        a.open.as_mut().unwrap().project.meta.rules_off = vec!["odd:2".into()];
        assert_eq!(is_off(&mut a), [true, true, false, false]);
        rules(&mut a, "on", "odd:1").unwrap();
        assert_eq!(is_off(&mut a), [false; 4]);

        // A rules file that cannot be read: off says why; on still drops the entry as written.
        std::fs::write(odd.join("rules.toml"), "[[rule]\n").unwrap();
        let r = rules(&mut a, "off", "odd:1");
        assert!(r.as_ref().is_err_and(|e| e.contains("cannot be read")), "{r:?}");
        assert!(off(&a).is_empty());
        a.open.as_mut().unwrap().project.meta.rules_off = vec!["odd:1".into()];
        rules(&mut a, "on", "odd:1").unwrap();
        assert!(off(&a).is_empty());
    }

    #[test]
    fn the_rules_tool_lists_sets_and_turns_library_rules_off_and_on() {
        let (tmp, mut a) = app(vec![]);
        let house = tmp.path().join("house");
        std::fs::create_dir_all(&house).unwrap();
        std::fs::write(
            house.join("rules.toml"),
            "[[rule]]\nkind = \"cable\"\n\n[[rule]]\nname = \"tags\"\nkind = \"tag\"\npattern = \"-QQ#\"\n",
        )
        .unwrap();
        a.open.as_mut().unwrap().project.meta.libraries = vec![wc_core::project::LibrarySource {
            name: "house".into(),
            kind: "folder".into(),
            location: house.to_string_lossy().into(),
            reference: None,
        }];
        a.open.as_mut().unwrap().project.pages[1]
            .symbols
            .push(wc_core::project::PlacedSymbol {
                id: "s1".into(),
                symbol: "wc.coil".into(),
                at: Point::new(50.0, 50.0),
                rotation: Default::default(),
                mirror: false,
                tag: DeviceTag::parse("-K1").ok(),
            });
        let meta = |a: &App| a.open.as_ref().unwrap().project.meta.clone();
        let check = |a: &mut App| call(a, "run_check", json!({})).unwrap();

        let list: Value = serde_json::from_str(&call(&mut a, "rules", json!({"action": "list"})).unwrap()).unwrap();
        assert_eq!(list["library"][0]["rules"][1]["rule"], "house:2");
        assert_eq!(list["library"][0]["rules"][1]["off"], false);
        let tags = "check rule 2 \"tags\" of library house";
        assert!(check(&mut a).contains(tags), "the library rule runs");

        // Set: kept as the dialog keeps them, and run by the check.
        call(
            &mut a,
            "rules",
            json!({"action": "set", "rules": [{"kind": "tag", "pattern": "-ZZ#", "severity": "error", "message": "{tag} breaks the house rule"}]}),
        )
        .unwrap();
        assert_eq!(meta(&a).rules[0].pattern.as_deref(), Some("-ZZ#"));
        assert!(check(&mut a).contains("-K1 breaks the house rule"));
        // The dialog's checks refuse a rule the check could not run, and a misspelt key.
        for bad in [
            json!([{"kind": "tag"}]),
            json!([{"kind": "max-wires"}]),
            json!([{"kind": "cable", "role": "bogus"}]),
            json!([{"kind": "cable", "severty": "error"}]),
            json!([{"kind": "nope"}]),
        ] {
            let r = call(&mut a, "rules", json!({"action": "set", "rules": bad}));
            assert!(r.as_ref().is_err_and(|e| e.starts_with("Rule 1")), "{bad}: {r:?}");
        }
        assert_eq!(meta(&a).rules.len(), 1, "a refused list changes nothing");

        // Off by name, on by number: both entries that turn it off go.
        call(&mut a, "rules", json!({"action": "off", "rule": "house:tags"})).unwrap();
        assert_eq!(meta(&a).rules_off, vec!["house:tags".to_owned()]);
        assert!(!check(&mut a).contains(tags));
        assert!(call(&mut a, "rules", json!({"action": "off", "rule": "house:9"})).is_err());
        call(&mut a, "rules", json!({"action": "off", "rule": "house:1"})).unwrap();
        let list: Value = serde_json::from_str(&call(&mut a, "rules", json!({"action": "list"})).unwrap()).unwrap();
        assert_eq!(list["library"][0]["rules"][1]["off"], true);
        call(&mut a, "rules", json!({"action": "on", "rule": "house:2"})).unwrap();
        assert_eq!(meta(&a).rules_off, vec!["house:1".to_owned()]);

        // Each change is one undo step; saved into project.toml.
        let o = a.open.as_mut().unwrap();
        o.save().unwrap();
        let toml = std::fs::read_to_string(o.dir.join("project.toml")).unwrap();
        assert!(toml.contains("rules_off") && toml.contains("-ZZ#"), "{toml}");
        assert!(o.undo());
        assert_eq!(o.project.meta.rules_off.len(), 2);
        assert!(o.undo() && o.undo());
        assert!(o.project.meta.rules_off.is_empty());
        assert!(o.undo());
        assert!(o.project.meta.rules.is_empty());
    }

    #[test]
    fn check_rules_added_changed_and_deleted_in_the_dialog_are_kept_checked_and_undone() {
        use crate::rules_form::{Change, RuleRow, change, rows_of};
        use wc_core::check_rule::CheckRule;
        let (_tmp, mut a) = app(vec![]);
        let rules = |a: &App| a.open.as_ref().unwrap().project.meta.rules.clone();
        let broken = |a: &App| match &a.dialog {
            Some(Dialog::Check(found)) => Some(found.iter().filter(|f| f.message.contains("house rule")).count()),
            _ => None,
        };
        a.open.as_mut().unwrap().project.pages[1]
            .symbols
            .push(wc_core::project::PlacedSymbol {
                id: "s1".into(),
                symbol: "wc.coil".into(),
                at: Point::new(50.0, 50.0),
                rotation: Default::default(),
                mirror: false,
                tag: DeviceTag::parse("-K1").ok(),
            });
        assert!(rules(&a).is_empty());
        // Add: every tag must match a pattern none does.
        a.edit_rules();
        let Some(Dialog::Rules { mut rows, loaded, dir }) = a.dialog.take() else {
            unreachable!("the rules dialog opens");
        };
        let mut row = RuleRow::new("tag");
        row.pattern = "-ZZ#".into();
        row.severity = "error".into();
        row.message = "{tag} breaks the house rule".into();
        rows.push(row);
        a.apply_rules(&rows, &loaded, &dir).unwrap();
        assert_eq!(rules(&a).len(), 1);
        assert!(
            broken(&a).unwrap() > 0,
            "the check list has the rule's findings at once"
        );
        a.open.as_mut().unwrap().save().unwrap();
        let toml = std::fs::read_to_string(a.open.as_ref().unwrap().dir.join("project.toml")).unwrap();
        assert!(toml.contains("[[rule]]") && toml.contains("-ZZ#"), "{toml}");

        // Change: a pattern every tag matches leaves no finding.
        let loaded = rules(&a);
        let mut rows = rows_of(&loaded);
        rows[0].pattern = "*".into();
        a.apply_rules(&rows, &loaded, &dir).unwrap();
        assert_eq!(rules(&a)[0].pattern.as_deref(), Some("*"));
        assert_eq!(broken(&a), Some(0));

        // Delete, then undo back to each earlier list.
        let loaded = rules(&a);
        a.apply_rules(&[], &loaded, &dir).unwrap();
        assert!(rules(&a).is_empty());
        let o = a.open.as_mut().unwrap();
        assert!(o.undo());
        assert_eq!(o.project.meta.rules[0].pattern.as_deref(), Some("*"));
        assert!(o.undo());
        assert_eq!(o.project.meta.rules[0].pattern.as_deref(), Some("-ZZ#"));
        assert!(o.undo());
        assert!(o.project.meta.rules.is_empty());

        // Reorder two rules: kept in the new order, and undo brings back the old one.
        let two = vec![
            CheckRule {
                kind: "cable".into(),
                ..CheckRule::default()
            },
            CheckRule {
                kind: "tag".into(),
                pattern: Some("*".into()),
                ..CheckRule::default()
            },
        ];
        a.open.as_mut().unwrap().set_check_rules(two.clone());
        let mut rows = rows_of(&two);
        change(&mut rows, Change::Up(1));
        a.apply_rules(&rows, &two, &dir).unwrap();
        let kinds = |a: &App| rules(a).iter().map(|r| r.kind.clone()).collect::<Vec<_>>();
        assert_eq!(kinds(&a), ["tag", "cable"]);
        assert!(a.open.as_mut().unwrap().undo());
        assert_eq!(rules(&a), two);

        // Rows left as loaded keep a change made meanwhile, and are no undo step: also when a
        // loaded rule has a value the dialog would not write (a cable rule's max).
        let odd = vec![CheckRule {
            kind: "cable".into(),
            max: Some(2),
            ..CheckRule::default()
        }];
        let o = a.open.as_mut().unwrap();
        o.project.meta.rules = [odd.clone(), two.clone()].concat();
        let mark = o.undo_mark();
        a.apply_rules(&rows_of(&odd), &odd, &dir).unwrap();
        assert_eq!(rules(&a), [odd.clone(), two.clone()].concat());
        assert_eq!(a.open.as_ref().unwrap().undo_mark(), mark);

        // A row edited while another rule changed in the project: nothing is written, and
        // the dialog stays open with the edit, now against the project's rules.
        let mut rows = rows_of(&two);
        rows[0].devices = "-M*".into();
        let now = rules(&a);
        let err = a.apply_rules(&rows, &two, &dir).err().unwrap();
        assert!(err.contains("changed while you edited"), "{err}");
        assert_eq!(rules(&a), now);
        let Some(Dialog::Rules {
            rows: kept,
            loaded: against,
            ..
        }) = a.dialog.take()
        else {
            unreachable!("the draft stays open");
        };
        assert_eq!((kept.clone(), against.clone()), (rows, now));
        // OK again keeps the draft.
        a.apply_rules(&kept, &against, &dir).unwrap();
        assert_eq!(rules(&a)[0].devices.as_deref(), Some("-M*"));

        // Another project open now does not get the rules.
        let err = a.apply_rules(&[], &odd, Path::new("elsewhere")).err().unwrap();
        assert!(err.contains("another project"), "{err}");
        assert_eq!(rules(&a).len(), 2);
    }

    #[test]
    fn the_assistant_reads_sets_and_resets_a_report_layout_as_one_undo_step_each() {
        let (_tmp, mut a) = app(vec![]);
        let get =
            |a: &mut App, v: Value| -> Value { serde_json::from_str(&call(a, "report_layouts", v).unwrap()).unwrap() };
        let reports = |a: &App| a.open.as_ref().unwrap().project.meta.reports.clone();
        let all = get(&mut a, json!({"action": "get"}));
        assert_eq!(all.as_array().map(Vec::len), Some(wc_report::Kind::ALL.len()));
        let parts = get(&mut a, json!({"action": "get", "kind": "parts"}));
        assert_eq!(parts[0]["layout"], Value::Null);
        let builtin: Vec<String> = serde_json::from_value(parts[0]["builtin_columns"].clone()).unwrap();
        assert!(builtin.len() >= 2, "{builtin:?}");

        // Bad input changes nothing.
        for bad in [
            json!({"action": "set", "kind": "parts"}),
            json!({"action": "set", "layout": {}}),
            json!({"action": "set", "kind": "nope", "layout": {}}),
            json!({"action": "set", "kind": "parts", "layout": {"column": "Tag"}}),
            json!({"action": "shuffle", "kind": "parts"}),
        ] {
            assert!(call(&mut a, "report_layouts", bad.clone()).is_err(), "{bad}");
        }
        assert!(reports(&a).is_empty());

        // Unknown keys, at the top or in a column entry, are named and change nothing, even
        // over a kept layout; an empty layout is the built-in one.
        let tag = json!({"action": "set", "kind": "parts", "layout": {"title": "Kept"}});
        call(&mut a, "report_layouts", tag).unwrap();
        let before = reports(&a);
        assert_eq!(before.len(), 1);
        for (layout, key) in [
            (json!({"columns": [{"from": builtin[0]}]}), "\"columns\""),
            (json!({"column": [{"from": builtin[0], "title": "X"}]}), "\"title\""),
        ] {
            let err = call(
                &mut a,
                "report_layouts",
                json!({"action": "set", "kind": "parts", "layout": layout}),
            )
            .unwrap_err();
            assert!(err.contains(key), "{err}");
            assert_eq!(reports(&a), before);
        }
        call(
            &mut a,
            "report_layouts",
            json!({"action": "set", "kind": "parts", "layout": {}}),
        )
        .unwrap();
        assert!(reports(&a).is_empty());
        let mark = a.open.as_ref().unwrap().undo_mark();

        // Set: kept as the dialog keeps it (trimmed), with wc_report's warnings in the answer.
        let said = call(
            &mut a,
            "report_layouts",
            json!({"action": "set", "kind": "parts", "layout": {
                "title": " Bill of materials ",
                "column": [{"from": builtin[1], "heading": "Second"}, {"from": builtin[0]}],
                "sort": [format!("-{}", builtin[0])],
                "group": "Maker",
                "header": ["project.name", "revison"]
            }}),
        )
        .unwrap();
        assert!(said.contains("cannot group by \"Maker\""), "{said}");
        assert!(said.contains("header \"revison\""), "{said}");
        let kept = reports(&a);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].kind, "parts");
        assert_eq!(kept[0].title.as_deref(), Some("Bill of materials"));
        assert_eq!(kept[0].columns.len(), 2);
        assert_eq!(kept[0].columns[0].heading.as_deref(), Some("Second"));
        assert_eq!(kept[0].sort, [format!("-{}", builtin[0])]);
        let read = get(&mut a, json!({"action": "get", "kind": "parts"}));
        assert_eq!(read[0]["layout"]["title"], json!("Bill of materials"));
        assert_eq!(read[0]["warnings"].as_array().map(Vec::len), Some(2));
        assert!(a.open.as_mut().unwrap().undo());
        assert!(reports(&a).is_empty(), "one undo step");
        assert_eq!(a.open.as_ref().unwrap().undo_mark(), mark);
        assert!(a.open.as_mut().unwrap().redo());
        assert_eq!(reports(&a), kept);

        // A layout that shows the built-in one keeps none, as in the dialog.
        let every: Vec<Value> = builtin.iter().map(|c| json!({"from": c})).collect();
        call(
            &mut a,
            "report_layouts",
            json!({"action": "set", "kind": "parts", "layout": {"column": every}}),
        )
        .unwrap();
        assert!(reports(&a).is_empty());
        assert!(a.open.as_mut().unwrap().undo());
        assert_eq!(reports(&a), kept);

        // Reset: back to the built-in layout, one undo step.
        let said = call(&mut a, "report_layouts", json!({"action": "reset", "kind": "parts"})).unwrap();
        assert!(said.contains("built-in"), "{said}");
        assert!(reports(&a).is_empty());
        assert!(a.open.as_mut().unwrap().undo());
        assert_eq!(reports(&a), kept);
    }

    #[test]
    fn a_report_layout_made_in_the_dialog_is_kept_previewed_saved_and_undone() {
        use wc_report::Kind;
        let (_tmp, mut a) = app(vec![]);
        let reports = |a: &App| a.open.as_ref().unwrap().project.meta.reports.clone();
        let inputs_table = |a: &App| {
            let o = a.open.as_ref().unwrap();
            wc_report::build(
                Kind::Parts,
                &wc_report::Inputs {
                    project: &o.project,
                    symbols: &o.symbols,
                    parts: &o.parts,
                },
            )
        };
        let builtin = inputs_table(&a).columns;
        assert!(builtin.len() >= 2, "{builtin:?}");
        let mark = a.open.as_ref().unwrap().undo_mark();

        a.edit_report_layout(Kind::Parts);
        let Some(Dialog::ReportLayout { mut form, dir }) = a.dialog.take() else {
            unreachable!("the layout dialog opens");
        };
        assert_eq!(form.builtin, builtin);
        // OK on the untouched form keeps no layout and changes nothing.
        a.apply_report_layout(&form, &dir).unwrap();
        assert!(reports(&a).is_empty());
        assert_eq!(a.open.as_ref().unwrap().undo_mark(), mark);
        assert!(
            matches!(a.dialog, Some(Dialog::Reports(Kind::Parts))),
            "back to the report"
        );

        form.title = "Bill of materials".into();
        form.move_column(1, -1);
        form.columns[0].heading = "First".into();
        let last = form.columns.len() - 1;
        form.columns[last].shown = false;
        form.sort.push(crate::report_layout_form::SortRow {
            column: builtin[0].clone(),
            descending: true,
        });
        a.apply_report_layout(&form, &dir).unwrap();
        let kept = reports(&a);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].kind, "parts");
        assert_eq!(kept[0].sort, [format!("-{}", builtin[0])]);
        // The report shows in it.
        let t = inputs_table(&a);
        assert_eq!(t.title, "Bill of materials");
        assert_eq!(t.columns[0], "First");
        assert_eq!(t.columns.len(), builtin.len() - 1);
        a.open.as_mut().unwrap().save().unwrap();
        let toml = std::fs::read_to_string(a.open.as_ref().unwrap().dir.join("project.toml")).unwrap();
        assert!(
            toml.contains("[[report]]") && toml.contains("Bill of materials"),
            "{toml}"
        );

        // One undo step takes it back.
        assert!(a.open.as_mut().unwrap().undo());
        assert!(reports(&a).is_empty());
        assert!(a.open.as_mut().unwrap().redo());
        assert_eq!(reports(&a), kept);

        // Edited while the project's layout changed: nothing is written and the draft stays open.
        a.edit_report_layout(Kind::Parts);
        let Some(Dialog::ReportLayout { mut form, dir }) = a.dialog.take() else {
            unreachable!("the layout dialog opens");
        };
        a.open.as_mut().unwrap().project.meta.reports[0].title = Some("Theirs".into());
        form.title = "Mine".into();
        let err = a.apply_report_layout(&form, &dir).err().unwrap();
        assert!(err.contains("changed while you edited"), "{err}");
        assert_eq!(reports(&a)[0].title.as_deref(), Some("Theirs"));
        let Some(Dialog::ReportLayout { form: again, .. }) = a.dialog.take() else {
            unreachable!("the draft stays open");
        };
        assert_eq!(again.title, "Mine");
        a.apply_report_layout(&again, &dir).unwrap();
        assert_eq!(reports(&a)[0].title.as_deref(), Some("Mine"));

        // Saved "Mine", draft "B", someone saves "Theirs", the draft goes back to "Mine": OK
        // keeps "Mine", the layout shown, in place of "Theirs".
        a.edit_report_layout(Kind::Parts);
        let Some(Dialog::ReportLayout { mut form, dir }) = a.dialog.take() else {
            unreachable!("the layout dialog opens");
        };
        form.title = "B".into();
        a.open.as_mut().unwrap().project.meta.reports[0].title = Some("Theirs".into());
        assert!(a.apply_report_layout(&form, &dir).is_err());
        let Some(Dialog::ReportLayout { form: mut again, .. }) = a.dialog.take() else {
            unreachable!("the draft stays open");
        };
        again.title = "Mine".into();
        a.apply_report_layout(&again, &dir).unwrap();
        assert_eq!(reports(&a)[0].title.as_deref(), Some("Mine"));

        // Back to the built-in layout drops it; another project gets nothing.
        let cleared = crate::report_layout_form::Form::new(Kind::Parts, builtin.clone(), reports(&a).first()).cleared();
        let err = a.apply_report_layout(&cleared, Path::new("elsewhere")).err().unwrap();
        assert!(err.contains("another project"), "{err}");
        // A second layout for the kind (unused) and one of another kind: back to the built-in
        // layout drops both of the kind, keeps the other, and one undo brings them back.
        let mut second = reports(&a)[0].clone();
        second.title = Some("Second".into());
        let other = wc_core::report_layout::ReportLayout {
            kind: "wires".into(),
            title: Some("Wires".into()),
            ..Default::default()
        };
        a.open
            .as_mut()
            .unwrap()
            .project
            .meta
            .reports
            .extend([second, other.clone()]);
        let before = reports(&a);
        // The preview shows a draft without keeping it.
        let o = a.open.as_ref().unwrap();
        let mut draft = before[0].clone();
        draft.title = Some("Draft".into());
        let preview = wc_report::build_with(
            Kind::Parts,
            &wc_report::Inputs {
                project: &o.project,
                symbols: &o.symbols,
                parts: &o.parts,
            },
            Some(&draft),
        );
        assert_eq!(preview.title, "Draft");
        assert_eq!(reports(&a), before);
        a.apply_report_layout(&cleared, &dir).unwrap();
        assert_eq!(reports(&a), [other]);
        assert_eq!(inputs_table(&a).columns, builtin);
        assert!(a.open.as_mut().unwrap().undo());
        assert_eq!(reports(&a), before);

        // A saved layout listing every built-in column as built in: untouched OK keeps it,
        // "Built-in layout" then OK drops it, and undo brings it back.
        let explicit = wc_core::report_layout::ReportLayout {
            kind: "parts".into(),
            columns: builtin
                .iter()
                .map(|c| wc_core::report_layout::ReportColumn {
                    from: c.clone(),
                    heading: None,
                })
                .collect(),
            ..Default::default()
        };
        a.open.as_mut().unwrap().project.meta.reports = vec![explicit.clone()];
        a.edit_report_layout(Kind::Parts);
        let Some(Dialog::ReportLayout { form, dir }) = a.dialog.take() else {
            unreachable!("the layout dialog opens");
        };
        a.apply_report_layout(&form, &dir).unwrap();
        assert_eq!(reports(&a), std::slice::from_ref(&explicit));
        a.apply_report_layout(&form.cleared(), &dir).unwrap();
        assert!(reports(&a).is_empty());
        assert!(a.open.as_mut().unwrap().undo());
        assert_eq!(reports(&a), [explicit]);
    }

    #[test]
    fn graphic_ids_from_the_screen_context_and_from_add_graphic_are_guarded() {
        let line = |x: f64| Shape::Line {
            a: Point::new(x, 0.0),
            b: Point::new(x, 5.0),
        };
        let (_tmp, mut a) = app(vec![]);
        let graphics = |a: &App| a.open.as_ref().unwrap().project.pages[1].graphics.clone();
        a.open.as_mut().unwrap().project.pages[1].graphics = vec![line(1.0), line(2.0)];
        // The user selects the second line on the page shown and sends a message naming it.
        a.ed.page = 1;
        a.ed.selection = vec![Selection::Graphic(1)];
        assert!(a.agent_context().contains("Selected: g1"));
        a.prefs.assistant.backend = wc_agent::Backend::ApiKey;
        a.assistant.input = "Make this line longer".into();
        a.assistant_send(&eframe::egui::Context::default());
        assert!(a.assistant.busy(), "the message is on its way");
        // A sync puts another line in front before the assistant writes to g1.
        a.open.as_mut().unwrap().project.pages[1].graphics.insert(0, line(0.0));
        let err = call(
            &mut a,
            "update_graphic",
            json!({"page": "p0002", "id": "g1", "shape": line(9.0)}),
        )
        .unwrap_err();
        assert!(err.contains("changed since you read it"), "{err}");
        assert_eq!(graphics(&a), vec![line(0.0), line(1.0), line(2.0)], "nothing written");

        // add_graphic on a page the assistant never read: the id it answers is guarded too.
        let first = a.open.as_ref().unwrap().project.pages[0].id.clone();
        a.open.as_mut().unwrap().project.pages[0].graphics = vec![line(1.0)];
        let out = call(&mut a, "add_graphic", json!({"page": first, "shape": line(2.0)})).unwrap();
        assert!(out.contains("g1"), "{out}");
        a.open.as_mut().unwrap().project.pages[0].graphics.insert(0, line(0.0));
        let err = call(
            &mut a,
            "update_graphic",
            json!({"page": first, "id": "g1", "shape": line(9.0)}),
        )
        .unwrap_err();
        assert!(err.contains("changed since you read it"), "{err}");
        // Unchanged since, the answered id can be used.
        call(&mut a, "get_page", json!({"page": first})).unwrap();
        call(&mut a, "add_graphic", json!({"page": first, "shape": line(3.0)})).unwrap();
        call(
            &mut a,
            "update_graphic",
            json!({"page": first, "id": "g3", "shape": line(9.0)}),
        )
        .unwrap();
        assert_eq!(
            a.open.as_ref().unwrap().project.pages[0].graphics,
            vec![line(0.0), line(1.0), line(2.0), line(9.0)]
        );
    }

    #[test]
    fn graphic_ids_are_checked_for_every_tool_and_page_they_name() {
        let line = |x: f64| Shape::Line {
            a: Point::new(x, 0.0),
            b: Point::new(x, 5.0),
        };
        let (_tmp, mut a) = app(vec![]);
        let pages = |a: &App| -> Vec<Vec<Shape>> {
            a.open
                .as_ref()
                .unwrap()
                .project
                .pages
                .iter()
                .map(|p| p.graphics.clone())
                .collect()
        };
        let open = a.open.as_mut().unwrap();
        open.project.pages[0].graphics = vec![line(1.0)];
        open.project.pages[1].graphics = vec![line(2.0), line(3.0)];
        let (first, second) = (open.project.pages[0].id.clone(), open.project.pages[1].id.clone());
        call(&mut a, "get_page", json!({"page": first})).unwrap();
        call(&mut a, "get_page", json!({"page": second})).unwrap();
        // A copy to another page the assistant read keeps that page known too: the id it
        // answers can be used at once.
        let out = call(
            &mut a,
            "copy_items",
            json!({"page": second, "ids": ["g0"], "to_page": first}),
        )
        .unwrap();
        assert!(out.contains("g1"), "{out}");
        call(
            &mut a,
            "update_graphic",
            json!({"page": first, "id": "g1", "shape": line(4.0)}),
        )
        .unwrap();
        assert_eq!(pages(&a)[0], vec![line(1.0), line(4.0)]);
        // Someone else reorders the second page.
        a.open.as_mut().unwrap().project.pages[1].graphics.reverse();
        // An id with a sign is still a position, and saving a macro by position is refused too.
        let err = call(&mut a, "delete_items", json!({"page": second, "ids": ["g+0"]})).unwrap_err();
        assert!(err.contains("changed since you read it"), "{err}");
        let err = call(
            &mut a,
            "macros",
            json!({"action": "save", "page": second, "ids": ["g0"], "name": "Stale"}),
        )
        .unwrap_err();
        assert!(err.contains("changed since you read it"), "{err}");
        assert!(
            !call(&mut a, "macros", json!({"action": "list"}))
                .unwrap()
                .contains("Stale")
        );
        // Deleting the first page while it is shown shows the second: what the assistant read of
        // the second stays as it was, so its old g0 is still refused.
        a.ed.page = 0;
        call(&mut a, "delete_page", json!({})).unwrap();
        let err = call(
            &mut a,
            "update_graphic",
            json!({"page": second, "id": "g0", "shape": line(9.0)}),
        )
        .unwrap_err();
        assert!(err.contains("changed since you read it"), "{err}");
        assert_eq!(pages(&a)[0], vec![line(3.0), line(2.0)]);
    }

    #[test]
    fn graphic_ids_follow_toolbar_edits_and_refused_changes() {
        let line = |x: f64| Shape::Line {
            a: Point::new(x, 0.0),
            b: Point::new(x, 5.0),
        };
        let (_tmp, mut a) = app(vec![]);
        a.open.as_mut().unwrap().project.pages[1].graphics = vec![line(1.0), line(2.0)];
        a.ed.page = 1;
        // Deleted from the toolbar by the assistant: the line left is g0 to it at once.
        call(&mut a, "get_page", json!({"page": "p0002"})).unwrap();
        call(&mut a, "select_items", json!({"page": "p0002", "ids": ["g0"]})).unwrap();
        call(&mut a, "run_action", json!({"action": Action::Delete.name()})).unwrap();
        call(
            &mut a,
            "update_graphic",
            json!({"page": "p0002", "id": "g0", "shape": line(3.0)}),
        )
        .unwrap();
        assert_eq!(page2(&a).graphics, vec![line(3.0)]);
        // A page named with a toolbar action does not count: the action works on the page
        // shown, and that is the page whose drawings the assistant still knows.
        a.open.as_mut().unwrap().project.pages[1].graphics = vec![line(1.0), line(3.0)];
        call(&mut a, "get_page", json!({"page": "p0001"})).unwrap();
        call(&mut a, "get_page", json!({"page": "p0002"})).unwrap();
        call(&mut a, "select_items", json!({"page": "p0002", "ids": ["g0"]})).unwrap();
        call(
            &mut a,
            "run_action",
            json!({"action": Action::Delete.name(), "page": "p0001"}),
        )
        .unwrap();
        assert_eq!(page2(&a).graphics, vec![line(3.0)]);
        call(
            &mut a,
            "update_graphic",
            json!({"page": "p0002", "id": "g0", "shape": line(3.0)}),
        )
        .unwrap();
        // A page change refused because it takes the cover mark from a held page is taken
        // back, and so is what the assistant would have known of its drawings.
        call(&mut a, "update_page", json!({"page": "p0001", "cover": true})).unwrap();
        a.ed.held_by.insert("p0001".into(), "Ola".into());
        a.open
            .as_mut()
            .unwrap()
            .held_by_others
            .insert("p0001".into(), "Ola".into());
        let mut content: Value =
            serde_json::from_str(&call(&mut a, "get_page", json!({"page": "p0002"})).unwrap()).unwrap();
        content["cover"] = json!(true);
        content["graphics"] = json!([line(4.0), line(3.0)]);
        assert!(call(&mut a, "edit_page_json", json!({"page": "p0002", "content": content})).is_err());
        assert_eq!(page2(&a).graphics, vec![line(3.0)], "taken back");
        a.ed.held_by.clear();
        a.open.as_mut().unwrap().held_by_others.clear();
        call(
            &mut a,
            "update_graphic",
            json!({"page": "p0002", "id": "g0", "shape": line(5.0)}),
        )
        .unwrap();
        assert_eq!(page2(&a).graphics, vec![line(5.0)]);
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
    fn get_report_offers_and_reads_every_report() {
        let (_tmp, mut a) = app(vec![]);
        let spec = specs().into_iter().find(|t| t["name"] == "get_report").unwrap();
        let offered = &spec["input_schema"]["properties"]["kind"]["enum"];
        for kind in wc_report::Kind::ALL {
            assert!(
                offered.as_array().unwrap().iter().any(|k| k == kind.name()),
                "{} is not offered",
                kind.name()
            );
            call(&mut a, "get_report", json!({"kind": kind.name()})).unwrap();
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
        // Noted for a shared session, which keeps a rename that waits together.
        let noted: Vec<(String, String)> = a
            .open
            .as_mut()
            .unwrap()
            .take_device_renames()
            .into_iter()
            .flatten()
            .map(|(f, t)| (f.to_string(), t.to_string()))
            .collect();
        assert_eq!(noted, [("=PUMP-K1".to_owned(), "=PUMP-K7".to_owned())]);
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
    fn a_page_edit_without_revision_marks_keeps_them() {
        let (_tmp, mut a) = app(vec![]);
        let cloud = wc_core::revision::RevisionCloud {
            min: wc_core::Point::new(10.0, 10.0),
            max: wc_core::Point::new(40.0, 30.0),
            revision: "B".into(),
        };
        if let Some(p) = a.open.as_mut().unwrap().project.page_mut("p0002") {
            p.clouds.push(cloud.clone());
            p.revision_table = Some(wc_core::Point::new(300.0, 200.0));
        }
        let mut content: Value =
            serde_json::from_str(&call(&mut a, "get_page", json!({"page": "p0002"})).unwrap()).unwrap();
        assert_eq!(content["clouds"][0]["revision"], json!("B"), "get_page lists them");
        let obj = content.as_object_mut().unwrap();
        obj.remove("clouds");
        obj.remove("revision_table");
        content["title"] = json!("Changed");
        call(
            &mut a,
            "edit_page_json",
            json!({"page": "p0002", "content": content.clone()}),
        )
        .unwrap();
        assert_eq!(page2(&a).title, "Changed");
        assert_eq!(page2(&a).clouds, vec![cloud], "kept");
        assert_eq!(
            page2(&a).revision_table,
            Some(wc_core::Point::new(300.0, 200.0)),
            "kept"
        );
        // Given, they are used: `[]` and `null` clear them.
        content["clouds"] = json!([]);
        content["revision_table"] = Value::Null;
        call(&mut a, "edit_page_json", json!({"page": "p0002", "content": content})).unwrap();
        assert!(page2(&a).clouds.is_empty());
        assert_eq!(page2(&a).revision_table, None);
    }

    #[test]
    fn a_page_edit_lets_go_of_a_selected_cloud_it_removes() {
        let (_tmp, mut a) = app(vec![]);
        let cloud = |x: f64| wc_core::revision::RevisionCloud {
            min: wc_core::Point::new(x, 10.0),
            max: wc_core::Point::new(x + 20.0, 30.0),
            revision: "B".into(),
        };
        if let Some(p) = a.open.as_mut().unwrap().project.page_mut("p0002") {
            p.clouds = vec![cloud(10.0), cloud(50.0)];
        }
        let i = a.open.as_ref().unwrap().find_page("p0002").unwrap();
        a.ed.page = i;
        a.ed.selection = vec![Selection::Cloud(0)];
        let mut content: Value =
            serde_json::from_str(&call(&mut a, "get_page", json!({"page": "p0002"})).unwrap()).unwrap();
        content["clouds"] = json!([cloud(50.0)]);
        call(
            &mut a,
            "edit_page_json",
            json!({"page": "p0002", "content": content.clone()}),
        )
        .unwrap();
        assert!(
            a.ed.selection.is_empty(),
            "the remaining cloud is not taken for the removed one"
        );
        // The one that stays stays selected.
        a.ed.selection = vec![Selection::Cloud(0)];
        content["clouds"] = json!([cloud(90.0), cloud(50.0)]);
        call(
            &mut a,
            "edit_page_json",
            json!({"page": "p0002", "content": content.clone()}),
        )
        .unwrap();
        assert_eq!(a.ed.selection, vec![Selection::Cloud(1)]);
        // Editing another page than the shown one lets go of the shown page's selection.
        a.ed.page = a.open.as_ref().unwrap().find_page("p0001").unwrap();
        a.ed.selection = vec![Selection::Cloud(0)];
        call(&mut a, "edit_page_json", json!({"page": "p0002", "content": content})).unwrap();
        assert!(a.ed.selection.is_empty());
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
        // Upright and custom sheets come through the tools too.
        call(&mut a, "add_page", json!({"title": "Upright", "size": "A4-portrait"})).unwrap();
        let size_of = |a: &App, title: &str| {
            let open = a.open.as_ref().unwrap();
            open.project
                .pages
                .iter()
                .find(|p| p.title == title)
                .map(|p| (p.size, p.number.clone()))
        };
        let (size, number) = size_of(&a, "Upright").unwrap();
        assert_eq!(size, PageSize::A4Portrait);
        call(&mut a, "update_page", json!({"page": number, "size": "500x700"})).unwrap();
        assert_eq!(
            size_of(&a, "Upright").map(|(s, _)| s),
            Some(PageSize::Custom {
                width: 500,
                height: 700
            })
        );

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

    /// Start `run_command` with `args` through the tools, as the assistant does: whether it runs,
    /// or the answer it is refused with straight away. A started command is waited for.
    fn command_runs(a: &mut App, args: &[&str]) -> Result<(), String> {
        let v = json!({ "args": args });
        match wc_agent::Tools::start(&mut AppTools(a), "run_command", &v) {
            Started::Running(p) => {
                let r = p.wait();
                let _ = wc_agent::Tools::finish(&mut AppTools(a), "run_command", &v, r);
                Ok(())
            }
            Started::Done(r) => Err(r.err().unwrap_or_else(|| "answered at once".into())),
        }
    }

    /// Every file under `dir` but `.git`, with its bytes.
    fn folder_bytes(dir: &std::path::Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                if e.file_name() == ".git" {
                    continue;
                }
                if e.path().is_dir() {
                    todo.push(e.path());
                } else {
                    out.insert(e.path(), std::fs::read(e.path()).unwrap());
                }
            }
        }
        out
    }

    #[test]
    fn a_writing_command_is_refused_while_someone_else_holds_a_page() {
        let (_tmp, mut a) = app(vec![]);
        let dir = a.open.as_ref().unwrap().dir.clone();
        a.open
            .as_mut()
            .unwrap()
            .held_by_others
            .insert("p0002".into(), "Ola".into());
        let before = folder_bytes(&dir);
        for args in [
            vec!["place", "p0002", "wc.lamp", "--at", "100,50"],
            vec!["page", "add", "Motors"],
            vec!["find", "-K1", "--replace", "-K2"],
            vec!["vcs", "commit", "-m", "x"],
            vec!["check", "-C", "/elsewhere"],
            vec!["check", "--project=/elsewhere"],
        ] {
            let e = command_runs(&mut a, &args).unwrap_err();
            assert!(e.contains("Ola") && e.contains("shared session"), "{args:?}: {e}");
        }
        assert_eq!(folder_bytes(&dir), before, "nothing was written");
        // Without a held page the same command runs.
        a.open.as_mut().unwrap().held_by_others.clear();
        assert!(command_runs(&mut a, &["place", "p0002", "wc.lamp", "--at", "100,50"]).is_ok());
    }

    #[test]
    fn a_file_whose_identity_cannot_be_read_is_refused_with_the_reason() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("project.toml"), "x").unwrap();
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("a.svg"), "y").unwrap();
        let out_path = out.path().display().to_string();
        // A project file whose identity could not be read: a hard link to it would go unseen,
        // so an output folder with a file in it is refused, with the reason.
        let mut guarded = Guarded::new(project.path());
        assert!(guarded.unknown.is_none());
        assert!(!guarded.reached_by(out.path(), &out_path).unwrap());
        guarded.unknown = Some("project.toml: access denied".into());
        let e = guarded.reached_by(out.path(), &out_path).unwrap_err();
        assert!(e.to_string().contains("project.toml: access denied"), "{e}");
        // A folder with nothing in it, or a path not there yet, is still fine.
        let empty = tempfile::tempdir().unwrap();
        let empty_path = empty.path().display().to_string();
        assert!(!guarded.reached_by(empty.path(), &empty_path).unwrap());
        let new_file = empty.path().join("new.svg").display().to_string();
        assert!(!guarded.reached_by(empty.path(), &new_file).unwrap());
        // An output file that cannot be looked at (its folder can be listed but not entered)
        // is an error with the system's reason, not "no match". Run as root it can be.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let guarded = Guarded::new(project.path());
            let shut = out.path().join("shut");
            std::fs::create_dir(&shut).unwrap();
            std::fs::write(shut.join("b.svg"), "z").unwrap();
            std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o600)).unwrap();
            let looked = std::fs::metadata(shut.join("b.svg")).is_ok();
            let r = guarded.reached_by(out.path(), &out_path);
            std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o700)).unwrap();
            if !looked {
                let e = r.unwrap_err();
                assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
            }
            // A project folder that cannot be listed hides its files, so a hard link to one of
            // them is refused with the reason rather than taken as no match.
            let pages = project.path().join("pages");
            std::fs::create_dir(&pages).unwrap();
            std::fs::write(pages.join("p0001.page.json"), "{}").unwrap();
            let linked = tempfile::tempdir().unwrap();
            std::fs::hard_link(pages.join("p0001.page.json"), linked.path().join("a.json")).unwrap();
            let linked_path = linked.path().display().to_string();
            std::fs::set_permissions(&pages, std::fs::Permissions::from_mode(0o000)).unwrap();
            let listed = std::fs::read_dir(&pages).is_ok();
            let blind = Guarded::new(project.path());
            let r = blind.reached_by(linked.path(), &linked_path);
            std::fs::set_permissions(&pages, std::fs::Permissions::from_mode(0o700)).unwrap();
            if !listed {
                let e = r.unwrap_err();
                assert!(e.to_string().contains("pages"), "{e}");
            }
            // Listed, the link is found and an unrelated copy is still fine.
            let seen = Guarded::new(project.path());
            assert!(seen.unknown.is_none());
            assert!(seen.reached_by(linked.path(), &linked_path).unwrap());
            let copy = tempfile::tempdir().unwrap();
            std::fs::write(copy.path().join("a.json"), "{}").unwrap();
            let copy_path = copy.path().display().to_string();
            assert!(!seen.reached_by(copy.path(), &copy_path).unwrap());
            // The same through a link in the project to a folder elsewhere whose parent cannot
            // be searched: where it points is not known, so a hard link to a page there is
            // refused with the reason rather than taken as no match.
            let ext = tempfile::tempdir().unwrap();
            let inner = ext.path().join("inner");
            std::fs::create_dir_all(inner.join("more")).unwrap();
            std::fs::write(inner.join("more/p0002.page.json"), "{}").unwrap();
            std::os::unix::fs::symlink(inner.join("more"), project.path().join("more")).unwrap();
            let far = tempfile::tempdir().unwrap();
            std::fs::hard_link(inner.join("more/p0002.page.json"), far.path().join("b.json")).unwrap();
            let far_path = far.path().display().to_string();
            std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o000)).unwrap();
            let searched = inner.join("more").canonicalize().is_ok();
            let blind = Guarded::new(project.path());
            let r = blind.reached_by(far.path(), &far_path);
            std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o700)).unwrap();
            if !searched {
                let e = r.unwrap_err();
                assert!(e.to_string().contains("more"), "{e}");
            }
            let seen = Guarded::new(project.path());
            assert!(seen.unknown.is_none());
            assert!(seen.reached_by(far.path(), &far_path).unwrap());
            assert!(!seen.reached_by(copy.path(), &copy_path).unwrap());
        }
    }

    #[test]
    fn commands_that_read_the_project_still_run_while_a_page_is_held() {
        let (_tmp, mut a) = app(vec![]);
        let dir = a.open.as_ref().unwrap().dir.clone();
        a.open
            .as_mut()
            .unwrap()
            .held_by_others
            .insert("p0002".into(), "Ola".into());
        let out = tempfile::tempdir().unwrap();
        let elsewhere = out.path().join("out.pdf").display().to_string();
        for args in [
            vec!["check"],
            vec!["report", "parts"],
            vec!["nets"],
            vec!["page", "list"],
            vec!["find", "-K1"],
            vec!["vcs", "log"],
            vec!["pdf", elsewhere.as_str()],
        ] {
            assert!(command_runs(&mut a, &args).is_ok(), "{args:?}");
        }
        // An output written into the project folder could land on a held page, also through
        // a link to it.
        #[cfg(unix)]
        {
            let link = dir.parent().unwrap().join("link-to-project");
            std::os::unix::fs::symlink(&dir, &link).unwrap();
            let through = link.join("pages").join("p0002.page.json").display().to_string();
            let e = command_runs(&mut a, &["plc-vars", through.as_str()]).unwrap_err();
            assert!(e.contains("project folder"), "{e}");
            // `..` after a link goes up from where the link points, not from the link.
            let to_pages = dir.parent().unwrap().join("link-to-pages");
            std::os::unix::fs::symlink(dir.join("pages"), &to_pages).unwrap();
            for name in ["p0002.page.json", "new.svg"] {
                let up = to_pages.join("..").join("pages").join(name).display().to_string();
                let e = command_runs(&mut a, &["vcs", "log", "--svg", up.as_str()]).unwrap_err();
                assert!(e.contains("project folder"), "{up}: {e}");
            }
            // A folder of the project linked to elsewhere is still the project's: both the
            // path through the project and the path where it really is are refused.
            let shared = tempfile::tempdir().unwrap();
            std::fs::write(shared.path().join("p0009.page.json"), "{}").unwrap();
            std::os::unix::fs::symlink(shared.path(), dir.join("linked")).unwrap();
            let deeper = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(deeper.path(), shared.path().join("deeper")).unwrap();
            for to in [
                dir.join("linked").join("p0009.page.json"),
                shared.path().join("new.st"),
                deeper.path().join("new.st"),
            ] {
                let to = to.display().to_string();
                let e = command_runs(&mut a, &["plc-vars", to.as_str()]).unwrap_err();
                assert!(e.contains("project folder"), "{to}: {e}");
            }
            std::fs::remove_file(dir.join("linked")).unwrap();
        }
        // Run from inside the project folder, a bare file name is in it too.
        let held = a.open.as_ref().unwrap().held_by_others.clone();
        let args: Vec<String> = ["plc-vars", "new.st"].map(String::from).to_vec();
        let e = writes_while_held(&args, &dir, &held, &dir.join("pages")).unwrap();
        assert!(e.contains("inside the project folder"), "{e}");
        // Commands with nothing after their own words still run there.
        for words in [
            vec!["check"],
            vec!["nets"],
            vec!["page", "list"],
            vec!["vcs", "log"],
            vec!["find", "-K1"],
        ] {
            let args: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
            assert!(
                writes_while_held(&args, &dir, &held, &dir.join("pages")).is_none(),
                "{args:?}"
            );
        }
        assert!(writes_while_held(&args, &dir, &held, out.path()).is_none());
        // An export writes its own file names into the folder it is given: a file there that
        // links to a held page, by name or as the same file, is a way onto the page.
        // A hard link is the same file under another name, on unix and on Windows alike (#459).
        {
            let page = dir.join("pages").join("p0002.page.json");
            let before = std::fs::read(&page).unwrap();
            let hard = tempfile::tempdir().unwrap();
            std::fs::create_dir(hard.path().join("deep")).unwrap();
            std::fs::hard_link(&page, hard.path().join("deep").join("2.svg")).unwrap();
            for to in [hard.path(), &hard.path().join("deep").join("2.svg")] {
                let to = to.display().to_string();
                let e = command_runs(&mut a, &["drawing", to.as_str()]).unwrap_err();
                assert!(e.contains("links to a file in it"), "{to}: {e}");
            }
            assert_eq!(std::fs::read(&page).unwrap(), before);
            // Named bare, from the folder it is in.
            let args: Vec<String> = ["drawing", "deep"].map(String::from).to_vec();
            assert!(writes_while_held(&args, &dir, &held, hard.path()).is_some());
            // A copy of the page is another file.
            let copied = tempfile::tempdir().unwrap();
            std::fs::copy(&page, copied.path().join("2.svg")).unwrap();
            let args = vec!["drawing".to_owned(), copied.path().display().to_string()];
            assert!(writes_while_held(&args, &dir, &held, out.path()).is_none());
        }
        #[cfg(unix)]
        {
            let page = dir.join("pages").join("p0002.page.json");
            let before = std::fs::read(&page).unwrap();
            let linked = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(&page, linked.path().join("2.svg")).unwrap();
            let to = linked.path().display().to_string();
            let e = command_runs(&mut a, &["drawing", to.as_str()]).unwrap_err();
            assert!(e.contains("links to a file in it"), "{to}: {e}");
            assert_eq!(std::fs::read(&page).unwrap(), before);
            // A folder that cannot be listed could hide such a link.
            let shut = tempfile::tempdir().unwrap();
            std::fs::set_permissions(shut.path(), std::os::unix::fs::PermissionsExt::from_mode(0o300)).unwrap();
            let args = vec!["drawing".to_owned(), shut.path().display().to_string()];
            let listed = std::fs::read_dir(shut.path()).is_ok();
            let refused = writes_while_held(&args, &dir, &held, out.path());
            std::fs::set_permissions(shut.path(), std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
            // Run as root the folder can still be listed, and then it is fine.
            if !listed {
                let e = refused.unwrap();
                assert!(e.contains("could not be looked through"), "{e}");
            }
            // A folder of its own is fine.
            let args = vec!["drawing".to_owned(), out.path().display().to_string()];
            assert!(writes_while_held(&args, &dir, &held, out.path()).is_none());
        }
        let page = dir.join("pages").join("p0002.page.json").display().to_string();
        for args in [
            vec!["vcs".to_owned(), "log".into(), "--svg".into(), page.clone()],
            vec!["vcs".to_owned(), "log".into(), format!("--svg={page}")],
            vec!["pdf".to_owned(), dir.join("out.pdf").display().to_string()],
        ] {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let e = command_runs(&mut a, &args).unwrap_err();
            assert!(e.contains("project folder") && e.contains("Ola"), "{args:?}: {e}");
        }
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
    fn a_background_tool_started_while_a_page_is_held_keeps_the_undo_history() {
        let (_tmp, mut a) = app(vec![]);
        call(&mut a, "add_page", json!({})).unwrap();
        call(&mut a, "add_page", json!({})).unwrap();
        call(&mut a, "undo", json!({})).unwrap();
        let open = a.open.as_mut().unwrap();
        open.held_by_others.insert("p0002".into(), "Ola".into());
        let (mark, can_undo, can_redo) = (open.undo_mark(), open.can_undo(), open.can_redo());
        let v = json!({"action": "search", "query": "relay"});
        let started = wc_agent::Tools::start(&mut AppTools(&mut a), "todo", &v);
        assert!(matches!(started, Started::Running(_)));
        let open = a.open.as_ref().unwrap();
        assert_eq!(
            (open.undo_mark(), open.can_undo(), open.can_redo()),
            (mark, can_undo, can_redo)
        );
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

    #[test]
    fn a_refused_cover_page_opens_no_form_and_keeps_the_panel_shown() {
        let (_tmp, mut a) = app(vec![]);
        let open = a.open.as_mut().unwrap();
        let panel = wc_core::panel::Panel::with_rails("panel1", "Panel 1", 600.0, 800.0);
        open.project.panels.push(panel);
        for p in &mut open.project.pages {
            p.cover = false;
        }
        let first = &mut open.project.pages[0];
        (first.title, first.symbols, first.wires, first.graphics) = ("Cover".into(), vec![], vec![], vec![]);
        let id = first.id.clone();
        open.held_by_others.insert(id, "Ola".into());
        let project = open.project.clone();
        a.ed.panel = Some(0);
        let r = call(&mut a, "run_action", json!({"action": Action::CoverPage.name()}));
        assert!(r.as_ref().is_err_and(|e| e.contains("Ola")), "{r:?}");
        assert!(a.dialog.is_none(), "no cover form for a refused change");
        assert_eq!(a.ed.panel, Some(0), "the panel stays shown");
        assert_eq!(a.open.as_ref().unwrap().project, project);
        // An open template form stays open, not replaced by the cover form.
        call(&mut a, "run_action", json!({"action": Action::PageTemplate.name()})).unwrap();
        let r = call(&mut a, "run_action", json!({"action": Action::CoverPage.name()}));
        assert!(r.as_ref().is_err_and(|e| e.contains("Ola")), "{r:?}");
        assert!(
            matches!(a.dialog, Some(Dialog::Template { .. })),
            "the template form stays"
        );
        assert_eq!(a.open.as_ref().unwrap().project, project);
    }

    #[test]
    fn a_form_opened_while_a_page_is_held_cancels_only_its_own_step() {
        let (_tmp, mut a) = app(vec![]);
        call(&mut a, "add_page", json!({})).unwrap();
        a.open
            .as_mut()
            .unwrap()
            .held_by_others
            .insert("p0002".into(), "Ola".into());
        call(&mut a, "run_action", json!({"action": Action::PageTemplate.name()})).unwrap();
        let mark = a.open.as_ref().unwrap().undo_mark();
        assert!(
            matches!(a.dialog, Some(Dialog::Template { since, .. }) if since == mark),
            "the form's undo mark is the window's"
        );
    }

    #[test]
    fn a_toolbar_edit_on_a_held_page_is_an_error_for_the_assistant() {
        let (_tmp, mut a) = app(vec![]);
        call(
            &mut a,
            "add_graphic",
            json!({"page": "p0002", "shape": {"kind": "text", "at": {"x": 30, "y": 30}, "text": "Hello"}}),
        )
        .unwrap();
        call(&mut a, "select_items", json!({"page": "p0002", "ids": ["g0"]})).unwrap();
        a.ed.held_by.insert("p0002".into(), "Ola".into());
        a.open
            .as_mut()
            .unwrap()
            .held_by_others
            .insert("p0002".into(), "Ola".into());
        let r = call(&mut a, "run_action", json!({"action": Action::Delete.name()}));
        assert!(r.as_ref().is_err_and(|e| e.contains("Ola has this page open")), "{r:?}");
        assert_eq!(page2(&a).graphics.len(), 1, "nothing deleted");
    }

    #[test]
    fn a_drawing_hidden_or_locked_in_the_layer_list_is_not_deleted_before_the_canvas_redraws() {
        use crate::layers::Layer;
        let (_tmp, mut a) = app(vec![]);
        call(
            &mut a,
            "add_graphic",
            json!({"page": "p0002", "shape": {"kind": "text", "at": {"x": 30, "y": 30}, "text": "Hello"}}),
        )
        .unwrap();
        for hide in [true, false] {
            call(&mut a, "select_items", json!({"page": "p0002", "ids": ["g0"]})).unwrap();
            if hide {
                a.prefs.layers.set_shown(Layer::Drawings, false);
            } else {
                a.prefs.layers.set_locked(Layer::Drawings, true);
            }
            // The layer list changed; Delete runs before the canvas is drawn again.
            a.apply_layers();
            assert!(a.ed.selection.is_empty());
            a.run(Action::Delete);
            assert_eq!(page2(&a).graphics.len(), 1, "the drawing stays");
            a.prefs.layers = Default::default();
            a.apply_layers();
        }
        // Shown and unlocked, the same Delete removes it.
        call(&mut a, "select_items", json!({"page": "p0002", "ids": ["g0"]})).unwrap();
        a.run(Action::Delete);
        assert!(page2(&a).graphics.is_empty());
    }

    #[test]
    fn the_text_and_comment_dialogs_add_nothing_to_a_layer_hidden_while_they_were_open() {
        use crate::layers::Layer;
        let (_tmp, mut a) = app(vec![]);
        a.ed.page = 1;
        let at = wc_core::geom::Point::new(40.0, 40.0);
        for hide in [true, false] {
            for l in [Layer::Drawings, Layer::Comments] {
                if hide {
                    a.prefs.layers.set_shown(l, false);
                } else {
                    a.prefs.layers.set_locked(l, true);
                }
            }
            a.add_text(at, "Note");
            assert!(page2(&a).graphics.is_empty());
            assert!(a.ed.message.contains("Drawings and notes"), "{}", a.ed.message);
            a.add_comment_at(at, "Check");
            assert!(a.open.as_ref().unwrap().project.comments.is_empty());
            assert!(a.ed.message.contains("Review comments"), "{}", a.ed.message);
            a.prefs.layers = Default::default();
        }
        // Shown and unlocked, both are added.
        a.add_text(at, "Note");
        assert_eq!(page2(&a).graphics.len(), 1);
        a.add_comment_at(at, "Check");
        assert_eq!(a.open.as_ref().unwrap().project.comments.len(), 1);
    }

    #[test]
    fn the_assistant_cannot_change_a_page_someone_else_holds_but_can_read_and_comment_there() {
        let (_tmp, mut a) = app(vec![]);
        call(
            &mut a,
            "add_graphic",
            json!({"page": "p0002", "shape": {"kind": "text", "at": {"x": 30, "y": 30}, "text": "Hello"}}),
        )
        .unwrap();
        a.open
            .as_mut()
            .unwrap()
            .held_by_others
            .insert("p0002".into(), "Ola".into());
        // A refused change keeps what can be redone and the page shown.
        call(&mut a, "add_page", json!({})).unwrap();
        call(&mut a, "undo", json!({})).unwrap();
        assert!(a.open.as_ref().unwrap().can_redo());
        let shown = a.ed.page;
        let before = page2(&a).clone();
        let r = call(
            &mut a,
            "add_graphic",
            json!({"page": "p0002", "shape": {"kind": "text", "at": {"x": 1, "y": 1}, "text": "x"}}),
        );
        assert!(r.is_err(), "{r:?}");
        assert!(a.open.as_ref().unwrap().can_redo(), "redo survives a refusal");
        assert_eq!(a.ed.page, shown);
        let line = json!({"kind": "line", "a": {"x": 0, "y": 0}, "b": {"x": 5, "y": 0}});
        let edits = [
            (
                "place_symbol",
                json!({"page": "p0002", "symbol": "wc.coil", "x": 50, "y": 50}),
            ),
            (
                "add_wire",
                json!({"page": "p0002", "points": [{"x": 0, "y": 0}, {"x": 9, "y": 0}]}),
            ),
            ("update_symbol", json!({"page": "p0002", "id": "s0", "tag": "-K9"})),
            ("update_wire", json!({"page": "p0002", "id": "w0", "label": "1"})),
            ("add_graphic", json!({"page": "p0002", "shape": line})),
            ("update_graphic", json!({"page": "p0002", "id": "g0", "shape": line})),
            ("delete_items", json!({"page": "p0002", "ids": ["g0"]})),
            ("move_items", json!({"page": "p0002", "ids": ["g0"], "dx": 5, "dy": 5})),
            ("rotate_items", json!({"page": "p0002", "ids": ["g0"]})),
            ("mirror_items", json!({"page": "p0002", "ids": ["g0"]})),
            ("delete_page", json!({"page": "p0002"})),
            ("edit_page_json", json!({"page": "p0002", "json": "{}"})),
            ("macros", json!({"action": "insert", "page": "p0002", "name": "any"})),
        ];
        for (tool, input) in edits {
            let r = call(&mut a, tool, input);
            assert!(
                r.as_ref().is_err_and(|e| e.contains("Ola has this page open")),
                "{tool}: {r:?}"
            );
            assert_eq!(page2(&a), &before, "{tool} changed the held page");
        }

        // A change that reaches the held page from elsewhere is refused as a whole: moving
        // the cover flag off it, or renaming a device drawn on it and on a free page.
        for p in &mut a.open.as_mut().unwrap().project.pages {
            p.cover = p.id == "p0002";
        }
        let project = a.open.as_ref().unwrap().project.clone();
        let r = call(&mut a, "update_page", json!({"page": "1", "cover": true}));
        assert!(r.as_ref().is_err_and(|e| e.contains("Ola has open")), "{r:?}");
        assert_eq!(a.open.as_ref().unwrap().project, project, "nothing of it stays");
        let tag = wc_core::DeviceTag::parse("-K1").unwrap();
        let held = std::mem::take(&mut a.open.as_mut().unwrap().held_by_others);
        for page in ["1", "2"] {
            call(
                &mut a,
                "place_symbol",
                json!({"symbol": "wc.coil", "x": 60, "y": 120, "page": page, "tag": "-K1"}),
            )
            .unwrap();
        }
        a.open.as_mut().unwrap().held_by_others = held;
        let (before, project) = (page2(&a).clone(), a.open.as_ref().unwrap().project.clone());
        let r = call(&mut a, "set_device", json!({"tag": "-K1", "rename_to": "-K2"}));
        assert!(r.as_ref().is_err_and(|e| e.contains("Ola has open")), "{r:?}");
        assert_eq!(page2(&a), &before);
        assert_eq!(a.open.as_ref().unwrap().project, project, "the free page keeps -K1 too");
        assert!(a.open.as_ref().unwrap().project.tag_index().contains_key(&tag));
        // The live path (start) refuses the same way.
        let started = wc_agent::Tools::start(
            &mut AppTools(&mut a),
            "set_device",
            &json!({"tag": "-K1", "rename_to": "-K2"}),
        );
        assert!(
            matches!(&started, Started::Done(Err(e)) if e.contains("Ola has open")),
            "set_device runs at once and is refused"
        );
        assert_eq!(a.open.as_ref().unwrap().project, project);

        // Reading, selecting and commenting still work there.
        assert!(
            call(&mut a, "get_page", json!({"page": "p0002"}))
                .unwrap()
                .contains("Hello")
        );
        call(&mut a, "select_items", json!({"page": "p0002", "ids": ["g0"]})).unwrap();
        call(
            &mut a,
            "comments",
            json!({"action": "add", "page": "p0002", "text": "Check this", "x": 10, "y": 10}),
        )
        .unwrap();
        assert_eq!(a.open.as_ref().unwrap().project.comments.len(), 1);

        // Once Ola leaves, the same tools work again.
        a.open.as_mut().unwrap().held_by_others.clear();
        call(&mut a, "delete_items", json!({"page": "p0002", "ids": ["g0"]})).unwrap();
        assert!(page2(&a).graphics.is_empty());

        a.open
            .as_mut()
            .unwrap()
            .held_by_others
            .insert("p0002".into(), "Ola".into());
        // Opening another project (with the same page ids) is not taken for a change to this
        // one: the new project stays as it was opened.
        let other = _tmp.path().join("other");
        let r = call(
            &mut a,
            "open_project",
            json!({"path": other.to_string_lossy(), "create": true, "name": "Other"}),
        );
        assert!(r.is_ok(), "{r:?}");
        let open = a.open.as_ref().unwrap();
        assert_eq!(
            (open.dir.as_path(), open.project.meta.name.as_str()),
            (other.as_path(), "Other")
        );
        assert_eq!(
            open.project,
            wc_core::store::load(&other).unwrap(),
            "nothing of the old project in it"
        );
    }

    #[test]
    fn a_new_project_can_start_from_a_base_project() {
        let (tmp, mut a) = app(vec![]);
        let base = a.open.as_ref().unwrap().dir.clone();
        let mut p = wc_core::store::load(&base).unwrap();
        p.meta.title_block.insert("customer".into(), "ACME".into());
        wc_core::store::save(&base, &p).unwrap();
        let new = tmp.path().join("from-base");
        let r = call(
            &mut a,
            "open_project",
            json!({"path": new.to_string_lossy(), "create": true, "name": "From base", "base": base.to_string_lossy()}),
        );
        assert!(r.is_ok(), "{r:?}");
        let open = a.open.as_ref().unwrap();
        assert_eq!(open.dir, new);
        assert_eq!(open.project.meta.name, "From base");
        assert_eq!(open.project.meta.title_block["customer"], "ACME");
        assert_eq!(open.project.pages.len(), p.pages.len());
        assert_eq!(
            open.project.meta.libraries, p.meta.libraries,
            "the base project's library sources, no starter library added"
        );
    }
}
