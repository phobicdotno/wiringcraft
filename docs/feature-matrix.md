# Feature matrix: EPLAN Electric P8, SolidWorks Electrical and WiringCraft

Research for issue #55 (November 2026). It lists the functions the two established electrical CAD
products offer, grouped by area, and says for each one where WiringCraft stands. Like
[known-gaps.md](known-gaps.md) it is a snapshot; the todo list on GitHub is the live list.

## How to read it

- **EPLAN** is EPLAN Electric P8 with its usual add-ons (Pro Panel, Fluid, Harness proD, Data
  Portal) where a row says so. **SWE** is SOLIDWORKS Electrical Schematic and Electrical 3D.
  Yes means the product has the function; *add-on* means it needs a separate module.
- **WiringCraft** says what the code on `main` does today: yes, partly (with what is missing) or
  no.
- **Triage:**
  - **done**: WiringCraft has it.
  - **todo (#n)**: wanted, and the issue tracks it.
  - **open PR (#n)**: a pull request is under way.
  - **later**: wanted, but after the basics; it gets an issue when it comes up for work.
  - **not planned**: with the reason.
- Descriptions are written from the products' public documentation and feature pages in our
  own words. Nothing is copied from them, and no symbol, icon or template is taken over.

Sources (public documentation, per area below):

- EPLAN online help: <https://www.eplan.help/> and the product pages under
  <https://www.eplan.com/>.
- SOLIDWORKS Electrical help (the "SOLIDWORKS Electrical" section): <https://help.solidworks.com/>
  and the product pages under <https://www.solidworks.com/>.

Documents that cover every area:

- EPLAN Electric P8 performance description (the vendor's full function list, PDF):
  <https://www.eplan.com/content/dam/eplan/corporate/performance-descriptions/2024/en/performance-description-eplan-electric-p8.pdf>
- EPLAN Information Portal (online help): <https://www.eplan.help/en-us/Infoportal/Content/htm/portal_home.htm>
- SOLIDWORKS Electrical help, table of contents: <https://help.solidworks.com/2022/English/swelec/Accueil.htm>
- SOLIDWORKS Electrical tutorials (one per area: projects, schematics, cables, terminals, PLC,
  reports, 3D): <https://help.solidworks.com/2024/English/swelec/c_swelec_c-ov.htm>

Pages for single areas, where the help has one:

- 7. Terminals (SWE): terminal strip editor
  <https://help.solidworks.com/2022/English/swelec/EwUiModelessDialogTerminalStripEditor.htm>,
  terminal strip drawings <https://help.solidworks.com/2023/English/swelec/c_swelec_DrawTerminalStrip-sb.htm>
- 8. Cables (SWE): <https://help.solidworks.com/2024/English/swelec/c_swelec_cables.htm>
- 9. PLC (EPLAN): bus data exchange tech tip
  <https://www.eplan.help/cs-cz/Infoportal/Content/misc/Techtipps/SPS/TechTipp-SPS-Busdatenaustausch.pdf>

For the other areas, the chapter to read in each help is:

| Area | EPLAN help chapter | SOLIDWORKS Electrical help chapter |
|---|---|---|
| 1. Project and pages | Projects; Pages; Structure identifiers | Projects; Books and drawings; Locations |
| 2. Editing | Graphical editing; Macros; Find | Schematic editing; Macros; Find and replace |
| 3. Symbols and library | Symbols and symbol libraries | Symbol manager |
| 4. Devices and parts | Devices; Parts management | Components; Manufacturer parts catalog |
| 5. Wires and potentials | Connections; Potentials; Numbering | Wires and equipotentials; Wire numbering |
| 6. Cross-references | Cross-references; Contact images | Cross references |
| 7. Terminals | Terminal strips | Terminal strips |
| 8. Cables | Cables | Cables |
| 9. PLC | PLC and bus engineering | PLC manager |
| 10. Reports | Reports; Forms | Report manager |
| 11. Panel layout and 3D | EPLAN Pro Panel | Electrical 3D; 2D cabinet layout |
| 12. Checks | Check runs; Message management | Design rules check |
| 13. Data exchange | Import and export; Data Portal | Import and export; Content portal |
| 14. Revisions and collaboration | Revision management; Multi-user | Revisions; Collaborative work |
| 15. Automation | Scripts and API | API |
| 16. Other disciplines | EPLAN Fluid; EPLAN Preplanning; EPLAN Harness proD | Electrical 3D (harness routing) |

## 1. Project and pages

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Project with numbered schematic pages, page navigator | yes | yes | yes | done |
| Page properties: number, title, size | yes | yes | yes (A4 to A0, landscape or portrait, or a custom size in mm; #261) | done |
| Page types (schematic, single-line overview, graphic, report, panel layout) | yes | yes | partly: schematic, overview flag, cover page, graphic pages (`page graphic`, #1845), panel layouts, report pages in the PDF, after the pages or in a page's place among them (`page report`, #1897) | done for the basics; more types later |
| Structure identifiers per page (function `=` and location `+`) | yes | yes (book / file / location) | yes: IEC 81346 tags, page defaults | done |
| Locations list with descriptions | yes | yes | yes: project properties, `wiringcraft location`, Locations report, SolidWorks and AutomationML import | done |
| Frame with column and row references (e.g. 3.C) | yes | yes | yes | done (#54) |
| Title block shared by all pages | yes | yes | yes | done |
| Cover page and page template editing | yes | yes | yes: cover page form, Page template form (columns and title-block boxes, kept in `project.toml`); `wiringcraft page cover` and `wiringcraft template` on the command line | done (#53, #119) |
| Project templates and base projects | yes | yes (project templates) | yes: `wiringcraft new <dir> --template <folder or git URL>` and "Start from" in New project; `{name}` and `{date}` filled in | done (#270) |
| Project properties (customer, dates, revision ...) | yes | yes | yes | done |
| Project archive (pack and unpack) | yes | yes (.tewzip) | yes: import of archives, projects are plain folders | done |
| Multi-language project texts and translation dictionary | yes | yes | yes: `translations.toml` dictionary, `wiringcraft pdf --lang nb` (or `nb,en` for two; `wiringcraft drawing --lang` too, for SVG and DXF), `wiringcraft translations --lang nb` lists missing texts; in the app, File, Translations edits the texts with a column per language (missing ones marked) and the PDF export asks for the language; Undo and Redo take back and bring back translation edits made in the app or by its assistant (#589; not in a shared session, whose history does not hold the file), and refuse the step when another program changed the file since, so that change is not written over (#657) | done: CLI (#276), app (#451) |

## 2. Editing on the sheet

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Place, move, rotate, mirror symbols | yes | yes | yes | done |
| Select, box select, copy, cut, paste, duplicate (Ctrl+D), delete, undo/redo | yes | yes | yes; `wiringcraft delete <page> <ids>` deletes symbols and wires (and the wires on a deleted symbol) from the command line | done (#845) |
| Grid, snap, zoom, pan, fit page | yes | yes | yes | done |
| Free graphics: lines, rectangles, texts | yes | yes | yes | done |
| Circles, arcs and polylines in the editor | yes | yes | yes: drawn by hand (Graphics toolbar), picked, moved, deleted and exported like other drawings | done |
| Dimensions in the editor | yes | yes | yes: kept from imports (drawn by their block), and drawn by hand with the Dimension tool (Graphics toolbar: two points, then the dimension line); picked, moved, deleted, saved and exported to PDF, SVG and DXF like other drawings | done (#317) |
| Find and replace texts, tags and wire numbers across the project | yes | yes | yes: Edit → Find (Ctrl+F), `wiringcraft find` | done (#109) |
| Macros / window macros (save and insert groups) | yes | yes | yes | done |
| Open, Save as and export with the system file dialogs | yes | yes | yes: Open (Ctrl+O) and Save as (Ctrl+Shift+S, a copy in a new folder, history included) use the system pickers, and every export from the menus asks where to save, starting in `export/` (or the nearest folder of it that exists). The assistant's export tool writes to `export/` without asking, when no file dialog can open an export goes there too and says so, and a dialog that closes at once without an answer writes nothing (Open and Save as then offer to type the path) | done (#338) |
| Macro placeholders and value sets (one macro, several variants) | yes | partly | partly: `{{name}}` placeholders in a macro's device descriptions, parts, accessories and properties, wire cross-section, colour and type, and drawn texts, filled from a named value set chosen at insert (Insert > Macro, the assistant, `wiringcraft macro place --set`); a project macro's placeholders and value sets are edited in the app (Insert > Macro, Placeholders…), a renamed placeholder renamed in its texts | done (#272, #407) |
| Auto-connect pins that line up | yes | yes | yes | done (#51) |
| Scanned drawing as underlay to trace over | partly | partly | yes | done |
| Layers / show-hide groups | yes | yes (DWG layers) | yes: PDF, DXF and SVG layers, editor layers (View → Layers) | done: show, hide and lock on the canvas (also by the assistant), kept per user |

## 3. Symbols and library

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| IEC symbol library | yes | yes | yes: own starter library | done |
| Symbol editor (draw graphics, place and name pins) | yes | yes | yes: draw (lines, rectangles, circles, arcs, polylines that can be closed and filled, text), move (drag or arrow keys) and change shapes (drag a line end, rectangle corner, circle edge or arc end to reshape it), place pins, save to a library folder | done (#105, #159) |
| Import symbols from DXF / SVG | DXF/DWG | DWG | yes (SVG limits: see known-gaps) | done |
| Import symbols from KiCad libraries | no | no | yes: every unit of a multi-unit symbol becomes a symbol of its own, also from a `.kicad_sym` inside a zip; old KiCad 5 `.lib` libraries too (#1839); limits: see known-gaps | done (#96) |
| Import QElectroTech elements | no | no | yes: `.elmt` elements become symbols (`wiringcraft import symbol-qet`, Library → Import symbols); limits: see known-gaps | done (#91) |
| Library versioned with lockfile and per-item update | no (master data sync) | no (shared database) | yes | done |
| Library from git or folders | no | no | yes | done |

## 4. Devices, parts and properties

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Device tags with function, location and product aspects | yes | yes | yes | done |
| Automatic next free tag on placement | yes | yes | yes | done |
| Device properties and description | yes | yes | yes | done |
| Part (article) database with manufacturer, order number, data | yes | yes (catalog) | yes | done |
| Several parts on one device (main part plus accessories) | yes | yes | yes: a main part plus accessories (with a quantity) per device, edited in the properties panel, counted in the parts list and the contact check | done (#106) |
| Black box / device box for devices without a symbol | yes | yes | yes: imported, drawn with connection points from the command line (`wiringcraft box`) and with the Black box tool in the app (drag the rectangle, click its edges for connection points, then tag it); a placed box's width, height and connection points (add, rename, move, remove) are changed in the properties panel as one undo step, with a warning naming wires on points that are gone or moved; with the select tool a selected box also shows a sizing handle at each corner: dragging one resizes the box on release so the opposite corner stays put, moving the box when a near corner is dragged (one undo step, points kept where they were on the page where they can, else on their edges); the assistant's `black_box` tool draws, resizes and edits boxes the same way, and `wiringcraft box-edit` resizes one and replaces or renames its points from the command line | done (#266, #371, #489, #524, #529, #542) |
| Part data from the maker's data portal | add-on (Data Portal) | yes (content portal) | EDZ part data, and macros as symbols with their connection points (multi-line, variant A; or a symbol by `--symbol-map`), and BMEcat | done (#144, #311) |
| Manufacturer symbol and macro sources | yes | yes | listed in docs | done (#46) |

## 5. Wires, potentials and connections

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Draw wires pin to pin with bends | yes (auto-connect) | yes | yes | done |
| Route style (square or sloped) | yes | partly | yes | done (#52) |
| Potentials (named signals across pages) | yes | yes (equipotentials) | yes | done |
| Interruption points between pages | yes | yes (origin/destination arrows) | yes | done |
| Wire numbering by rule | yes | yes | yes | done |
| Wire properties: cross-section, colour, wire type | yes | yes (wire styles) | yes: wire properties, wire list columns, from EPLAN and SolidWorks imports | done (#107) |
| Net list and connection list | yes | yes | yes | done |
| Shorts and open pins check | yes | yes | yes | done |

## 6. Cross-references and contacts

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Coil to contact cross-references with page/column | yes | yes | yes, clickable | done |
| Contact image under the coil (list of contacts and pins) | yes | yes | yes (texts with pins and locations) | done |
| Contact overflow check (more contacts drawn than the part has) | yes | yes | yes: parts list their contacts (`contacts` in the part file), `check` warns | done |

## 7. Terminals

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Terminals and terminal strips | yes | yes | yes | done |
| Terminal plan report | yes | yes | yes | done |
| Terminal strip editor (reorder, add spares) | yes | yes | yes: rail order and spare terminals in the project file, levels and jumpers edited there; app window (drag or arrows, tick terminals to jumper them), `strip` CLI commands; terminal plan and WAGO configurator follow it | done (#263) |
| Jumpers (bridges) between terminals | yes | yes | yes: project file, one net, terminal plan column, CLI and app; drawn as a bar on the page in the editor and in the PDF, SVG and DXF exports (#1213); jumpers from the SolidWorks Electrical terminal strip editor are imported (#247) | done (#108) |
| Multi-level terminals | yes | yes | yes: `Level` device property, terminal plan column, one WAGO marker per block | done (#124) |
| Terminal and wire marking export (label printers) | yes | yes | yes: WAGO smartScript CSV with lines, engraving text, language and one-strip export ([docs](wago-marking.md)) | done (#57, #129) |
| SolidWorks terminal strips on import | - | - | yes: terminal strips with their terminals, the from/to terminals of each wire, and the jumpers of the terminal strip editor | done (#70, #247) |

## 8. Cables

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Cables as devices with cores | yes | yes | yes | done |
| Cable list report | yes | yes | yes | done |
| Cable types from the catalog | yes | yes | yes: cable types as parts with their cores, from SolidWorks archives (`import swe-library`) | done (#62) |
| Cable routing through ducts and length calculation | add-on (Pro Panel) | yes (3D) | no | later (with the SolidCraft export, phase 5) |

## 9. PLC

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| PLC I/O list report | yes | yes | yes | done |
| PLC addresses from a CSV list | yes | yes | yes | done |
| PLC variable list export (structured text) | yes | partly | yes | done |
| PLC card overview pages and rack configuration | yes | yes | partly: `[[rack]]` tables with their `[[rack.card]]`s in `project.toml` (slot, tag, type, channels, start address); the `plc-cards` report lists every card channel by channel with the device, connection and page from the PLC I/O list, as a table in the PDF (`pdf --report plc-cards`) and the app; see `wc_core::plc_rack`. The app edits racks and cards in File > PLC racks (#436), showing the report's rack warnings while typing. The command line lists and edits them too (`wiringcraft rack list`, `rack add`/`set`/`remove`, `rack card add`/`set`/`remove`), with the form's range checks, printing the rack warnings after a change (#592). The PDF draws a sheet per card: the card as a box with a terminal per channel, its address and the device and page of each point (#437) | done (#275, #436, #437, #592) |
| Fieldbus (PROFINET, EtherCAT) topology | yes | partly | yes: `[[network]]` tables in `project.toml` (devices, addresses, station names, cabled ports), Networks report (`--report networks`) with checks, and a drawn topology page per network after the list in the PDF (boxes per device linking to its page, ports and cable tags on each link, rings drawn once, cables from different devices each on a track of their own so they do not look joined, cables fanning out of one device sharing one run; tags, port names and cables never cut: smaller type, then a numbered key with a legend under the drawing; [docs](networks.md)) | done: network list (#278), topology pages (#453), separate tracks (#486), whole names (#493) |
| Exchange with PLC engineering tools (AutomationML) | yes | partly | yes: export (`wiringcraft aml`) and import (`import aml`; in the app the second File → AutomationML entry, among the imports below the exports, or AutomationML in the ribbon's Import tab) of devices and connections | done |

## 10. Reports

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Parts list / bill of materials | yes | yes | yes | done |
| Device, wire, net, terminal, cable, PLC lists | yes | yes | yes | done |
| Table of contents | yes | yes | yes, linked in the PDF | done |
| Report pages in the drawing set | yes | yes | yes | done |
| Report templates the user designs | yes | yes | yes: `[[report]]` layouts in `project.toml` pick, order, rename and sort the columns of any report, in the PDF, CSV, Markdown and Excel output; `group = "<column>"` prints a heading per group in the PDF (one row per record in the CSV), and `header = [...]` prints title-block values above the table; edited in the app (Reports, Layout…; #480), by the assistant (#545) and with `wiringcraft report-layout show/set/reset` (#604) | done: columns and sort (#273), grouping and page header fields (#421), an unknown group column or header key warned about (#481), editor in the app (#480) |
| Reports as CSV / Markdown / Excel | yes (Excel) | yes (Excel) | yes: `report --format xlsx`, Reports → Save as Excel | done (#264) |

## 11. Panel layout and 3D

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| 2D mounting plate with rails and ducts | add-on (Pro Panel) | yes | yes: `panel rail <panel> --at x,y --length L` and `panel duct <panel> --at x,y --length L [--width W] [--vertical]` add one, `--remove N` takes one off, numbered as `panel show` lists them (#817); in the app's panel view a rail or duct is dragged to move it, kept on the plate, one undo step per drag; a picked rail's right end or a duct's bottom-right corner is dragged to change its length or size, snapped, kept on the plate, one undo step per drag, and a rail is never shortened past the devices on it (#813, #983); a panel layout's name and plate width and height are changed in its properties in the app, refusing a size that would leave rails, ducts or devices sticking out (#894, #1196) | done |
| Place devices onto rails from the schematic | add-on | yes | yes: `panel arrange` (Page menu → Arrange devices in the app), or one device at a time with Place in the panel layout's "Not on the plate yet" list in the app (first DIN rail with room, else a free spot; #928) or with `panel device <panel> <tag> --at x,y [--rotate 90]` (or `--remove`); `panel show` lists each device with its place, size and turn (#507); `panel edit <panel> [--name ...] [--width W] [--height H]` renames a layout or changes its plate size, keeping rails, ducts and devices where they are and refusing a size that would leave any of them sticking out (#734) | done |
| Maker outline drawings as device footprints | yes (2D macros of the part) | yes (2D footprint) | yes: `import footprint-dxf`, or Attach footprint drawing in the app's panel layout properties, stores a maker DXF with the part; layout pages in the app and the PDF draw it at real size (shrunk to fit a smaller device box), and a part without a `Width` or `Height` takes it from the drawing. A device can be turned in quarter steps (Rotate 90° in its properties, or R on the layout); its box, its drawing and its STEP block turn with it (#485) | done (#312, #485) |
| 3D model export (STEP) | add-on | yes | yes | done |
| Maker 3D models (STEP) give a part's size | yes (Pro Panel) | yes | partly: `import model-step` reads a maker's STEP model (mm, m or inch) and stores its bounding box with the part, as does Attach 3D model in the app's panel layout properties (#501); a part without `Width`, `Height` or `Depth` takes them from it for layouts and the STEP export. An assembly model is measured with its parts where they are placed, nested assemblies included (#500), each part in its own length unit (#510). Not yet: the model's shape in 3D | size done (#313, #500, #501, #510); the shape in 3D later, with the SolidCraft export (phase 5) |
| 3D cabinet design with wire routing | add-on (Pro Panel) | yes (Electrical 3D) | no | later: one-way export to SolidCraft (phase 5) |
| Drilling and NC data for plates | add-on | partly | no | not planned for now: belongs to the 3D side (SolidCraft) |
| SolidWorks cabinet layouts on import | - | - | yes: each 2D cabinet layout page becomes a panel with its plate, rails, ducts and device footprints | done (#60) |

## 12. Checks

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Project check with errors and warnings | yes | yes | yes, in the app and the CLI; it also warns about panel layout devices that overlap, stick out of the plate or are no longer in the project, and rails or ducts that stick out of the plate | done (#733, #1158) |
| Online check while drawing | yes | partly | yes: the check runs again shortly after each edit, off the window thread; problems are ringed on the canvas and counted in the status bar (File → Check while drawing turns it off) | done (#269) |
| User-defined check rules | yes | no | yes: `[[rule]]` tables in `project.toml` (a cable on matching devices, a tag pattern, at most N wires per pin), run with the built-in check in the app and `wiringcraft check`; added, changed, reordered and deleted in the Rules dialog next to the check list (one undo step); house rules in a library source's `rules.toml` run for every project using it, named by source, and a project turns one off with `rules_off = ["source:2"]` or by name; listed, replaced from a TOML or JSON file and turned off or on with `wiringcraft rules list/set/off/on`, with the dialog's checks; see `wc_core::check_rule` | done (#274, #429, #430, #587) |

## 13. Data exchange

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| PDF export with links and layers | yes | yes | yes | done |
| DXF / SVG export | yes (DXF/DWG) | yes (DWG/DXF) | yes: one file per schematic page and one per panel layout (`panel-<name>`: plate, rails, ducts, device outlines and tags as on its PDF sheet) | done (#927) |
| DXF import: ELLIPSE, SPLINE, HATCH | yes | yes | yes: sampled curves, solid hatches filled | done |
| DWG read and write | yes | yes | read: AutoCAD 2004 to 2018 drawings (AC1018 to AC1032; later releases still save in the 2018 format), for SolidWorks Electrical symbols and pages and for `import page-dwg` and File → DWG drawing; no write | read done (#161, #162, #163, #215, #292); write not planned, DXF covers it |
| Import EPLAN projects | - | yes (some) | yes | done |
| Import SolidWorks Electrical projects | no | - | yes | done; library parts, cable types and symbol blocks with `import swe-library` (#62, blocks drawn from their DWG files #59); cabinet layouts as panels (#60, footprints sized from the catalog part #149); symbol and page graphics from the DWG files (#163, stretched circles and arcs as ellipses #307); cover page and title block data, with the project's revision and checked by (#61, #170); terminal strips and wire ends (#70) and their jumpers (#108, #247) |
| PDF trace import (turn a PDF drawing into a page) | partly | no | yes | done |
| AutomationML export | yes | partly | yes | done |
| List exchange as CSV (import and export) | yes | yes | yes | done |
| Excel workbook (.xlsx) import and export | yes | yes | yes: parts and PLC address lists read from the first visible sheet with the CSV column names (`import parts-csv`, `import plc-csv`, `.xlsx` entries in archives); reports written one sheet per report, every cell as text; the old binary `.xls` is not read | done (#264) |
| WAGO and other maker configurator links | add-on | partly | yes for WAGO: terminal strips with rails, jumpers and accessories as a CSV for the WAGO configurator ([docs](wago-configurator.md)), and the chosen articles back onto the devices and jumpers (`import wago`, File → WAGO articles back); other makers' configurators not yet | done for WAGO (#122, #123); other makers later |

## 14. Revisions and collaboration

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Revision tracking and compare | yes | yes | yes: git history, subway map, change lists | done |
| Revision marks (clouds, revision table on pages) | yes | yes | yes: revisions listed in the project properties, revision clouds tagged with the latest revision and the revision table drawn on pages and in the PDF, made in the app (Revision cloud and Revision table tools) or with `wiringcraft revision` | done (#265, #377) |
| Review comments on pages | yes (comments in PDF) | partly | yes | done |
| Several people on one project at once | yes (multi-user) | yes (server database) | yes: share and join a session on the network in the app, page locks, one change per edit, undo per change ([known-gaps.md](known-gaps.md#shared-sessions)) | done (#43, with #74 to #78, #197, #198, #201, #286) |
| User rights per project | yes | yes | no | not planned: git hosting already controls who can change a project |

## 15. Automation and help

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Scripting API | yes (C# scripts, API) | yes (API) | yes: the `wiringcraft` command line | done |
| Generating schematics from tables or configurators | add-on (Cogineer) | no | partly: `wiringcraft macro generate` places a macro once per row of a CSV or Excel table, the cells filling its placeholders and value set, a `page` column titling the pages and a `macro` column or `--rule column=value:macro` rules on another column picking the macro per row, on new pages ([docs](cli/library-and-macros.md#pages-from-a-table)); in the app with Insert > Macro, From table…; the assistant draws from plain words; not yet: the rules in the app, or connections drawn between copies | partly done (#1837, #1884, #1893, #1900) |
| AI assistant that edits the open project | no | no | yes: its `get_state` also tells how the grid, auto-wiring, checking while drawing, theme and menu style are set, so it switches one knowingly | done |
| Log file for bug reports | yes | yes | yes | done (#48) |

## 16. Other disciplines

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Fluid power (pneumatics, hydraulics) | add-on (Fluid) | no | no | not planned for now: electrical first; a symbol set could add it later without core changes |
| Process and instrumentation (P&ID) | add-on (Preplanning) | no | no | not planned |
| Wire harness manufacturing drawings | add-on (Harness proD) | partly (3D routing) | no | not planned: needs 3D, belongs to SolidCraft |

## Todo issues filed with this matrix

Every "todo" row links an issue. These five were new with this document, and all five are done:

- #105 Symbol editor in the app.
- #106 Several parts on one device.
- #107 Wire cross-section, colour and type, shown in the wire list.
- #108 Jumpers between terminals, shown in the terminal plan.
- #109 Find and replace across the project.

The "later" rows get an issue when they come up for work, so the todo list keeps to work that
can start now.
