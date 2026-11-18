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
| Page types (schematic, single-line overview, graphic, report, panel layout) | yes | yes | partly: schematic, overview flag, panel layouts, report pages in the PDF | done for the basics; more types later |
| Structure identifiers per page (function `=` and location `+`) | yes | yes (book / file / location) | yes: IEC 81346 tags, page defaults | done |
| Locations list with descriptions | yes | yes | yes: project properties, `wiringcraft location`, Locations report, SolidWorks and AutomationML import | done |
| Frame with column and row references (e.g. 3.C) | yes | yes | yes | done (#54) |
| Title block shared by all pages | yes | yes | yes | done |
| Cover page and page template editing | yes | yes | yes: cover page form, Page template form (columns and title-block boxes, kept in `project.toml`) | done (#53, #119) |
| Project templates and base projects | yes | yes (project templates) | no | later |
| Project properties (customer, dates, revision ...) | yes | yes | yes | done |
| Project archive (pack and unpack) | yes | yes (.tewzip) | yes: import of archives, projects are plain folders | done |
| Multi-language project texts and translation dictionary | yes | yes | no | later |

## 2. Editing on the sheet

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Place, move, rotate, mirror symbols | yes | yes | yes | done |
| Select, box select, copy, cut, paste, delete, undo/redo | yes | yes | yes | done |
| Grid, snap, zoom, pan, fit page | yes | yes | yes | done |
| Free graphics: lines, rectangles, texts | yes | yes | yes | done |
| Circles, arcs, polylines and dimensions in the editor | yes | yes | partly: kept from imports, not drawn by hand | later |
| Find and replace texts, tags and wire numbers across the project | yes | yes | yes: Edit → Find (Ctrl+F), `wiringcraft find` | done (#109) |
| Macros / window macros (save and insert groups) | yes | yes | yes | done |
| Macro placeholders and value sets (one macro, several variants) | yes | partly | no | later |
| Auto-connect pins that line up | yes | yes | yes | done (#51) |
| Scanned drawing as underlay to trace over | partly | partly | yes | done |
| Layers / show-hide groups | yes | yes (DWG layers) | partly: PDF layers | done for export; editor layers later |

## 3. Symbols and library

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| IEC symbol library | yes | yes | yes: own starter library | done |
| Symbol editor (draw graphics, place and name pins) | yes | yes | yes: draw, move (drag or arrow keys) and change shapes, place pins, save to a library folder | done (#105, #159) |
| Import symbols from DXF / SVG | DXF/DWG | DWG | yes | done |
| Import symbols from KiCad libraries | no | no | yes (first unit only) | done; multi-unit todo (#96) |
| Import QElectroTech elements | no | no | no | todo (#91) |
| Library versioned with lockfile and per-item update | no (master data sync) | no (shared database) | yes | done |
| Library from git or folders | no | no | yes | done |

## 4. Devices, parts and properties

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Device tags with function, location and product aspects | yes | yes | yes | done |
| Automatic next free tag on placement | yes | yes | yes | done |
| Device properties and description | yes | yes | yes | done |
| Part (article) database with manufacturer, order number, data | yes | yes (catalog) | yes | done |
| Several parts on one device (main part plus accessories) | yes | yes | no: one part per device | todo (#106) |
| Black box / device box for devices without a symbol | yes | yes | partly: imported, not drawable | later |
| Part data from the maker's data portal | add-on (Data Portal) | yes (content portal) | EDZ part data (macros listed per part, symbol by `--symbol-map`) and BMEcat | done (#144) |
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
| Contact overflow check (more contacts drawn than the part has) | yes | yes | no: parts do not list their contacts yet | later (after #106) |

## 7. Terminals

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Terminals and terminal strips | yes | yes | yes | done |
| Terminal plan report | yes | yes | yes | done |
| Terminal strip editor (reorder, add spares) | yes | yes | no | later |
| Jumpers (bridges) between terminals | yes | yes | yes: project file, one net, terminal plan column, CLI and app; SolidWorks import to follow | done (#108) |
| Multi-level terminals | yes | yes | yes: `Level` device property, terminal plan column, one WAGO marker per block | done (#124) |
| Terminal and wire marking export (label printers) | yes | yes | yes: WAGO smartScript CSV with lines, engraving text, language and one-strip export ([docs](wago-marking.md)) | done (#57, #129) |
| SolidWorks terminal strips on import | - | - | open | open PR (#104, issue #70) |

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
| PLC card overview pages and rack configuration | yes | yes | no | later |
| Fieldbus (PROFINET, EtherCAT) topology | yes | partly | no | later |
| Exchange with PLC engineering tools (AutomationML) | yes | partly | yes (export) | done |

## 10. Reports

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Parts list / bill of materials | yes | yes | yes | done |
| Device, wire, net, terminal, cable, PLC lists | yes | yes | yes | done |
| Table of contents | yes | yes | yes, linked in the PDF | done |
| Report pages in the drawing set | yes | yes | yes | done |
| Report templates the user designs | yes | yes | no | later |
| Reports as CSV / Markdown | yes (Excel) | yes (Excel) | yes | done |

## 11. Panel layout and 3D

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| 2D mounting plate with rails and ducts | add-on (Pro Panel) | yes | yes | done |
| Place devices onto rails from the schematic | add-on | yes | yes | done |
| 3D model export (STEP) | add-on | yes | yes | done |
| 3D cabinet design with wire routing | add-on (Pro Panel) | yes (Electrical 3D) | no | later: one-way export to SolidCraft (phase 5) |
| Drilling and NC data for plates | add-on | partly | no | not planned for now: belongs to the 3D side (SolidCraft) |
| SolidWorks cabinet layouts on import | - | - | yes: each 2D cabinet layout page becomes a panel with its plate, rails, ducts and device footprints | done (#60) |

## 12. Checks

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Project check with errors and warnings | yes | yes | yes, in the app and the CLI | done |
| Online check while drawing | yes | partly | no: run on demand | later |
| User-defined check rules | yes | no | no | later |

## 13. Data exchange

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| PDF export with links and layers | yes | yes | yes | done |
| DXF / SVG export | yes (DXF/DWG) | yes (DWG/DXF) | yes | done |
| DXF import: ELLIPSE, SPLINE, HATCH | yes | yes | yes: sampled curves, solid hatches filled | done |
| DWG read and write | yes | yes | read: AutoCAD 2004 and 2010 drawings, for SolidWorks Electrical symbols and pages; no write | read done (#161, #162, #163); write not planned, DXF covers it |
| Import EPLAN projects | - | yes (some) | yes | done |
| Import SolidWorks Electrical projects | no | - | yes | done; library parts, cable types and symbol blocks with `import swe-library` (#62); cabinet layouts as panels (#60); symbol and page graphics from the DWG files (#163); follow-ups #61, #108, #149 |
| PDF trace import (turn a PDF drawing into a page) | partly | no | yes | done |
| AutomationML export | yes | partly | yes | done |
| List exchange as CSV (import and export) | yes | yes | yes | done |
| Excel workbook (.xlsx) import and export | yes | yes | no: CSV only | later |
| WAGO and other maker configurator links | add-on | partly | partly: terminal strips with rails, jumpers and accessories as a CSV for the WAGO configurator ([docs](wago-configurator.md)), and the chosen articles back onto the devices and jumpers (`import wago`, File → WAGO articles back) | partly (#122) |

## 14. Revisions and collaboration

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Revision tracking and compare | yes | yes | yes: git history, subway map, change lists | done |
| Revision marks (clouds, revision table on pages) | yes | yes | no | later |
| Review comments on pages | yes (comments in PDF) | partly | yes | done |
| Several people on one project at once | yes (multi-user) | yes (server database) | share and join in the app, page locks | todo (#43: #77, #78) |
| User rights per project | yes | yes | no | not planned: git hosting already controls who can change a project |

## 15. Automation and help

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Scripting API | yes (C# scripts, API) | yes (API) | yes: the `wiringcraft` command line | done |
| Generating schematics from tables or configurators | add-on (Cogineer) | no | partly: the assistant draws from plain words | later |
| AI assistant that edits the open project | no | no | yes | done |
| Log file for bug reports | yes | yes | yes | done (#48) |

## 16. Other disciplines

| Function | EPLAN | SWE | WiringCraft | Triage |
|---|---|---|---|---|
| Fluid power (pneumatics, hydraulics) | add-on (Fluid) | no | no | not planned for now: electrical first; a symbol set could add it later without core changes |
| Process and instrumentation (P&ID) | add-on (Preplanning) | no | no | not planned |
| Wire harness manufacturing drawings | add-on (Harness proD) | partly (3D routing) | no | not planned: needs 3D, belongs to SolidCraft |

## Todo issues filed with this matrix

Every "todo" row links an issue. These five were new with this document:

- #105 Symbol editor in the app.
- #106 Several parts on one device.
- #107 Wire cross-section, colour and type, shown in the wire list.
- #108 Jumpers between terminals, shown in the terminal plan.
- #109 Find and replace across the project.

The "later" rows get an issue when they come up for work, so the todo list keeps to work that
can start now.
