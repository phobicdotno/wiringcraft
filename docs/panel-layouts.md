# Panel layouts

A panel layout is a drawing of a cabinet's mounting plate: the plate itself, the DIN rails and
wire ducts on it, and the devices from the schematic placed on the rails. It is drawn to scale
in millimetres, so it shows whether everything fits before the cabinet is built.

This page explains how layouts work. For every command and option, see the command reference
in [docs/cli/panel-layouts.md](cli/panel-layouts.md).

## What a layout holds

A project can have any number of layouts. Each one is a file `panels/<id>.panel.json` in the
project folder, with:

- an id (the file name) and a name, shown on its page;
- the plate size, width and height in mm;
- a list of items: rails, ducts and devices.

Positions are in millimetres from the plate's top-left corner, with y counting down.

In the app, "Panel layout" adds a layout and "Arrange devices" fills it. On the plate you drag
devices, rails and ducts to move them, drag a rail's right end or a duct's bottom-right corner
to change its size, and press Delete to take an item off.

## Rails and ducts

A rail is a 35 mm DIN rail. It is stored by its left end on its centre line and its length to
the right. A duct is a rectangle, 40 mm wide unless you give another width, running across the
plate or down it.

A new layout (`panel new`, or "Panel layout" in the app, 600 x 800 mm by default) starts with
the usual arrangement: ducts around the edge, and the height split into rows of at least
125 mm with a duct between rows and a rail in the middle of each row. You can add and remove
rails and ducts afterwards, and change the plate size. A new size that would leave a rail, duct
or device sticking out of the plate is refused.

## Devices come from the schematic

Devices on a plate are named by their device tag, the same tag the schematic and the device
list use (`+CAB1-K1`). A layout does not copy the device: it only says where that tag sits on
the plate, its size and its turn (0, 90, 180 or 270 degrees clockwise).

Arranging (`panel arrange`, or "Arrange devices") goes through the project's devices in tag
order and puts each one on the rails, left to right and then rail by rail, centred on its rail
with 1 mm between neighbours. It takes a device when:

- it is drawn on a page or listed in the devices file;
- it has a part (set with `wiringcraft device <tag> --part <id>`, or in the app);
- its part is not door-mounted (a `Mounting` property of `door` or `front`, as for push buttons
  and pilot lights);
- it is not on this plate yet.

`--location +CAB1` keeps only the devices of one location, so each cabinet can have its own
layout. Devices already on the plate stay where they are, so you can arrange again after
adding devices to the schematic. A device that fits on no rail is listed as not fitting.

You can also put one device on the plate yourself, move it or turn it (`panel device`). That
works for a device without a part too.

## Sizes come from part data

A device's size on the plate comes from its part, in this order for each of width and height:

1. the part's `Width` and `Height` properties (a value like `18 mm` or `17,5`);
2. the part's footprint drawing, a maker's DXF outline added with `import footprint-dxf`;
3. the size of the part's 3D model, added with `import model-step`.

A part with none of these is drawn as one 18 mm DIN module, 90 mm high, with a warning. When
the part has a footprint drawing, the layout draws the maker's outline instead of a plain box.

The size is stored in the layout when the device is placed. Placing the device again with
`panel device <id> <tag>` (no `--at`) keeps its place and takes the size its part has now.

For example, with the starter library's example parts on a 400 x 500 mm plate:

```
$ wiringcraft panel new plate1 --name "Main plate" --width 400 --height 500
panel plate1: 400 x 500 mm, 2 rail(s)
$ wiringcraft panel arrange plate1
placed 2 device(s): +CAB1-K1, +CAB1-Q1
$ wiringcraft panel show plate1
plate1 Main plate (400 x 500 mm)
duct 1       at 0,0  40 x 500 mm
duct 2       at 360,0  40 x 500 mm
duct 3       at 40,0  320 x 40 mm
rail 1       at 45,135  310 mm long
duct 4       at 40,230  320 x 40 mm
rail 2       at 45,365  310 mm long
duct 5       at 40,460  320 x 40 mm
+CAB1-K1     at 45,92.5  45 x 85 mm  turned 0
+CAB1-Q1     at 91,90  18 x 90 mm  turned 0
```

The project also had a push button `+CAB1-S2` (its part is door-mounted) and a lamp `+CAB1-P1`
(no part yet); arranging left both off the plate.

## How it ties to the device list and the checks

Because the plate names devices by tag, the device list (`report devices`), the parts list and
the layouts all describe the same devices. Renaming a device (for example with
`find --replace`) renames it on the plates too. Changing a device's part does not resize it on
a plate by itself; place it again as described above.

`wiringcraft check` warns about layouts where:

- two devices overlap (outlines that only touch are fine);
- a device, rail or duct sticks out of the plate;
- a device on the plate is no longer in the project (drawn on no page and not in the devices
  file).

## P sheets in the PDF

`wiringcraft pdf` prints every layout after the schematic pages, one sheet each, numbered
P1, P2 and so on in order of layout id. The contents report lists them by that number and the
layout's name. Each sheet has the frame and title block of the first schematic page and the
same paper size, with the plate drawn at a standard scale that fits (1:2, 1:2.5, 1:5, 1:10 and
so on). The caption gives the name, plate size, scale and device count. Devices are labelled by
their product part (`-K1`), or by the full tag when two devices on the plate share it
(`+CAB1-K1` and `+CAB2-K1`). PLC card sheets, when printed, come after the P sheets.

A layout can also leave WiringCraft as a drawing or a model:

- `wiringcraft drawing <folder>` writes one SVG or DXF file per page and one
  `panel-<name>` file per layout;
- `wiringcraft panel step <id> <file>` writes a 3D STEP model, with device depths from the
  part's `Depth` property or its 3D model.
