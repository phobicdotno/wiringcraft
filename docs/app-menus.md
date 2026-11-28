# Menus of the desktop app

This page lists every menu of the WiringCraft desktop app and each item in it, in the order the
app shows them. A line in a list marks a separator in the menu. For the keys of each command see
[app-shortcuts.md](app-shortcuts.md).

The menu bar is shown in both layouts, the classic toolbars and the ribbon. An item that cannot be
used right now is greyed out. A setting that is on (Grid, Auto-wire, Dark mode and the like) is
shown highlighted. Hover over an item to see a longer note on what it does.

## Menu bar

### File

- **New project**: create a new project folder, which is also a git repository.
- **Open**: open a project folder.
- **Save**: save all changed pages.
- **Save as**: save the project as a copy in a new folder and go on working in the copy.
- ---
- **Project properties**: edit the project's name, title-block fields, locations and revisions.
- **Translations**: translate the project's texts into other languages (`translations.toml`).
- ---
- **Check**: check the project for problems such as devices drawn twice, contacts without a coil, shorts and open pins.
- **Check while drawing**: check the project again shortly after each edit and mark problems on the canvas. Click again to turn it off.
- **Reports**: show the contents, parts, devices, wires, nets, terminals, cables, PLC I/O, PLC cards, locations and networks as tables.
- **PLC racks**: add and edit the PLC racks and the I/O cards in their slots, laid out on the PLC card overview.
- **PDF**: export all pages and the reports to PDF.
- **SVG/DXF**: export every page as an SVG and a DXF drawing.
- **AutomationML**: export devices, part data and connections as AutomationML.
- **WAGO marking**: export terminal, wire, device and cable markers for WAGO smartScript (CSV).
- **WAGO configurator**: export terminal strips with jumpers, accessories and rails for the WAGO configurator (CSV).
- **WAGO articles back**: read a WAGO configurator list with the chosen articles back onto the terminals and strips.
- ---
- **EPLAN project**: import an EPLAN Electric P8 project (`.elk` or a zip of it).
- **SolidWorks Electrical project**: import a SolidWorks Electrical project archive (`.tewzip`).
- **SolidWorks Electrical libraries**: import the parts, cable types and symbols of a SolidWorks Electrical archive into the project.
- **AutomationML**: import AutomationML devices and connections as overview pages.
- **DWG drawing**: import a DWG drawing onto the current page.
- **DXF drawing**: import a DXF drawing onto the current page; blocks named like library symbols become symbols.
- **PLC addresses**: import PLC addresses from a CSV or Excel list onto the project's devices.
- **PDF pages**: import PDF pages as trace pages.
- **Project archive**: import the PDF, DXF, AutomationML, CSV and Excel files of another tool's project archive.
- **Parts from EDZ**: import the parts of an EPLAN Data Exchange file (`.edz`) into a library folder.
- **Parts from BMEcat**: import the parts of a BMEcat catalogue (XML) into a library folder.
- **Parts from CSV**: import parts from a CSV or Excel list into a library folder.

### Edit

- **Undo**: undo the last change.
- **Redo**: redo the last change you undid (Ctrl+Shift+Z works too).
- ---
- **Cut**: cut the selection.
- **Copy**: copy the selection.
- **Paste**: paste at the pointer; devices get the next free tags.
- **Duplicate**: put a copy of the selection one grid step down and right, selected so it can be moved; devices get the next free tags.
- **Select all**: select everything on this page.
- **Find**: find and replace texts, device tags, wire numbers, potentials and descriptions on every page.
- ---
- **Rotate**: rotate the selected symbols 90°, the symbol being placed, or the device picked on a panel layout.
- **Mirror**: mirror the selected symbols left to right, or the symbol being placed.
- **Delete**: delete the selection.
- ---
- **Number wires**: give every connection without a number one, from its page and column (for example 3.4).
- **Terminal strips**: edit terminal strips: order the terminals on the rail, add spares, set levels; the terminal plan and WAGO export follow.

While the symbol editor is open, Undo, Redo and Delete work on the symbol being drawn, not on
the schematic behind it.

### Insert

