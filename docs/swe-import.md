# Importing SolidWorks Electrical projects and libraries

What WiringCraft takes from a SolidWorks Electrical archive, what it approximates, and what it
leaves out. There are two imports: `import swe` brings in a project (pages, symbols, wires,
devices), and `import swe-library` brings in the libraries an archive carries (parts, cable
types, symbols). Their command line options are listed in
[the command line imports page](cli/imports.md). EPLAN projects are covered in
[eplan-import.md](eplan-import.md).

## Projects: `import swe`

`wiringcraft import swe <path>`, or in the app **File > SolidWorks Electrical project** (also on
the **Import** ribbon tab).

### What it reads

The path is one of:

- a project archive (`.tewzip`), the zip SolidWorks Electrical exports. An archive can hold
  several projects. The first is imported unless you pick another with `--pick`, and the import
  says which one it took when there are several;
- a project database (`tew_project_data.sqlite`) on its own.

From an archive, the import reads the picked project's database, the project's own fields from
the archive's project list (name, customer, addresses, contract number, who made it and when),
the archive's catalog, and the DWG drawings of the symbols and pages. A bare database has none of
these: its title block fields stay empty, devices keep their part numbers as text, and symbols
that match no library symbol are drawn as boxes.

The import adds to the open project: it never replaces pages, devices or symbols the project
already has. To see an import's result on its own, make a new project first and import into it:
`wiringcraft new pump-import`, then `wiringcraft -C pump-import import swe <path>`.

### Pages

- Schematic, line-diagram, mixed schematic, P&ID and fluid pages are rebuilt as drawn. They keep
  the source's page labels (with the book number in front, `2.05`, when the project has several
  books), unless you give `--first-page`, which numbers them from there. A number used twice
  gets a suffix (`5` becomes `5-2`). Line-diagram pages are marked as overview pages.
- Each page is titled `<folder>: <page title>` and takes the sheet size its title block names,
  or a larger one when the content does not fit. A page reaching beyond A2 is kept as is, with a
  warning.
- The first cover page becomes the project's cover page, which WiringCraft draws from the
  project's fields. Further cover pages are left out.
- 2D cabinet layout pages become panel layouts: the mounting plate, DIN rails, wire ducts and
  the device footprints. Upright rails, and rails or ducts without a length, are left out.
- Reports (bill of materials, terminal tables) and other pages are not imported. WiringCraft
  makes its own reports.

From each page's own drawing, only texts, frames and notes come across. Block references (the
symbols, and the title block, which WiringCraft draws itself) and lines that lie on a wire are
left out.

### Symbols

Each placed symbol keeps its connection points at their original positions. It is drawn as:

1. a library symbol, when a block's pins match a built-in one (coil A1/A2, contacts 13/14 and
   11/12, lamp X1/X2, terminal 1/2) and sit where the block's points are, or when your symbol
   map (`--symbol-map`) names one for the block. The symbol must be in the project or its
   libraries, and a mapped symbol must fit the block too: the same pin names and number of
   pins, each pin where the block's point is (in one of the eight right-angle turns and
   mirrors), and a terminal or interruption point only for a terminal or interruption point
   block. A mapped symbol that does not fit is not used, and the block is drawn as below;
2. otherwise, its block's DWG drawing from the archive;
3. otherwise, a box with its connection points.

Symbols of the last two kinds get ids `swe.<block>.<n>` and the category
`SolidWorks Electrical import`. A symbol without connection points (a frame, a label) is not
placed.

### Devices and parts

Every component with a usable tag becomes a device. Tag paths are kept with their aspects
(`=F1+L1-K1`). When every device shares one function, it is left out of the tags and kept as
the device property `Function`. A tag already taken gets a suffix (`-K1` becomes `-K1_2`).

A device gets its description and the properties `PartNumber` and `Manufacturer`, with further
parts as `PartNumber 2`, `Manufacturer 2`, and so on. From an archive, a device whose part is in
the archive's catalog is linked to that part, further parts become accessories, and those parts
come along into the project.

From an archive, the project's fields fill the title block. Customer, project number, drawn by
and date are printed; the others (description, addresses, drawing office, modified by and
modified date) are kept in Project Properties.

