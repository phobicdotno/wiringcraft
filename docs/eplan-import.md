# Importing EPLAN and SolidWorks Electrical projects

What WiringCraft takes from an EPLAN Electric P8 project and from a SolidWorks Electrical
archive, what it approximates, and what it leaves out. The command line options of these
imports are listed in [the command line imports page](cli/imports.md).

Both imports add to the open project: they never replace pages, devices or symbols it already
has. To see an import's result on its own, import into a new project (`wiringcraft new`).

## EPLAN Electric P8

`wiringcraft import eplan <path>`, or in the app **File > EPLAN project** (also on the **Import** ribbon tab).

### What it reads

The path is one of:

- the project's `.elk` file: its first line names the `.edb` folder next to it, which is read;
- the `.edb` folder itself;
- a zip of the `.edb` folder (the folder may sit at any depth in the zip).

The app's file dialog offers `.elk` and `.zip` files. To import an `.edb` folder directly, use
the command line.

From the folder, only the project tables (`*.eod` files) and `ProjectInfo.xml` are read. EPLAN's
format is not published: the importer was worked out from sample projects and their PDF
exports. A project saved by a newer EPLAN version may not be recognised (see the warnings below).

### Pages

Every EPLAN page that has its own graphics or placed devices is rebuilt as drawn, on an A3 page
titled `Eplan page <name>: <description>`. The pages are numbered from 400 unless you give
another first page number. Pages with nothing on them are skipped.

From each page:

- lines, rectangles, circles and texts drawn on the page itself. A text in several languages
  is shown in English, else in its language-neutral form, else in its first language. Each line
  of a multi-line text becomes a text of its own;
