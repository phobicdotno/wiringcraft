# Command line imports

`wiringcraft import <command>` reads a file from another tool into the open project, or into a
library folder for the commands that take `--lib`. The project is the current folder unless
`-C <dir>` (or `--project <dir>`) names another; after a subcommand that takes a device tag,
write it with a space (`-C <dir>`), since an attached `-C<dir>` there is read as a tag.
`wiringcraft import <command> --help` prints the same options as this page.

All imports print warnings on standard error as `warning: ...` lines and the result on standard
output. A command that fails prints `error: ...` and exits with code 1. Texts taken from the file
(entry names, block names) are printed with control characters escaped.

Commands that add pages refuse a page number or page id the project already has, before anything
is written, and name the page that holds it: import again with `--first-page` set to a free
number. One exception: `pdf` checks each page's id (`pdf-<number>`) only as it gets to that page,
so a scanned page before it may already have left its picture in `underlays/` (as
`underlays/pdf-<number>.png` or `.jpg`), writing over a file of that name. Before you delete
that file, check that no other page uses it (search `pages/` for its name); if one does, get the
old file back from the project's history instead (`git checkout -- underlays/<file>` when it was
committed). A first
page number above 1000000000 is refused.

Imports that add devices, symbols or parts to the project (`aml`, `eplan`, `swe`, `swe-library`
without `--lib`) check everything first, so a refused import leaves the project as it was. They
refuse:

- a page or panel id the project already has (`page <id> exists; remove it or import into a fresh project`),
- a device tag the project has with other data (`device <tag> exists with other data; rename it or import into a fresh project`),
- a symbol or part the project vendors with other content (`... exists with other content; import into a fresh project`),
- a new version of a symbol that drops a pin a wire in the project ends on (`move those wires first`).

The same item coming again unchanged is fine. When a save fails half way, the files are put back.
If putting a file back fails too, the error says which ones (`putting back the vendored files
failed too: ...`); check those files, for example against the project's history.

## Symbols into a library

### symbol-dxf

```
wiringcraft import symbol-dxf [OPTIONS] --id <ID> --name <NAME> --lib <LIB> <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | The DXF file (ASCII DXF; a binary DXF is refused with `save as ASCII DXF`) |
| `--id <ID>` | The new symbol's id, e.g. `my.relay.coil` |
| `--name <NAME>` | The symbol's name, shown in the library |
| `--lib <LIB>` | The library folder to write into |
| `--category <CATEGORY>` | Library category to file it under, e.g. `relays` (default empty) |
| `--letter-code <LETTER_CODE>` | Letter code for new devices, e.g. `K` (default empty) |

Writes `<lib>/symbols/<id>.json`. Pins are POINT entities, or entities on a layer named `PIN`;
the first pin becomes the symbol's origin. A symbol file with the same id is written over. A
drawing with no pins is still written (`0 pins`), with no warning. Prints the file written and
its pin and shape counts.

```sh
wiringcraft import symbol-dxf coil.dxf --id my.coil --name "My coil" --lib ../mylib --category relays --letter-code K
```

### symbol-svg

```
wiringcraft import symbol-svg [OPTIONS] --id <ID> --name <NAME> --lib <LIB> <FILE>
```

The same options as `symbol-dxf`, with `<FILE>` an SVG file. Pins are elements with
`id="pin:NAME"` or `data-pin="NAME"`. Writes `<lib>/symbols/<id>.json`, over a file with the same id.

```sh
wiringcraft import symbol-svg lamp.svg --id my.lamp --name "My lamp" --lib ../mylib --letter-code P
```

### symbol-kicad

```
wiringcraft import symbol-kicad [OPTIONS] --lib <LIB> <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | The `.kicad_sym` file (KiCad 6 or later) |
| `--lib <LIB>` | The library folder to write into |
| `--prefix <PREFIX>` | Id prefix (default `kicad.<file name>`) |
| `--scale <SCALE>` | Multiply coordinates, e.g. 2 to put KiCad's 2.54 mm pin grid near a 5 mm grid (default 1) |
| `--category <CATEGORY>` | Library category to file the symbols under (default empty) |

