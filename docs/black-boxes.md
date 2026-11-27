# Black boxes

A black box (device box) is a plain rectangle with connection points on its edges. It stands
in for a device that has no symbol in your libraries: a power supply, a drive, a controller
from a maker that ships no macro. Wires attach to its connection points the same way they
attach to symbol pins.

## When to use one

Use a library symbol when one exists: it brings the right drawing, pin names and often a
part. Use a black box when there is none and you do not want to make a library symbol for a
device drawn once or twice. The box draws only the outline and the point names, but for
connections, reports and checks it counts like any other symbol.

## How it is stored

Each box is its own symbol in the project, with the id `box.<page>.<item>` (for example
`box.p1.s1` for item `s1` on page `p1`). Its file is
`vendor/symbols/box.<page>.<item>.json`, and `library.lock` pins it with `source = "project"`.
The symbol's category is `Black box`, and its name is `Black box <tag>` (or just `Black box`
without a tag).

A save removes the file of a box symbol that no page places and the lock no longer pins, so
deleted boxes do not leave files behind.

## Connection points

A connection point has a name, an edge (`left`, `right`, `top` or `bottom`) and an offset in
mm along that edge, measured from its left end (top and bottom edges) or its top end (left and
right edges). A wire leaves each point away from the box.

A box is refused when:

- its width or height is not above zero,
- a point has no name, or two points share a name,
- a point lies off its edge (offset below 0 or past the edge's length),
- two points sit at the same place (for example on a corner shared by two edges).

## Device tag and part

The tag is optional. A box with a tag such as `-A1` becomes that device: it is listed in
`devices.json`, and you set its part and description like for any device, with
`wiringcraft device -A1 --part <id> --description <text>` or in the app's properties panel.
The tag is drawn just above the box's top-left corner.

## Drawing one in the app

1. Pick the Black box tool (toolbar, or the Draw menu).
2. Drag the rectangle on the page, or click one corner and then the opposite corner.
3. Click an edge to add a connection point. A dialog asks for its name and offers the lowest
   number no point has yet. Repeat for each point.
4. Click inside the box or press Enter. The Black box dialog asks for the tag (it can be left
   empty). Put on page places the box and selects it.

Esc cancels at any step. The status bar says what to do next.

To change a placed box, select it. The properties panel shows a Black box section with Width,
Height and a row per connection point (name, edge and mm along it). Add, rename, move or
remove points, then press Apply: the change is one undo step. Renamed points keep their wires.
A wire on a point that was removed or moved keeps a free end where the point was, and the
status bar names it. With the select tool, a selected box also has a sizing handle at each
corner.

The assistant's `black_box` tool draws and edits boxes the same way (see
[assistant.md](assistant.md)).

## Drawing one from the CLI

`wiringcraft box` draws a box and prints its item id. `--at` is the top-left corner and
`--size` the width and height, both in mm. Each `--pin` is `name:side:offset`:

```
$ wiringcraft page add p1 --number 2 --title Power
$ wiringcraft box p1 --at 100,60 --size 40,30 --tag -A1 \
    --pin L1:top:10 --pin N:top:30 --pin 24V:bottom:10 --pin 0V:bottom:30
s1
$ wiringcraft wire p1 s1:24V 110,120 --potential 24V
w1
```

`wiringcraft box-edit` changes a placed box. Without `--pin` the points stay and are pulled
onto the edges of the new size; `--pin` given replaces the whole list; `--rename OLD=NEW`
renames a point and its wires follow:

```
$ wiringcraft box-edit p1 s1 --size 50,30 --rename 0V=GND
$ wiringcraft box-edit p1 s1 --pin L1:top:10 --pin GND:bottom:30
warning: wire w1 on page 2 ended on connection point 24V of s1, which is gone: its end stays where the point was
```

`--clear-pins` takes every point off, and `--rename-pair OLD NEW` renames a point whose name
holds `=`. Sizes and offsets are in the box's own frame, so for a turned or mirrored box the
corner that stays put is the box's own top-left. See `wiringcraft box --help` and
`wiringcraft box-edit --help` for every option.

## In reports and checks

A tagged box shows up like any device. With the box drawn above (before the edits) and
`wiringcraft device -A1 --description "Power supply 24 V"`:

```
$ wiringcraft report devices
Device list
Device  Description        Part  Symbols        Pages
------  -----------------  ----  -------------  -----
-A1     Power supply 24 V        Black box -A1  2

$ wiringcraft report nets
Net list
Net  Pins  Connections
---  ----  -----------
24V  1     -A1:24V
```

The parts list counts it under its part (or as "no part assigned"), and `wiringcraft check
--all` notes its open connection points and a missing part like for any symbol:

```
$ wiringcraft check --all
note    page 2: -A1: pin L1, N, 0V not connected
note    -A1 has no part
0 errors, 0 warnings, 2 notes
```