- **Select**: the select and move tool.
- **Symbol**: place the symbol chosen in the library picker.
- **Wire**: draw a wire: click a pin or point, then the other end.
- **Auto-wire**: draw wires between pins that line up when a symbol is placed or moved (hold Alt to skip once). Click again to turn it off.
- ---
- **Macro**: insert a saved group of symbols and wires from the project or the library, or place one once per row of a table with **From table…** ([details](macros-and-templates.md#pages-from-a-table)).
- **Save as macro**: save the selected symbols, wires and drawings as a macro.

### Page

- **New page**: add a schematic page.
- **Cover page**: open the cover page and edit the project's name and fields; they show on the cover and every title block.
- **Page template**: edit the frame's reference columns and the title block's boxes, shared by every page.
- **Duplicate page**: copy this page; its devices get the next free tags.
- **Page properties**: edit this page's number, title, sheet size and its own title-block values.
- ---
- **Move page up**: swap this page's number with the one before.
- **Move page down**: swap this page's number with the one after.
- ---
- **Previous page**: open the page before this one.
- **Next page**: open the page after this one.
- ---
- **Delete page**: delete this page (asks first).
- ---
- **Panel layout**: add a cabinet panel layout: a 600 x 800 mm mounting plate with wire ducts and DIN rails.
- **Arrange devices**: put the devices that have a part and are not on the plate yet onto its DIN rails. Devices whose part is door- or front-mounted (push buttons, pilot lights) stay off the plate and are not listed as not fitting.
- **Delete panel layout**: delete the panel layout shown.

### View

- **Zoom in**: show the page larger.
- **Zoom out**: show the page smaller.
- **Fit page**: show the whole page.
- ---
- **Grid**: show or hide the grid.
- **Layers**: show, hide and lock drawings, wire numbers, cross-references, comments and scanned sheets.
- **Layout**: switch between the classic toolbars and the ribbon.
- **Dark mode**: switch between the light and dark interface.
- ---
- **Assistant**: show or hide the AI assistant: tell it what to draw or change and it works in the open project.

### Library

- **Library**: compare the project's library items with the library.
- **Update**: take the newer library versions of the project's symbols and parts, then show the library status.
- ---
- **New symbol**: draw a new symbol and save it into a library folder.
- **Edit symbol**: change a library symbol (the selected one, or pick one).
- **Import symbols**: import DXF, SVG, KiCad (`.kicad_sym`, and old `.lib` libraries) and QElectroTech (`.elmt`) files into a library folder.

### Versions

- **Commit**: record a version of the project.
- **Branch**: start a branch from this version.
- **History**: show the version history as a subway map.
- ---
- **Comment**: pin a review comment on the sheet: click where it goes.
- **Page history**: who changed this page, when, and what they changed.
- ---
- **Share session**: let colleagues on the network work in this project with you; you let each one in.
- **Join session**: work in a colleague's shared project: pick their session and ask to join.

### Help

- **Report a problem…**: something broke or gave a wrong result.
- **Request a function…**: ask for something WiringCraft should do.
- **Something is missing…**: a symbol, part, report or setting you expected.

Request a function and Something is missing open a GitHub issue form in the browser. Report a
problem first shows WiringCraft's recent log, which you can choose to include, then opens the
form. The form's fields are filled in through the link the browser opens, so with the log
included, the end of the log reaches GitHub when the form opens, before you press Submit.
Nothing is published until you press Submit, and what you send goes on the todo list.

## Right-click menus

### On the canvas

Right-click a wire, symbol, free text or drawing. It is selected (a selection it is already part
of is kept) and the menu shows what fits the selection. Nothing opens on a page someone else holds
in a shared session.

With wires selected:

- **Route** (or **Route of N wires**): a heading; the current style is marked.
- **Square corners**, **Sloped middle (45°)**, **Straight line**: set the route style of the selected wires.
- ---
- **Delete**: delete the selected wires. Only shown when nothing but wires is selected.

With symbols selected (after a separator when wires are selected too):

- **Rotate**, **Mirror**, **Copy**, **Cut**, **Duplicate**, **Delete**: as in the Edit menu.

With free text or drawings selected and no symbols (after a separator when wires are selected
too):

- **Copy**, **Cut**, **Duplicate**, **Delete**: as in the Edit menu.

When wires are selected together with symbols, texts or drawings, this Delete removes the whole
selection, wires included.

### On a panel layout

Right-click an item on the mounting plate:

- A device: **Rotate** (turn it 90°) and **Delete** (take it off the plate).
- A DIN rail or wire duct: **Delete**.

### In the pages list

Right-click a page in the left panel. The page is opened first.

- **Cover page**: only on the cover page.
- **Page properties**, **Duplicate page**, **Move page up**, **Move page down**, **Delete page**: as in the Page menu. Move up is greyed out on the first page, Move down on the last, and Delete when it is the only page.

Right-click a panel layout under **Panels**:

- **Arrange devices**, **Delete panel layout**: as in the Page menu, for that layout.

### In the devices list

Right-click a terminal under **Devices**:

- **Export markers of -X1...**: export the WAGO markers of that terminal's strip only (here `-X1`).

## Other menus

- **Insert…** in the Macro window: shown for a macro with variants instead of Insert. It asks which variant to insert and lists them.
- **From table…** in the Macro window: asks for a CSV or Excel table and places the macro once per row on new pages. For a macro with variants it is a menu that first asks which variant rows without a `set` column use. A `macro` column places another macro on its row.
