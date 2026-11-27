# Panel layouts on the command line

A panel layout is a cabinet mounting plate with its DIN rails, wire ducts and the device
outlines on it. `wiringcraft panel` makes and edits layouts. Each one is stored as
`panels/<id>.panel.json` in the project folder. Layouts print after the schematic pages in the
PDF (`wiringcraft pdf`), `wiringcraft drawing` writes one SVG or DXF file per layout
(`panel-<name>.svg` or `.dxf`; not when pages are picked with `--page`), and `wiringcraft check` warns about devices that overlap, stick out of the
plate or are no longer in the project.

All sizes and positions are in mm. `x,y` is measured from the plate's top-left corner, with y
running down the plate.

## Commands

| Command | What it does |
|---|---|
| `panel new <ID> [--name N] [--width W] [--height H]` | A new plate, 600 x 800 mm by default (200 to 5000 mm each way), with ducts around the edge and a 35 mm DIN rail in every 125 mm row between horizontal ducts. A height below 205 mm gets no ducts or rails. The name shows on the layout's page; left out, the id is used |
| `panel edit <ID> [--name N] [--width W] [--height H]` | Rename the layout or change the plate size. Rails, ducts and devices stay where they are; a size that would leave any of them off the plate is refused |
| `panel show <ID>` | The plate, its numbered rails and ducts, and each device with its position, size and turn |
| `panel list` | Every layout, its plate size and how many devices are on it |
| `panel rail <ID> --at X,Y --length L` | Add a rail. `--at` is its left end on its centre line; it runs `L` mm to the right |
| `panel rail <ID> --remove N` | Take off rail `N`, numbered as `panel show` lists them |
| `panel duct <ID> --at X,Y --length L [--width W] [--vertical]` | Add a duct. `--at` is its top-left corner. It runs to the right, or down with `--vertical`; `--width` is across the length (default 40) |
| `panel duct <ID> --remove N` | Take off duct `N`, numbered as `panel show` lists them |
| `panel arrange <ID> [--location +LOC]` | Put every device that has a part and is not on the plate yet onto the rails, in tag order, left to right and then rail by rail, centred on the rail. `--location` keeps only devices in that location |
| `panel device <ID> <TAG> --at X,Y [--rotate 0\|90\|180\|270]` | Put one device on the plate, or move or turn it. `--at` is the top-left corner of its (turned) outline. A device already on the plate keeps its place or turn when that option is left out |
| `panel device <ID> <TAG> --remove` | Take the device off the plate |
| `panel step <ID> <OUT>` | Write the layout as a 3D STEP model (plate, rails, ducts and devices as solids) |
| `panel remove <ID>` | Delete the layout |

Add `-C <dir>` to work on a project other than the current folder. `OUT` for `panel step` is
relative to the current folder, not the project.

### Device sizes

A device's outline comes from its part, in this order:

1. The part's `Width` and `Height` properties (a value such as `45 mm`).
2. The part's footprint drawing (`import footprint-dxf`).
3. The part's 3D model (`import model-step`).

A part with none of these is drawn 18 x 90 mm, and `arrange` and `device` print a warning that
names it. `panel show` prints the part's size before turning, with the turn after it.

`panel step` takes each device's depth from the part's `Depth` property, then its 3D model. A
part with neither is drawn 75 mm deep, with a warning.

`arrange` leaves out parts whose `Mounting` property is `Door` or `Front` (push buttons, pilot
lights); place those with `panel device` if they belong on the plate. A device that fits on no
rail is named in a warning.

## Worked example

This builds a 400 x 500 mm plate for a contactor, a circuit breaker and a pilot light, using the
example parts in the repository's `library` folder.

```sh
wiringcraft new demo
cd demo
wiringcraft lib add starter folder /path/to/wiringcraft/library
wiringcraft place p0001 wc.coil --at 60,80 --tag -K1
wiringcraft place p0001 wc.breaker.1p --at 100,80 --tag -F1
wiringcraft place p0001 wc.lamp --at 140,80 --tag -P1
wiringcraft device -K1 --part example.ex-ct-9-24dc
wiringcraft device -F1 --part example.ex-mcb-1p-c10
wiringcraft device -P1 --part example.ex-lamp-24-gn
```

