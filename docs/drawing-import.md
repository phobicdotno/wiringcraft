# Importing drawings: DXF, DWG, SVG and PDF

What WiringCraft takes from a DXF, DWG, SVG or PDF file, what it approximates, and what it
leaves out. The command line options of these imports are listed in
[the command line imports page](cli/imports.md).

A drawing comes in as plain graphics: lines, circles, arcs, polylines and texts. The import does
not find wires, devices or connections in it, with one exception: a DXF or DWG block named like a
library symbol becomes that symbol (see [Symbols on the page](#symbols-on-the-page)).

| Command | In the app | Reads | Into |
|---|---|---|---|
| `import page-dxf` | **File > DXF drawing** | ASCII DXF | graphics on an existing page |
| `import page-dwg` | **File > DWG drawing** | DWG, AutoCAD 2004 to 2018 | graphics on an existing page |
| `import pdf` | **File > PDF pages** | PDF | new pages, one per PDF page |
| `import symbol-dxf`, `import symbol-svg` | **Library > Import symbols** | DXF, SVG | a library symbol |
| `import footprint-dxf` | | DXF | a part's footprint for cabinet layouts |

The page imports are also on the **Import** ribbon tab. `import archive` reads the PDF and DXF
files inside another tool's project archive the same way.

## DXF and DWG onto a page

```sh
wiringcraft page add p2 --number 2
wiringcraft import page-dxf plan.dxf --page p2
```

The page must exist. The drawing is added to what the page already has, and moved so its
top-left corner sits 20 mm in from the top-left of the sheet. Coordinates are taken as
millimetres; `--scale` multiplies them (`--scale 25.4` for a drawing in inches). The units the
file names in its header are not read.

### What it reads

From the `ENTITIES` and `BLOCKS` sections:

- LINE, CIRCLE, ARC, LWPOLYLINE and POLYLINE (an arc segment of a polyline becomes points
  along the arc);
- ELLIPSE and SPLINE, as polylines through points along the curve;
- HATCH: a solid hatch becomes a filled outline, with its holes cut out; a patterned hatch
  becomes its outline only;
- SOLID and TRACE, as filled outlines;
- TEXT, MTEXT and ATTRIB, with their position, height, alignment and turn. MTEXT formatting
  (fonts, colours, stacked fractions) is dropped and only the words are kept. `%%c`, `%%d` and
  `%%p` become ⌀, ° and ±;
- LEADER, with its arrow head;
- INSERT (a block reference), drawn as the block's contents, turned, scaled, mirrored and as an
  array as placed. Blocks inside blocks are followed 8 levels deep;
- DIMENSION, drawn by the block the file keeps for it.

A binary DXF is refused (`save as ASCII DXF`).

### What it leaves out

- Layers, colours, line types and line widths: everything is drawn in the page's own style. A
  layer that is turned off or frozen in the CAD program is drawn too.
- Paper space and model space are not told apart.
- Any other entity kind (3DFACE, IMAGE, REGION, ...). The import warns once per kind:
  `warning: 1 3DFACE entity not read`.
- Invisible attributes, and attribute templates (ATTDEF) of a block.
- A block reference whose block is not in the file, such as an external reference
  (`1 block insert not drawn: its block is not in the file (NAME)`).
- Entities more than 1000 km from the rest of the drawing.

When nothing is left to draw, the import stops with `the drawing has no entities WiringCraft can
read` and the reasons.

### Symbols on the page

A block reference whose block name matches the id of a symbol in the project's libraries
(ignoring case; a space or `/` counts as `-`) is placed as that symbol, not drawn as lines. Its
device tag comes from an attribute named `TAG`, `DEVICE`, `BMK` or `REF`. The symbol is copied
into the project as for any placed symbol. Only right-angle turns are kept: a block turned by
another angle is placed unturned. A negative X scale places the symbol mirrored; a negative Y
scale alone does not mirror it, and the scale itself is not kept, so the symbol has its
library size.

```
$ wiringcraft import page-dxf plan.dxf --page p2
warning: 1 3DFACE entity not read
warning: 1 IMAGE entity not read
imported 5 shapes and 1 symbols onto page p2
```

### DWG

`import page-dwg` reads the file with WiringCraft's own DWG reader (AutoCAD 2004 to 2018, the
formats `AC1018` to `AC1032`) and then draws it exactly as a DXF: the same entities, the same
symbols, the same warnings. Multi-line attributes, custom entities and objects the reader cannot
decode are counted in the warnings. Password-protected parts of a file are not read. When the
reader refuses the file, it is converted to DXF by an installed converter (ODA File Converter,
`dwg2dxf`, or the command in `WIRINGCRAFT_DWG2DXF`), and the import says which one read it.

## PDF pages

```sh
wiringcraft import pdf old-schematic.pdf --first-page 200
```

Each PDF page becomes a new page with the id `pdf-<number>`, titled `<file name> p.<n>`, on the
smallest ISO sheet that holds it (a larger drawing gets a sheet of its own size). The pages are
for tracing: redraw the schematic over them with real symbols and wires, or keep them as
reference.

From each page it reads:

- stroked lines and curves, as lines and polylines (curves become short straight pieces);
- small filled shapes of 3 mm or less, such as junction dots and arrow heads, as outlines;
- text in simple fonts, and in composite fonts that carry a ToUnicode map (as EPLAN and most
  CAD programs write them), with its size and turn;
- the contents of form XObjects, and the page's `/Rotate`, so the page is drawn as displayed.

It leaves out colours, line widths and dashes, larger filled areas (backgrounds), anything drawn
in white, clipping (clipped content is drawn whole), annotations and form fields, and text in a
font it cannot map. All text is left-aligned. An encrypted PDF is refused.

A scanned page has no vector content. Its largest picture that covers at least 30% of the page
is saved as `underlays/pdf-<number>.png` (or `.jpg`) and shown under the page, so you can trace
over it. Pictures compressed as Flate, LZW, RunLength, JPEG, JPEG 2000, CCITT fax or JBIG2 are
read. A page with neither gives `no vector content and no usable scan` and the reason.

## Symbols from DXF and SVG

`import symbol-dxf` and `import symbol-svg` turn one drawing into one library symbol. The
drawing's graphics become the symbol's graphics, its pins are found as below, and the first pin
is the symbol's origin. Each pin points out from the side of the symbol it is nearest to.

DXF: pins are POINT entities, and circles on a layer named `PIN`, `PINS`, `WC_PIN`, `CONNECTION`
or `CONNECTIONS` (in capitals). A text on such a layer within 6 mm of a pin names it; other pins
are numbered 1, 2, ... Pins are ordered top to bottom, then left to right. Everything else on a
pin layer is left out. The entities read and left out are those of a page import.

SVG: pins are elements with `id="pin:NAME"`, `data-pin="NAME"` or `class="pin"` (numbered by
their place); they are not drawn. One SVG unit is one millimetre, unless the root `<svg>` gives
a width in mm, cm, Q, in, pt or pc together with a `viewBox`. Pins keep the order of the file. It
reads `line`, `rect`, `circle`, `ellipse`, `polyline`, `polygon`, `path`, `text` and `tspan`,
with their transforms, and fills that are not white or none. It leaves out stroke colours and
widths, `<use>`, `<image>`, anything inside `<defs>`, `<symbol>`, `<marker>`, `<clipPath>`,
`<mask>` or `<pattern>`, and hidden elements. A path it can draw only in part is named in a warning.

## Part footprints

`import footprint-dxf` reads a maker's outline drawing (the front view, in mm at 1:1) as a
part's footprint for cabinet layouts. It reads the same entities as a page import and keeps all
of them, moved so the drawing's top-left corner is at 0, 0; the drawing's extents give the
footprint's width and height. Block references are always drawn, never placed as symbols.
