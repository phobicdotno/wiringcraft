# Manufacturer schematic data: who publishes what

Research for issue #46 (November 2026). Where component makers publish symbols, macros, part
data and 2D/3D drawings for electrical CAD, in which formats, whether a login is needed, under
which terms, and which of it WiringCraft can read today.

Nothing listed here is downloaded into this repository. Each site's terms apply to whoever
downloads the data. "Not stated" means the page we checked did not say; check the site's own
terms before relying on a download.

## What WiringCraft reads today

From `crates/wc-import`:

| Format | Module | Notes |
|---|---|---|
| ASCII DXF | `dxf` | Symbols and whole drawings, from the entities LINE, CIRCLE, ARC, ELLIPSE, SPLINE, HATCH, SOLID, TRACE, LWPOLYLINE, POLYLINE, TEXT, MTEXT, POINT, INSERT and DIMENSION (drawn by its block). Ellipses and splines become polylines through sampled points; a solid hatch and a SOLID become filled outlines, a patterned hatch its outline. Other entities (for example 3DFACE, IMAGE) are skipped, with one warning per kind. Binary DXF is **not** read. |
| DWG (formats 2004, 2007, 2010, 2013 and 2018; later AutoCAD releases save in the 2018 format) | `dwg` | Read for the SolidWorks Electrical import (symbol blocks and page drawings): custom entities (WIPEOUT, IMAGE and others numbered from 500) are counted under the name their class in `AcDb:Classes` gives, generic proxy entities as ACAD_PROXY_ENTITY; otherwise the same entities as DXF, except multi-line attributes of 2018 drawings, which are counted and skipped. |
| KiCad symbol libraries (`.kicad_sym`, KiCad 6+) | `kicad` | Graphics and visible pins of the common unit plus each unit (body style 1) become one WiringCraft symbol per unit: a single-unit symbol keeps its name, a multi-unit one becomes `<name>_A`, `<name>_B`, ... (a relay's coil and contacts), each with the common unit's graphics and pins. Alternate body styles are skipped with a warning; hidden pins are left out. |
| QElectroTech elements (`.elmt`) | `qet` | `wiringcraft import symbol-qet <files> --lib <folder>`: lines, rectangles, circles, ellipses, arcs, polygons and static texts become graphics (ellipses and elliptic arcs as polylines); terminals become pins with their direction; the label's place becomes the tag place; `link_type` sets the role (master, slave, terminal, report). 10 px of QElectroTech is 5 mm by default (`--scale`). Unknown items are counted in a warning. |
| SVG | `svg` | Graphics. |
| PDF (vector) | `pdf` | Traced drawings. |
| CSV | `csv` | Parts lists and PLC address lists. A parts list may give each part's contacts, for the contact overflow check: count columns `Main contacts`, `NO`, `NC`, `CO` (in capitals; also `Schließer`, `Öffner`, `Wechsler`, `Sluttekontakter`, `Brytekontakter`, `Vekselkontakter`, `Pole`, `Poler`), or a `Contacts`/`Kontakte`/`Kontakter` column written out (`3NO+1NC`, `3 main + 1 CO`, `1S+1Ö`). A count column wins over the same kind written out; a row whose contacts cannot be read is imported without them, with a warning naming the row (#382). |
| BMEcat catalogues (XML, 1.2 and 2005) | `bmecat` | `wiringcraft import parts-bmecat <file> --lib <folder>`: one part per product/article with order number, maker (or the catalogue's supplier), description in the catalogue's language, and as properties the maker's part number, GTIN, ECLASS (or other) class and every feature with its unit. Products marked for deletion are skipped; prices, pictures and other media are not read. |
| EDZ (EPLAN Data Exchange archives, 7-Zip or zip) | `edz` | `wiringcraft import parts-edz <file> --lib <folder>`: the part data (EPLAN part XML, `partsmanagement` root) becomes one part per `part`: order number (or type number, or the EPLAN part number without the maker), maker, description (English when given), and every other EPLAN part property; a `variant` fills what the part leaves empty. Macros (`.ema`) are EPLAN XML: each macro's multi-line representation, variant A, becomes a library symbol `eplan.<maker>.<macro name>` (the maker of the parts naming the macro, else the archive's one maker; without one the id is `eplan.<macro name>` and a warning says so, #343) with its drawing (lines, polylines, rectangles, arcs, texts, black box outlines) and a pin for each connection point (position, direction, designation; in a macro of several devices led by the device tag, `K1:A1`), and each part gets the symbol of the macro it names (`P_ARTICLE_MACRO`, else `P_ARTICLE_GROUPSYMBOLMACRO`). Symbols EPLAN keeps in its own libraries (coils, contacts, lamps) are drawn as the WiringCraft symbol of the same kind. The report counts what is left out (other representations and variants, connections, potentials) and lists, per part, the macro it names and whether the archive holds it (#311). `--symbol-map <file>` (one `macro = symbol.id` per line, matched by full name, file name or name without `.ema`) gives mapped parts that library symbol. Pictures and other files are counted (#144). Part data built from public format knowledge and synthetic archives; the macro reader worked out from a set of about 240 real maker macros (device, PLC module and circuit macros), which are not in the repository, and tested with synthetic macros. |
| AutomationML (IEC 62714, CAEX XML) | `aml` | Devices, properties, connection points, links. |
| EPLAN P8 projects (`.elk` + `.edb`) | `eplan` | Projects. Parts from EDZ files: see `edz` above. |
| SolidWorks Electrical archives (`.tewzip`) | `swe` | Projects. Content Portal parts also arrive as `.tewzip`; `import swe-library` reads their parts, cable types and symbol blocks (blocks drawn from their DWG files in the archive, #230; a block without one is a box with its connection points). |
| Plain zip | `archive` | Opens the container and imports PDF, DXF and AutomationML entries inside; `.kicad_sym` entries go through the KiCad importer into the folder given with `--lib` (listed with a warning without it). CSV and Excel (`.xlsx`) entries are told apart by their header, like the single-file commands: a device and an address column make a PLC address list, applied to the project's devices after the pages (as `import plc-csv`); an order/part number column makes a parts list, imported into `--lib` (as `import parts-csv`; listed with a warning without it). A CSV that is neither is listed with a warning. Other entries are listed, not imported. |

In the app, **File → Import** (and the ribbon's Import tab) runs the same project and parts
imports as `wiringcraft import`, through the same code: EPLAN project (`.elk` or a zip of it),
SolidWorks Electrical project (`.tewzip`) and libraries, AutomationML, a DWG or DXF drawing onto
the current page, PLC addresses from a CSV list, PDF pages, a project archive (zip), and parts
from EDZ, BMEcat and CSV. Each opens the system file picker filtered to the format's extensions.
Parts imports first ask for the library folder, like Import symbols: the project's own
`library/` (added to the project's libraries once parts are in it), one of its other library
folders, or another folder picked with the folder picker. Where the picker cannot open (Linux
without a desktop portal), the file and folder are typed into a dialog instead. The import runs
off the window thread: a window shows what it has done so far, and until it ends the project
cannot be edited, saved, shared or replaced by another one (a running assistant turn may only
use its reading tools, and no new turn starts). The window has the view controls that still work
(zoom, grid, theme, layers). Messages that name a command line option are worded
for the app.

Where the format has options on the command line, a small dialog asks for them after the file
is picked, with the command line's defaults filled in; a format without options starts at once.
A DWG or DXF drawing has its scale (`--scale`, 1 by default: the drawing's coordinates are
multiplied by it to get mm). The imports that add pages (EPLAN, SolidWorks Electrical,
AutomationML, PDF, archive) have the first page number (`--first-page`), filled in with the
command line's default, or with the number after the project's highest page number when that is
higher; a SolidWorks Electrical import can keep the source's own page numbers instead, as the
command line does without `--first-page`.

Changes not saved yet are saved first, the imported pages and devices come in as one undo step,
and the import's report and warnings are shown when it is done. The first page number offered
is the CLI's default, or one above the project's highest numeric page number when that is higher
(with page 101 in use, 102 even when 100 is free, since the imported pages follow on from one
number); for a SolidWorks archive it is offered from 100 in the same way, with its own page
numbers as the other choice. A SolidWorks archive that holds several projects lists them (number,
folder and name) and the first is chosen, as `--pick 1`; when the archive's project list cannot
be read, the folders are listed alone and the dialog says why. A first page number above
1 000 000 000 is refused, on the command line too.

A SolidWorks Electrical project and an EDZ parts import have the symbol map file
(`--symbol-map`): a field with a Browse button, empty by default, which imports with no map as
the command line does without the option. A file that is not there is refused before the import
starts. A relative path typed there is taken inside the projects folder, as a typed import file
is; the empty field names that folder.

An archive has the library folder for the KiCad symbol libraries and parts lists inside it
(`--lib`): "Do not import them" by default, which lists them with a warning as the command line
does without `--lib`, or the project's own `library/`, one of its other library folders, or
another folder typed or picked with Browse, as for a parts import. When the project's library
folders cannot be listed, the dialog says why and another folder can still be chosen. The
entries of an archive are numbered on from the first page number, also past the limit on the
number typed. A folder that is not among
the project's libraries yet is added to them once something was written there (a symbol
library or a part), also when the folder was there before. The SolidWorks Electrical libraries
import vendors into the project, as `import swe-library` does without `--lib`; writing them
into a library folder instead (`import swe-library --lib`) is on the command line only.

In the app, **Library → Import symbols…** takes DXF, SVG, `.kicad_sym` and `.elmt` files together,
each read by its extension, and writes them into one of the project's library folders. The
project's own folder, `library/` in the project, comes first and is added to the project's
libraries (ahead of the others) the first time symbols are written there. Ids are `dxf.<file name>`, `svg.<file name>`, `kicad.<file name>.<symbol>` and
`qet.<file name>`; QElectroTech uses the default 5 mm grid and KiCad scale 1. The result lists every
symbol written, every warning and every file that failed; a failing file does not stop the others.
A symbol already in the folder is kept unless "Replace symbols already in the library" is ticked.

Not read today: the panel layout, overview, single-line and 3D representations of EPLAN macros (counted only, #343), **DWG** older than AutoCAD 2004, **STEP** (3D), ECLASS data
outside a BMEcat catalogue.

## Portals that serve many manufacturers

| Portal | What | Formats | Login / cost | Terms | WiringCraft today |
|---|---|---|---|---|---|
| [EPLAN Data Portal](https://www.eplan.com/us-en/products/eplan-data-portal) | 550+ manufacturers, 2.3 million+ components: commercial and technical data, schematic macros, 3D, images | EDZ / EPLAN macros | Free **with a current EPLAN licence subscription and an EPLAN ID**. Viewing without a licence has been allowed since Data Portal 2.1 ([news](https://www.automation.com/article/eplan-updates-data-portal-to-allow-free-viewing)). | Made for use inside EPLAN ([portal terms](https://www.eplan.com.cn/content/dam/eplan/corporate/legal/terms-of-use/eplan-epulse-portal-terms-of-use-10-2020-cn-cn.pdf)) | No (EDZ) |
| [SOLIDWORKS Electrical Content Portal](https://help.solidworks.com/2025/English/SWElec/r_swelec_UsingECP.htm) | Manufacturer parts, cable references, symbols. TE Connectivity via TraceParts ([blog](https://blogs.solidworks.com/products/solidworks/traceparts-collaboration-integrating-the-te-connectivity-catalog-into-the-electrical-content-portal-ecp)) | `.tewzip` | 3DEXPERIENCE ID plus an active SOLIDWORKS Electrical Schematic subscription ([Javelin](https://www.javelin-tech.com/blog/2025/02/using-the-solidworks-electrical-content-portal/)) | Dassault terms | Container yes; parts, cable types and symbol blocks (drawn from their DWG files, #230) with `import swe-library` |
| CADENAS PARTcommunity / TraceParts | 2D/3D CAD for thousands of makers, some ECAD macros | 80+ native and neutral formats incl. DXF, DWG, STEP | Free account | Per portal and per maker | DXF yes (ASCII) |
| [KiCad libraries](https://www.kicad.org/libraries/license/index.html) | Generic and manufacturer-specific symbols (mostly electronics) | `.kicad_sym` | Free, no login (GitLab) | CC-BY-SA 4.0 with the KiCad libraries exception: free to use in any design, closed or commercial; redistributing the libraries as a collection keeps CC-BY-SA | **Yes** |
| [QElectroTech element collection](https://qelectrotech.org/) and [contrib repo](https://github.com/qelectrotech/qelectrotech-element-contrib) | IEC schematic elements, manufacturer variants (Hager, ABB, Hensel, Shelly, WAGO planned) | `.elmt` (XML) | Free, no login | Contrib repo under GPL-2.0; check the main collection's licence per element | **Yes** (symbols) |

## Manufacturers

| Manufacturer | Product area | Where | Formats | Login | Terms | WiringCraft today |
|---|---|---|---|---|---|---|
| **WAGO** | Terminal blocks, I/O, interface | [Product pages, Downloads / CAE data](https://www.wago.com/global/digitization/digital-engineering/eplan); EPLAN Data Portal; [other CAE tools](https://www.wago.com/global/digitization/digital-engineering/engineering-data) | EPLAN Data Standard macros (11,000+ products, logic macros, 3D); WSCAD, StriePlan, AutoCAD Electrical, SEE Electrical, HagerCAD; 2D/3D via CADENAS | Product pages: not stated; Data Portal: EPLAN ID | Not stated | 2D DXF via CADENAS yes; EPLAN no. No SolidWorks Electrical catalogue confirmed. |
| **Phoenix Contact** | Terminals, connectors, power, I/O | [Product data for engineering](https://phoenixcontact.com/en-gb/product-data-for-your-engineering-process) | EPLAN P8 file generator and EPLAN Data Portal; AutoCAD Electrical, SEE Electrical, WSCAD, Zuken, Aucotec, AmpereSoft, SDProget, NX Electrical; BMEcat XML; ECLASS | Not stated | Not stated | BMEcat parts: yes |
| **Siemens** | PLCs, drives, switchgear | Industry Support product page, **CAx data** tab, CAx Download Manager ([how-to](https://industrialmonitordirect.com/blogs/knowledgebase/download-siemens-eplan-edz-macros-via-cax-data-manager)) | EPLAN P8 macros (EDZ); 2D/3D CAD (DXF, STEP) without connection points | Not stated; the download tool needs Java | Siemens terms | DXF yes (graphics only) |
| **Schneider Electric** | Switchgear, drives, PLCs | se.com document downloads (e.g. [EPLAN EDZ](https://www.se.com/hk/en/download/document/EPLAN_EDZ_Spacial_SM/)); EPLAN Data Portal | EDZ, EPLAN macro templates; DXF/DWG per product | Not stated | Schneider terms | DXF yes |
| **Eaton** | Switchgear, motor control | [ECAD / MCAD / EPLAN tools](https://www.eaton.com/us/en-us/support/tools/ecad-mcad-eplan-.html): eCAD Data Portal and mCAD Data Portal on PARTcommunity; EPLAN Data Portal (30,000+ products) | eCAD macros in native formats plus neutral DXF/DWG; 2D/3D in 80+ formats | Personal eCAD/PARTcommunity account | Portal terms | DXF yes |
| **ABB** | Low voltage products, drives, PLCs | [ABB EPLAN page](https://new.abb.com/low-voltage/de/niederspannungsprodukte/eplan); EPLAN Data Portal | EPLAN macros and parts | EPLAN Data Portal: EPLAN ID | Not stated | No |
| **Mitsubishi Electric FA** | PLCs, servo, inverters | [FA download centre, CAD data](https://www.mitsubishielectric.com/fa/download/); EPLAN Data Portal | 2D DWG (2000) and DXF; 3D IGES, Parasolid, STEP AP203 | Accept terms; login not stated | [CAD terms](https://www.mitsubishielectric.com/fa/products/cnt/plc_fx/dldb/copyright_cad.html): for design consideration, simplified geometry, no guarantee of specs | DXF yes (graphics only) |
| **Rockwell Automation / Allen-Bradley** | PLCs, drives, motor control | [Product compatibility and download centre](https://www.rockwellautomation.com/en-us/support/product) | EPLAN and AutoCAD Electrical drawings, DXF/DWG, 3D | Not checked | Rockwell terms | DXF yes |
| **SEW-EURODRIVE** | Drives, gear motors | [CAE data in Online Support](https://download.sew-eurodrive.com/download/html/27806162/en-EN/41660761483.html); EPLAN Data Portal | EDZ | Data Portal: register and log in | Not stated | No |
| **Dold** | Safety and monitoring relays | [EPLAN macros](https://dold.com/en/service/eplan) on product pages and the EPLAN Data Portal | EPLAN product macros | Not stated | Business customers only for sales | No |
| **Euchner** | Safety switches | [EPLAN macros](https://www.euchner.com/en-us/service/downloads/eplan-macros/), Downloads tab, EPLAN Data Portal | EPLAN macros | Not stated | Not stated | No |
| **Lenze** | Inverters, motor starters | [E-catalogue per product](https://www.lenze.com/bg-bg/go/akb/200400010/1) | EPLAN macros (schematic and mounting plate), article data | Not stated; described as free | Not stated | No |
| **igus** | Pre-assembled cables (readycable) | EPLAN Data Portal, linked from the online shop ([press](https://press.igus.eu/igus-is-adding-2046-new-macros-to-the-eplan-circuit-diagram-software/)) | EPLAN macros (2,046 cable macros and more) | EPLAN ID | Not stated | No |
| **Beckhoff** | I/O, IPCs, drives | Beckhoff download site ([install guide](https://download.beckhoff.com/download/Document/fi/Install_EPLAN_Macros.pdf)) | EPLAN macros | Not stated | Not stated | No |
| **Festo** | Pneumatics, valve terminals | EPLAN Data Portal ([news](https://news.thomasnet.com/companystory/2-000-festo-devices-macros-developed-for-eplan-users-20000086)) | EPLAN macros | EPLAN ID | Not stated | No |
| **KEB** | Drives | [KEB macros in the EPLAN Data Portal](https://www.keb-automation.com/technology/industrial-standards/topics/eplan) | EPLAN macros | EPLAN ID | Not stated | No |
| **Rittal** | Enclosures, climate | [Engineering](https://www.rittal.com/au-en/Solutions/Enclosure-manufacturing-40/Engineering); EPLAN Data Portal | EPLAN macros, 3D | EPLAN ID | Not stated | No |
| **Weidmüller** | Terminals, connectors | [Digital engineering downloads](https://www.weidmuller.com/us/downloads/gsd---eds-files) | EPLAN and other CAE data | Not stated | Not stated | No |

## What this means for WiringCraft

1. **Readable now:** KiCad libraries (open licence with clear terms, read directly), and any
   maker's 2D **ASCII DXF** drawings for symbol graphics (within the entities listed above). Being able to read a file is not
   permission to use it: most makers' terms are not stated on the pages checked, and some limit
   use (Mitsubishi's CAD data is "for design consideration" only), so check each maker's terms
   before building a library from their files. DXF from Siemens and similar sites has no
   connection points, so pins must be added after import.
2. **The biggest pool is EPLAN EDZ.** Almost every industrial maker publishes EPLAN macros, most
   through the EPLAN Data Portal, which needs an EPLAN licence. Many also put EDZ files on their
   own product pages without the portal (Siemens CAx data, Schneider, SEW, Dold, Euchner, Lenze,
   Beckhoff). The EDZ importer (macros to symbols, parts to the library, #144 and #311) opens that pool
   for people who download from those pages; like the EPLAN project importer it was built from
   the public format, real files and its own test files.
3. **Part data without drawings:** Phoenix Contact's BMEcat XML and ECLASS classification carry
   order numbers, descriptions and technical data. The `bmecat` importer fills the parts library
   from them for makers who publish it.
4. **QElectroTech `.elmt`** is an open XML format with a free collection that includes some
   manufacturer variants (Hager, ABB, Shelly). The `qet` importer reads them as symbols with
   connection points; check each element's licence before shipping it in a library.
5. **DWG** is common (Eaton, Mitsubishi, Rockwell) but closed; DXF is usually offered next to it.
6. **SolidWorks Electrical Content Portal** data needs a SolidWorks subscription; its `.tewzip`
   parts, cable types and symbol blocks are read by `wiringcraft import swe-library`; a block is
   drawn from its DWG file in the archive (#230), else it is a box with its connection points.

Follow-up candidates for the todo list: EDZ import.