Writes every symbol of the library as `<lib>/symbols/<prefix>.<symbol name>.json`, one per unit,
and prints each one (`R -> .../kicad.demo.r.json (2 pins, 3 shapes)`). The first pin becomes the
origin. Fails with `no symbol could be imported from the file` when nothing could be read.

```sh
wiringcraft import symbol-kicad Device.kicad_sym --lib ../mylib --scale 2
```

### symbol-qet

```
wiringcraft import symbol-qet [OPTIONS] --lib <LIB> <FILES>...
```

| Option | Meaning |
|---|---|
| `<FILES>...` | One or more QElectroTech `.elmt` files |
| `--lib <LIB>` | The library folder to write into |
| `--prefix <PREFIX>` | Id prefix (default `qet`) |
| `--scale <SCALE>` | Millimetres per QElectroTech pixel; 0.5 puts its 10 px grid on a 5 mm grid (default 0.5) |
| `--category <CATEGORY>` | Library category to file the symbols under (default empty) |
| `--letter-code <LETTER_CODE>` | Letter code for new devices. Left out, each element keeps the letter code read from its label (`-K1` gives `K`) |

Writes one symbol per file, `<lib>/symbols/<prefix>.<file name>.json`; the first terminal becomes
the origin. Two files with the same name in different folders give the same id: the first is
written and the later one is reported (`gives the same id ...`) and skipped. A file that cannot be
read is reported and skipped, the others are still written,
and the command then fails with `<n> of <m> element(s) could not be imported`.

```sh
wiringcraft import symbol-qet relay.elmt lamp.elmt --lib ../mylib
```

## Parts into a library

### parts-csv

```
wiringcraft import parts-csv [OPTIONS] --lib <LIB> <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | A CSV or semicolon-separated file, or an Excel workbook (`.xlsx`, its first visible sheet) |
| `--lib <LIB>` | The library folder to write into |

Writes one `<lib>/parts/<id>.json` per row, the id made from the maker and order number
(`Example;EX-CT-9-24DC` gives `example.ex-ct-9-24dc`); a part file with the same id is written
over. Columns are matched by name in several languages: maker (`Manufacturer`, `Hersteller`,
...), order number (`Order number`, `Part number`, `Article`, `Type`, ...), `Description`,
`Symbol`, and contact counts (`Main contacts`, `NO`, `NC`, `CO`, or `Contacts` written out as
`3NO+1NC`). Any other column becomes a part property. A file with no order or part number column
is refused. Warns about and skips a row with no order number; a duplicate order number warns
that the later row wins. An old `.xls` workbook is refused; save it as `.xlsx`.

```sh
wiringcraft import parts-csv examples/parts.csv --lib ../mylib
```

### parts-bmecat

```
wiringcraft import parts-bmecat [OPTIONS] --lib <LIB> <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | The BMEcat XML file (version 1.2 or 2005) |
| `--lib <LIB>` | The library folder to write into |

Writes one `<lib>/parts/<id>.json` per product: order number, maker (or the catalogue's
supplier), description, and as properties the maker's part number, GTIN and ECLASS class and
features. A file that is not UTF-8 is read as Latin-1.

```sh
wiringcraft import parts-bmecat catalogue.xml --lib ../mylib
```

### parts-edz

