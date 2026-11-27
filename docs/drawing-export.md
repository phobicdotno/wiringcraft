# Drawing export (SVG and DXF)

Besides the PDF, WiringCraft can write each page as a drawing file for other tools: SVG for
browsers, Inkscape and documentation, DXF for CAD programs. These are the only two formats;
there is no PNG or DWG export. The PDF itself is described in [pdf-output.md](pdf-output.md),
and every option of the command in [cli/reports-and-export.md](cli/reports-and-export.md).

## What is in the files

Each file is one sheet, drawn from the same layout as its PDF page: the frame with its
reference columns, the title block, symbols, black boxes, wires, wire numbers and
cross-references. Report tables (parts list, terminal plan and so on) are not drawn; they are
their own export (`report`).

After the schematic pages comes one file per panel layout, drawn as its PDF sheet: the plate,
DIN rails, wire ducts and device footprints.

Files are named after the page number, not the page id:

| Sheet | File name |
|---|---|
| Page numbered `2` | `2.svg`, `2.dxf` |
| Page numbered `A/1` | `A_1.svg` (characters other than letters, digits, `-`, `_` and `.` become `_`) |
| Panel layout named `Main cabinet` | `panel-Main_cabinet.svg` |

A page with no number is named by its id. A name Windows reserves (`CON`, `NUL`, `COM1`) gets a
`_` in front. When two sheets would get the same name (compared without case, as Windows and
macOS do), the later one gets a `_` added.

## Units and coordinates

Both formats are in millimetres at 1:1, with the sheet's own size (an A3 page is 420 x 297).

- **SVG:** `width` and `height` are given in `mm`, and one user unit is one millimetre. The
  origin is the top left corner and y runs down, as on the page in the app.
- **DXF:** the origin is the bottom left corner of the sheet and y runs up, as CAD programs
  expect. A point at `50,50` in WiringCraft (from the top) is at `50,247` in the DXF of an A3
  page. The header sets the drawing extents to the sheet size.

## Layers

Most of the drawing (frame, title block, symbols, wires) is on the base layer. Three groups
that the PDF also puts on layers of their own get separate layers, so they can be hidden:

| Group | DXF layer | SVG layer (Inkscape label) |
|---|---|---|
| Free drawings and texts, revision clouds and the revision table | `DRAWINGS` | Drawings and notes |
| Wire numbers, potential names and cable cores | `WIRE-NUMBERS` | Wire numbers |
| Cross-references | `CROSS-REFERENCES` | Cross-references |
| Scanned underlay (SVG only) | | Scanned sheets |

In the DXF the base layer is `0`. In the SVG the layers are `<g>` groups that Inkscape shows as
layers; a browser just draws them. A layer appears only when the sheet has something on it.

## Texts and fonts

- **SVG:** texts use the font family `Helvetica, Arial, sans-serif`, so the viewer picks a
  matching font it has. Leading and repeated spaces are kept.
- **DXF:** texts are `TEXT` entities, drawn in the CAD program's own font. That font can be
  wider or narrower than the PDF's, so a centred or right-aligned text keeps its alignment point
  and the program places it from there. Characters outside ASCII are written as `\U+XXXX`
  codes, which CAD programs turn back into text. A character beyond U+FFFF (some emoji and rare
  scripts) has no such code and is written as `?`, with a warning naming the page and the text.

`--lang` draws the texts in a language from `translations.toml`, as `pdf --lang` does.

## Colours, line widths and pictures

- **SVG** keeps the exact colours and line widths, and embeds the project logo and any scanned
  underlay as PNG or JPEG pictures inside the file.
- **DXF** is the R12 ASCII format. Colours become the nearest AutoCAD colour index (black is
  colour 7, which a CAD program shows white on a dark background). Lines wider than the default
  0.25 mm pen are polylines that carry their width; thinner lines are plain lines. Filled shapes
  are `SOLID` entities. DXF has no pictures: the logo and underlays are left out, with a warning.

## From the command line

`drawing` writes one format per run into a folder, which it creates. SVG is the default:

```
$ wiringcraft drawing out
wrote 3 svg file(s) to out
$ wiringcraft drawing out --format dxf
wrote 3 dxf file(s) to out
$ ls out
1.dxf  1.svg  2.dxf  2.svg  panel-Main_cabinet.dxf  panel-Main_cabinet.svg
```

`--page` picks the schematic pages (by id) and leaves the panel layouts out; without it every
page and every panel layout is written. An
unknown page id is an error and nothing is written. `--date` fills the title block's date where
the page and project set none. Files with the same name in the folder are replaced, and files
from earlier runs that no longer match a sheet stay.

## From the app

**File > SVG/DXF** asks for a folder (`export/drawings` in the project folder by default) and
writes every page and panel layout in both formats at once. A new export into
`export/drawings` replaces the last one. In any other folder nothing is replaced: if the folder
already holds a file the export would write, the export stops and asks for an empty folder. The
assistant's export tool writes the same files to `export/drawings`.