### Wires, cables and terminals

- Wires are joined to the symbols' pins, with their wire number, cross-section and colour.
  Wires on the same potential stay connected across gaps and pages. When one line carries
  several wires, the WiringCraft wire shows the first number and the first cable core.
- Cables become devices with their part data and number of cores, and each wire on a core names
  its cable and core.
- Terminal strips and their terminals come across, a terminal `3` of strip `-X1` as `-X1.3`.
  WiringCraft lists a strip's terminals by number, also where the source has another order.
- A wire between two terminals of one strip that no drawn line carries (a jumper made in the
  terminal strip editor) becomes a jumper of that strip.
- The arrows that carry a wire to another page become interruption points `-OD1`, `-OD2`, ...

### The report and warnings

The import ends with a table of what the source has, what was imported, and why the rest was
not. A small database gave:

```
$ wiringcraft import swe tew_project_data.sqlite
warning: terminal strip(s) -X1 have their terminals in another order in the source than by number; WiringCraft lists them by number
warning: 1 wire(s) join two terminals that are not connected on the imported drawing: wire N2: -X1.2:1 (page 01) and =F1+L1.L2-K1:A1 (page 01)
                     source  imported  why the rest is not imported
pages                     2         1  1 report or other non-schematic page(s) (WiringCraft makes its own reports), or empty layout page(s)
components                6         5  1 without a usable tag
symbols                   6         4  1 on pages that are not imported; 1 without connection points (frames, labels, texts)
wires                     6         3  3 not on an imported page's line
cables                    2         2  1 not on any imported wire (device only)
...
```

Other warnings name rows that could not be read, tags that were taken, page numbers used twice,
a symbol map entry no library has, and catalog rows or drawings that could not be read.

The import stops without writing anything when a page number or page is already in the project
(`page number 01 is taken by page ...; import with --first-page ...`, or `page ... exists;
remove it or import into a fresh project`), or when a device tag the project already has comes
with other data. So importing the same project twice into one project is refused.

## Libraries: `import swe-library`

`wiringcraft import swe-library <archive.tewzip>`, or in the app **File > SolidWorks Electrical
libraries** (on the **Import** ribbon tab under Parts). It reads the libraries a `.tewzip`
archive carries, a project archive or a Content Portal download. A bare database is refused.

- Catalog parts, with ids `<manufacturer>.<reference>`, their description, their terminals in
  the property `Terminals` (`Coil: A1 A2; NO: 13 14`) and data such as voltage and current where
  the catalog has it.
- Cable types, as parts with the property `Kind` set to `Cable`, their number of cores, and
  their core colours and cross-section where given.
- Symbol blocks, as symbols `swe.lib.<block>` in the category `SolidWorks Electrical library`,
  with their connection points as pins, drawn from the block's DWG file in the archive, or as a
  box around the points when there is none.

They go into the library folder given with `--lib`, or into the project when none is given.
With `--lib`, a file already in the folder with the same id is written over without a warning,
so keep a copy of library items you edited by hand before importing again. A
part or cable type without a reference, or repeating one already read, and a symbol block
without connection points, are left out with a warning.
The import ends with one line each for parts, cable types and symbol blocks: how many the source
has and how many were imported.

Without `--lib`, a symbol or part the project already has is replaced only when its copy is
still the one the project pins; a copy changed in the project is refused, not replaced. A copy
with no entry in `library.lock` (put there by hand, or its entry lost) has no pin to compare
with and is replaced whatever it holds, so keep a copy of such a file before importing.

Both ways below replace the project's changed copy, so save your edits to it first (a copy of
the file under `vendor/`, or a commit) if you want to keep them.

To take a new version into a project whose copy was changed, import with `--lib` into a
library folder and add that folder as a source with `lib add`. `lib add` puts the new source
last and earlier sources win, so when another source has the same id, move the new
`[[library]]` entry in `project.toml` above it. Then run `lib update <id>`. When the archive's
item is the very version the project pins, `lib status` shows it as up to date and `lib
update` leaves the changed copy alone; to go back to the pinned version, delete the project's
copy (`vendor/symbols/<id>.json` or `vendor/parts/<id>.json`) and run `lib lock`, which writes
it again.