```
wiringcraft import parts-edz [OPTIONS] --lib <LIB> <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | The `.edz` file (EPLAN Data Exchange archive, 7-Zip or zip) |
| `--lib <LIB>` | The library folder to write into |
| `--symbol-map <SYMBOL_MAP>` | A text file with one `macro = symbol.id` per line: which library symbol to give parts by their macro. The macro is matched by its full name, its file name or its file name without `.ema`, ignoring case |

Writes the macros' symbols first (`<lib>/symbols/eplan.<maker>.<macro name>.json`, from each
macro's multi-line representation, variant A), then one part per EPLAN part, linked to the symbol
of the macro it names. A symbol that cannot be written is warned about and its parts are written
without it. The symbol map is read before anything is written, so a bad map stops the import.
Prints a report of what the macros leave out and counts pictures. A file that is neither 7-Zip
nor zip is refused (`not an EDZ archive`).

```sh
wiringcraft import parts-edz maker.edz --lib ../mylib --symbol-map edz-map.txt
```

## Part footprints and models

### footprint-dxf

```
wiringcraft import footprint-dxf [OPTIONS] --part <PART> <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | The DXF file: the maker's front view, in mm at 1:1 |
| `--part <PART>` | The part's library id |
| `--lib <LIB>` | The library folder that holds the part (default: the project's library source that has it) |

Writes the footprint into the part's library file (`<lib>/parts/<part>.json`), where cabinet
layouts draw it instead of a box. A part without `Width` and `Height` takes its size from the
drawing. Without `--lib`, the part is looked up in the project's library sources (the source the
project takes it from), and that source's file is changed; a part no source has is refused with
`name its folder with --lib`. When the project uses the part, its pinned copy in the project is
updated too, unless that copy has local changes or cannot be read, or the project takes the part
from another source; a warning says so. Naming the project's own pinned copy with `--lib` is
refused. With `--lib`, it also works outside a project.

```sh
wiringcraft import footprint-dxf contactor-front.dxf --part example.ex-ct-9-24dc --lib ../mylib
```

### model-step

```
wiringcraft import model-step [OPTIONS] --part <PART> <FILE>
```

The same options as `footprint-dxf`, with `<FILE>` a STEP file. The model's bounding box is
written into the part's library file as its 3D model size, for cabinet layouts and their 3D
export. The plate size comes from the part's `Width` and `Height` first, then its footprint
drawing, and only then the model; the depth from `Depth`, then the model. The model's x axis runs
across the plate, y up it and z out of it. Warns when the file names no length unit (millimetres
are assumed). The pinned copy is updated, or not, as for `footprint-dxf`.

```sh
wiringcraft import model-step contactor.step --part example.ex-ct-9-24dc --lib ../mylib
```

## Data onto the project's devices

### plc-csv

```
wiringcraft import plc-csv <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | A CSV file or an Excel workbook (`.xlsx`) |

The list needs a device column (`Device`, `Tag`, `BMK`, ...) and an address column (`Address`,
`PLC address`, `I/O`, `Operand`, ...); a description column (`Description`, `Comment`, ...) is
optional. Each row sets the device's `Address` property (or its existing `PLC address` property)
and, when the row has one, its description; an empty address clears it. A tag that no symbol in
the project carries is warned about and skipped, not added. The project is saved only when
something changed. A list written by `wiringcraft report plc --format csv` (or `xlsx`) reads back.

```sh
wiringcraft import plc-csv plc-addresses.csv
```

### wago

```
wiringcraft import wago <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | The CSV file from the configurator |