- devices, at their original positions and turned or mirrored as in EPLAN;
- wires, see [Connections](#connections).

Positions are divided by the page's scale. A page drawn at a larger scale than 1:2 is taken as
a mounting layout: its graphics come across, but no devices are placed on it.

Graphics of forms (EPLAN's report and frame templates) are not read, so report pages such as
parts lists and terminal diagrams do not come across as drawn. WiringCraft makes its own reports.

### Symbols

EPLAN keeps symbol graphics in its symbol libraries, not in the project, so the import cannot
copy them. Each placed device gets a simple WiringCraft symbol of the same kind instead, with
EPLAN's connection point names on its pins:

| EPLAN device | Drawn as |
|---|---|
| Terminal | a small circle with the terminal designation |
| Two connection points whose names start with A (A1, A2), not a fuse | a coil |
| Lamp with two connection points | a lamp |
| Relay, contactor or push-button contact with two points | a make contact, or a break contact when both point names end in 1 and 2 (or 5 and 6) |
| Contact with three points | a changeover contact |
| Circuit breaker with pairs of points | a multi-pole switch |
| Fuse with pairs of points | one fuse per pole |
| Anything else with connection points | a box with its points along the top |

The symbols are laid out on EPLAN's 4 mm grid, so wires meet the pins. They have ids starting
with `eplan.` and the category `imported`. A connection point without a name is numbered by its
place (1, 2, ...); a name used twice on one device gets a mark (`1'`).

A device without connection points (a black box outline, a push-button head) is not drawn. A
cable definition shows only its name, as text, where EPLAN places it.

Macros: a device placed as part of a macro or black box counts as being on that macro's page.
The macro itself is not kept as a unit. EPLAN macro files (`.ema` in an EDZ archive) are read by
the parts import, `wiringcraft import parts-edz`, not by this one.

### Devices and parts

Every device tag in the project becomes a WiringCraft device. EPLAN's tag `PUMPE1-M1` becomes
`-PUMPE1.M1`, and a terminal `7` of strip `X1` is placed as `-X1.7`. Characters a tag cannot
hold become `_`.

A device gets these properties, where EPLAN has them:

- `Function text` (also used as the device description) and `Technical data`. When a device has
  several functions with different texts, they are joined with ` / `;
- for cables: `Cable type`, `Conductors`, `Cross-section` and `Length`;
- `PartNumber`: the first part number of the device;
- `Manufacturer`: from the part data cached in the project, or else the part of the part number
  before its first dot (`SIE.3RT2015` gives `SIE`);
- `Part description` and `Type number`, from the cached part data;
- `Additional parts`: the device's other part numbers, as a list.

A device with a part number gets a part in the project, with the manufacturer, the part number
as order number and the part description. Only the first part number becomes a part; the others
stay in the `Additional parts` property. EPLAN projects often cache only some of the part data
they use; for full part data, import the maker's EDZ file with `import parts-edz`.

The project's customer and description fill the title block fields `customer` and `description`
when they are empty.

### Connections

On a rebuilt page, wires are traced the way EPLAN connects automatically: between two
connection points that face each other on one line with nothing in between. EPLAN's wire
corners and junctions are read for this. Each traced wire takes the cross-section and colour of
the EPLAN connection it draws. A plain number is read as mm² (`1.5` becomes `1.5 mm²`), anything
else is kept as written (`18 AWG`).

Devices that are not placed on a rebuilt page are laid out on connection overview pages after
the drawn ones: one section per EPLAN page, titled `Connections, Eplan page <name>:
<description>`, and a last section `Eplan devices on layout and report pages` for devices on no
schematic page. There, each device is a box with its connection points, and every connection of
EPLAN's connection list between two of them is a wire, with its cross-section and colour.

Not imported:

- connections to a name without a connection point, such as a potential or a cable end;
- the cable a connection runs in (cables come across only as devices with their properties);
- wire numbers and potential names.

### Warnings

The import ends with a summary such as `imported 12 Eplan pages as 14 page(s), 10 of them as
drawn: 85 devices, 240 connections`. Before it, it may print:

- `no pages or devices were recognised; the project may use a newer format`: the tables were
  found but nothing in them could be read.
- `<n> connection(s) without named ends were skipped`.
- `<n> connection(s) to a name without a pin (potentials, cable ends) are listed in the device
  properties only`: these connections are not imported (see above).
- `<n> connection(s) of the connection list to devices on drawn pages were left out; their wires
  are traced from the drawing`: expected. Compare the drawn pages with EPLAN's if a wire is
  missing.
- `<n> wire corner(s) or junction(s) of an unknown kind were joined in every direction a wire
  lines up; check them`.
- `device "<name>" has no usable tag`: that device is left out.
- `<file>: <error>; read without it`: `ProjectInfo.xml` could not be read, so the title block is
  not filled. Any other table that is there but cannot be read stops the import with an error
  naming it.

The import stops without writing anything when the path is not an `.elk` file, `.edb` folder or
zip (`... is not an Eplan .elk file, .edb folder or zip`), when no `*.eod` tables are found, when
a new page would take a page number the project already uses (`page number 400 is taken by page
...; import with --first-page ...`), or when a device tag the project already has would come
with other data (`device ... exists with other data; rename it or import into a fresh
project`).

### Getting closer to the original

The rebuilt pages use stand-in symbols. To compare with the original drawing, export the EPLAN
project as PDF and add it as trace pages with `wiringcraft import pdf`.

## SolidWorks Electrical

### Projects: `import swe`

`wiringcraft import swe <path>`, or in the app **File > SolidWorks Electrical project**. The
path is a project archive (`.tewzip`) or a project database (`tew_project_data.sqlite`). An
archive can hold several projects; the first is imported unless you pick another, and the import
says which one it took.

What comes across:

- Schematic, line-diagram, mixed schematic, P&ID and fluid pages, with their page numbers kept
  unless you give a first page number. A number used twice gets a suffix (`5` becomes `5-2`).
  Line-diagram pages are marked as overview pages.
- The first cover page. Further cover pages are not imported: WiringCraft has one cover page,
  drawn from the project's fields.
- 2D cabinet layout pages, as panel layouts.
- Symbols, drawn from their DWG blocks in the archive with their connection points at the
  original positions. A block whose pins match a built-in WiringCraft symbol (coil A1/A2,
  contacts 13/14 and 11/12, lamp X1/X2, terminal 1/2) uses that symbol; a symbol map file can
  choose others. A block without a readable drawing is drawn as a box, and so is every block
  when you import a bare database, which has no DWG files.
- From each page's own drawing: texts, frames and notes. Block references (the symbols and the
  title block, which WiringCraft draws itself) and lines that lie on a wire are left out.
- Wires joined to the symbols, with wire numbers, cross-section and colour; cables with their
  cores; terminal strips and their terminals; jumpers between terminals of one strip.
- The arrows that carry a wire to another page, as interruption points `-OD1`, `-OD2`, ...
- Devices with their part data. From an archive, a device that names a part of the archive's
  catalog is linked to that part, and the part comes along.

Reports (bill of materials, terminal tables) are not imported; WiringCraft makes its own.

The import ends with a table of what the source has, what was imported, and why the rest was
not, for pages, components, symbols, wire lines, wires, cables, terminal strips, terminals and
jumpers. Warnings before it name rows that could not be read, tags that were taken and got a
suffix (`-K1` became `-K1_2`), pages that reach beyond A2, and wires that join two terminals not
connected on the imported drawing.

### Libraries: `import swe-library`

`wiringcraft import swe-library <archive.tewzip>`, or in the app **File > SolidWorks
Electrical libraries**. It reads the libraries a `.tewzip` archive carries (a project archive or
a Content Portal download):

- catalog parts, with their terminals as properties;
- cable types, as parts with their cores;
- symbol blocks, as symbols `swe.lib.<block>` with their connection points as pins, drawn from
  the block's DWG file in the archive, or as a box around the points when there is none.

They go into a library folder, or into the project when no folder is given. A symbol or part the
project already has is replaced only when its copy is still the one the project pins; a copy
changed in the project is refused, not replaced. The help of `wiringcraft import swe-library`
says how to take a new version into such a project.