Make the plate and look at what it starts with:

```console
$ wiringcraft panel new plate1 --name "Main plate" --width 400 --height 500
panel plate1: 400 x 500 mm, 2 rail(s)
$ wiringcraft panel show plate1
plate1 Main plate (400 x 500 mm)
duct 1       at 0,0  40 x 500 mm
duct 2       at 360,0  40 x 500 mm
duct 3       at 40,0  320 x 40 mm
rail 1       at 45,135  310 mm long
duct 4       at 40,230  320 x 40 mm
rail 2       at 45,365  310 mm long
duct 5       at 40,460  320 x 40 mm
```

Put the devices on the rails. `-P1` is a door-mounted part, so it is left out:

```console
$ wiringcraft panel arrange plate1
placed 2 device(s): -F1, -K1
```

Split the lower rail in two with a vertical duct between the halves:

```console
$ wiringcraft panel rail plate1 --remove 2
removed rail 2 from plate1
$ wiringcraft panel rail plate1 --at 45,365 --length 130
rail on plate1 at 45,365, 130 mm long
$ wiringcraft panel rail plate1 --at 215,365 --length 140
rail on plate1 at 215,365, 140 mm long
$ wiringcraft panel duct plate1 --at 180,270 --length 190 --width 30 --vertical
duct on plate1 at 180,270, 30 x 190 mm
```

Move the contactor onto the right-hand rail. Its outline is 85 mm high, so a top edge at
365 - 42.5 = 322.5 centres it on the rail:

```console
$ wiringcraft panel device plate1 -K1 --at 220,322.5
-K1 on plate1 at 220,322.5 (45 x 85 mm, turned 0)
$ wiringcraft panel show plate1
plate1 Main plate (400 x 500 mm)
duct 1       at 0,0  40 x 500 mm
duct 2       at 360,0  40 x 500 mm
duct 3       at 40,0  320 x 40 mm
rail 1       at 45,135  310 mm long
duct 4       at 40,230  320 x 40 mm
duct 5       at 40,460  320 x 40 mm
rail 2       at 45,365  130 mm long
rail 3       at 215,365  140 mm long
duct 6       at 180,270  30 x 190 mm
-F1          at 45,90  18 x 90 mm  turned 0
-K1          at 220,322.5  45 x 85 mm  turned 0
```

Removing a rail or duct renumbers the ones after it, so run `panel show` before the next
`--remove`. Last, write the 3D model and check the project:

```console
$ wiringcraft panel step plate1 plate1.step
wrote plate1.step (12 solids)
$ wiringcraft check
0 errors, 0 warnings, 3 notes
```

## Part outlines and 3D models

Two imports give a part a size and a drawing for layouts. Both store the result in the library
with the part (`--part` is the part's library id), and update the project's pinned copy when the
project uses the part, unless that copy has local changes or the project takes the part from
another source (a warning says so). `--lib <folder>` names the library folder that holds the
part; by default it is the project's library source that has it.

```sh
wiringcraft import footprint-dxf mcb-front.dxf --part example.ex-mcb-1p-c10
```

`import footprint-dxf` reads a maker's outline drawing: a DXF of the front view in mm at 1:1. It is
drawn on layout pages in place of a plain box, and its extents give each dimension that the part's
`Width` or `Height` does not set. A part with only `Width` takes its height from the drawing. It
prints the size and the number of shapes read.

```sh
wiringcraft import model-step contactor.step --part example.ex-ct-9-24dc
```

`import model-step` reads a maker's 3D model (STEP) and keeps its bounding box as the part's
size. The model's x axis runs across the plate, y up it and z out of it. The plate size comes
from `Width` and `Height` first, then the footprint drawing, and only then the model; the depth
for `panel step` from `Depth`, then the model.

After either import, devices already on a plate keep the size they were placed with. Run
`panel device <ID> <TAG>` with no other options to give one the part's new size; it keeps its
place and turn.