Reads back a list written by `wiringcraft export wago-configurator`, with the articles and
accessories chosen in the configurator. Each row is matched by its Index to the terminal or strip
device it stands for, and its article to a part of the project or its libraries (by order
number, or part id); matched articles become the devices' parts and accessories, and a jumper row's
article becomes the part of that jumper. A row the configurator added without an Index is read as
a new accessory of the strip its Strip column names, when the project has that strip. Rows that cannot
be matched are listed as `not matched: line <n> (<index>): ...` with the reason, such as an
article no library has (import the maker's catalogue first) or a row the project no longer has
(export the list again). Unmatched rows do not make the command fail.

```sh
wiringcraft import wago export/wago-configurator.csv
```

See also [WAGO configurator export](../wago-configurator.md).

## Drawings onto pages

### page-dxf

```
wiringcraft import page-dxf [OPTIONS] --page <PAGE> <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | The DXF file |
| `--page <PAGE>` | The id of the page to draw onto; it must exist |
| `--scale <SCALE>` | Multiply the drawing's coordinates by this to get mm (default 1.0) |

Adds the drawing's shapes to the page. Blocks named like a symbol of the project's libraries
become symbols, and those library symbols are taken into the project. Refuses a page that does
not exist (`no page "<id>"`) and a scale that is not a positive number.

```sh
wiringcraft page add p0010 --number 10 --title "Old drawing"
wiringcraft import page-dxf old-drawing.dxf --page p0010
```

### page-dwg

```
wiringcraft import page-dwg [OPTIONS] --page <PAGE> <FILE>
```

The same options as `page-dxf`, with `<FILE>` a DWG file. WiringCraft's own DWG reader (AutoCAD
2004 to 2018) reads the file. Only when it refuses the file is it converted to DXF by an
installed converter: the command in `WIRINGCRAFT_DWG2DXF` (with `{in}` and `{out}`), else
`dwg2dxf`, else the ODA File Converter. The output says which one read the file. With no
converter, the command fails with the reader's reason and `no DWG converter found`.

```sh
WIRINGCRAFT_DWG2DXF="dwg2dxf -y -o {out} {in}" wiringcraft import page-dwg old-drawing.dwg --page p0010
```

### pdf

```
wiringcraft import pdf [OPTIONS] <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | The PDF file |
| `--first-page <FIRST_PAGE>` | Number of the first new page; the others follow it (default 200) |

Adds one new page per PDF page, id `pdf-<number>`, titled `<file name> p.<n>`, with the page's
vector content as trace graphics, on the smallest ISO sheet that holds it. A scanned page is
saved as a picture in the project and shown under the page. Warns about a page with no vector
content and no usable scan.

```sh
wiringcraft import pdf old-schematic.pdf --first-page 500
```

## Projects from other tools

### aml

```
wiringcraft import aml [OPTIONS] <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | The AutomationML (`.aml`) file |
| `--first-page <FIRST_PAGE>` | Number of the first new page; the others follow it (default 100) |

Adds the AutomationML (CAEX) devices with their properties and connections, laid out as
connection overview pages, since the file carries no drawing.

```sh
wiringcraft import aml plant.aml
```

### eplan

```
wiringcraft import eplan [OPTIONS] <PATH>
```

| Option | Meaning |
|---|---|
| `<PATH>` | The `.elk` file, its `.edb` folder, or a zip of the folder |
| `--first-page <FIRST_PAGE>` | Number of the first new page; the others follow it (default 400) |

Adds the Eplan Electric P8 project's pages as drawn, with devices at their original positions
and wires traced, plus connection overview pages for devices, parts and connections that are on
no drawn page. The project's customer and description fill empty title block fields. An `.elk`
file names its `.edb` folder in its first line, and that folder must sit beside it.

```sh
wiringcraft import eplan Plant.elk
```

### swe

```
wiringcraft import swe [OPTIONS] <PATH>
```

| Option | Meaning |
|---|---|
| `<PATH>` | The `.tewzip` archive or `tew_project_data.sqlite` file |
| `--pick <PICK>` | Which project of the archive, counted from 1 (default 1) |
| `--first-page <FIRST_PAGE>` | Number the pages from here instead of keeping the source's page numbers |
| `--symbol-map <SYMBOL_MAP>` | A text file with one `block name = symbol.id` per line: which library symbol to draw for which source block |

Adds the SolidWorks Electrical project's schematic and line-diagram pages, their texts and
frames, symbols drawn from their DWG blocks with the connection points at the original positions,
wires joined to the symbols, and devices with part data, linked to the archive catalog's part
when it has one. Without `--symbol-map`, blocks whose pins match a built-in symbol (coil A1/A2,
contacts 13/14 and 11/12, lamp X1/X2, terminal 1/2) use it when the pins sit where the block's
points are; the rest are drawn as boxes, and always for a bare database. When the archive holds
several projects, it says so and imports the one `--pick` names. Prints a table of what the
source had, what was imported and why the rest was not.

```sh
wiringcraft import swe Plant.tewzip --pick 2
```

### swe-library

```
wiringcraft import swe-library [OPTIONS] <PATH>
```

| Option | Meaning |
|---|---|
| `<PATH>` | The `.tewzip` archive |
| `--lib <LIB>` | A library folder to write into instead of the project |

Reads the archive's catalog parts with their terminals, cable types with their cores, and symbol
blocks with their connection points (drawn from the archive's block DWG files, boxes where a
block has none). With `--lib` they are written into that folder; without it they are vendored
into the project. A symbol or part the project already vendors is replaced by the new version
when its copy is still the one the project pins; a copy changed in the project is refused, not
replaced. To take the new version into such a project, import with `--lib`, add that folder as a
source (`wiringcraft lib add`) and run `wiringcraft lib update <id>`. `lib add` puts the new
source last and earlier sources win: when another source has the same id, first move the new
source's `[[library]]` table above that source's in `project.toml`.

```sh
wiringcraft import swe-library ContentPortal.tewzip --lib ../swe-lib
```

### archive

```
wiringcraft import archive [OPTIONS] <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | The archive file (zip) |
| `--first-page <FIRST_PAGE>` | Number of the first new page; the others follow it (default 300) |
| `--lib <LIB>` | Library folder for the symbols of `.kicad_sym` entries and the parts of CSV parts lists |

Imports the open-format entries of another tool's project archive, each as its own command would:

- PDF entries as trace pages (`pdf`), DXF entries each on a new page `dxf-<number>` (`page-dxf`),
  AutomationML entries as overview pages (`aml`), numbered on from `--first-page`;
- `.kicad_sym` entries into `--lib` with the prefix `kicad.<entry name>` (`symbol-kicad`);
- CSV and Excel (`.xlsx`) lists last, told apart by their header: a device and an address column
  make a PLC address list (`plc-csv`), an order or part number column a parts list for `--lib`
  (`parts-csv`).

Without `--lib`, KiCad libraries and parts lists are listed with a warning, not imported. An
entry that fails is warned about and the others still come in; a DXF that fails leaves no empty
page behind (if removing that page fails, a warning says `the empty page <id> could not be
removed`; delete it with `wiringcraft page remove`). Old `.xls` workbooks are skipped with a
warning (save them as `.xlsx`), and a list that is neither kind is skipped. An archive with nothing importable is refused; run
`wiringcraft inspect` to see what is there.

```sh
wiringcraft import archive handover.zip --lib ../mylib
```

## inspect

```
wiringcraft inspect <FILE>
```

| Option | Meaning |
|---|---|
| `<FILE>` | The archive file (.tewzip, .zw1, .elk, .zip) |

Lists what is inside a project archive from another tool, without changing anything: the
container kind, then each entry grouped by what it is (PDF, DXF, DWG, AutomationML, KiCad symbol
library, CSV or Excel list, old `.xls` workbook, XML, databases, images, nested archives, other)
with its size, and how many entries `wiringcraft import archive` can import. Only zip containers
are listed entry by entry; for a 7z, gzip or other file it names the container and lists no
entries. It does not need a project.

```sh
wiringcraft inspect handover.zip
```

Output looks like:

```
container: zip
PDF (importable as trace pages):
         16728  out.pdf
DXF drawing (importable):
           143  coil.dxf
CSV list (importable: parts list into a library, or PLC addresses):
           570  parts.csv
old Excel workbook .xls (not read; save it as .xlsx):
             5  old.xls
3 of 4 entries can be imported with `wiringcraft import archive`
```
